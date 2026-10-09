//! Repository for `media_cache_runs` — one row per invocation of the
//! single-pass `media_analyze` pipeline, recording trigger source, timing,
//! and per-branch outcomes.
//!
//! The backfill walker uses `pipeline_version` to find files whose cache
//! predates the current pipeline version and needs regeneration.

use super::Result;
use serde::Serialize;
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

/// Trigger source for a media_analyze run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggeredBy {
    /// Manual button press from the entity detail UI.
    Manual,
    /// Auto-ingest after a kobload callback (Phase 5+).
    AutoIngest,
    /// Backfill walker over the existing library (Phase 5+).
    Backfill,
}

impl TriggeredBy {
    pub fn as_str(self) -> &'static str {
        match self {
            TriggeredBy::Manual => "manual",
            TriggeredBy::AutoIngest => "auto_ingest",
            TriggeredBy::Backfill => "backfill",
        }
    }
}

/// One row in `media_cache_runs` as returned to API consumers.
#[derive(Debug, Clone, Serialize)]
pub struct MediaCacheRunRow {
    pub id: Uuid,
    pub file_id: Uuid,
    pub media_type: String,
    pub triggered_by: String,
    pub pipeline_version: i32,
    pub started_at: OffsetDateTime,
    pub completed_at: Option<OffsetDateTime>,
    pub duration_ms: Option<i64>,
    pub branch_results: JsonValue,
}

/// Insert a new run row at the start of a media_analyze job. Caller updates
/// the row later via [`finalize`].
pub async fn start_run(
    pool: &PgPool,
    file_id: Uuid,
    media_type: &str,
    triggered_by: TriggeredBy,
    pipeline_version: i32,
) -> Result<Uuid> {
    let id = Uuid::now_v7();
    start_run_with_id(
        pool,
        id,
        file_id,
        media_type,
        triggered_by,
        pipeline_version,
    )
    .await?;
    Ok(id)
}

/// Insert a run row with a caller-provided id. Idempotent: if the id already
/// exists (e.g. a manual UI flow generated it client-side and the worker is
/// re-entering after a NATS redeliver), the existing row is left untouched
/// so [`finalize`] can still find and patch it. Use this whenever the id is
/// owned by the caller; use [`start_run`] when the worker generates it.
///
/// Avoids the historical pattern of `let _ = start_run(...)` followed by
/// using the caller's id — that left an orphan row in the table because
/// `start_run` minted a fresh id internally.
pub async fn start_run_with_id(
    pool: &PgPool,
    id: Uuid,
    file_id: Uuid,
    media_type: &str,
    triggered_by: TriggeredBy,
    pipeline_version: i32,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO media_cache_runs
            (id, file_id, media_type, triggered_by, pipeline_version, started_at)
        VALUES ($1, $2, $3, $4, $5, NOW())
        ON CONFLICT (id) DO NOTHING
        "#,
    )
    .bind(id)
    .bind(file_id)
    .bind(media_type)
    .bind(triggered_by.as_str())
    .bind(pipeline_version)
    .execute(pool)
    .await?;
    Ok(())
}

/// Patch a run row with per-branch outcomes and timing once the analyze pass
/// completes.
pub async fn finalize(
    pool: &PgPool,
    id: Uuid,
    duration_ms: i64,
    branch_results: &JsonValue,
) -> Result<()> {
    sqlx::query!(
        r#"
        UPDATE media_cache_runs
           SET completed_at   = NOW(),
               duration_ms    = $2,
               branch_results = $3
         WHERE id = $1
        "#,
        id,
        duration_ms,
        branch_results,
    )
    .execute(pool)
    .await?;
    Ok(())
}
