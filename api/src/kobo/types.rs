//! Shared types for Kobo sync protocol.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Response types for reading state
#[derive(Serialize)]
pub struct ReadingStateResponse {
    #[serde(rename = "EntitlementId")]
    pub entitlement_id: Uuid,
    #[serde(rename = "Created")]
    pub created: String,
    #[serde(rename = "LastModified")]
    pub last_modified: String,
    #[serde(rename = "PriorityTimestamp")]
    pub priority_timestamp: String,
    #[serde(rename = "StatusInfo")]
    pub status_info: StatusInfoResponse,
    #[serde(rename = "Statistics")]
    pub statistics: StatisticsResponse,
    #[serde(rename = "CurrentBookmark")]
    pub current_bookmark: BookmarkResponse,
}

#[derive(Serialize)]
pub struct StatusInfoResponse {
    #[serde(rename = "LastModified")]
    pub last_modified: String,
    #[serde(rename = "Status")]
    pub status: String,
    #[serde(rename = "TimesStartedReading")]
    pub times_started_reading: i32,
    #[serde(
        rename = "LastTimeStartedReading",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_time_started_reading: Option<String>,
}

#[derive(Serialize)]
pub struct StatisticsResponse {
    #[serde(rename = "LastModified")]
    pub last_modified: String,
    #[serde(
        rename = "SpentReadingMinutes",
        skip_serializing_if = "Option::is_none"
    )]
    pub spent_reading_minutes: Option<i32>,
    #[serde(
        rename = "RemainingTimeMinutes",
        skip_serializing_if = "Option::is_none"
    )]
    pub remaining_time_minutes: Option<i32>,
}

#[derive(Serialize)]
pub struct BookmarkResponse {
    #[serde(rename = "LastModified")]
    pub last_modified: String,
    #[serde(rename = "ProgressPercent", skip_serializing_if = "Option::is_none")]
    pub progress_percent: Option<f64>,
    #[serde(
        rename = "ContentSourceProgressPercent",
        skip_serializing_if = "Option::is_none"
    )]
    pub content_source_progress_percent: Option<f64>,
    #[serde(rename = "Location", skip_serializing_if = "Option::is_none")]
    pub location: Option<LocationResponse>,
}

#[derive(Serialize)]
pub struct LocationResponse {
    #[serde(rename = "Value")]
    pub value: String,
    #[serde(rename = "Type")]
    pub location_type: String,
    #[serde(rename = "Source")]
    pub source: String,
}

/// Request types for reading state updates
#[derive(Deserialize)]
pub struct UpdateReadingStateRequest {
    #[serde(rename = "ReadingStates")]
    pub reading_states: Vec<ReadingStateUpdatePayload>,
}

#[derive(Deserialize)]
pub struct ReadingStateUpdatePayload {
    #[serde(rename = "CurrentBookmark")]
    pub current_bookmark: Option<BookmarkUpdate>,
    #[serde(rename = "Statistics")]
    pub statistics: Option<StatisticsUpdate>,
    #[serde(rename = "StatusInfo")]
    pub status_info: Option<StatusUpdate>,
}

#[derive(Deserialize)]
pub struct BookmarkUpdate {
    #[serde(rename = "ProgressPercent")]
    pub progress_percent: Option<f64>,
    #[serde(rename = "ContentSourceProgressPercent")]
    pub content_source_progress_percent: Option<f64>,
    #[serde(rename = "Location")]
    pub location: Option<LocationUpdate>,
}

#[derive(Deserialize)]
pub struct LocationUpdate {
    #[serde(rename = "Value")]
    pub value: Option<String>,
    #[serde(rename = "Type")]
    pub location_type: Option<String>,
    #[serde(rename = "Source")]
    pub source: Option<String>,
}

#[derive(Deserialize)]
pub struct StatisticsUpdate {
    #[serde(rename = "SpentReadingMinutes")]
    pub spent_reading_minutes: Option<i32>,
    #[serde(rename = "RemainingTimeMinutes")]
    pub remaining_time_minutes: Option<i32>,
}

#[derive(Deserialize)]
pub struct StatusUpdate {
    #[serde(rename = "Status")]
    pub status: Option<String>,
}

/// Request types for tags
#[derive(Deserialize)]
pub struct TagRequest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Items")]
    pub items: Option<Vec<TagItemRequest>>,
}

#[derive(Deserialize)]
pub struct TagItemRequest {
    #[serde(rename = "RevisionId")]
    pub revision_id: String,
    #[serde(rename = "Type")]
    pub item_type: String,
}

/// Helper to get max of two OffsetDateTime values
pub fn max_dt(a: OffsetDateTime, b: OffsetDateTime) -> OffsetDateTime {
    if a > b { a } else { b }
}

/// Format OffsetDateTime to RFC3339 string
pub fn format_rfc3339(dt: OffsetDateTime) -> String {
    dt.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}
