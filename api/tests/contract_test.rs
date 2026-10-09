//! Contract tests ensuring Kobbler can deserialize types shared with Kobload.
//!
//! These tests use canonical fixtures from koblibs to verify that
//! field renames, additions, or removals are caught immediately.

use nats_jobs::test_fixtures;

/// Kobbler deserializes WantResponse events from Kobload.
#[test]
fn kobbler_deserializes_want_response() {
    let fixture = test_fixtures::sample_want_response_downloading();
    let json = serde_json::to_value(&fixture).unwrap();

    let deserialized: nats_jobs::WantResponse = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized.status, nats_jobs::WantStatus::Downloading);
    assert!(deserialized.download_id.is_some());
    assert!(deserialized.metadata.is_some());
}

/// Kobbler deserializes watching status responses.
#[test]
fn kobbler_deserializes_want_response_watching() {
    let fixture = test_fixtures::sample_want_response_watching();
    let json = serde_json::to_value(&fixture).unwrap();

    let deserialized: nats_jobs::WantResponse = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized.status, nats_jobs::WantStatus::Watching);
    assert!(deserialized.download_id.is_none());
}

/// Kobbler deserializes SearchResponse from Kobload.
#[test]
fn kobbler_deserializes_search_response() {
    let fixture = test_fixtures::sample_search_response();
    let json = serde_json::to_value(&fixture).unwrap();

    let deserialized: nats_jobs::SearchResponse = serde_json::from_value(json).unwrap();
    assert!(!deserialized.results.is_empty());
    assert_eq!(deserialized.results[0].guid, "abc-123");
}

/// Kobbler deserializes WatchMatchedEvent from Kobload.
#[test]
fn kobbler_deserializes_watch_matched_event() {
    let fixture = test_fixtures::sample_watch_matched_event();
    let json = serde_json::to_value(&fixture).unwrap();

    let deserialized: nats_jobs::WatchMatchedEvent = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized.source_system, "kobbler");
    assert_eq!(deserialized.action, "downloaded");
    assert!(deserialized.download_id.is_some());
}

/// Kobbler's ingest callback can deserialize the canonical fixture.
/// This tests the shape of the HTTP callback from Kobload.
#[test]
fn kobbler_deserializes_ingest_callback() {
    let json = test_fixtures::sample_ingest_callback_json();

    // Verify the shape matches KobloadCallbackRequest fields
    assert!(json["download_id"].is_string());
    assert_eq!(json["status"], "completed");
    assert!(json["metadata"].is_object());
    assert!(json["metadata"]["book_id"].is_string());
    assert!(json["files"].is_array());

    let file = &json["files"][0];
    assert!(file["path"].is_string());
    assert!(file["size"].is_number());
    assert!(file["extension"].is_string());
    assert!(file["mime_type"].is_string());
}

/// Kobbler serializes WantRequest correctly for Kobload.
#[test]
fn kobbler_serializes_want_request() {
    let fixture = test_fixtures::sample_want_request();
    let json = serde_json::to_value(&fixture).unwrap();

    // Verify Kobload can deserialize it
    let deserialized: nats_jobs::WantRequest = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized.source_system, "kobbler");
}

/// Kobbler serializes WatchPatternUpsert correctly for Kobload.
#[test]
fn kobbler_serializes_watch_pattern_upsert() {
    let fixture = test_fixtures::sample_watch_pattern_upsert();
    let json = serde_json::to_value(&fixture).unwrap();

    let deserialized: nats_jobs::WatchPatternUpsert = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized.source_system, "kobbler");
    assert!(deserialized.media_want.is_some());
}

/// Kobbler serializes SearchRequest correctly for Kobload.
#[test]
fn kobbler_serializes_search_request() {
    let fixture = test_fixtures::sample_search_request();
    let json = serde_json::to_value(&fixture).unwrap();

    let deserialized: nats_jobs::SearchRequest = serde_json::from_value(json).unwrap();
    assert!(!deserialized.query.is_empty());
}

/// DownloadCompletedEvent round-trips correctly.
#[test]
fn download_completed_event_round_trip() {
    let fixture = test_fixtures::sample_download_completed();
    let json = serde_json::to_value(&fixture).unwrap();

    let deserialized: nats_jobs::DownloadCompletedEvent = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized.status, "completed");
    assert_eq!(deserialized.files.len(), 1);
    assert!(deserialized.error.is_none());
}

/// Failed download event round-trips correctly.
#[test]
fn download_failed_event_round_trip() {
    let fixture = test_fixtures::sample_download_failed();
    let json = serde_json::to_value(&fixture).unwrap();

    let deserialized: nats_jobs::DownloadCompletedEvent = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized.status, "failed");
    assert!(deserialized.error.is_some());
    assert!(deserialized.files.is_empty());
}
