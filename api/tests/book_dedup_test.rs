use crate::common::*;
use api::repositories::{audiobook_files, book_files, books};
use serde_json::json;

/// When an ebook already exists with a hardcover_id and we upsert an audiobook
/// with the same identifier, it should merge into the existing book rather than
/// creating a duplicate.
#[tokio::test]
async fn test_resolve_audiobook_merges_with_existing_ebook_by_identifier() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    // 1. Create an ebook with hardcover_id = "hc_abc"
    let ebook_id = books::upsert_library_book(
        pool,
        books::BookInput {
            title: "The Great Novel".to_string(),
            identifiers: Some(json!({"hardcover_id": "hc_abc"})),
            want_format: Some("ebook".to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Add an ebook file
    book_files::create(
        pool,
        book_files::BookFileInput {
            book_id: ebook_id,
            format: "EPUB".to_string(),
            size_bytes: 1024,
            file_path: "/books/the-great-novel.epub".to_string(),
            source_type: Some("media_root".to_string()),
            source_metadata: Some(json!({})),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // 2. Simulate what resolve_book_group does for an audiobook with the same identifier:
    //    look up existing book by identifier, then upsert with that ID.
    let existing = books::find_by_identifier(pool, "hardcover_id", "hc_abc")
        .await
        .unwrap();
    assert!(existing.is_some(), "should find the existing ebook");
    let existing_id = existing.unwrap().id;
    assert_eq!(existing_id, ebook_id);

    // Upsert with the existing ID (as resolve_book_group would)
    let merged_id = books::upsert_library_book(
        pool,
        books::BookInput {
            id: Some(existing_id),
            title: "The Great Novel".to_string(),
            identifiers: Some(json!({"hardcover_id": "hc_abc"})),
            want_format: Some("audiobook".to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(merged_id, ebook_id, "should reuse the same book ID");

    // Add an audiobook file to the same book
    audiobook_files::create(
        pool,
        audiobook_files::AudiobookFileInput {
            book_id: merged_id,
            format: "M4B".to_string(),
            file_path: "/audiobooks/the-great-novel.m4b".to_string(),
            file_size: 500_000_000,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // 3. Assert only one book row exists with this identifier
    let count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM books WHERE identifiers->>'hardcover_id' = 'hc_abc'")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(count.0, 1, "should have exactly one book, not a duplicate");

    // 4. Assert the book has both ebook and audiobook files
    let ebook_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM book_files WHERE book_id = $1")
        .bind(merged_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(ebook_count.0, 1, "should have one ebook file");

    let audio_count = audiobook_files::count_by_book(pool, merged_id)
        .await
        .unwrap();
    assert_eq!(audio_count, 1, "should have one audiobook file");

    // 5. Assert want_format was upgraded to 'both' since the book now has ebook + audiobook
    let want_format: (String,) = sqlx::query_as("SELECT want_format FROM books WHERE id = $1")
        .bind(merged_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        want_format.0, "both",
        "want_format should be 'both' when ebook and audiobook are merged"
    );
}

/// Without identifier match, two upserts create separate books.
#[tokio::test]
async fn test_no_identifier_creates_separate_books() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let id1 = books::upsert_library_book(
        pool,
        books::BookInput {
            title: "Same Title".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let id2 = books::upsert_library_book(
        pool,
        books::BookInput {
            title: "Same Title".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_ne!(
        id1, id2,
        "without matching IDs, should create separate books"
    );
}
