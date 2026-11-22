//! A torrent and an NZB are one kind of release to kobbler: each session, cache file,
//! outcome, preference and on-disk offer is keyed by the release's `release_key`, which an
//! NZB only learns from kobload's resolve.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::{Seen, UpstreamPlan, WholeBody};
use crate::common::*;
use api::jobs::handlers::{DebridDownloadAheadHandler, DebridKeepHandler};
use api::jobs::worker::JobHandler;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, PartialEq)]
struct SessionKeys {
    release_key: Option<String>,
    nzb_ref: Option<Value>,
    cache_path: Option<String>,
}

async fn session_keys(rig: &Rig, session: Uuid) -> SessionKeys {
    sqlx::query_as(
        "SELECT release_key, nzb_ref, cache_path FROM debrid_stream_sessions WHERE id = $1",
    )
    .bind(session)
    .fetch_one(&rig.server.pool)
    .await
    .unwrap()
}

async fn cache_keys(rig: &Rig) -> Vec<(Option<String>, String)> {
    sqlx::query_as("SELECT release_key, file_path FROM debrid_cache_files ORDER BY file_path")
        .fetch_all(&rig.server.pool)
        .await
        .unwrap()
}

async fn admin(rig: &Rig) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE role = 'admin' LIMIT 1")
        .fetch_one(&rig.server.pool)
        .await
        .unwrap()
}

async fn on_disk_offer(rig: &Rig, media_type: &str, media_id: Uuid) -> Value {
    let resp = rig
        .server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/stream/local?media_type={media_type}&media_id={media_id}"
        ))
        .await;
    resp.assert_ok();
    resp.json()
}

async fn search(rig: &Rig, body: Value) -> Value {
    let resp = rig
        .server
        .admin_client()
        .post_json("/api/v1/admin/stream/search", &body)
        .await;
    resp.assert_ok();
    resp.json()
}

async fn wait_for_requests(rig: &Rig, suffix: &str, count: usize) -> Vec<Value> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let seen = rig.kobload_requests(suffix).await;
        if seen.len() >= count || std::time::Instant::now() > deadline {
            return seen;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The resolve kobload was asked for names the release the way the client did, and never
/// by the old torrent-only fields.
fn assert_resolve_names(source: Source, request: &Value, release_key: Value) {
    assert_eq!(request["release_key"], release_key, "{source:?}: {request}");
    assert_eq!(
        request["nzb_ref"],
        source.nzb_ref(),
        "{source:?}: {request}"
    );
    let text = request.to_string();
    assert!(
        !text.contains("info_hash") && !text.contains("nzb_url"),
        "{source:?}: {request}"
    );
}

#[tokio::test]
async fn torrent_and_usenet_sessions_key_by_the_release_key() {
    for source in Source::ALL {
        let key = source.key();
        let rig = Rig::start_as(
            source,
            UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec()),
            "film.mkv",
        )
        .await;
        rig.mount_search(json!([rig.option_from(source)])).await;
        let roots = rig.media_roots().await;
        let episodes = season_of(&rig.server, 2).await;
        let body = rig.resolve_body_from(source, "episode", episodes[0]);

        let first = session_id_of(&rig.resolve_with(&body).await);
        let resolves = rig.kobload_requests("/stream/resolve").await;
        assert_resolve_names(source, &resolves[0], source.listed_key());

        rig.wait_for_complete(key).await;
        process_download(&rig.server, first, &rig.cache_file(key)).await;
        let first_keys = session_keys(&rig, first).await;
        assert_eq!(
            first_keys,
            SessionKeys {
                release_key: Some(key.into()),
                nzb_ref: Some(source.nzb_ref()).filter(|r| !r.is_null()),
                cache_path: Some(rig.cache_file(key).to_string_lossy().into()),
            },
            "{source:?}"
        );
        assert_eq!(
            cache_keys(&rig).await,
            [(
                Some(key.to_string()),
                rig.cache_file(key).to_string_lossy().into()
            )],
            "{source:?}"
        );

        rig.server
            .admin_client()
            .post_json(
                &format!("/api/v1/admin/stream/{first}/outcome"),
                &json!({ "ok": true }),
            )
            .await
            .assert_status(StatusCode::NO_CONTENT);
        let outcomes = wait_for_requests(&rig, "/stream/outcome", 1).await;
        assert_eq!(outcomes.len(), 1, "{source:?}: the outcome was dropped");
        assert_eq!(outcomes[0]["release_key"], key, "{source:?}");

        let offer = on_disk_offer(&rig, "episode", episodes[0]).await;
        assert_eq!(offer["release_key"], key, "{source:?}: {offer}");
        assert_eq!(offer["source_type"], source.source_type(), "{offer}");
        assert_eq!(offer["nzb_ref"], source.nzb_ref(), "{offer}");

        let replay = search(&rig, json!({ "episode_id": episodes[0] })).await;
        assert_eq!(replay["preferred_release_key"], key, "{source:?}: {replay}");
        assert!(replay.get("preferred_info_hash").is_none(), "{replay}");
        search(&rig, json!({ "episode_id": episodes[1] })).await;
        let searches = rig.kobload_requests("/stream/search").await;
        for (n, request) in searches.iter().enumerate() {
            assert_eq!(
                request["prefer"]["release_key"], key,
                "{source:?}: search {n} lost the replay/binge preference: {request}"
            );
        }

        end_playback(&rig.server, first).await;
        let second = session_id_of(&rig.resolve_with(&body).await);
        assert_ne!(second, first);
        let second_keys = session_keys(&rig, second).await;
        assert_eq!(second_keys.release_key.as_deref(), Some(key), "{source:?}");
        assert_eq!(
            second_keys.cache_path, first_keys.cache_path,
            "{source:?}: the second session did not find the first one's bytes"
        );
        let analysed: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM debrid_files WHERE cache_path = $1")
                .bind(first_keys.cache_path.as_deref())
                .fetch_one(&rig.server.pool)
                .await
                .unwrap();
        assert_eq!(
            analysed, 1,
            "{source:?}: one download, one debrid_files row"
        );
        assert_eq!(cache_keys(&rig).await.len(), 1, "{source:?}");

        rig.server
            .admin_client()
            .post_json(&format!("/api/v1/admin/stream/{second}/keep"), &json!({}))
            .await
            .assert_status(StatusCode::ACCEPTED);
        rig.server
            .process_pending_jobs(&DebridKeepHandler::new(rig.server.app_state.clone()))
            .await;
        let kept: Option<(String, Value)> = sqlx::query_as(
            "SELECT f.file_path, f.source_metadata FROM tv_episode_files f
             JOIN tv_episode_file_episodes l ON l.episode_file_id = f.id
             WHERE l.episode_id = $1",
        )
        .bind(episodes[0])
        .fetch_optional(&rig.server.pool)
        .await
        .unwrap();
        let (kept_path, provenance) = kept.unwrap_or_else(|| panic!("{source:?}: not kept"));
        assert!(kept_path.starts_with(&*roots.path().to_string_lossy()));
        assert_eq!(provenance["info_hash"], key, "{source:?}: {provenance}");
        assert!(cache_keys(&rig).await.is_empty(), "{source:?}");
    }
}

#[tokio::test]
async fn an_expired_session_re_resolves_with_its_stored_release() {
    for source in Source::ALL {
        let rig = Rig::start_as(source, UpstreamPlan::new(noise(MB)), "film.mp4").await;
        let movie = rig.movie().await;
        let session = session_id_of(
            &rig.resolve_with(&rig.resolve_body_from(source, "movie", movie))
                .await,
        );
        rig.wait_for_complete(source.key()).await;
        end_playback(&rig.server, session).await;
        sqlx::query(
            "UPDATE debrid_stream_sessions
             SET cache_path = NULL, debrid_url_expires_at = '2020-01-01T00:00:00Z' WHERE id = $1",
        )
        .bind(session)
        .execute(&rig.server.pool)
        .await
        .unwrap();

        let played = play(&rig.server, session, "bytes=0-1023").await;
        assert_eq!(played.status, StatusCode::PARTIAL_CONTENT, "{source:?}");

        let resolves = rig.kobload_requests("/stream/resolve").await;
        assert_eq!(resolves.len(), 2, "{source:?}");
        assert_resolve_names(source, &resolves[1], json!(source.key()));
    }
}

#[tokio::test]
async fn warm_and_ahead_downloads_land_under_the_release_key() {
    for source in Source::ALL {
        let key = source.key();
        let data = noise(2 * MB);

        let rig = Rig::start_as(source, UpstreamPlan::new(data.clone()), "ep.mkv").await;
        rig.mount_search(json!([rig.option_from(source)])).await;
        let episodes = season_of(&rig.server, 2).await;
        let user = admin(&rig).await;
        DebridDownloadAheadHandler::new(rig.server.app_state.clone())
            .execute(
                Uuid::now_v7(),
                json!({ "user_id": user, "episode_id": episodes[1] }),
                CancellationToken::new(),
            )
            .await
            .unwrap_or_else(|e| panic!("{source:?}: {e}"));
        assert!(
            std::fs::read(rig.cache_file(key)).ok().as_deref() == Some(&data[..]),
            "{source:?}: the download ahead is not under its release key"
        );
        let resolves = rig.kobload_requests("/stream/resolve").await;
        assert_resolve_names(source, &resolves[0], source.listed_key());
        assert_eq!(resolves[0]["priority"], "warm");
        let offer = on_disk_offer(&rig, "episode", episodes[1]).await;
        assert_eq!(offer["release_key"], key, "{source:?}: {offer}");

        let rig = Rig::start_as(
            source,
            UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec()),
            "film.mkv",
        )
        .await;
        rig.mount_search(json!([rig.option_from(source)])).await;
        let movie = rig.movie().await;
        let warm = rig
            .server
            .admin_client()
            .post_json(
                "/api/v1/admin/stream/prewarm",
                &json!({ "media_type": "movie", "media_id": movie, "capabilities": direct_play_caps() }),
            )
            .await;
        warm.assert_ok();
        let prewarm_id = warm.json::<Value>()["prewarm_id"]
            .as_str()
            .unwrap()
            .to_string();
        let resolves = rig.kobload_requests("/stream/resolve").await;
        assert_resolve_names(source, &resolves[0], source.listed_key());
        rig.wait_for_local(key, 1).await;

        let claimed = rig
            .server
            .admin_client()
            .post_json(
                &format!("/api/v1/admin/stream/prewarm/{prewarm_id}/claim"),
                &json!({}),
            )
            .await;
        claimed.assert_ok();
        let session = session_id_of(&claimed.json());
        assert_eq!(
            session_keys(&rig, session).await.release_key.as_deref(),
            Some(key),
            "{source:?}"
        );
        let again = rig
            .resolve_with(&rig.resolve_body_from(source, "movie", movie))
            .await;
        assert_eq!(
            session_id_of(&again),
            session,
            "{source:?}: a resolve of the warmed release opened a second session"
        );
    }
}

#[tokio::test]
async fn kobload_warm_ready_events_reach_clients_by_release_key() {
    for source in Source::ALL {
        let rig = Rig::start(UpstreamPlan::new(noise(1024)), "film.mkv").await;
        let event = json!({
            "type": "stream_warm_ready",
            "release_key": source.key(),
            "debrid_provider_id": PROVIDER_ID,
            "debrid_provider": "realdebrid",
        });
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/downloads/stream"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(format!("data: {event}\n\n")),
            )
            .mount(&rig.kobload)
            .await;
        let (tx, mut rx) = tokio::sync::broadcast::channel(8);
        let bridge = tokio::spawn(api::kobload_bridge::run_kobload_bridge(
            rig.server.runtime_config.clone(),
            tx,
        ));
        let got = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("no event")
            .unwrap();
        bridge.abort();
        let got = serde_json::to_value(got).unwrap();
        assert_eq!(got["kind"], "stream_warm_ready", "{got}");
        assert_eq!(got["release_key"], source.key(), "{source:?}: {got}");
    }
}

/// kobload can hand back a second URL for the same bytes, served below playback: the
/// cache-ahead reads that one, the player the stream URL.
#[tokio::test]
async fn the_cache_ahead_reads_the_cache_url_and_the_player_the_stream_url() {
    struct Case {
        cache_url: bool,
        expect_cache_path: &'static str,
    }
    for case in [
        Case {
            cache_url: true,
            expect_cache_path: "/cache/film.mp4",
        },
        Case {
            cache_url: false,
            expect_cache_path: "/play/film.mp4",
        },
    ] {
        let data = noise(4 * MB);
        let mut plan = UpstreamPlan::new(data.clone());
        plan.whole = WholeBody::Stall { first: 64 * 1024 };
        let rig = Rig::start(plan, "film.mp4").await;
        let mut resolved = json!({ "stream_url": rig.upstream.url("play/film.mp4") });
        if case.cache_url {
            resolved["cache_url"] = json!(rig.upstream.url("cache/film.mp4"));
        }
        rig.mount_resolve_with(resolved).await;
        let movie = rig.movie().await;
        let session = session_id_of(
            &rig.resolve_with(&rig.resolve_body("cachehash", movie, direct_play_caps()))
                .await,
        );
        rig.wait_for_local("cachehash", 64 * 1024).await;

        let tail = play(&rig.server, session, "bytes=3000000-3000999").await;
        assert_eq!(tail.status, StatusCode::PARTIAL_CONTENT);
        assert!(tail.body == data[3_000_000..3_001_000]);

        rig.server
            .app_state
            .playback_sessions
            .get(&session)
            .unwrap()
            .download_cancel
            .as_ref()
            .unwrap()
            .cancel();
        play(&rig.server, session, "bytes=0-1023").await;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let resumed = loop {
            let seen = rig.upstream.requests();
            let resumes: Vec<Seen> = seen
                .into_iter()
                .filter(|r| {
                    r.path == case.expect_cache_path
                        && r.range.as_deref().is_some_and(|r| r.ends_with('-'))
                        && !r.agent.as_deref().is_some_and(|a| a.starts_with("Lavf"))
                })
                .collect();
            if !resumes.is_empty() || std::time::Instant::now() > deadline {
                break resumes;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };

        let seen = rig.upstream.requests();
        let whole: Vec<&str> = seen
            .iter()
            .filter(|r| r.range.is_none())
            .map(|r| r.path.as_str())
            .collect();
        assert_eq!(whole, [case.expect_cache_path], "cache-ahead: {seen:?}");
        assert!(
            seen.iter().any(|r| r.path == "/play/film.mp4"
                && r.range.as_deref() == Some("bytes=3000000-3000999")),
            "the player's read went elsewhere: {seen:?}"
        );
        assert!(
            !resumed.is_empty(),
            "the stopped download never resumed from {}: {seen:?}",
            case.expect_cache_path
        );
    }
}
