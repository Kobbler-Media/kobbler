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
use crate::wanted::SOURCE_SYSTEM;
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
        let Some(book_data) = books::get_wanted_for_sync(&self.pool, payload.book_id)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to fetch book: {}", e)))?
        else {
            info!(book_id = %payload.book_id, "Book is not wanted or already owned, skipping book_want");
            return Ok(json!({
                "book_id": payload.book_id,
                "status": "skipped",
                "reason": "not_wanted"
            }));
        };

        // Read rejected releases to pass as exclusions
        let rejected = books::get_rejected_releases(&self.pool, payload.book_id)
            .await
            .unwrap_or(json!([]));
        let excluded_releases = build_excluded_releases(&rejected);

        let book = crate::wanted::book_media_want::WantedBook {
            id: payload.book_id,
            title: &book_data.title,
            author: book_data.author.as_deref(),
            narrator: book_data.narrator.as_deref(),
            identifiers: &book_data.identifiers,
            series_name: book_data.series_name.as_deref(),
            series_index: book_data.series_index,
            pubdate: book_data.pubdate,
            language: book_data.language.as_deref(),
        };
        let book_want = crate::wanted::book_media_want::BookWant::load(
            &self.pool,
            &book,
            &runtime.preferred_audio_language,
        )
        .await;
        let sources = match book_data.want_format.as_str() {
            "ebook" | "audiobook" | "both" => {
                crate::wanted::book_source::want_sources(payload.book_id, &book_data.want_format)
            }
            _ => Vec::new(),
        };
        let media_wants: Vec<(String, nats_jobs::MediaWant)> = sources
            .into_iter()
            .map(|(format, source_id)| (source_id, book_want.media_want(format == "audiobook")))
            .collect();

        // Enqueue a want_request for each media want
        let mut enqueued = 0;
        for (source_id, media_want) in &media_wants {
            nats_jobs::check_cancelled(&cancel)?;
            let want_request = nats_jobs::WantRequest {
                source_system: SOURCE_SYSTEM.to_string(),
                source_id: source_id.clone(),
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
