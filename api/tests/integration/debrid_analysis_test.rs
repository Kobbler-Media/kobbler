//! A debrid release gets the analysis a library file gets, and what it finds reaches
//! whoever plays that release next.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::UpstreamPlan;
use crate::common::*;
use api::embedding::{EMBED_DIM, EMBED_MODEL};
use api::media_cache::Branch;
use api::media_cache::episode_cache::{MediaKind, episode_cache_dir};
use api::media_cache::readers::read_manifest_at;
use api::media_cache::writers::{
    ArtefactInfo, new_manifest_at, write_manifest_at, write_sidecar_at,
};
use api::repositories::{debrid_files, tv_episode_files};
use api::runtime_config::RuntimeConfigUpdate;
use serde_json::{Value, json};
use std::path::Path;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

async fn analysis_on(server: &TestServer, vision_url: Option<String>) {
    server
        .runtime_config
        .update(RuntimeConfigUpdate {
            enrichment_enabled: Some(true),
            skip_detect_mode: Some("full".to_string()),
            vision_service_url: Some(vision_url.unwrap_or_default()),
            ..Default::default()
        })
        .await
        .unwrap();
}

async fn vision_service() -> MockServer {
    let vision = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&vision)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let count = body["input"].as_array().map_or(0, Vec::len);
            let data: Vec<Value> = (0..count)
                .map(|i| json!({"object": "embedding", "index": i, "embedding": vec![0.5f32; EMBED_DIM]}))
                .collect();
            ResponseTemplate::new(200).set_body_json(json!({"object": "list", "data": data}))
        })
        .mount(&vision)
        .await;
    vision
}

#[tokio::test]
async fn a_debrid_release_is_embedded_like_a_library_file() {
    let server = TestServer::start_api_only().await;
    let vision = vision_service().await;
    analysis_on(&server, Some(vision.uri())).await;
    let episode = season_of(&server, 1).await[0];
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("e01.mkv");
    std::fs::write(&file, LETTERBOXED_FIXTURE).unwrap();
    let session = downloaded_session(&server, "episode", episode, "embedhash", &file).await;

    process_download(&server, session, &file).await;

    let cache = file_cache(&server, &file).await;
    let manifest = read_manifest_at(&cache).expect("episode cache manifest");
    let embeddings = manifest.artefacts.embeddings.expect("embeddings attempted");
    assert!(embeddings.present, "{:?}", embeddings.params);
    assert_eq!(embeddings.params["model"], json!(EMBED_MODEL));
    assert!(cache.join("embeddings.bin").is_file());
}

/// Where the analysis of the download at `path` lives.
async fn file_cache(server: &TestServer, path: &Path) -> std::path::PathBuf {
    let file = debrid_files::at(&server.pool, &path.to_string_lossy())
        .await
        .unwrap()
        .expect("the download has a row");
    episode_cache_dir(
        &server.storage_root(),
        &MediaKind::DebridFile { file_id: file.id },
    )
}

fn last_cue_end(vtt: &str) -> String {
    vtt.lines()
        .rev()
        .find_map(|l| l.split_once(" --> ").map(|(_, end)| end.to_string()))
        .unwrap_or_default()
}

#[tokio::test]
async fn each_episode_of_a_season_pack_keeps_its_own_seek_thumbnails() {
    let server = TestServer::start_api_only().await;
    analysis_on(&server, None).await;
    let episodes = season_of(&server, 2).await;
    let pack = tempfile::tempdir().unwrap();
    let files = [
        (
            episodes[0],
            pack.path().join("Show_S01E01.mkv"),
            LETTERBOXED_FIXTURE,
        ),
        (
            episodes[1],
            pack.path().join("Show_S01E02.mkv"),
            ITA_ENG_FIXTURE,
        ),
    ];
    for (episode, file, bytes) in &files {
        std::fs::write(file, bytes).unwrap();
        let session = downloaded_session(&server, "episode", *episode, "packhash", file).await;
        process_download(&server, session, file).await;
    }

    let thumbnails = |file: &Path| {
        std::fs::read_to_string(
            api::media_cache::debrid_file_cache_dir(file).join("trickplay/thumbnails.vtt"),
        )
        .unwrap_or_default()
    };
    let (first, second) = (thumbnails(&files[0].1), thumbnails(&files[1].1));
    assert!(
        !first.is_empty() && !second.is_empty(),
        "{first}\n---\n{second}"
    );
    assert_ne!(
        last_cue_end(&first),
        last_cue_end(&second),
        "{first}\n---\n{second}"
    );
}

/// Write an analysis into `dir` as `media_analyze` leaves one for a video file with
/// vision off, with the audio fingerprint and picture hashes given.
fn stage_cache(dir: &Path, fingerprint: &[u8], vhash: &[u8], duration: f64) {
    std::fs::create_dir_all(dir).unwrap();
    write_sidecar_at(dir, Branch::Chromaprint, fingerprint).unwrap();
    write_sidecar_at(dir, Branch::Vhash, vhash).unwrap();
    write_sidecar_at(dir, Branch::Silence, b"[]").unwrap();
    write_sidecar_at(dir, Branch::BlackFrames, b"[]").unwrap();
    let mut manifest = new_manifest_at(dir, duration);
    for branch in [
        Branch::Trickplay,
        Branch::Vhash,
        Branch::Chromaprint,
        Branch::Silence,
        Branch::BlackFrames,
    ] {
        manifest.artefacts.set(
            branch,
            ArtefactInfo {
                present: true,
                params: Default::default(),
            },
        );
    }
    write_manifest_at(dir, &manifest).unwrap();
}

fn no_vhash() -> Vec<u8> {
    let mut vhash = 64u16.to_le_bytes().to_vec();
    vhash.extend_from_slice(&0u32.to_le_bytes());
    vhash
}

/// A release of `episode` downloaded to `release` and analysed earlier, with a session
/// that played it.
async fn analysed_release(
    server: &TestServer,
    episode: Uuid,
    release_key: &str,
    release: &Path,
    (fingerprint, vhash, duration): (&[u8], &[u8], f64),
) -> Uuid {
    let file = debrid_files::upsert(&server.pool, &release.to_string_lossy(), "episode", episode)
        .await
        .unwrap();
    stage_cache(
        &episode_cache_dir(
            &server.storage_root(),
            &MediaKind::DebridFile { file_id: file.id },
        ),
        fingerprint,
        vhash,
        duration,
    );
    downloaded_session(server, "episode", episode, release_key, release).await
}

/// The segments of `kind` the player of `session` is served: its download's.
async fn stored_spans(server: &TestServer, session: Uuid, kind: &str) -> Vec<(f64, f64)> {
    let segments: Option<Value> = sqlx::query_scalar(
        "SELECT f.skip_segments FROM debrid_stream_sessions s
         JOIN debrid_files f ON f.cache_path = s.cache_path WHERE s.id = $1",
    )
    .bind(session)
    .fetch_optional(&server.pool)
    .await
    .unwrap()
    .flatten();
    spans(segments, kind)
}

fn spans(segments: Option<Value>, kind: &str) -> Vec<(f64, f64)> {
    segments
        .and_then(|s| s.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter(|s| s["segment_type"] == kind)
        .map(|s| {
            (
                s["start_seconds"].as_f64().unwrap(),
                s["end_seconds"].as_f64().unwrap(),
            )
        })
        .collect()
}

async fn stored_credits(server: &TestServer, session: Uuid) -> Vec<f64> {
    stored_spans(server, session, "credits")
        .await
        .into_iter()
        .map(|(start, _)| start)
        .collect()
}

/// A Good Girl's Guide to Murder S1: episode 1 was another release that shares none of
/// the season's audio; episodes 2 and 3 share their credits. Episode 1 arriving last
/// must neither hide the credits the other two share nor leave them only in a table
/// that the next playback of either never reads.
#[tokio::test]
async fn a_new_episode_brings_credits_to_the_releases_already_played() {
    let server = TestServer::start_api_only().await;
    analysis_on(&server, None).await;
    let episodes = season_of(&server, 3).await;
    let downloads = tempfile::tempdir().unwrap();
    let vhash = no_vhash();
    let mut played = Vec::new();
    for (episode, (fingerprint, _, duration), name) in [
        (episodes[1], AGG[1], "AGG_S01E02.mkv"),
        (episodes[2], AGG[2], "AGG_S01E03.mkv"),
    ] {
        let release = downloads.path().join(name);
        played.push(
            analysed_release(
                &server,
                episode,
                "agghash",
                &release,
                (fingerprint, &vhash, duration),
            )
            .await,
        );
    }
    let other_release = downloaded_session(
        &server,
        "episode",
        episodes[1],
        "otherhash",
        &downloads.path().join("Other.Release.S01E02.mkv"),
    )
    .await;

    let first = downloads.path().join("AGG.S01E01.480p.mkv");
    std::fs::write(&first, LETTERBOXED_FIXTURE).unwrap();
    let session = downloaded_session(&server, "episode", episodes[0], "rubikhash", &first).await;
    process_download(&server, session, &first).await;

    for (session, expected) in played.iter().zip([2476.4, 2412.8]) {
        let credits = stored_credits(&server, *session).await;
        assert!(
            credits.len() == 1 && (credits[0] - expected).abs() < 1.0,
            "{credits:?}"
        );
    }
    assert_eq!(
        stored_credits(&server, other_release).await,
        Vec::<f64>::new()
    );
}

async fn queued_processing(server: &TestServer) -> Vec<Value> {
    sqlx::query_scalar(
        "SELECT payload FROM job_status WHERE job_type = 'debrid_process' AND status = 'pending'",
    )
    .fetch_all(&server.pool)
    .await
    .unwrap()
}

/// Tests run no job worker: a queued processing job stands for one that ran.
async fn processing_ran(server: &TestServer) {
    sqlx::query("DELETE FROM job_status WHERE job_type = 'debrid_process'")
        .execute(&server.pool)
        .await
        .unwrap();
}

/// A download analysed before debrid releases got everything a library file gets is
/// analysed again the next time it plays from disk; one that is up to date is not.
#[tokio::test]
async fn a_release_on_disk_is_analysed_again_when_it_plays_until_it_is_up_to_date() {
    let rig = Rig::start(UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec()), "film.mkv").await;
    analysis_on(&rig.server, None).await;
    let movie = rig.movie().await;
    let body = rig.resolve_body("stalehash", movie, direct_play_caps());
    let first = session_id_of(&rig.resolve_with(&body).await);
    rig.wait_for_complete("stalehash").await;
    let server = &rig.server;
    wait_until_async(
        std::time::Duration::from_secs(10),
        "the finished download was never queued for processing",
        || async move { !queued_processing(server).await.is_empty() },
    )
    .await;
    processing_ran(&rig.server).await;
    end_playback(&rig.server, first).await;

    let replay = session_id_of(&rig.resolve_with(&body).await);
    let file = rig.cache_file("stalehash");
    assert_eq!(
        queued_processing(&rig.server).await,
        vec![json!({ "session_id": replay, "cache_path": file.to_string_lossy() })]
    );

    processing_ran(&rig.server).await;
    process_download(&rig.server, replay, &file).await;
    end_playback(&rig.server, replay).await;
    rig.resolve_with(&body).await;
    assert_eq!(queued_processing(&rig.server).await, Vec::<Value>::new());
}

async fn analysis_off(server: &TestServer) {
    server
        .runtime_config
        .update(RuntimeConfigUpdate {
            enrichment_enabled: Some(false),
            skip_detect_mode: Some("full".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
}

/// A Good Girl's Guide to Murder S1, as prod analysed it: the titles are a ~30 s card
/// over music that changes every episode, so only the picture repeats. Episode 1 is
/// another release (480p) whose picture barely matches the rest.
const AGG: [(&[u8], &[u8], f64); 4] = [
    (
        include_bytes!("../fixtures/chromaprint/agg_s01e01.bin"),
        include_bytes!("../fixtures/vhash/agg_s01e01_intro.bin"),
        2406.522,
    ),
    (
        include_bytes!("../fixtures/chromaprint/agg_s01e02.bin"),
        include_bytes!("../fixtures/vhash/agg_s01e02_intro.bin"),
        2667.168,
    ),
    (
        include_bytes!("../fixtures/chromaprint/agg_s01e03.bin"),
        include_bytes!("../fixtures/vhash/agg_s01e03_intro.bin"),
        2603.424,
    ),
    (
        include_bytes!("../fixtures/chromaprint/agg_s01e04.bin"),
        include_bytes!("../fixtures/vhash/agg_s01e04_intro.bin"),
        2672.16,
    ),
];

/// Where each AGG episode's title card sits.
const AGG_INTROS: [Option<(f64, f64)>; 4] = [
    None,
    Some((157.6, 187.6)),
    Some((51.2, 81.3)),
    Some((17.2, 46.9)),
];

fn near(found: &[(f64, f64)], expected: Option<(f64, f64)>) -> bool {
    match (found, expected) {
        ([], None) => true,
        ([(s, e)], Some((xs, xe))) => (s - xs).abs() < 1.0 && (e - xe).abs() < 1.0,
        _ => false,
    }
}

/// The AGG season as debrid downloads, each analysed, with the session that played it.
async fn agg_debrid_season(
    server: &TestServer,
    downloads: &Path,
) -> Vec<(Uuid, std::path::PathBuf)> {
    let episodes = season_of(server, 4).await;
    let mut sessions = Vec::new();
    for (n, (episode, agg)) in episodes.iter().zip(AGG).enumerate() {
        let release = downloads.join(format!("AGG_S01E0{}.mkv", n + 1));
        std::fs::write(&release, LETTERBOXED_FIXTURE).unwrap();
        let session = analysed_release(server, *episode, &format!("agg{n}"), &release, agg).await;
        sessions.push((session, release));
    }
    sessions
}

/// Three of four episodes share the card; the fourth is another release. An intro that
/// two siblings agree on, and that those two also find in each other, is the season's
/// intro whether it is heard or only seen.
#[tokio::test]
async fn a_debrid_season_finds_an_intro_that_is_only_on_screen() {
    let server = TestServer::start_api_only().await;
    analysis_off(&server).await;
    let downloads = tempfile::tempdir().unwrap();
    let sessions = agg_debrid_season(&server, downloads.path()).await;

    let (session, release) = &sessions[1];
    process_download(&server, *session, release).await;

    let mut found = Vec::new();
    for (session, _) in &sessions {
        found.push(stored_spans(&server, *session, "intro").await);
    }
    assert!(
        found.iter().zip(&AGG_INTROS).all(|(f, e)| near(f, *e)),
        "{found:?}"
    );
}

/// What a detection run weighed for one file, as `skip_detect_candidates` keeps it.
async fn candidates(
    server: &TestServer,
    column: &str,
    id: Uuid,
) -> Vec<(String, i64, i64, bool, Option<String>)> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT segment_type, round(start_seconds)::int8, round(end_seconds)::int8, selected,
                rejection_reason
         FROM skip_detect_candidates WHERE {column} = $1 AND config_tag IS NOT NULL
         ORDER BY segment_type, start_seconds, selected"
    )))
    .bind(id)
    .fetch_all(&server.pool)
    .await
    .unwrap()
}

/// The AGG season as library files, analysed alike, and the library's detection run on it.
async fn agg_library_season(server: &TestServer) -> Vec<Uuid> {
    let episodes = season_of(server, 4).await;
    let mut files = Vec::new();
    for (n, (episode, (fingerprint, vhash, duration))) in episodes.iter().zip(AGG).enumerate() {
        let file_id = tv_episode_files::create(
            &server.pool,
            tv_episode_files::EpisodeFileInput {
                file_path: format!("/library/AGG/S01E0{}.mkv", n + 1),
                file_size: Some(1),
                file_fingerprint: None,
                format: "mkv".to_string(),
                quality: None,
                languages: None,
                subtitles: None,
                source_type: Some("test".to_string()),
                source_metadata: None,
                video_codec: None,
                video_width: None,
                video_height: None,
                video_bitrate_kbps: None,
                framerate: None,
                bit_depth: None,
                is_hdr: false,
                is_interlaced: false,
                hdr_format: None,
                duration_seconds: Some(duration),
                container: None,
                overall_bitrate_kbps: None,
            },
        )
        .await
        .unwrap();
        tv_episode_files::link_episodes(&server.pool, file_id, &[*episode])
            .await
            .unwrap();
        tv_episode_files::set_media_cache_version(
            &server.pool,
            file_id,
            api::media_cache::PIPELINE_VERSION,
        )
        .await
        .unwrap();
        stage_cache(
            &episode_cache_dir(&server.storage_root(), &MediaKind::TvFile { file_id }),
            fingerprint,
            vhash,
            duration,
        );
        files.push(file_id);
    }
    use api::jobs::worker::JobHandler;
    let (library_events, _) = tokio::sync::broadcast::channel(16);
    api::jobs::handlers::SkipDetectHandler::new(
        server.pool.clone(),
        server.nats_publisher.clone(),
        server.runtime_config.clone(),
        server.storage_root(),
        library_events,
    )
    .execute(
        Uuid::now_v7(),
        json!({ "file_id": files[0], "media_type": "episode", "run_id": Uuid::now_v7() }),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    files
}

async fn library_spans(server: &TestServer, file_id: Uuid, kind: &str) -> Vec<(f64, f64)> {
    sqlx::query_as(
        "SELECT start_seconds, end_seconds FROM episode_skip_segments
         WHERE episode_file_id = $1 AND segment_type = $2",
    )
    .bind(file_id)
    .bind(kind)
    .fetch_all(&server.pool)
    .await
    .unwrap()
}

/// The same analyses give the same intros and the same audit trail whether the files
/// are in the library or streamed from debrid: both go through one detector.
#[tokio::test]
async fn library_and_debrid_detect_alike_from_the_same_analyses() {
    let server = TestServer::start_api_only().await;
    analysis_off(&server).await;
    let library = agg_library_season(&server).await;
    let downloads = tempfile::tempdir().unwrap();
    let debrid = agg_debrid_season(&server, downloads.path()).await;
    process_download(&server, debrid[1].0, &debrid[1].1).await;

    for (n, (file_id, (session, release))) in library.iter().zip(&debrid).enumerate() {
        let debrid_file = debrid_files::at(&server.pool, &release.to_string_lossy())
            .await
            .unwrap()
            .unwrap();
        for kind in ["intro", "credits"] {
            assert_eq!(
                library_spans(&server, *file_id, kind).await,
                stored_spans(&server, *session, kind).await,
                "episode {} {kind}",
                n + 1
            );
        }
        assert_eq!(
            candidates(&server, "file_id", *file_id).await,
            candidates(&server, "debrid_file_id", debrid_file.id).await,
            "episode {}",
            n + 1
        );
    }
    assert!(
        library_spans(&server, library[1], "intro").await.len() == 1,
        "the season's intro was found at all"
    );
}

/// Two releases of one episode are cut differently, so each keeps its own chapters and
/// its own analysis.
#[tokio::test]
async fn each_release_of_an_episode_keeps_its_own_chapters_and_analysis() {
    let server = TestServer::start_api_only().await;
    analysis_on(&server, None).await;
    let episode = season_of(&server, 1).await[0];
    let downloads = tempfile::tempdir().unwrap();
    let mut releases = Vec::new();
    for (hash, name, fixture) in [
        ("plainhash", "Show.S01E01.mkv", ITA_ENG_FIXTURE),
        ("chapterhash", "Show.S01E01.Chapters.mkv", CHAPTERED_FIXTURE),
    ] {
        let path = downloads.path().join(name);
        std::fs::write(&path, fixture).unwrap();
        let session = downloaded_session(&server, "episode", episode, hash, &path).await;
        process_download(&server, session, &path).await;
        releases.push((session, path));
    }

    let chapters = |path: &Path| {
        let pool = server.pool.clone();
        let path = path.to_string_lossy().into_owned();
        async move {
            let file = debrid_files::at(&pool, &path).await.unwrap().unwrap();
            debrid_files::chapters(&pool, file.id).await.unwrap().len()
        }
    };
    assert_eq!(chapters(&releases[0].1).await, 0);
    assert!(chapters(&releases[1].1).await > 0);
    assert_ne!(
        file_cache(&server, &releases[0].1).await,
        file_cache(&server, &releases[1].1).await
    );
}

/// Playing one release and then another, back and forth, analyses neither again: each
/// keeps its own analysis instead of taking over the title's.
#[tokio::test]
async fn alternating_between_two_releases_analyses_neither_again() {
    let rig = Rig::start(UpstreamPlan::new(ITA_ENG_FIXTURE.to_vec()), "film.mkv").await;
    analysis_on(&rig.server, None).await;
    let movie = rig.movie().await;
    let bodies: Vec<Value> = ["releasea", "releaseb"]
        .iter()
        .map(|hash| rig.resolve_body(hash, movie, direct_play_caps()))
        .collect();
    for (hash, body) in ["releasea", "releaseb"].iter().zip(&bodies) {
        let session = session_id_of(&rig.resolve_with(body).await);
        rig.wait_for_complete(hash).await;
        let server = &rig.server;
        wait_until_async(
            std::time::Duration::from_secs(10),
            "the finished download was never queued for processing",
            || async move { !queued_processing(server).await.is_empty() },
        )
        .await;
        processing_ran(&rig.server).await;
        process_download(&rig.server, session, &rig.cache_file(hash)).await;
        end_playback(&rig.server, session).await;
    }

    for body in bodies.iter().chain(&bodies) {
        let replay = session_id_of(&rig.resolve_with(body).await);
        assert_eq!(
            queued_processing(&rig.server).await,
            Vec::<Value>::new(),
            "{} was analysed again",
            body["release_key"]
        );
        end_playback(&rig.server, replay).await;
    }
}

/// A download with broken stretches is analysed as a library file is, and its next
/// session starts out knowing where they are.
#[tokio::test]
async fn corrupt_ranges_found_in_a_download_reach_its_next_session() {
    let rig = Rig::start(
        UpstreamPlan::new(include_bytes!("../fixtures/corrupt/damaged.mkv").to_vec()),
        "damaged.mkv",
    )
    .await;
    analysis_on(&rig.server, None).await;
    let movie = rig.movie().await;
    let body = rig.resolve_body("damagedhash", movie, direct_play_caps());
    let first = session_id_of(&rig.resolve_with(&body).await);
    rig.wait_for_complete("damagedhash").await;
    let file = rig.cache_file("damagedhash");
    process_download(&rig.server, first, &file).await;
    end_playback(&rig.server, first).await;

    let stored = debrid_files::corrupt_ranges_at(&rig.server.pool, &file.to_string_lossy())
        .await
        .unwrap();
    assert!(!stored.is_empty(), "analysis found nothing broken");

    let replay = session_id_of(&rig.resolve_with(&body).await);
    let session = rig
        .server
        .app_state
        .playback_sessions
        .get(&replay)
        .expect("the replay is playing");
    assert_eq!(session.corrupt_ranges, stored);
}

const MOVIE_SECS: f64 = 6000.0;

/// A movie analysed with a fade to black where its credits start.
fn stage_movie_cache(dir: &Path) {
    stage_cache(dir, &[], &no_vhash(), MOVIE_SECS);
    write_sidecar_at(
        dir,
        Branch::BlackFrames,
        br#"[{"start_secs":5500.0,"end_secs":5503.0,"duration_secs":3.0}]"#,
    )
    .unwrap();
}

fn end_credits_chapters() -> Vec<api::routes::admin::streaming::ChapterResponse> {
    [("Movie", 0.0, 5500.0), ("End Credits", 5500.0, MOVIE_SECS)]
        .into_iter()
        .enumerate()
        .map(
            |(n, (title, start, end))| api::routes::admin::streaming::ChapterResponse {
                index: n as i32,
                title: Some(title.to_string()),
                start_seconds: start,
                end_seconds: end,
            },
        )
        .collect()
}

async fn chapter_agreement(server: &TestServer, column: &str, id: Uuid) -> Vec<Option<f64>> {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT chapter_agreement FROM skip_detect_candidates
         WHERE {column} = $1 AND segment_type = 'credits'"
    )))
    .bind(id)
    .fetch_all(&server.pool)
    .await
    .unwrap()
}

/// A movie's chapters are evidence for its credits, whether the movie is a library file
/// or a debrid download: an "End Credits" chapter where the picture fades out agrees.
#[tokio::test]
async fn a_movies_chapters_weigh_in_on_its_credits_in_the_library_and_from_debrid() {
    use api::jobs::worker::JobHandler;
    use api::repositories::{movie_files, video_chapters};
    let server = TestServer::start_api_only().await;
    analysis_off(&server).await;

    let movie: Uuid = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": "Chaptered", "year": 2024 }),
        )
        .await
        .json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let library_file = movie_files::create(
        &server.pool,
        movie_files::MovieFileInput {
            movie_id: movie,
            file_path: "/library/Chaptered.mkv".to_string(),
            format: "mkv".to_string(),
            duration_seconds: Some(MOVIE_SECS),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let chapters: Vec<video_chapters::ChapterInput> = end_credits_chapters()
        .into_iter()
        .map(|c| video_chapters::ChapterInput {
            movie_file_id: Some(library_file),
            episode_file_id: None,
            chapter_index: c.index,
            title: c.title,
            start_seconds: c.start_seconds,
            end_seconds: c.end_seconds,
        })
        .collect();
    video_chapters::insert_chapters(&server.pool, &chapters)
        .await
        .unwrap();
    stage_movie_cache(&episode_cache_dir(
        &server.storage_root(),
        &MediaKind::Movie { movie_id: movie },
    ));
    let (library_events, _) = tokio::sync::broadcast::channel(16);
    api::jobs::handlers::SkipDetectHandler::new(
        server.pool.clone(),
        server.nats_publisher.clone(),
        server.runtime_config.clone(),
        server.storage_root(),
        library_events,
    )
    .execute(
        Uuid::now_v7(),
        json!({ "file_id": library_file, "media_type": "movie", "run_id": Uuid::now_v7(), "force": true }),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();

    let downloads = tempfile::tempdir().unwrap();
    let release = downloads.path().join("Chaptered.2024.mkv");
    std::fs::write(&release, LETTERBOXED_FIXTURE).unwrap();
    let download = debrid_files::upsert(&server.pool, &release.to_string_lossy(), "movie", movie)
        .await
        .unwrap();
    debrid_files::set_chapters(&server.pool, download.id, &end_credits_chapters())
        .await
        .unwrap();
    stage_movie_cache(&episode_cache_dir(
        &server.storage_root(),
        &MediaKind::DebridFile {
            file_id: download.id,
        },
    ));
    let session = downloaded_session(&server, "movie", movie, "chapteredhash", &release).await;
    process_download(&server, session, &release).await;

    for (scope, column, id) in [
        ("library", "movie_file_id", library_file),
        ("debrid", "debrid_file_id", download.id),
    ] {
        assert_eq!(
            chapter_agreement(&server, column, id).await,
            vec![Some(0.10)],
            "{scope}"
        );
    }
}
