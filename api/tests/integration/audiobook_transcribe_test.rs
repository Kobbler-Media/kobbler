use crate::common::audiobooks::*;
use crate::common::client::TestResponse;
use crate::common::*;
use api::gpu::GpuLock;
use api::jobs::handlers::{
    AudiobookTranscribeHandler, TranscribeBackfillHandler, TranscribeRefreshHandler,
};
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
    let main: Vec<(f64, f64, String)> = main
        .iter()
        .map(|(s, e, t)| (*s, *e, t.to_string()))
        .collect();
    stub_stt_responding(stt_body(&main)).await
}

async fn stub_stt_responding(body: Value) -> MockServer {
    let mock = MockServer::start().await;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&mock)
        .await;
    mock
}

fn word_body(words: &[Seg<'_>]) -> Value {
    let words: Vec<Value> = words
        .iter()
        .map(|(start, end, text)| json!({"start": start, "end": end, "text": text}))
        .collect();
    json!({
        "text": words.iter().map(|w| w["text"].as_str().unwrap()).collect::<Vec<_>>().join(" "),
        "segments": words,
        "words": words,
    })
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

async fn transcribe_all(server: &TestServer) -> TestResponse {
    server
        .admin_client()
        .post("/api/v1/admin/books/bulk-transcribe")
        .await
}

async fn backlog(server: &TestServer) -> Value {
    server
        .admin_client()
        .get("/api/v1/admin/books/transcript-backlog")
        .await
        .assert_ok()
        .json()
}

fn refresher(server: &TestServer) -> TranscribeRefreshHandler {
    TranscribeRefreshHandler::new(
        server.pool.clone(),
        server.nats_publisher.clone(),
        server.runtime_config.clone(),
        server.job_events.clone(),
    )
}

async fn set_transcript_mode(server: &TestServer, mode: &str) {
    server
        .admin_client()
        .patch_json("/api/v1/admin/config", &json!({"transcript_mode": mode}))
        .await
        .assert_ok();
}

async fn queued_transcriptions(server: &TestServer) -> Vec<Value> {
    sqlx::query_scalar(
        "SELECT payload FROM job_status
         WHERE job_type = 'audiobook_transcribe' AND status IN ('pending', 'running')
         ORDER BY created_at",
    )
    .fetch_all(&server.pool)
    .await
    .unwrap()
}

/// Books of `hours` each whose files carry a transcript from an older STT generation.
async fn outdated_books(server: &TestServer, count: usize, hours: f64) -> Vec<Uuid> {
    let mut books = Vec::new();
    let mut files = Vec::new();
    for i in 0..count {
        let (book_id, ids) = seed_audiobook(
            server,
            &format!("Outdated {i}"),
            &[file("old.m4b", hours * 3600.0, Audio::None)],
        )
        .await;
        add_transcript(server, ids[0], &[(0.0, "old words")]).await;
        books.push(book_id);
        files.extend(ids);
    }
    mark_available(server, &files).await;
    books
}

#[tokio::test]
async fn backfill_fills_in_untranscribed_books_but_leaves_outdated_ones_alone() {
    let server = TestServer::start_api_only().await;
    configure_stt(&server, "http://stt-engine:8023", None).await;
    outdated_books(&server, 1, 1.0).await;
    let (fresh, ids) =
        seed_audiobook(&server, "Fresh", &[file("new.m4b", 3600.0, Audio::None)]).await;
    mark_available(&server, &ids).await;

    let result = run_backfill(&server).await;

    assert_eq!(result["enqueued"], 1, "{result}");
    let queued = queued_transcriptions(&server).await;
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0]["book_id"], json!(fresh));
}

#[tokio::test]
async fn routine_transcription_only_fills_in_files_with_no_transcript() {
    let stt = stub_stt(&[(0.0, 4.8, "words")]).await;
    let server = TestServer::start_api_only().await;
    configure_stt(&server, &stt.uri(), None).await;
    let (book_id, files) = seed_audiobook(
        &server,
        "Half done",
        &[
            file("one.m4b", 5.0, Audio::Tone(5.0)),
            file("two.m4b", 5.0, Audio::Tone(5.0)),
        ],
    )
    .await;
    add_transcript(&server, files[0], &[(0.0, "old words")]).await;

    let routine = transcribe(&server, book_id, false).await;

    assert_eq!(routine["transcribed"], 1, "{routine}");
    assert_eq!(transcription_requests(&stt).await, 1);
    assert_eq!(stored(&server, files[0]).await[0].2, "old words");
    assert_eq!(stored(&server, files[1]).await[0].2, "words");

    let refresh = transcriber(&server)
        .execute(
            Uuid::now_v7(),
            json!({"book_id": book_id, "outdated": true}),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(refresh["transcribed"], 1, "{refresh}");
    assert_eq!(refresh["up_to_date"], 1, "{refresh}");
    assert_eq!(stored(&server, files[0]).await[0].2, "words");
}

#[tokio::test]
async fn backlog_reports_missing_and_outdated_audio_separately() {
    let server = TestServer::start_api_only().await;
    configure_stt(&server, "http://stt-engine:8023", None).await;
    outdated_books(&server, 2, 10.0).await;
    let (_, ids) = seed_audiobook(
        &server,
        "Fresh",
        &[file("new.m4b", 5.0 * 3600.0, Audio::None)],
    )
    .await;
    mark_available(&server, &ids).await;

    let backlog = backlog(&server).await;

    assert_eq!(backlog["outdated"]["books"], 2, "{backlog}");
    assert_eq!(backlog["outdated"]["files"], 2, "{backlog}");
    assert_eq!(backlog["outdated"]["hours"], 20.0, "{backlog}");
    assert_eq!(backlog["missing"]["books"], 1, "{backlog}");
    assert_eq!(backlog["missing"]["hours"], 5.0, "{backlog}");
    assert_eq!(backlog["refresh_job"], Value::Null, "{backlog}");
}

#[tokio::test]
async fn transcribe_all_runs_the_backlog_a_batch_at_a_time_until_cancelled() {
    let server = TestServer::start_api_only().await;
    configure_stt(&server, "http://stt-engine:8023", None).await;
    set_transcript_mode(&server, "manual").await;
    outdated_books(&server, 5, 10.0).await;

    let started: Value = transcribe_all(&server)
        .await
        .assert_status(StatusCode::ACCEPTED)
        .json();
    let job_id: Uuid = serde_json::from_value(started["job_id"].clone()).unwrap();
    transcribe_all(&server)
        .await
        .assert_status(StatusCode::CONFLICT);
    assert_eq!(backlog(&server).await["refresh_job"]["id"], json!(job_id));

    let cancel = CancellationToken::new();
    let run = tokio::spawn({
        let handler = refresher(&server);
        let cancel = cancel.clone();
        async move { handler.execute(job_id, json!({}), cancel).await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while queued_transcriptions(&server).await.len() < 3 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("first batch queued");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let batch = queued_transcriptions(&server).await;
    assert_eq!(batch.len(), 3, "one day of audio per batch: {batch:?}");
    assert!(batch.iter().all(|p| p["outdated"] == true), "{batch:?}");

    cancel.cancel();
    assert!(
        run.await.unwrap().is_err(),
        "a cancelled run ends as cancelled"
    );
    assert!(
        queued_transcriptions(&server).await.is_empty(),
        "cancelling drops the batch it queued"
    );
}

#[tokio::test]
async fn a_refresh_with_nothing_left_to_do_finishes_at_once() {
    let server = TestServer::start_api_only().await;
    configure_stt(&server, "http://stt-engine:8023", None).await;

    let result = refresher(&server)
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(result["batches"], 0, "{result}");
    assert!(queued_transcriptions(&server).await.is_empty());
}

#[tokio::test]
async fn transcribe_all_is_refused_while_transcription_is_off() {
    let server = TestServer::start_api_only().await;
    transcribe_all(&server)
        .await
        .assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn transcript_refresh_needs_a_login() {
    let server = TestServer::start_api_only().await;
    server
        .api_client()
        .post("/api/v1/admin/books/bulk-transcribe")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    server
        .api_client()
        .get("/api/v1/admin/books/transcript-backlog")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

const TREEHOUSE_OPENING: &[Seg] = &[
    (33.63, 34.43, "Chapter"),
    (34.43, 34.59, "one"),
    (35.15, 35.31, "The"),
    (35.71, 36.51, "104"),
    (36.75, 37.31, "Story"),
    (37.31, 37.63, "Tree"),
    (37.87, 38.51, "House"),
    (39.79, 40.11, "Hi,"),
    (40.67, 40.91, "my"),
    (40.91, 41.23, "name"),
    (41.23, 41.55, "is"),
    (41.55, 42.11, "Andy"),
    (42.27, 42.75, "Oho."),
    (44.67, 44.99, "This"),
    (44.99, 45.31, "is"),
    (45.31, 45.55, "my"),
    (45.55, 45.79, "friend"),
    (46.03, 46.59, "Terry."),
    (48.75, 48.99, "How"),
    (49.15, 49.31, "you"),
    (49.55, 49.87, "going?"),
];

#[tokio::test]
async fn transcript_lines_break_at_sentence_ends_and_pauses_between_words() {
    let stt = stub_stt_responding(word_body(TREEHOUSE_OPENING)).await;
    let server = TestServer::start_api_only().await;
    configure_stt(&server, &stt.uri(), None).await;
    let (book_id, files) = seed_audiobook(
        &server,
        "The 104-Storey Treehouse",
        &[file("treehouse.m4b", 10.0, Audio::Tone(10.0))],
    )
    .await;

    transcribe(&server, book_id, false).await;

    let lines: Vec<(f64, String)> = stored(&server, files[0])
        .await
        .into_iter()
        .map(|(_, start, text)| (start, text))
        .collect();
    assert_eq!(
        lines,
        vec![
            (33.63, "Chapter one The 104 Story Tree House".to_string()),
            (39.79, "Hi, my name is Andy Oho.".to_string()),
            (44.67, "This is my friend Terry.".to_string()),
            (48.75, "How you going?".to_string()),
        ]
    );
    let request = &stt.received_requests().await.unwrap()[0];
    let body = String::from_utf8_lossy(&request.body);
    assert!(
        body.contains("name=\"timestamp_granularity\"\r\n\r\nword"),
        "{body}"
    );
}
