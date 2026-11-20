//! GET /api/v1/admin/playback/sessions/{id}/stream.ts — a transcoded session as one
//! MPEG-TS response, for DLNA renderers. A seek is a new session at a new start_time.

use super::streaming::{
    STALL_THRESHOLD, error_response, session_missing, session_pending_response, wait_out_restart,
};
use crate::AppState;
use crate::streaming::decision::PlaybackMethod;
use crate::streaming::{activity, dlna, mpegts};
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

pub async fn stream_ts(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Ok(session_id) = Uuid::parse_str(&session_id) else {
        return (StatusCode::BAD_REQUEST, "invalid session_id").into_response();
    };
    wait_out_restart(&state, session_id).await;
    {
        let Some(mut session) = state.playback_sessions.get_mut(&session_id) else {
            return session_missing(&state, session_id);
        };
        session.touch();
        if session.file_path.as_os_str().is_empty() {
            return session_pending_response();
        }
        if let Some(ref err) = session.error {
            return error_response(err.code, &err.message);
        }
        if session.decision.method == PlaybackMethod::DirectPlay {
            return (StatusCode::CONFLICT, "direct play session: use direct_url").into_response();
        }
    }

    let sessions = state.playback_sessions.clone();
    let body = activity::touching(
        mpegts::stream(sessions.clone(), session_id, STALL_THRESHOLD),
        sessions,
        session_id,
    );
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "video/mp2t")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(body))
        .unwrap();
    dlna::add_headers(response.headers_mut(), &headers, dlna::Resource::Transcoded);
    response
}
