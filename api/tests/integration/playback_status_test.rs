use crate::common::*;
use api::streaming::session::PlaybackSession;
use reqwest::StatusCode;
use serde_json::Value;
use std::path::PathBuf;
use uuid::Uuid;

async fn status_of(server: &TestServer, session: PlaybackSession) -> Value {
    let session_id = session.id;
    server
        .app_state
        .playback_sessions
        .insert(session_id, session);
    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/playback/sessions/{session_id}/status"
        ))
        .await;
    resp.assert_status(StatusCode::OK);
    resp.json::<Value>()
}

#[tokio::test]
async fn source_readability_tracks_the_source_kind() {
    let server = TestServer::start_api_only().await;
    let cases = [
        (
            "live IPTV session whose source is the upstream url",
            "http://provider.invalid/live/u/p/1905816.ts",
            true,
            "ready",
        ),
        (
            "file session whose file has gone",
            "/nonexistent/kobbler/film.mkv",
            false,
            "failed",
        ),
    ];
    for (name, source, is_live, expected) in cases {
        let mut session = PlaybackSession::test_direct_play(
            Uuid::new_v4(),
            Uuid::new_v4(),
            PathBuf::from(source),
        );
        session.is_live = is_live;
        let body = status_of(&server, session).await;
        assert_eq!(body["state"], expected, "{name}: {body}");
    }
}
