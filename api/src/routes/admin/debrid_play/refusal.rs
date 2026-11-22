use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use tracing::warn;
use uuid::Uuid;

use super::upstream::fail_session;
use crate::AppState;
use crate::debrid_outcomes::{self, StreamRelease};
use crate::repositories::debrid_stream_sessions;
use crate::routes::admin::debrid_errors::StreamFailure;
use crate::streaming::download_meter::DownloadMeter;
use crate::streaming::resume;

pub(super) struct Refusal<'a> {
    pub session_id: Uuid,
    pub release: &'a StreamRelease,
    /// Shared with the session's cache-ahead, so the session reports the release once.
    pub meter: Option<&'a DownloadMeter>,
    pub status: reqwest::StatusCode,
    pub code: Option<&'a str>,
}

/// The player's answer to a host that would not serve a read. Only damage to the release
/// and a link that is refused outright are the release's fault; a busy engine, an expired
/// token and a server error are not.
pub(super) async fn answer(state: &AppState, r: Refusal<'_>) -> Response {
    if let Some(code) = r.code.filter(|c| resume::dead_code(c)) {
        let failure = StreamFailure::NotCached;
        fail_session(state, r.session_id, failure.code(), failure.message());
        report_once(state, &r, code).await;
        return (StatusCode::BAD_GATEWAY, "the release is damaged").into_response();
    }
    if r.code == Some(resume::FORGOTTEN_CODE) {
        if let Err(e) = debrid_stream_sessions::expire_urls(&state.pool, r.session_id).await {
            warn!(session_id = %r.session_id, error = %e, "failed to expire the forgotten URL");
        }
        return (StatusCode::BAD_GATEWAY, "upstream error").into_response();
    }
    let refused_outright = r.status.is_client_error()
        && r.status != reqwest::StatusCode::RANGE_NOT_SATISFIABLE
        && r.status != reqwest::StatusCode::TOO_MANY_REQUESTS;
    if refused_outright {
        fail_session(
            state,
            r.session_id,
            "upstream_refused",
            &format!("The debrid host refused the file ({})", r.status),
        );
        report_once(state, &r, "upstream_failed").await;
    }
    (StatusCode::BAD_GATEWAY, "upstream error").into_response()
}

async fn report_once(state: &AppState, r: &Refusal<'_>, reason: &str) {
    if r.meter.is_some_and(DownloadMeter::first_failure_report) {
        debrid_outcomes::report_failure(state, r.release, reason).await;
    }
}
