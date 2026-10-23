//! Series enrichment job handler.
//!
//! Fetches all books in a series from the tracking provider and creates/updates
//! book records for discovery.

use crate::images::{self, ImageEntry, ImageType};
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{
    authors, book_series, books, metadata_providers,
    provider_reconcile::{self, EntityKind},
    series::{self, SeriesRefreshInput},
};
use crate::tracking::{CachedAuthorProvider, reconcile};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// The tracking provider used for series enrichment.
const TRACKING_PROVIDER: &str = "hardcover";

/// Payload for series_enrich jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct SeriesEnrichPayload {
    pub series_id: Uuid,
}

pub struct SeriesEnrichHandler {
    pool: PgPool,
    storage_root: String,
    nats_publisher: nats_jobs::Publisher,
    library_events: tokio::sync::broadcast::Sender<crate::LibraryEvent>,
}

impl SeriesEnrichHandler {
    pub fn new(
        pool: PgPool,
        storage_root: String,
        nats_publisher: nats_jobs::Publisher,
        library_events: tokio::sync::broadcast::Sender<crate::LibraryEvent>,
    ) -> Self {
        Self {
            pool,
            storage_root,
            nats_publisher,
            library_events,
        }
    }

    fn create_provider(&self, api_key: String) -> Result<CachedAuthorProvider, JobError> {
        crate::tracking::create_tracking_provider(TRACKING_PROVIDER, api_key, self.pool.clone())
            .ok_or_else(|| {
                JobError::ExecutionFailed(format!(
                    "Unknown tracking provider: {}",
                    TRACKING_PROVIDER
                ))
            })
    }

    async fn get_tracking_api_key(&self) -> Result<String, JobError> {
        metadata_providers::get_api_key(&self.pool, TRACKING_PROVIDER)
            .await?
            .ok_or_else(|| {
                JobError::ExecutionFailed(format!(
                    "{} provider not configured or missing API key",
                    TRACKING_PROVIDER
                ))
            })
    }

    async fn find_or_create_author(&self, name: &str) -> Option<Uuid> {
        match authors::get_or_create_by_name(&self.pool, name).await {
            Ok(id) => Some(id),
            Err(e) => {
                warn!(error = %e, name = %name, "Failed to create author");
                None
            }
        }
    }
}

#[async_trait]
impl JobHandler for SeriesEnrichHandler {
    fn job_type(&self) -> &'static str {
        "series_enrich"
    }

    fn max_concurrent(&self) -> usize {
        2
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: SeriesEnrichPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {}", e)))?;
        nats_jobs::check_cancelled(&cancel)?;

        let _ = series::bump_next_refresh(&self.pool, payload.series_id).await;

        info!(job_id = %job_id, series_id = %payload.series_id, "Starting series enrichment");

        let series_info = series::get_for_enrichment(&self.pool, payload.series_id)
            .await?
            .ok_or_else(|| JobError::ExecutionFailed("Series not found".to_string()))?;

        let series_name = series_info.name;
        let identifiers = series_info.identifiers;
        let auto_want_mode = series_info.auto_want_mode;

        // Get provider API key and create provider
        let api_key = self.get_tracking_api_key().await?;
        let provider = self.create_provider(api_key.clone())?;
        let pid = provider.provider_id();
        let slug_key = format!("{}_slug", pid);

        // Get external ID from identifiers
        let external_id = identifiers
            .as_ref()
            .and_then(|ids| ids.get(pid))
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| {
                JobError::ExecutionFailed(format!("Series has no {} ID - link metadata first", pid))
            })?;

        info!(
            series_id = %payload.series_id,
            series_name = %series_name,
            external_id = %external_id,
            provider = %pid,
            "Fetching series from tracking provider"
        );

        let series_data = match provider.get_series_books(&external_id).await {
            Ok(outcome) => outcome.data,
            Err(e) => {
                if reconcile::settle_after_fetch_failure(
                    &self.pool,
                    &self.nats_publisher,
                    &api_key,
                    EntityKind::Series,
                    payload.series_id,
                    &external_id,
                )
                .await
                {
                    return Ok(json!({
                        "series_id": payload.series_id,
                        "series_name": series_name,
                        "status": "followed_hardcover",
                    }));
                }
                return Err(JobError::retryable(format!("Provider API error: {}", e)));
            }
        };

        // Update series metadata
        let refresh_input = SeriesRefreshInput {
            description: series_data.description.clone(),
            total_book_count: Some(series_data.book_count),
            images: series_data.image_url.as_ref().map(|url| {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    "cover".to_string(),
                    ImageEntry {
                        source_url: Some(url.clone()),
                        cached_at: None,
                    },
                );
                m
            }),
        };
        let _ = series::refresh_metadata(&self.pool, payload.series_id, refresh_input).await;

        // Store series slug
        if let Some(ref slug) = series_data.slug {
            let slug_ids = Some(json!({ slug_key.clone(): slug }));
            series::merge_identifiers(&self.pool, payload.series_id, &slug_ids).await;
        }
        if let Some(ref meta) = series_data.provider_metadata {
            let _ = sqlx::query!(
                "UPDATE series SET metadata = COALESCE(metadata, '{}'::jsonb) || $2 WHERE id = $1",
                payload.series_id,
                meta,
            )
            .execute(&self.pool)
            .await;
        }

        images::try_cache_image(
            &self.storage_root,
            ImageType::Series,
            payload.series_id,
            series_data.image_url.as_deref(),
        )
        .await;

        let monitoring = authors::get_series_monitoring(&self.pool, payload.series_id)
            .await
            .ok()
            .flatten();

        let effective_mode = if auto_want_mode == "inherit" {
            monitoring
                .as_ref()
                .map(|m| m.auto_want_mode.clone())
                .unwrap_or_else(|| "none".to_string())
        } else {
            auto_want_mode
        };

        let monitoring_started_at = monitoring.and_then(|m| m.monitoring_started_at);

        let mut created_count = 0;
        let mut updated_count = 0;
        let mut wanted_count = 0;
        let current_ids: Vec<String> = series_data.books.iter().map(|b| b.id.clone()).collect();

        for book in series_data.books {
            nats_jobs::check_cancelled(&cancel)?;
            let book_external_id = &book.id;

            // Build identifiers
            let mut book_identifiers = json!({ pid: book_external_id });
            if let Some(ref slug) = book.slug {
                book_identifiers[&slug_key] = json!(slug);
            }

            // Check if book already exists by hardcover ID or series+title
            let mut existing = books::find_by_hardcover_or_series_title(
                &self.pool,
                book_external_id,
                payload.series_id,
                &book.title,
            )
            .await?;

            // Fallback: check for in-library book by title + author names.
            // The series-based check above misses library books when the series
            // is newly created and has no members yet.
            if existing.is_none() {
                let author_names: Vec<String> =
                    book.authors.iter().map(|a| a.to_string()).collect();
                existing = books::find_library_book_by_title_and_author_names(
                    &self.pool,
                    &book.title,
                    &author_names,
                )
                .await?;
            }

            let book_id = match existing {
                Some(existing_info) => {
                    let existing_id = existing_info.id;
                    let is_in_library = existing_info.is_in_library;
                    let user_edited_at = existing_info.user_edited_at;

                    if user_edited_at.is_none() {
                        let pubdate = book.release_date.map(|d| d.midnight().assume_utc());
                        let update = books::BookUpdate {
                            description: book.description.clone(),
                            images: book.cover_url.as_ref().map(|url| {
                                let mut m = std::collections::HashMap::new();
                                m.insert(
                                    "cover".to_string(),
                                    ImageEntry {
                                        source_url: Some(url.clone()),
                                        cached_at: None,
                                    },
                                );
                                m
                            }),
                            pubdate,
                            is_released: Some(book.is_released),
                            page_count: book.pages,
                            rating: book.rating,
                            rating_count: book.ratings_count,
                            identifiers: Some(book_identifiers.clone()),
                            provider_metadata: book.provider_metadata.clone(),
                            ..Default::default()
                        };
                        let _ = books::update(&self.pool, existing_id, update).await;

                        if !is_in_library {
                            images::try_cache_image(
                                &self.storage_root,
                                ImageType::Series,
                                existing_id,
                                book.cover_url.as_deref(),
                            )
                            .await;
                        }

                        updated_count += 1;
                    }

                    // Add to series via junction table (books can be in multiple series)
                    let _ = book_series::add(
                        &self.pool,
                        existing_id,
                        payload.series_id,
                        book.position,
                        false,
                    )
                    .await;
                    let _ = book_series::auto_select_primary(&self.pool, existing_id, false).await;

                    existing_id
                }
                None => {
                    let want_status = if book.is_released && effective_mode != "none" {
                        let should_want = match effective_mode.as_str() {
                            "all" => true,
                            "fill_gaps" => true,
                            "new_only" => match (book.release_date, monitoring_started_at) {
                                (Some(d), Some(started_at)) => {
                                    let release_dt = d.midnight().assume_utc();
                                    release_dt > started_at
                                }
                                _ => true,
                            },
                            _ => false,
                        };
                        if should_want {
                            wanted_count += 1;
                            "wanted"
                        } else {
                            "pending"
                        }
                    } else {
                        "pending"
                    };

                    let input = books::DiscoveredBookInput {
                        title: book.title.clone(),
                        description: book.description.clone(),
                        images: book.cover_url.as_ref().map(|url| {
                            let mut m = std::collections::HashMap::new();
                            m.insert(
                                "cover".to_string(),
                                ImageEntry {
                                    source_url: Some(url.clone()),
                                    cached_at: None,
                                },
                            );
                            m
                        }),
                        release_date: book.release_date,
                        is_released: Some(book.is_released),
                        identifiers: Some(book_identifiers.clone()),
                        want_status: Some(want_status.to_string()),
                        page_count: book.pages,
                        rating: book.rating,
                        rating_count: book.ratings_count,
                        discovered_from_author_id: None,
                        provider_metadata: book.provider_metadata.clone(),
                        ..Default::default()
                    };

                    let new_id = match books::create_discovered_book(&self.pool, input).await {
                        Ok(id) => id,
                        Err(e) => {
                            warn!(error = %e, title = %book.title, "Failed to create book");
                            continue;
                        }
                    };

                    for author_name in &book.authors {
                        if let Some(author_id) = self.find_or_create_author(author_name).await {
                            let _ = authors::link_book(&self.pool, new_id, author_id).await;
                        }
                    }

                    let _ = series::set_book_series(
                        &self.pool,
                        new_id,
                        Some(payload.series_id),
                        book.position,
                    )
                    .await;
                    let _ = book_series::auto_select_primary(&self.pool, new_id, false).await;

                    images::try_cache_image(
                        &self.storage_root,
                        ImageType::Series,
                        new_id,
                        book.cover_url.as_deref(),
                    )
                    .await;

                    if want_status == "wanted"
                        && let Err(e) = self
                            .nats_publisher
                            .enqueue_job("kobbler", "book_want", json!({ "book_id": new_id }))
                            .await
                    {
                        warn!(book_id = %new_id, error = %e, "Failed to enqueue book_want job");
                    }

                    created_count += 1;
                    new_id
                }
            };

            info!(
                book_id = %book_id,
                title = %book.title,
                position = ?book.position,
                "Processed series book"
            );
        }

        // Follow Hardcover: books it no longer lists here leave the series, and any that
        // only this series had brought in go. An empty list is more likely a bad
        // response than an empty series.
        if !current_ids.is_empty() {
            match provider_reconcile::remove_stale_series_members(
                &self.pool,
                payload.series_id,
                &current_ids,
            )
            .await
            {
                Ok(removed) if !removed.is_empty() => {
                    let deleted = provider_reconcile::delete_unanchored_books(&self.pool, &removed)
                        .await
                        .unwrap_or_default();
                    info!(
                        series_id = %payload.series_id,
                        removed = removed.len(),
                        deleted = deleted.len(),
                        "Removed books Hardcover no longer lists in this series"
                    );
                    for book_id in removed {
                        crate::jobs::enqueue_book_index(&self.pool, &self.nats_publisher, book_id)
                            .await;
                    }
                }
                Err(e) => warn!(error = %e, "Failed to remove stale series members"),
                _ => {}
            }
        }

        // Post-hoc cleanup: match any discovered books to library books now that
        // authors are linked. This catches duplicates from the series_enrich /
        // media_root_match race condition.
        match books::match_series_discovered_to_library(&self.pool, payload.series_id).await {
            Ok(matched) if matched > 0 => {
                info!(matched = matched, series_id = %payload.series_id, "Matched series discovered books to library");
            }
            Err(e) => {
                warn!(error = %e, "Failed to match series discovered books to library");
            }
            _ => {}
        }

        let _ = self
            .library_events
            .send(crate::LibraryEvent::series_updated(payload.series_id));

        info!(
            job_id = %job_id,
            series_id = %payload.series_id,
            series_name = %series_name,
            created = created_count,
            updated = updated_count,
            wanted = wanted_count,
            "Series enrichment complete"
        );

        Ok(json!({
            "series_id": payload.series_id,
            "series_name": series_name,
            "books_found": series_data.book_count,
            "created": created_count,
            "updated": updated_count,
            "wanted": wanted_count
        }))
    }
}

// =============================================================================
// Series Sync Check Handler (Scheduled Job)
// =============================================================================

/// Scheduled job handler that checks for series due for metadata refresh.
pub struct SeriesSyncCheckHandler {
    pool: PgPool,
    nats_publisher: nats_jobs::Publisher,
}

impl SeriesSyncCheckHandler {
    pub fn new(pool: PgPool, nats_publisher: nats_jobs::Publisher) -> Self {
        Self {
            pool,
            nats_publisher,
        }
    }
}

#[async_trait]
impl JobHandler for SeriesSyncCheckHandler {
    fn job_type(&self) -> &'static str {
        "series_sync_check"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        _payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let series_list = series::list_due_for_refresh(&self.pool, 3).await?;

        if series_list.is_empty() {
            return Ok(json!({ "checked": 0, "message": "No series due for check" }));
        }

        let mut enqueued = 0;

        for s in series_list {
            nats_jobs::check_cancelled(&cancel)?;
            if s.hardcover_id.is_none()
                || s.hardcover_id
                    .as_ref()
                    .map(|h| h.is_empty())
                    .unwrap_or(true)
            {
                warn!(series_id = %s.id, "Skipping series without tracking provider ID");
                continue;
            }

            let _ = series::bump_next_refresh(&self.pool, s.id).await;

            info!(series_id = %s.id, name = %s.name, "Enqueuing series enrichment");

            match self
                .nats_publisher
                .enqueue_job("kobbler", "series_enrich", json!({ "series_id": s.id }))
                .await
            {
                Ok(_) => {
                    enqueued += 1;
                }
                Err(e) => {
                    warn!(
                        series_id = %s.id,
                        error = %e,
                        "Failed to enqueue series_enrich job"
                    );
                }
            }

            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        Ok(json!({
            "enqueued": enqueued,
            "message": format!("Enqueued {} series for enrichment", enqueued)
        }))
    }
}
