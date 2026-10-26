use crate::common::audiobooks::*;
use crate::common::*;
use api::gpu::GpuLock;
use api::jobs::handlers::{AudiobookTranscribeHandler, TranscribeBackfillHandler};
use api::jobs::worker::JobHandler;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wiremock::{Mock, MockServer, ResponseTemplate};

type Seg<'a> = (f64, f64, &'a str);

fn stt_body(segments: &[(f64, f64, String)]) -> Value {
    json!({
        "text": segments.iter().map(|s| s.2.clone()).collect::<Vec<_>>().join(" "),
        "segments": segments
            .iter()
            .map(|(start, end, text)| json!({"start": start, "end": end, "text": text}))
            .collect::<Vec<_>>(),
    })
}

async fn stub_stt(main: &[Seg<'_>]) -> MockServer {
    let mock = MockServer::start().await;
    let main: Vec<(f64, f64, String)> = main
        .iter()
        .map(|(s, e, t)| (*s, *e, t.to_string()))
        .collect();
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(stt_body(&main)))
        .mount(&mock)
        .await;
    mock
}

async fn transcription_requests(mock: &MockServer) -> usize {
    mock.received_requests().await.unwrap().len()
}

async fn configure_stt(server: &TestServer, stt_url: &str, via_koblem: Option<&str>) {
    let mut patch = json!({"stt_service_url": stt_url, "route_gpu_via_koblem": false});
    if let Some(koblem) = via_koblem {
        patch["koblem_service_url"] = json!(koblem);
        patch["route_gpu_via_koblem"] = json!(true);
    }
    server
        .admin_client()
        .patch_json("/api/v1/admin/config", &patch)
        .await
        .assert_ok();
}

fn transcriber(server: &TestServer) -> AudiobookTranscribeHandler {
    AudiobookTranscribeHandler::new(
        server.pool.clone(),
        server.storage_root(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.search_backend_arc(),
        GpuLock::new(),
    )
}

fn backfill(server: &TestServer) -> TranscribeBackfillHandler {
    TranscribeBackfillHandler::new(
        server.pool.clone(),
        server.nats_publisher.clone(),
        server.runtime_config.clone(),
        Arc::new(AtomicBool::new(true)),
    )
}

async fn transcribe(server: &TestServer, book_id: Uuid, manual: bool) -> Value {
    transcriber(server)
        .execute(
            Uuid::now_v7(),
            json!({"book_id": book_id, "manual": manual}),
            CancellationToken::new(),
        )
        .await
        .unwrap()
}

async fn run_backfill(server: &TestServer) -> Value {
    backfill(server)
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await
        .unwrap()
}

async fn stored(server: &TestServer, file_id: Uuid) -> Vec<(i32, f64, String)> {
    sqlx::query_as(
        "SELECT segment_index, start_seconds, text FROM audiobook_transcripts
         WHERE audiobook_file_id = $1 ORDER BY segment_index",
    )
    .bind(file_id)
    .fetch_all(&server.pool)
    .await
    .unwrap()
}

async fn mark_available(server: &TestServer, file_ids: &[Uuid]) {
    let root_id = Uuid::now_v7();
    sqlx::query("INSERT INTO media_roots (id, path, media_type) VALUES ($1, $2, 'book')")
        .bind(root_id)
        .bind(format!("/transcribe-test/{root_id}"))
        .execute(&server.pool)
        .await
        .unwrap();
    for id in file_ids {
        sqlx::query(
            "INSERT INTO media_root_files (id, media_root_id, file_path, file_name, available)
             SELECT $1, $2, file_path, 'f', true FROM audiobook_files WHERE id = $3",
        )
        .bind(Uuid::now_v7())
        .bind(root_id)
        .bind(id)
        .execute(&server.pool)
        .await
        .unwrap();
    }
}

async fn active_transcribe_jobs(server: &TestServer) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM job_status
         WHERE job_type = 'audiobook_transcribe' AND status IN ('pending', 'running')",
    )
    .fetch_one(&server.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn backfill_leaves_books_alone_once_transcribed_through_koblem() {
    let stt = stub_stt(&[(0.0, 9.8, "words")]).await;
    let server = TestServer::start_api_only().await;
    configure_stt(&server, "http://stt-engine:8023", Some(&stt.uri())).await;
    let (book_id, files) = seed_audiobook(
        &server,
        "Truckers",
        &[file("truckers.m4b", 10.0, Audio::Tone(10.0))],
    )
    .await;
    mark_available(&server, &files).await;

    transcribe(&server, book_id, false).await;
    let result = run_backfill(&server).await;

    assert_eq!(result["enqueued"], 0, "{result}");
}

#[tokio::test]
async fn backfill_paces_itself_by_audio_hours_and_waits_for_the_queue_to_drain() {
    let server = TestServer::start_api_only().await;
    configure_stt(&server, "http://stt-engine:8023", None).await;
    let mut files = Vec::new();
    for i in 0..5 {
        let (_, ids) = seed_audiobook(
            &server,
            &format!("Ten hour book {i}"),
            &[file("long.m4b", 10.0 * 3600.0, Audio::None)],
        )
        .await;
        files.extend(ids);
    }
    mark_available(&server, &files).await;

    let first = run_backfill(&server).await;
    assert_eq!(first["enqueued"], 3, "{first}");
    assert_eq!(active_transcribe_jobs(&server).await, 3);

    let second = run_backfill(&server).await;
    assert_eq!(second["enqueued"], 0, "{second}");
    assert_eq!(active_transcribe_jobs(&server).await, 3);
}

#[tokio::test]
async fn rerun_skips_current_files_unless_manually_requested() {
    let stt = stub_stt(&[(0.0, 9.8, "words")]).await;
    let server = TestServer::start_api_only().await;
    configure_stt(&server, &stt.uri(), None).await;
    let (book_id, _) = seed_audiobook(
        &server,
        "Diggers",
        &[file("diggers.m4b", 10.0, Audio::Tone(10.0))],
    )
    .await;

    let first = transcribe(&server, book_id, false).await;
    assert_eq!(first["transcribed"], 1, "{first}");

    let routine = transcribe(&server, book_id, false).await;
    assert_eq!(routine["transcribed"], 0, "{routine}");
    assert_eq!(routine["up_to_date"], 1, "{routine}");
    assert_eq!(transcription_requests(&stt).await, 1);

    let manual = transcribe(&server, book_id, true).await;
    assert_eq!(manual["transcribed"], 1, "{manual}");
    assert_eq!(transcription_requests(&stt).await, 2);
}

async fn transcribe_all(server: &TestServer) -> Value {
    server
        .admin_client()
        .post("/api/v1/admin/books/bulk-transcribe")
        .await
        .assert_ok()
        .json()
}

#[tokio::test]
async fn transcribe_all_queues_every_book_without_a_current_transcript() {
    let stt = stub_stt(&[(0.0, 9.8, "words")]).await;
    let server = TestServer::start_api_only().await;
    configure_stt(&server, &stt.uri(), None).await;
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({"transcript_mode": "manual"}),
        )
        .await
        .assert_ok();
    let mut files = Vec::new();
    let mut books = Vec::new();
    for title in ["Current", "Stale", "Untouched"] {
        let (book_id, ids) =
            seed_audiobook(&server, title, &[file("a.m4b", 10.0, Audio::Tone(10.0))]).await;
        files.extend(ids);
        books.push(book_id);
    }
    mark_available(&server, &files).await;
    transcribe(&server, books[0], false).await;
    transcribe(&server, books[1], false).await;
    sqlx::query(
        "UPDATE audiobook_transcripts SET config_tag = 'stt:old/g1' WHERE audiobook_file_id = $1",
    )
    .bind(files[1])
    .execute(&server.pool)
    .await
    .unwrap();

    let first = transcribe_all(&server).await;
    assert_eq!(first["queued"], 2, "{first}");
    assert_eq!(active_transcribe_jobs(&server).await, 2);

    let again = transcribe_all(&server).await;
    assert_eq!(again["queued"], 0, "{again}");
    assert_eq!(active_transcribe_jobs(&server).await, 2);
}

#[tokio::test]
async fn transcribe_all_is_refused_while_transcription_is_off() {
    let server = TestServer::start_api_only().await;
    server
        .admin_client()
        .post("/api/v1/admin/books/bulk-transcribe")
        .await
        .assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn transcribe_all_needs_a_login() {
    let server = TestServer::start_api_only().await;
    server
        .api_client()
        .post("/api/v1/admin/books/bulk-transcribe")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}
