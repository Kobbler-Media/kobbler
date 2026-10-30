use crate::common::*;
use api::jobs::handlers::{OfflineExportGcHandler, OfflineExportHandler};
use api::repositories::{movie_files, offline_exports};
use api::streaming::offline_export::export_dir;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use uuid::Uuid;

const EXPORTS: &str = "/api/v1/admin/offline/exports";
const ENG_STREAM: i64 = 1;
const FRE_STREAM: i64 = 2;

/// 960x540 with an English track then a French one, so both the scaler and the
/// language pick have something to decide.
fn source_mkv() -> &'static [u8] {
    static SOURCE: OnceLock<Vec<u8>> = OnceLock::new();
    SOURCE.get_or_init(|| {
        let file = tempfile::NamedTempFile::with_suffix(".mkv").unwrap();
        let status = std::process::Command::new("ffmpeg")
            .args(
                "-v error -y -f lavfi -i testsrc=s=960x540:r=10 \
                 -f lavfi -i sine=f=440:r=48000 -f lavfi -i sine=f=880:r=48000 -t 3 \
                 -map 0 -map 1 -map 2 -c:v libx264 -preset ultrafast -g 10 \
                 -c:a aac -b:a 64k -ac 2 \
                 -metadata:s:a:0 language=eng -metadata:s:a:1 language=fre \
                 -disposition:a:0 default -disposition:a:1 0"
                    .split_whitespace(),
            )
            .arg(file.path())
            .status()
            .expect("ffmpeg");
        assert!(status.success());
        std::fs::read(file.path()).unwrap()
    })
}

struct Seeded {
    movie_id: Uuid,
    source: PathBuf,
}

async fn seed_movie(server: &TestServer) -> Seeded {
    let movie = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": "Offline", "year": 2024 }),
        )
        .await;
    movie.assert_status(StatusCode::CREATED);
    let movie_id: Uuid = movie.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let source = Path::new(&server.storage_root()).join(format!("{movie_id}.mkv"));
    std::fs::write(&source, source_mkv()).unwrap();
    movie_files::create(
        &server.pool,
        movie_files::MovieFileInput {
            movie_id,
            file_path: source.to_string_lossy().into_owned(),
            file_size: Some(source_mkv().len() as i64),
            format: "mkv".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    Seeded { movie_id, source }
}

async fn request_export(server: &TestServer, body: Value) -> TestResponse {
    server.admin_client().post_json(EXPORTS, &body).await
}

async fn create_export(server: &TestServer, movie_id: Uuid, quality: &str, lang: &str) -> Value {
    let resp = request_export(
        server,
        json!({
            "media_type": "movie",
            "media_id": movie_id.to_string(),
            "quality": quality,
            "preferred_audio_language": lang,
        }),
    )
    .await;
    resp.assert_ok();
    resp.json()
}

async fn get_export(server: &TestServer, id: &str) -> Value {
    let resp = server.admin_client().get(&format!("{EXPORTS}/{id}")).await;
    resp.assert_ok();
    resp.json()
}

async fn run_export_jobs(server: &TestServer) {
    server
        .process_pending_jobs(&OfflineExportHandler::new(server.app_state.clone()))
        .await;
}

async fn run_gc(server: &TestServer) {
    server
        .process_pending_jobs(&OfflineExportGcHandler::new(
            server.pool.clone(),
            server.storage_root(),
        ))
        .await;
}

async fn enqueue_gc(server: &TestServer) {
    api::jobs::enqueue(
        &server.pool,
        &server.nats_publisher,
        "offline_export_gc",
        json!({}),
        api::jobs::JobContext::new("test gc"),
    )
    .await
    .unwrap();
}

async fn export_job_payloads(server: &TestServer, export_id: &str) -> Vec<Value> {
    sqlx::query_scalar::<_, Value>(
        "SELECT payload FROM job_status WHERE job_type = 'offline_export' \
         AND payload->>'export_id' = $1 ORDER BY created_at",
    )
    .bind(export_id)
    .fetch_all(&server.pool)
    .await
    .unwrap()
}

fn assert_export_shape(export: &Value) {
    let obj = export.as_object().unwrap();
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "error",
            "export_id",
            "file_url",
            "progress",
            "size_bytes",
            "state"
        ],
        "{export}"
    );
    let id = export["export_id"].as_str().unwrap();
    assert_eq!(
        export["file_url"],
        format!("/api/v1/admin/offline/exports/{id}/file")
    );
}

struct Probed {
    width: u64,
    height: u64,
    video_codec: String,
    audio: Vec<(String, u64)>,
    duration: f64,
}

fn probe(bytes: &[u8]) -> Probed {
    let file = tempfile::NamedTempFile::with_suffix(".mp4").unwrap();
    std::fs::write(file.path(), bytes).unwrap();
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,codec_name,width,height,channels:format=duration",
            "-of",
            "json",
        ])
        .arg(file.path())
        .output()
        .expect("ffprobe");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let streams = v["streams"].as_array().unwrap();
    let video = streams
        .iter()
        .find(|s| s["codec_type"] == "video")
        .expect("video stream");
    Probed {
        width: video["width"].as_u64().unwrap(),
        height: video["height"].as_u64().unwrap(),
        video_codec: video["codec_name"].as_str().unwrap().to_string(),
        audio: streams
            .iter()
            .filter(|s| s["codec_type"] == "audio")
            .map(|s| {
                (
                    s["codec_name"].as_str().unwrap().to_string(),
                    s["channels"].as_u64().unwrap(),
                )
            })
            .collect(),
        duration: v["format"]["duration"].as_str().unwrap().parse().unwrap(),
    }
}

fn top_level_boxes(bytes: &[u8]) -> Vec<String> {
    let mut boxes = Vec::new();
    let mut at = 0usize;
    while at + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        boxes.push(String::from_utf8_lossy(&bytes[at + 4..at + 8]).into_owned());
        if size < 8 {
            break;
        }
        at += size;
    }
    boxes
}

#[tokio::test]
async fn an_export_is_transcoded_to_one_mp4_served_with_ranges() {
    let server = TestServer::start_api_only().await;
    let seeded = seed_movie(&server).await;

    for (quality, width, height) in [("480p", 852, 480), ("original", 960, 540)] {
        let created = create_export(&server, seeded.movie_id, quality, "fre").await;
        assert_export_shape(&created);
        assert_eq!(created["state"], "queued", "{quality}: {created}");
        assert_eq!(created["progress"], 0.0);
        assert_eq!(created["size_bytes"], Value::Null);
        assert_eq!(created["error"], Value::Null);
        let id = created["export_id"].as_str().unwrap().to_string();
        let file_url = created["file_url"].as_str().unwrap().to_string();

        server
            .admin_client()
            .get(&file_url)
            .await
            .assert_status(StatusCode::CONFLICT);

        let payloads = export_job_payloads(&server, &id).await;
        assert_eq!(payloads.len(), 1, "{quality}");
        assert_eq!(payloads[0]["audio_stream_index"], FRE_STREAM, "{quality}");

        run_export_jobs(&server).await;

        let ready = get_export(&server, &id).await;
        assert_export_shape(&ready);
        assert_eq!(ready["state"], "ready", "{quality}: {ready}");
        assert_eq!(ready["progress"], 1.0);
        assert_eq!(ready["error"], Value::Null);
        let size = ready["size_bytes"].as_u64().unwrap();

        let full = server.admin_client().get(&file_url).await;
        full.assert_ok();
        full.assert_header("content-type", "video/mp4");
        full.assert_header("accept-ranges", "bytes");
        full.assert_header("content-length", &size.to_string());
        assert_eq!(full.bytes().len() as u64, size);

        let probed = probe(full.bytes());
        assert_eq!(
            (probed.width, probed.height),
            (width, height),
            "{quality} dimensions"
        );
        assert_eq!(probed.video_codec, "h264");
        assert_eq!(
            probed.audio,
            vec![("aac".to_string(), 2)],
            "{quality} audio"
        );
        assert!(
            (probed.duration - 3.0).abs() < 0.5,
            "{quality} duration {}",
            probed.duration
        );

        let boxes = top_level_boxes(full.bytes());
        let moov = boxes.iter().position(|b| b == "moov").expect("moov");
        let mdat = boxes.iter().position(|b| b == "mdat").expect("mdat");
        assert!(
            moov < mdat,
            "{quality}: moov should precede mdat: {boxes:?}"
        );

        let partial = server
            .admin_client()
            .get_with_header(&file_url, "Range", "bytes=100-1123")
            .await;
        partial.assert_status(StatusCode::PARTIAL_CONTENT);
        partial.assert_header("content-range", &format!("bytes 100-1123/{size}"));
        partial.assert_header("content-length", "1024");
        partial.assert_header("content-type", "video/mp4");
        partial.assert_header("accept-ranges", "bytes");
        assert_eq!(partial.bytes(), &full.bytes()[100..1124]);

        let tail = server
            .admin_client()
            .get_with_header(&file_url, "Range", "bytes=-10")
            .await;
        tail.assert_status(StatusCode::PARTIAL_CONTENT);
        assert_eq!(tail.bytes(), &full.bytes()[size as usize - 10..]);

        let dir = export_dir(&server.storage_root(), id.parse().unwrap());
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["export.mp4"], "{quality}: work dir cleaned up");
    }
}

#[tokio::test]
async fn the_same_file_quality_and_audio_reuse_one_export_and_a_failure_is_requeued() {
    let server = TestServer::start_api_only().await;
    let seeded = seed_movie(&server).await;

    let first = create_export(&server, seeded.movie_id, "720p", "fre").await;
    let again = create_export(&server, seeded.movie_id, "720p", "fra").await;
    let id = first["export_id"].as_str().unwrap().to_string();
    assert_eq!(again["export_id"], first["export_id"]);
    assert_eq!(export_job_payloads(&server, &id).await.len(), 1);

    let english = create_export(&server, seeded.movie_id, "720p", "eng").await;
    assert_ne!(english["export_id"], first["export_id"]);
    let english_jobs = export_job_payloads(&server, english["export_id"].as_str().unwrap()).await;
    assert_eq!(english_jobs[0]["audio_stream_index"], ENG_STREAM);
    let other_quality = create_export(&server, seeded.movie_id, "480p", "fre").await;
    assert_ne!(other_quality["export_id"], first["export_id"]);

    std::fs::remove_file(&seeded.source).unwrap();
    run_export_jobs(&server).await;
    let failed = get_export(&server, &id).await;
    assert_eq!(failed["state"], "failed", "{failed}");
    assert!(failed["error"].as_str().is_some_and(|e| !e.is_empty()));
    assert_eq!(failed["size_bytes"], Value::Null);

    std::fs::write(&seeded.source, source_mkv()).unwrap();
    let requeued = create_export(&server, seeded.movie_id, "720p", "fre").await;
    // A rebuilt export is a different file, so it must not answer to the id a client may
    // already hold half a download of.
    assert_ne!(requeued["export_id"], first["export_id"]);
    assert_eq!(requeued["state"], "queued", "{requeued}");
    assert_eq!(requeued["error"], Value::Null);
    assert_eq!(requeued["progress"], 0.0);
    assert_eq!(
        export_job_payloads(&server, requeued["export_id"].as_str().unwrap())
            .await
            .len(),
        1
    );
    get_status(&server, &id)
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

async fn get_status(server: &TestServer, id: &str) -> TestResponse {
    server.admin_client().get(&format!("{EXPORTS}/{id}")).await
}

#[tokio::test]
async fn an_export_whose_job_went_quiet_is_queued_again() {
    let server = TestServer::start_api_only().await;
    let seeded = seed_movie(&server).await;
    let first = create_export(&server, seeded.movie_id, "720p", "eng").await;
    let id = first["export_id"].as_str().unwrap().to_string();
    sqlx::query("UPDATE offline_exports SET updated_at = NOW() - INTERVAL '2 hours' WHERE id = $1")
        .bind(Uuid::parse_str(&id).unwrap())
        .execute(&server.pool)
        .await
        .unwrap();

    let again = create_export(&server, seeded.movie_id, "720p", "eng").await;
    assert_eq!(again["export_id"], first["export_id"]);
    assert_eq!(export_job_payloads(&server, &id).await.len(), 2);
    create_export(&server, seeded.movie_id, "720p", "eng").await;
    assert_eq!(export_job_payloads(&server, &id).await.len(), 2);
}

#[tokio::test]
async fn a_kids_profile_cannot_read_an_export_of_a_title_it_may_not_play() {
    let server = TestServer::start_api_only().await;
    let seeded = seed_movie(&server).await;
    let export = create_export(&server, seeded.movie_id, "480p", "eng").await;
    let id = export["export_id"].as_str().unwrap();
    run_export_jobs(&server).await;

    let kids = crate::kids_mode_test::kids_client(&server, "kid@test.com").await;
    kids.get(&format!("{EXPORTS}/{id}"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    kids.get(&format!("{EXPORTS}/{id}/file"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_media_bad_input_and_unready_exports_are_rejected() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();
    let seeded = seed_movie(&server).await;
    let unknown = Uuid::new_v4().to_string();

    let fileless = client
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": "No File", "year": 2024 }),
        )
        .await;
    let fileless_id = fileless.json::<Value>()["id"].as_str().unwrap().to_string();

    let movie_id = seeded.movie_id.to_string();
    let cases = [
        (
            json!({"media_type": "movie", "media_id": unknown, "quality": "720p", "preferred_audio_language": null}),
            StatusCode::NOT_FOUND,
        ),
        (
            json!({"media_type": "episode", "media_id": unknown, "quality": "720p", "preferred_audio_language": null}),
            StatusCode::NOT_FOUND,
        ),
        (
            json!({"media_type": "movie", "media_id": fileless_id, "quality": "720p", "preferred_audio_language": null}),
            StatusCode::NOT_FOUND,
        ),
        (
            json!({"media_type": "movie", "media_id": movie_id, "quality": "4k", "preferred_audio_language": null}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"media_type": "recording", "media_id": movie_id, "quality": "720p", "preferred_audio_language": null}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"media_type": "book", "media_id": movie_id, "quality": "720p"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"media_type": "movie", "media_id": "nope", "quality": "720p"}),
            StatusCode::BAD_REQUEST,
        ),
    ];
    for (body, status) in cases {
        let resp = client.post_json(EXPORTS, &body).await;
        assert_eq!(resp.status, status, "{body}: {}", resp.text());
    }

    let created = create_export(&server, seeded.movie_id, "1080p", "").await;
    assert_eq!(created["state"], "queued");

    for path in [
        format!("{EXPORTS}/{unknown}"),
        format!("{EXPORTS}/{unknown}/file"),
        format!("{EXPORTS}/not-a-uuid"),
    ] {
        client.get(&path).await.assert_status(StatusCode::NOT_FOUND);
    }
    client
        .get(created["file_url"].as_str().unwrap())
        .await
        .assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn offline_export_routes_require_auth() {
    let server = TestServer::start_api_only().await;
    let seeded = seed_movie(&server).await;
    let created = create_export(&server, seeded.movie_id, "720p", "eng").await;
    let id = created["export_id"].as_str().unwrap();
    let anon = server.api_client();

    anon.post_json(
        EXPORTS,
        &json!({"media_type": "movie", "media_id": seeded.movie_id.to_string(), "quality": "720p", "preferred_audio_language": null}),
    )
    .await
    .assert_status(StatusCode::UNAUTHORIZED);
    anon.get(&format!("{EXPORTS}/{id}"))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    anon.get(&format!("{EXPORTS}/{id}/file"))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn gc_drops_idle_and_old_failed_exports_and_orphaned_dirs() {
    let server = TestServer::start_api_only().await;
    let root = server.storage_root();

    let make = |quality: &'static str| {
        let pool = server.pool.clone();
        let root = root.clone();
        async move {
            let (export, _) =
                offline_exports::find_or_create(&pool, "movie", Uuid::new_v4(), quality, 1)
                    .await
                    .unwrap();
            let dir = export_dir(&root, export.id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("export.mp4"), b"not really a video").unwrap();
            export.id
        }
    };
    let backdate = |id: Uuid, column: &'static str, days: i32| {
        let pool = server.pool.clone();
        async move {
            let sql = match column {
                "last_accessed_at" => {
                    "UPDATE offline_exports SET last_accessed_at = NOW() - make_interval(days => $2) WHERE id = $1"
                }
                _ => {
                    "UPDATE offline_exports SET updated_at = NOW() - make_interval(days => $2) WHERE id = $1"
                }
            };
            sqlx::query(sql)
                .bind(id)
                .bind(days)
                .execute(&pool)
                .await
                .unwrap();
        }
    };

    let idle = make("720p").await;
    offline_exports::mark_ready(&server.pool, idle, 18)
        .await
        .unwrap();
    backdate(idle, "last_accessed_at", 15).await;

    let touched = make("720p").await;
    offline_exports::mark_ready(&server.pool, touched, 18)
        .await
        .unwrap();
    backdate(touched, "last_accessed_at", 15).await;
    server
        .admin_client()
        .get(&format!("{EXPORTS}/{touched}/file"))
        .await
        .assert_ok();

    let old_failure = make("480p").await;
    offline_exports::mark_failed(&server.pool, old_failure, "boom")
        .await
        .unwrap();
    backdate(old_failure, "updated_at", 2).await;

    let new_failure = make("480p").await;
    offline_exports::mark_failed(&server.pool, new_failure, "boom")
        .await
        .unwrap();

    let queued = make("1080p").await;

    let orphan = export_dir(&root, Uuid::new_v4());
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("export.mp4"), b"left behind").unwrap();

    enqueue_gc(&server).await;
    run_gc(&server).await;

    for (id, kept) in [
        (idle, false),
        (touched, true),
        (old_failure, false),
        (new_failure, true),
        (queued, true),
    ] {
        let row = offline_exports::get(&server.pool, id).await.unwrap();
        assert_eq!(row.is_some(), kept, "{id} row");
        assert_eq!(export_dir(&root, id).exists(), kept, "{id} dir");
    }
    assert!(!orphan.exists(), "orphaned dir removed");
}
