use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::UpstreamPlan;
use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

const PREWARM: &str = "/api/v1/admin/stream/prewarm";
const WARM_HEAD: usize = 8 * MB;

/// A real video followed by enough padding that the warm head is only part of it.
fn padded_fixture() -> Vec<u8> {
    let mut data = ITA_ENG_FIXTURE.to_vec();
    data.extend(noise(12 * MB));
    data
}

async fn warm_rig(data: Vec<u8>, info_hash: &str) -> Rig {
    let rig = Rig::start(UpstreamPlan::new(data), "film.mkv").await;
    rig.mount_search(json!([rig.option(info_hash)])).await;
    rig
}

fn movie_body(movie: Uuid) -> Value {
    json!({ "media_type": "movie", "media_id": movie, "capabilities": direct_play_caps() })
}

async fn prewarm(rig: &Rig, body: &Value) -> TestResponse {
    rig.server.admin_client().post_json(PREWARM, body).await
}

async fn prewarm_ok(rig: &Rig, body: &Value) -> Value {
    let resp = prewarm(rig, body).await;
    resp.assert_ok();
    resp.json()
}

fn prewarm_id(reply: &Value) -> Uuid {
    reply["prewarm_id"].as_str().unwrap().parse().unwrap()
}

async fn claim(rig: &Rig, id: Uuid) -> TestResponse {
    rig.server
        .admin_client()
        .post_json(&format!("{PREWARM}/{id}/claim"), &json!({}))
        .await
}

async fn wait_ready(rig: &Rig, body: &Value) -> Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let reply = prewarm_ok(rig, body).await;
        if reply["state"] == "ready" {
            return reply;
        }
        assert!(std::time::Instant::now() < deadline, "never ready: {reply}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn session_of(rig: &Rig, id: Uuid) -> Option<Uuid> {
    rig.server
        .app_state
        .stream_prewarms
        .entries_snapshot()
        .into_iter()
        .find(|(pid, _)| *pid == id)
        .map(|(_, sid)| sid)
}

#[tokio::test]
async fn repeat_prewarms_of_a_title_are_one_warm_resolve() {
    let rig = warm_rig(padded_fixture(), "warmhash").await;
    rig.set_config(json!({ "preferred_audio_language": "english" }))
        .await;
    let body = movie_body(rig.movie().await);

    let first = prewarm_ok(&rig, &body).await;
    let second = prewarm_ok(&rig, &body).await;

    assert_eq!(prewarm_id(&first), prewarm_id(&second));
    assert_eq!(first["option"]["id"], "opt-warmhash", "{first}");
    assert!(
        matches!(first["state"].as_str(), Some("warming" | "ready")),
        "{first}"
    );
    let resolves = rig.kobload_requests("/stream/resolve").await;
    assert_eq!(resolves.len(), 1, "a repeat prewarm resolved again");
    assert_eq!(resolves[0]["priority"], "warm");
    let searches = rig.kobload_requests("/stream/search").await;
    assert_eq!(searches[0]["audio_languages"], json!(["english"]));
}

#[tokio::test]
async fn a_claimed_prewarm_plays_its_first_bytes_from_disk_at_once() {
    let data = padded_fixture();
    let rig = warm_rig(data.clone(), "claimhash").await;
    let movie = rig.movie().await;
    let body = movie_body(movie);

    let id = prewarm_id(&wait_ready(&rig, &body).await);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let warmed = std::fs::metadata(rig.cache_file("claimhash"))
        .unwrap()
        .len() as usize;
    assert!(
        warmed >= WARM_HEAD.min(data.len()) && warmed < data.len(),
        "a warm session downloads its head and stops: {warmed} of {}",
        data.len()
    );

    let resp = claim(&rig, id).await;
    resp.assert_ok();
    let session: Value = resp.json();
    let session_id = session_id_of(&session);
    assert!(session["direct_url"].is_string(), "{session}");
    assert_eq!(session["media_id"], json!(movie), "{session}");
    assert_eq!(session["preferred_audio_stream_index"], 2, "{session}");

    let head = play(&rig.server, session_id, "bytes=0-65535").await;
    assert_eq!(head.status, StatusCode::PARTIAL_CONTENT);
    assert!(head.body == data[..65536]);
    assert!(
        !rig.upstream
            .ranges_seen()
            .iter()
            .any(|r| r == "bytes=0-65535"),
        "the warmed head was proxied"
    );
    rig.wait_for_local("claimhash", data.len() as u64).await;

    let again = claim(&rig, id).await;
    again.assert_ok();
    assert_eq!(
        session_id_of(&again.json()),
        session_id,
        "claims are idempotent"
    );
}

#[tokio::test]
async fn resolving_the_warmed_release_claims_it() {
    let rig = warm_rig(padded_fixture(), "walkhash").await;
    let movie = rig.movie().await;
    let id = prewarm_id(&prewarm_ok(&rig, &movie_body(movie)).await);
    let session = session_of(&rig, id).unwrap();

    let resolved = rig
        .resolve_with(&rig.resolve_body("walkhash", movie, direct_play_caps()))
        .await;

    assert_eq!(session_id_of(&resolved), session);
    assert_eq!(rig.kobload_requests("/stream/resolve").await.len(), 1);
}

#[tokio::test]
async fn an_untouched_prewarm_expires_and_its_claim_is_gone() {
    let rig = warm_rig(ITA_ENG_FIXTURE.to_vec(), "expirehash").await;
    let state = &rig.server.app_state;
    for sweep in [false, true] {
        let body = movie_body(rig.movie().await);
        let id = prewarm_id(&wait_ready(&rig, &body).await);
        let session = session_of(&rig, id).unwrap();

        state
            .stream_prewarms
            .backdate(id, Duration::from_secs(4 * 60));
        rig.server
            .admin_client()
            .post_json(&format!("{PREWARM}/{id}/touch"), &json!({}))
            .await
            .assert_status(StatusCode::NO_CONTENT);
        state
            .stream_prewarms
            .backdate(id, Duration::from_secs(2 * 60));
        assert!(
            state.playback_sessions.contains_key(&session),
            "a touch keeps it warm"
        );

        state
            .stream_prewarms
            .backdate(id, Duration::from_secs(6 * 60));
        if sweep {
            api::routes::admin::stream_prewarm::sweep(state).await;
            assert!(!state.playback_sessions.contains_key(&session));
        }
        claim(&rig, id).await.assert_status(StatusCode::GONE);
        assert!(!state.playback_sessions.contains_key(&session));
        assert!(
            rig.cache_file("expirehash").exists(),
            "expiry keeps the cache"
        );
    }
}

#[tokio::test]
async fn a_released_prewarm_cannot_be_claimed() {
    let rig = warm_rig(ITA_ENG_FIXTURE.to_vec(), "releasehash").await;
    let id = prewarm_id(&prewarm_ok(&rig, &movie_body(rig.movie().await)).await);
    let session = session_of(&rig, id).unwrap();

    rig.server
        .admin_client()
        .delete(&format!("{PREWARM}/{id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    assert!(
        !rig.server
            .app_state
            .playback_sessions
            .contains_key(&session)
    );
    claim(&rig, id).await.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_prewarm_whose_stream_failed_is_a_conflict() {
    let mut plan = UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec());
    plan.refuse = Some(StatusCode::FORBIDDEN);
    let rig = Rig::start(plan, "film.mkv").await;
    rig.mount_search(json!([rig.option("failhash")])).await;
    let id = prewarm_id(&prewarm_ok(&rig, &movie_body(rig.movie().await)).await);
    let session = session_of(&rig, id).unwrap();
    let state = &rig.server.app_state;
    wait_until(Duration::from_secs(10), "the probe never failed", || {
        state
            .playback_sessions
            .get(&session)
            .is_some_and(|s| s.error.is_some())
    })
    .await;

    claim(&rig, id).await.assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn nothing_streamable_is_no_content() {
    let rig = Rig::start(UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec()), "film.mkv").await;
    let mut uncached = rig.option("coldhash");
    uncached["cached"] = json!(false);
    uncached["availability"] = json!("uncached");
    rig.mount_search(json!([uncached])).await;

    prewarm(&rig, &movie_body(rig.movie().await))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert!(rig.kobload_requests("/stream/resolve").await.is_empty());
}

#[tokio::test]
async fn a_viewer_holds_at_most_two_prewarms() {
    let rig = Rig::start(UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec()), "film.mkv").await;
    let mut ids = Vec::new();
    for hash in ["cap1hash", "cap2hash", "cap3hash"] {
        rig.kobload.reset().await;
        rig.mount_resolve().await;
        rig.mount_search(json!([rig.option(hash)])).await;
        ids.push(prewarm_id(
            &prewarm_ok(&rig, &movie_body(rig.movie().await)).await,
        ));
    }

    claim(&rig, ids[0])
        .await
        .assert_status(StatusCode::NOT_FOUND);
    claim(&rig, ids[2]).await.assert_ok();
}

/// kobapp sends a Discover episode's numbers at the top level and inside `auto_create`.
#[tokio::test]
async fn a_discover_episode_prewarm_takes_its_numbers_from_either_place() {
    let rig = warm_rig(ITA_ENG_FIXTURE.to_vec(), "episodehash").await;
    let cases = [
        (
            "both agree",
            json!(1),
            json!(2),
            json!(1),
            json!(2),
            "1",
            "2",
        ),
        (
            "top level only",
            json!(3),
            json!(4),
            Value::Null,
            Value::Null,
            "3",
            "4",
        ),
        (
            "auto_create only",
            Value::Null,
            Value::Null,
            json!(5),
            json!(6),
            "5",
            "6",
        ),
        (
            "top level wins",
            json!(7),
            json!(8),
            json!(9),
            json!(9),
            "7",
            "8",
        ),
    ];
    for (i, (name, season, episode, ac_season, ac_episode, want_s, want_e)) in
        cases.into_iter().enumerate()
    {
        let body = json!({
            "media_type": "tv",
            "season": season,
            "episode": episode,
            "capabilities": direct_play_caps(),
            "auto_create": {
                "title": "Warm Show",
                "identifiers": { "tmdb_id": format!("70{i}") },
                "provider": "tmdb",
                "provider_id": format!("70{i}"),
                "season_number": ac_season,
                "episode_number": ac_episode
            }
        });
        let reply = prewarm(&rig, &body).await;
        assert!(reply.status.is_success(), "{name}: {}", reply.status);
        let search = rig.kobload_requests("/stream/search").await.pop().unwrap();
        let want = &search["media_want"];
        assert_eq!(want["season"], want_s, "{name}: {search}");
        assert_eq!(want["episode"], want_e, "{name}: {search}");
        if let Ok(reply) = serde_json::from_slice::<Value>(reply.bytes()) {
            rig.server
                .admin_client()
                .delete(&format!("{PREWARM}/{}", prewarm_id(&reply)))
                .await;
        }
    }
}
