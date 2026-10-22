//! Book want job handler - enqueues want_request job to Kobload via NATS.
//!
//! This job is enqueued when a book is created/updated with want_status='wanted'.
//! It builds search patterns and enqueues a want_request job to Kobload.
//! The response comes back via events.want.response.kobbler subscriber.

use crate::LibraryEvent;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::books;
use crate::runtime_config::RuntimeConfigManager;
use crate::wanted::watch_sync::SOURCE_SYSTEM;
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use super::want_helpers::build_excluded_releases;

/// Payload for book_want jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct BookWantPayload {
    pub book_id: Uuid,
    /// Who asked, when a person did. Schedulers leave this unset.
    #[serde(default)]
    pub user_id: Option<Uuid>,
    /// A person asked for this book again right now. Kobload reopens an
    /// already-imported book instead of skipping it, and still picks under the
    /// quality profile.
    #[serde(default)]
    pub manual: bool,
}

/// Handler for wanting a book via Kobload (search + watch).
pub struct BookWantHandler {
    pool: PgPool,
    runtime_config: RuntimeConfigManager,
    nats_publisher: Publisher,
    library_events: broadcast::Sender<LibraryEvent>,
}

impl BookWantHandler {
    pub fn new(
        pool: PgPool,
        runtime_config: RuntimeConfigManager,
        nats_publisher: Publisher,
        library_events: broadcast::Sender<LibraryEvent>,
    ) -> Self {
        Self {
            pool,
            runtime_config,
            nats_publisher,
            library_events,
        }
    }
}

#[async_trait]
impl JobHandler for BookWantHandler {
    fn job_type(&self) -> &'static str {
        "book_want"
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: BookWantPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {}", e)))?;

        info!(job_id = %job_id, book_id = %payload.book_id, "Processing book_want job");

        // Get runtime config
        let runtime = self.runtime_config.get().await;

        // Get callback URL - required for want requests
        let callback_url = match &runtime.kobload_ingest_url {
            Some(url) if !url.trim().is_empty() => url.clone(),
            _ => {
                info!(book_id = %payload.book_id, "Kobload ingest URL not configured, skipping book_want");
                return Ok(json!({
                    "book_id": payload.book_id,
                    "status": "skipped",
                    "reason": "ingest_url_not_configured"
                }));
            }
        };

        let min_score = runtime.wanted_min_score.unwrap_or(0.65);

        // Fetch book data for pattern generation
        let book_data = books::get_wanted_for_sync(&self.pool, payload.book_id)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to fetch book: {}", e)))?
            .ok_or_else(|| JobError::ExecutionFailed("Book not found".to_string()))?;

        let identifiers = nats_jobs::BookIdentifiers {
            isbn10: book_data
                .identifiers
                .get("isbn10")
                .and_then(|v| v.as_str())
                .map(String::from),
            isbn13: book_data
                .identifiers
                .get("isbn13")
                .and_then(|v| v.as_str())
                .map(String::from),
            hardcover_id: book_data
                .identifiers
                .get("hardcover")
                .and_then(|v| v.as_str())
                .map(String::from),
            goodreads_id: book_data
                .identifiers
                .get("goodreads")
                .and_then(|v| v.as_str())
                .map(String::from),
            openlibrary_id: book_data
                .identifiers
                .get("openlibrary")
                .and_then(|v| v.as_str())
                .map(String::from),
        };

        // Read rejected releases to pass as exclusions
        let rejected = books::get_rejected_releases(&self.pool, payload.book_id)
            .await
            .unwrap_or(json!([]));
        let excluded_releases = build_excluded_releases(&rejected);

        let alternate_titles = crate::wanted::book_titles::book_alternate_titles(
            &crate::metadata::ProviderRegistry::new(self.pool.clone()),
            &crate::wanted::book_titles::BookRef {
                title: &book_data.title,
                series: book_data.series_name.as_deref(),
                position: book_data.series_index,
                hardcover_id: identifiers.hardcover_id.as_deref(),
            },
            crate::wanted::book_titles::Lookup::Fetch,
        )
        .await;

        // Determine which media wants to send based on want_format
        let isbn = identifiers.isbn13.clone().or(identifiers.isbn10.clone());
        let mut media_wants: Vec<nats_jobs::MediaWant> = Vec::new();

        if book_data.want_format == "ebook" || book_data.want_format == "both" {
            media_wants.push(nats_jobs::MediaWant::Book {
                title: book_data.title.clone(),
                author: book_data.author.clone(),
                isbn: isbn.clone(),
                format_preferences: crate::routes::ingest::EBOOK_EXTENSIONS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
                identifiers: Some(identifiers.clone()),
                series: book_data.series_name.clone(),
                series_position: book_data.series_index.map(|idx| idx.to_string()),
                release_date: book_data.pubdate.map(|dt| dt.date().to_string()),
                alternate_titles: alternate_titles.clone(),
            });
        }

        if book_data.want_format == "audiobook" || book_data.want_format == "both" {
            media_wants.push(nats_jobs::MediaWant::Audiobook {
                title: book_data.title.clone(),
                author: book_data.author.clone(),
                narrator: book_data.narrator.clone(),
                isbn: isbn.clone(),
                format_preferences: crate::routes::ingest::AUDIOBOOK_EXTENSIONS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
                identifiers: Some(identifiers.clone()),
                series: book_data.series_name.clone(),
                series_position: book_data.series_index.map(|idx| idx.to_string()),
                release_date: book_data.pubdate.map(|dt| dt.date().to_string()),
                alternate_titles: alternate_titles.clone(),
            });
        }

        // Enqueue a want_request for each media want
        let mut enqueued = 0;
        for media_want in &media_wants {
            nats_jobs::check_cancelled(&cancel)?;
            let want_request = nats_jobs::WantRequest {
                source_system: SOURCE_SYSTEM.to_string(),
                source_id: payload.book_id.to_string(),
                callback_url: callback_url.clone(),
                auth_token: None,
                query: None,
                patterns: Vec::new(),
                match_targets: Vec::new(),
                match_mode: "fuzzy".to_string(),
                min_score,
                early_exit_score: 0.90,
                media_type: Some(
                    match media_want {
                        nats_jobs::MediaWant::Audiobook { .. } => "audiobook",
                        _ => "book",
                    }
                    .to_string(),
                ),
                auto_download: true,
                limit: 100,
                metadata: Some(json!({
                    "book_id": payload.book_id,
                    "title": book_data.title,
                    "author": book_data.author,
                    "user_id": payload.user_id,
                })),
                media_want: Some(media_want.clone()),
                upgrade_allowed: Some(false),
                manual: payload.manual,
                excluded_releases: excluded_releases.clone(),
                search_not_before: None,
            };

            let job_payload = serde_json::to_value(&want_request).map_err(|e| {
                JobError::ExecutionFailed(format!("Failed to serialize request: {}", e))
            })?;

            match self
                .nats_publisher
                .enqueue_job("kobload", "want_request", job_payload)
                .await
            {
                Ok(_) => enqueued += 1,
                Err(e) => {
                    warn!(book_id = %payload.book_id, error = %e, "Failed to enqueue want_request job");
                    return Err(JobError::retryable(format!(
                        "Failed to enqueue want_request: {}",
                        e
                    )));
                }
            }
        }

        info!(
            book_id = %payload.book_id,
            want_format = %book_data.want_format,
            enqueued_count = enqueued,
            "Want request jobs enqueued to Kobload"
        );

        // Update want_status to "searching" now that the request is in flight
        if let Err(e) = books::set_want_status(&self.pool, payload.book_id, "searching").await {
            warn!(book_id = %payload.book_id, error = %e, "Failed to set want_status to searching");
        }
        let _ = self
            .library_events
            .send(LibraryEvent::library_updated(Some(payload.book_id)));

        Ok(json!({
            "book_id": payload.book_id,
            "status": "enqueued",
            "want_format": book_data.want_format,
            "enqueued_count": enqueued,
        }))
    }
}
