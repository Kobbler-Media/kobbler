//! Background watcher service.
//!
//! Monitors media root directories for new media files and enqueues
//! `media_root_scan` jobs. Also provides `process_queue_item` for ingest
//! queue approval. Real-time filesystem events are handled via notify.
//! Periodic polling is handled by the NATS scheduler (`media_root_scan_check` jobs).

use super::FileMode;
use crate::DbPool;
use crate::convert::{ConversionPipeline, EbookFormat};
use crate::events::LibraryEvent;
use crate::media_root_naming::{self, NamingContext};
use crate::repositories::{
    audiobook_chapters, audiobook_files, authors, book_files, books, ingest_queue,
    media_root_files, media_roots,
};
use nats_jobs::Publisher;
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebouncedEventKind, Debouncer, new_debouncer};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::fs;
use tokio::sync::{RwLock, broadcast, mpsc};
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

/// Configuration for the watcher service.
#[derive(Debug, Clone)]
pub struct WatcherConfig {
    /// Debounce delay for filesystem events.
    pub debounce_delay: Duration,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            debounce_delay: Duration::from_secs(2),
        }
    }
}

/// The background watcher service.
pub struct WatcherService {
    pool: DbPool,
    config: WatcherConfig,
    /// Root directory for book storage (used by process_queue_item).
    #[allow(dead_code)]
    storage_root: String,
    /// Channel to notify library updates (used by process_queue_item).
    #[allow(dead_code)]
    library_events: broadcast::Sender<LibraryEvent>,
    /// Active watchers keyed by media root ID.
    media_root_watchers: Arc<RwLock<HashMap<Uuid, ActiveWatcher>>>,
    /// Shutdown signal.
    shutdown_tx: Option<mpsc::Sender<()>>,
    /// NATS publisher for job enqueueing.
    nats_publisher: Publisher,
}

struct ActiveWatcher {
    #[allow(dead_code)]
    directory_id: Uuid,
    #[allow(dead_code)]
    path: PathBuf,
    /// The debouncer must be kept alive for real-time watching to work.
    /// When this is Some, real-time filesystem events are active.
    /// When None, the directory falls back to polling.
    #[allow(dead_code)]
    debouncer: Option<Debouncer<RecommendedWatcher>>,
}

impl WatcherService {
    /// Create a new watcher service.
    pub fn new(
        pool: DbPool,
        config: WatcherConfig,
        storage_root: String,
        library_events: broadcast::Sender<LibraryEvent>,
        nats_publisher: Publisher,
    ) -> Self {
        Self {
            pool,
            config,
            storage_root,
            library_events,
            media_root_watchers: Arc::new(RwLock::new(HashMap::new())),
            shutdown_tx: None,
            nats_publisher,
        }
    }

    /// Start the watcher service.
    ///
    /// This sets up real-time filesystem watchers for enabled directories.
    /// Periodic scanning is handled by the NATS scheduler via `directory_scan_check`.
    pub async fn start(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Starting watcher service");

        let (shutdown_tx, _shutdown_rx) = mpsc::channel::<()>(1);
        self.shutdown_tx = Some(shutdown_tx);

        // Load enabled watch directories and set up real-time watchers
        self.refresh_watchers().await?;

        Ok(())
    }

    /// Stop the watcher service.
    pub async fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }
        self.media_root_watchers.write().await.clear();
        info!("Watcher service stopped");
    }

    /// Refresh watchers from database.
    pub async fn refresh_watchers(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.refresh_media_root_watchers().await?;
        Ok(())
    }

    /// Refresh media root watchers from database.
    async fn refresh_media_root_watchers(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let roots = media_roots::get_all_enabled(&self.pool).await?;

        let mut watchers = self.media_root_watchers.write().await;

        // Remove watchers for media roots that are no longer enabled
        let current_ids: Vec<Uuid> = roots.iter().map(|r| r.id).collect();
        watchers.retain(|id, _| current_ids.contains(id));

        // Add watchers for new media roots
        for root in roots {
            if let Entry::Vacant(entry) = watchers.entry(root.id) {
                let path = PathBuf::from(&root.path);

                if !path.exists() {
                    warn!("Media root directory does not exist: {}", root.path);
                    continue;
                }

                let debouncer = self.setup_media_root_notify_watcher(&root, &path).await;

                let has_realtime = debouncer.is_some();
                entry.insert(ActiveWatcher {
                    directory_id: root.id,
                    path,
                    debouncer,
                });

                if has_realtime {
                    info!(
                        "Watching media root with real-time events: {} ({})",
                        root.name, root.path
                    );
                } else {
                    info!(
                        "Watching media root with polling fallback: {} ({})",
                        root.name, root.path
                    );
                }
            }
        }

        Ok(())
    }

    /// Set up a notify-based real-time watcher for a media root directory.
    /// Returns the debouncer which must be kept alive for events to be received.
    /// Returns None for network filesystems (NFS/SMB) where inotify doesn't work.
    async fn setup_media_root_notify_watcher(
        &self,
        root: &media_roots::EnabledMediaRoot,
        path: &Path,
    ) -> Option<Debouncer<RecommendedWatcher>> {
        if is_network_mount(path) {
            info!(
                "Media root {} ({}) is on a network filesystem, skipping inotify (polling only)",
                root.name,
                path.display()
            );
            return None;
        }
        let pool = self.pool.clone();
        let nats = self.nats_publisher.clone();
        let media_root_id = root.id;
        let root_name = root.name.clone();
        let root_path = path.to_path_buf();
        let media_type = root.media_type.clone();
        let debounce_delay = self.config.debounce_delay;

        // Create event channel for file paths
        let (tx, mut rx) = mpsc::channel::<PathBuf>(100);

        // Spawn handler task that coalesces file events into batched scan jobs.
        // Instead of one scan job per file, we collect events over a short window:
        // - 1 file → single-file scan
        // - 2+ files → full directory scan (avoids thousands of individual jobs)
        tokio::spawn(async move {
            loop {
                // Wait for the first event
                let first = match rx.recv().await {
                    Some(p) => p,
                    None => break, // Channel closed
                };

                // Collect additional events over a short coalescing window
                let mut batch = vec![first];
                let coalesce_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
                loop {
                    match tokio::time::timeout_at(coalesce_deadline, rx.recv()).await {
                        Ok(Some(path)) => batch.push(path),
                        Ok(None) => return, // Channel closed
                        Err(_) => break,    // Timeout — process batch
                    }
                }

                // Filter to media files only
                let media_files: Vec<PathBuf> = batch
                    .into_iter()
                    .filter(|p| {
                        let is_media =
                            crate::jobs::handlers::media_root_scan::is_media_file(p, &media_type);
                        if !is_media {
                            trace!("Skipping non-media file event: {}", p.display());
                        }
                        is_media
                    })
                    .collect();

                if media_files.is_empty() {
                    continue;
                }

                // Skip if there's already an active scan for this media root
                let media_root_id_str = media_root_id.to_string();
                match crate::jobs::has_active_job(
                    &pool,
                    "media_root_scan",
                    "media_root_id",
                    &media_root_id_str,
                )
                .await
                {
                    Ok(true) => {
                        debug!(
                            "Skipping watcher enqueue for {} - scan already active for media root {}",
                            root_name, media_root_id
                        );
                        continue;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        warn!(
                            "Failed to check for active scan job for {}: {}",
                            root_name, e
                        );
                        // Continue to enqueue on check failure
                    }
                }

                // Single file → single-file scan; multiple → full directory scan
                let (payload, description) = if media_files.len() == 1 {
                    let file_path = &media_files[0];
                    let relative = file_path
                        .strip_prefix(&root_path)
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|_| file_path.to_string_lossy().to_string());
                    let file_name = file_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown file")
                        .to_string();
                    (
                        serde_json::json!({
                            "media_root_id": media_root_id,
                            "single_file": relative,
                        }),
                        format!("Processing '{}' in '{}'", file_name, root_name),
                    )
                } else {
                    // Full directory scan supersedes any pending single-file scans
                    if let Err(e) = crate::jobs::cancel_pending_jobs_with_field(
                        &pool,
                        "media_root_scan",
                        "media_root_id",
                        &media_root_id_str,
                        "single_file",
                    )
                    .await
                    {
                        warn!(
                            "Failed to cancel pending single-file scans for {}: {}",
                            root_name, e
                        );
                    }

                    let count = media_files.len();
                    (
                        serde_json::json!({
                            "media_root_id": media_root_id,
                        }),
                        format!("Scanning '{}' ({} file events coalesced)", root_name, count),
                    )
                };

                let display =
                    crate::jobs::fetch_job_display(&pool, "media_root", media_root_id).await;
                let context =
                    nats_jobs::JobContext::for_entity("media_root", media_root_id, description)
                        .with_display_opt(display);

                match crate::jobs::enqueue_routine(
                    &pool,
                    &nats,
                    "media_root_scan",
                    payload,
                    context,
                )
                .await
                {
                    Ok(job_id) => {
                        debug!(
                            "Enqueued media_root_scan job {} for {} file(s) in {}",
                            job_id,
                            media_files.len(),
                            root_name,
                        );
                    }
                    Err(e) => {
                        error!("Failed to enqueue media_root_scan for {}: {}", root_name, e);
                    }
                }
            }
        });

        // Create debounced watcher - media roots use recursive mode
        let tx_clone = tx.clone();
        match new_debouncer(
            debounce_delay,
            move |res: Result<Vec<notify_debouncer_mini::DebouncedEvent>, notify::Error>| {
                if let Ok(events) = res {
                    for event in events {
                        if event.kind == DebouncedEventKind::Any {
                            let tx = tx_clone.clone();
                            let _ = tx.try_send(event.path);
                        }
                    }
                }
            },
        ) {
            Ok(mut debouncer) => {
                if let Err(e) = debouncer.watcher().watch(path, RecursiveMode::Recursive) {
                    warn!(
                        "Failed to set up real-time watcher for media root {}: {}. Using polling fallback.",
                        path.display(),
                        e
                    );
                    return None;
                }

                info!("Real-time watcher active for media root {}", path.display());
                Some(debouncer)
            }
            Err(e) => {
                warn!(
                    "Failed to create debouncer for media root {}: {}. Using polling fallback.",
                    path.display(),
                    e
                );
                None
            }
        }
    }
}

/// Check if a path resides on a network filesystem (NFS or SMB/CIFS).
///
/// inotify only works on local Linux filesystems. Network mounts (NFS, SMB)
/// silently accept watch registrations but never deliver events. This function
/// uses `statfs(2)` to detect the filesystem type so we can skip inotify setup
/// and rely on the polling fallback instead.
fn is_network_mount(path: &Path) -> bool {
    use nix::sys::statfs::{FsType, NFS_SUPER_MAGIC, SMB_SUPER_MAGIC, statfs};
    // SMB2/3 uses a different magic number (0xFE534D42) not exported by nix
    const SMB2_SUPER_MAGIC: FsType = FsType(0xFE534D42);
    match statfs(path) {
        Ok(stat) => {
            let fstype = stat.filesystem_type();
            fstype == NFS_SUPER_MAGIC || fstype == SMB_SUPER_MAGIC || fstype == SMB2_SUPER_MAGIC
        }
        Err(_) => false, // conservative: attempt inotify if detection fails
    }
}

/// Process a queue item: convert the file, create book record, store file, create entitlement.
/// This is the shared logic used by both manual approval and auto-ingest.
pub async fn process_queue_item(
    pool: &DbPool,
    storage_root: &str,
    library_events: &broadcast::Sender<LibraryEvent>,
    nats_publisher: &Publisher,
    runtime_config: &crate::runtime_config::RuntimeConfigManager,
    queue_id: Uuid,
    file_mode: FileMode,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    // Get the queue item
    let item: crate::watcher::IngestQueueItem = ingest_queue::get_by_id(pool, queue_id)
        .await?
        .ok_or("Queue item not found")?;

    // Determine if this is an update to an existing book or a new book
    let is_update = item.update_target_book_id.is_some();
    let book_id = item.update_target_book_id.unwrap_or_else(Uuid::now_v7);

    // Resolve media root for output directory
    let source_path = Path::new(&item.file_path);
    let entity_root_id: Option<Uuid> = if is_update {
        sqlx::query_scalar!("SELECT media_root_id FROM books WHERE id = $1", book_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .flatten()
    } else {
        None
    };
    let media_root = if let Some(root_id) = entity_root_id {
        media_roots::get_enabled_by_id(pool, root_id)
            .await
            .ok()
            .flatten()
    } else {
        let default = media_roots::get_default_for_type(pool, "book")
            .await
            .ok()
            .flatten();
        // Set media_root_id on the book if resolved from default (for updates)
        if let Some(ref root) = default
            && is_update
        {
            let _ = sqlx::query!(
                "UPDATE books SET media_root_id = $1 WHERE id = $2",
                root.id,
                book_id,
            )
            .execute(pool)
            .await;
        }
        default
    };

    let (output_dir, target_kepub_name) = if let Some(ref root) = media_root {
        // For wanted books (update_target_book_id set), query actual DB metadata.
        // For new books, fall back to ingest queue item metadata.
        let (title, author, series, series_index) = if is_update {
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
                book_id,
            )
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
            if let Some(row) = row {
                (
                    row.title,
                    row.author,
                    row.series_name,
                    row.series_index.map(|i| i as u32),
                )
            } else {
                (
                    item.title.clone().unwrap_or_else(|| item.file_name.clone()),
                    item.merged_author_names
                        .as_ref()
                        .and_then(|a| a.first())
                        .cloned(),
                    None,
                    None,
                )
            }
        } else {
            (
                item.title.clone().unwrap_or_else(|| item.file_name.clone()),
                item.merged_author_names
                    .as_ref()
                    .and_then(|a| a.first())
                    .cloned(),
                item.series_name.clone().or_else(|| {
                    item.pending_series
                        .as_ref()
                        .and_then(|ps| ps.get("name"))
                        .and_then(|v| v.as_str())
                        .map(String::from)
                }),
                item.series_index
                    .or(item.pending_series_index)
                    .map(|i| i as u32),
            )
        };

        let template = root
            .naming_template
            .as_deref()
            .unwrap_or_else(|| media_root_naming::default_book_template());
        let ctx = NamingContext {
            title: Some(title),
            author,
            series,
            series_index,
            ext: Some("kepub.epub".to_string()),
            ..Default::default()
        };
        let fresh = media_root_naming::compute_target_path(&root.path, template, &ctx);
        // Prefer existing author directory to avoid duplicates when DB name differs from on-disk
        let full_path = if let Some(existing) =
            media_root_naming::find_existing_file_for_entity(pool, root.id, "book", book_id).await
            && let Some(author_dir) =
                media_root_naming::extract_dir_from_existing(&root.path, &existing, 1)
        {
            media_root_naming::reuse_existing_dir(&root.path, &author_dir, &fresh, 1)
        } else {
            fresh
        };
        let dir = full_path.parent().unwrap_or(&full_path).to_path_buf();
        let name = full_path
            .file_name()
            .and_then(|s| s.to_str())
            .map(String::from);
        (dir, name)
    } else {
        (
            PathBuf::from(storage_root)
                .join("books")
                .join(book_id.to_string()),
            None,
        )
    };

    // Create output directory
    fs::create_dir_all(&output_dir).await?;

    // Check if this is an audiobook file (audio format, not ebook)
    let is_audiobook = matches!(
        item.source_format.to_lowercase().as_str(),
        "m4b" | "m4a" | "mp3" | "flac" | "ogg" | "opus"
    );

    if is_audiobook {
        return process_audiobook_ingest(
            pool,
            storage_root,
            library_events,
            nats_publisher,
            runtime_config,
            &item,
            queue_id,
            book_id,
            is_update,
            &output_dir,
            source_path,
            file_mode,
            &media_root,
        )
        .await;
    }

    // Convert the file (or reference original if link mode)
    let mut kepub_path = if file_mode == FileMode::Link {
        // Link mode: reference original file path
        source_path.to_path_buf()
    } else {
        // Copy mode: convert to KEPUB
        let pipeline = ConversionPipeline::new();
        match pipeline.convert_to_kepub(source_path, &output_dir).await {
            Ok(path) => path,
            Err(e) => {
                let error_msg = format!("Conversion failed: {}", e);
                let _ = ingest_queue::set_error(pool, queue_id, &error_msg).await;
                return Err(error_msg.into());
            }
        }
    };

    // Rename KEPUB to template-derived name (strips UUID download prefix from filename)
    if let Some(ref name) = target_kepub_name {
        let target = output_dir.join(name);
        if kepub_path != target {
            match fs::rename(&kepub_path, &target).await {
                Ok(()) => kepub_path = target,
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to rename KEPUB to template name");
                }
            }
        }
    }

    // Create or update book record
    if is_update {
        // For updates, keep the existing book metadata and just update the file.
        let _ = sqlx::query!(
            "UPDATE books SET want_status = 'none', updated_at = NOW() WHERE id = $1",
            book_id,
        )
        .execute(pool)
        .await;
        info!(book_id = %book_id, "Updating existing book file");
    } else {
        // Create new book record using repository
        let title = item.title.clone().unwrap_or_else(|| item.file_name.clone());

        let input = books::BookInput {
            id: Some(book_id),
            title,
            description: item.description.clone(),
            ..Default::default()
        };

        books::upsert_library_book(pool, input).await?;

        // Set media_root_id on new book if resolved from default
        if let Some(ref root) = media_root {
            let _ = sqlx::query!(
                "UPDATE books SET media_root_id = $1 WHERE id = $2",
                root.id,
                book_id,
            )
            .execute(pool)
            .await;
        }

        // Add authors — prefer linked authors (from provider), fall back to
        // merged (which includes calibre-extracted pending_authors).
        // When provider authors already exist, skip pending_authors to avoid
        // linking mangled names from bad epub metadata.
        let has_linked_authors = item
            .linked_authors
            .as_ref()
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty());

        if !has_linked_authors && let Some(ref merged_authors) = item.merged_author_names {
            for author_name in merged_authors {
                let author_name = author_name.trim();
                if author_name.is_empty() {
                    continue;
                }

                // Get or create author using repository (uses normalized_name for dedup)
                if let Ok(author_id) = authors::get_or_create_by_name(pool, author_name).await {
                    let _ = authors::link_book(pool, book_id, author_id).await;
                }
            }
        }
    }

    // Store the file record with source metadata for deduplication and provenance
    let file_size = fs::metadata(&kepub_path)
        .await
        .map(|m| m.len() as i64)
        .unwrap_or(item.file_size);

    // Extract source provenance from source_metadata (set by Kobload ingest)
    let (source_type, source_provenance): (String, serde_json::Value) = item
        .source_metadata
        .as_ref()
        .map(|meta| {
            let st = meta
                .get("source_type")
                .and_then(|v| v.as_str())
                .unwrap_or("watch_directory")
                .to_string();
            let sm = meta
                .get("source_provenance")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            (st, sm)
        })
        .unwrap_or_else(|| {
            // Default for watch directory scans
            ("watch_directory".to_string(), serde_json::json!({}))
        });

    let _ = book_files::upsert_full(
        pool,
        book_files::BookFileFullInput {
            book_id,
            format: "KEPUB".to_string(),
            size_bytes: file_size,
            file_path: kepub_path.to_string_lossy().to_string(),
            source_path: Some(item.file_path.clone()),
            source_mtime: item.source_mtime,
            file_hash: item.file_hash.clone(),
            source_type: Some(source_type.clone()),
            source_metadata: Some(source_provenance.clone()),
        },
    )
    .await;

    // Also store the original format if it differs from KEPUB.
    // This preserves both the KEPUB (for Kobo sync) and the original (for other readers).
    let original_format = EbookFormat::from_path(source_path);
    if !original_format.is_kepub() && original_format != EbookFormat::Unknown {
        let format_name = item.source_format.to_uppercase();
        let original_dest = if file_mode == FileMode::Link {
            source_path.to_path_buf()
        } else {
            // Copy original file into the book directory with template-derived name
            let dest = if let Some(ref kepub_name) = target_kepub_name {
                // Derive name from KEPUB name: "Title.kepub.epub" → "Title.epub"
                let original_name = kepub_name
                    .strip_suffix(".kepub.epub")
                    .map(|stem| format!("{}.{}", stem, item.source_format.to_lowercase()))
                    .unwrap_or_else(|| {
                        source_path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string()
                    });
                output_dir.join(original_name)
            } else {
                output_dir.join(source_path.file_name().unwrap_or_default())
            };
            if dest != kepub_path
                && let Err(e) = fs::copy(source_path, &dest).await
            {
                tracing::warn!(error = %e, "Failed to copy original format file");
            }
            dest
        };

        let original_size = fs::metadata(&original_dest)
            .await
            .map(|m| m.len() as i64)
            .unwrap_or(item.file_size);

        let _ = book_files::upsert_full(
            pool,
            book_files::BookFileFullInput {
                book_id,
                format: format_name,
                size_bytes: original_size,
                file_path: original_dest.to_string_lossy().to_string(),
                source_path: Some(item.file_path.clone()),
                source_mtime: item.source_mtime,
                file_hash: item.file_hash.clone(),
                source_type: Some(source_type),
                source_metadata: Some(source_provenance),
            },
        )
        .await;
    }

    // Register files in media_root_files so they appear as 'added' immediately
    if let Some(ref root) = media_root {
        let kepub_name = kepub_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let kepub_size = fs::metadata(&kepub_path).await.ok().map(|m| m.len() as i64);
        if let Err(e) = media_root_files::upsert_added(
            pool,
            root.id,
            &kepub_path.to_string_lossy(),
            &kepub_name,
            kepub_size,
            "book",
            book_id,
        )
        .await
        {
            tracing::warn!(error = %e, "Failed to register KEPUB in media_root_files");
        }
    }

    // Enqueue metadata_write to embed DB metadata into the file
    {
        let title = item.title.as_deref().unwrap_or(&item.file_name).to_string();
        crate::jobs::enqueue_metadata_write(pool, nats_publisher, book_id, &title).await;
    }

    // Store cover if we have one
    let cover_data: Option<Vec<u8>> = ingest_queue::get_cover(pool, queue_id).await.ok().flatten();

    if let Some(cover_bytes) = cover_data {
        let covers_dir = PathBuf::from(storage_root).join("covers");
        if fs::create_dir_all(&covers_dir).await.is_ok() {
            let cover_path = covers_dir.join(format!("{}.jpg", book_id));
            let _ = fs::write(&cover_path, &cover_bytes).await;
        }
    }

    // Create per-device entitlements for auto-sync devices
    crate::auto_sync::create_auto_sync_entitlements(pool, nats_publisher, book_id).await;

    // Update queue item as approved
    let _ = ingest_queue::approve(pool, queue_id, book_id).await;

    // Notify library update
    let _ = library_events.send(LibraryEvent::library_updated(Some(book_id)));

    // Clean up Kobload watch pattern if this book was wanted
    crate::wanted::on_book_acquired(runtime_config, library_events, book_id).await;

    // Auto-extract ebook text if transcript_mode=auto
    {
        let config = runtime_config.get().await;
        if config.transcript_mode == "auto" {
            let title = item.title.as_deref().unwrap_or(&item.file_name).to_string();
            crate::jobs::enqueue_ebook_extract(pool, nats_publisher, book_id, &title).await;
        }
    }

    let action = if is_update { "updated" } else { "created" };
    info!(
        "Auto-ingested queue item {} -> book {} ({})",
        queue_id, book_id, action
    );

    Ok(book_id)
}

/// Process an audiobook file from the ingest queue.
///
/// Unlike ebook ingest (KEPUB conversion), this copies/links the audio file,
/// creates a book_file record, probes for metadata (narrator, duration, chapters),
/// and stores chapters in the audiobook_chapters table.
#[allow(clippy::too_many_arguments)]
async fn process_audiobook_ingest(
    pool: &DbPool,
    storage_root: &str,
    library_events: &broadcast::Sender<LibraryEvent>,
    nats_publisher: &Publisher,
    runtime_config: &crate::runtime_config::RuntimeConfigManager,
    item: &super::IngestQueueItem,
    queue_id: Uuid,
    book_id: Uuid,
    is_update: bool,
    output_dir: &Path,
    source_path: &Path,
    file_mode: FileMode,
    media_root: &Option<media_roots::EnabledMediaRoot>,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let format_name = item.source_format.to_uppercase();

    // Copy or link the audio file
    let dest_path = if file_mode == FileMode::Link {
        source_path.to_path_buf()
    } else {
        let file_name = source_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let dest = output_dir.join(&file_name);
        if let Err(e) = fs::copy(source_path, &dest).await {
            let error_msg = format!("Failed to copy audiobook file: {}", e);
            let _ = ingest_queue::set_error(pool, queue_id, &error_msg).await;
            return Err(error_msg.into());
        }
        dest
    };

    // Create or update book record
    if is_update {
        let _ = sqlx::query!(
            "UPDATE books SET want_status = 'none', updated_at = NOW() WHERE id = $1",
            book_id,
        )
        .execute(pool)
        .await;
        info!(book_id = %book_id, "Updating existing book with audiobook file");
    } else {
        let title = item.title.clone().unwrap_or_else(|| item.file_name.clone());
        let input = books::BookInput {
            id: Some(book_id),
            title,
            description: item.description.clone(),
            want_format: Some("audiobook".to_string()),
            ..Default::default()
        };
        books::upsert_library_book(pool, input).await?;

        if let Some(root) = media_root {
            let _ = sqlx::query!(
                "UPDATE books SET media_root_id = $1 WHERE id = $2",
                root.id,
                book_id,
            )
            .execute(pool)
            .await;
        }

        // Add authors
        let has_linked_authors = item
            .linked_authors
            .as_ref()
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty());
        if !has_linked_authors && let Some(merged_authors) = &item.merged_author_names {
            for author_name in merged_authors {
                let author_name = author_name.trim();
                if !author_name.is_empty()
                    && let Ok(author_id) = authors::get_or_create_by_name(pool, author_name).await
                {
                    let _ = authors::link_book(pool, book_id, author_id).await;
                }
            }
        }
    }

    // Store book_file record
    let file_size = fs::metadata(&dest_path)
        .await
        .map(|m| m.len() as i64)
        .unwrap_or(item.file_size);

    let (source_type, source_provenance) = item
        .source_metadata
        .as_ref()
        .map(|meta| {
            let st = meta
                .get("source_type")
                .and_then(|v| v.as_str())
                .unwrap_or("watch_directory")
                .to_string();
            let sm = meta
                .get("source_provenance")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            (st, sm)
        })
        .unwrap_or_else(|| ("watch_directory".to_string(), serde_json::json!({})));

    let book_file_id = audiobook_files::upsert_by_path(
        pool,
        audiobook_files::AudiobookFileInput {
            book_id,
            format: format_name,
            file_path: dest_path.to_string_lossy().to_string(),
            file_size,
            file_hash: item.file_hash.clone(),
            source_path: Some(item.file_path.clone()),
            source_mtime: item.source_mtime,
            source_type: Some(source_type),
            source_metadata: Some(source_provenance),
            ..Default::default()
        },
    )
    .await?;

    // Register in media_root_files
    if let Some(root) = media_root {
        let file_name = dest_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if let Err(e) = media_root_files::upsert_added(
            pool,
            root.id,
            &dest_path.to_string_lossy(),
            &file_name,
            Some(file_size),
            "book",
            book_id,
        )
        .await
        {
            warn!(error = %e, "Failed to register audiobook in media_root_files");
        }
    }

    // Probe audio file for metadata and chapters
    let probe_path = &dest_path;
    // Add Xing/Info header to MP3s that lack one — without it ffprobe and
    // browsers estimate duration from bitrate, which is wrong for old rips
    // and breaks browser seek (read-along desync).
    if let Err(e) = crate::audio::repair::ensure_xing_header(probe_path).await {
        warn!(error = %e, path = %probe_path.display(), "Failed to repair MP3 header");
    }
    match crate::audio::probe::probe_audio(probe_path).await {
        Ok(probe_result) => {
            // Update book with probed metadata
            let mut update = books::BookUpdate::default();
            if let Some(dur) = probe_result.duration_seconds {
                update.duration_seconds = Some(dur as i32);
            }
            if probe_result.narrator.is_some() {
                update.narrator = probe_result.narrator;
            }
            if let Some(is_abridged) = probe_result.is_abridged {
                update.is_abridged = Some(is_abridged);
            }
            let _ = books::update(pool, book_id, update).await;

            // Resolve and store chapters (uses fallback chain)
            let chapters = crate::audio::chapters::resolve_chapters(probe_path, None, None).await;
            if !chapters.is_empty()
                && let Err(e) =
                    audiobook_chapters::insert_chapters(pool, book_file_id, &chapters).await
            {
                warn!(error = %e, "Failed to store audiobook chapters");
            }

            info!(
                book_id = %book_id,
                duration = ?probe_result.duration_seconds,
                chapters = chapters.len(),
                "Probed audiobook metadata"
            );
        }
        Err(e) => {
            warn!(error = %e, path = %probe_path.display(), "Failed to probe audiobook, continuing without metadata");
        }
    }

    // Store cover if available
    if let Ok(Some(cover_bytes)) = ingest_queue::get_cover(pool, queue_id).await {
        let covers_dir = PathBuf::from(storage_root).join("covers");
        if fs::create_dir_all(&covers_dir).await.is_ok() {
            let cover_path = covers_dir.join(format!("{}.jpg", book_id));
            let _ = fs::write(&cover_path, &cover_bytes).await;
        }
    }

    // Create per-device entitlements for auto-sync devices
    crate::auto_sync::create_auto_sync_entitlements(pool, nats_publisher, book_id).await;

    // Approve queue item
    let _ = ingest_queue::approve(pool, queue_id, book_id).await;

    // Notify
    let _ = library_events.send(LibraryEvent::library_updated(Some(book_id)));
    crate::wanted::on_book_acquired(runtime_config, library_events, book_id).await;

    // Auto-transcribe audiobook if transcript_mode=auto
    {
        let config = runtime_config.get().await;
        if config.transcript_mode == "auto" {
            let title = item.title.as_deref().unwrap_or(&item.file_name).to_string();
            crate::jobs::enqueue_audiobook_transcribe(pool, nats_publisher, book_id, &title).await;
        }
    }

    let action = if is_update { "updated" } else { "created" };
    info!(
        "Auto-ingested audiobook queue item {} -> book {} ({})",
        queue_id, book_id, action
    );

    Ok(book_id)
}
