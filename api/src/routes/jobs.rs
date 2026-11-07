//! Job queue API routes.

use crate::AppState;
use crate::jobs::{self, JobStatusRow};
use crate::routes::AuthUser;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct ListJobsQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    pub status: Option<String>,
    pub job_type: Option<String>,
    #[serde(default = "default_true")]
    pub exclude_routine: bool,
}

fn default_limit() -> i64 {
    50
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
pub struct ListJobsResponse {
    pub jobs: Vec<JobStatusRow>,
    pub total: i64,
}

#[derive(Debug, Serialize)]
pub struct RunningCountResponse {
    pub count: i64,
}

#[derive(Debug, Serialize)]
pub struct JobCountsResponse {
    pub pending: i64,
    pub running: i64,
    pub completed: i64,
    pub failed: i64,
    pub retrying: i64,
}

/// GET /api/v1/admin/jobs - List jobs with pagination
pub async fn list_jobs(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<ListJobsQuery>,
) -> impl IntoResponse {
    let jobs = match jobs::list_jobs(
        &state.pool,
        query.limit,
        query.offset,
        query.status.as_deref(),
        query.job_type.as_deref(),
        query.exclude_routine,
    )
    .await
    {
        Ok(jobs) => jobs,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    // Get total from job counts (use same exclude_routine filter)
    let total = match jobs::job_counts(&state.pool, query.exclude_routine).await {
        Ok(counts) => {
            counts.pending + counts.running + counts.completed + counts.failed + counts.retrying
        }
        Err(_) => jobs.len() as i64,
    };

    Json(ListJobsResponse { jobs, total }).into_response()
}

/// GET /api/v1/admin/jobs/{job_id} - Get a single job by ID
pub async fn get_job(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(job_id): Path<Uuid>,
) -> impl IntoResponse {
    match jobs::get_by_id(&state.pool, job_id).await {
        Ok(Some(job)) => Json(job).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Job not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct ExcludeRoutineQuery {
    #[serde(default = "default_true")]
    pub exclude_routine: bool,
}

/// GET /api/v1/admin/jobs/running - Get count of running/pending jobs
pub async fn running_count(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<ExcludeRoutineQuery>,
) -> impl IntoResponse {
    match jobs::running_count(&state.pool, query.exclude_routine).await {
        Ok(count) => Json(RunningCountResponse { count }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// GET /api/v1/admin/jobs/counts - Get job counts by status
pub async fn job_counts(State(state): State<AppState>, _auth: AuthUser) -> impl IntoResponse {
    match jobs::job_counts(&state.pool, false).await {
        Ok(counts) => Json(JobCountsResponse {
            pending: counts.pending,
            running: counts.running,
            completed: counts.completed,
            failed: counts.failed,
            retrying: counts.retrying,
        })
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

#[derive(Debug, Serialize)]
pub struct RetryJobResponse {
    pub new_job_id: Uuid,
    pub message: String,
}

/// DELETE /api/v1/admin/jobs/completed - Delete all finished (non-pending, non-running) jobs
pub async fn delete_completed_jobs(
    State(state): State<AppState>,
    auth: AuthUser,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }

    match jobs::delete_finished_jobs(&state.pool).await {
        Ok(count) => Json(serde_json::json!({
            "deleted": count,
            "message": format!("Cleared {} job(s)", count)
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// DELETE /api/v1/admin/jobs/{job_id} - Delete a single job
pub async fn delete_job(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(job_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }

    match jobs::delete_job(&state.pool, job_id).await {
        Ok(true) => Json(serde_json::json!({
            "deleted": true,
            "message": format!("Job {} deleted", job_id)
        }))
        .into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Job not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// Marks the job cancelled, trips a running handler's token and purges its
/// NATS message so a pending one is never picked up.
pub(crate) async fn cancel(state: &AppState, job_id: Uuid) -> jobs::JobResult<Option<String>> {
    let job_type = jobs::cancel_job(&state.pool, job_id).await?;
    if let Some(job_type) = &job_type {
        state.cancel_registry.cancel(job_id);
        nats_jobs::purge_nats_job(&state.nats.client, "kobbler", job_type, job_id).await;
    }
    Ok(job_type)
}

/// POST /api/v1/admin/jobs/{job_id}/cancel - Cancel a pending or running job
pub async fn cancel_job(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(job_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }

    match cancel(&state, job_id).await {
        Ok(Some(_)) => Json(serde_json::json!({
            "cancelled": true,
            "message": format!("Job {} cancelled", job_id)
        }))
        .into_response(),
        Ok(None) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "Job not found or already completed"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// POST /api/v1/admin/jobs/{job_id}/retry - Retry a failed job
pub async fn retry_job(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(job_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }

    // Get the failed job from database
    let job = match jobs::get_by_id(&state.pool, job_id).await {
        Ok(Some(j)) => j,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Job not found"})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    // Only allow retry of failed jobs
    if job.status != "failed" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!("Cannot retry job with status '{}'. Only failed jobs can be retried.", job.status)
            })),
        )
            .into_response();
    }

    // Get the payload
    let payload = match job.payload {
        Some(p) => p,
        None => serde_json::json!({}),
    };

    // Build context from original job, updating description to indicate retry
    let display = match (&job.entity_type, job.entity_id) {
        (Some(et), Some(eid)) => crate::jobs::fetch_job_display(&state.pool, et, eid).await,
        _ => job.display.clone(),
    };
    let context = nats_jobs::JobContext {
        entity_type: job.entity_type.clone(),
        entity_id: job.entity_id,
        description: job
            .description
            .as_ref()
            .map(|d| format!("Retry: {}", d))
            .unwrap_or_else(|| format!("Retry of {} job", job.job_type)),
        display,
    };

    // Enqueue a new job with the same type and payload, preserving context
    let new_job_id = match jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        &job.job_type,
        payload,
        context,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to enqueue retry: {}", e)})),
            )
                .into_response();
        }
    };

    Json(RetryJobResponse {
        new_job_id,
        message: format!("Job {} retried as new job {}", job_id, new_job_id),
    })
    .into_response()
}
