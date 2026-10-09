//! Admin routes for media root management.

use crate::AppState;
use crate::jobs::JobContext;
use crate::media_root_resolve::{self, ResolveContext};
use crate::repositories::{deferred_ingests, job_status, media_root_files, media_roots};
use crate::routes::AuthUser;
use crate::search;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Deserializer, Serialize};

/// Deserialize a year field that may arrive as a number or a string.
fn deserialize_year_flexible<'de, D>(deserializer: D) -> Result<Option<i32>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum YearValue {
        Num(i32),
        Str(String),
        Null,
    }

    match Option::<YearValue>::deserialize(deserializer)? {
        None | Some(YearValue::Null) => Ok(None),
        Some(YearValue::Num(n)) => Ok(Some(n)),
        Some(YearValue::Str(s)) => {
            if s.is_empty() {
                Ok(None)
            } else {
                s.parse::<i32>().map(Some).map_err(de::Error::custom)
            }
        }
    }
}
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

// ============================================================================
// Types
// ============================================================================

#[derive(Deserialize)]
pub struct CreateMediaRootRequest {
    pub path: String,
    #[serde(default)]
    pub name: String,
    pub media_type: String,
    #[serde(default = "default_true")]
    pub auto_add: bool,
    pub naming_template: Option<String>,
    pub default_entitlement_scope: Option<String>,
    #[serde(default = "default_true")]
    pub write_sidecars: bool,
    #[serde(default = "default_true")]
    pub write_images: bool,
    #[serde(default)]
    pub is_default: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
pub struct UpdateMediaRootRequest {
    pub path: Option<String>,
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub auto_add: Option<bool>,
    pub naming_template: Option<Option<String>>,
    pub default_entitlement_scope: Option<String>,
    pub write_sidecars: Option<bool>,
    pub write_images: Option<bool>,
    pub is_default: Option<bool>,
    pub max_concurrent_io: Option<i32>,
    /// Per-root scan interval override.
    ///
    /// Sentinel semantics (since serde's `Option<Option<T>>` can't reliably
    /// distinguish `null` from missing for partial updates):
    ///   - field omitted → don't touch
    ///   - `0`            → clear override (fall back to global default)
    ///   - positive int   → set override
    pub scan_interval_seconds: Option<i32>,
}

#[derive(Serialize)]
pub struct MediaRootResponse {
    pub id: Uuid,
    pub path: String,
    pub name: String,
    pub media_type: String,
    pub enabled: bool,
    pub auto_add: bool,
    pub scan_status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_scan_at: Option<OffsetDateTime>,
    pub scan_error: Option<String>,
    pub error_count: i32,
    pub naming_template: Option<String>,
    pub default_entitlement_scope: String,
    pub write_sidecars: bool,
    pub write_images: bool,
    pub is_default: bool,
    pub max_concurrent_io: i32,
    pub scan_interval_seconds: Option<i32>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_scan_completed_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<media_roots::MediaRootStats>,
    pub deferred_ingest_count: i64,
}

impl From<media_roots::MediaRootRow> for MediaRootResponse {
    fn from(r: media_roots::MediaRootRow) -> Self {
        Self {
            id: r.id,
            path: r.path,
            name: r.name,
            media_type: r.media_type,
            enabled: r.enabled,
            auto_add: r.auto_add,
            scan_status: r.scan_status,
            last_scan_at: r.last_scan_at,
            scan_error: r.scan_error,
            error_count: r.error_count,
            naming_template: r.naming_template,
            default_entitlement_scope: r.default_entitlement_scope,
            write_sidecars: r.write_sidecars,
            write_images: r.write_images,
            is_default: r.is_default,
            max_concurrent_io: r.max_concurrent_io,
            scan_interval_seconds: r.scan_interval_seconds,
            last_scan_completed_at: r.last_scan_completed_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
            stats: None,
            deferred_ingest_count: 0,
        }
    }
}

#[derive(Serialize)]
pub struct MediaRootFileResponse {
    pub id: Uuid,
    pub media_root_id: Uuid,
    pub file_path: String,
    pub file_name: String,
    pub file_size: Option<i64>,
    pub match_status: String,
    pub entity_type: Option<String>,
    pub entity_id: Option<Uuid>,
    pub interpretation: Option<serde_json::Value>,
    pub match_candidates: Option<serde_json::Value>,
    pub sidecar_metadata: Option<serde_json::Value>,
    pub available: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

impl From<media_root_files::MediaRootFileRow> for MediaRootFileResponse {
    fn from(f: media_root_files::MediaRootFileRow) -> Self {
        Self {
            id: f.id,
            media_root_id: f.media_root_id,
            file_path: f.file_path,
            file_name: f.file_name,
            file_size: f.file_size,
            match_status: f.match_status,
            entity_type: f.entity_type,
            entity_id: f.entity_id,
            interpretation: f.interpretation,
            match_candidates: f.match_candidates,
            sidecar_metadata: f.sidecar_metadata,
            available: f.available,
            created_at: f.created_at,
        }
    }
}

#[derive(Serialize)]
pub struct MediaRootFileLiteResponse {
    pub id: Uuid,
    pub media_root_id: Uuid,
    pub file_path: String,
    pub file_name: String,
    pub file_size: Option<i64>,
    pub match_status: String,
    pub entity_type: Option<String>,
    pub entity_id: Option<Uuid>,
    pub interpretation: Option<serde_json::Value>,
    pub available: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

impl From<media_root_files::MediaRootFileLiteRow> for MediaRootFileLiteResponse {
    fn from(f: media_root_files::MediaRootFileLiteRow) -> Self {
        Self {
            id: f.id,
            media_root_id: f.media_root_id,
            file_path: f.file_path,
            file_name: f.file_name,
            file_size: f.file_size,
            match_status: f.match_status,
            entity_type: f.entity_type,
            entity_id: f.entity_id,
            interpretation: f.interpretation,
            available: f.available,
            created_at: f.created_at,
        }
    }
}

#[derive(Deserialize)]
pub struct FileListQuery {
    pub status: Option<String>,
    pub group_key: Option<String>,
    pub season: Option<i32>,
    pub directory: Option<String>,
    #[serde(default)]
    pub lite: bool,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    100
}

#[derive(Serialize)]
pub struct FileListResponse {
    pub items: Vec<MediaRootFileResponse>,
    pub total_count: i64,
}

#[derive(Serialize)]
pub struct FileListLiteResponse {
    pub items: Vec<MediaRootFileLiteResponse>,
    pub total_count: i64,
}

#[derive(Serialize)]
pub struct EntityGroupsResponse {
    pub groups: Vec<media_root_files::EntityGroupSummary>,
    pub ungrouped_count: i64,
}

#[derive(Serialize)]
pub struct StatusCountsResponse {
    pub counts: std::collections::HashMap<String, i64>,
}

#[derive(Deserialize)]
pub struct MatchResolveRequest {
    pub provider: String,
    pub provider_id: String,
    pub title: String,
    #[serde(default, deserialize_with = "deserialize_year_flexible")]
    pub year: Option<i32>,
}

#[derive(Deserialize)]
pub struct BulkResolveRequest {
    pub file_ids: Vec<Uuid>,
}

#[derive(Deserialize)]
pub struct BulkManualResolveRequest {
    pub file_ids: Vec<Uuid>,
    pub provider: String,
    pub provider_id: String,
    pub title: String,
    #[serde(default, deserialize_with = "deserialize_year_flexible")]
    pub year: Option<i32>,
}

#[derive(Deserialize)]
pub struct LinkToEntityRequest {
    pub entity_type: String,
    pub entity_id: Uuid,
}

#[derive(Deserialize)]
pub struct BulkLinkToEntityRequest {
    pub file_ids: Vec<Uuid>,
    pub entity_type: String,
    pub entity_id: Uuid,
}

#[derive(Deserialize)]
pub struct DirectoryActionRequest {
    pub directory: String,
    pub action: String,
}

// ============================================================================
// CRUD
// ============================================================================

/// List all media roots.
pub async fn list_media_roots(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    // Contributors need this for the destination-root picker when adding or
    // moving titles; per-root detail and file browsing stay admin-only.
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    match media_roots::list_all(&state.pool).await {
        Ok(roots) => {
            let mut responses = Vec::new();
            for root in roots {
                let stats = media_roots::get_stats(&state.pool, root.id).await.ok();
                let deferred = deferred_ingests::count_for_media_root(&state.pool, root.id)
                    .await
                    .unwrap_or(0);
                let mut resp = MediaRootResponse::from(root);
                resp.stats = stats;
                resp.deferred_ingest_count = deferred;
                responses.push(resp);
            }
            Json(responses).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Create a new media root.
pub async fn create_media_root(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<CreateMediaRootRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    // Auto-set as default if this is the first root for this type or explicitly requested
    let is_default = if body.is_default {
        true
    } else {
        !media_roots::has_default_for_type(&state.pool, &body.media_type)
            .await
            .unwrap_or(true)
    };

    let input = media_roots::MediaRootInput {
        path: body.path,
        name: body.name,
        media_type: body.media_type,
        auto_add: body.auto_add,
        naming_template: body.naming_template,
        default_entitlement_scope: body.default_entitlement_scope,
        write_sidecars: body.write_sidecars,
        write_images: body.write_images,
        is_default,
    };

    match media_roots::create(&state.pool, input).await {
        Ok(id) => {
            // Fire-and-forget: enqueue an initial scan so the user doesn't wait for the 5-min check
            let root = media_roots::get_by_id(&state.pool, id).await.ok().flatten();
            if let Some(r) = &root {
                let display = crate::jobs::fetch_job_display(&state.pool, "media_root", id).await;
                let context = JobContext::for_entity(
                    "media_root",
                    id,
                    format!("Initial scan of '{}'", r.name),
                )
                .with_display_opt(display);
                if let Err(e) = crate::jobs::enqueue(
                    &state.pool,
                    state.nats.publisher(),
                    "media_root_scan",
                    json!({"media_root_id": id}),
                    context,
                )
                .await
                {
                    tracing::warn!(error = %e, media_root_id = %id, "Failed to enqueue initial scan");
                }
            }

            match root {
                Some(r) => (StatusCode::CREATED, Json(MediaRootResponse::from(r))).into_response(),
                None => (StatusCode::CREATED, Json(json!({"id": id}))).into_response(),
            }
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

/// Get a media root by ID.
pub async fn get_media_root(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    match media_roots::get_by_id(&state.pool, id).await {
        Ok(Some(root)) => {
            let stats = media_roots::get_stats(&state.pool, id).await.ok();
            let deferred = deferred_ingests::count_for_media_root(&state.pool, id)
                .await
                .unwrap_or(0);
            let mut resp = MediaRootResponse::from(root);
            resp.stats = stats;
            resp.deferred_ingest_count = deferred;
            Json(resp).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Update a media root.
pub async fn update_media_root(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateMediaRootRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let io_cap_changed = body.max_concurrent_io.is_some();
    let input = media_roots::MediaRootUpdate {
        path: body.path,
        name: body.name,
        enabled: body.enabled,
        auto_add: body.auto_add,
        naming_template: body.naming_template,
        default_entitlement_scope: body.default_entitlement_scope,
        write_sidecars: body.write_sidecars,
        write_images: body.write_images,
        is_default: body.is_default,
        max_concurrent_io: body.max_concurrent_io,
        scan_interval_seconds: body.scan_interval_seconds.map(|v| (v > 0).then_some(v)),
    };

    match media_roots::update(&state.pool, id, input).await {
        Ok(true) => {
            if io_cap_changed {
                state.media_root_io_gate.reload(id).await;
            }
            let root = media_roots::get_by_id(&state.pool, id).await.ok().flatten();
            match root {
                Some(r) => Json(MediaRootResponse::from(r)).into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

/// Delete a media root and all associated library items via background job.
pub async fn delete_media_root(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let root = match media_roots::get_by_id(&state.pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Cancel pending jobs for this media root before deletion
    if let Err(e) = job_status::cancel_by_entity(&state.pool, "media_root", id).await {
        tracing::warn!(error = %e, "Failed to cancel pending jobs for media root");
    }

    let display = crate::jobs::fetch_job_display(&state.pool, "media_root", id).await;
    let context = JobContext::for_entity(
        "media_root",
        id,
        format!("Deleting media root '{}' and library items", root.name),
    )
    .with_display_opt(display);

    match crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "media_root_delete",
        json!({"media_root_id": id}),
        context,
    )
    .await
    {
        Ok(job_id) => Json(json!({"job_id": job_id})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ============================================================================
// Set Default
// ============================================================================

/// Set a media root as the default for its type.
pub async fn set_default_media_root(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    match media_roots::set_default(&state.pool, id).await {
        Ok(true) => {
            let root = media_roots::get_by_id(&state.pool, id).await.ok().flatten();
            match root {
                Some(r) => Json(MediaRootResponse::from(r)).into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

// ============================================================================
// Scan
// ============================================================================

/// Trigger a manual scan.
pub async fn trigger_scan(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let root = match media_roots::get_by_id(&state.pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let id_str = id.to_string();

    // Clear any backoff/error state so the scan runs immediately
    if let Err(e) = media_roots::clear_error(&state.pool, id).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    // Cancel any pending scans for this root (both full and single-file).
    // Use media_root_id as the required_field — it's always present, so this
    // matches all pending scan jobs for this root.
    let _ = crate::jobs::cancel_pending_jobs_with_field(
        &state.pool,
        "media_root_scan",
        "media_root_id",
        &id_str,
        "media_root_id",
    )
    .await;

    let display = crate::jobs::fetch_job_display(&state.pool, "media_root", id).await;
    let context = JobContext::for_entity(
        "media_root",
        id,
        format!("Scanning media root '{}'", root.name),
    )
    .with_display_opt(display);

    match crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "media_root_scan",
        json!({"media_root_id": id}),
        context,
    )
    .await
    {
        Ok(job_id) => Json(json!({"job_id": job_id})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ============================================================================
// Files
// ============================================================================

/// List files for a media root.
///
/// Supports optional filters: `status`, `group_key` (entity group), `season` (TV).
/// Pass `lite=true` to omit match_candidates and sidecar_metadata from the response.
pub async fn list_files(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Query(query): Query<FileListQuery>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let limit = query.limit.clamp(1, 5000);
    let offset = query.offset.max(0);
    let status_filter = query.status.as_deref();

    // Group-filtered queries need the media root's media_type.
    if let Some(ref group_key) = query.group_key {
        let root = match media_roots::get_by_id(&state.pool, id).await {
            Ok(Some(r)) => r,
            Ok(None) => return StatusCode::NOT_FOUND.into_response(),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };

        let total_count = if offset == 0 {
            match media_root_files::count_for_group(
                &state.pool,
                id,
                &root.media_type,
                group_key,
                query.season,
                status_filter,
                &root.path,
            )
            .await
            {
                Ok(c) => c,
                Err(e) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
                }
            }
        } else {
            -1
        };

        // Group queries always use lite projection (the full row is available via get_by_id).
        match media_root_files::list_for_group_lite(
            &state.pool,
            id,
            &root.media_type,
            group_key,
            query.season,
            status_filter,
            limit,
            offset,
            &root.path,
        )
        .await
        {
            Ok(files) => {
                let items: Vec<MediaRootFileLiteResponse> =
                    files.into_iter().map(|f| f.into()).collect();
                Json(FileListLiteResponse { items, total_count }).into_response()
            }
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    } else if query.lite {
        // Lite projection for root-level listing.
        let total_count = if offset == 0 {
            match media_root_files::count_for_root(&state.pool, id, status_filter).await {
                Ok(c) => c,
                Err(e) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
                }
            }
        } else {
            -1
        };

        // Directory filtering needs root_path to match absolute file_path values.
        let root_path = if query.directory.is_some() {
            match media_roots::get_by_id(&state.pool, id).await {
                Ok(Some(r)) => r.path,
                Ok(None) => return StatusCode::NOT_FOUND.into_response(),
                Err(e) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
                }
            }
        } else {
            String::new()
        };

        match media_root_files::list_for_root_lite(
            &state.pool,
            id,
            status_filter,
            query.directory.as_deref(),
            &root_path,
            limit,
            offset,
        )
        .await
        {
            Ok(files) => {
                let items: Vec<MediaRootFileLiteResponse> =
                    files.into_iter().map(|f| f.into()).collect();
                Json(FileListLiteResponse { items, total_count }).into_response()
            }
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    } else {
        // Full projection (original behavior).
        let total_count = if offset == 0 {
            match media_root_files::count_for_root(&state.pool, id, status_filter).await {
                Ok(c) => c,
                Err(e) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
                }
            }
        } else {
            -1
        };

        match media_root_files::list_for_root(&state.pool, id, status_filter, limit, offset).await {
            Ok(files) => {
                let items: Vec<MediaRootFileResponse> =
                    files.into_iter().map(|f| f.into()).collect();
                Json(FileListResponse { items, total_count }).into_response()
            }
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// List directories for a media root (for lazy-loading directory tree).
pub async fn list_directories(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Query(query): Query<DirectoryListQuery>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let root = match media_roots::get_by_id(&state.pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    match media_root_files::list_directories(&state.pool, id, query.parent.as_deref(), &root.path)
        .await
    {
        Ok(dirs) => Json(dirs).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
pub struct DirectoryListQuery {
    pub parent: Option<String>,
}

/// List entity groups for a media root (server-side grouping).
pub async fn list_entity_groups(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let root = match media_roots::get_by_id(&state.pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let groups =
        match media_root_files::list_entity_groups(&state.pool, id, &root.media_type, &root.path)
            .await
        {
            Ok(g) => g,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };

    let ungrouped_count = match media_root_files::count_ungrouped(&state.pool, id).await {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    Json(EntityGroupsResponse {
        groups,
        ungrouped_count,
    })
    .into_response()
}

/// Get status counts for a media root's files.
pub async fn list_status_counts(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    match media_root_files::count_by_status(&state.pool, id).await {
        Ok(pairs) => {
            let counts: std::collections::HashMap<String, i64> = pairs.into_iter().collect();
            Json(StatusCountsResponse { counts }).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ============================================================================
// File search
// ============================================================================

#[derive(Deserialize)]
pub struct FileSearchQuery {
    pub q: String,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_search_limit() -> usize {
    20
}

/// Search files within a specific media root via MeiliSearch.
pub async fn search_files(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Query(query): Query<FileSearchQuery>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let limit = query.limit.clamp(1, 200);
    match state
        .search
        .search_media_root_files(&query.q, id, limit, query.offset)
        .await
    {
        Ok(docs) => {
            let responses: Vec<MediaRootFileSearchResponse> =
                docs.into_iter().map(|d| d.into()).collect();
            Json(responses).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Serialize)]
pub struct MediaRootFileSearchResponse {
    pub id: String,
    pub file_name: String,
    pub file_path: String,
    pub match_status: String,
    pub entity_type: Option<String>,
    pub show_name: Option<String>,
    pub movie_title: Option<String>,
    pub book_title: Option<String>,
    pub series_name: Option<String>,
}

impl From<search::MediaRootFileDocument> for MediaRootFileSearchResponse {
    fn from(d: search::MediaRootFileDocument) -> Self {
        Self {
            id: d.id,
            file_name: d.file_name,
            file_path: d.file_path,
            match_status: d.match_status,
            entity_type: d.entity_type,
            show_name: d.show_name,
            movie_title: d.movie_title,
            book_title: d.book_title,
            series_name: d.series_name,
        }
    }
}

// ============================================================================
// Match resolution
// ============================================================================

/// Get a single media root file by ID (full projection with match_candidates).
pub async fn get_file(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(file_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    match media_root_files::get_by_id(&state.pool, file_id).await {
        Ok(Some(f)) => Json(MediaRootFileResponse::from(f)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Resolve an ambiguous/review match (user picks a candidate).
///
/// For ambiguous files: confirms identity. If auto_add is enabled on the media
/// root, also adds to library. Otherwise moves to review status.
/// For review files: updates the selected candidate, stays in review.
pub async fn resolve_match(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(file_id): Path<Uuid>,
    Json(body): Json<MatchResolveRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let file = match media_root_files::get_by_id(&state.pool, file_id).await {
        Ok(Some(f)) => f,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let root = match media_roots::get_by_id(&state.pool, file.media_root_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Find the matching candidate from stored match_candidates
    let candidate = find_candidate(&file, &body).unwrap_or_else(|| {
        // Fall back to building a candidate from the request body
        json!({
            "provider": body.provider,
            "provider_id": body.provider_id,
            "title": body.title,
            "year": body.year,
        })
    });

    let should_add =
        (file.match_status == "unmatched" || file.match_status == "ambiguous") && root.auto_add;

    if should_add {
        // User explicitly picked a candidate and auto_add is on: resolve directly into library
        let ctx = ResolveContext {
            pool: &state.pool,
            nats: state.nats.publisher(),
            search: state.search.as_ref(),
        };
        match media_root_resolve::resolve_file(&ctx, &root, &file, file_id, &candidate).await {
            Ok(outcome) => Json(json!({
                "status": "resolved",
                "entity_type": outcome.entity_type,
                "entity_id": outcome.entity_id,
            }))
            .into_response(),
            Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
        }
    } else {
        // Confirm identity only: move to review with chosen candidate first
        let confirmed = media_root_files::build_confirmed_candidates(&file, &candidate);
        if let Err(e) = media_root_files::set_review(&state.pool, file_id, &confirmed).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
        let _ = search::index_media_root_file(state.search.as_ref(), &state.pool, file_id).await;
        Json(json!({"status": "confirmed"})).into_response()
    }
}

/// Add a review file to the library using its top stored candidate.
pub async fn add_to_library(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(file_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let file = match media_root_files::get_by_id(&state.pool, file_id).await {
        Ok(Some(f)) => f,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    if file.match_status != "review" {
        return (
            StatusCode::BAD_REQUEST,
            format!("File is '{}', must be 'review'", file.match_status),
        )
            .into_response();
    }

    let candidates: Vec<serde_json::Value> = file
        .match_candidates
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let Some(candidate) = candidates.into_iter().next() else {
        return (StatusCode::BAD_REQUEST, "No stored candidates").into_response();
    };

    let root = match media_roots::get_by_id(&state.pool, file.media_root_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let ctx = ResolveContext {
        pool: &state.pool,
        nats: state.nats.publisher(),
        search: state.search.as_ref(),
    };

    match media_root_resolve::resolve_file(&ctx, &root, &file, file_id, &candidate).await {
        Ok(outcome) => Json(json!({
            "status": "resolved",
            "entity_type": outcome.entity_type,
            "entity_id": outcome.entity_id,
        }))
        .into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

/// Find the candidate in match_candidates matching the user's selection.
fn find_candidate(
    file: &media_root_files::MediaRootFileRow,
    body: &MatchResolveRequest,
) -> Option<serde_json::Value> {
    let candidates: Vec<serde_json::Value> = file
        .match_candidates
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    candidates.into_iter().find(|c| {
        c["provider"].as_str() == Some(&body.provider)
            && (c["provider_id"].as_str() == Some(&body.provider_id)
                || c["provider_id"].as_u64().map(|n| n.to_string()).as_deref()
                    == Some(&body.provider_id))
    })
}

/// Bulk-resolve files from their stored match candidates.
///
/// Enqueues a background job to process all files asynchronously.
/// Returns the job ID so the frontend can track progress via SSE.
pub async fn bulk_resolve(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<BulkResolveRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    if body.file_ids.is_empty() {
        return (StatusCode::BAD_REQUEST, "No file IDs provided").into_response();
    }

    let count = body.file_ids.len();
    let payload = json!({
        "file_ids": body.file_ids,
    });
    let display = crate::jobs::fetch_job_display(&state.pool, "media_root", Uuid::nil()).await;
    let context = JobContext::for_entity(
        "media_root",
        Uuid::nil(),
        format!(
            "Adding {} file{} to library",
            count,
            if count == 1 { "" } else { "s" }
        ),
    )
    .with_display_opt(display);

    match crate::jobs::enqueue_with_event(
        &state.pool,
        state.nats.publisher(),
        &state.job_events,
        "media_root_bulk_resolve",
        payload,
        context,
        false,
    )
    .await
    {
        Ok(job_id) => (StatusCode::ACCEPTED, Json(json!({"job_id": job_id}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Match review files to existing library entities.
///
/// Fetches all review-status files for the given media root, checks if their
/// first match candidate's provider_id corresponds to an entity already in
/// the library (tv_shows or movies by TMDB ID, books by identifiers), and
/// enqueues a bulk-resolve job for the matches.
pub async fn match_to_library(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(root_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }

    // Get the media root to know its type
    let root = match media_roots::get_by_id(&state.pool, root_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "Media root not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Fetch all review files (with match_candidates)
    let review_files =
        match media_root_files::list_for_root(&state.pool, root_id, Some("review"), 10000, 0).await
        {
            Ok(f) => f,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };

    if review_files.is_empty() {
        return (StatusCode::OK, Json(json!({"file_count": 0}))).into_response();
    }

    // Collect all provider_ids from first match candidate of each file
    let candidate_pids: Vec<(Uuid, String)> = review_files
        .iter()
        .filter_map(|f| {
            let candidate = f.match_candidates.as_ref()?.as_array()?.first()?;
            let pid = match candidate.get("provider_id")? {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                _ => return None,
            };
            Some((f.id, pid))
        })
        .collect();

    if candidate_pids.is_empty() {
        return (StatusCode::OK, Json(json!({"file_count": 0}))).into_response();
    }

    // Build set of library provider IDs based on media type
    let library_pids: std::collections::HashSet<String> = match root.media_type.as_str() {
        "tv" => {
            sqlx::query_scalar::<_, String>(
                "SELECT identifiers->>'tmdb_id' FROM tv_shows WHERE identifiers->>'tmdb_id' IS NOT NULL",
            )
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect()
        }
        "movie" => {
            sqlx::query_scalar::<_, String>(
                "SELECT identifiers->>'tmdb_id' FROM movies WHERE identifiers->>'tmdb_id' IS NOT NULL",
            )
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect()
        }
        _ => {
            // For other media types, no library matching supported yet
            return (StatusCode::OK, Json(json!({"file_count": 0, "message": "Library matching not supported for this media type"}))).into_response();
        }
    };

    // Filter to files whose candidate matches a library entity
    let matching_ids: Vec<Uuid> = candidate_pids
        .into_iter()
        .filter(|(_, pid)| library_pids.contains(pid))
        .map(|(id, _)| id)
        .collect();

    if matching_ids.is_empty() {
        return (StatusCode::OK, Json(json!({"file_count": 0}))).into_response();
    }

    let count = matching_ids.len();
    let payload = json!({ "file_ids": matching_ids, "link_to_library": true });
    let display = crate::jobs::fetch_job_display(&state.pool, "media_root", root_id).await;
    let context = JobContext::for_entity(
        "media_root",
        root_id,
        format!(
            "Matching {count} file{} to library",
            if count == 1 { "" } else { "s" }
        ),
    )
    .with_display_opt(display);

    match crate::jobs::enqueue_with_event(
        &state.pool,
        state.nats.publisher(),
        &state.job_events,
        "media_root_bulk_resolve",
        payload,
        context,
        false,
    )
    .await
    {
        Ok(job_id) => (
            StatusCode::ACCEPTED,
            Json(json!({"job_id": job_id, "file_count": count})),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Bulk manual resolve: user picks a candidate for multiple files at once.
///
/// Similar to bulk_resolve but uses a user-supplied candidate instead of
/// each file's stored match_candidates. Enqueues the same background job
/// with the candidate included in the payload.
pub async fn bulk_manual_resolve(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<BulkManualResolveRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    if body.file_ids.is_empty() {
        return (StatusCode::BAD_REQUEST, "No file IDs provided").into_response();
    }

    let count = body.file_ids.len();
    let candidate = json!({
        "provider": body.provider,
        "provider_id": body.provider_id,
        "title": body.title,
        "year": body.year,
    });
    let payload = json!({
        "file_ids": body.file_ids,
        "candidate": candidate,
    });
    let display = crate::jobs::fetch_job_display(&state.pool, "media_root", Uuid::nil()).await;
    let context = JobContext::for_entity(
        "media_root",
        Uuid::nil(),
        format!(
            "Resolving {} file{} as \"{}\"",
            count,
            if count == 1 { "" } else { "s" },
            body.title
        ),
    )
    .with_display_opt(display);

    match crate::jobs::enqueue_with_event(
        &state.pool,
        state.nats.publisher(),
        &state.job_events,
        "media_root_bulk_resolve",
        payload,
        context,
        false,
    )
    .await
    {
        Ok(job_id) => (StatusCode::ACCEPTED, Json(json!({"job_id": job_id}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Bulk-ignore files.
pub async fn bulk_ignore(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<BulkResolveRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    if body.file_ids.is_empty() {
        return (StatusCode::BAD_REQUEST, "No file IDs provided").into_response();
    }
    for file_id in &body.file_ids {
        let _ = media_root_files::set_ignored(&state.pool, *file_id).await;
    }
    let _ =
        search::batch_index_media_root_files(state.search.as_ref(), &state.pool, &body.file_ids)
            .await;
    StatusCode::NO_CONTENT.into_response()
}

/// Bulk-rematch files (reset to pending for re-processing).
pub async fn bulk_rematch(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<BulkResolveRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    for file_id in &body.file_ids {
        let _ = media_root_files::reset_for_rematch(&state.pool, *file_id).await;
    }
    let _ =
        search::batch_index_media_root_files(state.search.as_ref(), &state.pool, &body.file_ids)
            .await;
    StatusCode::NO_CONTENT.into_response()
}

// ============================================================================
// Reorganize
// ============================================================================

/// Trigger file reorganization (rename files to match naming template).
pub async fn trigger_reorganize(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<ReorganizeRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let root = match media_roots::get_by_id(&state.pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let display = crate::jobs::fetch_job_display(&state.pool, "media_root", id).await;
    let context = JobContext::for_entity(
        "media_root",
        id,
        format!(
            "{} files in '{}'",
            if body.dry_run {
                "Previewing reorganize"
            } else {
                "Reorganizing"
            },
            root.name
        ),
    )
    .with_display_opt(display);

    match crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "media_root_reorganize",
        json!({"media_root_id": id, "dry_run": body.dry_run, "include_unconfirmed": body.include_unconfirmed}),
        context,
    )
    .await
    {
        Ok(job_id) => Json(json!({"job_id": job_id})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
pub struct ReorganizeRequest {
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub include_unconfirmed: bool,
}

/// Backfill media_root_files from entity file tables (book_files, movie_files, tv_episode_files).
pub async fn trigger_backfill(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let root = match media_roots::get_by_id(&state.pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let display = crate::jobs::fetch_job_display(&state.pool, "media_root", id).await;
    let context = JobContext::for_entity(
        "media_root",
        id,
        format!("Backfilling media root '{}'", root.name),
    )
    .with_display_opt(display);

    match crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "media_root_backfill",
        json!({"media_root_id": id}),
        context,
    )
    .await
    {
        Ok(job_id) => Json(json!({"job_id": job_id})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Trigger sidecar writing for all added files in a media root.
pub async fn trigger_write_sidecars(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let root = match media_roots::get_by_id(&state.pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let display = crate::jobs::fetch_job_display(&state.pool, "media_root", id).await;
    let context = JobContext::for_entity(
        "media_root",
        id,
        format!("Writing sidecars for '{}'", root.name),
    )
    .with_display_opt(display);

    match crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "media_root_write_sidecars",
        json!({"media_root_id": id}),
        context,
    )
    .await
    {
        Ok(job_id) => Json(json!({"job_id": job_id})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Trigger a full sidecar rewrite across all media roots.
///
/// For TV shows, this enqueues `tv_show_enrich` jobs to populate episode-level
/// identifiers before writing sidecars. Movies and books are written directly.
/// POST /api/v1/admin/media-roots/sidecar-rewrite
pub async fn trigger_sidecar_rewrite(
    State(state): State<AppState>,
    auth: AuthUser,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }

    let context = JobContext::new("Rewriting all sidecar files");

    match crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "sidecar_rewrite",
        json!({}),
        context,
    )
    .await
    {
        Ok(job_id) => Json(json!({"job_id": job_id, "message": "Sidecar rewrite job enqueued"}))
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Trigger image writing for all added files in a media root.
pub async fn trigger_write_images(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let root = match media_roots::get_by_id(&state.pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let display = crate::jobs::fetch_job_display(&state.pool, "media_root", id).await;
    let context = JobContext::for_entity(
        "media_root",
        id,
        format!("Writing images for '{}'", root.name),
    )
    .with_display_opt(display);

    match crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "media_root_write_images",
        json!({"media_root_id": id}),
        context,
    )
    .await
    {
        Ok(job_id) => Json(json!({"job_id": job_id})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Mark a file as ignored.
pub async fn ignore_file(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(file_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    match media_root_files::set_ignored(&state.pool, file_id).await {
        Ok(()) => {
            let _ =
                search::index_media_root_file(state.search.as_ref(), &state.pool, file_id).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Reset a file for re-matching.
pub async fn rematch_file(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(file_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    match media_root_files::reset_for_rematch(&state.pool, file_id).await {
        Ok(()) => {
            let _ =
                search::index_media_root_file(state.search.as_ref(), &state.pool, file_id).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ============================================================================
// Manual entity linking
// ============================================================================

fn validate_entity_type_for_root(root_media_type: &str, entity_type: &str) -> Result<(), String> {
    match (root_media_type, entity_type) {
        ("book" | "audiobook", "book") => Ok(()),
        ("movie", "movie") => Ok(()),
        ("tv" | "sport", "tv_episode") => Ok(()),
        _ => Err(format!(
            "Cannot link entity type '{entity_type}' to a '{root_media_type}' media root"
        )),
    }
}

/// Link a media root file to an existing library entity (no provider needed).
pub async fn link_to_entity(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(file_id): Path<Uuid>,
    Json(body): Json<LinkToEntityRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }

    let file = match media_root_files::get_by_id(&state.pool, file_id).await {
        Ok(Some(f)) => f,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let root = match media_roots::get_by_id(&state.pool, file.media_root_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    if let Err(msg) = validate_entity_type_for_root(&root.media_type, &body.entity_type) {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }

    let ctx = ResolveContext {
        pool: &state.pool,
        nats: state.nats.publisher(),
        search: state.search.as_ref(),
    };

    match media_root_resolve::link_file_to_entity(
        &ctx,
        &root,
        &file,
        file_id,
        &body.entity_type,
        body.entity_id,
    )
    .await
    {
        Ok(outcome) => Json(json!({
            "status": "linked",
            "entity_type": outcome.entity_type,
            "entity_id": outcome.entity_id,
        }))
        .into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

/// Link multiple media root files to the same existing library entity.
pub async fn bulk_link_to_entity(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<BulkLinkToEntityRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }

    if body.file_ids.is_empty() {
        return (StatusCode::BAD_REQUEST, "No file IDs provided").into_response();
    }

    let ctx = ResolveContext {
        pool: &state.pool,
        nats: state.nats.publisher(),
        search: state.search.as_ref(),
    };

    let mut linked = 0u32;
    for fid in &body.file_ids {
        let file = match media_root_files::get_by_id(&state.pool, *fid).await {
            Ok(Some(f)) => f,
            _ => continue,
        };
        let root = match media_roots::get_by_id(&state.pool, file.media_root_id).await {
            Ok(Some(r)) => r,
            _ => continue,
        };
        if validate_entity_type_for_root(&root.media_type, &body.entity_type).is_err() {
            continue;
        }
        if media_root_resolve::link_file_to_entity(
            &ctx,
            &root,
            &file,
            *fid,
            &body.entity_type,
            body.entity_id,
        )
        .await
        .is_ok()
        {
            linked += 1;
        }
    }

    Json(json!({ "linked": linked })).into_response()
}

// ============================================================================
// Reinterpret
// ============================================================================

#[derive(Deserialize)]
pub struct ReinterpretRequest {
    #[serde(default)]
    pub status: Option<String>,
}

/// Reset files in a media root for re-interpretation.
pub async fn reinterpret_media_root(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(root_id): Path<Uuid>,
    Json(body): Json<ReinterpretRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let root = match media_roots::get_by_id(&state.pool, root_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let count =
        match media_root_files::reset_for_reinterpret(&state.pool, root_id, body.status.as_deref())
            .await
        {
            Ok(n) => n,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };

    if count > 0 {
        let display = crate::jobs::fetch_job_display(&state.pool, "media_root", root_id).await;
        let context = JobContext::for_entity(
            "media_root",
            root_id,
            format!("Re-interpreting {} files in '{}'", count, root.name),
        )
        .with_display_opt(display);
        if let Err(e) = crate::jobs::enqueue(
            &state.pool,
            state.nats.publisher(),
            "media_root_interpret",
            json!({"media_root_id": root_id}),
            context,
        )
        .await
        {
            tracing::warn!(error = %e, "Failed to enqueue media_root_interpret for reinterpret");
        }
    }

    Json(json!({ "count": count })).into_response()
}

/// Perform a bulk action on all files within a directory.
pub async fn directory_action(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<DirectoryActionRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let root = match media_roots::get_by_id(&state.pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let file_ids = match media_root_files::list_file_ids_in_directory(
        &state.pool,
        id,
        &body.directory,
        &root.path,
    )
    .await
    {
        Ok(ids) => ids,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    if file_ids.is_empty() {
        return (StatusCode::NOT_FOUND, "No files found in directory").into_response();
    }

    match body.action.as_str() {
        "ignore" => {
            for file_id in &file_ids {
                let _ = media_root_files::set_ignored(&state.pool, *file_id).await;
            }
        }
        "rematch" => {
            for file_id in &file_ids {
                let _ = media_root_files::reset_for_rematch(&state.pool, *file_id).await;
            }
        }
        other => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Unknown action: {other}. Valid actions: ignore, rematch"),
            )
                .into_response();
        }
    }
    let _ =
        search::batch_index_media_root_files(state.search.as_ref(), &state.pool, &file_ids).await;

    Json(json!({ "count": file_ids.len() })).into_response()
}
