use crate::common::*;
use api::images::{ImageEntry, ImageMap};
use api::repositories::{authors, books, series};
use serde_json::json;

fn profile_image(url: &str) -> ImageMap {
    let mut m = ImageMap::new();
    m.insert(
        "profile".to_string(),
        ImageEntry {
            source_url: Some(url.to_string()),
            cached_at: None,
        },
    );
    m
}

// =============================================================================
// Author Duplicate Name Tests
// =============================================================================

#[tokio::test]
async fn test_two_authors_same_name_different_hardcover_ids() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let id1 = authors::get_or_create(pool, "Brandon Sanderson", Some("hc_111"))
        .await
        .unwrap();
    let id2 = authors::get_or_create(pool, "Brandon Sanderson", Some("hc_222"))
        .await
        .unwrap();

    assert_ne!(
        id1, id2,
        "Different hardcover IDs should create separate authors"
    );

    // Calling again with existing hardcover_id should return the same author
    let id1_again = authors::get_or_create(pool, "Brandon Sanderson", Some("hc_111"))
        .await
        .unwrap();
    assert_eq!(id1, id1_again);
}

#[tokio::test]
async fn test_get_or_create_hardcover_id_matches_by_id() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    // Create author with a specific hardcover_id
    let id = authors::get_or_create(pool, "B. Sanderson", Some("hc_abc"))
        .await
        .unwrap();

    // Look up with same hardcover_id but slightly different name - should still match
    let id_again = authors::get_or_create(pool, "Brandon Sanderson", Some("hc_abc"))
        .await
        .unwrap();
    assert_eq!(
        id, id_again,
        "Hardcover ID match should be authoritative regardless of name"
    );
}

#[tokio::test]
async fn test_get_or_create_no_hardcover_one_existing_returns_it() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let id1 = authors::get_or_create(pool, "Test Author", None)
        .await
        .unwrap();
    let id2 = authors::get_or_create(pool, "Test Author", None)
        .await
        .unwrap();

    assert_eq!(
        id1, id2,
        "Single name match without hardcover_id should return existing"
    );
}

#[tokio::test]
async fn test_get_or_create_no_hardcover_multiple_returns_oldest() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    // Create two authors with the same name using different hardcover IDs
    let oldest = authors::get_or_create(pool, "John Smith", Some("hc_001"))
        .await
        .unwrap();
    let _newer = authors::get_or_create(pool, "John Smith", Some("hc_002"))
        .await
        .unwrap();

    // Now look up without hardcover_id - should get oldest
    let found = authors::get_or_create(pool, "John Smith", None)
        .await
        .unwrap();
    assert_eq!(
        found, oldest,
        "Multiple name matches without hardcover_id should return oldest"
    );
}

#[tokio::test]
async fn test_get_or_create_with_hardcover_prefers_unlinked() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    // Create two authors with different hardcover IDs - they'll be separate records
    let linked = authors::get_or_create(pool, "Jane Doe", Some("hc_existing"))
        .await
        .unwrap();

    // Now manually insert an unlinked author with the same name (bypassing get_or_create
    // which would match the existing one)
    let unlinked_id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO authors (id, name, normalized_name, identifiers) VALUES ($1, $2, $3, $4)",
    )
    .bind(unlinked_id)
    .bind("Jane Doe")
    .bind("jane doe")
    .bind(serde_json::json!({}))
    .execute(pool)
    .await
    .unwrap();

    // Now try to get_or_create with a NEW hardcover_id - should prefer the unlinked one
    let found = authors::get_or_create(pool, "Jane Doe", Some("hc_new"))
        .await
        .unwrap();
    assert_ne!(
        found, linked,
        "Should not match the linked author with different hardcover_id"
    );
    assert_eq!(
        found, unlinked_id,
        "With new hardcover_id, should prefer unlinked author for backfill"
    );
}

#[tokio::test]
async fn test_upsert_hardcover_id_updates_correct_record() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    // Create two authors with same name, different hardcover IDs
    let id1 = authors::upsert(
        pool,
        authors::AuthorInput::new("Test Author").with_identifiers(json!({"hardcover": "hc_a"})),
    )
    .await
    .unwrap();

    let id2 = authors::upsert(
        pool,
        authors::AuthorInput::new("Test Author").with_identifiers(json!({"hardcover": "hc_b"})),
    )
    .await
    .unwrap();

    assert_ne!(id1, id2);

    // Upserting with hc_a should update the first, not the second
    let updated = authors::upsert(
        pool,
        authors::AuthorInput::new("Test Author Updated")
            .with_identifiers(json!({"hardcover": "hc_a"}))
            .with_images(profile_image("https://example.com/img.jpg")),
    )
    .await
    .unwrap();

    assert_eq!(
        updated, id1,
        "Upsert with hc_a should update the first author"
    );

    // Verify the name was updated on id1
    let name = authors::get_name(pool, id1).await.unwrap();
    assert_eq!(name.unwrap(), "Test Author Updated");

    // id2 name should be unchanged
    let name2 = authors::get_name(pool, id2).await.unwrap();
    assert_eq!(name2.unwrap(), "Test Author");
}

#[tokio::test]
async fn test_upsert_no_hardcover_single_name_match() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    // Create an author without hardcover_id
    let id = authors::get_or_create(pool, "Solo Author", None)
        .await
        .unwrap();

    // Upsert with same name, no hardcover_id - should update existing
    let upserted = authors::upsert(
        pool,
        authors::AuthorInput::new("Solo Author")
            .with_images(profile_image("https://example.com/solo.jpg")),
    )
    .await
    .unwrap();

    assert_eq!(
        upserted, id,
        "Upsert with single name match should update existing"
    );
}

// =============================================================================
// Series Duplicate Name Tests
// =============================================================================

#[tokio::test]
async fn test_series_same_name_different_authors_separate_records() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    // Create two authors
    let author_a = authors::get_or_create(pool, "Isaac Asimov", Some("hc_asimov"))
        .await
        .unwrap();
    let author_b = authors::get_or_create(pool, "David Brin", Some("hc_brin"))
        .await
        .unwrap();

    // Create "Foundation" series linked to author_a (via a book)
    let series_a_id = series::get_or_create_with_author_overlap(
        pool,
        "Foundation",
        &[author_a],
        Some(json!({"hardcover": "hc_foundation_asimov"})),
    )
    .await
    .unwrap()
    .unwrap();

    // Link a book to this series with author_a so overlap can be detected
    let book_a = books::create_ingest_book(
        pool,
        books::IngestBookInput::new("Foundation 1", "complete"),
    )
    .await
    .unwrap();
    authors::link_book(pool, book_a, author_a).await.unwrap();
    series::set_book_series(pool, book_a, Some(series_a_id), Some(1.0))
        .await
        .unwrap();

    // Now create "Foundation" series linked to author_b with different hardcover_id
    let series_b_id = series::get_or_create_with_author_overlap(
        pool,
        "Foundation",
        &[author_b],
        Some(json!({"hardcover": "hc_foundation_brin"})),
    )
    .await
    .unwrap()
    .unwrap();

    assert_ne!(
        series_a_id, series_b_id,
        "Same series name with different hardcover IDs should create separate records"
    );
}

#[tokio::test]
async fn test_series_get_or_create_with_author_overlap_finds_correct() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let author_id = authors::get_or_create(pool, "Patrick Rothfuss", None)
        .await
        .unwrap();

    // Create series and link a book to it with this author
    let series_id = series::get_or_create_by_name(pool, "The Kingkiller Chronicle")
        .await
        .unwrap();

    let book_id = books::create_ingest_book(
        pool,
        books::IngestBookInput::new("The Name of the Wind", "complete"),
    )
    .await
    .unwrap();
    authors::link_book(pool, book_id, author_id).await.unwrap();
    series::set_book_series(pool, book_id, Some(series_id), Some(1.0))
        .await
        .unwrap();

    // Now use get_or_create_with_author_overlap - should find the existing series
    let found = series::get_or_create_with_author_overlap(
        pool,
        "The Kingkiller Chronicle",
        &[author_id],
        None,
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(
        found, series_id,
        "Author overlap should match existing series"
    );
}

#[tokio::test]
async fn test_series_get_or_create_with_author_overlap_no_overlap_creates_new() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let author_a = authors::get_or_create(pool, "Author A", None)
        .await
        .unwrap();
    let author_b = authors::get_or_create(pool, "Author B", None)
        .await
        .unwrap();

    // Create series with author_a linked via book
    let series_id = series::get_or_create_by_name(pool, "Shared Name")
        .await
        .unwrap();
    let book_id =
        books::create_ingest_book(pool, books::IngestBookInput::new("Book A", "complete"))
            .await
            .unwrap();
    authors::link_book(pool, book_id, author_a).await.unwrap();
    series::set_book_series(pool, book_id, Some(series_id), Some(1.0))
        .await
        .unwrap();

    // Try to find "Shared Name" with author_b - no overlap, should create new
    let found = series::get_or_create_with_author_overlap(pool, "Shared Name", &[author_b], None)
        .await
        .unwrap()
        .unwrap();

    assert_ne!(
        found, series_id,
        "No author overlap should create a new series with the same name"
    );
}

// =============================================================================
// Author Rename Tests
// =============================================================================

#[tokio::test]
async fn test_author_rename_to_existing_name_succeeds() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let id1 = authors::get_or_create(pool, "Author One", None)
        .await
        .unwrap();
    let _id2 = authors::get_or_create(pool, "Author Two", None)
        .await
        .unwrap();

    // Rename Author One to the same name as Author Two - should succeed
    // (no unique constraint on normalized_name anymore)
    let result = authors::rename(pool, id1, "Author Two").await;
    assert!(
        result.is_ok(),
        "Rename to existing name should succeed without unique violation"
    );

    let name = authors::get_name(pool, id1).await.unwrap();
    assert_eq!(name.unwrap(), "Author Two");
}

// =============================================================================
// Series Upsert Tests
// =============================================================================

#[tokio::test]
async fn test_series_upsert_hardcover_id_updates_correct() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let id1 = series::upsert(
        pool,
        series::SeriesInput::new("Mistborn").with_identifiers(json!({"hardcover": "hc_mistborn"})),
    )
    .await
    .unwrap();

    let id2 = series::upsert(
        pool,
        series::SeriesInput::new("Stormlight").with_identifiers(json!({"hardcover": "hc_storm"})),
    )
    .await
    .unwrap();

    assert_ne!(id1, id2);

    // Upsert with hc_mistborn should update id1
    let updated = series::upsert(
        pool,
        series::SeriesInput::new("Mistborn Era 1")
            .with_identifiers(json!({"hardcover": "hc_mistborn"})),
    )
    .await
    .unwrap();

    assert_eq!(
        updated, id1,
        "Upsert with same hardcover_id should update existing series"
    );

    let name = series::get_name(pool, id1).await.unwrap();
    assert_eq!(name.unwrap(), "Mistborn Era 1");
}

#[tokio::test]
async fn test_series_hardcover_id_authoritative_in_overlap() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    // Create a series with hardcover_id
    let series_id = series::upsert(
        pool,
        series::SeriesInput::new("Foundation").with_identifiers(json!({"hardcover": "hc_found"})),
    )
    .await
    .unwrap();

    // Look up with same hardcover_id via get_or_create_with_author_overlap
    // Should find it by hardcover_id even without author overlap
    let found = series::get_or_create_with_author_overlap(
        pool,
        "Foundation",
        &[], // No authors
        Some(json!({"hardcover": "hc_found"})),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(
        found, series_id,
        "Hardcover ID should be authoritative in overlap lookup"
    );
}
