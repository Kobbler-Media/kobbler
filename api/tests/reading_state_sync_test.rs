use crate::common::*;
use serde_json::Value;
use uuid::Uuid;

/// Helper: update reading state via Kobo API and return the response.
async fn update_state(
    server: &TestServer,
    device_token: &str,
    book_id: uuid::Uuid,
    status: &str,
    progress: f64,
    location_source: Option<&str>,
) {
    let location = location_source.map(|src| {
        serde_json::json!({
            "Value": format!("loc-{}", progress),
            "Type": if src == "web-reader" { "cfi" } else { "Location" },
            "Source": src
        })
    });

    let mut bookmark = serde_json::json!({
        "ProgressPercent": progress,
        "ContentSourceProgressPercent": progress,
    });
    if let Some(loc) = location {
        bookmark["Location"] = loc;
    }

    let resp = server
        .kobo_client()
        .kobo_update_state(
            device_token,
            &book_id.to_string(),
            &serde_json::json!({
                "ReadingStates": [{
                    "StatusInfo": { "Status": status },
                    "CurrentBookmark": bookmark,
                }]
            }),
        )
        .await;
    resp.assert_ok();
}

/// Helper: get reading state via Kobo API.
async fn get_state(server: &TestServer, device_token: &str, book_id: uuid::Uuid) -> Value {
    let resp = server
        .kobo_client()
        .kobo_get_state(device_token, &book_id.to_string())
        .await;
    resp.assert_ok();
    let states: Vec<Value> = resp.json();
    assert_eq!(states.len(), 1);
    states.into_iter().next().unwrap()
}

/// Setup: user with two devices and an entitled book.
struct TwoDeviceSetup {
    book_id: uuid::Uuid,
    token_a: String,
    token_b: String,
}

async fn setup_two_devices(server: &TestServer) -> TwoDeviceSetup {
    let user_id = create_user(server, "sync-test@test.com").await;
    let (_, token_a) = create_device(server, user_id).await;
    let (_, token_b) = create_device(server, user_id).await;
    let book_id = create_book(server, "Sync Test Book").await;
    entitle_book(server, book_id).await;
    TwoDeviceSetup {
        book_id,
        token_a,
        token_b,
    }
}

#[tokio::test]
async fn reading_beats_zero() {
    let server = TestServer::start().await;
    let s = setup_two_devices(&server).await;

    // Device A (web reader): Reading at 53%
    update_state(
        &server,
        &s.token_a,
        s.book_id,
        "Reading",
        53.0,
        Some("web-reader"),
    )
    .await;

    // Device B (Kobo): ReadyToRead at 0% (newer timestamp)
    update_state(&server, &s.token_b, s.book_id, "ReadyToRead", 0.0, None).await;

    // Device B should see the Reading/53% state, not the newer 0%
    let state = get_state(&server, &s.token_b, s.book_id).await;
    assert_eq!(state["StatusInfo"]["Status"].as_str().unwrap(), "Reading");
    let progress = state["CurrentBookmark"]["ProgressPercent"]
        .as_f64()
        .unwrap();
    assert!(
        (progress - 53.0).abs() < 0.1,
        "Expected ~53%, got {progress}"
    );
}

#[tokio::test]
async fn both_reading_most_recent_wins() {
    let server = TestServer::start().await;
    let s = setup_two_devices(&server).await;

    // Device A: Reading at 40%
    update_state(
        &server,
        &s.token_a,
        s.book_id,
        "Reading",
        40.0,
        Some("Kobo"),
    )
    .await;

    // Device B: Reading at 70% (newer)
    update_state(
        &server,
        &s.token_b,
        s.book_id,
        "Reading",
        70.0,
        Some("Kobo"),
    )
    .await;

    // Device A should see 70% (both Reading, most recent wins)
    let state = get_state(&server, &s.token_a, s.book_id).await;
    assert_eq!(state["StatusInfo"]["Status"].as_str().unwrap(), "Reading");
    let progress = state["CurrentBookmark"]["ProgressPercent"]
        .as_f64()
        .unwrap();
    assert!(
        (progress - 70.0).abs() < 0.1,
        "Expected ~70%, got {progress}"
    );
}

#[tokio::test]
async fn reading_beats_finished() {
    let server = TestServer::start().await;
    let s = setup_two_devices(&server).await;

    // Device B: Finished at 100%
    update_state(
        &server,
        &s.token_b,
        s.book_id,
        "Finished",
        100.0,
        Some("Kobo"),
    )
    .await;

    // Device A: Reading at 5% (newer, but Reading beats Finished in priority)
    update_state(
        &server,
        &s.token_a,
        s.book_id,
        "Reading",
        5.0,
        Some("web-reader"),
    )
    .await;

    // Should see Reading/5%
    let state = get_state(&server, &s.token_b, s.book_id).await;
    assert_eq!(state["StatusInfo"]["Status"].as_str().unwrap(), "Reading");
    let progress = state["CurrentBookmark"]["ProgressPercent"]
        .as_f64()
        .unwrap();
    assert!((progress - 5.0).abs() < 0.1, "Expected ~5%, got {progress}");
}

#[tokio::test]
async fn all_zero_most_recent_wins() {
    let server = TestServer::start().await;
    let s = setup_two_devices(&server).await;

    // Device A: ReadyToRead 0%
    update_state(&server, &s.token_a, s.book_id, "ReadyToRead", 0.0, None).await;

    // Device B: ReadyToRead 0% (newer)
    update_state(&server, &s.token_b, s.book_id, "ReadyToRead", 0.0, None).await;

    // Both are equivalent priority, most recent (device B) wins - just check it doesn't crash
    let state = get_state(&server, &s.token_a, s.book_id).await;
    assert_eq!(
        state["StatusInfo"]["Status"].as_str().unwrap(),
        "ReadyToRead"
    );
}

#[tokio::test]
async fn non_kobo_location_stripped() {
    let server = TestServer::start().await;
    let s = setup_two_devices(&server).await;

    // Device A: web-reader location
    update_state(
        &server,
        &s.token_a,
        s.book_id,
        "Reading",
        53.0,
        Some("web-reader"),
    )
    .await;

    // When Kobo device fetches, the web-reader location should be stripped
    let state = get_state(&server, &s.token_b, s.book_id).await;
    assert!(
        state["CurrentBookmark"]["Location"].is_null(),
        "web-reader Location should be stripped from Kobo sync response, got: {}",
        state["CurrentBookmark"]["Location"]
    );
    // But progress should still be present
    let progress = state["CurrentBookmark"]["ProgressPercent"]
        .as_f64()
        .unwrap();
    assert!((progress - 53.0).abs() < 0.1);
}

#[tokio::test]
async fn reading_state_change_triggers_sync() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "rs-sync@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;
    let book_id = create_book(&server, "Reading Sync Book").await;
    entitle_book(&server, book_id).await;

    // Initial sync: book should appear as NewEntitlement
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();
    assert_sync_contains_book(&items, book_id);
    let sync_token = resp
        .get_sync_token()
        .expect("should have sync token after initial sync");

    // Second sync with token: nothing should appear (no changes)
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &sync_token)
        .await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();
    assert_sync_excludes_book(&items, book_id);
    let sync_token = resp.get_sync_token().unwrap_or(sync_token);

    // Update reading state (no book metadata or entitlement change)
    update_state(&server, &device_token, book_id, "Reading", 42.0, None).await;

    // Third sync: reading state change should appear as ChangedReadingState (NOT ChangedEntitlement)
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &sync_token)
        .await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();

    // Book should NOT appear as a ChangedEntitlement (no re-download)
    assert_sync_excludes_book(&items, book_id);

    // But should appear as a standalone ChangedReadingState
    assert_sync_contains_reading_state(&items, book_id);

    // Fourth sync: reading state should NOT appear again (cursor advanced)
    let sync_token = resp
        .get_sync_token()
        .expect("should have sync token after reading state sync");
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &sync_token)
        .await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();
    assert_sync_excludes_book(&items, book_id);
    assert_sync_excludes_reading_state(&items, book_id);
}

// =============================================================================
// Feedback loop prevention tests
// =============================================================================

/// The core bug fix: reading progress should never cause re-download.
/// Simulates the real-world loop: read → progress saved → sync → should NOT re-download.
#[tokio::test]
async fn reading_state_only_change_never_triggers_entitlement() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "no-redownload@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;
    let book_id = create_book(&server, "No Redownload Book").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;
    entitle_book(&server, book_id).await;

    // Initial sync
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let sync_token = resp.get_sync_token().unwrap();

    // Simulate reading: update progress 5 times (like a real reading session)
    for progress in [10.0, 25.0, 40.0, 55.0, 70.0] {
        update_state(&server, &device_token, book_id, "Reading", progress, None).await;
    }

    // Sync: should get ChangedReadingState, NOT ChangedEntitlement
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &sync_token)
        .await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();

    assert_sync_excludes_book(&items, book_id);
    assert_sync_contains_reading_state(&items, book_id);

    // Verify cursor advances: next sync is empty
    let sync_token = resp.get_sync_token().unwrap();
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &sync_token)
        .await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();
    assert_sync_excludes_book(&items, book_id);
    assert_sync_excludes_reading_state(&items, book_id);
}

/// Multiple reading state changes between syncs should produce only one
/// ChangedReadingState per book (deduplicated by book_id via DISTINCT ON).
#[tokio::test]
async fn multiple_reading_state_changes_one_sync_entry() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "multi-rs@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;
    let book_id = create_book(&server, "Multi RS Book").await;
    entitle_book(&server, book_id).await;

    // Initial sync
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    let sync_token = resp.get_sync_token().unwrap();

    // Multiple reading state updates
    update_state(&server, &device_token, book_id, "Reading", 10.0, None).await;
    update_state(&server, &device_token, book_id, "Reading", 30.0, None).await;
    update_state(&server, &device_token, book_id, "Reading", 50.0, None).await;

    // Should appear exactly once as ChangedReadingState
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &sync_token)
        .await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();

    let rs_count = items
        .iter()
        .filter(|item| {
            item.get("ChangedReadingState")
                .and_then(|crs| crs.get("ReadingState"))
                .and_then(|rs| rs.get("EntitlementId"))
                .and_then(|id| id.as_str())
                .map(|id| id == book_id.to_string())
                .unwrap_or(false)
        })
        .count();

    assert_eq!(
        rs_count, 1,
        "Should have exactly 1 ChangedReadingState for the book, got {}",
        rs_count
    );
}

/// When a book has BOTH metadata changes and reading state changes between syncs,
/// the book should appear as ChangedEntitlement (with inline ReadingState),
/// NOT as a separate ChangedReadingState entry.
#[tokio::test]
async fn metadata_and_reading_state_change_combined() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "combined@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;
    let book_id = create_book(&server, "Combined Change Book").await;
    entitle_book(&server, book_id).await;

    // Initial sync
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    let sync_token = resp.get_sync_token().unwrap();

    // Change both reading state AND metadata
    update_state(&server, &device_token, book_id, "Reading", 42.0, None).await;
    touch_book(&server, book_id).await; // Simulates metadata edit

    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &sync_token)
        .await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();

    // Book should appear as ChangedEntitlement (metadata changed)
    assert_sync_contains_book(&items, book_id);

    // Reading state should be INLINE on the entitlement, not separate
    let book_item = find_sync_book(&items, book_id).unwrap();
    let entry = book_item.get("ChangedEntitlement").unwrap();
    assert!(
        entry.get("ReadingState").is_some(),
        "Reading state should be inline on ChangedEntitlement"
    );

    // Should NOT also appear as a separate ChangedReadingState
    assert_sync_excludes_reading_state(&items, book_id);
}

// =============================================================================
// count_pending_sync consistency tests
// =============================================================================

/// count_pending_sync should NOT count reading-state-only changes.
/// This ensures the count and the list stay in agreement.
#[tokio::test]
async fn count_pending_sync_excludes_reading_state_only() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "count-rs@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;
    let book_id = create_book(&server, "Count RS Book").await;
    entitle_book(&server, book_id).await;

    // Initial sync to establish baseline
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    let sync_token = resp.get_sync_token().unwrap();

    // Verify empty after initial sync
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &sync_token)
        .await;
    let items: Vec<Value> = resp.json();
    assert_eq!(count_sync_books(&items), 0);
    let sync_token = resp.get_sync_token().unwrap_or(sync_token);

    // Update only reading state
    update_state(&server, &device_token, book_id, "Reading", 42.0, None).await;

    // Sync should NOT return any entitlements
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &sync_token)
        .await;
    let items: Vec<Value> = resp.json();
    assert_eq!(
        count_sync_books(&items),
        0,
        "Reading state change should not produce entitlement items, got: {}",
        serde_json::to_string_pretty(&items).unwrap_or_default()
    );
    // But should have a ChangedReadingState
    assert_sync_contains_reading_state(&items, book_id);
}

// =============================================================================
// RevisionId stability tests
// =============================================================================

/// Metadata-only change should NOT change RevisionId.
/// The device uses RevisionId to decide whether to re-download the file.
#[tokio::test]
async fn metadata_change_preserves_revision_id() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "rev-stable@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;
    let book_id = create_book(&server, "Revision Stable Book").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;
    entitle_book(&server, book_id).await;

    // First sync: capture RevisionId
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();
    let metadata1 = get_sync_book_metadata(&items, book_id).unwrap();
    let rev1 = metadata1["RevisionId"].as_str().unwrap().to_string();

    // RevisionId should be a UUID (content_revision), not just the book_id
    assert!(
        Uuid::parse_str(&rev1).is_ok(),
        "RevisionId should be a valid UUID, got: {}",
        rev1
    );

    // Touch book (simulates metadata edit) and sync again
    touch_book(&server, book_id).await;
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();
    let metadata2 = get_sync_book_metadata(&items, book_id).unwrap();
    let rev2 = metadata2["RevisionId"].as_str().unwrap().to_string();

    // RevisionId should be SAME (no file content change)
    assert_eq!(
        rev1, rev2,
        "RevisionId should be stable when only metadata changes. rev1={}, rev2={}",
        rev1, rev2
    );
}

/// Uploading a new file should change RevisionId.
/// This tells the device to re-download the file.
#[tokio::test]
async fn file_upload_changes_revision_id() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "rev-change@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;
    let book_id = create_book(&server, "Revision Change Book").await;
    upload_epub(&server, book_id, test_epub_bytes()).await;
    entitle_book(&server, book_id).await;

    // First sync: capture RevisionId
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();
    let metadata1 = get_sync_book_metadata(&items, book_id).unwrap();
    let rev1 = metadata1["RevisionId"].as_str().unwrap().to_string();

    // Re-upload the file (same format, new content — triggers upsert ON CONFLICT)
    upload_epub(&server, book_id, test_epub_bytes()).await;
    // Also touch entitlements so the book reappears in sync
    sqlx::query("UPDATE entitlements SET updated_at = NOW() WHERE book_id = $1")
        .bind(book_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();
    let metadata2 = get_sync_book_metadata(&items, book_id).unwrap();
    let rev2 = metadata2["RevisionId"].as_str().unwrap().to_string();

    // RevisionId should be DIFFERENT (file content changed)
    assert_ne!(
        rev1, rev2,
        "RevisionId should change when file is re-uploaded. rev1={}, rev2={}",
        rev1, rev2
    );
}

/// Book without files should use book_id as fallback RevisionId.
#[tokio::test]
async fn no_files_uses_book_id_as_revision_id() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "rev-nofiles@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;
    let book_id = create_book(&server, "No Files Revision Book").await;
    entitle_book(&server, book_id).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();
    let metadata = get_sync_book_metadata(&items, book_id).unwrap();
    let rev = metadata["RevisionId"].as_str().unwrap();

    assert_eq!(
        rev,
        book_id.to_string(),
        "RevisionId should fall back to book_id when no files exist"
    );
}

// =============================================================================
// Reading state on removed books
// =============================================================================

/// Sync jobs queued via Kobo device reading state updates must include user_id.
/// Without user_id, the handler cannot retrieve stored hardcover_read_id,
/// causing duplicate entries on Hardcover instead of updates.
#[tokio::test]
async fn kobo_reading_state_sync_job_includes_user_id() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "kobo-userid@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;
    let book_id = create_book(&server, "Kobo User ID Test Book").await;
    entitle_book(&server, book_id).await;

    // Enable Hardcover sync for the user via user_sync_providers
    let config = serde_json::json!({"api_key": "test-key"});
    api::repositories::user_sync_providers::upsert(
        &server.pool,
        user_id,
        "hardcover",
        true,
        &config,
    )
    .await
    .expect("Failed to enable hardcover");

    // Enable Hardcover sync for the device via device_sync_providers
    api::repositories::user_sync_providers::upsert_device_enabled(
        &server.pool,
        device_id,
        "hardcover",
        true,
    )
    .await
    .expect("Failed to enable device hardcover sync");

    // Update reading state via Kobo API (this triggers queue_sync_job)
    update_state(&server, &device_token, book_id, "Reading", 42.0, None).await;

    // Check that sync job payloads have user_id set
    let rows: Vec<(serde_json::Value,)> = sqlx::query_as(
        "SELECT payload FROM job_status WHERE job_type = 'sync_provider' AND payload->>'book_id' = $1 ORDER BY created_at",
    )
    .bind(book_id.to_string())
    .fetch_all(&server.pool)
    .await
    .expect("Failed to query sync jobs");

    assert!(
        !rows.is_empty(),
        "Should have queued sync jobs for Hardcover"
    );

    for (payload,) in &rows {
        let payload_user_id = payload.get("user_id").and_then(|v| v.as_str());
        assert!(
            payload_user_id.is_some(),
            "Kobo sync job payload must include user_id for read ID lookup. Got: {:?}",
            payload
        );
        assert_eq!(
            payload_user_id.unwrap(),
            user_id.to_string(),
            "Sync job user_id should match the device owner"
        );
    }
}

/// Reading state change on a removed book should NOT produce a ChangedReadingState.
/// The device has already been told to remove the book.
#[tokio::test]
async fn reading_state_on_removed_book_ignored() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "removed-rs@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;
    let book_id = create_book(&server, "Removed RS Book").await;
    entitle_book(&server, book_id).await;

    // Initial sync
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    let sync_token = resp.get_sync_token().unwrap();

    // Remove the book
    server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/books/{}/remove", book_id),
            &serde_json::json!({}),
        )
        .await
        .assert_success();

    // Sync to pick up the removal
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &sync_token)
        .await;
    let sync_token = resp.get_sync_token().unwrap();

    // Now update reading state on the removed book
    update_state(&server, &device_token, book_id, "Reading", 42.0, None).await;

    // Sync should NOT include a ChangedReadingState for the removed book
    let resp = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &sync_token)
        .await;
    let items: Vec<Value> = resp.json();
    assert_sync_excludes_reading_state(&items, book_id);
}
