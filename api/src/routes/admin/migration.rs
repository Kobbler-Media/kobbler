//! Sonarr/Radarr migration endpoints.
//!
//! Test connection, preview items, and import movies/TV shows from
//! external Sonarr and Radarr instances.

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use tracing::{info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::jobs::{self, JobContext};
use crate::repositories::{media_root_files, movies, tv_shows};
use crate::routes::AuthUser;
use crate::runtime_config::RuntimeConfigUpdate;

// =============================================================================
// Sonarr/Radarr API Types
// =============================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct SonarrSeries {
    id: i64,
    title: String,
    year: Option<i32>,
    tvdb_id: Option<i64>,
    #[serde(default)]
    tmdb_id: Option<i64>,
    imdb_id: Option<String>,
    monitored: bool,
    status: Option<String>,
    path: String,
    root_folder_path: Option<String>,
    statistics: Option<SonarrSeriesStatistics>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SonarrSeriesStatistics {
    episode_file_count: Option<i64>,
    episode_count: Option<i64>,
}

/// Episode data from Sonarr. Used by the `migration_episode_reconcile` job handler
/// via [`fetch_arr`].
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SonarrEpisode {
    pub episode_number: i32,
    pub season_number: i32,
    pub has_file: bool,
    pub monitored: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct RadarrMovie {
    id: i64,
    title: String,
    year: Option<i32>,
    tmdb_id: Option<i64>,
    imdb_id: Option<String>,
    monitored: bool,
    has_file: bool,
    path: String,
    root_folder_path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SystemStatusResponse {
    version: Option<String>,
}

// =============================================================================
// HTTP Client Helper
// =============================================================================

pub async fn fetch_arr<T: DeserializeOwned>(
    url: &str,
    api_key: &str,
    path: &str,
) -> Result<T, String> {
    let base = url.trim_end_matches('/');
    let full_url = format!("{base}{path}");
    let client = crate::http::new_client();
    let resp = client
        .get(&full_url)
        .header("X-Api-Key", api_key)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!(
            "API returned status {} for {full_url}",
            resp.status()
        ));
    }

    resp.json::<T>()
        .await
        .map_err(|e| format!("Failed to parse response from {full_url}: {e}"))
}

// =============================================================================
// Request/Response Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct TestConnectionRequest {
    service: String,
    url: String,
    api_key: String,
}

#[derive(Debug, Deserialize)]
pub struct PreviewRequest {
    service: String,
}

#[derive(Debug, Deserialize)]
pub struct ImportRequest {
    service: String,
    tmdb_ids: Vec<String>,
    root_mapping: HashMap<String, Uuid>,
    want_missing: Option<bool>,
}

#[derive(Debug, Serialize)]
struct PreviewResponse {
    service: String,
    summary: PreviewSummary,
    items: Vec<PreviewItem>,
    root_folders: Vec<RootFolderMapping>,
}

#[derive(Debug, Serialize)]
struct PreviewSummary {
    total: usize,
    new: usize,
    existing: usize,
    no_tmdb_id: usize,
}

#[derive(Debug, Serialize)]
struct PreviewItem {
    title: String,
    year: Option<i32>,
    tmdb_id: Option<String>,
    tvdb_id: Option<String>,
    imdb_id: Option<String>,
    monitored: bool,
    has_files: bool,
    will_want: bool,
    source_status: Option<String>,
    import_status: String,
    kobbler_id: Option<Uuid>,
    source_id: i64,
    episode_stats: Option<EpisodeStats>,
}

#[derive(Debug, Serialize)]
struct EpisodeStats {
    total: i64,
    with_file: i64,
    monitored: i64,
}

#[derive(Debug, Serialize)]
struct RootFolderMapping {
    path: String,
    suggested_media_root_id: Option<Uuid>,
    suggested_media_root_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct ImportResponse {
    imported: usize,
    skipped: usize,
    wants_enqueued: usize,
    skipped_reasons: Vec<SkippedItem>,
    scans_triggered: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
struct SkippedItem {
    title: String,
    reason: String,
}

// =============================================================================
// Handlers
// =============================================================================

/// POST /api/v1/admin/migration/test
pub async fn test_connection(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<TestConnectionRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let status_result: Result<SystemStatusResponse, String> =
        fetch_arr(&req.url, &req.api_key, "/api/v3/system/status").await;

    match status_result {
        Ok(status) => {
            let mut update = RuntimeConfigUpdate::default();
            match req.service.as_str() {
                "sonarr" => {
                    update.sonarr_url = Some(req.url);
                    update.sonarr_api_key = Some(req.api_key);
                }
                "radarr" => {
                    update.radarr_url = Some(req.url);
                    update.radarr_api_key = Some(req.api_key);
                }
                other => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"error": format!("Unknown service: {other}")})),
                    )
                        .into_response();
                }
            }

            if let Err(e) = state.runtime_config.update(update).await {
                warn!(error = %e, "Failed to save migration config");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("Failed to save config: {e}")})),
                )
                    .into_response();
            }

            info!(
                service = %req.service,
                version = ?status.version,
                "Migration connection test successful"
            );

            Json(json!({
                "ok": true,
                "version": status.version,
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("Connection failed: {e}")})),
        )
            .into_response(),
    }
}

/// POST /api/v1/admin/migration/preview
pub async fn preview(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<PreviewRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let config = state.runtime_config.get().await;

    match req.service.as_str() {
        "sonarr" => preview_sonarr(&state, &config).await,
        "radarr" => preview_radarr(&state, &config).await,
        other => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("Unknown service: {other}")})),
        )
            .into_response(),
    }
}

async fn preview_sonarr(
    state: &AppState,
    config: &crate::runtime_config::RuntimeConfig,
) -> axum::response::Response {
    let (url, api_key) = match (&config.sonarr_url, &config.sonarr_api_key) {
        (Some(u), Some(k)) => (u.as_str(), k.as_str()),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "Sonarr not configured. Test connection first."})),
            )
                .into_response();
        }
    };

    let series: Vec<SonarrSeries> = match fetch_arr(url, api_key, "/api/v3/series").await {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("Failed to fetch Sonarr series: {e}")})),
            )
                .into_response();
        }
    };

    // Query existing TV shows by tmdb_id (runtime query — migration-only)
    let existing = match sqlx::query_as::<_, (Uuid, Option<String>)>(
        "SELECT id, identifiers->>'tmdb_id' as tmdb_id FROM tv_shows WHERE identifiers->>'tmdb_id' IS NOT NULL",
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => {
            let mut map = HashMap::new();
            for (id, tmdb_id) in rows {
                if let Some(tid) = tmdb_id {
                    map.insert(tid, id);
                }
            }
            map
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error: {e}")})),
            )
                .into_response();
        }
    };

    // Collect unique root folder paths
    let mut root_paths: Vec<String> = series
        .iter()
        .filter_map(|s| s.root_folder_path.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    root_paths.sort();

    let root_folders = match build_root_folder_mappings(state, &root_paths, "tv").await {
        Ok(rf) => rf,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error: {e}")})),
            )
                .into_response();
        }
    };

    let mut summary = PreviewSummary {
        total: series.len(),
        new: 0,
        existing: 0,
        no_tmdb_id: 0,
    };

    let items: Vec<PreviewItem> = series
        .into_iter()
        .map(|s| {
            let tmdb_str = s.tmdb_id.map(|id| id.to_string());
            let (import_status, kobbler_id) = if let Some(ref tid) = tmdb_str {
                if let Some(&kid) = existing.get(tid) {
                    summary.existing += 1;
                    ("exists".to_string(), Some(kid))
                } else {
                    summary.new += 1;
                    ("new".to_string(), None)
                }
            } else {
                summary.no_tmdb_id += 1;
                ("new".to_string(), None)
            };

            let has_files = s
                .statistics
                .as_ref()
                .and_then(|st| st.episode_file_count)
                .unwrap_or(0)
                > 0;

            let episode_stats = s.statistics.as_ref().map(|st| EpisodeStats {
                total: st.episode_count.unwrap_or(0),
                with_file: st.episode_file_count.unwrap_or(0),
                monitored: 0, // not available from series-level stats
            });

            let has_gaps = s
                .statistics
                .as_ref()
                .map(|st| st.episode_file_count.unwrap_or(0) < st.episode_count.unwrap_or(0))
                .unwrap_or(false);

            PreviewItem {
                title: s.title,
                year: s.year,
                tmdb_id: tmdb_str,
                tvdb_id: s.tvdb_id.map(|id| id.to_string()),
                imdb_id: s.imdb_id,
                monitored: s.monitored,
                has_files,
                will_want: s.monitored && has_gaps,
                source_status: s.status,
                import_status,
                kobbler_id,
                source_id: s.id,
                episode_stats,
            }
        })
        .collect();

    Json(PreviewResponse {
        service: "sonarr".to_string(),
        summary,
        items,
        root_folders,
    })
    .into_response()
}

async fn preview_radarr(
    state: &AppState,
    config: &crate::runtime_config::RuntimeConfig,
) -> axum::response::Response {
    let (url, api_key) = match (&config.radarr_url, &config.radarr_api_key) {
        (Some(u), Some(k)) => (u.as_str(), k.as_str()),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "Radarr not configured. Test connection first."})),
            )
                .into_response();
        }
    };

    let radarr_movies: Vec<RadarrMovie> = match fetch_arr(url, api_key, "/api/v3/movie").await {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("Failed to fetch Radarr movies: {e}")})),
            )
                .into_response();
        }
    };

    // Query existing movies by tmdb_id (runtime query — migration-only)
    let existing = match sqlx::query_as::<_, (Uuid, Option<String>)>(
        "SELECT id, identifiers->>'tmdb_id' as tmdb_id FROM movies WHERE identifiers->>'tmdb_id' IS NOT NULL",
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => {
            let mut map = HashMap::new();
            for (id, tmdb_id) in rows {
                if let Some(tid) = tmdb_id {
                    map.insert(tid, id);
                }
            }
            map
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error: {e}")})),
            )
                .into_response();
        }
    };

    // Collect unique root folder paths
    let mut root_paths: Vec<String> = radarr_movies
        .iter()
        .filter_map(|m| m.root_folder_path.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    root_paths.sort();

    let root_folders = match build_root_folder_mappings(state, &root_paths, "movie").await {
        Ok(rf) => rf,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Database error: {e}")})),
            )
                .into_response();
        }
    };

    let mut summary = PreviewSummary {
        total: radarr_movies.len(),
        new: 0,
        existing: 0,
        no_tmdb_id: 0,
    };

    let items: Vec<PreviewItem> = radarr_movies
        .into_iter()
        .map(|m| {
            let tmdb_str = m.tmdb_id.map(|id| id.to_string());
            let (import_status, kobbler_id) = if let Some(ref tid) = tmdb_str {
                if let Some(&kid) = existing.get(tid) {
                    summary.existing += 1;
                    ("exists".to_string(), Some(kid))
                } else {
                    summary.new += 1;
                    ("new".to_string(), None)
                }
            } else {
                summary.no_tmdb_id += 1;
                ("new".to_string(), None)
            };

            PreviewItem {
                title: m.title,
                year: m.year,
                tmdb_id: tmdb_str,
                tvdb_id: None,
                imdb_id: m.imdb_id,
                monitored: m.monitored,
                has_files: m.has_file,
                will_want: m.monitored && !m.has_file,
                source_status: None,
                import_status,
                kobbler_id,
                source_id: m.id,
                episode_stats: None,
            }
        })
        .collect();

    Json(PreviewResponse {
        service: "radarr".to_string(),
        summary,
        items,
        root_folders,
    })
    .into_response()
}

/// Match external root folder paths to Kobbler media roots by directory name suffix.
async fn build_root_folder_mappings(
    state: &AppState,
    root_paths: &[String],
    media_type: &str,
) -> Result<Vec<RootFolderMapping>, sqlx::Error> {
    // Runtime query — migration-only
    let media_roots = sqlx::query_as::<_, (Uuid, String, String, String)>(
        "SELECT id, name, path, media_type FROM media_roots WHERE enabled = TRUE AND media_type = $1",
    )
    .bind(media_type)
    .fetch_all(&state.pool)
    .await?;

    Ok(root_paths
        .iter()
        .map(|rp| {
            let rp_trimmed = rp.trim_end_matches('/');
            let rp_dir = rp_trimmed.rsplit('/').next().unwrap_or(rp_trimmed);

            // Tuple fields: (id, name, path, media_type)
            let matched = media_roots.iter().find(|mr| {
                let mr_trimmed = mr.2.trim_end_matches('/');
                let mr_dir = mr_trimmed.rsplit('/').next().unwrap_or(mr_trimmed);
                mr_dir == rp_dir
            });

            RootFolderMapping {
                path: rp.clone(),
                suggested_media_root_id: matched.map(|mr| mr.0),
                suggested_media_root_name: matched.map(|mr| mr.1.clone()),
            }
        })
        .collect())
}

/// POST /api/v1/admin/migration/import
pub async fn import_items(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<ImportRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let config = state.runtime_config.get().await;

    match req.service.as_str() {
        "sonarr" => import_sonarr(&state, &config, &req).await,
        "radarr" => import_radarr(&state, &config, &req).await,
        other => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("Unknown service: {other}")})),
        )
            .into_response(),
    }
}

async fn import_radarr(
    state: &AppState,
    config: &crate::runtime_config::RuntimeConfig,
    req: &ImportRequest,
) -> axum::response::Response {
    let (url, api_key) = match (&config.radarr_url, &config.radarr_api_key) {
        (Some(u), Some(k)) => (u.as_str(), k.as_str()),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "Radarr not configured. Test connection first."})),
            )
                .into_response();
        }
    };

    let radarr_movies: Vec<RadarrMovie> = match fetch_arr(url, api_key, "/api/v3/movie").await {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("Failed to fetch Radarr movies: {e}")})),
            )
                .into_response();
        }
    };

    let requested: HashSet<String> = req.tmdb_ids.iter().cloned().collect();
    let selected: Vec<&RadarrMovie> = radarr_movies
        .iter()
        .filter(|m| {
            m.tmdb_id
                .map(|id| requested.contains(&id.to_string()))
                .unwrap_or(false)
        })
        .collect();

    let mut imported = 0usize;
    let mut wants_enqueued = 0usize;
    let mut skipped_reasons = Vec::new();

    for movie in &selected {
        let tmdb_id = match movie.tmdb_id {
            Some(id) => id.to_string(),
            None => {
                skipped_reasons.push(SkippedItem {
                    title: movie.title.clone(),
                    reason: "No TMDB ID".to_string(),
                });
                continue;
            }
        };

        let root_path = movie.root_folder_path.as_deref().unwrap_or("");
        let media_root_id = req.root_mapping.get(root_path).copied();

        let mut identifiers = serde_json::Map::new();
        identifiers.insert("tmdb_id".to_string(), json!(tmdb_id));
        if let Some(ref imdb) = movie.imdb_id {
            identifiers.insert("imdb_id".to_string(), json!(imdb));
        }

        let input = movies::MovieInput {
            title: movie.title.clone(),
            sort_title: None,
            original_title: None,
            year: movie.year,
            runtime: None,
            description: None,
            tagline: None,
            images: None,
            identifiers: Some(serde_json::Value::Object(identifiers)),
            genres: None,
            rating: None,
            rating_count: None,
            content_rating: None,
            release_date: None,
            is_released: None,
            quality_profile_id: None,
            preferred_languages: None,
            media_root_id,
            streamed_at: None,
        };

        match movies::upsert_by_provider(&state.pool, "tmdb", &tmdb_id, input).await {
            Ok(id) => {
                let display = crate::jobs::fetch_job_display(&state.pool, "movie", id).await;
                let context =
                    JobContext::for_entity("movie", id, format!("Enriching '{}'", movie.title))
                        .with_display_opt(display);
                if let Err(e) = jobs::enqueue(
                    &state.pool,
                    state.nats.publisher(),
                    "movie_enrich",
                    json!({"movie_id": id}),
                    context,
                )
                .await
                {
                    warn!(error = %e, "Failed to enqueue movie_enrich job");
                }

                // Want missing movies: monitored in Radarr but no file
                if req.want_missing.unwrap_or(true) && movie.monitored && !movie.has_file {
                    if let Err(e) = movies::set_want_status(&state.pool, id, "wanted").await {
                        warn!(movie_id = %id, error = %e, "Failed to set movie want status");
                    }
                    let want_display =
                        crate::jobs::fetch_job_display(&state.pool, "movie", id).await;
                    let want_context = JobContext::for_entity(
                        "movie",
                        id,
                        format!("Searching for '{}'", movie.title),
                    )
                    .with_display_opt(want_display);
                    if let Err(e) = jobs::enqueue(
                        &state.pool,
                        state.nats.publisher(),
                        "movie_want",
                        json!({"movie_id": id}),
                        want_context,
                    )
                    .await
                    {
                        warn!(error = %e, "Failed to enqueue movie_want job");
                    } else {
                        wants_enqueued += 1;
                    }
                }

                imported += 1;
            }
            Err(e) => {
                warn!(title = %movie.title, error = %e, "Failed to upsert movie");
                skipped_reasons.push(SkippedItem {
                    title: movie.title.clone(),
                    reason: format!("Database error: {e}"),
                });
            }
        }
    }

    // Auto-resolve review files in mapped media roots that match imported movies
    let imported_tmdb_ids: HashSet<String> = selected
        .iter()
        .filter_map(|m| m.tmdb_id.map(|id| id.to_string()))
        .collect();

    let unique_roots: HashSet<Uuid> = req.root_mapping.values().copied().collect();
    let mut resolve_triggered = Vec::new();
    for root_id in &unique_roots {
        match resolve_matching_review_files(state, *root_id, &imported_tmdb_ids).await {
            Ok(count) if count > 0 => {
                info!(root_id = %root_id, count, "Enqueued bulk resolve for matching review files");
                resolve_triggered.push(*root_id);
            }
            Err(e) => {
                warn!(error = %e, root_id = %root_id, "Failed to resolve matching review files")
            }
            _ => {}
        }
    }

    info!(
        imported,
        wants_enqueued,
        skipped = skipped_reasons.len(),
        "Radarr import complete"
    );

    Json(ImportResponse {
        imported,
        skipped: skipped_reasons.len(),
        wants_enqueued,
        skipped_reasons,
        scans_triggered: resolve_triggered,
    })
    .into_response()
}

async fn import_sonarr(
    state: &AppState,
    config: &crate::runtime_config::RuntimeConfig,
    req: &ImportRequest,
) -> axum::response::Response {
    let (url, api_key) = match (&config.sonarr_url, &config.sonarr_api_key) {
        (Some(u), Some(k)) => (u.as_str(), k.as_str()),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "Sonarr not configured. Test connection first."})),
            )
                .into_response();
        }
    };

    let series: Vec<SonarrSeries> = match fetch_arr(url, api_key, "/api/v3/series").await {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("Failed to fetch Sonarr series: {e}")})),
            )
                .into_response();
        }
    };

    let requested: HashSet<String> = req.tmdb_ids.iter().cloned().collect();
    let selected: Vec<&SonarrSeries> = series
        .iter()
        .filter(|s| {
            s.tmdb_id
                .map(|id| requested.contains(&id.to_string()))
                .unwrap_or(false)
        })
        .collect();

    let mut imported = 0usize;
    let mut skipped_reasons = Vec::new();

    for show in &selected {
        let tmdb_id = match show.tmdb_id {
            Some(id) => id.to_string(),
            None => {
                skipped_reasons.push(SkippedItem {
                    title: show.title.clone(),
                    reason: "No TMDB ID".to_string(),
                });
                continue;
            }
        };

        let root_path = show.root_folder_path.as_deref().unwrap_or("");
        let media_root_id = req.root_mapping.get(root_path).copied();

        let mut identifiers = serde_json::Map::new();
        identifiers.insert("tmdb_id".to_string(), json!(tmdb_id));
        if let Some(tvdb) = show.tvdb_id {
            identifiers.insert("tvdb_id".to_string(), json!(tvdb.to_string()));
        }
        if let Some(ref imdb) = show.imdb_id {
            identifiers.insert("imdb_id".to_string(), json!(imdb));
        }

        let has_gaps = show
            .statistics
            .as_ref()
            .map(|st| st.episode_file_count.unwrap_or(0) < st.episode_count.unwrap_or(0))
            .unwrap_or(false);
        let auto_want_mode = if !show.monitored {
            Some("none".to_string())
        } else if has_gaps {
            Some("all".to_string())
        } else {
            Some("new_only".to_string())
        };

        let input = tv_shows::TvShowInput {
            title: show.title.clone(),
            sort_title: None,
            original_title: None,
            year: show.year,
            status: show.status.clone(),
            description: None,
            images: None,
            identifiers: Some(serde_json::Value::Object(identifiers)),
            genres: None,
            rating: None,
            rating_count: None,
            content_rating: None,
            network: None,
            is_monitored: Some(show.monitored),
            auto_want_mode,
            quality_profile_id: None,
            preferred_languages: None,
            media_root_id,
            first_aired: None,
            last_aired: None,
            next_airing: None,
            runtime: None,
            total_episode_count: None,
            content_category: None,
            sport_metadata: None,
            streamed_at: None,
        };

        match tv_shows::upsert_by_provider(&state.pool, "tmdb", &tmdb_id, input).await {
            Ok(id) => {
                let display = crate::jobs::fetch_job_display(&state.pool, "tv_show", id).await;
                let context =
                    JobContext::for_entity("tv_show", id, format!("Enriching '{}'", show.title))
                        .with_display_opt(display);
                if let Err(e) = jobs::enqueue(
                    &state.pool,
                    state.nats.publisher(),
                    "tv_show_enrich",
                    json!({"show_id": id}),
                    context,
                )
                .await
                {
                    warn!(error = %e, "Failed to enqueue tv_show_enrich job");
                }

                let ep_display = crate::jobs::fetch_job_display(&state.pool, "tv_show", id).await;
                let ep_context = JobContext::for_entity(
                    "tv_show",
                    id,
                    format!("Reconciling episodes for '{}'", show.title),
                )
                .with_display_opt(ep_display);
                if let Err(e) = jobs::enqueue(
                    &state.pool,
                    state.nats.publisher(),
                    "migration_episode_reconcile",
                    json!({
                        "show_id": id,
                        "sonarr_url": url,
                        "sonarr_api_key": api_key,
                        "sonarr_series_id": show.id,
                        "want_missing": req.want_missing.unwrap_or(true),
                    }),
                    ep_context,
                )
                .await
                {
                    warn!(error = %e, "Failed to enqueue migration_episode_reconcile job");
                }

                imported += 1;
            }
            Err(e) => {
                warn!(title = %show.title, error = %e, "Failed to upsert TV show");
                skipped_reasons.push(SkippedItem {
                    title: show.title.clone(),
                    reason: format!("Database error: {e}"),
                });
            }
        }
    }

    // Auto-resolve review files in mapped media roots that match imported shows
    let imported_tmdb_ids: HashSet<String> = selected
        .iter()
        .filter_map(|s| s.tmdb_id.map(|id| id.to_string()))
        .collect();

    let unique_roots: HashSet<Uuid> = req.root_mapping.values().copied().collect();
    let mut resolve_triggered = Vec::new();
    for root_id in &unique_roots {
        match resolve_matching_review_files(state, *root_id, &imported_tmdb_ids).await {
            Ok(count) if count > 0 => {
                info!(root_id = %root_id, count, "Enqueued bulk resolve for matching review files");
                resolve_triggered.push(*root_id);
            }
            Err(e) => {
                warn!(error = %e, root_id = %root_id, "Failed to resolve matching review files")
            }
            _ => {}
        }
    }

    info!(
        imported,
        skipped = skipped_reasons.len(),
        "Sonarr import complete"
    );

    Json(ImportResponse {
        imported,
        skipped: skipped_reasons.len(),
        wants_enqueued: 0, // TV wants are enqueued asynchronously by the reconcile job
        skipped_reasons,
        scans_triggered: resolve_triggered,
    })
    .into_response()
}

/// Resolve review-status media root files whose first match candidate has a
/// TMDB provider_id matching one of the imported IDs. Enqueues a bulk-resolve
/// job and returns the number of matching files.
async fn resolve_matching_review_files(
    state: &AppState,
    root_id: Uuid,
    imported_tmdb_ids: &HashSet<String>,
) -> Result<usize, String> {
    let review_files =
        media_root_files::list_for_root(&state.pool, root_id, Some("review"), 5000, 0)
            .await
            .map_err(|e| format!("Failed to list review files: {e}"))?;

    let matching_ids: Vec<Uuid> = review_files
        .iter()
        .filter(|f| {
            f.match_candidates
                .as_ref()
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|c| c.get("provider_id"))
                .map(|v| {
                    let pid = match v {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Number(n) => n.to_string(),
                        _ => return false,
                    };
                    imported_tmdb_ids.contains(&pid)
                })
                .unwrap_or(false)
        })
        .map(|f| f.id)
        .collect();

    if matching_ids.is_empty() {
        return Ok(0);
    }

    let count = matching_ids.len();
    let payload = json!({ "file_ids": matching_ids, "link_to_library": true });
    let display = crate::jobs::fetch_job_display(&state.pool, "media_root", root_id).await;
    let context = JobContext::for_entity(
        "media_root",
        root_id,
        format!("Resolving {count} imported files"),
    )
    .with_display_opt(display);

    jobs::enqueue_with_event(
        &state.pool,
        state.nats.publisher(),
        &state.job_events,
        "media_root_bulk_resolve",
        payload,
        context,
        false,
    )
    .await
    .map_err(|e| format!("Failed to enqueue bulk resolve: {e}"))?;

    Ok(count)
}
