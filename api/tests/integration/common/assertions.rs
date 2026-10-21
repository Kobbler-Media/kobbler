use serde_json::Value;
use uuid::Uuid;

// =============================================================================
// Book Sync Assertions
// =============================================================================

/// Assert that the sync response contains a book with the given ID.
pub fn assert_sync_contains_book(items: &[Value], book_id: Uuid) {
    assert!(
        find_sync_book(items, book_id).is_some(),
        "Book {} not found in sync response.\nItems: {}",
        book_id,
        serde_json::to_string_pretty(items).unwrap_or_default()
    );
}

/// Assert that the sync response does NOT contain a book with the given ID.
pub fn assert_sync_excludes_book(items: &[Value], book_id: Uuid) {
    assert!(
        find_sync_book(items, book_id).is_none(),
        "Book {} should NOT be in sync response.\nItems: {}",
        book_id,
        serde_json::to_string_pretty(items).unwrap_or_default()
    );
}

/// Assert that a book in the sync response is marked as removed.
pub fn assert_book_is_removed(items: &[Value], book_id: Uuid) {
    let item = find_sync_book(items, book_id)
        .unwrap_or_else(|| panic!("Book {} not found in sync response", book_id));

    let entitlement = item
        .get("NewEntitlement")
        .or_else(|| item.get("ChangedEntitlement"))
        .unwrap();

    let is_removed = entitlement
        .get("BookEntitlement")
        .and_then(|be| be.get("IsRemoved"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    assert!(
        is_removed,
        "Book {} should be marked as removed.\nEntitlement: {}",
        book_id,
        serde_json::to_string_pretty(entitlement).unwrap_or_default()
    );
}

/// Assert that a book in the sync response is NOT marked as removed.
pub fn assert_book_is_not_removed(items: &[Value], book_id: Uuid) {
    let item = find_sync_book(items, book_id)
        .unwrap_or_else(|| panic!("Book {} not found in sync response", book_id));

    let entitlement = item
        .get("NewEntitlement")
        .or_else(|| item.get("ChangedEntitlement"))
        .unwrap();

    let is_removed = entitlement
        .get("BookEntitlement")
        .and_then(|be| be.get("IsRemoved"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    assert!(
        !is_removed,
        "Book {} should NOT be marked as removed.\nEntitlement: {}",
        book_id,
        serde_json::to_string_pretty(entitlement).unwrap_or_default()
    );
}

/// Find a book in sync response by ID.
pub fn find_sync_book(items: &[Value], book_id: Uuid) -> Option<&Value> {
    items.iter().find(|item| {
        let entitlement = item
            .get("NewEntitlement")
            .or_else(|| item.get("ChangedEntitlement"));

        if let Some(ent) = entitlement {
            if let Some(be) = ent.get("BookEntitlement") {
                if let Some(id) = be.get("Id").and_then(|v| v.as_str()) {
                    return id == book_id.to_string();
                }
            }
        }
        false
    })
}

/// Find a book in sync response by ID (string version for store books).
pub fn find_sync_book_by_str<'a>(items: &'a [Value], book_id: &str) -> Option<&'a Value> {
    items.iter().find(|item| {
        let entitlement = item
            .get("NewEntitlement")
            .or_else(|| item.get("ChangedEntitlement"));

        if let Some(ent) = entitlement {
            if let Some(be) = ent.get("BookEntitlement") {
                if let Some(id) = be.get("Id").and_then(|v| v.as_str()) {
                    return id == book_id;
                }
            }
        }
        false
    })
}

/// Assert sync contains a book with ID (string version).
pub fn assert_sync_contains_book_str(items: &[Value], book_id: &str) {
    assert!(
        find_sync_book_by_str(items, book_id).is_some(),
        "Book {} not found in sync response.\nItems: {}",
        book_id,
        serde_json::to_string_pretty(items).unwrap_or_default()
    );
}

/// Count the number of books in a sync response.
pub fn count_sync_books(items: &[Value]) -> usize {
    items
        .iter()
        .filter(|item| {
            item.get("NewEntitlement").is_some() || item.get("ChangedEntitlement").is_some()
        })
        .count()
}

/// Get all book IDs from a sync response.
pub fn get_sync_book_ids(items: &[Value]) -> Vec<Uuid> {
    items
        .iter()
        .filter_map(|item| {
            let entitlement = item
                .get("NewEntitlement")
                .or_else(|| item.get("ChangedEntitlement"))?;
            let id_str = entitlement.get("BookEntitlement")?.get("Id")?.as_str()?;
            Uuid::parse_str(id_str).ok()
        })
        .collect()
}

/// Get all book IDs as strings (includes store books with non-UUID IDs).
pub fn get_sync_book_id_strings(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| {
            let entitlement = item
                .get("NewEntitlement")
                .or_else(|| item.get("ChangedEntitlement"))?;
            let id_str = entitlement.get("BookEntitlement")?.get("Id")?.as_str()?;
            Some(id_str.to_string())
        })
        .collect()
}

// =============================================================================
// Book Metadata Assertions
// =============================================================================

/// Get the metadata for a book from sync response.
pub fn get_sync_book_metadata(items: &[Value], book_id: Uuid) -> Option<&Value> {
    let item = find_sync_book(items, book_id)?;
    let entitlement = item
        .get("NewEntitlement")
        .or_else(|| item.get("ChangedEntitlement"))?;
    entitlement.get("BookMetadata")
}

/// Assert a book's title in sync response.
pub fn assert_sync_book_title(items: &[Value], book_id: Uuid, expected_title: &str) {
    let metadata = get_sync_book_metadata(items, book_id)
        .unwrap_or_else(|| panic!("Book {} not found in sync", book_id));

    let title = metadata.get("Title").and_then(|v| v.as_str()).unwrap_or("");

    assert_eq!(
        title,
        expected_title,
        "Book {} title mismatch.\nMetadata: {}",
        book_id,
        serde_json::to_string_pretty(metadata).unwrap_or_default()
    );
}

/// Assert a book has download URLs in sync response.
pub fn assert_sync_book_has_download_urls(items: &[Value], book_id: Uuid) {
    let metadata = get_sync_book_metadata(items, book_id)
        .unwrap_or_else(|| panic!("Book {} not found in sync", book_id));

    let urls = metadata.get("DownloadUrls").and_then(|v| v.as_array());

    assert!(
        urls.is_some() && !urls.unwrap().is_empty(),
        "Book {} should have download URLs.\nMetadata: {}",
        book_id,
        serde_json::to_string_pretty(metadata).unwrap_or_default()
    );
}

/// Assert a book has a specific download format.
pub fn assert_sync_book_has_format(items: &[Value], book_id: Uuid, format: &str) {
    let metadata = get_sync_book_metadata(items, book_id)
        .unwrap_or_else(|| panic!("Book {} not found in sync", book_id));

    let empty_vec = vec![];
    let urls = metadata
        .get("DownloadUrls")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty_vec);

    let has_format = urls.iter().any(|url| {
        url.get("Format")
            .and_then(|f| f.as_str())
            .map(|f| f == format)
            .unwrap_or(false)
    });

    assert!(
        has_format,
        "Book {} should have format {}.\nDownloadUrls: {}",
        book_id,
        format,
        serde_json::to_string_pretty(urls).unwrap_or_default()
    );
}

/// Assert a book's series info in sync response.
pub fn assert_sync_book_series(
    items: &[Value],
    book_id: Uuid,
    expected_series: &str,
    expected_index: i32,
) {
    let metadata = get_sync_book_metadata(items, book_id)
        .unwrap_or_else(|| panic!("Book {} not found in sync", book_id));

    let series = metadata
        .get("Series")
        .and_then(|s| s.get("Name"))
        .and_then(|n| n.as_str());
    let index = metadata
        .get("Series")
        .and_then(|s| s.get("Number"))
        .and_then(|n| n.as_i64());

    assert_eq!(
        series,
        Some(expected_series),
        "Book {} series name mismatch.\nMetadata: {}",
        book_id,
        serde_json::to_string_pretty(metadata).unwrap_or_default()
    );
    assert_eq!(
        index,
        Some(expected_index as i64),
        "Book {} series index mismatch.\nMetadata: {}",
        book_id,
        serde_json::to_string_pretty(metadata).unwrap_or_default()
    );
}

// =============================================================================
// Tag Assertions
// =============================================================================

/// Assert that the sync response contains a tag with the given ID and name.
pub fn assert_sync_contains_tag(items: &[Value], tag_id: Uuid, expected_name: &str) {
    let found = find_sync_tag(items, tag_id);

    assert!(
        found.is_some(),
        "Tag {} ({}) not found in sync response.\nItems: {}",
        expected_name,
        tag_id,
        serde_json::to_string_pretty(items).unwrap_or_default()
    );

    let tag = found.unwrap();
    let tag_data = tag.get("NewTag").or_else(|| tag.get("ChangedTag")).unwrap();
    let inner = tag_data.get("Tag").unwrap_or(tag_data);
    let name = inner.get("Name").and_then(|v| v.as_str()).unwrap_or("");

    assert_eq!(
        name, expected_name,
        "Tag {} name mismatch. Expected: {}, Got: {}",
        tag_id, expected_name, name
    );
}

/// Find a tag in sync response.
pub fn find_sync_tag(items: &[Value], tag_id: Uuid) -> Option<&Value> {
    items.iter().find(|item| {
        let tag = item.get("NewTag").or_else(|| item.get("ChangedTag"));
        if let Some(t) = tag {
            // Handle both {"NewTag": {"Tag": {...}}} and {"NewTag": {...}} formats
            let inner = t.get("Tag").unwrap_or(t);
            if let Some(id) = inner.get("Id").and_then(|v| v.as_str()) {
                return id == tag_id.to_string();
            }
        }
        false
    })
}

/// Assert that the sync response contains a deleted tag with the given ID.
pub fn assert_sync_contains_deleted_tag(items: &[Value], tag_id: Uuid) {
    let found = items.iter().any(|item| {
        if let Some(tag) = item.get("DeletedTag") {
            let inner = tag.get("Tag").unwrap_or(tag);
            if let Some(id) = inner.get("Id").and_then(|v| v.as_str()) {
                return id == tag_id.to_string();
            }
        }
        false
    });

    assert!(
        found,
        "Deleted tag {} not found in sync response.\nItems: {}",
        tag_id,
        serde_json::to_string_pretty(items).unwrap_or_default()
    );
}

/// Count the number of tags in a sync response.
pub fn count_sync_tags(items: &[Value]) -> usize {
    items
        .iter()
        .filter(|item| {
            item.get("NewTag").is_some()
                || item.get("ChangedTag").is_some()
                || item.get("DeletedTag").is_some()
        })
        .count()
}

// =============================================================================
// Initialization Assertions
// =============================================================================

/// Assert initialization response contains expected resource.
pub fn assert_init_has_resource(init_response: &Value, resource_name: &str) {
    let resources = init_response.get("Resources");
    assert!(
        resources.is_some(),
        "No Resources in init response: {}",
        serde_json::to_string_pretty(init_response).unwrap_or_default()
    );

    let resources = resources.unwrap();
    let has_resource = resources.get(resource_name).is_some();
    assert!(
        has_resource,
        "Resource '{}' not found in init response.\nResources: {}",
        resource_name,
        serde_json::to_string_pretty(resources).unwrap_or_default()
    );
}

/// Assert initialization resource URL contains expected base URL.
pub fn assert_init_resource_contains(init_response: &Value, resource_name: &str, contains: &str) {
    let resources = init_response
        .get("Resources")
        .expect("No Resources in init");
    let url = resources
        .get(resource_name)
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("Resource {} not found", resource_name));

    assert!(
        url.contains(contains),
        "Resource '{}' URL should contain '{}'. Got: {}",
        resource_name,
        contains,
        url
    );
}

// =============================================================================
// Reading State Assertions
// =============================================================================

/// Find a `ChangedReadingState` entry by book ID (EntitlementId) in sync response.
pub fn find_sync_reading_state(items: &[Value], book_id: Uuid) -> Option<&Value> {
    items.iter().find(|item| {
        if let Some(crs) = item.get("ChangedReadingState") {
            if let Some(rs) = crs.get("ReadingState") {
                if let Some(id) = rs.get("EntitlementId").and_then(|v| v.as_str()) {
                    return id == book_id.to_string();
                }
            }
        }
        false
    })
}

/// Assert that the sync response contains a `ChangedReadingState` for the given book.
pub fn assert_sync_contains_reading_state(items: &[Value], book_id: Uuid) {
    assert!(
        find_sync_reading_state(items, book_id).is_some(),
        "ChangedReadingState for book {} not found in sync response.\nItems: {}",
        book_id,
        serde_json::to_string_pretty(items).unwrap_or_default()
    );
}

/// Assert that the sync response does NOT contain a `ChangedReadingState` for the given book.
pub fn assert_sync_excludes_reading_state(items: &[Value], book_id: Uuid) {
    assert!(
        find_sync_reading_state(items, book_id).is_none(),
        "ChangedReadingState for book {} should NOT be in sync response.\nItems: {}",
        book_id,
        serde_json::to_string_pretty(items).unwrap_or_default()
    );
}

/// Assert reading state progress.
pub fn assert_reading_state_progress(state: &Value, expected_progress: f64) {
    let progress = state
        .get("CurrentBookmark")
        .and_then(|b| b.get("ProgressPercent"))
        .and_then(|p| p.as_f64())
        .unwrap_or(0.0);

    assert!(
        (progress - expected_progress).abs() < 0.01,
        "Expected progress {}, got {}.\nState: {}",
        expected_progress,
        progress,
        serde_json::to_string_pretty(state).unwrap_or_default()
    );
}

/// Assert reading state status.
pub fn assert_reading_state_status(state: &Value, expected_status: &str) {
    let status = state
        .get("StatusInfo")
        .and_then(|s| s.get("Status"))
        .and_then(|s| s.as_str())
        .unwrap_or("");

    assert_eq!(
        status,
        expected_status,
        "Expected status '{}', got '{}'.\nState: {}",
        expected_status,
        status,
        serde_json::to_string_pretty(state).unwrap_or_default()
    );
}
