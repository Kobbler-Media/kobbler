//! IPTV admin route handlers.
//!
//! Providers, channels, EPG, recordings, favourites, watch history,
//! playback proxy, series rules, groups, unmatched sources, and settings.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use tracing::{info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::images::{self, ImageType};
use crate::repositories::{
    iptv_channels, iptv_epg, iptv_favourites, iptv_groups, iptv_programme_watches, iptv_providers,
    iptv_recordings, iptv_series_rules, iptv_sources, iptv_unmatched, iptv_watch_history,
};
use crate::routes::{AuthUser, PlaybackAuth};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};

// =============================================================================
// Error Type
// =============================================================================

pub enum AppError {
    Forbidden,
    NotFound(String),
    BadRequest(String),
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            AppError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

/// Provider credentials and sync settings are admin-only; the rest of the IPTV
/// surface (guide, playback, favourites) is open to every authenticated user.
fn require_admin(user: &AuthUser) -> Result<(), AppError> {
    user.require_admin().map_err(|_| AppError::Forbidden)
}

/// Channels, groups, recordings and unmatched entries are shared state, so
/// changing them is curation work. Per-user surfaces (favourites, watch
/// history, programme watches) stay open to everyone.
fn require_contributor(user: &AuthUser) -> Result<(), AppError> {
    user.require_contributor().map_err(|_| AppError::Forbidden)
}

/// Build a local icon endpoint URL for a channel, or None if the channel has no icon.
fn channel_icon_url(id: Uuid, has_icon: bool) -> Option<String> {
    if has_icon {
        Some(format!("/api/v1/admin/iptv/channels/{id}/icon"))
    } else {
        None
    }
}

// =============================================================================
// Provider Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateProviderInput {
    pub name: String,
    pub url: String,
    pub username: String,
    pub password: String,
    pub preferred_format: Option<String>,
    pub sync_interval_secs: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProviderInput {
    pub name: Option<String>,
    pub url: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub preferred_format: Option<String>,
    pub sync_interval_secs: Option<i32>,
    pub enabled: Option<bool>,
    pub account_info: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct ProviderResponse {
    id: Uuid,
    name: String,
    url: String,
    username: String,
    preferred_format: String,
    sync_interval_secs: i32,
    enabled: bool,
    account_info: serde_json::Value,
    #[serde(with = "time::serde::rfc3339::option")]
    last_sync_at: Option<OffsetDateTime>,
    last_sync_error: Option<String>,
    consecutive_sync_failures: i32,
    #[serde(with = "time::serde::rfc3339::option")]
    next_sync_retry_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

impl From<iptv_providers::ProviderRow> for ProviderResponse {
    fn from(r: iptv_providers::ProviderRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            url: r.url,
            username: r.username,
            preferred_format: r.preferred_format,
            sync_interval_secs: r.sync_interval_secs,
            enabled: r.enabled,
            account_info: r.account_info,
            last_sync_at: r.last_sync_at,
            last_sync_error: r.last_sync_error,
            consecutive_sync_failures: r.consecutive_sync_failures,
            next_sync_retry_at: r.next_sync_retry_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

// =============================================================================
// Channel Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ChannelListParams {
    pub search: Option<String>,
    pub category: Option<String>,
    pub language: Option<String>,
    pub enabled: Option<bool>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateChannelInput {
    pub name: Option<String>,
    pub language: Option<String>,
    pub country: Option<String>,
    pub category: Option<String>,
    pub icon_url: Option<String>,
    pub epg_channel_id: Option<String>,
    pub enabled: Option<bool>,
    pub display_order: Option<i32>,
}

#[derive(Debug, Serialize)]
struct ChannelResponse {
    id: Uuid,
    name: String,
    normalized_name: String,
    external_id: Option<String>,
    language: Option<String>,
    country: Option<String>,
    category: Option<String>,
    icon_url: Option<String>,
    epg_channel_id: Option<String>,
    enabled: bool,
    display_order: i32,
    source_count: i64,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

impl From<iptv_channels::ChannelWithSourceCount> for ChannelResponse {
    fn from(r: iptv_channels::ChannelWithSourceCount) -> Self {
        let icon = channel_icon_url(r.id, r.icon_url.is_some());
        Self {
            id: r.id,
            name: r.name,
            normalized_name: r.normalized_name,
            external_id: r.external_id,
            language: r.language,
            country: r.country,
            category: r.category,
            icon_url: icon,
            epg_channel_id: r.epg_channel_id,
            enabled: r.enabled,
            display_order: r.display_order,
            source_count: r.source_count,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

#[derive(Debug, Serialize)]
struct ChannelDetailResponse {
    id: Uuid,
    name: String,
    normalized_name: String,
    external_id: Option<String>,
    language: Option<String>,
    country: Option<String>,
    category: Option<String>,
    icon_url: Option<String>,
    epg_channel_id: Option<String>,
    enabled: bool,
    display_order: i32,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

impl From<iptv_channels::ChannelRow> for ChannelDetailResponse {
    fn from(r: iptv_channels::ChannelRow) -> Self {
        let icon = channel_icon_url(r.id, r.icon_url.is_some());
        Self {
            id: r.id,
            name: r.name,
            normalized_name: r.normalized_name,
            external_id: r.external_id,
            language: r.language,
            country: r.country,
            category: r.category,
            icon_url: icon,
            epg_channel_id: r.epg_channel_id,
            enabled: r.enabled,
            display_order: r.display_order,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct MergeChannelsInput {
    pub source_id: Uuid,
    pub target_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct BulkEnableInput {
    pub channel_ids: Vec<Uuid>,
    pub enabled: bool,
}

// =============================================================================
// EPG Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct EpgTimeRange {
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub from: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub to: Option<OffsetDateTime>,
}

#[derive(Debug, Deserialize)]
pub struct EpgGridParams {
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub from: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub to: Option<OffsetDateTime>,
    pub enabled_only: Option<bool>,
    /// Comma-separated channel UUIDs to filter by. If omitted, defaults to first 50 enabled.
    pub channel_ids: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EpgSearchParams {
    pub query: String,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub from: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub to: Option<OffsetDateTime>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
struct EpgResponse {
    id: Uuid,
    channel_id: Uuid,
    title: Option<String>,
    description: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    start_time: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    stop_time: OffsetDateTime,
    categories: Vec<String>,
    episode_info: Option<serde_json::Value>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

impl From<iptv_epg::EpgRow> for EpgResponse {
    fn from(r: iptv_epg::EpgRow) -> Self {
        Self {
            id: r.id,
            channel_id: r.channel_id,
            title: r.title,
            description: r.description,
            start_time: r.start_time,
            stop_time: r.stop_time,
            categories: r.categories,
            episode_info: r.episode_info,
            created_at: r.created_at,
        }
    }
}

// =============================================================================
// Source Types
// =============================================================================

#[derive(Debug, Serialize)]
struct SourceResponse {
    id: Uuid,
    channel_id: Uuid,
    provider_id: Uuid,
    provider_name: Option<String>,
    remote_stream_id: i64,
    name: Option<String>,
    icon_url: Option<String>,
    stream_type: String,
    quality: String,
    priority: i32,
    health_score: i32,
    match_method: Option<String>,
    match_confidence: f32,
    remote_category_name: Option<String>,
}

impl SourceResponse {
    fn from_row(r: iptv_sources::SourceRow, provider_name: Option<String>) -> Self {
        Self {
            id: r.id,
            channel_id: r.channel_id,
            provider_id: r.provider_id,
            provider_name,
            remote_stream_id: r.remote_stream_id,
            name: r.name,
            icon_url: r.icon_url,
            stream_type: r.stream_type,
            quality: r.quality,
            priority: r.priority,
            health_score: (r.health_score * 100.0).round() as i32,
            match_method: r.match_method,
            match_confidence: r.match_confidence,
            remote_category_name: r.remote_category_name,
        }
    }
}

impl From<iptv_sources::SourceRow> for SourceResponse {
    fn from(r: iptv_sources::SourceRow) -> Self {
        Self::from_row(r, None)
    }
}

// =============================================================================
// Group Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateGroupInput {
    pub name: String,
    pub slug: String,
    pub icon: Option<String>,
    pub display_order: Option<i32>,
    pub auto_assign_categories: Option<Vec<String>>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateGroupInput {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub icon: Option<String>,
    pub display_order: Option<i32>,
    pub auto_assign_categories: Option<Vec<String>>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Serialize)]
struct GroupResponse {
    id: Uuid,
    name: String,
    slug: String,
    icon: Option<String>,
    display_order: i32,
    auto_assign_categories: Vec<String>,
    enabled: bool,
    channel_count: i64,
}

impl From<iptv_groups::GroupWithCount> for GroupResponse {
    fn from(r: iptv_groups::GroupWithCount) -> Self {
        Self {
            id: r.id,
            name: r.name,
            slug: r.slug,
            icon: r.icon,
            display_order: r.display_order,
            auto_assign_categories: r.auto_assign_categories,
            enabled: r.enabled,
            channel_count: r.channel_count,
        }
    }
}

// =============================================================================
// Unmatched Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct UnmatchedListParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct AssignUnmatchedInput {
    pub channel_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct CreateChannelFromUnmatchedInput {
    pub normalized_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct UnmatchedResponse {
    id: Uuid,
    provider_id: Uuid,
    remote_stream_id: i64,
    remote_category_name: Option<String>,
    name: Option<String>,
    icon_url: Option<String>,
    epg_channel_id: Option<String>,
    stream_type: String,
    language: Option<String>,
    suggested_channel_id: Option<Uuid>,
    suggested_confidence: Option<f32>,
    dismissed: bool,
}

impl From<iptv_unmatched::UnmatchedRow> for UnmatchedResponse {
    fn from(r: iptv_unmatched::UnmatchedRow) -> Self {
        Self {
            id: r.id,
            provider_id: r.provider_id,
            remote_stream_id: r.remote_stream_id,
            remote_category_name: r.remote_category_name,
            name: r.name,
            icon_url: r.icon_url,
            epg_channel_id: r.epg_channel_id,
            stream_type: r.stream_type,
            language: r.language,
            suggested_channel_id: r.suggested_channel_id,
            suggested_confidence: r.suggested_confidence,
            dismissed: r.dismissed,
        }
    }
}

// =============================================================================
// Recording Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateRecordingInput {
    pub channel_id: Uuid,
    pub title: String,
    #[serde(with = "time::serde::rfc3339")]
    pub start_time: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub end_time: OffsetDateTime,
    pub pre_padding_secs: Option<i32>,
    pub post_padding_secs: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct RecordFromEpgInput {
    pub epg_id: Uuid,
    pub pre_padding_secs: Option<i32>,
    pub post_padding_secs: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct RecordingListParams {
    pub status: Option<String>,
}

#[derive(Debug, Serialize)]
struct RecordingResponse {
    id: Uuid,
    channel_id: Uuid,
    epg_id: Option<Uuid>,
    title: String,
    status: String,
    #[serde(with = "time::serde::rfc3339")]
    start_time: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    end_time: OffsetDateTime,
    pre_padding_secs: i32,
    post_padding_secs: i32,
    file_path: Option<String>,
    file_size_bytes: Option<i64>,
    error_message: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    duration_seconds: Option<f64>,
}

impl From<iptv_recordings::RecordingRow> for RecordingResponse {
    fn from(r: iptv_recordings::RecordingRow) -> Self {
        Self {
            id: r.id,
            channel_id: r.channel_id,
            epg_id: r.epg_id,
            title: r.title,
            status: r.status,
            start_time: r.start_time,
            end_time: r.end_time,
            pre_padding_secs: r.pre_padding_secs,
            post_padding_secs: r.post_padding_secs,
            file_path: r.file_path,
            file_size_bytes: r.file_size_bytes,
            error_message: r.error_message,
            created_at: r.created_at,
            duration_seconds: r.duration_seconds,
        }
    }
}

// =============================================================================
// Series Rule Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateSeriesRuleInput {
    pub title_pattern: String,
    pub channel_id: Option<Uuid>,
    pub record_new_only: Option<bool>,
    pub keep_count: Option<i32>,
    pub pre_padding_secs: Option<i32>,
    pub post_padding_secs: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSeriesRuleInput {
    pub title_pattern: Option<String>,
    pub channel_id: Option<Uuid>,
    pub record_new_only: Option<bool>,
    pub keep_count: Option<i32>,
    pub pre_padding_secs: Option<i32>,
    pub post_padding_secs: Option<i32>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Serialize)]
struct SeriesRuleResponse {
    id: Uuid,
    title_pattern: String,
    channel_id: Option<Uuid>,
    record_new_only: bool,
    keep_count: Option<i32>,
    pre_padding_secs: i32,
    post_padding_secs: i32,
    enabled: bool,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

impl From<iptv_series_rules::SeriesRuleRow> for SeriesRuleResponse {
    fn from(r: iptv_series_rules::SeriesRuleRow) -> Self {
        Self {
            id: r.id,
            title_pattern: r.title_pattern,
            channel_id: r.channel_id,
            record_new_only: r.record_new_only,
            keep_count: r.keep_count,
            pre_padding_secs: r.pre_padding_secs,
            post_padding_secs: r.post_padding_secs,
            enabled: r.enabled,
            created_at: r.created_at,
        }
    }
}

// =============================================================================
// Programme Watch Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateWatchInput {
    title_pattern: String,
    match_mode: Option<String>,
}

#[derive(Debug, Serialize)]
struct WatchResponse {
    id: Uuid,
    title_pattern: String,
    match_mode: String,
    enabled: bool,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

impl From<iptv_programme_watches::WatchRow> for WatchResponse {
    fn from(r: iptv_programme_watches::WatchRow) -> Self {
        Self {
            id: r.id,
            title_pattern: r.title_pattern,
            match_mode: r.match_mode,
            enabled: r.enabled,
            created_at: r.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
struct WatchMatchResponse {
    watch_id: Uuid,
    channel_id: Uuid,
    channel_name: String,
    channel_icon_url: Option<String>,
    epg_title: String,
    #[serde(with = "time::serde::rfc3339")]
    start_time: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    stop_time: OffsetDateTime,
    is_live: bool,
}

impl From<iptv_programme_watches::WatchMatch> for WatchMatchResponse {
    fn from(m: iptv_programme_watches::WatchMatch) -> Self {
        let icon = channel_icon_url(m.channel_id, m.channel_icon_url.is_some());
        Self {
            watch_id: m.watch_id,
            channel_id: m.channel_id,
            channel_name: m.channel_name,
            channel_icon_url: icon,
            epg_title: m.epg_title,
            start_time: m.start_time,
            stop_time: m.stop_time,
            is_live: m.is_live,
        }
    }
}

// =============================================================================
// Favourite Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct AddFavouriteInput {
    pub channel_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct ReorderFavouritesInput {
    pub channel_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
struct FavouriteResponse {
    channel_id: Uuid,
    display_order: i32,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

impl From<iptv_favourites::FavouriteRow> for FavouriteResponse {
    fn from(r: iptv_favourites::FavouriteRow) -> Self {
        Self {
            channel_id: r.channel_id,
            display_order: r.display_order,
            created_at: r.created_at,
        }
    }
}

// =============================================================================
// Watch History Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct RecordWatchInput {
    pub channel_id: Uuid,
    pub duration_secs: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct WatchHistoryParams {
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
struct WatchHistoryResponse {
    channel_id: Uuid,
    watch_count: i32,
    #[serde(with = "time::serde::rfc3339")]
    last_watched_at: OffsetDateTime,
    duration_secs: i32,
}

impl From<iptv_watch_history::WatchHistoryRow> for WatchHistoryResponse {
    fn from(r: iptv_watch_history::WatchHistoryRow) -> Self {
        Self {
            channel_id: r.channel_id,
            watch_count: r.watch_count,
            last_watched_at: r.last_watched_at,
            duration_secs: r.duration_secs,
        }
    }
}

// =============================================================================
// Settings Types
// =============================================================================

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct IptvSettings {
    pub default_format: Option<String>,
    pub recording_path: Option<String>,
    pub epg_update_interval_secs: Option<i32>,
    pub channel_cleanup_enabled: Option<bool>,
    pub epg_retention_days: Option<i32>,
    // Runtime config fields
    pub language_filter: Option<Vec<String>>,
    pub auto_create_channels: Option<bool>,
    pub health_check_enabled: Option<bool>,
}

// =============================================================================
// Provider Handlers
// =============================================================================

pub async fn list_providers(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    require_admin(&user)?;

    let rows = iptv_providers::list_all(&state.pool)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let providers: Vec<ProviderResponse> = rows.into_iter().map(Into::into).collect();
    Ok(Json(providers))
}

pub async fn create_provider(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<CreateProviderInput>,
) -> Result<impl IntoResponse, AppError> {
    require_admin(&user)?;

    let row = iptv_providers::create(
        &state.pool,
        iptv_providers::CreateProvider {
            name: input.name,
            url: input.url,
            username: input.username,
            password: input.password,
            preferred_format: input.preferred_format,
            sync_interval_secs: input.sync_interval_secs,
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    let resp: ProviderResponse = row.into();
    Ok((StatusCode::CREATED, Json(resp)))
}

pub async fn get_provider(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    require_admin(&user)?;

    let row = iptv_providers::get_by_id(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("Provider {id} not found")))?;

    let resp: ProviderResponse = row.into();
    Ok(Json(resp))
}

pub async fn update_provider(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateProviderInput>,
) -> Result<impl IntoResponse, AppError> {
    require_admin(&user)?;

    let updated = iptv_providers::update(
        &state.pool,
        id,
        iptv_providers::UpdateProvider {
            name: input.name,
            url: input.url,
            username: input.username,
            password: input.password,
            preferred_format: input.preferred_format,
            sync_interval_secs: input.sync_interval_secs,
            enabled: input.enabled,
            account_info: input.account_info,
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    if !updated {
        return Err(AppError::NotFound(format!("Provider {id} not found")));
    }

    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_provider(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    require_admin(&user)?;

    let deleted = iptv_providers::delete(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    if !deleted {
        return Err(AppError::NotFound(format!("Provider {id} not found")));
    }

    // Clean up channels that no longer have any sources (orphaned by cascade delete)
    match iptv_channels::delete_orphaned(&state.pool).await {
        Ok(count) if count > 0 => {
            info!(
                orphaned_channels = count,
                "cleaned up orphaned channels after provider deletion"
            );
        }
        Err(e) => {
            warn!(error = %e, "failed to clean up orphaned channels");
        }
        _ => {}
    }

    Ok(StatusCode::NO_CONTENT)
}

pub async fn sync_provider(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    require_admin(&user)?;

    let provider = iptv_providers::get_by_id(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("Provider {id} not found")))?;

    let context = crate::jobs::JobContext::for_entity(
        "iptv_provider",
        id,
        format!("Syncing IPTV provider: {}", provider.name),
    );
    match crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "iptv_sync",
        json!({"provider_id": id}),
        context,
    )
    .await
    {
        Ok(job_id) => Ok((StatusCode::ACCEPTED, Json(json!({ "job_id": job_id })))),
        Err(e) => {
            warn!(error = %e, provider_id = %id, "Failed to enqueue iptv_sync job");
            Err(AppError::Internal("Failed to enqueue sync job".to_string()))
        }
    }
}

// =============================================================================
// Channel Handlers
// =============================================================================

pub async fn list_channels(
    State(state): State<AppState>,
    _user: AuthUser,
    Query(params): Query<ChannelListParams>,
) -> Result<impl IntoResponse, AppError> {
    let mut rows = iptv_channels::list_with_source_counts(&state.pool)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Apply language filter from runtime config unless ?language=all
    let config = state.runtime_config.get().await;
    if params.language.as_deref() != Some("all") && !config.iptv_language_filter.is_empty() {
        let filter = &config.iptv_language_filter;
        rows.retain(|r| {
            r.language
                .as_ref()
                .is_none_or(|l| filter.iter().any(|f| f == l))
        });
    }

    // Apply category/enabled filters in-memory
    if let Some(ref category) = params.category {
        rows.retain(|r| r.category.as_deref() == Some(category.as_str()));
    }
    if let Some(enabled) = params.enabled {
        rows.retain(|r| r.enabled == enabled);
    }

    // Use MeiliSearch for text search (fuzzy, typo-tolerant)
    if let Some(ref search) = params.search {
        let languages = if params.language.as_deref() != Some("all")
            && !config.iptv_language_filter.is_empty()
        {
            Some(config.iptv_language_filter.clone())
        } else {
            None
        };
        let filters = crate::search::IptvChannelSearchFilters {
            category: params.category.clone(),
            enabled: params.enabled,
            languages,
            ..Default::default()
        };
        let limit = params.limit.unwrap_or(100) as usize;
        let offset = params.offset.unwrap_or(0) as usize;
        match state
            .search
            .search_iptv_channels(search, &filters, limit + offset, 0)
            .await
        {
            Ok(ids) => {
                // Build relevance-ordered lookup from MeiliSearch results
                let id_order: std::collections::HashMap<String, usize> = ids
                    .iter()
                    .enumerate()
                    .map(|(i, id)| (id.clone(), i))
                    .collect();
                rows.retain(|r| id_order.contains_key(&r.id.to_string()));
                // Sort by MeiliSearch relevance order
                rows.sort_by_key(|r| {
                    id_order
                        .get(&r.id.to_string())
                        .copied()
                        .unwrap_or(usize::MAX)
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "MeiliSearch channel search failed, falling back to substring");
                let q = search.to_lowercase();
                rows.retain(|r| r.name.to_lowercase().contains(&q));
            }
        }
    }

    let total = rows.len();
    let offset = params.offset.unwrap_or(0) as usize;
    let limit = params.limit.unwrap_or(100) as usize;
    let page: Vec<ChannelResponse> = rows
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(Into::into)
        .collect();

    Ok(Json(json!({ "items": page, "total": total })))
}

pub async fn get_channel(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let row = iptv_channels::get_by_id(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("Channel {id} not found")))?;

    let resp: ChannelDetailResponse = row.into();
    Ok(Json(resp))
}

pub async fn update_channel(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateChannelInput>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let normalized_name = input
        .name
        .as_ref()
        .map(|n| crate::iptv::normalization::normalize_stream_name(n).normalized_name);

    let updated = iptv_channels::update(
        &state.pool,
        id,
        iptv_channels::UpdateChannel {
            name: input.name,
            normalized_name,
            external_id: None,
            language: input.language,
            country: input.country.map(|c| {
                let code = crate::iptv::normalization::normalize_country_code(&c);
                if code == "XX" { c } else { code.to_string() }
            }),
            category: input.category,
            icon_url: input.icon_url,
            epg_channel_id: input.epg_channel_id,
            enabled: input.enabled,
            display_order: input.display_order,
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    if !updated {
        return Err(AppError::NotFound(format!("Channel {id} not found")));
    }

    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_channel_icon(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let storage_root = &state.config.storage_root;
    let image_type = ImageType::IptvChannelIcon;

    // Serve from disk cache if available
    if images::cached_image_exists(storage_root, image_type, id) {
        let bytes = images::load_cached_image(storage_root, image_type, id)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let ct = images::detect_content_type(&bytes);
        return Ok((
            [(CONTENT_TYPE, ct), (CACHE_CONTROL, "public, max-age=86400")],
            bytes,
        )
            .into_response());
    }

    // Lazy-fetch from the channel's remote icon_url
    let channel = iptv_channels::get_by_id(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("Channel {id} not found")))?;

    let icon_url = channel
        .icon_url
        .as_deref()
        .ok_or_else(|| AppError::NotFound("No icon for this channel".to_string()))?;

    images::cache_image(storage_root, image_type, id, icon_url)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to fetch icon: {e}")))?;

    let bytes = images::load_cached_image(storage_root, image_type, id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let ct = images::detect_content_type(&bytes);
    Ok((
        [(CONTENT_TYPE, ct), (CACHE_CONTROL, "public, max-age=86400")],
        bytes,
    )
        .into_response())
}

pub async fn get_channel_epg(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(id): Path<Uuid>,
    Query(params): Query<EpgTimeRange>,
) -> Result<impl IntoResponse, AppError> {
    let now = OffsetDateTime::now_utc();
    let from = params.from.unwrap_or(now);
    let to = params.to.unwrap_or_else(|| now + time::Duration::hours(24));

    let rows = iptv_epg::list_by_channel_time_range(&state.pool, id, from, to)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let items: Vec<EpgResponse> = rows.into_iter().map(Into::into).collect();
    Ok(Json(items))
}

pub async fn get_channel_sources(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let rows = iptv_sources::list_by_channel(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Batch-lookup provider names
    let provider_ids: Vec<Uuid> = rows.iter().map(|r| r.provider_id).collect();
    let providers = iptv_providers::list_all(&state.pool)
        .await
        .unwrap_or_default();
    let provider_names: std::collections::HashMap<Uuid, String> =
        providers.into_iter().map(|p| (p.id, p.name)).collect();

    let items: Vec<SourceResponse> = rows
        .into_iter()
        .map(|r| {
            let pname = provider_names.get(&r.provider_id).cloned();
            SourceResponse::from_row(r, pname)
        })
        .collect();
    let _ = provider_ids; // used for batch lookup above
    Ok(Json(items))
}

pub async fn merge_channels(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<MergeChannelsInput>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    if input.source_id == input.target_id {
        return Err(AppError::BadRequest(
            "Source and target channels must be different".to_string(),
        ));
    }

    let merged = iptv_channels::merge(&state.pool, input.source_id, input.target_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    if !merged {
        return Err(AppError::NotFound(
            "Source or target channel not found".to_string(),
        ));
    }

    Ok(StatusCode::NO_CONTENT)
}

pub async fn bulk_enable_channels(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<BulkEnableInput>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let mut count = 0u64;
    for channel_id in &input.channel_ids {
        if iptv_channels::set_enabled(&state.pool, *channel_id, input.enabled)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?
        {
            count += 1;
        }
    }

    Ok(Json(json!({ "updated": count })))
}

pub async fn cleanup_channels(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let context = crate::jobs::JobContext::new("Cleaning up orphaned IPTV channels");
    match crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "iptv_channel_cleanup",
        json!({}),
        context,
    )
    .await
    {
        Ok(job_id) => Ok((StatusCode::ACCEPTED, Json(json!({ "job_id": job_id })))),
        Err(e) => {
            warn!(error = %e, "Failed to enqueue iptv_channel_cleanup job");
            Err(AppError::Internal(
                "Failed to enqueue cleanup job".to_string(),
            ))
        }
    }
}

pub async fn rematch_channels(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let context =
        crate::jobs::JobContext::new("Re-matching IPTV channels against provider streams");
    match crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "iptv_rematch",
        json!({}),
        context,
    )
    .await
    {
        Ok(job_id) => Ok((StatusCode::ACCEPTED, Json(json!({ "job_id": job_id })))),
        Err(e) => {
            warn!(error = %e, "Failed to enqueue iptv_rematch job");
            Err(AppError::Internal(
                "Failed to enqueue rematch job".to_string(),
            ))
        }
    }
}

// =============================================================================
// Group Handlers
// =============================================================================

pub async fn list_groups(
    State(state): State<AppState>,
    _user: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let rows = iptv_groups::list_all_with_counts(&state.pool)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let groups: Vec<GroupResponse> = rows.into_iter().map(Into::into).collect();
    Ok(Json(groups))
}

pub async fn create_group(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<CreateGroupInput>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let row = iptv_groups::create(
        &state.pool,
        iptv_groups::CreateGroup {
            name: input.name,
            slug: input.slug,
            icon: input.icon,
            display_order: input.display_order,
            auto_assign_categories: input.auto_assign_categories.unwrap_or_default(),
            enabled: input.enabled,
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": row.id,
            "name": row.name,
            "slug": row.slug,
            "icon": row.icon,
            "display_order": row.display_order,
            "auto_assign_categories": row.auto_assign_categories,
            "enabled": row.enabled,
        })),
    ))
}

pub async fn update_group(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateGroupInput>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let updated = iptv_groups::update(
        &state.pool,
        id,
        iptv_groups::UpdateGroup {
            name: input.name,
            slug: input.slug,
            icon: input.icon,
            display_order: input.display_order,
            auto_assign_categories: input.auto_assign_categories,
            enabled: input.enabled,
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    if !updated {
        return Err(AppError::NotFound(format!("Group {id} not found")));
    }

    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_group(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let deleted = iptv_groups::delete(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    if !deleted {
        return Err(AppError::NotFound(format!("Group {id} not found")));
    }

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Unmatched Handlers
// =============================================================================

pub async fn list_unmatched(
    State(state): State<AppState>,
    _user: AuthUser,
    Query(params): Query<UnmatchedListParams>,
) -> Result<impl IntoResponse, AppError> {
    let limit = params.limit.unwrap_or(100);
    let offset = params.offset.unwrap_or(0);

    let count = iptv_unmatched::count_pending(&state.pool)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let rows = iptv_unmatched::list_pending(&state.pool, limit, offset)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let items: Vec<UnmatchedResponse> = rows.into_iter().map(Into::into).collect();
    Ok(Json(json!({ "items": items, "total": count })))
}

pub async fn assign_unmatched(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(input): Json<AssignUnmatchedInput>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let source_id = iptv_unmatched::assign_to_channel(&state.pool, id, input.channel_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok((StatusCode::CREATED, Json(json!({ "source_id": source_id }))))
}

pub async fn create_channel_from_unmatched(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(input): Json<CreateChannelFromUnmatchedInput>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    // If no normalized name provided, we'll use the unmatched source's name
    let normalized_name = input.normalized_name.unwrap_or_default();
    let norm = if normalized_name.is_empty() {
        // Fetch unmatched to get its name for normalization
        let rows = iptv_unmatched::list_pending(&state.pool, 1000, 0)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let unmatched = rows
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| AppError::NotFound(format!("Unmatched source {id} not found")))?;
        crate::iptv::normalization::normalize_stream_name(
            unmatched.name.as_deref().unwrap_or("unknown"),
        )
        .normalized_name
    } else {
        normalized_name
    };

    let (channel_id, source_id) = iptv_unmatched::create_channel_from(&state.pool, id, &norm)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(json!({ "channel_id": channel_id, "source_id": source_id })),
    ))
}

pub async fn dismiss_unmatched(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let dismissed = iptv_unmatched::dismiss(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    if !dismissed {
        return Err(AppError::NotFound(format!(
            "Unmatched source {id} not found"
        )));
    }

    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_unmatched(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    // Dismiss is effectively a soft-delete; for hard delete we dismiss
    let dismissed = iptv_unmatched::dismiss(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    if !dismissed {
        return Err(AppError::NotFound(format!(
            "Unmatched source {id} not found"
        )));
    }

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Playback Handlers
// =============================================================================

pub async fn play_channel(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let http = crate::http::new_client();
    // Default to HLS format; provider preferred_format could override
    let format = "m3u8";

    match crate::iptv::proxy::proxy_channel(&state.pool, &http, id, format).await {
        Ok(response) => Ok(response.into_response()),
        Err(e) => Err(AppError::Internal(format!("Playback error: {e}"))),
    }
}

pub async fn play_segment(
    State(state): State<AppState>,
    _user: AuthUser,
    Path((id, idx)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse, AppError> {
    // Touch the live session to keep it alive during active playback.
    // The channel_id is stored as file_id on the PlaybackSession.
    for mut entry in state.playback_sessions.iter_mut() {
        let session = entry.value_mut();
        if session.is_live && session.file_id == id {
            session.touch();
            break;
        }
    }

    let segment_idx: u32 = idx
        .parse()
        .map_err(|_| AppError::BadRequest("Invalid segment index".to_string()))?;

    let url = crate::iptv::proxy::get_cached_segment_url(id, segment_idx)
        .ok_or_else(|| AppError::NotFound("Segment not found or expired".to_string()))?;

    let http = crate::http::new_client();
    match crate::iptv::proxy::proxy_segment(&http, &url).await {
        Ok(response) => Ok(response.into_response()),
        Err(e) => Err(AppError::Internal(format!("Segment proxy error: {e}"))),
    }
}

pub async fn play_ts(
    State(state): State<AppState>,
    _auth: PlaybackAuth,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    // Try to consume a pre-opened stream from session creation (single-connection path)
    if let Some((_, live_stream)) = state.live_streams.remove(&id) {
        let content_type = live_stream.content_type;
        let byte_stream = live_stream
            .byte_stream
            .into_inner()
            .expect("mutex not poisoned");
        // Chain the buffered probe head + remaining byte stream into one body
        let probe_bytes = bytes::Bytes::from(live_stream.probe_buf);
        let head = futures_util::stream::once(async move { Ok::<_, std::io::Error>(probe_bytes) });
        let tail =
            futures_util::StreamExt::map(byte_stream, |chunk| chunk.map_err(std::io::Error::other));
        let chained = futures_util::StreamExt::chain(head, tail);
        let body = axum::body::Body::from_stream(chained);

        return Ok(axum::http::Response::builder()
            .header("content-type", content_type)
            .body(body)
            .expect("valid response")
            .into_response());
    }

    // Fallback: open a new connection (for session reuse or if stream was already consumed)
    let http = crate::http::new_client();
    match crate::iptv::proxy::proxy_channel(&state.pool, &http, id, "ts").await {
        Ok(response) => Ok(response.into_response()),
        Err(e) => Err(AppError::Internal(format!("Playback error: {e}"))),
    }
}

// =============================================================================
// EPG Handlers
// =============================================================================

pub async fn get_epg_grid(
    State(state): State<AppState>,
    _user: AuthUser,
    Query(params): Query<EpgGridParams>,
) -> Result<impl IntoResponse, AppError> {
    let now = OffsetDateTime::now_utc();
    let from = params.from.unwrap_or(now);
    let to = params.to.unwrap_or_else(|| now + time::Duration::hours(6));
    let rfc3339 = time::format_description::well_known::Rfc3339;
    let from_str = from.format(&rfc3339).unwrap_or_default();
    let to_str = to.format(&rfc3339).unwrap_or_default();

    // Determine which channels to include
    let channel_list = if let Some(ref ids_str) = params.channel_ids {
        // Parse comma-separated UUIDs
        let requested_ids: Vec<Uuid> = ids_str
            .split(',')
            .filter_map(|s| s.trim().parse::<Uuid>().ok())
            .collect();
        if requested_ids.is_empty() {
            return Ok(Json(json!({ "grid": [], "from": from_str, "to": to_str })));
        }

        // Fetch only the requested channels — SQL filter, not full table scan
        iptv_channels::list_by_ids_with_source_counts(&state.pool, &requested_ids)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?
    } else {
        // Default: first 50 enabled channels (not ALL)
        let channels = iptv_channels::list_with_source_counts(&state.pool)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;

        let enabled_only = params.enabled_only.unwrap_or(true);
        channels
            .into_iter()
            .filter(|c| !enabled_only || c.enabled)
            .take(50)
            .collect::<Vec<_>>()
    };

    if channel_list.is_empty() {
        return Ok(Json(json!({ "grid": [], "from": from_str, "to": to_str })));
    }

    // Single batch query instead of N+1
    let channel_ids: Vec<Uuid> = channel_list.iter().map(|c| c.id).collect();
    let all_epg = iptv_epg::list_by_channel_ids_time_range(&state.pool, &channel_ids, from, to)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Group EPG rows by channel_id
    let mut epg_by_channel: std::collections::HashMap<Uuid, Vec<EpgResponse>> =
        std::collections::HashMap::new();
    for row in all_epg {
        epg_by_channel
            .entry(row.channel_id)
            .or_default()
            .push(row.into());
    }

    // Get catchup info for all channels in one query
    let catchup_info = iptv_sources::get_catchup_channels(&state.pool, &channel_ids)
        .await
        .unwrap_or_default();

    // Build grid maintaining channel order
    let grid: Vec<serde_json::Value> = channel_list
        .iter()
        .map(|ch| {
            let programs = epg_by_channel.remove(&ch.id).unwrap_or_default();
            let catchup_days = catchup_info.get(&ch.id).copied();
            json!({
                "channel": {
                    "id": ch.id,
                    "name": ch.name,
                    "icon_url": channel_icon_url(ch.id, ch.icon_url.is_some()),
                    "category": ch.category,
                    "has_catchup": catchup_days.is_some(),
                    "catchup_days": catchup_days,
                },
                "programs": programs,
            })
        })
        .collect();

    Ok(Json(
        json!({ "grid": grid, "from": from_str, "to": to_str }),
    ))
}

pub async fn search_epg(
    State(state): State<AppState>,
    _user: AuthUser,
    Query(params): Query<EpgSearchParams>,
) -> Result<impl IntoResponse, AppError> {
    let limit = params.limit.unwrap_or(50) as usize;

    let mut filters = crate::search::IptvEpgSearchFilters::default();

    // Apply time range filters if provided
    if let Some(from) = params.from {
        filters.start_after = Some(from.unix_timestamp());
    }
    if let Some(to) = params.to {
        filters.start_before = Some(to.unix_timestamp());
    }

    let results = state
        .search
        .search_iptv_epg(&params.query, &filters, limit, 0)
        .await
        .map_err(|e| AppError::Internal(format!("EPG search failed: {e}")))?;

    let entries: Vec<serde_json::Value> = results
        .into_iter()
        .map(|doc| {
            json!({
                "id": doc.id,
                "channel_id": doc.channel_id,
                "channel_name": doc.channel_name,
                "title": doc.title,
                "description": doc.description,
                "categories": doc.categories,
                "start_time": doc.start_time_iso,
                "stop_time": doc.stop_time_iso,
            })
        })
        .collect();

    Ok(Json(serde_json::Value::Array(entries)))
}

pub async fn get_epg_now(
    State(state): State<AppState>,
    _user: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let now = OffsetDateTime::now_utc();

    let channels = iptv_channels::list_with_source_counts(&state.pool)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let mut items = Vec::with_capacity(channels.len());
    for ch in channels.iter().filter(|c| c.enabled) {
        let current = iptv_epg::get_current_program(&state.pool, ch.id, now)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;

        items.push(json!({
            "channel": {
                "id": ch.id,
                "name": ch.name,
                "icon_url": channel_icon_url(ch.id, ch.icon_url.is_some()),
                "category": ch.category,
            },
            "current_program": current.map(EpgResponse::from),
        }));
    }

    Ok(Json(json!({ "items": items })))
}

pub async fn get_epg_favourites(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<EpgGridParams>,
) -> Result<impl IntoResponse, AppError> {
    let now = OffsetDateTime::now_utc();
    let from = params.from.unwrap_or(now);
    let to = params.to.unwrap_or_else(|| now + time::Duration::hours(6));
    let rfc3339 = time::format_description::well_known::Rfc3339;
    let from_str = from.format(&rfc3339).unwrap_or_default();
    let to_str = to.format(&rfc3339).unwrap_or_default();

    // Get favourite channel IDs for this user
    let fav_rows = iptv_favourites::list_for_user(&state.pool, user.user_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    if fav_rows.is_empty() {
        return Ok(Json(json!({ "grid": [], "from": from_str, "to": to_str })));
    }

    let fav_channel_ids: Vec<Uuid> = fav_rows.iter().map(|f| f.channel_id).collect();

    // Look up channel metadata — SQL filter by IDs, not full table scan
    let all_channels = iptv_channels::list_by_ids_with_source_counts(&state.pool, &fav_channel_ids)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let channel_map: std::collections::HashMap<Uuid, _> =
        all_channels.into_iter().map(|c| (c.id, c)).collect();

    // Batch query EPG for all favourite channels
    let all_epg = iptv_epg::list_by_channel_ids_time_range(&state.pool, &fav_channel_ids, from, to)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let mut epg_by_channel: std::collections::HashMap<Uuid, Vec<EpgResponse>> =
        std::collections::HashMap::new();
    for row in all_epg {
        epg_by_channel
            .entry(row.channel_id)
            .or_default()
            .push(row.into());
    }

    // Get catchup info for all favourite channels
    let catchup_info = iptv_sources::get_catchup_channels(&state.pool, &fav_channel_ids)
        .await
        .unwrap_or_default();

    // Build grid in favourite order
    let grid: Vec<serde_json::Value> = fav_channel_ids
        .iter()
        .filter_map(|cid| {
            let ch = channel_map.get(cid)?;
            let programs = epg_by_channel.remove(cid).unwrap_or_default();
            let catchup_days = catchup_info.get(cid).copied();
            Some(json!({
                "channel": {
                    "id": ch.id,
                    "name": ch.name,
                    "icon_url": channel_icon_url(ch.id, ch.icon_url.is_some()),
                    "category": ch.category,
                    "has_catchup": catchup_days.is_some(),
                    "catchup_days": catchup_days,
                },
                "programs": programs,
            }))
        })
        .collect();

    Ok(Json(
        json!({ "grid": grid, "from": from_str, "to": to_str }),
    ))
}

// =============================================================================
// Recording Handlers
// =============================================================================

pub async fn list_recordings(
    State(state): State<AppState>,
    _user: AuthUser,
    Query(params): Query<RecordingListParams>,
) -> Result<impl IntoResponse, AppError> {
    let rows = iptv_recordings::list_with_filters(&state.pool, params.status.as_deref())
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let items: Vec<RecordingResponse> = rows.into_iter().map(Into::into).collect();
    Ok(Json(items))
}

pub async fn create_recording(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<CreateRecordingInput>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let row = iptv_recordings::create(
        &state.pool,
        iptv_recordings::CreateRecording {
            channel_id: input.channel_id,
            epg_id: None,
            title: input.title,
            start_time: input.start_time,
            end_time: input.end_time,
            pre_padding_secs: input.pre_padding_secs,
            post_padding_secs: input.post_padding_secs,
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    let resp: RecordingResponse = row.into();
    Ok((StatusCode::CREATED, Json(resp)))
}

pub async fn record_from_epg(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<RecordFromEpgInput>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    // Look up the EPG entry to get channel, title, and time range
    // We need to find the EPG entry by ID across all channels
    let now = OffsetDateTime::now_utc();
    let future = now + time::Duration::days(14);

    // Get all enabled channels and search for the EPG entry
    let channels = iptv_channels::list_with_source_counts(&state.pool)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let mut epg_entry = None;
    for ch in &channels {
        let entries = iptv_epg::list_by_channel_time_range(&state.pool, ch.id, now, future)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;

        if let Some(entry) = entries.into_iter().find(|e| e.id == input.epg_id) {
            epg_entry = Some(entry);
            break;
        }
    }

    let entry = epg_entry
        .ok_or_else(|| AppError::NotFound(format!("EPG entry {} not found", input.epg_id)))?;

    let row = iptv_recordings::create(
        &state.pool,
        iptv_recordings::CreateRecording {
            channel_id: entry.channel_id,
            epg_id: Some(input.epg_id),
            title: entry.title.unwrap_or_default(),
            start_time: entry.start_time,
            end_time: entry.stop_time,
            pre_padding_secs: input.pre_padding_secs,
            post_padding_secs: input.post_padding_secs,
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    let resp: RecordingResponse = row.into();
    Ok((StatusCode::CREATED, Json(resp)))
}

pub async fn delete_recording(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    // Fetch recording to get file path before deleting
    let recording = iptv_recordings::get_by_id(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let deleted = iptv_recordings::delete(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    if !deleted {
        return Err(AppError::NotFound(format!("Recording {id} not found")));
    }

    // Best-effort cleanup of recording file and thumbnails
    if let Some(rec) = recording
        && let Some(file_path) = rec.file_path
        && let Err(e) = tokio::fs::remove_file(&file_path).await
    {
        tracing::warn!(error = %e, path = %file_path, "Failed to delete recording file");
    }
    crate::repositories::cleanup_thumbnail_dir(&state.config.storage_root, id).await;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn stream_recording(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let recording = iptv_recordings::get_by_id(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("Recording {id} not found")))?;

    let file_path = recording
        .file_path
        .ok_or_else(|| AppError::BadRequest("Recording has no file yet".to_string()))?;

    // Stream the recording file
    let file = tokio::fs::File::open(&file_path)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to open recording file: {e}")))?;

    let stream = tokio_util::io::ReaderStream::new(file);
    let body = axum::body::Body::from_stream(stream);

    Ok(axum::response::Response::builder()
        .header("content-type", "video/mp4")
        .header(
            "content-disposition",
            format!(
                "inline; filename=\"{}\"",
                std::path::Path::new(&file_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("recording.ts")
            ),
        )
        .body(body)
        .expect("valid response")
        .into_response())
}

// =============================================================================
// Series Rule Handlers
// =============================================================================

pub async fn list_series_rules(
    State(state): State<AppState>,
    _user: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let rows = iptv_series_rules::list_all(&state.pool)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let items: Vec<SeriesRuleResponse> = rows.into_iter().map(Into::into).collect();
    Ok(Json(items))
}

pub async fn create_series_rule(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<CreateSeriesRuleInput>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let row = iptv_series_rules::create(
        &state.pool,
        iptv_series_rules::CreateSeriesRule {
            title_pattern: input.title_pattern,
            channel_id: input.channel_id,
            record_new_only: input.record_new_only,
            keep_count: input.keep_count,
            pre_padding_secs: input.pre_padding_secs,
            post_padding_secs: input.post_padding_secs,
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    let resp: SeriesRuleResponse = row.into();
    Ok((StatusCode::CREATED, Json(resp)))
}

pub async fn update_series_rule(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateSeriesRuleInput>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let updated = iptv_series_rules::update(
        &state.pool,
        id,
        iptv_series_rules::UpdateSeriesRule {
            title_pattern: input.title_pattern,
            channel_id: input.channel_id,
            record_new_only: input.record_new_only,
            keep_count: input.keep_count,
            pre_padding_secs: input.pre_padding_secs,
            post_padding_secs: input.post_padding_secs,
            enabled: input.enabled,
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    if !updated {
        return Err(AppError::NotFound(format!("Series rule {id} not found")));
    }

    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_series_rule(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let deleted = iptv_series_rules::delete(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    if !deleted {
        return Err(AppError::NotFound(format!("Series rule {id} not found")));
    }

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Catchup Handlers
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct CatchupInput {
    pub channel_id: Uuid,
    pub epg_id: Uuid,
}

/// Known timeshift URL format templates.
const TIMESHIFT_FORMATS: &[&str] = &[
    // Format A: B1G-style with date
    "{base}/timeshift/{user}/{pass}/{duration}/{date}/{stream_id}.ts",
    // Format B: simple duration+stream
    "{base}/timeshift/{user}/{pass}/{duration}/{stream_id}.ts",
    // Format C: streaming PHP endpoint
    "{base}/streaming/timeshift.php?username={user}&password={pass}&stream={stream_id}&start={date}&duration={duration}",
];

fn build_timeshift_url(
    format_template: &str,
    base_url: &str,
    username: &str,
    password: &str,
    duration_minutes: i64,
    start_time: OffsetDateTime,
    stream_id: i64,
) -> String {
    let date_str = format!(
        "{}-{:02}-{:02}:{:02}-{:02}",
        start_time.year(),
        start_time.month() as u8,
        start_time.day(),
        start_time.hour(),
        start_time.minute(),
    );
    format_template
        .replace("{base}", base_url.trim_end_matches('/'))
        .replace("{user}", username)
        .replace("{pass}", password)
        .replace("{duration}", &duration_minutes.to_string())
        .replace("{date}", &date_str)
        .replace("{stream_id}", &stream_id.to_string())
}

pub async fn catchup(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<CatchupInput>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    // Look up EPG entry
    let now = OffsetDateTime::now_utc();
    let past = now - time::Duration::days(14);
    let epg_entries =
        iptv_epg::list_by_channel_time_range(&state.pool, input.channel_id, past, now)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;

    let entry = epg_entries
        .into_iter()
        .find(|e| e.id == input.epg_id)
        .ok_or_else(|| AppError::NotFound("EPG entry not found".into()))?;

    let channel_id = entry.channel_id;
    let start_time = entry.start_time;
    let stop_time = entry.stop_time;

    let duration_secs = (stop_time - start_time).whole_seconds();
    let duration_minutes = (duration_secs as u64).div_ceil(60) as i64;

    // Find a source with tv_archive support for this channel
    let sources = iptv_sources::list_by_channel(&state.pool, channel_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let archive_source = sources.iter().find(|s| {
        s.metadata
            .get("tv_archive")
            .and_then(|v| v.as_str())
            .is_some_and(|v| v == "1")
    });

    let source = archive_source
        .ok_or_else(|| AppError::BadRequest("No catchup-capable source for this channel".into()))?;

    // Get provider credentials
    let provider = iptv_providers::get_by_id(&state.pool, source.provider_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::Internal("Provider not found".into()))?;

    // Check if we have a cached format for this provider
    let cached_format: Option<String> = provider
        .account_info
        .get("timeshift_format")
        .and_then(|v| v.as_str())
        .map(String::from);

    let http = crate::http::new_client();

    // Helper to build a session response from a catchup URL
    let make_session = |url: String| -> IptvSessionResponse {
        IptvSessionResponse {
            session_id: Uuid::now_v7().to_string(),
            method: crate::streaming::decision::PlaybackMethod::DirectPlay,
            playlist_url: None,
            direct_url: Some(url),
            duration_seconds: duration_secs as f64,
            subtitles: vec![],
            audio_tracks: vec![],
        }
    };

    if let Some(ref fmt) = cached_format {
        let url = build_timeshift_url(
            fmt,
            &provider.url,
            &provider.username,
            &provider.password,
            duration_minutes,
            start_time,
            source.remote_stream_id,
        );
        return Ok(Json(make_session(url)).into_response());
    }

    // Auto-detect: try each format with a HEAD request
    for fmt in TIMESHIFT_FORMATS {
        let url = build_timeshift_url(
            fmt,
            &provider.url,
            &provider.username,
            &provider.password,
            duration_minutes,
            start_time,
            source.remote_stream_id,
        );

        // Try HEAD first; some providers reject HEAD so fall back to GET with Range
        let is_ok_status =
            |r: &reqwest::Response| r.status().is_success() || r.status().as_u16() == 302;

        let head_ok = http
            .head(&url)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .is_ok_and(|r| is_ok_status(&r));

        let probe_ok = head_ok || {
            // HEAD rejected — try GET with a tiny range
            http.get(&url)
                .header("Range", "bytes=0-1")
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
                .is_ok_and(|r| is_ok_status(&r) || r.status().as_u16() == 206)
        };

        if probe_ok {
            // Cache the working format
            let mut account_info = provider.account_info.clone();
            if let Some(obj) = account_info.as_object_mut() {
                obj.insert("timeshift_format".into(), json!(fmt));
            }
            let _ = iptv_providers::update(
                &state.pool,
                provider.id,
                iptv_providers::UpdateProvider {
                    name: None,
                    url: None,
                    username: None,
                    password: None,
                    preferred_format: None,
                    sync_interval_secs: None,
                    enabled: None,
                    account_info: Some(account_info),
                },
            )
            .await;

            info!(
                provider = %provider.name,
                format = fmt,
                "auto-detected timeshift URL format"
            );
            return Ok(Json(make_session(url)).into_response());
        }
    }

    Err(AppError::Internal(
        "Could not find a working timeshift URL format for this provider".into(),
    ))
}

// =============================================================================
// Favourite Handlers
// =============================================================================

pub async fn list_favourites(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let rows = iptv_favourites::list_for_user(&state.pool, user.user_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let items: Vec<FavouriteResponse> = rows.into_iter().map(Into::into).collect();
    Ok(Json(items))
}

pub async fn add_favourite(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<AddFavouriteInput>,
) -> Result<impl IntoResponse, AppError> {
    iptv_favourites::add(&state.pool, user.user_id, input.channel_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_favourite(
    State(state): State<AppState>,
    user: AuthUser,
    Path(channel_id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let removed = iptv_favourites::remove(&state.pool, user.user_id, channel_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    if !removed {
        return Err(AppError::NotFound("Favourite not found".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}

pub async fn reorder_favourites(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<ReorderFavouritesInput>,
) -> Result<impl IntoResponse, AppError> {
    for (order, channel_id) in input.channel_ids.iter().enumerate() {
        iptv_favourites::reorder(&state.pool, user.user_id, *channel_id, order as i32)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
    }

    Ok(StatusCode::NO_CONTENT)
}

pub async fn add_favourite_group(
    State(state): State<AppState>,
    user: AuthUser,
    Path(group_id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    iptv_favourites::add_group(&state.pool, user.user_id, group_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_favourite_group(
    State(state): State<AppState>,
    user: AuthUser,
    Path(group_id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    iptv_favourites::remove_group(&state.pool, user.user_id, group_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_favourite_groups(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let group_ids = iptv_favourites::list_favourite_group_ids(&state.pool, user.user_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(group_ids))
}

// =============================================================================
// Watch History Handlers
// =============================================================================

pub async fn list_history(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<WatchHistoryParams>,
) -> Result<impl IntoResponse, AppError> {
    let limit = params.limit.unwrap_or(50);
    let rows = iptv_watch_history::list_recent(&state.pool, user.user_id, limit)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let items: Vec<WatchHistoryResponse> = rows.into_iter().map(Into::into).collect();
    Ok(Json(items))
}

pub async fn record_watch(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<RecordWatchInput>,
) -> Result<impl IntoResponse, AppError> {
    let duration = input.duration_secs.unwrap_or(0);
    iptv_watch_history::record_watch(&state.pool, user.user_id, input.channel_id, duration)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Programme Watch Handlers
// =============================================================================

pub async fn list_watches(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let rows = iptv_programme_watches::list_for_user(&state.pool, user.user_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let items: Vec<WatchResponse> = rows.into_iter().map(Into::into).collect();
    Ok(Json(items))
}

pub async fn create_watch(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<CreateWatchInput>,
) -> Result<impl IntoResponse, AppError> {
    let mode = input.match_mode.as_deref().unwrap_or("contains");
    if !matches!(mode, "contains" | "exact" | "regex") {
        return Err(AppError::BadRequest(format!(
            "Invalid match_mode: {mode}. Must be 'contains', 'exact', or 'regex'"
        )));
    }

    let id = iptv_programme_watches::create(&state.pool, user.user_id, &input.title_pattern, mode)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

pub async fn delete_watch(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let deleted = iptv_programme_watches::delete(&state.pool, id, user.user_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    if !deleted {
        return Err(AppError::NotFound(format!("Watch {id} not found")));
    }

    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_watch_matches(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let matches = iptv_programme_watches::find_matches(&state.pool, user.user_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Deduplicate: same programme on similar channels → pick best match
    // Group by (normalized_title, start_time rounded to 30min)
    let favourites = iptv_favourites::list_for_user(&state.pool, user.user_id)
        .await
        .unwrap_or_default();
    let fav_order: std::collections::HashMap<Uuid, usize> = favourites
        .iter()
        .enumerate()
        .map(|(i, f)| (f.channel_id, i))
        .collect();

    let mut grouped: std::collections::HashMap<(String, i64), Vec<WatchMatchResponse>> =
        std::collections::HashMap::new();

    for m in matches {
        let norm_title = m.epg_title.to_lowercase();
        // Round start_time to nearest 30-minute slot
        let slot = m.start_time.unix_timestamp() / 1800;
        let resp: WatchMatchResponse = m.into();
        grouped.entry((norm_title, slot)).or_default().push(resp);
    }

    let mut items: Vec<WatchMatchResponse> = grouped
        .into_values()
        .map(|mut group| {
            // Sort: favourites first (by order), then alphabetically
            group.sort_by(|a, b| {
                let a_fav = fav_order.get(&a.channel_id).copied().unwrap_or(usize::MAX);
                let b_fav = fav_order.get(&b.channel_id).copied().unwrap_or(usize::MAX);
                a_fav.cmp(&b_fav)
            });
            group.into_iter().next().expect("group is non-empty")
        })
        .collect();

    // Sort final results by start_time
    items.sort_by_key(|m| m.start_time);

    Ok(Json(items))
}

// =============================================================================
// Settings Handlers
// =============================================================================

/// Get IPTV settings.
///
/// Reads IPTV-specific fields from `iptv_config` (singleton row).
/// Returns defaults if no config row exists yet.
pub async fn get_settings(
    State(state): State<AppState>,
    _user: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let row = sqlx::query!(
        r#"
        SELECT default_format, recording_path, epg_update_interval_secs,
               channel_cleanup_enabled, epg_retention_days
        FROM iptv_config
        LIMIT 1
        "#,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    let config = state.runtime_config.get().await;

    let settings = match row {
        Some(r) => IptvSettings {
            default_format: r.default_format,
            recording_path: r.recording_path,
            epg_update_interval_secs: r.epg_update_interval_secs,
            channel_cleanup_enabled: r.channel_cleanup_enabled,
            epg_retention_days: Some(r.epg_retention_days),
            language_filter: Some(config.iptv_language_filter),
            auto_create_channels: Some(config.iptv_auto_create_channels),
            health_check_enabled: Some(config.iptv_health_check_enabled),
        },
        None => IptvSettings {
            epg_retention_days: Some(7),
            language_filter: Some(config.iptv_language_filter),
            auto_create_channels: Some(config.iptv_auto_create_channels),
            health_check_enabled: Some(config.iptv_health_check_enabled),
            ..IptvSettings::default()
        },
    };

    Ok(Json(settings))
}

/// Update IPTV settings (upsert singleton config row + runtime config).
pub async fn update_settings(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<IptvSettings>,
) -> Result<impl IntoResponse, AppError> {
    require_admin(&user)?;

    // Update runtime config fields if provided
    let mut rc_update = crate::runtime_config::RuntimeConfigUpdate::default();
    if let Some(v) = input.language_filter {
        rc_update.iptv_language_filter = Some(v);
    }
    if let Some(v) = input.auto_create_channels {
        rc_update.iptv_auto_create_channels = Some(v);
    }
    if let Some(v) = input.health_check_enabled {
        rc_update.iptv_health_check_enabled = Some(v);
    }
    state
        .runtime_config
        .update(rc_update)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Update iptv_config table for recording-specific settings
    sqlx::query!(
        r#"
        INSERT INTO iptv_config (id, default_format, recording_path,
                                  epg_update_interval_secs, channel_cleanup_enabled,
                                  epg_retention_days)
        VALUES (1, $1, $2, $3, $4, COALESCE($5, 7))
        ON CONFLICT (id)
        DO UPDATE SET
            default_format = COALESCE($1, iptv_config.default_format),
            recording_path = COALESCE($2, iptv_config.recording_path),
            epg_update_interval_secs = COALESCE($3, iptv_config.epg_update_interval_secs),
            channel_cleanup_enabled = COALESCE($4, iptv_config.channel_cleanup_enabled),
            epg_retention_days = COALESCE($5, iptv_config.epg_retention_days),
            updated_at = NOW()
        "#,
        input.default_format,
        input.recording_path,
        input.epg_update_interval_secs,
        input.channel_cleanup_enabled,
        input.epg_retention_days,
    )
    .execute(&state.pool)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Live Playback Sessions
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateIptvSessionInput {
    pub channel_id: Uuid,
    pub source_id: Option<Uuid>,
    pub capabilities: Option<crate::streaming::decision::ClientCapabilities>,
}

#[derive(Debug, Serialize)]
pub struct IptvSessionResponse {
    pub session_id: String,
    pub method: crate::streaming::decision::PlaybackMethod,
    pub playlist_url: Option<String>,
    pub direct_url: Option<String>,
    pub duration_seconds: f64,
    pub subtitles: Vec<()>,
    pub audio_tracks: Vec<crate::routes::admin::streaming::AudioTrackResponse>,
}

pub async fn create_iptv_session(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<CreateIptvSessionInput>,
) -> Result<impl IntoResponse, AppError> {
    let capabilities = input.capabilities.unwrap_or_default();

    let result = crate::iptv::live_session::create_live_session(
        &state.pool,
        &state,
        input.channel_id,
        user.user_id,
        capabilities,
        input.source_id,
    )
    .await
    .map_err(|e| AppError::Internal(format!("Failed to create IPTV session: {e}")))?;

    let audio_tracks = result
        .audio_tracks
        .iter()
        .map(|a| crate::routes::admin::streaming::AudioTrackResponse {
            stream_index: a.stream_index,
            codec: a.codec.clone(),
            channels: a.channels,
            language: a.language.clone(),
            title: a.title.clone(),
            is_default: a.is_default,
        })
        .collect();

    Ok(Json(IptvSessionResponse {
        session_id: result.session_id.to_string(),
        method: result.method,
        playlist_url: result.playlist_url,
        direct_url: result.direct_url,
        duration_seconds: 0.0,
        subtitles: vec![],
        audio_tracks,
    })
    .into_response())
}
