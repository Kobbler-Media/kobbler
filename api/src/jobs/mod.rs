//! Job system for background tasks via NATS JetStream.
//!
//! All jobs are processed through NATS:
//! - Jobs are published to NATS JetStream for reliable delivery
//! - Job status is tracked in `job_status` table for UI queries
//! - Handlers implement the JobHandler trait for job execution

pub mod file_lock;
pub mod handlers;
pub mod media_root_io_gate;
pub mod timings;
pub mod worker;

pub use media_root_io_gate::MediaRootIoGate;
pub use timings::PhaseTimings;

use crate::repositories::job_status;
use nats_jobs::{JobStatusDb, Publisher};
use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;
use thiserror::Error;
use uuid::Uuid;

// Re-export useful types from nats-jobs
pub use nats_jobs::{JobContext, JobStatus, JobStatusRow};

/// Errors that can occur during job operations.
#[derive(Error, Debug)]
pub enum JobError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Repository error: {0}")]
    Repository(#[from] crate::repositories::RepositoryError),

    #[error("Job not found: {0}")]
    NotFound(Uuid),

    #[error("Handler not found for job type: {0}")]
    HandlerNotFound(String),

    #[error("Job execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Retryable error: {0}")]
    Retryable(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("NATS error: {0}")]
    Nats(String),

    #[error("Job cancelled")]
    Cancelled,
}

impl From<nats_jobs::JobError> for JobError {
    fn from(e: nats_jobs::JobError) -> Self {
        match e {
            nats_jobs::JobError::Cancelled => JobError::Cancelled,
            other => JobError::ExecutionFailed(other.to_string()),
        }
    }
}

impl JobError {
    /// Check if this error is retryable.
    pub fn is_retryable(&self) -> bool {
        match self {
            // Database errors might be transient (connection loss, etc.)
            JobError::Database(e) => {
                // Check for transient database errors
                let msg = e.to_string().to_lowercase();
                msg.contains("connection") || msg.contains("timeout") || msg.contains("temporarily")
            }
            // Explicitly marked retryable errors
            JobError::Retryable(_) => true,
            // NATS errors might be transient
            JobError::Nats(msg) => {
                let msg = msg.to_lowercase();
                msg.contains("connection") || msg.contains("timeout")
            }
            // Repository errors - delegate to underlying error
            JobError::Repository(e) => {
                // Check for transient database errors in the underlying error
                let msg = e.to_string().to_lowercase();
                msg.contains("connection") || msg.contains("timeout") || msg.contains("temporarily")
            }
            // Other errors are permanent
            JobError::NotFound(_)
            | JobError::HandlerNotFound(_)
            | JobError::ExecutionFailed(_)
            | JobError::Json(_)
            | JobError::Cancelled => false,
        }
    }

    /// Create a retryable error.
    pub fn retryable(msg: impl Into<String>) -> Self {
        JobError::Retryable(msg.into())
    }
}

pub type JobResult<T> = Result<T, JobError>;

/// Job event for real-time SSE updates.
#[derive(Debug, Clone, Serialize)]
pub struct JobEvent {
    pub job_id: Uuid,
    pub job_type: String,
    pub status: String,
    pub progress: Option<i32>,
    pub error: Option<String>,
    pub result: Option<Value>,
    pub description: Option<String>,
    pub entity_type: Option<String>,
    pub entity_id: Option<Uuid>,
    pub display: Option<Value>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    pub scheduled: bool,
}

/// Enqueue a job for processing via NATS.
///
/// The job is published to NATS JetStream and recorded in the `job_status`
/// table for UI visibility. Context is required to provide meaningful
/// information in the UI about what the job is doing.
pub async fn enqueue(
    pool: &PgPool,
    nats: &Publisher,
    job_type: &str,
    payload: Value,
    context: JobContext,
) -> JobResult<Uuid> {
    enqueue_internal(pool, nats, job_type, payload, context, false).await
}

/// Enqueue a routine job (hidden by default in UI).
///
/// Use this for periodic/automated jobs like directory scans triggered by
/// the scheduler or file watchers. These jobs are marked as `scheduled=true`
/// and hidden by default in the jobs UI.
pub async fn enqueue_routine(
    pool: &PgPool,
    nats: &Publisher,
    job_type: &str,
    payload: Value,
    context: JobContext,
) -> JobResult<Uuid> {
    enqueue_internal(pool, nats, job_type, payload, context, true).await
}

/// Internal job enqueueing with scheduled flag.
async fn enqueue_internal(
    pool: &PgPool,
    nats: &Publisher,
    job_type: &str,
    payload: Value,
    context: JobContext,
    scheduled: bool,
) -> JobResult<Uuid> {
    let job_id = nats
        .enqueue_job_with_context_and_options(
            "kobbler",
            job_type,
            payload.clone(),
            context.clone(),
            scheduled,
        )
        .await
        .map_err(|e| JobError::Nats(e.to_string()))?;

    // Write initial status to DB for UI visibility
    let _ = job_status::create_if_not_exists(
        pool,
        job_status::JobStatusInput {
            id: job_id,
            job_type: job_type.to_string(),
            status: "pending".to_string(),
            payload: payload.clone(),
            entity_type: context.entity_type.clone(),
            entity_id: context.entity_id,
            description: Some(context.description.clone()),
            display: context.display.clone(),
            scheduled,
        },
    )
    .await;

    tracing::info!(job_id = %job_id, job_type = %job_type, description = %context.description, scheduled = %scheduled, "Job enqueued");
    Ok(job_id)
}

/// Enqueue a job and broadcast a pending event to SSE listeners.
pub async fn enqueue_with_event(
    pool: &PgPool,
    nats: &Publisher,
    job_events: &tokio::sync::broadcast::Sender<JobEvent>,
    job_type: &str,
    payload: Value,
    context: JobContext,
    scheduled: bool,
) -> JobResult<Uuid> {
    let job_id =
        enqueue_internal(pool, nats, job_type, payload, context.clone(), scheduled).await?;

    let _ = job_events.send(JobEvent {
        job_id,
        job_type: job_type.to_string(),
        status: "pending".to_string(),
        progress: None,
        error: None,
        result: None,
        description: Some(context.description),
        entity_type: context.entity_type,
        entity_id: context.entity_id,
        display: context.display,
        created_at: time::OffsetDateTime::now_utc(),
        scheduled,
    });

    Ok(job_id)
}

/// Check if a job of the given type with matching payload is already active.
pub async fn has_active_job(
    pool: &PgPool,
    job_type: &str,
    payload_key: &str,
    payload_value: &str,
) -> JobResult<bool> {
    let db = JobStatusDb::new(pool.clone());
    Ok(db
        .has_active_job(job_type, payload_key, payload_value)
        .await?)
}

/// Cancel pending jobs that have a specific payload field present.
///
/// Used to supersede single-file scans when a full directory scan is enqueued.
pub async fn cancel_pending_jobs_with_field(
    pool: &PgPool,
    job_type: &str,
    match_field: &str,
    match_value: &str,
    required_field: &str,
) -> JobResult<u64> {
    let db = JobStatusDb::new(pool.clone());
    Ok(db
        .cancel_pending_jobs_with_field(job_type, match_field, match_value, required_field)
        .await?)
}

/// Count recent failures for a specific job type + entity within the last N hours.
/// Backfills use this to stop re-enqueuing permanently broken items.
pub async fn recent_failure_count(
    pool: &PgPool,
    job_type: &str,
    payload_key: &str,
    payload_value: &str,
    hours: i64,
) -> JobResult<i64> {
    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) AS "count!"
        FROM job_status
        WHERE job_type = $1
          AND status = 'failed'
          AND payload->>$4::text = $2
          AND completed_at > NOW() - make_interval(hours => $3::int)
        "#,
    )
    .bind(job_type)
    .bind(payload_value)
    .bind(hours as i32)
    .bind(payload_key)
    .fetch_one(pool)
    .await?;
    Ok(count)
}

/// Job types that are always treated as routine regardless of how they were enqueued.
/// These are hidden from the default jobs view (unless failed/running).
const ROUTINE_JOB_TYPES: &[&str] = &[
    // Search indexing
    "search_index",
    // Enrichment
    "book_enrich",
    "movie_enrich",
    "tv_show_enrich",
    "sports_league_enrich",
    "enrich_audiobook",
    "person_enrich",
    "author_enrich",
    "series_enrich",
    // Media root pipeline
    "media_root_scan",
    "media_root_interpret",
    "media_root_match",
    "media_root_enrich",
    "media_root_book_convert",
    "media_root_scan_check",
    "media_root_bulk_resolve",
    "media_root_write_sidecars",
    "media_root_write_images",
    "media_root_backfill",
    // Probing
    "media_probe",
    "audiobook_probe",
    "metadata_extraction",
    // Images/subtitles
    "entity_write_images",
    "subtitle_search",
    "forced_subtitle_check",
    // Periodic checks
    "person_sync_check",
    "series_sync_check",
    "author_sync_check",
    "hardcover_reconcile",
    "kepub_check",
    "movie_want_check",
    "book_want_check",
    "discover_sync",
    "stremio_sync",
    "sports_monitor",
    "iptv_sync",
    "iptv_sync_check",
    "iptv_channel_cleanup",
    "iptv_epg_cleanup",
    "iptv_health_check",
    "iptv_org_refresh",
    "iptv_recording_scheduler",
    "livetv_affinity",
    "livetv_guide_sync",
    "livetv_scan",
    "livetv_recording_scheduler",
    "livetv_retention",
    // Housekeeping
    "session_cleanup",
    "job_cleanup",
    "database_cleanup",
    "orphan_cleanup",
    "proxy_log_cleanup",
    "chat_turn_log_cleanup",
    "chat_cleanup",
    "chat_watch_cleanup",
    "temp_file_cleanup",
    "debrid_cache_cleanup",
    // Background processing
    "notification_send",
    "trakt_sync_watch_state",
    "keyframe_backfill",
    "audiobook_transcribe",
    "transcribe_backfill",
    "media_analyze_backfill",
    "chapter_detect",
    "chapter_redetect",
    "sidecar_rewrite",
    "metadata_write",
    "cross_format_sync",
    "file_availability",
    // Automated wants
    "tv_want",
    "movie_want",
    "book_want",
    "sports_want",
    "tv_monitor",
    "media_extract",
];

/// List recent jobs with pagination.
pub async fn list_jobs(
    pool: &PgPool,
    limit: i64,
    offset: i64,
    status_filter: Option<&str>,
    job_type_filter: Option<&str>,
    exclude_routine: bool,
) -> JobResult<Vec<JobStatusRow>> {
    let db = JobStatusDb::new(pool.clone());
    Ok(db
        .list_recent(
            limit,
            offset,
            status_filter,
            job_type_filter,
            exclude_routine,
            ROUTINE_JOB_TYPES,
        )
        .await?)
}

/// Get count of active (pending/running) jobs.
pub async fn running_count(pool: &PgPool, exclude_routine: bool) -> JobResult<i64> {
    let db = JobStatusDb::new(pool.clone());
    let counts = db.job_counts(exclude_routine, ROUTINE_JOB_TYPES).await?;
    Ok(counts.pending + counts.running + counts.retrying)
}

/// Get job counts by status.
pub async fn job_counts(pool: &PgPool, exclude_routine: bool) -> JobResult<nats_jobs::JobCounts> {
    let db = JobStatusDb::new(pool.clone());
    Ok(db.job_counts(exclude_routine, ROUTINE_JOB_TYPES).await?)
}

/// Get a job by ID.
pub async fn get_by_id(pool: &PgPool, job_id: Uuid) -> JobResult<Option<JobStatusRow>> {
    Ok(job_status::get_by_id(pool, job_id).await?)
}

// =============================================================================
// Audiobook Transcribe Helpers
// =============================================================================

/// Enqueue a background audiobook transcription job for a book.
///
/// Background transcriptions yield to media_extract between files and pause
/// 10 seconds between files to reduce system pressure.
pub async fn enqueue_audiobook_transcribe(
    pool: &PgPool,
    nats: &Publisher,
    book_id: Uuid,
    title: &str,
) {
    let display = fetch_job_display(pool, "book", book_id).await;
    let context = JobContext::for_entity(
        "book",
        book_id,
        format!("Transcribing audiobook '{}'", title),
    )
    .with_display_opt(display);
    if let Err(e) = enqueue(
        pool,
        nats,
        "audiobook_transcribe",
        serde_json::json!({"book_id": book_id}),
        context,
    )
    .await
    {
        tracing::warn!(error = %e, book_id = %book_id, "Failed to enqueue audiobook_transcribe job");
    }
}

/// Enqueue a manual (user-triggered) audiobook transcription job.
///
/// Manual transcriptions hold the I/O-heavy semaphore for the entire job and
/// skip inter-file delays, so they complete as fast as possible.
pub async fn enqueue_audiobook_transcribe_manual(
    pool: &PgPool,
    nats: &Publisher,
    book_id: Uuid,
    title: &str,
) {
    let display = fetch_job_display(pool, "book", book_id).await;
    let context = JobContext::for_entity(
        "book",
        book_id,
        format!("Transcribing audiobook '{}'", title),
    )
    .with_display_opt(display);
    if let Err(e) = enqueue(
        pool,
        nats,
        "audiobook_transcribe",
        serde_json::json!({"book_id": book_id, "manual": true}),
        context,
    )
    .await
    {
        tracing::warn!(error = %e, book_id = %book_id, "Failed to enqueue audiobook_transcribe job");
    }
}

// =============================================================================
// Metadata Write Helpers
// =============================================================================

/// Enqueue a metadata_write job for a book.
/// This writes DB metadata (title, authors, series, etc.) into ebook files.
pub async fn enqueue_metadata_write(pool: &PgPool, nats: &Publisher, book_id: Uuid, title: &str) {
    let display = fetch_job_display(pool, "book", book_id).await;
    let context =
        JobContext::for_entity("book", book_id, format!("Writing metadata to '{}'", title))
            .with_display_opt(display);
    if let Err(e) = enqueue(
        pool,
        nats,
        "metadata_write",
        serde_json::json!({"book_id": book_id.to_string()}),
        context,
    )
    .await
    {
        tracing::warn!(error = %e, book_id = %book_id, "Failed to enqueue metadata_write job");
    }
}

// =============================================================================
// Search Index Helpers
// =============================================================================

/// Enqueue a `search_index` job unless an identical one is still pending.
///
/// A pending job has not read the database yet, so it will pick up whatever
/// change triggered this call; a second copy would only repeat the same write.
/// Running jobs do not count: they may already have read the old rows. This
/// collapses bursts such as one `tv_show_updated` per episode of a season want.
async fn enqueue_search_index(
    pool: &PgPool,
    nats: &Publisher,
    entity_type: &str,
    entity_id: Uuid,
    display_type: &str,
    description: &str,
) {
    let pending = sqlx::query_scalar!(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM job_status
            WHERE job_type = 'search_index'
              AND status = 'pending'
              AND payload->>'entity_type' = $1
              AND payload->>'entity_id' = $2
        ) AS "exists!"
        "#,
        entity_type,
        entity_id.to_string(),
    )
    .fetch_one(pool)
    .await;
    if matches!(pending, Ok(true)) {
        tracing::debug!(entity_type, entity_id = %entity_id, "Search index job already pending, skipping");
        return;
    }

    let payload = serde_json::json!({
        "entity_type": entity_type,
        "entity_id": entity_id
    });
    let display = fetch_job_display(pool, display_type, entity_id).await;
    let context =
        JobContext::for_entity(display_type, entity_id, description).with_display_opt(display);
    if let Err(e) = enqueue_routine(pool, nats, "search_index", payload, context).await {
        tracing::warn!(error = %e, entity_type, entity_id = %entity_id, "Failed to enqueue search index job");
    }
}

/// Enqueue a search index job for a book.
/// This should be called whenever a book is created or updated.
pub async fn enqueue_book_index(pool: &PgPool, nats: &Publisher, book_id: Uuid) {
    enqueue_search_index(
        pool,
        nats,
        "book",
        book_id,
        "book",
        "Indexing book for search",
    )
    .await;
}

/// Enqueue a search index job for an author.
pub async fn enqueue_author_index(pool: &PgPool, nats: &Publisher, author_id: Uuid) {
    enqueue_search_index(
        pool,
        nats,
        "author",
        author_id,
        "author",
        "Indexing author for search",
    )
    .await;
}

/// Enqueue a search index job for a series.
pub async fn enqueue_series_index(pool: &PgPool, nats: &Publisher, series_id: Uuid) {
    enqueue_search_index(
        pool,
        nats,
        "series",
        series_id,
        "series",
        "Indexing series for search",
    )
    .await;
}

/// Enqueue a search index job for a movie.
pub async fn enqueue_movie_index(pool: &PgPool, nats: &Publisher, movie_id: Uuid) {
    enqueue_search_index(
        pool,
        nats,
        "movie",
        movie_id,
        "movie",
        "Indexing movie for search",
    )
    .await;
}

/// Enqueue a search index job for a TV show.
pub async fn enqueue_tv_show_index(pool: &PgPool, nats: &Publisher, show_id: Uuid) {
    enqueue_search_index(
        pool,
        nats,
        "tv_show",
        show_id,
        "tv_show",
        "Indexing TV show for search",
    )
    .await;
}

/// Enqueue one search index job covering every season and episode of a TV show.
pub async fn enqueue_tv_show_children_index(pool: &PgPool, nats: &Publisher, show_id: Uuid) {
    enqueue_search_index(
        pool,
        nats,
        "tv_show_children",
        show_id,
        "tv_show",
        "Indexing TV show seasons and episodes for search",
    )
    .await;
}

/// Enqueue a search index job for a TV season.
pub async fn enqueue_tv_season_index(pool: &PgPool, nats: &Publisher, season_id: Uuid) {
    enqueue_search_index(
        pool,
        nats,
        "tv_season",
        season_id,
        "tv_season",
        "Indexing TV season for search",
    )
    .await;
}

/// Enqueue a search index job for a TV episode.
pub async fn enqueue_tv_episode_index(pool: &PgPool, nats: &Publisher, episode_id: Uuid) {
    enqueue_search_index(
        pool,
        nats,
        "tv_episode",
        episode_id,
        "tv_episode",
        "Indexing TV episode for search",
    )
    .await;
}

/// Enqueue a search index job for a person.
pub async fn enqueue_person_index(pool: &PgPool, nats: &Publisher, person_id: Uuid) {
    enqueue_search_index(
        pool,
        nats,
        "person",
        person_id,
        "person",
        "Indexing person for search",
    )
    .await;
}

// =============================================================================
// Ebook Extract Helpers
// =============================================================================

/// Enqueue an ebook text extraction job for a book.
pub async fn enqueue_ebook_extract(pool: &PgPool, nats: &Publisher, book_id: Uuid, title: &str) {
    let display = fetch_job_display(pool, "book", book_id).await;
    let context =
        JobContext::for_entity("book", book_id, format!("Extracting text from '{}'", title))
            .with_display_opt(display);
    if let Err(e) = enqueue(
        pool,
        nats,
        "ebook_extract",
        serde_json::json!({"book_id": book_id}),
        context,
    )
    .await
    {
        tracing::warn!(error = %e, book_id = %book_id, "Failed to enqueue ebook_extract job");
    }
}

// =============================================================================
// Cross-Format Sync Helpers
// =============================================================================

/// Enqueue a cross-format sync job (align audio timestamps ↔ ebook CFIs).
pub async fn enqueue_cross_format_sync(
    pool: &PgPool,
    nats: &Publisher,
    book_id: Uuid,
    title: &str,
) {
    let display = fetch_job_display(pool, "book", book_id).await;
    let context = JobContext::for_entity(
        "book",
        book_id,
        format!("Syncing audio/ebook positions for '{}'", title),
    )
    .with_display_opt(display);
    if let Err(e) = enqueue(
        pool,
        nats,
        "cross_format_sync",
        serde_json::json!({"book_id": book_id}),
        context,
    )
    .await
    {
        tracing::warn!(error = %e, book_id = %book_id, "Failed to enqueue cross_format_sync job");
    }
}

/// Enqueue a chapter detection job for a book.
pub async fn enqueue_chapter_detect(pool: &PgPool, nats: &Publisher, book_id: Uuid, title: &str) {
    let display = fetch_job_display(pool, "book", book_id).await;
    let context = JobContext::for_entity(
        "book",
        book_id,
        format!("Detecting chapters for '{}'", title),
    )
    .with_display_opt(display);
    if let Err(e) = enqueue(
        pool,
        nats,
        "chapter_detect",
        serde_json::json!({"book_id": book_id}),
        context,
    )
    .await
    {
        tracing::warn!(error = %e, book_id = %book_id, "Failed to enqueue chapter_detect job");
    }
}

/// Enqueue a chapter re-detection job for a book.
///
/// Uses combined silence + transcript analysis when transcripts are available,
/// falling back to silence-only detection.
pub async fn enqueue_chapter_redetect(pool: &PgPool, nats: &Publisher, book_id: Uuid, title: &str) {
    let display = fetch_job_display(pool, "book", book_id).await;
    let context = JobContext::for_entity(
        "book",
        book_id,
        format!("Re-detecting chapters for '{}'", title),
    )
    .with_display_opt(display);
    if let Err(e) = enqueue(
        pool,
        nats,
        "chapter_redetect",
        serde_json::json!({"book_id": book_id}),
        context,
    )
    .await
    {
        tracing::warn!(error = %e, book_id = %book_id, "Failed to enqueue chapter_redetect job");
    }
}

// =============================================================================
// Display Metadata Helper
// =============================================================================

/// Fetch structured display metadata for a job's entity.
///
/// Returns a JSON object with `title`, optional `subtitle`, and optional
/// `image_url` that the frontend can render directly without extra API calls.
pub async fn fetch_job_display(pool: &PgPool, entity_type: &str, entity_id: Uuid) -> Option<Value> {
    match entity_type {
        "book" => fetch_book_display(pool, entity_id).await,
        "movie" => fetch_movie_display(pool, entity_id).await,
        "tv_show" => fetch_tv_show_display(pool, entity_id).await,
        "tv_season" => fetch_tv_season_display(pool, entity_id).await,
        "tv_episode" => fetch_tv_episode_display(pool, entity_id).await,
        "author" => fetch_author_display(pool, entity_id).await,
        "person" => fetch_person_display(pool, entity_id).await,
        "series" => fetch_series_display(pool, entity_id).await,
        "media_root" => fetch_media_root_display(pool, entity_id).await,
        _ => None,
    }
}

async fn fetch_book_display(pool: &PgPool, id: Uuid) -> Option<Value> {
    let row = sqlx::query!(
        r#"
        SELECT b.title,
               string_agg(a.name, ', ' ORDER BY a.name) AS "authors?",
               (b.images != '{}'::jsonb) AS "has_image!"
        FROM books b
        LEFT JOIN book_authors ba ON ba.book_id = b.id
        LEFT JOIN authors a ON a.id = ba.author_id
        WHERE b.id = $1
        GROUP BY b.id, b.title
        "#,
        id,
    )
    .fetch_optional(pool)
    .await
    .ok()??;

    let image_url = if row.has_image {
        Some(format!("/api/v1/library/books/{}/cover", id))
    } else {
        None
    };

    Some(serde_json::json!({
        "title": row.title,
        "subtitle": row.authors,
        "image_url": image_url,
    }))
}

async fn fetch_movie_display(pool: &PgPool, id: Uuid) -> Option<Value> {
    let row = sqlx::query!(
        r#"SELECT title, year, (images->>'poster') AS "poster_url?" FROM movies WHERE id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await
    .ok()??;

    let image_url = row
        .poster_url
        .as_ref()
        .map(|_| format!("/api/v1/admin/movies/{}/poster", id));

    Some(serde_json::json!({
        "title": row.title,
        "subtitle": row.year.map(|y| y.to_string()),
        "image_url": image_url,
    }))
}

async fn fetch_tv_show_display(pool: &PgPool, id: Uuid) -> Option<Value> {
    let row = sqlx::query!(
        r#"SELECT title, year, (images->>'poster') AS "poster_url?" FROM tv_shows WHERE id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await
    .ok()??;

    let image_url = row
        .poster_url
        .as_ref()
        .map(|_| format!("/api/v1/admin/tv/shows/{}/poster", id));

    Some(serde_json::json!({
        "title": row.title,
        "subtitle": row.year.map(|y| y.to_string()),
        "image_url": image_url,
    }))
}

async fn fetch_tv_season_display(pool: &PgPool, id: Uuid) -> Option<Value> {
    let row = sqlx::query!(
        r#"
        SELECT s.title AS "show_title!",
               ts.season_identifier,
               ts.title AS "season_title?",
               (ts.images->>'poster') AS "poster_url?"
        FROM tv_seasons ts
        JOIN tv_shows s ON s.id = ts.show_id
        WHERE ts.id = $1
        "#,
        id,
    )
    .fetch_optional(pool)
    .await
    .ok()??;

    let subtitle = row
        .season_title
        .unwrap_or_else(|| row.season_identifier.clone());

    let image_url = row
        .poster_url
        .as_ref()
        .map(|_| format!("/api/v1/admin/tv/seasons/{}/poster", id));

    Some(serde_json::json!({
        "title": row.show_title,
        "subtitle": subtitle,
        "image_url": image_url,
    }))
}

async fn fetch_tv_episode_display(pool: &PgPool, id: Uuid) -> Option<Value> {
    let row = sqlx::query!(
        r#"
        SELECT s.title AS "show_title!",
               ts.season_identifier,
               te.episode_identifier,
               te.title AS "episode_title?",
               (s.images->>'poster') AS "show_poster_url?"
        FROM tv_episodes te
        JOIN tv_seasons ts ON ts.id = te.season_id
        JOIN tv_shows s ON s.id = ts.show_id
        WHERE te.id = $1
        "#,
        id,
    )
    .fetch_optional(pool)
    .await
    .ok()??;

    let subtitle = if let Some(ep_title) = &row.episode_title {
        format!(
            "{} {} · {}",
            row.season_identifier, row.episode_identifier, ep_title
        )
    } else {
        format!("{} {}", row.season_identifier, row.episode_identifier)
    };

    let image_url = row
        .show_poster_url
        .as_ref()
        .map(|_| format!("/api/v1/admin/tv/shows/{}/poster", id));

    Some(serde_json::json!({
        "title": row.show_title,
        "subtitle": subtitle,
        "image_url": image_url,
    }))
}

async fn fetch_author_display(pool: &PgPool, id: Uuid) -> Option<Value> {
    let row = sqlx::query!(
        r#"SELECT name, images->>'photo' AS "image_url?" FROM authors WHERE id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await
    .ok()??;

    Some(serde_json::json!({
        "title": row.name,
        "image_url": row.image_url,
    }))
}

async fn fetch_person_display(pool: &PgPool, id: Uuid) -> Option<Value> {
    let row = sqlx::query!(
        r#"SELECT name, (images != '{}'::jsonb) AS "has_image!" FROM people WHERE id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await
    .ok()??;

    let image_url = if row.has_image {
        Some(format!("/api/v1/admin/people/{}/image", id))
    } else {
        None
    };

    Some(serde_json::json!({
        "title": row.name,
        "image_url": image_url,
    }))
}

async fn fetch_series_display(pool: &PgPool, id: Uuid) -> Option<Value> {
    let row = sqlx::query!(r#"SELECT name FROM series WHERE id = $1"#, id,)
        .fetch_optional(pool)
        .await
        .ok()??;

    Some(serde_json::json!({
        "title": row.name,
    }))
}

async fn fetch_media_root_display(pool: &PgPool, id: Uuid) -> Option<Value> {
    let row = sqlx::query!(
        r#"SELECT name, media_type FROM media_roots WHERE id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await
    .ok()??;

    Some(serde_json::json!({
        "title": row.name,
        "subtitle": row.media_type,
    }))
}

/// Cancel a pending or running job by marking it as failed.
/// Cancel a pending or running job. Returns the job_type if cancelled, None if not found.
pub async fn cancel_job(pool: &PgPool, job_id: Uuid) -> JobResult<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        "UPDATE job_status SET status = 'failed', error = 'Cancelled by user', completed_at = NOW() WHERE id = $1 AND status IN ('pending', 'running') RETURNING job_type"
    )
    .bind(job_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(jt,)| jt))
}

/// Delete a job by ID.
pub async fn delete_job(pool: &PgPool, job_id: Uuid) -> JobResult<bool> {
    Ok(job_status::delete(pool, job_id).await?)
}

/// Delete all finished (non-pending, non-running) jobs.
pub async fn delete_finished_jobs(pool: &PgPool) -> JobResult<u64> {
    Ok(job_status::delete_finished(pool).await?)
}

/// Update job progress and broadcast event.
pub async fn update_progress(
    pool: &PgPool,
    job_events: &tokio::sync::broadcast::Sender<JobEvent>,
    job_id: Uuid,
    job_type: &str,
    progress: i32,
    result: Option<Value>,
) -> JobResult<()> {
    // Update database
    job_status::update_progress(pool, job_id, progress, result.as_ref()).await?;

    // Use the job's real scheduled flag so routine jobs don't flash as active in the UI.
    let scheduled = job_status::get_by_id(pool, job_id)
        .await
        .ok()
        .flatten()
        .is_some_and(|row| row.scheduled);

    // Broadcast progress event
    let _ = job_events.send(JobEvent {
        job_id,
        job_type: job_type.to_string(),
        status: "running".to_string(),
        progress: Some(progress),
        error: None,
        result: None,
        description: None,
        entity_type: None,
        entity_id: None,
        display: None,
        created_at: time::OffsetDateTime::now_utc(),
        scheduled,
    });

    Ok(())
}
