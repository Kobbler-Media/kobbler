//! Tests that verify the metadata content in sync responses is correct.
//!
//! These tests ensure that:
//! - Book titles, authors, series info are correct in sync
//! - Download URLs are present and properly formatted
//! - All required fields are populated

use crate::common::*;
use serde_json::Value;
use test_case::test_case;

// =============================================================================
// Basic Metadata Tests
// =============================================================================

#[tokio::test]
async fn test_sync_metadata_has_title() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "metatitle@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "My Test Title").await;
    entitle_book(&server, book_id).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    assert_sync_book_title(&items, book_id, "My Test Title");
}

#[tokio::test]
async fn test_sync_metadata_has_download_urls() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "metaurls@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Download URLs Book").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;
    entitle_book(&server, book_id).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    assert_sync_book_has_download_urls(&items, book_id);
    assert_sync_book_has_format(&items, book_id, "KEPUB");
}

#[tokio::test]
async fn test_sync_metadata_has_revision_id() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "metarev@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Revision ID Book").await;
    entitle_book(&server, book_id).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    let metadata = get_sync_book_metadata(&items, book_id).expect("Book not found");
    let revision_id = metadata.get("RevisionId").and_then(|v| v.as_str());

    assert!(
        revision_id.is_some(),
        "Metadata should have RevisionId. Got: {}",
        serde_json::to_string_pretty(metadata).unwrap_or_default()
    );
}

#[tokio::test]
async fn test_sync_metadata_has_entitlement_id() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "metaent@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Entitlement ID Book").await;
    entitle_book(&server, book_id).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    let item = find_sync_book(&items, book_id).expect("Book not found");
    let entitlement = item
        .get("NewEntitlement")
        .or_else(|| item.get("ChangedEntitlement"))
        .expect("No entitlement");

    let book_ent = entitlement
        .get("BookEntitlement")
        .expect("No BookEntitlement");
    let ent_id = book_ent.get("Id").and_then(|v| v.as_str());

    assert_eq!(
        ent_id,
        Some(book_id.to_string().as_str()),
        "BookEntitlement.Id should match book_id"
    );
}

// =============================================================================
// Series Metadata Tests
// =============================================================================

#[tokio::test]
async fn test_sync_metadata_has_series_info() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "metaseries@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book_in_series(&server, "Series Book", "Epic Fantasy Series", 3).await;
    entitle_book(&server, book_id).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    let metadata = get_sync_book_metadata(&items, book_id).expect("Book not found");

    // Check series is present
    let series = metadata.get("Series");
    assert!(
        series.is_some(),
        "Metadata should have Series. Got: {}",
        serde_json::to_string_pretty(metadata).unwrap_or_default()
    );
}

// =============================================================================
// Table Tests for Various Title Patterns
// =============================================================================

#[test_case("Simple Title" ; "simple ascii")]
#[test_case("Title with Numbers 123" ; "with numbers")]
#[test_case("Title: With Colon" ; "with colon")]
#[test_case("Title - With Dash" ; "with dash")]
#[test_case("Title's Apostrophe" ; "with apostrophe")]
#[test_case("Title \"Quoted\"" ; "with quotes")]
#[test_case("Title (Parentheses)" ; "with parentheses")]
#[test_case("日本語タイトル" ; "japanese")]
#[test_case("Título en Español" ; "spanish accents")]
#[test_case("Émile's Café" ; "french accents")]
#[tokio::test]
async fn test_sync_preserves_title_characters(title: &str) {
    let server = TestServer::start().await;

    let email = format!("title{}@test.com", uuid::Uuid::new_v4());
    let user_id = create_user(&server, &email).await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, title).await;
    entitle_book(&server, book_id).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    assert_sync_book_title(&items, book_id, title);
}

// =============================================================================
// Entitlement Structure Tests
// =============================================================================

#[tokio::test]
async fn test_sync_entitlement_has_required_fields() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "entfields@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Entitlement Fields Book").await;
    entitle_book(&server, book_id).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    let item = find_sync_book(&items, book_id).expect("Book not found");
    let entitlement = item
        .get("NewEntitlement")
        .expect("Should be NewEntitlement for first sync");

    // Check BookEntitlement has required fields
    let book_ent = entitlement
        .get("BookEntitlement")
        .expect("No BookEntitlement");

    assert!(book_ent.get("Id").is_some(), "BookEntitlement must have Id");
    assert!(
        book_ent.get("RevisionId").is_some(),
        "BookEntitlement must have RevisionId"
    );
    assert!(
        book_ent.get("Created").is_some(),
        "BookEntitlement must have Created"
    );
    assert!(
        book_ent.get("LastModified").is_some(),
        "BookEntitlement must have LastModified"
    );

    // Check BookMetadata exists
    let book_meta = entitlement.get("BookMetadata").expect("No BookMetadata");
    assert!(
        book_meta.get("Title").is_some(),
        "BookMetadata must have Title"
    );
    assert!(
        book_meta.get("RevisionId").is_some(),
        "BookMetadata must have RevisionId"
    );
}

#[tokio::test]
async fn test_sync_changed_entitlement_for_updated_book() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "changed@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Original Title").await;
    entitle_book(&server, book_id).await;

    // First sync - NewEntitlement
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");
    let items: Vec<Value> = resp.json();

    let item = find_sync_book(&items, book_id).expect("Book not found");
    assert!(
        item.get("NewEntitlement").is_some(),
        "First sync should be NewEntitlement"
    );

    // Touch book to trigger re-sync
    touch_book(&server, book_id).await;

    // Second sync with token - ChangedEntitlement
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await
        .assert_ok()
        .json();

    let item = find_sync_book(&items, book_id).expect("Book not found after update");
    assert!(
        item.get("ChangedEntitlement").is_some(),
        "Updated book should be ChangedEntitlement"
    );
}

// =============================================================================
// Download URL Format Tests
// =============================================================================

#[tokio::test]
async fn test_sync_download_url_format() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "urlformat@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "URL Format Book").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;
    entitle_book(&server, book_id).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    let metadata = get_sync_book_metadata(&items, book_id).expect("Book not found");
    let urls = metadata
        .get("DownloadUrls")
        .and_then(|u| u.as_array())
        .expect("Should have DownloadUrls array");

    assert!(!urls.is_empty(), "Should have at least one download URL");

    let first_url = &urls[0];
    assert!(
        first_url.get("Url").is_some(),
        "Download URL must have Url field"
    );
    assert!(
        first_url.get("Format").is_some(),
        "Download URL must have Format field"
    );

    // URL should be valid HTTP URL
    let url_str = first_url.get("Url").and_then(|u| u.as_str()).unwrap();
    assert!(
        url_str.starts_with("http://") || url_str.starts_with("https://"),
        "Download URL should be HTTP(S): {}",
        url_str
    );
}

// =============================================================================
// IsRemoved Flag Tests
// =============================================================================

#[tokio::test]
async fn test_sync_is_removed_flag() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "removed@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Will Be Removed").await;
    entitle_book(&server, book_id).await;

    // First sync
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");

    // Remove book
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/remove", book_id),
            &serde_json::json!({}),
        )
        .await
        .assert_success();

    // Second sync with token - should have IsRemoved=true
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await
        .assert_ok()
        .json();

    let item = find_sync_book(&items, book_id).expect("Removed book not in sync");
    let entitlement = item
        .get("NewEntitlement")
        .or_else(|| item.get("ChangedEntitlement"))
        .expect("No entitlement");

    let is_removed = entitlement
        .get("BookEntitlement")
        .and_then(|be| be.get("IsRemoved"))
        .and_then(|r| r.as_bool())
        .unwrap_or(false);

    assert!(is_removed, "Removed book should have IsRemoved=true");
}

#[tokio::test]
async fn test_sync_active_book_is_removed_false() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "active@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Active Book").await;
    entitle_book(&server, book_id).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    let item = find_sync_book(&items, book_id).expect("Book not found");
    let entitlement = item
        .get("NewEntitlement")
        .expect("Should be NewEntitlement");

    let is_removed = entitlement
        .get("BookEntitlement")
        .and_then(|be| be.get("IsRemoved"))
        .and_then(|r| r.as_bool())
        .unwrap_or(true); // Default to true so test fails if field missing

    assert!(
        !is_removed,
        "Active book should have IsRemoved=false (or not present)"
    );
}

// =============================================================================
// Contributors / ContributorRoles Tests
// =============================================================================

#[tokio::test]
async fn test_sync_metadata_has_contributor_roles_with_role() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "contributor_roles@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book_with_author(&server, "Roles Book", "Jane Doe").await;
    entitle_book(&server, book_id).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    let metadata = get_sync_book_metadata(&items, book_id).expect("Book not found");

    // Contributors should contain the author name
    let contributors = metadata
        .get("Contributors")
        .and_then(|v| v.as_array())
        .expect("Should have Contributors");
    assert!(
        contributors.iter().any(|c| c.as_str() == Some("Jane Doe")),
        "Contributors should contain 'Jane Doe', got: {:?}",
        contributors
    );

    // ContributorRoles should have Name and Role
    let roles = metadata
        .get("ContributorRoles")
        .and_then(|v| v.as_array())
        .expect("Should have ContributorRoles");
    let jane_role = roles
        .iter()
        .find(|r| r.get("Name").and_then(|n| n.as_str()) == Some("Jane Doe"))
        .expect("Should find Jane Doe in ContributorRoles");
    assert_eq!(
        jane_role.get("Role").and_then(|r| r.as_str()),
        Some("Author"),
        "ContributorRole should have Role: Author"
    );
}

// =============================================================================
// Page Count and Rating Tests
// =============================================================================

#[tokio::test]
async fn test_sync_metadata_has_page_count() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "pagecount@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Page Count Book").await;
    entitle_book(&server, book_id).await;

    // Set page_count directly in DB
    sqlx::query("UPDATE books SET page_count = 350, updated_at = NOW() WHERE id = $1")
        .bind(book_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    let metadata = get_sync_book_metadata(&items, book_id).expect("Book not found");
    assert_eq!(
        metadata.get("NumberOfPages").and_then(|v| v.as_i64()),
        Some(350),
        "Metadata should have NumberOfPages=350"
    );
}

#[tokio::test]
async fn test_sync_metadata_has_rating() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "rating@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Rated Book").await;
    entitle_book(&server, book_id).await;

    // Set rating directly in DB
    sqlx::query("UPDATE books SET rating = 4.5, updated_at = NOW() WHERE id = $1")
        .bind(book_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    let metadata = get_sync_book_metadata(&items, book_id).expect("Book not found");
    let rating = metadata
        .get("Rating")
        .and_then(|v| v.as_f64())
        .expect("Should have Rating");
    assert!(
        (rating - 4.5).abs() < f64::EPSILON,
        "Rating should be 4.5, got {}",
        rating
    );
}

#[tokio::test]
async fn test_sync_metadata_no_page_count_when_null() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "nopagecount@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "No Pages Book").await;
    entitle_book(&server, book_id).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    let metadata = get_sync_book_metadata(&items, book_id).expect("Book not found");
    assert!(
        metadata.get("NumberOfPages").is_none(),
        "NumberOfPages should be absent when page_count is null"
    );
    assert!(
        metadata.get("Rating").is_none(),
        "Rating should be absent when rating is null"
    );
}

// =============================================================================
// Combined Metadata Test (validates JSONB aggregation path)
// =============================================================================

/// Validates that the JSONB aggregation in list_for_sync produces correct
/// Contributors, DownloadUrls, and ReadingState in a single sync response.
#[tokio::test]
async fn test_sync_metadata_complete_with_authors_files_reading_state() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "complete_meta@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create book with author and epub file
    let book_id = create_book_with_author(&server, "Complete Metadata Book", "Test Author").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;
    entitle_book(&server, book_id).await;

    // Set reading state via Kobo API (progress 42%)
    server
        .kobo_client()
        .kobo_update_state(
            &device_token,
            &book_id.to_string(),
            &serde_json::json!({
                "ReadingStates": [{
                    "CurrentBookmark": {
                        "ProgressPercent": 0.42,
                        "ContentSourceProgressPercent": 0.42
                    },
                    "StatusInfo": {
                        "Status": "Reading"
                    }
                }]
            }),
        )
        .await
        .assert_ok();

    // Touch book so it appears in next sync
    touch_book(&server, book_id).await;

    // Sync and verify all three: authors, files, reading state
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    // 1. Contributors populated
    let metadata = get_sync_book_metadata(&items, book_id).expect("Book not found in sync");
    let contributors = metadata
        .get("Contributors")
        .and_then(|v| v.as_array())
        .expect("Should have Contributors");
    assert!(
        contributors
            .iter()
            .any(|c| c.as_str() == Some("Test Author")),
        "Contributors should contain 'Test Author', got: {:?}",
        contributors
    );

    let roles = metadata
        .get("ContributorRoles")
        .and_then(|v| v.as_array())
        .expect("Should have ContributorRoles");
    assert!(
        roles
            .iter()
            .any(|r| r.get("Name").and_then(|n| n.as_str()) == Some("Test Author")),
        "ContributorRoles should contain 'Test Author'"
    );

    // 2. DownloadUrls populated with KEPUB format
    assert_sync_book_has_download_urls(&items, book_id);
    assert_sync_book_has_format(&items, book_id, "KEPUB");

    // 3. ReadingState present with progress
    let item = find_sync_book(&items, book_id).expect("Book not found");
    let entitlement = item
        .get("NewEntitlement")
        .or_else(|| item.get("ChangedEntitlement"))
        .expect("No entitlement wrapper");
    let reading_state = entitlement.get("ReadingState");
    assert!(
        reading_state.is_some(),
        "Sync item should include ReadingState. Full item: {}",
        serde_json::to_string_pretty(entitlement).unwrap_or_default()
    );
    assert_reading_state_progress(reading_state.unwrap(), 0.42);
    assert_reading_state_status(reading_state.unwrap(), "Reading");
}
