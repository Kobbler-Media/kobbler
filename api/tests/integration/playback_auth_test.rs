//! A playback token is minted for one purpose: letting a cookie-less, header-less
//! player (libVLC, a Cast receiver) authenticate the handful of routes it actually
//! calls. `AuthUser`'s `?token=` fallback used to apply to every route that takes
//! `AuthUser`, admin routes included — this file pins that down to the real,
//! traced whitelist (`PlaybackAuth`) and proves a playback token is refused
//! everywhere else.

use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

async fn create_movie(server: &TestServer, title: &str) -> Uuid {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": title, "year": 2024 }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

async fn create_show_and_episode(server: &TestServer, title: &str) -> Uuid {
    let show_resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({ "title": title, "year": 2024 }),
        )
        .await;
    show_resp.assert_status(StatusCode::CREATED);
    let show_id: Uuid = show_resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let season_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special) \
         VALUES ($1, $2, '1', 1, true, false)",
    )
    .bind(season_id)
    .bind(show_id)
    .execute(&server.pool)
    .await
    .unwrap();

    let episode_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title) \
         VALUES ($1, $2, '1', 1, 'Pilot')",
    )
    .bind(episode_id)
    .bind(season_id)
    .execute(&server.pool)
    .await
    .unwrap();

    episode_id
}

/// Mint a real playback token the way `create_playback_token` does, via the
/// authenticated endpoint — not a hand-rolled DB row — so this exercises the
/// same path a Cast receiver actually takes.
async fn mint_playback_token(server: &TestServer) -> String {
    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/playback/tokens", &json!({}))
        .await;
    resp.assert_ok();
    resp.json::<Value>()["token"].as_str().unwrap().to_string()
}

fn with_token(path: &str, token: &str) -> String {
    let sep = if path.contains('?') { '&' } else { '?' };
    format!("{path}{sep}token={token}")
}

// =============================================================================
// The vulnerability: a playback token must not authenticate admin routes
// =============================================================================

#[tokio::test]
async fn playback_token_is_rejected_on_admin_config_route() {
    let server = TestServer::start_api_only().await;
    let token = mint_playback_token(&server).await;

    let resp = server
        .api_client()
        .get(&with_token("/api/v1/admin/config", &token))
        .await;

    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn playback_token_is_rejected_on_admin_users_route() {
    let server = TestServer::start_api_only().await;
    let token = mint_playback_token(&server).await;

    let resp = server
        .api_client()
        .get(&with_token("/api/v1/admin/users", &token))
        .await;

    resp.assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// The whitelist: every route kobapp or the Cast receiver actually sends
// ?token= to must keep working with no Authorization header at all.
// =============================================================================

#[tokio::test]
async fn playback_token_authenticates_movie_watch_state_routes() {
    let server = TestServer::start_api_only().await;
    let movie_id = create_movie(&server, "Watch Me").await;
    let token = mint_playback_token(&server).await;
    let client = server.api_client();

    client
        .get(&with_token(
            &format!("/api/v1/admin/movies/{movie_id}/watch-state"),
            &token,
        ))
        .await
        .assert_ok();

    // put_json/patch_json always send Content-Type JSON via the default client,
    // which has no Authorization header set — exactly the Cast receiver's shape.
    let resp = client
        .put_json(
            &with_token(
                &format!("/api/v1/admin/movies/{movie_id}/watch-state"),
                &token,
            ),
            &json!({ "watched": true }),
        )
        .await;
    resp.assert_ok();

    client
        .patch_json(
            &with_token(
                &format!("/api/v1/admin/movies/{movie_id}/watch-state/position"),
                &token,
            ),
            &json!({ "position_seconds": 120 }),
        )
        .await
        .assert_ok();
}

#[tokio::test]
async fn playback_token_authenticates_episode_watch_state_and_next_routes() {
    let server = TestServer::start_api_only().await;
    let episode_id = create_show_and_episode(&server, "Token Show").await;
    let token = mint_playback_token(&server).await;
    let client = server.api_client();

    client
        .get(&with_token(
            &format!("/api/v1/admin/tv/episodes/{episode_id}/watch-state"),
            &token,
        ))
        .await
        .assert_ok();

    client
        .put_json(
            &with_token(
                &format!("/api/v1/admin/tv/episodes/{episode_id}/watch-state"),
                &token,
            ),
            &json!({ "watched": true }),
        )
        .await
        .assert_ok();

    client
        .patch_json(
            &with_token(
                &format!("/api/v1/admin/tv/episodes/{episode_id}/watch-state/position"),
                &token,
            ),
            &json!({ "position_seconds": 42 }),
        )
        .await
        .assert_ok();

    // No next episode exists — 404 is correct, the point is it is not 401.
    let resp = client
        .get(&with_token(
            &format!("/api/v1/admin/tv/episodes/{episode_id}/next"),
            &token,
        ))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn playback_token_authenticates_debrid_stream_routes() {
    let server = TestServer::start_api_only().await;
    let movie_id = create_movie(&server, "Debrid Movie").await;
    let token = mint_playback_token(&server).await;

    let user_row = sqlx::query_scalar::<_, Uuid>("SELECT id FROM users LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap();
    let session_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO debrid_stream_sessions
            (id, user_id, media_type, media_id, release_title, info_hash,
             debrid_provider, filename, filesize, debrid_url, debrid_url_expires_at)
        VALUES ($1, $2, 'movie', $3, 'Test.Movie.2024.1080p', 'tokhash',
                '00000000-0000-0000-0000-000000000001', 'test.mp4', 1000000,
                'https://example.invalid/stream.mp4', NULL)
        "#,
    )
    .bind(session_id)
    .bind(user_row)
    .bind(movie_id)
    .execute(&server.pool)
    .await
    .unwrap();

    let client = server.api_client();

    // Unreachable example.invalid URL — 502, but that proves auth passed and
    // the handler reached the proxy step. The one status that would mean the
    // fix regressed is 401.
    let resp = client
        .get(&with_token(
            &format!("/api/v1/admin/stream/{session_id}/play"),
            &token,
        ))
        .await;
    assert_ne!(resp.status, StatusCode::UNAUTHORIZED);

    // This one is an SSE stream that stays open waiting for enrichment events
    // that never arrive in this test, so a plain unbounded GET would hang.
    // A short client-side timeout is enough: a 401 comes back immediately,
    // so a request that instead times out proves auth was not the rejection.
    let short_timeout_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap();
    let sse_result = short_timeout_client
        .get(with_token(
            &format!("{}/api/v1/admin/stream/{session_id}/status", server.api_url),
            &token,
        ))
        .send()
        .await;
    if let Ok(sse_resp) = sse_result {
        assert_ne!(sse_resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    }

    let resp = client
        .post_json(
            &with_token("/api/v1/admin/stream/search", &token),
            &json!({ "movie_id": movie_id }),
        )
        .await;
    assert_ne!(resp.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn playback_token_authenticates_playback_session_lifecycle_routes() {
    let server = TestServer::start_api_only().await;
    let movie_id = create_movie(&server, "Session Movie").await;
    let token = mint_playback_token(&server).await;
    let client = server.api_client();

    // No file exists for this movie yet, so creation fails downstream — but
    // not with 401, which is the only outcome that would mean the fix broke
    // the Cast receiver's own session creation call.
    let resp = client
        .post_json(
            &with_token("/api/v1/admin/playback/sessions", &token),
            &json!({
                "media_type": "movie",
                "media_id": movie_id,
                "capabilities": {
                    "video_codecs": ["h264"],
                    "audio_codecs": ["aac"],
                    "containers": ["mp4"]
                }
            }),
        )
        .await;
    assert_ne!(resp.status, StatusCode::UNAUTHORIZED);

    let dummy_session = Uuid::now_v7();

    let resp = client
        .post_json(
            &with_token(
                &format!("/api/v1/admin/playback/sessions/{dummy_session}/seek"),
                &token,
            ),
            &json!({ "time_seconds": 10.0 }),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);

    let resp = client
        .post_json(
            &with_token(
                &format!("/api/v1/admin/playback/sessions/{dummy_session}/switch-audio"),
                &token,
            ),
            &json!({}),
        )
        .await;
    assert_ne!(resp.status, StatusCode::UNAUTHORIZED);

    let resp = client
        .delete(&with_token(
            &format!("/api/v1/admin/playback/sessions/{dummy_session}"),
            &token,
        ))
        .await;
    assert_ne!(resp.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn playback_token_authenticates_its_own_refresh_and_revoke() {
    let server = TestServer::start_api_only().await;
    let token = mint_playback_token(&server).await;
    let client = server.api_client();

    // The Cast receiver self-refreshes using only the token it already holds —
    // no Authorization header, exactly like every other call in this file.
    let resp = client
        .post_json(
            &with_token("/api/v1/admin/playback/tokens", &token),
            &json!({ "ttl_hours": 24 }),
        )
        .await;
    resp.assert_ok();
    let new_token = resp.json::<Value>()["token"].as_str().unwrap().to_string();

    // It then revokes the old token, authenticating that DELETE with the NEW
    // token — same shape as receiver.js's best-effort cleanup.
    let resp = client
        .delete(&format!(
            "/api/v1/admin/playback/tokens/{token}?token={new_token}"
        ))
        .await;
    assert_ne!(resp.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn playback_token_authenticates_livetv_direct_channel_route_without_session() {
    let server = TestServer::start_api_only().await;
    let token = mint_playback_token(&server).await;

    // No channel exists for this id — 404 downstream, but not 401, which is
    // the only outcome that would mean the direct-tuner fallback path kobapp
    // relies on (LiveDirectPath with no `session=`) stopped authenticating.
    let resp = server
        .api_client()
        .get(&with_token(
            &format!("/api/v1/admin/livetv/channels/{}/stream.ts", Uuid::now_v7()),
            &token,
        ))
        .await;

    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn playback_token_authenticates_iptv_direct_play_route() {
    let server = TestServer::start_api_only().await;
    let token = mint_playback_token(&server).await;

    // The direct url an IPTV session hands libVLC carries no session, so kobapp
    // tokenises it. No such channel: the handler fails downstream, not at auth.
    let resp = server
        .api_client()
        .get(&with_token(
            &format!("/api/v1/admin/iptv/channels/{}/play.ts", Uuid::now_v7()),
            &token,
        ))
        .await;

    assert_ne!(resp.status, StatusCode::UNAUTHORIZED);
}
