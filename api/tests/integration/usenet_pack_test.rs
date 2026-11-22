//! A usenet season pack is one release, one `release_key`, holding a file per episode.
//! kobload picks the episode's file from the episode kobbler names and says which it
//! picked, so every session, cache file and analysis row is that file's alone.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::{Upstream, UpstreamPlan};
use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::matchers::{body_partial_json, method, path_regex};
use wiremock::{Mock, ResponseTemplate};

struct Episode {
    id: Uuid,
    number: i32,
    file_id: i32,
    data: Vec<u8>,
    _upstream: Upstream,
}

struct Pack {
    rig: Rig,
    e05: Episode,
    e06: Episode,
}

/// Every file in the pack has the same name, so only `file_id` tells them apart.
async fn pack() -> Pack {
    let rig = Rig::start(
        UpstreamPlan::new(LETTERBOXED_FIXTURE.to_vec()),
        "episode.mkv",
    )
    .await;
    Mock::given(method("POST"))
        .and(path_regex("/api/v1/stream/resolve$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "stream_url": rig.upstream.url("episode.mkv"),
            "filename": "episode.mkv",
            "filesize": LETTERBOXED_FIXTURE.len(),
            "cached": true,
            "release_key": USENET_KEY,
            "file_id": 0,
        })))
        .with_priority(2)
        .mount(&rig.kobload)
        .await;
    let episodes = season_of(&rig.server, 6).await;
    let mut files = Vec::new();
    for (number, file_id, data) in [(5, 4, ITA_ENG_FIXTURE), (6, 5, CHAPTERED_FIXTURE)] {
        let data = data.to_vec();
        let upstream = Upstream::start(UpstreamPlan::new(data.clone())).await;
        Mock::given(method("POST"))
            .and(path_regex("/api/v1/stream/resolve$"))
            .and(body_partial_json(
                json!({ "episode": { "season": 1, "episode": number } }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "stream_url": upstream.url("play/episode.mkv"),
                "cache_url": upstream.url("cache/episode.mkv"),
                "filename": "episode.mkv",
                "filesize": data.len(),
                "cached": true,
                "release_key": USENET_KEY,
                "file_id": file_id,
            })))
            .with_priority(1)
            .mount(&rig.kobload)
            .await;
        files.push(Episode {
            id: episodes[number as usize - 1],
            number,
            file_id,
            data,
            _upstream: upstream,
        });
    }
    let e06 = files.pop().unwrap();
    let e05 = files.pop().unwrap();
    Pack { rig, e05, e06 }
}

impl Pack {
    async fn tap(&self, episode: &Episode) -> Uuid {
        session_id_of(&self.rig.resolve_with(&self.body(episode)).await)
    }

    fn body(&self, episode: &Episode) -> Value {
        self.rig
            .resolve_body_from(Source::Usenet, "episode", episode.id)
    }

    async fn cache_path(&self, session: Uuid) -> String {
        let (path, file_id): (Option<String>, Option<i32>) =
            sqlx::query_as("SELECT cache_path, file_id FROM debrid_stream_sessions WHERE id = $1")
                .bind(session)
                .fetch_one(&self.rig.server.pool)
                .await
                .unwrap();
        assert!(file_id.is_some(), "session {session} stored no file_id");
        path.unwrap_or_else(|| panic!("session {session} has no cache path"))
    }

    /// The session played its episode's bytes, downloaded them to its own file, and that
    /// file was analysed once.
    async fn assert_played(&self, episode: &Episode, session: Uuid) -> String {
        let what = format!("E{:02}", episode.number);
        let played = play(&self.rig.server, session, "bytes=0-").await;
        assert_eq!(played.status, StatusCode::PARTIAL_CONTENT, "{what}");
        assert!(played.body == episode.data, "{what} played another file");

        let (file_id,): (Option<i32>,) =
            sqlx::query_as("SELECT file_id FROM debrid_stream_sessions WHERE id = $1")
                .bind(session)
                .fetch_one(&self.rig.server.pool)
                .await
                .unwrap();
        assert_eq!(file_id, Some(episode.file_id), "{what}");

        let path = self.cache_path(session).await;
        wait_until(
            std::time::Duration::from_secs(10),
            &format!("{what} never finished downloading"),
            || std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) >= episode.data.len() as u64,
        )
        .await;
        assert!(
            std::fs::read(&path).unwrap() == episode.data,
            "{what} cached another file's bytes at {path}"
        );
        process_download(&self.rig.server, session, std::path::Path::new(&path)).await;
        let rows: Vec<(String, Option<i32>)> = sqlx::query_as(
            "SELECT file_path, file_id FROM debrid_cache_files WHERE file_path = $1",
        )
        .bind(&path)
        .fetch_all(&self.rig.server.pool)
        .await
        .unwrap();
        assert_eq!(rows, [(path.clone(), Some(episode.file_id))], "{what}");
        path
    }

    async fn analysed(&self, path: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM debrid_files WHERE cache_path = $1")
            .bind(path)
            .fetch_one(&self.rig.server.pool)
            .await
            .unwrap()
    }

    async fn asked_for(&self) -> Vec<Value> {
        self.rig
            .kobload_requests("/stream/resolve")
            .await
            .into_iter()
            .map(|r| r["episode"].clone())
            .collect()
    }
}

#[tokio::test]
async fn pack_episodes_played_in_turn_each_get_their_own_file() {
    let pack = pack().await;

    let e05 = pack.tap(&pack.e05).await;
    let e05_path = pack.assert_played(&pack.e05, e05).await;
    let e06 = pack.tap(&pack.e06).await;
    assert_ne!(e06, e05, "E06 was handed E05's live session");
    let e06_path = pack.assert_played(&pack.e06, e06).await;

    assert_ne!(e05_path, e06_path);
    assert_eq!(pack.analysed(&e05_path).await, 1);
    assert_eq!(pack.analysed(&e06_path).await, 1);
    assert_eq!(
        pack.asked_for().await,
        [
            json!({ "season": 1, "episode": 5 }),
            json!({ "season": 1, "episode": 6 })
        ]
    );

    let again = pack.tap(&pack.e05).await;
    assert_eq!(again, e05, "a second tap of E05 opened another session");

    end_playback(&pack.rig.server, e05).await;
    let replay = pack.tap(&pack.e05).await;
    assert_ne!(replay, e05);
    assert_eq!(
        pack.cache_path(replay).await,
        e05_path,
        "the replay did not find E05's file"
    );
    let played = play(&pack.rig.server, replay, "bytes=0-").await;
    assert!(
        played.body == pack.e05.data,
        "the replay played another file"
    );
    assert_eq!(pack.analysed(&e05_path).await, 1, "one file, one analysis");
}

#[tokio::test]
async fn pack_episodes_played_at_once_each_get_their_own_file() {
    let pack = pack().await;

    let (e05, e06, e05_again) = tokio::join!(
        pack.tap(&pack.e05),
        pack.tap(&pack.e06),
        pack.tap(&pack.e05)
    );

    assert_ne!(e05, e06);
    assert_eq!(e05_again, e05, "two taps of E05 opened two sessions");
    let e05_path = pack.assert_played(&pack.e05, e05).await;
    let e06_path = pack.assert_played(&pack.e06, e06).await;
    assert_ne!(e05_path, e06_path);
}
