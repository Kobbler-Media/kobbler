//! Per-book enrichment job handler.
//!
//! Fetches full metadata from providers (Hardcover, OpenLibrary, Google Books) for a
//! single book. Handles series resolution with author overlap, author Hardcover ID
//! backfilling, and cover extraction from ebook files.
//!
//! Enqueued by media_root_match (on auto-add) and media_root_enrich (catch-up).

use crate::convert::EbookMeta;
use crate::images::{self, ImageEntry, ImageMap, ImageType};
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::metadata::{MetadataQuery, ProviderRegistry};
use crate::repositories::{
    audiobook_chapters, audiobook_files, authors, book_files, book_series, books, media_root_files,
    metadata_providers, series,
};
use crate::sidecar::images::DiscoveredImage;

use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::Path;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Payload for book_enrich jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct BookEnrichPayload {
    pub book_id: Uuid,
}

pub struct BookEnrichHandler {
    pool: PgPool,
    nats: Publisher,
    storage_root: String,
    library_events: tokio::sync::broadcast::Sender<crate::LibraryEvent>,
    runtime_config: crate::RuntimeConfigManager,
}

impl BookEnrichHandler {
    pub fn new(
        pool: PgPool,
        nats: Publisher,
        storage_root: String,
        library_events: tokio::sync::broadcast::Sender<crate::LibraryEvent>,
        runtime_config: crate::RuntimeConfigManager,
    ) -> Self {
        Self {
            pool,
            nats,
            storage_root,
            library_events,
            runtime_config,
        }
    }
}

#[async_trait]
impl JobHandler for BookEnrichHandler {
    fn job_type(&self) -> &'static str {
        "book_enrich"
    }

    fn max_concurrent(&self) -> usize {
        self.runtime_config.job_concurrency("book_enrich", 2)
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: BookEnrichPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {}", e)))?;

        nats_jobs::check_cancelled(&cancel)?;

        info!(job_id = %job_id, book_id = %payload.book_id, "Starting book enrichment");

        let mut enriched_metadata = false;
        let mut author_jobs_enqueued = 0u32;
        let mut series_enriched = false;

        // Phase 1: Metadata enrichment from providers
        if let Some(result) = self.enrich_metadata(payload.book_id).await? {
            enriched_metadata = true;

            nats_jobs::check_cancelled(&cancel)?;

            // Phase 2: Series resolution — add all series from metadata
            if !result.series_list.is_empty() {
                series_enriched = self.resolve_all_series(payload.book_id, &result).await;
            } else if let Some(series_name) = &result.series {
                // Fallback: single series field
                series_enriched = self
                    .resolve_single_series(payload.book_id, series_name, &result)
                    .await;
            }

            // Phase 3: Author Hardcover ID backfilling
            author_jobs_enqueued = self.backfill_author_ids(payload.book_id).await;
        }

        nats_jobs::check_cancelled(&cancel)?;

        // Phase 4: Cover extraction from ebook files
        let enriched_cover = self.extract_cover(payload.book_id).await;

        // Mark as enriched so catch-up handler doesn't re-enqueue
        let _ = sqlx::query("UPDATE books SET enriched_at = NOW() WHERE id = $1")
            .bind(payload.book_id)
            .execute(&self.pool)
            .await;

        crate::sidecar::write_back::write_book_sidecars(&self.pool, payload.book_id).await;
        crate::sidecar::write_back::write_book_images(
            &self.pool,
            &self.storage_root,
            payload.book_id,
        )
        .await;

        // Broadcast update event for SSE
        let _ = self
            .library_events
            .send(crate::LibraryEvent::library_updated(Some(payload.book_id)));

        info!(
            job_id = %job_id,
            book_id = %payload.book_id,
            enriched_metadata,
            enriched_cover,
            author_jobs_enqueued,
            series_enriched,
            "Book enrichment complete"
        );

        Ok(json!({
            "book_id": payload.book_id,
            "enriched_metadata": enriched_metadata,
            "enriched_cover": enriched_cover,
            "author_jobs_enqueued": author_jobs_enqueued,
            "series_enriched": series_enriched,
        }))
    }
}

impl BookEnrichHandler {
    /// Fetch metadata from providers and update the book record.
    async fn enrich_metadata(
        &self,
        book_id: Uuid,
    ) -> Result<Option<crate::metadata::MetadataResult>, JobError> {
        type BookInfo = (String, Option<String>, Option<String>);
        let book_info: Option<BookInfo> = sqlx::query_as(
            r#"SELECT b.title, b.identifiers->>'isbn' AS isbn, b.identifiers->>'asin' AS asin
               FROM books b WHERE b.id = $1"#,
        )
        .bind(book_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;

        let Some((title, isbn, asin)) = book_info else {
            return Err(JobError::ExecutionFailed("Book not found".to_string()));
        };

        let author = primary_author_name(&self.pool, book_id)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;

        let registry = ProviderRegistry::new(self.pool.clone());
        let mut query = if let Some(isbn) = &isbn {
            MetadataQuery::by_isbn(isbn)
        } else {
            MetadataQuery::by_title(&title)
        };
        query.asin = asin;
        query.authors = author.into_iter().collect();

        let mut outcome = registry.search(&query, None).await;
        outcome.adjust_relevance(&query);
        outcome.boost_series_matches();
        let Some(best) = outcome.results.into_iter().next() else {
            debug!(book_id = %book_id, title = %title, "No enrichment results found");
            return Ok(None);
        };

        // Build images: sidecar files first (local files take precedence), then provider
        let sidecar_imgs = load_sidecar_images_for_entity(&self.pool, "book", book_id).await;
        let mut book_images: HashMap<String, ImageEntry> =
            import_sidecar_images(&self.storage_root, book_id, &sidecar_imgs, "book").await;

        if let Some(url) = &best.cover_url {
            book_images
                .entry("cover".into())
                .or_insert_with(|| ImageEntry {
                    source_url: Some(url.clone()),
                    cached_at: None,
                });
        }

        let update = books::BookUpdate {
            description: best.description.clone(),
            language: best.language.clone(),
            publisher: best.publisher.clone(),
            page_count: best.page_count.map(|p| p as i32),
            rating: best.rating,
            rating_count: best.rating_count,
            narrator: best.narrator.clone(),
            duration_seconds: best.duration_seconds.map(|d| d as i32),
            is_abridged: None, // Audnexus doesn't provide abridged info directly
            images: if book_images.is_empty() {
                None
            } else {
                Some(book_images)
            },
            ..Default::default()
        };

        if let Err(e) = books::update(&self.pool, book_id, update).await {
            warn!(book_id = %book_id, error = %e, "Failed to update book metadata");
        } else {
            // Merge provider identifiers (e.g. hardcover_slug)
            if let Some(obj) = best.identifiers.as_object()
                && !obj.is_empty()
            {
                let _ =
                    books::update_identifiers(&self.pool, book_id, best.identifiers.clone()).await;
            }

            // Store chapters from provider on audiobook files that lack chapters
            if !best.chapters.is_empty() {
                self.store_provider_chapters(book_id, &best.chapters).await;
            }
        }

        Ok(Some(best))
    }

    /// Store provider-sourced chapters on audiobook files that don't already have chapters.
    ///
    /// For single-file books: store all chapters on that file.
    /// For multi-file books with known durations: split chapters across files
    /// by timestamp range, adjusting to file-local timestamps.
    /// For multi-file books without durations: skip (probe job will handle later).
    async fn store_provider_chapters(
        &self,
        book_id: Uuid,
        chapters: &[crate::metadata::types::Chapter],
    ) {
        let audio_files = match audiobook_files::list_by_book(&self.pool, book_id).await {
            Ok(files) => files,
            Err(_) => return,
        };

        if audio_files.is_empty() {
            return;
        }

        // Convert provider chapters to seconds
        let all_chapters: Vec<(String, f64, f64)> = chapters
            .iter()
            .map(|ch| {
                let title = if ch.title.is_empty() {
                    String::new()
                } else {
                    ch.title.clone()
                };
                (
                    title,
                    ch.start_ms as f64 / 1000.0,
                    ch.end_ms as f64 / 1000.0,
                )
            })
            .collect();

        if audio_files.len() == 1 {
            // Single file: store all chapters on it
            let file = &audio_files[0];
            if let Ok(existing) = audiobook_chapters::get_chapters(&self.pool, file.id).await
                && !existing.is_empty()
            {
                return;
            }
            let inputs: Vec<audiobook_chapters::ChapterInput> = all_chapters
                .iter()
                .enumerate()
                .map(
                    |(i, (title, start, end))| audiobook_chapters::ChapterInput {
                        chapter_index: i as i32,
                        title: if title.is_empty() {
                            None
                        } else {
                            Some(title.clone())
                        },
                        start_seconds: *start,
                        end_seconds: *end,
                        file_path: None,
                        source: "provider".to_string(),
                    },
                )
                .collect();
            if let Err(e) = audiobook_chapters::insert_chapters(&self.pool, file.id, &inputs).await
            {
                warn!(book_id = %book_id, error = %e, "Failed to store provider chapters");
            }
            return;
        }

        // Multi-file: check if all files have known durations
        let all_durations_known = audio_files.iter().all(|f| f.duration_seconds.is_some());
        if !all_durations_known {
            debug!(
                book_id = %book_id,
                "Skipping provider chapter split: not all files have duration data"
            );
            return;
        }

        // Split chapters across files by duration boundaries
        let mut cumulative_offset = 0.0;
        for file in &audio_files {
            let file_duration = file.duration_seconds.unwrap_or(0.0);
            let file_end = cumulative_offset + file_duration;

            // Skip if file already has chapters
            if let Ok(existing) = audiobook_chapters::get_chapters(&self.pool, file.id).await
                && !existing.is_empty()
            {
                cumulative_offset = file_end;
                continue;
            }

            // Find chapters that overlap with this file's time range
            let mut file_chapters = Vec::new();
            for (title, start, end) in &all_chapters {
                // Chapter overlaps this file if it starts before file_end and ends after file_start
                if *start < file_end && *end > cumulative_offset {
                    let local_start = (*start - cumulative_offset).max(0.0);
                    let local_end = (*end - cumulative_offset).min(file_duration);
                    file_chapters.push(audiobook_chapters::ChapterInput {
                        chapter_index: file_chapters.len() as i32,
                        title: if title.is_empty() {
                            None
                        } else {
                            Some(title.clone())
                        },
                        start_seconds: local_start,
                        end_seconds: local_end,
                        file_path: None,
                        source: "provider".to_string(),
                    });
                }
            }

            if !file_chapters.is_empty() {
                if let Err(e) =
                    audiobook_chapters::insert_chapters(&self.pool, file.id, &file_chapters).await
                {
                    warn!(book_id = %book_id, file_id = %file.id, error = %e, "Failed to store split provider chapters");
                } else {
                    debug!(
                        book_id = %book_id,
                        file_id = %file.id,
                        count = file_chapters.len(),
                        "Stored split provider chapters on audiobook file"
                    );
                }
            }

            cumulative_offset = file_end;
        }
    }

    /// Resolve ALL series from metadata result and add junction entries.
    async fn resolve_all_series(
        &self,
        book_id: Uuid,
        result: &crate::metadata::MetadataResult,
    ) -> bool {
        let author_ids = authors::get_book_author_ids(&self.pool, book_id)
            .await
            .unwrap_or_default();

        let mut any_resolved = false;
        for si in &result.series_list {
            let series_identifiers = si.provider_id.as_ref().map(|id| json!({ "hardcover": id }));

            match series::get_or_create_with_author_overlap(
                &self.pool,
                &si.name,
                &author_ids,
                series_identifiers,
            )
            .await
            {
                Ok(Some(series_id)) => {
                    // Add as non-primary — auto_select_primary picks the best one after the loop
                    let _ = book_series::add(&self.pool, book_id, series_id, si.index, false).await;
                    any_resolved = true;

                    // Enqueue series_enrich if this series has a Hardcover ID
                    let has_hc_id: bool = sqlx::query_scalar(
                        "SELECT identifiers->>'hardcover' IS NOT NULL FROM series WHERE id = $1",
                    )
                    .bind(series_id)
                    .fetch_optional(&self.pool)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or(false);

                    if has_hc_id {
                        let display =
                            crate::jobs::fetch_job_display(&self.pool, "series", series_id).await;
                        let context = crate::jobs::JobContext::for_entity(
                            "series",
                            series_id,
                            format!("Enriching series '{}'", si.name),
                        )
                        .with_display_opt(display);
                        let _ = crate::jobs::enqueue(
                            &self.pool,
                            &self.nats,
                            "series_enrich",
                            json!({ "series_id": series_id }),
                            context,
                        )
                        .await;
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    warn!(book_id = %book_id, series = %si.name, error = %e, "Failed to resolve series");
                }
            }
        }

        if any_resolved {
            let _ = book_series::auto_select_primary(&self.pool, book_id, false).await;
        }

        any_resolved
    }

    /// Resolve a single series using author overlap and Hardcover series ID.
    async fn resolve_single_series(
        &self,
        book_id: Uuid,
        series_name: &str,
        result: &crate::metadata::MetadataResult,
    ) -> bool {
        // Get author IDs for this book
        let author_ids = authors::get_book_author_ids(&self.pool, book_id)
            .await
            .unwrap_or_default();

        // Build series identifiers from provider
        let series_identifiers = result
            .series_provider_id
            .as_ref()
            .map(|id| json!({ "hardcover": id }));

        match series::get_or_create_with_author_overlap(
            &self.pool,
            series_name,
            &author_ids,
            series_identifiers,
        )
        .await
        {
            Ok(Some(series_id)) => {
                // Link book to series via junction table
                let _ = series::set_book_series(
                    &self.pool,
                    book_id,
                    Some(series_id),
                    result.series_index,
                )
                .await;
                let _ = book_series::auto_select_primary(&self.pool, book_id, false).await;

                // Check if this series now has a Hardcover ID and needs enrichment
                let has_hc_id: bool = sqlx::query_scalar(
                    "SELECT identifiers->>'hardcover' IS NOT NULL FROM series WHERE id = $1",
                )
                .bind(series_id)
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten()
                .unwrap_or(false);

                if has_hc_id {
                    let display =
                        crate::jobs::fetch_job_display(&self.pool, "series", series_id).await;
                    let context = crate::jobs::JobContext::for_entity(
                        "series",
                        series_id,
                        format!("Enriching series '{}'", series_name),
                    )
                    .with_display_opt(display);
                    if let Err(e) = crate::jobs::enqueue(
                        &self.pool,
                        &self.nats,
                        "series_enrich",
                        json!({ "series_id": series_id }),
                        context,
                    )
                    .await
                    {
                        warn!(series_id = %series_id, error = %e, "Failed to enqueue series_enrich job");
                    }
                }
                true
            }
            Ok(None) => false,
            Err(e) => {
                warn!(book_id = %book_id, series = %series_name, error = %e, "Failed to resolve series");
                false
            }
        }
    }

    /// For each author on this book that lacks a Hardcover ID, search Hardcover
    /// by name, backfill the ID if found, then enqueue author_enrich.
    async fn backfill_author_ids(&self, book_id: Uuid) -> u32 {
        let author_ids = authors::get_book_author_ids(&self.pool, book_id)
            .await
            .unwrap_or_default();

        let tracking_provider_id = "hardcover";
        let api_key = match metadata_providers::get_api_key(&self.pool, tracking_provider_id).await
        {
            Ok(Some(key)) => key,
            _ => return 0,
        };

        let mut enqueued = 0u32;
        let Some(provider) = crate::tracking::create_tracking_provider(
            tracking_provider_id,
            api_key,
            self.pool.clone(),
        ) else {
            return 0;
        };
        let pid = provider.provider_id();

        for author_id in author_ids {
            // Check if author already has a tracking provider ID
            let has_provider_id: bool = sqlx::query_scalar(
                "SELECT identifiers->>$1::text IS NOT NULL AND identifiers->>$1::text != '' FROM authors WHERE id = $2",
            )
            .bind(pid)
            .bind(author_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(false);

            if has_provider_id {
                continue;
            }

            // Get author name for search
            let author_name = match authors::get_name(&self.pool, author_id).await {
                Ok(Some(name)) => name,
                _ => continue,
            };

            // Search tracking provider for this author (with caching)
            let outcome = match provider.search_authors(&author_name).await {
                Ok(r) => r,
                Err(e) => {
                    debug!(author = %author_name, error = %e, "Author search failed");
                    continue;
                }
            };

            let Some(best) = outcome.data.first() else {
                debug!(author = %author_name, "No results for author");
                continue;
            };

            // Backfill the provider ID
            let ids = json!({ pid: best.id });
            let _ = authors::merge_identifiers(&self.pool, author_id, &ids).await;

            info!(author_id = %author_id, author = %author_name, external_id = %best.id, provider = %pid, "Backfilled author provider ID");

            // Enqueue author_enrich to fetch full author data
            if let Err(e) = self
                .nats
                .enqueue_job(
                    "kobbler",
                    "author_enrich",
                    json!({ "author_id": author_id.to_string() }),
                )
                .await
            {
                warn!(author_id = %author_id, error = %e, "Failed to enqueue author_enrich job");
            } else {
                enqueued += 1;
            }

            // Rate limit between Hardcover searches (skip if result came from cache)
            if !outcome.cached {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }

        enqueued
    }

    /// Extract cover from ebook file if no cover exists yet.
    async fn extract_cover(&self, book_id: Uuid) -> bool {
        // Check if book already has an extracted cover
        let has_extracted: bool =
            sqlx::query_scalar("SELECT images ? 'extracted' FROM books WHERE id = $1")
                .bind(book_id)
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten()
                .unwrap_or(false);

        if has_extracted {
            return false;
        }

        let ebook_meta = EbookMeta::new();

        // Get preferred ebook file
        let file_path = match book_files::get_preferred_file(&self.pool, book_id).await {
            Ok(Some((_file_id, path))) => path,
            _ => return false,
        };

        // Native pipeline handles EPUB/MOBI/AZW/AZW3/KFX without Calibre. For
        // anything else we still need ebook-meta to be installed.
        if !kob_ebook::native_supports(Path::new(&file_path)) && !ebook_meta.is_available().await {
            return false;
        }

        // Check if extracted cover already exists on disk
        if images::cached_image_exists(&self.storage_root, ImageType::BookExtracted, book_id) {
            let _ = images::mark_cached(&self.pool, "books", book_id, "extracted").await;
            return true;
        }

        // Extract cover to temp directory
        let temp_dir = std::env::temp_dir().join(format!("kobbler_cover_{}", book_id));
        let cover_path = match ebook_meta
            .extract_cover(Path::new(&file_path), &temp_dir)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                debug!(book_id = %book_id, error = %e, "No cover in ebook file");
                let _ = tokio::fs::remove_dir_all(&temp_dir).await;
                return false;
            }
        };

        let cover_bytes = match tokio::fs::read(&cover_path).await {
            Ok(b) => b,
            Err(e) => {
                debug!(book_id = %book_id, error = %e, "Failed to read extracted cover");
                let _ = tokio::fs::remove_dir_all(&temp_dir).await;
                return false;
            }
        };

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        match images::save_image_bytes(
            &self.storage_root,
            ImageType::BookExtracted,
            book_id,
            &cover_bytes,
        )
        .await
        {
            Ok(_) => {
                let _ = images::mark_cached(&self.pool, "books", book_id, "extracted").await;
                debug!(book_id = %book_id, "Extracted and cached ebook cover");
            }
            Err(e) => {
                debug!(book_id = %book_id, error = %e, "Failed to save extracted cover");
                return false;
            }
        }

        // Promote extracted cover to primary if no other cover source exists
        let has_uploaded = {
            let mut p = std::path::PathBuf::from(&self.storage_root);
            p.push("covers");
            p.push(format!("{}.jpg", book_id));
            p.exists()
        };
        let has_series_cache =
            images::cached_image_exists(&self.storage_root, ImageType::Series, book_id);
        let has_source_url = books::get_cover_url(&self.pool, book_id)
            .await
            .ok()
            .flatten()
            .is_some();

        if !has_uploaded
            && !has_series_cache
            && !has_source_url
            && images::save_image_bytes(
                &self.storage_root,
                ImageType::Series,
                book_id,
                &cover_bytes,
            )
            .await
            .is_ok()
        {
            let now = time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default();
            let update = books::BookUpdate {
                images: Some({
                    let mut m = ImageMap::new();
                    m.insert(
                        "cover".into(),
                        ImageEntry {
                            source_url: Some("extracted://embedded".into()),
                            cached_at: Some(now),
                        },
                    );
                    m
                }),
                ..Default::default()
            };
            let _ = books::update(&self.pool, book_id, update).await;
            debug!(book_id = %book_id, "Promoted extracted cover to primary");
        }

        true
    }
}

// =============================================================================
// Shared helpers (duplicated from media_root_enrich for now, could be extracted)
// =============================================================================

/// Import sidecar images for a matched entity.
async fn import_sidecar_images(
    storage_root: &str,
    entity_id: Uuid,
    sidecar_images: &[DiscoveredImage],
    media_type: &str,
) -> HashMap<String, ImageEntry> {
    let mut map = HashMap::new();

    for img in sidecar_images {
        let image_type = match (media_type, img.image_key.as_str()) {
            ("book", "cover") => images::ImageType::Series,
            _ => continue,
        };

        let bytes = match tokio::fs::read(&img.file_path).await {
            Ok(b) => b,
            Err(e) => {
                debug!(path = %img.file_path, error = %e, "Failed to read sidecar image");
                continue;
            }
        };

        match images::save_image_bytes(storage_root, image_type, entity_id, &bytes).await {
            Ok(_) => {
                let now = time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default();
                map.insert(
                    img.image_key.clone(),
                    ImageEntry {
                        source_url: Some(format!("file://{}", img.file_path)),
                        cached_at: Some(now),
                    },
                );
            }
            Err(e) => {
                debug!(path = %img.file_path, error = %e, "Failed to save sidecar image");
            }
        }
    }

    map
}

/// Load sidecar images for all files matched to an entity.
async fn load_sidecar_images_for_entity(
    pool: &PgPool,
    entity_type: &str,
    entity_id: Uuid,
) -> Vec<DiscoveredImage> {
    let files = match media_root_files::get_by_entity(pool, entity_type, entity_id).await {
        Ok(f) => f,
        Err(_) => return vec![],
    };

    let mut all_images = Vec::new();
    for file in &files {
        if let Some(ref val) = file.sidecar_images
            && let Ok(imgs) = serde_json::from_value::<Vec<DiscoveredImage>>(val.clone())
        {
            all_images.extend(imgs);
        }
    }
    all_images
}

/// `book_authors` has no primary flag, so the whole codebase agrees on the
/// lowest `author_id` (see `routes/admin/book_moves.rs`, `watcher/service.rs`,
/// `jobs/handlers/media_root_reorganize.rs`). This one feeds the provider
/// search, so an arbitrary pick can match a different upstream record on every
/// re-enrichment.
pub async fn primary_author_name(
    pool: &PgPool,
    book_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        r#"SELECT a.name FROM book_authors ba
           JOIN authors a ON a.id = ba.author_id
           WHERE ba.book_id = $1
           ORDER BY ba.author_id
           LIMIT 1"#,
    )
    .bind(book_id)
    .fetch_optional(pool)
    .await
}
