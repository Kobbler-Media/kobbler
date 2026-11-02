use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

fn chapters() -> Vec<String> {
    vec![
        chapter_xhtml(
            "Chapter One",
            &[
                "It was a bright cold day in April. The clocks were striking thirteen.",
                "Second paragraph of the first chapter.",
            ],
        ),
        chapter_xhtml(
            "Chapter Two",
            &[
                "Winston Smith, his chin nuzzled into his breast. He slipped quickly through the doors.",
            ],
        ),
    ]
}

fn real_epub() -> Vec<u8> {
    epub_with(&chapters())
}

async fn book_with_epub(server: &TestServer) -> uuid::Uuid {
    let book_id = create_book(server, "Pack Test").await;
    upload_epub(server, book_id, real_epub()).await;
    book_id
}

fn block_text(block: &Value) -> String {
    block["spans"]
        .as_array()
        .map(|spans| spans.iter().filter_map(|s| s["text"].as_str()).collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn reader_pack_requires_auth_and_a_readable_file() {
    let server = TestServer::start_api_only().await;
    let book_id = book_with_epub(&server).await;
    let empty = create_book(&server, "No File").await;

    for path in [
        format!("/api/v1/admin/books/{book_id}/reader-pack"),
        format!("/api/v1/admin/books/{book_id}/reader-pack/chapters/0"),
    ] {
        server
            .api_client()
            .get(&path)
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }
    server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{empty}/reader-pack"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/books/{book_id}/reader-pack/chapters/9"
        ))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unreadable_book_is_unprocessable() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Broken").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;
    server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{book_id}/reader-pack"))
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn reader_pack_serves_manifest_and_chapters() {
    let server = TestServer::start_api_only().await;
    let book_id = book_with_epub(&server).await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{book_id}/reader-pack"))
        .await;
    resp.assert_ok();
    let manifest: Value = resp.json();
    assert_eq!(manifest["title"], "Pack Test");
    let chapters = manifest["chapters"].as_array().unwrap();
    assert_eq!(chapters.len(), 2);
    assert_eq!(chapters[1]["href"], "OEBPS/ch2.xhtml");
    assert_eq!(
        chapters[0]["weight"],
        self::chapters()[0].encode_utf16().count()
    );

    let resp = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/books/{book_id}/reader-pack/chapters/1"
        ))
        .await;
    resp.assert_ok();
    let chapter: Value = resp.json();
    let texts: Vec<String> = chapter["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(block_text)
        .collect();
    assert_eq!(texts[0], "Chapter Two");
    assert!(texts[1].starts_with("Winston Smith"));
}

#[tokio::test]
async fn app_position_round_trips_through_epubcfi() {
    let server = TestServer::start_api_only().await;
    let book_id = book_with_epub(&server).await;
    let path = format!("/api/v1/admin/books/{book_id}/reader-progress");

    server
        .admin_client()
        .post_json(
            &path,
            &json!({"progress_percent": 70.0, "position": {"chapter": 1, "block": 1, "offset": 31}}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let web: Value = server.admin_client().get(&path).await.json();
    let cfi = web["cfi"].as_str().unwrap();
    assert!(cfi.starts_with("epubcfi(/6/4!/4/"), "{cfi}");
    assert!(web.get("position").is_none());

    let app: Value = server
        .admin_client()
        .get(&format!("{path}?position=true"))
        .await
        .json();
    assert_eq!(
        app["position"],
        json!({"chapter": 1, "block": 1, "offset": 31})
    );
    assert_eq!(app["progress_percent"], 70.0);
}

#[tokio::test]
async fn web_epubcfi_resolves_to_an_app_position() {
    let server = TestServer::start_api_only().await;
    let book_id = book_with_epub(&server).await;
    let path = format!("/api/v1/admin/books/{book_id}/reader-progress");

    // What foliate writes for "The clocks" in chapter one: a range over the
    // visible page, which resolves to its start.
    server
        .admin_client()
        .post_json(
            &path,
            &json!({"progress_percent": 10.0, "cfi": "epubcfi(/6/2!/4/4,/1:35,/1:60)"}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let app: Value = server
        .admin_client()
        .get(&format!("{path}?position=true"))
        .await
        .json();
    assert_eq!(
        app["position"],
        json!({"chapter": 0, "block": 1, "offset": 35})
    );
    assert_eq!(app["cfi"], "epubcfi(/6/2!/4/4,/1:35,/1:60)");
}

#[tokio::test]
async fn kobo_and_app_share_one_position() {
    let server = TestServer::start().await;
    let user_id = get_admin_user_id(&server).await;
    let (_, device_token) = create_device(&server, user_id).await;
    let book_id = book_with_epub(&server).await;
    entitle_book_to_user(&server, book_id, user_id).await;
    let progress = format!("/api/v1/admin/books/{book_id}/reader-progress");

    // App → Kobo: the app's position reaches the Kobo as a KoboSpan.
    server
        .admin_client()
        .post_json(
            &progress,
            &json!({"progress_percent": 40.0, "position": {"chapter": 0, "block": 1, "offset": 35}}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let states: Vec<Value> = server
        .kobo_client()
        .kobo_get_state(&device_token, &book_id.to_string())
        .await
        .json();
    let location = &states[0]["CurrentBookmark"]["Location"];
    assert_eq!(location["Type"], "KoboSpan");
    assert_eq!(location["Source"], "ch1.xhtml");
    assert_eq!(location["Value"], "kobo.2.2");

    // Kobo → app: a later Kobo position comes back as a pack position.
    server
        .kobo_client()
        .kobo_update_state(
            &device_token,
            &book_id.to_string(),
            &json!({"ReadingStates": [{
                "EntitlementId": book_id.to_string(),
                "CurrentBookmark": {
                    "ProgressPercent": 80.0,
                    "Location": {"Value": "kobo.2.2", "Type": "KoboSpan", "Source": "ch2.xhtml"}
                }
            }]}),
        )
        .await
        .assert_ok();
    let app: Value = server
        .admin_client()
        .get(&format!("{progress}?position=true"))
        .await
        .json();
    assert_eq!(app["progress_percent"], 80.0);
    let position = &app["position"];
    assert_eq!(position["chapter"], 1);
    assert_eq!(position["block"], 1);
    let chapter: Value = server
        .admin_client()
        .get(&format!(
            "/api/v1/admin/books/{book_id}/reader-pack/chapters/1"
        ))
        .await
        .json();
    let text = block_text(&chapter["blocks"][1]);
    let offset = position["offset"].as_u64().unwrap() as usize;
    let rest: String =
        String::from_utf16_lossy(&text.encode_utf16().skip(offset).collect::<Vec<_>>());
    assert!(rest.starts_with("He slipped"), "{rest}");
}

#[tokio::test]
async fn the_sweep_builds_missing_packs_and_clears_those_of_deleted_files() {
    use api::jobs::handlers::ReaderPackHandler;
    use api::jobs::worker::JobHandler;
    use std::sync::Arc;

    let server = TestServer::start_api_only().await;
    let book_id = book_with_epub(&server).await;
    let packs = std::path::Path::new(&server.storage_root()).join("reader_packs");
    let orphan = packs.join(uuid::Uuid::new_v4().to_string()).join("rev-g1");
    std::fs::create_dir_all(&orphan).unwrap();

    let sweeper = ReaderPackHandler::new(
        server.pool.clone(),
        server.storage_root(),
        server.nats_publisher.clone(),
        Arc::new(tokio::sync::Semaphore::new(1)),
    );
    let result = sweeper
        .execute(
            uuid::Uuid::now_v7(),
            json!({}),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result["queued"], 1, "{result}");
    assert!(
        !orphan.parent().unwrap().exists(),
        "pack of a deleted book file left on disk"
    );

    sweeper
        .execute(
            uuid::Uuid::now_v7(),
            json!({"book_id": book_id}),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    let again = sweeper
        .execute(
            uuid::Uuid::now_v7(),
            json!({}),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(again["queued"], 0, "a built pack was queued again: {again}");
}

/// More books than the server keeps loaded at once: opening the one after the
/// cache fills must evict another, not hang.
#[tokio::test]
async fn opening_more_books_than_are_kept_loaded_does_not_hang() {
    let server = TestServer::start_api_only().await;
    let mut books = Vec::new();
    for _ in 0..40 {
        books.push(book_with_epub(&server).await);
    }
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        for book_id in &books {
            server
                .admin_client()
                .get(&format!("/api/v1/admin/books/{book_id}/reader-pack"))
                .await
                .assert_ok();
        }
    })
    .await
    .expect("opening books stalled once the loaded-pack cache was full");
}
