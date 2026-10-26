use crate::common::audiobooks::*;
use crate::common::*;
use api::jobs::handlers::{AudiobookChapterRepairHandler, AudiobookProbeHandler};
use api::jobs::worker::JobHandler;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const OWN: [Span; 3] = [
    ("Opening Credits", 0.0, 4.0),
    ("Chapter 1", 4.0, 8.0),
    ("Chapter 2", 8.0, 12.0),
];

fn repairer(server: &TestServer) -> AudiobookChapterRepairHandler {
    AudiobookChapterRepairHandler::new(
        server.pool.clone(),
        server.storage_root(),
        server.nats_publisher.clone(),
        server.search_backend_arc(),
    )
}

async fn repair_via_api(server: &TestServer) {
    let resp = server
        .admin_client()
        .post("/api/v1/admin/books/repair-audiobook-chapters")
        .await;
    resp.assert_status(StatusCode::ACCEPTED);
    let handler = repairer(server);
    server.process_pending_jobs(&handler).await;
    server.process_pending_jobs(&handler).await;
}

async fn dry_run(server: &TestServer) -> Vec<Uuid> {
    let result = repairer(server)
        .execute(
            Uuid::now_v7(),
            json!({"dry_run": true}),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    result["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["book_id"].as_str().unwrap().parse().unwrap())
        .collect()
}

async fn assert_settled(server: &TestServer, book_id: Uuid) {
    let pending = dry_run(server).await;
    assert!(
        !pending.contains(&book_id),
        "a second sweep still wants to change {book_id}"
    );
}

#[tokio::test]
async fn repair_brings_back_embedded_chapters_a_transcript_run_overwrote() {
    let server = TestServer::start_api_only().await;
    let (book_id, files) = seed_audiobook(
        &server,
        "Max and the Midknights",
        &[file("max.m4b", 12.0, Audio::Chaptered(12.0, &OWN))],
    )
    .await;
    store(&server, files[0], "embedded", &[OWN[2]]).await;

    assert!(dry_run(&server).await.contains(&book_id));
    assert_eq!(titles(&unified(&server, book_id).await), vec!["Chapter 2"]);

    repair_via_api(&server).await;

    let got = unified(&server, book_id).await;
    assert_eq!(
        titles(&got),
        OWN.iter().map(|(t, _, _)| *t).collect::<Vec<_>>()
    );
    assert!(got.iter().all(|c| c.source == "embedded"), "{got:?}");
    assert_settled(&server, book_id).await;
}

#[tokio::test]
async fn repair_turns_chapter_sized_files_back_into_one_chapter_each() {
    let server = TestServer::start_api_only().await;
    let (book_id, files) = seed_audiobook(
        &server,
        "A Storm of Swords",
        &[
            file("00 - Prologue.mp3", 2654.0, Audio::Tone(2.0)),
            file("01 - Jaime I.mp3", 2242.0, Audio::Tone(2.0)),
            file("02 - Catelyn I.mp3", 1514.0, Audio::Tone(2.0)),
        ],
    )
    .await;
    store(
        &server,
        files[0],
        "detected",
        &[("Chapter 1", 0.0, 2.5), ("Chapter 2", 2.5, 2654.0)],
    )
    .await;
    store(
        &server,
        files[1],
        "transcript",
        &[("part I", 564.0, 2242.0)],
    )
    .await;

    repair_via_api(&server).await;

    let got = unified(&server, book_id).await;
    assert_eq!(
        titles(&got),
        vec!["00 - Prologue", "01 - Jaime I", "02 - Catelyn I"]
    );
    assert!(got.iter().all(|c| c.source == "filename"), "{got:?}");
    assert_settled(&server, book_id).await;
}

#[tokio::test]
async fn repair_reruns_transcript_detection_with_whole_chapter_numbers() {
    let server = TestServer::start_api_only().await;
    let (book_id, files) = seed_audiobook(
        &server,
        "Harry Potter and the Deathly Hallows",
        &[file("hp7.mp3", 3000.0, Audio::Tone(2.0))],
    )
    .await;
    store(
        &server,
        files[0],
        "transcript",
        &[
            ("CHAPTER twenty", 0.0, 600.0),
            ("Chapter twenty", 600.0, 3000.0),
        ],
    )
    .await;
    add_transcript(
        &server,
        files[0],
        &[
            (0.0, "CHAPTER twenty two The Deathly Hallows"),
            (600.0, "Chapter twenty three Malfoy Manor"),
        ],
    )
    .await;

    repair_via_api(&server).await;

    assert_eq!(
        titles(&unified(&server, book_id).await),
        vec!["Chapter 22", "Chapter 23"]
    );
    assert_settled(&server, book_id).await;
}

#[tokio::test]
async fn a_cue_sheets_last_track_runs_to_the_end_of_the_file() {
    let server = TestServer::start_api_only().await;
    let (book_id, files) = seed_audiobook(
        &server,
        "Cue Book",
        &[file("cue.mp3", 6.0, Audio::Tone(6.0))],
    )
    .await;
    let path: String = sqlx::query_scalar("SELECT file_path FROM audiobook_files WHERE id = $1")
        .bind(files[0])
        .fetch_one(&server.pool)
        .await
        .unwrap();
    std::fs::write(
        std::path::Path::new(&path).with_extension("cue"),
        "FILE \"cue.mp3\" MP3\n  TRACK 01 AUDIO\n    TITLE \"Start\"\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    TITLE \"Finish\"\n    INDEX 01 00:03:00\n",
    )
    .unwrap();

    repair_via_api(&server).await;

    let got = unified(&server, book_id).await;
    assert_eq!(titles(&got), vec!["Start", "Finish"]);
    assert!((got[1].end - 6.0).abs() < 0.2, "{got:?}");
}

#[tokio::test]
async fn a_dry_run_changes_nothing() {
    let server = TestServer::start_api_only().await;
    let (book_id, files) = seed_audiobook(
        &server,
        "Max and the Midknights",
        &[file("max.m4b", 12.0, Audio::Chaptered(12.0, &OWN))],
    )
    .await;
    store(&server, files[0], "embedded", &[OWN[2]]).await;

    let resp = server
        .admin_client()
        .post("/api/v1/admin/books/repair-audiobook-chapters?dry_run=true")
        .await;
    resp.assert_status(StatusCode::ACCEPTED);
    let body: Value = resp.json();
    assert_eq!(body["dry_run"], true);
    let handler = repairer(&server);
    server.process_pending_jobs(&handler).await;
    server.process_pending_jobs(&handler).await;

    assert_eq!(titles(&unified(&server, book_id).await), vec!["Chapter 2"]);
}

#[tokio::test]
async fn repair_needs_a_login() {
    let server = TestServer::start_api_only().await;
    server
        .api_client()
        .post("/api/v1/admin/books/repair-audiobook-chapters")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn probing_chapter_sized_files_names_a_chapter_after_each_file() {
    let server = TestServer::start_api_only().await;
    let (book_id, _) = seed_audiobook(
        &server,
        "The Witches",
        &[
            file("01 - A Note About Witches.mp3", 0.0, Audio::TwoHalves),
            file("02 - My Grandmother.mp3", 0.0, Audio::TwoHalves),
        ],
    )
    .await;

    AudiobookProbeHandler::new(
        server.pool.clone(),
        server.storage_root(),
        Arc::new(Semaphore::new(1)),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
    )
    .execute(
        Uuid::now_v7(),
        json!({"book_id": book_id}),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    let got = unified(&server, book_id).await;
    assert_eq!(
        titles(&got),
        vec!["01 - A Note About Witches", "02 - My Grandmother"]
    );
    assert!(got.iter().all(|c| c.source == "filename"), "{got:?}");
}
