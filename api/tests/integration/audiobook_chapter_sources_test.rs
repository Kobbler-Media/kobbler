use crate::common::audiobooks::*;
use crate::common::*;
use api::jobs::handlers::ChapterDetectHandler;
use api::jobs::worker::JobHandler;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const EMBEDDED: [Span; 6] = [
    ("Max And The Midknights", 0.0, 25.0),
    ("Chapter 1", 25.0, 620.0),
    ("Chapter 2", 620.0, 1110.0),
    ("Chapter 3", 1110.0, 1685.0),
    ("Chapter 4", 1685.0, 2348.0),
    ("Chapter 5", 2348.0, 3000.0),
];

async fn single_file(server: &TestServer) -> (Uuid, Uuid) {
    let (book_id, files) = seed_audiobook(
        server,
        "Max and the Midknights",
        &[file("max.m4b", 3000.0, Audio::None)],
    )
    .await;
    (book_id, files[0])
}

async fn detect(server: &TestServer, book_id: Uuid) -> Value {
    ChapterDetectHandler::new(server.pool.clone(), server.search_backend_arc())
        .execute(
            Uuid::now_v7(),
            json!({"book_id": book_id}),
            CancellationToken::new(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn transcript_detection_leaves_embedded_chapters_whole() {
    let server = TestServer::start_api_only().await;
    let (book_id, file_id) = single_file(&server).await;
    store(&server, file_id, "embedded", &EMBEDDED).await;
    add_transcript(
        &server,
        file_id,
        &[
            (6.5, "Chapter one"),
            (40.0, "Once upon a time there was a squire."),
            (621.0, "Chapter two"),
            (700.0, "The castle was very large."),
            (1112.0, "Chapter three"),
            (1200.0, "And then the dragon came."),
        ],
    )
    .await;

    for _ in 0..2 {
        detect(&server, book_id).await;
    }

    let got = unified(&server, book_id).await;
    assert_eq!(
        titles(&got),
        EMBEDDED.iter().map(|(t, _, _)| *t).collect::<Vec<_>>()
    );
    assert!(got.iter().all(|c| c.source == "embedded"), "{got:?}");
}

#[tokio::test]
async fn a_stronger_source_replaces_a_weaker_one_and_never_the_reverse() {
    let server = TestServer::start_api_only().await;
    let cases = [
        ("embedded", "transcript", "embedded"),
        ("embedded", "detected", "embedded"),
        ("embedded", "filename", "embedded"),
        ("transcript", "detected", "transcript"),
        ("transcript", "filename", "transcript"),
        ("provider", "transcript", "provider"),
        ("detected", "transcript", "transcript"),
        ("transcript", "embedded", "embedded"),
        ("detected", "detected", "detected"),
        ("filename", "provider", "provider"),
        ("filename", "detected", "filename"),
        ("detected", "filename", "filename"),
    ];
    for (existing, incoming, kept) in cases {
        let (book_id, file_id) = single_file(&server).await;
        store(&server, file_id, existing, &EMBEDDED).await;
        store(
            &server,
            file_id,
            incoming,
            &[("A", 6.0, 600.0), ("B", 600.0, 3000.0)],
        )
        .await;

        let got = unified(&server, book_id).await;
        assert!(
            got.iter().all(|c| c.source == kept),
            "{existing} then {incoming} should leave only {kept}: {got:?}"
        );
        let expected = if kept == incoming { 2 } else { EMBEDDED.len() };
        assert_eq!(got.len(), expected, "{existing} then {incoming}: {got:?}");
    }
}

#[tokio::test]
async fn chapters_are_listed_by_where_they_start() {
    let server = TestServer::start_api_only().await;
    let (book_id, file_id) = single_file(&server).await;
    store(
        &server,
        file_id,
        "embedded",
        &[
            ("Chapter five", 2348.0, 3000.0),
            ("Chapter 1", 25.0, 620.0),
            ("Chapter 3", 1110.0, 2348.0),
            ("Chapter two", 620.0, 1110.0),
        ],
    )
    .await;

    let starts: Vec<f64> = unified(&server, book_id)
        .await
        .iter()
        .map(|c| c.start)
        .collect();
    assert_eq!(starts, vec![25.0, 620.0, 1110.0, 2348.0]);
}

#[tokio::test]
async fn a_chapter_with_no_length_is_not_stored() {
    let server = TestServer::start_api_only().await;
    let (book_id, file_id) = single_file(&server).await;
    store(
        &server,
        file_id,
        "embedded",
        &[("Chapter 29", 0.0, 3000.0), ("Chapter 30", 3000.0, 3000.0)],
    )
    .await;

    assert_eq!(titles(&unified(&server, book_id).await), vec!["Chapter 29"]);
}

#[tokio::test]
async fn a_file_without_chapters_still_moves_the_ones_after_it() {
    let server = TestServer::start_api_only().await;
    let (book_id, files) = seed_audiobook(
        &server,
        "Three Discs",
        &[
            file("1.mp3", 600.0, Audio::None),
            file("2.mp3", 600.0, Audio::None),
            file("3.mp3", 600.0, Audio::None),
        ],
    )
    .await;
    store(
        &server,
        files[0],
        "transcript",
        &[("Chapter 1", 0.0, 600.0)],
    )
    .await;
    store(
        &server,
        files[2],
        "transcript",
        &[("Chapter 2", 0.0, 300.0), ("Chapter 3", 300.0, 600.0)],
    )
    .await;

    let spans: Vec<(f64, f64)> = unified(&server, book_id)
        .await
        .iter()
        .map(|c| (c.start, c.end))
        .collect();
    assert_eq!(
        spans,
        vec![(0.0, 1200.0), (1200.0, 1500.0), (1500.0, 1800.0)]
    );
}

#[tokio::test]
async fn transcript_titles_name_the_whole_number_the_narrator_said() {
    let server = TestServer::start_api_only().await;
    let (book_id, file_id) = single_file(&server).await;
    add_transcript(
        &server,
        file_id,
        &[
            (0.0, "CHAPTER twenty four The Long Night"),
            (300.0, "I could start a self-help program."),
            (600.0, "Chapter twenty-five."),
            (900.0, "Part I think we should go, said Ron."),
            (1200.0, "Chapter XXVI The Return"),
            (1500.0, "It was the best book."),
            (1501.0, "I knew that already."),
            (1800.0, "chapter one hundred and two"),
            (2100.0, "PROLOGUE"),
        ],
    )
    .await;

    detect(&server, book_id).await;

    assert_eq!(
        titles(&unified(&server, book_id).await),
        vec![
            "Chapter 24",
            "Chapter 25",
            "Chapter 26",
            "Chapter 102",
            "Prologue"
        ]
    );
}

#[tokio::test]
async fn transcript_detection_that_misses_most_chapters_keeps_the_files_as_chapters() {
    let server = TestServer::start_api_only().await;
    let (book_id, files) = seed_audiobook(
        &server,
        "A Storm of Swords",
        &[
            file("00 - Prologue.mp3", 3000.0, Audio::None),
            file("01 - Jaime I.mp3", 3000.0, Audio::None),
            file("02 - Catelyn I.mp3", 3000.0, Audio::None),
            file("03 - Epilogue.mp3", 3000.0, Audio::None),
        ],
    )
    .await;
    let names = [
        "00 - Prologue",
        "01 - Jaime I",
        "02 - Catelyn I",
        "03 - Epilogue",
    ];
    for (id, name) in files.iter().zip(names) {
        store(&server, *id, "filename", &[(name, 0.0, 3000.0)]).await;
    }
    add_transcript(
        &server,
        files[0],
        &[(0.0, "Prologue"), (60.0, "The night was cold.")],
    )
    .await;
    add_transcript(&server, files[3], &[(0.0, "Epilogue"), (60.0, "The end.")]).await;

    let result = detect(&server, book_id).await;

    assert_eq!(result["detected_chapters"], 0, "{result}");
    assert_eq!(titles(&unified(&server, book_id).await), names.to_vec());
}
