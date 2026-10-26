use super::{FileMode, IngestQueueItem};
use crate::DbPool;
use crate::convert::{ConversionPipeline, EbookFormat};
use crate::events::LibraryEvent;
use crate::media_root_naming::{self, NamingContext};
use crate::repositories::{
    audiobook_chapters, audiobook_files, authors, book_files, books, entitlements, genres,
    ingest_queue, media_root_files, media_roots, series,
};
use crate::runtime_config::RuntimeConfigManager;
use crate::types::EntitlementScope;
use nats_jobs::Publisher;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::sync::broadcast;
use tracing::{info, warn};
use uuid::Uuid;

type IngestResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestEntitlements {
    AutoSync,
    Devices(Vec<Uuid>),
}

#[derive(Debug, Clone)]
pub struct IngestOptions {
    pub file_mode: FileMode,
    pub entitlements: IngestEntitlements,
}

impl IngestOptions {
    pub fn auto(file_mode: FileMode) -> Self {
        Self {
            file_mode,
            entitlements: IngestEntitlements::AutoSync,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MediaKind {
    Ebook,
    Audiobook,
}

impl MediaKind {
    fn of(source_format: &str) -> Self {
        match source_format.to_lowercase().as_str() {
            "m4b" | "m4a" | "mp3" | "flac" | "ogg" | "opus" => Self::Audiobook,
            _ => Self::Ebook,
        }
    }
}

struct Placement {
    output_dir: PathBuf,
    kepub_name: Option<String>,
    media_root: Option<media_roots::EnabledMediaRoot>,
}

struct Ingest<'a> {
    pool: &'a DbPool,
    storage_root: &'a str,
    nats: &'a Publisher,
    runtime_config: &'a RuntimeConfigManager,
    item: &'a IngestQueueItem,
    book_id: Uuid,
    is_update: bool,
    file_mode: FileMode,
    source_type: String,
    source_provenance: serde_json::Value,
}

pub async fn process_queue_item(
    pool: &DbPool,
    storage_root: &str,
    library_events: &broadcast::Sender<LibraryEvent>,
    nats_publisher: &Publisher,
    runtime_config: &RuntimeConfigManager,
    queue_id: Uuid,
    options: IngestOptions,
) -> IngestResult<Uuid> {
    let item = ingest_queue::get_by_id(pool, queue_id)
        .await?
        .ok_or("Queue item not found")?;
    let (source_type, source_provenance) = source_provenance(&item);
    let ingest = Ingest {
        pool,
        storage_root,
        nats: nats_publisher,
        runtime_config,
        item: &item,
        book_id: item.update_target_book_id.unwrap_or(queue_id),
        is_update: item.update_target_book_id.is_some(),
        file_mode: options.file_mode,
        source_type,
        source_provenance,
    };

    let kind = MediaKind::of(&item.source_format);
    if let Err(e) = ingest.store(kind).await {
        let _ = ingest_queue::set_error(pool, queue_id, &e.to_string()).await;
        return Err(e);
    }

    let book_id = ingest.book_id;
    ingest.store_cover(queue_id).await;
    ingest.entitle(kind, &options.entitlements).await;
    let _ = ingest_queue::approve(pool, queue_id, book_id).await;
    let _ = library_events.send(LibraryEvent::library_updated(Some(book_id)));
    crate::wanted::on_book_acquired(runtime_config, library_events, book_id).await;
    ingest.enqueue_followups(kind).await;

    info!(
        queue_id = %queue_id,
        book_id = %book_id,
        update = ingest.is_update,
        audiobook = kind == MediaKind::Audiobook,
        "Ingested queue item"
    );
    Ok(book_id)
}

fn source_provenance(item: &IngestQueueItem) -> (String, serde_json::Value) {
    let meta = item.source_metadata.as_ref();
    let source_type = meta
        .and_then(|m| m.get("source_type"))
        .and_then(|v| v.as_str())
        .unwrap_or("watch_directory")
        .to_string();
    let provenance = meta
        .and_then(|m| m.get("source_provenance"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    (source_type, provenance)
}

impl Ingest<'_> {
    fn title(&self) -> String {
        self.item
            .title
            .clone()
            .unwrap_or_else(|| self.item.file_name.clone())
    }

    async fn store(&self, kind: MediaKind) -> IngestResult<()> {
        if self.is_update {
            books::set_want_status(self.pool, self.book_id, "none").await?;
        } else {
            self.apply_pending_metadata().await;
            if kind == MediaKind::Audiobook {
                books::update(
                    self.pool,
                    self.book_id,
                    books::BookUpdate {
                        want_format: Some("audiobook".to_string()),
                        ..Default::default()
                    },
                )
                .await?;
            }
        }

        let placement = self.placement().await;
        fs::create_dir_all(&placement.output_dir).await?;
        match kind {
            MediaKind::Ebook => self.store_ebook(&placement).await,
            MediaKind::Audiobook => self.store_audiobook(&placement).await,
        }
    }

    async fn apply_pending_metadata(&self) {
        let pool = self.pool;
        let book_id = self.book_id;

        if let Some(metadata) = &self.item.source_metadata {
            let mut identifiers = serde_json::Map::new();
            for key in ["isbn13", "isbn10"] {
                if let Some(value) = metadata.get(key).and_then(|v| v.as_str()) {
                    identifiers.insert(key.to_string(), serde_json::json!(value));
                }
            }
            if let Some(external_id) = metadata.get("external_id").and_then(|v| v.as_str())
                && let Some(provider) = metadata.get("provider").and_then(|v| v.as_str())
            {
                identifiers.insert(provider.to_string(), serde_json::json!(external_id));
            }
            if !identifiers.is_empty() {
                let _ = books::update_identifiers(
                    pool,
                    book_id,
                    serde_json::Value::Object(identifiers),
                )
                .await;
            }

            if let Some(subjects) = metadata.get("subjects").and_then(|v| v.as_array()) {
                let subjects: Vec<String> = subjects
                    .iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect();
                if let Err(e) = genres::set_book_genres(pool, book_id, &subjects).await {
                    warn!(error = %e, "Failed to set book genres");
                }
            }
        }

        for author in self
            .item
            .pending_authors
            .as_ref()
            .and_then(|p| p.as_array())
            .into_iter()
            .flatten()
        {
            let Some(name) = author.get("name").and_then(|n| n.as_str()) else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let hardcover_id = author.get("hardcover_id").and_then(|v| v.as_str());
            match authors::get_or_create(pool, name, hardcover_id).await {
                Ok(author_id) => {
                    let _ = authors::link_book(pool, book_id, author_id).await;
                }
                Err(e) => warn!(author = %name, error = %e, "Failed to create pending author"),
            }
        }

        let has_series: bool = sqlx::query_scalar!(
            r#"SELECT EXISTS(SELECT 1 FROM book_series WHERE book_id = $1) as "has_series!""#,
            book_id,
        )
        .fetch_one(pool)
        .await
        .unwrap_or(false);
        if has_series {
            return;
        }
        let Some(pending) = &self.item.pending_series else {
            return;
        };
        let Some(name) = pending
            .get("name")
            .and_then(|n| n.as_str())
            .map(str::trim)
            .filter(|n| !n.is_empty())
        else {
            return;
        };
        let series_identifiers = pending
            .get("hardcover_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|hc_id| serde_json::json!({"hardcover": hc_id}));
        let author_ids = authors::get_book_author_ids(pool, book_id)
            .await
            .unwrap_or_default();
        match series::get_or_create_with_author_overlap(pool, name, &author_ids, series_identifiers)
            .await
        {
            Ok(Some(series_id)) => {
                let _ = books::set_series(
                    pool,
                    book_id,
                    Some(series_id),
                    self.item.pending_series_index,
                )
                .await;
            }
            Ok(None) => warn!(series = %name, "Series creation returned None"),
            Err(e) => warn!(series = %name, error = %e, "Failed to create pending series"),
        }
    }

    async fn media_root(&self) -> Option<media_roots::EnabledMediaRoot> {
        let assigned: Option<Uuid> = sqlx::query_scalar!(
            "SELECT media_root_id FROM books WHERE id = $1",
            self.book_id
        )
        .fetch_optional(self.pool)
        .await
        .ok()
        .flatten()
        .flatten();
        if let Some(root_id) = assigned {
            return media_roots::get_enabled_by_id(self.pool, root_id)
                .await
                .ok()
                .flatten();
        }
        let default = media_roots::get_default_for_type(self.pool, "book")
            .await
            .ok()
            .flatten()?;
        let _ = books::set_media_root_id(self.pool, self.book_id, Some(default.id)).await;
        Some(default)
    }

    async fn placement(&self) -> Placement {
        let Some(root) = self.media_root().await else {
            return Placement {
                output_dir: PathBuf::from(self.storage_root)
                    .join("books")
                    .join(self.book_id.to_string()),
                kepub_name: None,
                media_root: None,
            };
        };

        let row = sqlx::query!(
            r#"
            SELECT
                b.title,
                (SELECT a.name FROM book_authors ba JOIN authors a ON a.id = ba.author_id
                 WHERE ba.book_id = b.id ORDER BY ba.author_id LIMIT 1) AS author,
                s.name AS "series_name?",
                bs.series_index AS "series_index?"
            FROM books b
            LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
            LEFT JOIN series s ON s.id = bs.series_id
            WHERE b.id = $1
            "#,
            self.book_id,
        )
        .fetch_optional(self.pool)
        .await
        .ok()
        .flatten();
        let ctx = NamingContext {
            title: Some(
                row.as_ref()
                    .map_or_else(|| self.title(), |r| r.title.clone()),
            ),
            author: row.as_ref().and_then(|r| r.author.clone()),
            series: row.as_ref().and_then(|r| r.series_name.clone()),
            series_index: row.as_ref().and_then(|r| r.series_index).map(|i| i as u32),
            ext: Some("kepub.epub".to_string()),
            ..Default::default()
        };

        let template = root
            .naming_template
            .as_deref()
            .unwrap_or_else(|| media_root_naming::default_book_template());
        let fresh = media_root_naming::compute_target_path(&root.path, template, &ctx);
        // Reuse the on-disk author directory so a DB name that differs from it
        // does not split one author's books across two directories.
        let full_path = if let Some(existing) = media_root_naming::find_existing_file_for_entity(
            self.pool,
            root.id,
            "book",
            self.book_id,
        )
        .await
            && let Some(author_dir) =
                media_root_naming::extract_dir_from_existing(&root.path, &existing, 1)
        {
            media_root_naming::reuse_existing_dir(&root.path, &author_dir, &fresh, 1)
        } else {
            fresh
        };
        Placement {
            output_dir: full_path.parent().unwrap_or(&full_path).to_path_buf(),
            kepub_name: full_path
                .file_name()
                .and_then(|s| s.to_str())
                .map(String::from),
            media_root: Some(root),
        }
    }

    async fn store_ebook(&self, placement: &Placement) -> IngestResult<()> {
        let item = self.item;
        let source_path = Path::new(&item.file_path);
        let output_dir = &placement.output_dir;

        let mut kepub_path = if self.file_mode == FileMode::Link {
            source_path.to_path_buf()
        } else {
            ConversionPipeline::new()
                .convert_to_kepub(source_path, output_dir)
                .await
                .map_err(|e| format!("Conversion failed: {e}"))?
        };

        if let Some(name) = &placement.kepub_name {
            let target = output_dir.join(name);
            if kepub_path != target {
                match fs::rename(&kepub_path, &target).await {
                    Ok(()) => kepub_path = target,
                    Err(e) => warn!(error = %e, "Failed to rename KEPUB to template name"),
                }
            }
        }
        self.upsert_ebook_file("KEPUB", &kepub_path).await;

        let original_format = EbookFormat::from_path(source_path);
        if !original_format.is_kepub() && original_format != EbookFormat::Unknown {
            let original_dest = if self.file_mode == FileMode::Link {
                source_path.to_path_buf()
            } else {
                let source_name = || {
                    source_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                };
                let name = placement
                    .kepub_name
                    .as_deref()
                    .and_then(|n| n.strip_suffix(".kepub.epub"))
                    .map(|stem| format!("{stem}.{}", item.source_format.to_lowercase()))
                    .unwrap_or_else(source_name);
                let dest = output_dir.join(name);
                if dest != kepub_path
                    && let Err(e) = fs::copy(source_path, &dest).await
                {
                    warn!(error = %e, "Failed to copy original format file");
                }
                dest
            };
            self.upsert_ebook_file(&item.source_format.to_uppercase(), &original_dest)
                .await;
        }

        self.register_in_media_root(placement, &kepub_path).await;
        crate::jobs::enqueue_metadata_write(self.pool, self.nats, self.book_id, &self.title())
            .await;
        Ok(())
    }

    async fn upsert_ebook_file(&self, format: &str, path: &Path) {
        let size_bytes = file_size(path).await.unwrap_or(self.item.file_size);
        if let Err(e) = book_files::upsert_full(
            self.pool,
            book_files::BookFileFullInput {
                book_id: self.book_id,
                format: format.to_string(),
                size_bytes,
                file_path: path.to_string_lossy().to_string(),
                source_path: Some(self.item.file_path.clone()),
                source_mtime: self.item.source_mtime,
                file_hash: self.item.file_hash.clone(),
                source_type: Some(self.source_type.clone()),
                source_metadata: Some(self.source_provenance.clone()),
            },
        )
        .await
        {
            warn!(error = %e, format, "Failed to record ebook file");
        }
    }

    async fn store_audiobook(&self, placement: &Placement) -> IngestResult<()> {
        let item = self.item;
        let source_path = Path::new(&item.file_path);
        let dest_path = if self.file_mode == FileMode::Link {
            source_path.to_path_buf()
        } else {
            let dest = placement
                .output_dir
                .join(source_path.file_name().unwrap_or_default());
            fs::copy(source_path, &dest)
                .await
                .map_err(|e| format!("Failed to copy audiobook file: {e}"))?;
            dest
        };
        let size = file_size(&dest_path).await.unwrap_or(item.file_size);

        let book_file_id = audiobook_files::upsert_by_path(
            self.pool,
            audiobook_files::AudiobookFileInput {
                book_id: self.book_id,
                format: item.source_format.to_uppercase(),
                file_path: dest_path.to_string_lossy().to_string(),
                file_size: size,
                file_hash: item.file_hash.clone(),
                source_path: Some(item.file_path.clone()),
                source_mtime: item.source_mtime,
                source_type: Some(self.source_type.clone()),
                source_metadata: Some(self.source_provenance.clone()),
                ..Default::default()
            },
        )
        .await?;
        self.register_in_media_root(placement, &dest_path).await;

        // Without a Xing/Info header ffprobe and browsers estimate MP3 duration
        // from bitrate, which is wrong for old rips and breaks read-along seek.
        if let Err(e) = crate::audio::repair::ensure_xing_header(&dest_path).await {
            warn!(error = %e, path = %dest_path.display(), "Failed to repair MP3 header");
        }
        match crate::audio::probe::probe_audio(&dest_path).await {
            Ok(probe) => {
                let update = books::BookUpdate {
                    duration_seconds: probe.duration_seconds.map(|d| d as i32),
                    narrator: probe.narrator,
                    is_abridged: probe.is_abridged,
                    ..Default::default()
                };
                let _ = books::update(self.pool, self.book_id, update).await;

                let chapters =
                    crate::audio::chapters::resolve_chapters(&dest_path, None, None).await;
                if !chapters.is_empty()
                    && let Err(e) =
                        audiobook_chapters::store_chapters(self.pool, book_file_id, &chapters).await
                {
                    warn!(error = %e, "Failed to store audiobook chapters");
                }
            }
            Err(e) => {
                warn!(error = %e, path = %dest_path.display(), "Failed to probe audiobook, continuing without metadata");
            }
        }
        Ok(())
    }

    async fn register_in_media_root(&self, placement: &Placement, path: &Path) {
        let Some(root) = &placement.media_root else {
            return;
        };
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if let Err(e) = media_root_files::upsert_added(
            self.pool,
            root.id,
            &path.to_string_lossy(),
            &name,
            file_size(path).await,
            "book",
            self.book_id,
        )
        .await
        {
            warn!(error = %e, "Failed to register ingested file in media_root_files");
        }
    }

    async fn store_cover(&self, queue_id: Uuid) {
        let Ok(Some(cover)) = ingest_queue::get_cover(self.pool, queue_id).await else {
            return;
        };
        let covers_dir = PathBuf::from(self.storage_root).join("covers");
        if fs::create_dir_all(&covers_dir).await.is_ok()
            && let Err(e) =
                fs::write(covers_dir.join(format!("{}.jpg", self.book_id)), &cover).await
        {
            warn!(error = %e, "Failed to write cover");
        }
    }

    async fn entitle(&self, kind: MediaKind, requested: &IngestEntitlements) {
        let device_ids = match requested {
            IngestEntitlements::AutoSync => {
                crate::auto_sync::create_auto_sync_entitlements(self.pool, self.nats, self.book_id)
                    .await;
                return;
            }
            IngestEntitlements::Devices(ids) => ids,
        };
        for &device_id in device_ids {
            if let Err(e) = entitlements::create_or_restore(
                self.pool,
                self.book_id,
                EntitlementScope::Device,
                Some(device_id),
            )
            .await
            {
                warn!(device_id = %device_id, error = %e, "Failed to create device entitlement");
            }
            if kind == MediaKind::Ebook {
                crate::routes::admin::trigger_kindle_delivery_if_needed(
                    self.pool,
                    self.nats,
                    self.book_id,
                    device_id,
                )
                .await;
            }
        }
    }

    async fn enqueue_followups(&self, kind: MediaKind) {
        let title = self.title();
        let transcribe = self.runtime_config.get().await.transcript_mode == "auto";
        match kind {
            MediaKind::Audiobook => {
                crate::jobs::enqueue_audiobook_probe(self.pool, self.nats, self.book_id, &title)
                    .await;
                if transcribe {
                    crate::jobs::enqueue_audiobook_transcribe(
                        self.pool,
                        self.nats,
                        self.book_id,
                        &title,
                    )
                    .await;
                }
            }
            MediaKind::Ebook => {
                if transcribe {
                    crate::jobs::enqueue_ebook_extract(self.pool, self.nats, self.book_id, &title)
                        .await;
                }
            }
        }
    }
}

async fn file_size(path: &Path) -> Option<i64> {
    fs::metadata(path).await.ok().map(|m| m.len() as i64)
}
