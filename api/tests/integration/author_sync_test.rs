use crate::common::*;

use api::repositories::{authors, books};

#[tokio::test]
async fn test_match_discovered_to_library_happy_path() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let author_id = authors::get_or_create(pool, "J.R.R. Tolkien", None)
        .await
        .unwrap();

    // Library book
    let lib_book_id = books::upsert_library_book(pool, books::BookInput::new("The Hobbit"))
        .await
        .unwrap();
    create_stub_book_file(pool, lib_book_id).await;
    books::link_author(pool, lib_book_id, author_id)
        .await
        .unwrap();

    // Discovered book with same title + author
    let disc_book_id = books::create_discovered_book(pool, {
        let mut input = books::DiscoveredBookInput::new("The Hobbit");
        input.discovered_from_author_id = Some(author_id);
        input.want_status = Some("pending".to_string());
        input
    })
    .await
    .unwrap();
    books::link_author(pool, disc_book_id, author_id)
        .await
        .unwrap();

    let matched = books::match_discovered_to_library(pool, author_id)
        .await
        .unwrap();
    assert_eq!(matched, 1);

    // Discovered book should be deleted
    let disc_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM books WHERE id = $1)")
        .bind(disc_book_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert!(!disc_exists, "discovered book should be deleted");

    // Library book should still exist
    let lib_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM books WHERE id = $1)")
        .bind(lib_book_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert!(lib_exists, "library book should still exist");
}

#[tokio::test]
async fn test_match_discovered_no_match_different_title() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let author_id = authors::get_or_create(pool, "J.R.R. Tolkien", None)
        .await
        .unwrap();

    let lib_book_id = books::upsert_library_book(pool, books::BookInput::new("The Hobbit"))
        .await
        .unwrap();
    create_stub_book_file(pool, lib_book_id).await;
    books::link_author(pool, lib_book_id, author_id)
        .await
        .unwrap();

    let disc_book_id = books::create_discovered_book(pool, {
        let mut input = books::DiscoveredBookInput::new("The Silmarillion");
        input.discovered_from_author_id = Some(author_id);
        input.want_status = Some("pending".to_string());
        input
    })
    .await
    .unwrap();
    books::link_author(pool, disc_book_id, author_id)
        .await
        .unwrap();

    let matched = books::match_discovered_to_library(pool, author_id)
        .await
        .unwrap();
    assert_eq!(matched, 0);

    // Discovered book should still exist and have no files (not in library)
    let has_file: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM book_files WHERE book_id = $1)")
            .bind(disc_book_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(
        !has_file,
        "discovered book should have no files (not in library)"
    );
}

#[tokio::test]
async fn test_match_discovered_case_insensitive() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let author_id = authors::get_or_create(pool, "J.R.R. Tolkien", None)
        .await
        .unwrap();

    let lib_book_id = books::upsert_library_book(pool, books::BookInput::new("The Hobbit"))
        .await
        .unwrap();
    create_stub_book_file(pool, lib_book_id).await;
    books::link_author(pool, lib_book_id, author_id)
        .await
        .unwrap();

    let disc_book_id = books::create_discovered_book(pool, {
        let mut input = books::DiscoveredBookInput::new("the hobbit");
        input.discovered_from_author_id = Some(author_id);
        input.want_status = Some("pending".to_string());
        input
    })
    .await
    .unwrap();
    books::link_author(pool, disc_book_id, author_id)
        .await
        .unwrap();

    let matched = books::match_discovered_to_library(pool, author_id)
        .await
        .unwrap();
    assert_eq!(matched, 1);
}
