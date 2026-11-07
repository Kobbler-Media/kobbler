//! Players fire their first seek, playlist and segment requests the moment a session
//! opens, while its transcode is still starting in the background. Whatever they ask
//! for has to join that start, not race it: a second start leaves a generation nobody
//! serves, and a read that lands mid-restart must wait rather than 503.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::UpstreamPlan;
use crate::common::*;
use api::repositories::movie_files;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

const MINUTE: &[u8] = include_bytes!("../fixtures/playback/minute.mkv");

fn hls_caps() -> Value {
    json!({ "video_codecs": ["h264"], "audio_codecs": ["aac"], "containers": ["mp4"] })
}

fn base(session_id: Uuid) -> String {
    format!("/api/v1/admin/playback/sessions/{session_id}")
}

async fn seed_file(server: &TestServer, path: &Path) -> Uuid {
    let movie = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": "Race", "year": 2024 }),
        )
        .await;
    movie.assert_status(StatusCode::CREATED);
    let movie_id: Uuid = movie.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    movie_files::create(
        &server.pool,
        movie_files::MovieFileInput {
            movie_id,
            file_path: path.to_string_lossy().into_owned(),
            file_size: Some(std::fs::metadata(path).unwrap().len() as i64),
            format: "mkv".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

/// The minute fixture behind a large attachment, which Matroska reads whole on open: the
/// background start takes long enough for the player's first requests to land inside
/// it, as a cold NAS read or a first GPU context does in production.
fn write_slow_minute(server: &TestServer) -> PathBuf {
    let root = Path::new(&server.storage_root()).to_path_buf();
    let (src, blob, path) = (
        root.join("minute.mkv"),
        root.join("blob.bin"),
        root.join("slow.mkv"),
    );
    std::fs::write(&src, MINUTE).unwrap();
    std::fs::write(&blob, vec![0u8; 128 << 20]).unwrap();
    let status = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(&src)
        .arg("-attach")
        .arg(&blob)
        .args([
            "-metadata:s:t",
            "mimetype=application/octet-stream",
            "-c",
            "copy",
        ])
        .arg(&path)
        .status()
        .expect("ffmpeg");
    assert!(status.success());
    std::fs::remove_file(blob).unwrap();
    path
}

/// 720p in a codec the client can't take, so the session transcodes and offers ABR variants.
fn write_720p(server: &TestServer) -> PathBuf {
    let path = Path::new(&server.storage_root()).join("tall.mkv");
    let status = std::process::Command::new("ffmpeg")
        .args(
            "-v error -y -f lavfi -i testsrc=s=1280x720:r=5 -f lavfi -i sine=r=48000 -t 4 \
             -c:v mpeg4 -c:a aac"
                .split_whitespace(),
        )
        .arg(&path)
        .status()
        .expect("ffmpeg");
    assert!(status.success());
    path
}

async fn open(server: &TestServer, file_id: Uuid) -> Uuid {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/playback/sessions",
            &json!({ "media_type": "movie", "file_id": file_id, "capabilities": hls_caps() }),
        )
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_ne!(body["method"], "direct_play", "{body}");
    body["session_id"].as_str().unwrap().parse().unwrap()
}

fn gen_dirs(dir: &Path) -> BTreeSet<u32> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.file_name().to_str()?.strip_prefix("gen_")?.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

struct Settled {
    dir: PathBuf,
    seek_generation: u32,
    live_generation: Option<u32>,
    live_runs: BTreeSet<u32>,
    variant_runs: Vec<(PathBuf, BTreeSet<u32>)>,
}

fn run_generations(t: &api::ffmpeg::transcode::TranscodeProcess) -> BTreeSet<u32> {
    t.update_segments();
    t.runs().into_iter().map(|r| r.generation).collect()
}

/// Waits out any start still in flight, then reads back what the session is producing.
async fn settle(server: &TestServer, session_id: Uuid) -> Settled {
    let mut last = None;
    for _ in 0..200 {
        tokio::time::sleep(Duration::from_millis(25)).await;
        let Some(session) = server.app_state.playback_sessions.get(&session_id) else {
            continue;
        };
        let settled = Settled {
            dir: session.segment_dir().to_path_buf(),
            seek_generation: session.seek_generation,
            live_generation: session.transcode.as_ref().map(|t| t.current_generation()),
            live_runs: session
                .transcode
                .as_ref()
                .map(run_generations)
                .unwrap_or_default(),
            variant_runs: session
                .variants
                .iter()
                .flatten()
                .map(|(_, v)| {
                    (
                        v.segment_cache.base_dir.clone(),
                        v.transcode
                            .as_ref()
                            .map(run_generations)
                            .unwrap_or_default(),
                    )
                })
                .collect(),
        };
        let disk = gen_dirs(&settled.dir);
        if settled.live_generation.is_some() && disk.iter().all(|g| settled.live_runs.contains(g)) {
            return settled;
        }
        last = Some(settled);
    }
    last.expect("session never came back")
}

/// Every generation on disk is one the live transcode serves, bar those the action
/// deliberately replaced.
fn assert_no_orphans(what: &str, s: &Settled, retired: &[u32]) {
    let known = |runs: &BTreeSet<u32>, g: &u32| runs.contains(g) || retired.contains(g);
    let orphans: Vec<u32> = gen_dirs(&s.dir)
        .into_iter()
        .filter(|g| !known(&s.live_runs, g))
        .collect();
    assert!(
        orphans.is_empty(),
        "{what}: generations {orphans:?} were started and abandoned (live runs {:?}, live generation {:?})",
        s.live_runs,
        s.live_generation
    );
    for (dir, runs) in &s.variant_runs {
        let orphans: Vec<u32> = gen_dirs(dir)
            .into_iter()
            .filter(|g| !runs.contains(g))
            .collect();
        assert!(
            orphans.is_empty(),
            "{what}: variant {} generations {orphans:?} abandoned (live {runs:?})",
            dir.display()
        );
    }
}

fn assert_never_unavailable(what: &str, responses: &[TestResponse]) {
    for r in responses {
        assert_ne!(
            r.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{what}: a read during the start was turned away: {}",
            r.text()
        );
    }
}

/// The player's first burst: master and media playlist alongside the action.
async fn burst(
    server: &TestServer,
    session_id: Uuid,
    action: &Action,
) -> (TestResponse, Vec<TestResponse>) {
    let client = server.admin_client();
    let b = base(session_id);
    let fire = async {
        match action {
            Action::Post(path, body) => client.post_json(&format!("{b}/{path}"), body).await,
            Action::Get(path) => client.get(&format!("{b}/{path}")).await,
        }
    };
    let master = format!("{b}/master.m3u8");
    let media = format!("{b}/stream.m3u8?g=0");
    let (acted, master, media, again) =
        tokio::join!(fire, client.get(&master), client.get(&media), async {
            match action {
                Action::Get(path) => Some(client.get(&format!("{b}/{path}")).await),
                Action::Post(..) => None,
            }
        },);
    let mut reads = vec![master, media];
    reads.extend(again);
    (acted, reads)
}

enum Action {
    Post(&'static str, Value),
    Get(&'static str),
}

const ROUNDS: usize = 2;

#[tokio::test]
async fn every_restart_joins_the_start_already_in_flight() {
    let server = TestServer::start_api_only().await;
    let minute = seed_file(&server, &write_slow_minute(&server)).await;
    let cases: [(&str, Action, &[u32]); 3] = [
        (
            "seek",
            Action::Post("seek", json!({ "time_seconds": 40.0 })),
            &[],
        ),
        (
            "decode-error",
            Action::Post("decode-error", json!({ "at_seconds": 20.0 })),
            &[],
        ),
        (
            "audio switch",
            Action::Post("switch-audio", json!({ "burn_in_subtitle_index": -1 })),
            &[0],
        ),
    ];
    for (what, action, retired) in &cases {
        for _ in 0..ROUNDS {
            let id = open(&server, minute).await;
            let (acted, reads) = burst(&server, id, action).await;
            assert!(
                acted.status.is_success(),
                "{what}: {} {}",
                acted.status,
                acted.text()
            );
            assert_never_unavailable(what, &reads);
            assert_no_orphans(what, &settle(&server, id).await, retired);
        }
    }
}

#[tokio::test]
async fn a_variant_start_joins_the_primary_start_and_its_twin() {
    let server = TestServer::start_api_only().await;
    let tall = seed_file(&server, &write_720p(&server)).await;
    let action = Action::Get("stream.m3u8?v=480p");
    for _ in 0..ROUNDS {
        let id = open(&server, tall).await;
        let (acted, reads) = burst(&server, id, &action).await;
        assert!(
            acted.status.is_success(),
            "variant: {} {}",
            acted.status,
            acted.text()
        );
        for r in &reads {
            assert!(
                r.status.is_success(),
                "variant twin/primary: {} {}",
                r.status,
                r.text()
            );
        }
        assert_no_orphans("variant", &settle(&server, id).await, &[]);
    }
}

#[tokio::test]
async fn a_seek_joins_a_debrid_start_still_launching() {
    let rig = Rig::start(UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec()), "film.mkv").await;
    let movie = rig.movie().await;
    let id = session_id_of(
        &rig.resolve_with(&rig.resolve_body("racehash", movie, hls_caps()))
            .await,
    );

    let state = rig.server.app_state.clone();
    wait_until(
        Duration::from_secs(20),
        "probe never stamped the session",
        || {
            state
                .playback_sessions
                .get(&id)
                .is_some_and(|s| !s.file_path.as_os_str().is_empty())
        },
    )
    .await;
    assert!(
        state
            .playback_sessions
            .get(&id)
            .is_some_and(|s| s.transcode.is_none()),
        "the seek has to land while the debrid transcode is still launching"
    );

    let action = Action::Post("seek", json!({ "time_seconds": 3.0 }));
    let (acted, reads) = burst(&rig.server, id, &action).await;
    assert!(
        acted.status.is_success(),
        "debrid seek: {} {}",
        acted.status,
        acted.text()
    );
    assert_never_unavailable("debrid seek", &reads);
    assert_no_orphans("debrid seek", &settle(&rig.server, id).await, &[]);
}
