//! Keep in library: finish a debrid stream's download and import the file as a real
//! library file for the session's movie or episode.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use tracing::error;
use uuid::Uuid;

use crate::AppState;
use crate::jobs::JobContext;
use crate::repositories::{debrid_cache_files as cache_files, debrid_keeps};
use crate::routes::AuthUser;
use crate::routes::admin::debrid_stream::session_owner;

pub const KEEP_JOB: &str = "debrid_keep";

fn db_error(e: impl std::fmt::Display) -> Response {
    error!(error = %e, "keep: database error");
    (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
}

/// POST /api/v1/admin/stream/{session_id}/keep
pub async fn keep(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(session_id): Path<Uuid>,
) -> Response {
    let media_type = match sqlx::query_scalar!(
        "SELECT media_type FROM debrid_stream_sessions
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
        session_id,
        session_owner(auth.user_id, &auth.role),
    )
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(t)) => t,
        Ok(None) => return (StatusCode::NOT_FOUND, "session not found").into_response(),
        Err(e) => return db_error(e),
    };
    if !matches!(media_type.as_str(), "movie" | "episode") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("cannot keep a {media_type} stream")})),
        )
            .into_response();
    }

    match debrid_keeps::begin(&state.pool, session_id).await {
        Ok(true) => {
            let _ = cache_files::set_pinned_for_session(&state.pool, session_id, true).await;
            let job = crate::jobs::enqueue(
                &state.pool,
                state.nats.publisher(),
                KEEP_JOB,
                json!({ "session_id": session_id }),
                JobContext::for_entity(
                    "debrid_session",
                    session_id,
                    "Keeping streamed file in library",
                ),
            )
            .await;
            match job {
                Ok(job_id) => {
                    let _ = debrid_keeps::set_job(&state.pool, session_id, job_id).await;
                }
                Err(e) => {
                    let message = format!("could not queue the keep: {e}");
                    let _ = debrid_keeps::set_state(
                        &state.pool,
                        session_id,
                        "failed",
                        0.0,
                        Some(&message),
                    )
                    .await;
                }
            }
        }
        Ok(false) => {}
        Err(e) => return db_error(e),
    }

    match debrid_keeps::get(&state.pool, session_id).await {
        Ok(Some(k)) => (
            StatusCode::ACCEPTED,
            Json(json!({ "job_id": k.job_id, "state": k.state })),
        )
            .into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "session not found").into_response(),
        Err(e) => db_error(e),
    }
}

/// GET /api/v1/admin/stream/{session_id}/keep
pub async fn keep_status(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(session_id): Path<Uuid>,
) -> Response {
    let owned = sqlx::query_scalar!(
        r#"SELECT EXISTS(
             SELECT 1 FROM debrid_stream_sessions
             WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)
           ) AS "exists!""#,
        session_id,
        session_owner(auth.user_id, &auth.role),
    )
    .fetch_one(&state.pool)
    .await;
    match owned {
        Ok(true) => {}
        Ok(false) => return (StatusCode::NOT_FOUND, "session not found").into_response(),
        Err(e) => return db_error(e),
    }
    match debrid_keeps::get(&state.pool, session_id).await {
        Ok(Some(k)) => {
            let mut body = json!({ "state": k.state, "progress": k.progress });
            if let Some(err) = k.error {
                body["error"] = json!(err);
            }
            Json(body).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "not kept").into_response(),
        Err(e) => db_error(e),
    }
}
