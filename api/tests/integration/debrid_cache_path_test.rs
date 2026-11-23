//! Release keys and filenames reach the cache path from kobload and providers, not only from
//! validated requests. Whatever they say, the file lands inside its release's directory.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::UpstreamPlan;
use crate::common::*;
use api::jobs::handlers::DebridDownloadAheadHandler;
use api::jobs::worker::JobHandler;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone, Copy, Debug)]
enum Entry {
    Prewarm,
    Stremio,
    DownloadAhead,
    LearnedKey,
}

const ENTRIES: [Entry; 4] = [
    Entry::Prewarm,
    Entry::Stremio,
    Entry::DownloadAhead,
    Entry::LearnedKey,
];

struct Sandbox {
    rig: Rig,
    cache: PathBuf,
    outside: tempfile::TempDir,
}

async fn sandbox() -> Sandbox {
    let rig = Rig::start(UpstreamPlan::new(noise(64 * 1024)), "film.mkv").await;
    let cache = rig.cache_dir.path().join("a/b/c");
    std::fs::create_dir_all(&cache).unwrap();
    rig.set_config(json!({ "debrid_cache_dir": cache.to_string_lossy() }))
        .await;
    Sandbox {
        rig,
        cache,
        outside: tempfile::tempdir().unwrap(),
    }
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .flat_map(|e| {
            let path = e.path();
            if path.is_dir() {
                files_under(&path)
            } else {
                vec![path]
            }
        })
        .collect()
}

impl Sandbox {
    fn escaped(&self) -> Vec<PathBuf> {
        let mut files = files_under(self.rig.cache_dir.path());
        files.retain(|f| !f.starts_with(&self.cache));
        files.extend(files_under(self.outside.path()));
        files
    }

    async fn remount(&self, filename: &str, learned_key: Option<&str>) {
        self.rig.kobload.reset().await;
        self.rig.mount_resolve().await;
        let mut answer = json!({
            "stream_url": self.rig.upstream.url("film.mkv"),
            "filename": filename,
        });
        if let Some(key) = learned_key {
            answer["release_key"] = json!(key);
        }
        self.rig.mount_resolve_with(answer).await;
    }

    async fn drive(&self, entry: Entry, key: &str, case: usize) {
        let rig = &self.rig;
        let mut option = rig.option(key);
        option["id"] = json!(format!("opt-case-{case}"));
        rig.mount_search(json!([option])).await;
        match entry {
            Entry::Prewarm => {
                let movie = rig.movie().await;
                let resp = rig
                    .server
                    .admin_client()
                    .post_json(
                        "/api/v1/admin/stream/prewarm",
                        &json!({
                            "media_type": "movie",
                            "media_id": movie,
                            "capabilities": direct_play_caps()
                        }),
                    )
                    .await;
                assert!(resp.status.as_u16() < 500, "prewarm: {}", resp.status);
            }
            Entry::Stremio => {
                let imdb = format!("tt77{case:05}");
                let movie = Uuid::now_v7();
                sqlx::query(
                    "INSERT INTO movies (id, title, year, identifiers, enriched_at)
                     VALUES ($1, 'Path Movie', 2025,
                             jsonb_build_object('tmdb_id', $2::TEXT, 'imdb_id', $3::TEXT), NOW())",
                )
                .bind(movie)
                .bind(movie.simple().to_string())
                .bind(&imdb)
                .execute(&rig.server.pool)
                .await
                .unwrap();
                let api_key = format!("path-key-{case}");
                sqlx::query(
                    "INSERT INTO stremio_api_keys (user_id, api_key)
                     SELECT id, $1 FROM users WHERE role = 'admin' LIMIT 1",
                )
                .bind(&api_key)
                .execute(&rig.server.pool)
                .await
                .unwrap();
                let client = TestClient::new(&rig.server.api_url);
                let listed: Value = client
                    .get(&format!("/stremio/{api_key}/stream/movie/{imdb}.json"))
                    .await
                    .json();
                let url = listed["streams"][0]["url"]
                    .as_str()
                    .unwrap_or_else(|| panic!("nothing listed: {listed}"));
                // The redirect names the public https origin, which the test server is not.
                let played = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .unwrap()
                    .get(format!(
                        "{}{}",
                        rig.server.api_url,
                        &url[url.find("/stremio/").unwrap()..]
                    ))
                    .send()
                    .await
                    .unwrap();
                let status = played.status().as_u16();
                assert!(status == 307 || status == 503, "stremio play: {status}");
            }
            Entry::DownloadAhead => {
                let episodes = season_of(&rig.server, 2).await;
                let user: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE role = 'admin'")
                    .fetch_one(&rig.server.pool)
                    .await
                    .unwrap();
                let outcome = DebridDownloadAheadHandler::new(rig.server.app_state.clone())
                    .execute(
                        Uuid::now_v7(),
                        json!({ "user_id": user, "episode_id": episodes[1] }),
                        CancellationToken::new(),
                    )
                    .await;
                assert!(outcome.is_ok(), "{outcome:?}");
            }
            Entry::LearnedKey => {
                let movie = rig.movie().await;
                let mut body = rig.resolve_body_from(Source::Usenet, "movie", movie);
                body["nzb_ref"] = json!({ "indexer_id": "7", "guid": format!("guid-{case}") });
                let resp = rig
                    .server
                    .admin_client()
                    .post_json("/api/v1/admin/stream/resolve", &body)
                    .await;
                assert!(resp.status.as_u16() < 500 || resp.status.as_u16() == 502);
            }
        }
    }
}

#[tokio::test]
async fn a_hostile_release_key_writes_nothing_outside_the_cache() {
    let sandbox = sandbox().await;
    let absolute = sandbox.outside.path().join("evil");
    let keys = ["../../evil", absolute.to_str().unwrap(), "a/b"];
    let mut case = 0;
    for entry in ENTRIES {
        for key in keys {
            case += 1;
            let learned = matches!(entry, Entry::LearnedKey).then_some(key);
            sandbox.remount("film.mkv", learned).await;
            sandbox.drive(entry, key, case).await;
            tokio::time::sleep(Duration::from_millis(250)).await;
            let escaped = sandbox.escaped();
            assert!(escaped.is_empty(), "{entry:?} {key}: {escaped:?}");
            let inside = files_under(&sandbox.cache);
            assert!(inside.is_empty(), "{entry:?} {key}: {inside:?}");
        }
    }
}

#[tokio::test]
async fn a_provider_filename_is_reduced_to_its_basename() {
    let sandbox = sandbox().await;
    let absolute = sandbox.outside.path().join("x.mkv");
    let names = ["../../x.mkv", absolute.to_str().unwrap(), "..\\..\\x.mkv"];
    let mut case = 100;
    for entry in ENTRIES {
        for name in names {
            case += 1;
            let key = format!("safekey{case}");
            let learned = matches!(entry, Entry::LearnedKey).then_some(key.as_str());
            sandbox.remount(name, learned).await;
            sandbox.drive(entry, &key, case).await;
            let expected = sandbox.cache.join(&key).join("x.mkv");
            wait_until(
                Duration::from_secs(5),
                &format!("{entry:?} {name}: nothing at {expected:?}"),
                || expected.is_file(),
            )
            .await;
            let escaped = sandbox.escaped();
            assert!(escaped.is_empty(), "{entry:?} {name}: {escaped:?}");
        }
    }
}
