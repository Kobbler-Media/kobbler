//! Tests for the demand-driven stall watchdog and the HTTP-500 error
//! response surface that replaced the old ENDLIST-as-error pathway.
//!
//! The producer-side `StallState` watchdog was removed in favour of detection
//! on the playlist/segment serving path: a stall is "the player asked for a
//! segment that doesn't exist, and `STALL_THRESHOLD` later it still doesn't
//! exist". Errors are now signalled to the player via HTTP 500 with a
//! machine-readable `X-Stream-Error` header and a JSON body, instead of an
//! empty `#EXT-X-ENDLIST` playlist (which hls.js treated as a clean end of
//! stream and dispatched `ended` — exactly the wrong UX for an error).

use crate::common::*;
use api::streaming::session::{PlaybackSession, SessionError};
use reqwest::StatusCode;
use uuid::Uuid;

/// When `session.error` is set, the playlist endpoint must return:
///   - HTTP 500 (terminal — hls.js gives up after bounded retries)
///   - `Content-Type: application/json`
///   - `X-Stream-Error` header carrying the stable code
///   - JSON body `{"error": <code>, "message": <text>}`
///
/// Regression guard against the bug where errors were signalled with an
/// empty `#EXT-X-ENDLIST` playlist, which the player treats as success.
#[tokio::test]
async fn test_session_error_returns_500_with_machine_readable_code() {
    let server = TestServer::start_api_only().await;

    let session_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();

    let mut session = PlaybackSession::test_direct_play(
        session_id,
        user_id,
        std::path::PathBuf::from("/dev/null"),
    );
    session.error = Some(SessionError::new(
        "transcode_stalled",
        "Stalled waiting for segment 42 for 31.0s",
    ));
    server
        .app_state
        .playback_sessions
        .insert(session_id, session);

    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/playback/sessions/{session_id}/stream.m3u8"
        ))
        .await;

    assert_eq!(resp.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        resp.headers
            .get("x-stream-error")
            .expect("X-Stream-Error header missing")
            .to_str()
            .unwrap(),
        "transcode_stalled"
    );
    assert_eq!(
        resp.headers.get("content-type").unwrap().to_str().unwrap(),
        "application/json"
    );

    let body: serde_json::Value = serde_json::from_str(&resp.text()).expect("body must be JSON");
    assert_eq!(body["error"], "transcode_stalled");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or("")
            .contains("segment 42"),
        "message body should preserve the detail: {}",
        body["message"]
    );

    // The playlist body must NOT contain ENDLIST. The whole point of this
    // change is that errors no longer masquerade as "stream finished".
    assert!(
        !resp.text().contains("EXT-X-ENDLIST"),
        "error response must not contain #EXT-X-ENDLIST"
    );
}

/// Same shape, different code: the segment endpoint must also return 500
/// with the machine-readable surface when the session is in error state.
/// The frontend's hls.js error handler treats playlist and fragment errors
/// through the same `Hls.Events.ERROR` event, so consistency matters.
#[tokio::test]
async fn test_session_error_segment_endpoint_returns_500() {
    let server = TestServer::start_api_only().await;

    let session_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();

    let mut session = PlaybackSession::test_direct_play(
        session_id,
        user_id,
        std::path::PathBuf::from("/dev/null"),
    );
    session.error = Some(SessionError::new(
        "source_disconnected",
        "Live source disconnected",
    ));
    server
        .app_state
        .playback_sessions
        .insert(session_id, session);

    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/playback/sessions/{session_id}/segments/gen_0/seg_00005.m4s"
        ))
        .await;

    assert_eq!(resp.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        resp.headers
            .get("x-stream-error")
            .unwrap()
            .to_str()
            .unwrap(),
        "source_disconnected"
    );
}

/// A freshly-created session with no transcode and no error must NOT spuriously
/// stall just because a long time passes between requests. The original
/// false-positive watchdog killed a healthy cached-seek playback after 20s of
/// quiet because it polled segment counts producer-side. Demand-driven
/// detection only fires when the player actually asks for something missing.
///
/// We can't easily wait STALL_THRESHOLD (30s) in a unit test without slowing
/// the suite, so this test asserts the equivalent invariant: with no
/// `pending_segment` recorded and no `session.error`, the playlist handler
/// returns a non-error response (anything other than 500). An empty playlist
/// is a perfectly valid response here — what matters is that we don't see a
/// 500 with `transcode_stalled` purely from the passage of time.
#[tokio::test]
async fn test_quiet_session_does_not_spuriously_error() {
    let server = TestServer::start_api_only().await;

    let session_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();

    let session = PlaybackSession::test_direct_play(
        session_id,
        user_id,
        std::path::PathBuf::from("/dev/null"),
    );
    server
        .app_state
        .playback_sessions
        .insert(session_id, session);

    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/playback/sessions/{session_id}/stream.m3u8"
        ))
        .await;

    assert_ne!(
        resp.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "quiet session should not return a terminal error: body={}",
        resp.text()
    );
    assert!(
        resp.headers.get("x-stream-error").is_none(),
        "no X-Stream-Error header should be present on a healthy session"
    );
}
