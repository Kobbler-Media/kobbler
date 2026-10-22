//! Debrid stream route handlers.
//!
//! Provides endpoints for searching, resolving, and proxying debrid streams.
//! Stream sessions are persisted in `debrid_stream_sessions` for resume/replay.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Sse},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use time::OffsetDateTime;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::debrid_outcomes;
use crate::repositories::debrid_cache_files as cache_files;
use crate::repositories::{movie_watch_states, tv_watch_states};
use crate::routes::admin::debrid_errors::StreamFailure;
use crate::routes::admin::stream_search_request::{self, SearchTarget};
use crate::routes::admin::{debrid_open, stream_prewarm};
use crate::routes::helpers::build_kobload_client;
use crate::routes::{AuthUser, PlaybackAuth};

// =============================================================================
// Request/Response Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct StreamSearchBody {
    /// Look up a movie by ID and construct the search automatically.
    #[serde(default)]
    pub movie_id: Option<Uuid>,
    /// Look up an episode by ID and construct the search automatically.
    #[serde(default)]
    pub episode_id: Option<Uuid>,
    /// Raw metadata search (discover page — no DB entity yet).
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub imdb_id: Option<String>,
    #[serde(default)]
    pub tmdb_id: Option<i32>,
    #[serde(default)]
    pub media_type: Option<String>,
    /// Season number for TV episode search from discover.
    #[serde(default)]
    pub season: Option<u32>,
    /// Episode number for TV episode search from discover.
    #[serde(default)]
    pub episode: Option<u32>,
    /// This viewer's audio language, over the server-wide preference.
    #[serde(default)]
    pub preferred_audio_language: Option<String>,
    /// Raw search request (used when movie_id/episode_id not provided).
    #[serde(flatten)]
    pub raw: Option<nats_jobs::StreamSearchRequest>,
}

#[derive(Debug, Deserialize)]
pub struct ProgressBody {
    pub progress_seconds: f64,
}

#[derive(Debug, Deserialize)]
pub struct IngestBody {
    pub media_root_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct CreateStremioKeyBody {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct StremioKeyResponse {
    pub id: Uuid,
    pub api_key: String,
    pub name: String,
    pub created_at: String,
}

/// Row shape for SSE polling query (runtime query, not compile-time checked).
#[derive(sqlx::FromRow)]
struct DebridStatusRow {
    probe_data: Option<serde_json::Value>,
    skip_segments: Option<serde_json::Value>,
    cache_path: Option<String>,
    filesize: i64,
}

// =============================================================================
// Handlers
// =============================================================================

/// POST /api/v1/admin/stream/search
pub async fn stream_search(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Json(body): Json<StreamSearchBody>,
) -> impl IntoResponse {
    if auth.is_kids && !kids_may_search(&state.pool, &body).await {
        return (StatusCode::NOT_FOUND, "not available on a Kids profile").into_response();
    }
    let runtime = state.runtime_config.get().await;
    let client = match build_kobload_client(&runtime) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let target = if let Some(movie_id) = body.movie_id {
        Some(SearchTarget::Movie(movie_id))
    } else if let Some(episode_id) = body.episode_id {
        Some(SearchTarget::Episode(episode_id))
    } else {
        body.title.clone().map(|title| SearchTarget::Discover {
            media_type: body
                .media_type
                .clone()
                .unwrap_or_else(|| "movie".to_string()),
            title,
            year: body.year,
            imdb_id: body.imdb_id.clone(),
            tmdb_id: body.tmdb_id.map(|id| id.to_string()),
            season: body.season,
            episode: body.episode,
        })
    };
    let audio_language = body.preferred_audio_language.as_deref();
    let (request, preference) = match (target, body.raw) {
        (Some(target), _) => {
            match stream_search_request::prepare(&state, auth.user_id, &target, audio_language)
                .await
            {
                Ok(p) => (p.request, p.preference),
                Err(resp) => return resp,
            }
        }
        (None, Some(mut raw)) => {
            raw.audio_languages =
                stream_search_request::audio_languages(&state, audio_language).await;
            let preference =
                debrid_outcomes::search_preference(&state.pool, auth.user_id, None, None).await;
            raw.prefer = preference.prefer.clone().or(raw.prefer);
            (raw, preference)
        }
        (None, None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "provide movie_id, episode_id, title, or a raw search request"})),
            )
                .into_response();
        }
    };

    match client.stream_search_value(&request).await {
        Ok(mut value) => {
            let options: Vec<nats_jobs::StreamOption> =
                serde_json::from_value(value["options"].clone()).unwrap_or_default();
            let replay = debrid_outcomes::playable_preference(preference.replay, &options);
            if let Some(replay) = replay {
                value["preferred_info_hash"] = json!(replay.info_hash);
                if let Some(fid) = replay.file_id {
                    value["preferred_file_id"] = json!(fid);
                }
            }
            Json(value).into_response()
        }
        Err(e) => {
            error!(error = %e, "stream search failed");
            StreamFailure::from_kobload(&e).response(StatusCode::BAD_GATEWAY, &e.to_string())
        }
    }
}

/// Path of the probe sidecar for a cached debrid file: `foo.mkv` →
/// `foo.mkv.probe.json`.
///
/// Shared with `debrid_cache_cleanup` so the sidecar is deleted alongside the
/// file it describes — an orphaned sidecar is trusted verbatim if the same
/// info_hash is ever re-downloaded.
pub(crate) fn probe_sidecar_path(cache_path: &std::path::Path) -> std::path::PathBuf {
    cache_path.with_extension(
        cache_path
            .extension()
            .map(|e| format!("{}.probe.json", e.to_string_lossy()))
            .unwrap_or_else(|| "probe.json".to_string()),
    )
}

/// POST /api/v1/admin/stream/resolve
pub async fn stream_resolve(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Json(body): Json<StreamResolveBody>,
) -> impl IntoResponse {
    if auth.is_kids && !kids_may_resolve(&state.pool, &body).await {
        return (StatusCode::NOT_FOUND, "not available on a Kids profile").into_response();
    }
    if let Some(claimed) = stream_prewarm::claim_for_resolve(&state, auth.user_id, &body).await {
        return claimed;
    }

    let key = debrid_open::release_key(body.info_hash.as_deref(), body.file_id);
    // Held until the new session is in the dedup index, so concurrent resolves of the
    // same release share one session.
    let _resolve_guard = match key {
        Some(ref key) => {
            if let Some(resp) = dedup_hit(&state, key) {
                return resp;
            }
            let guard = state
                .debrid_resolve_locks
                .entry(key.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
                .lock_owned()
                .await;
            if let Some(resp) = dedup_hit(&state, key) {
                return resp;
            }
            Some(guard)
        }
        None => None,
    };

    match debrid_open::open(&state, auth.user_id, &body, debrid_open::OpenMode::Play).await {
        Ok(opened) => Json(opened.response).into_response(),
        Err(resp) => resp,
    }
}

fn dedup_hit(state: &AppState, key: &str) -> Option<axum::response::Response> {
    let existing_id = state.debrid_session_index.get(key).map(|e| *e)?;
    if let Some(mut session) = state.playback_sessions.get_mut(&existing_id) {
        session.touch();
    }
    match debrid_open::live_response(state, existing_id, None) {
        Some(resp) => {
            info!(session_id = %existing_id, cache_key = %key, "debrid session dedup hit");
            Some(Json(resp).into_response())
        }
        None => {
            state.debrid_session_index.remove(key);
            None
        }
    }
}

async fn kids_may_search(pool: &sqlx::PgPool, body: &StreamSearchBody) -> bool {
    let allowed = if let Some(movie_id) = body.movie_id {
        crate::kids::movie_allowed(pool, movie_id).await
    } else if let Some(episode_id) = body.episode_id {
        crate::kids::episode_allowed(pool, episode_id).await
    } else if let Some(tmdb_id) = body.tmdb_id {
        let media_type = body.media_type.as_deref().unwrap_or("movie");
        crate::kids::discover_title_allowed(pool, media_type, &tmdb_id.to_string()).await
    } else {
        Ok(false)
    };
    allowed.unwrap_or(false)
}

pub(crate) async fn kids_may_resolve(pool: &sqlx::PgPool, body: &StreamResolveBody) -> bool {
    let library_id = body.media_id.filter(|id| !id.is_nil());
    let tmdb_id = body
        .auto_create
        .as_ref()
        .and_then(|a| a.identifiers.as_ref())
        .and_then(|ids| ids.get("tmdb_id"))
        .and_then(|v| {
            v.as_str()
                .map(String::from)
                .or_else(|| v.as_i64().map(|n| n.to_string()))
        });
    let allowed = match (body.media_type.as_str(), library_id, tmdb_id) {
        ("movie", Some(id), _) => crate::kids::movie_allowed(pool, id).await,
        ("episode", Some(id), _) => crate::kids::episode_allowed(pool, id).await,
        (media_type, None, Some(tmdb_id)) => {
            crate::kids::discover_title_allowed(pool, media_type, &tmdb_id).await
        }
        _ => Ok(false),
    };
    allowed.unwrap_or(false)
}

/// Metadata for auto-creating a movie/show when streaming from discover.
#[derive(Debug, Clone, Deserialize)]
pub struct AutoCreateMedia {
    pub title: String,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub identifiers: Option<serde_json::Value>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub poster_url: Option<String>,
    #[serde(default)]
    pub backdrop_url: Option<String>,
    #[serde(default)]
    pub genres: Option<Vec<String>>,
    #[serde(default)]
    pub rating: Option<f64>,
    #[serde(default)]
    pub rating_count: Option<i32>,
    #[serde(default)]
    pub runtime: Option<i32>,
    #[serde(default)]
    pub network: Option<String>,
    /// For TV episode streaming: metadata provider type (e.g. "tmdb").
    #[serde(default)]
    pub provider: Option<String>,
    /// For TV episode streaming: provider show ID (e.g. TMDB show ID).
    #[serde(default)]
    pub provider_id: Option<String>,
    /// For TV episode streaming: season number to sync.
    #[serde(default)]
    pub season_number: Option<u32>,
    /// For TV episode streaming: episode number within the season.
    #[serde(default)]
    pub episode_number: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamResolveBody {
    pub debrid_provider_id: Uuid,
    #[serde(default)]
    pub info_hash: Option<String>,
    #[serde(default)]
    pub magnet_url: Option<String>,
    #[serde(default)]
    pub nzb_url: Option<String>,
    #[serde(default)]
    pub file_id: Option<i32>,
    // Metadata for session persistence
    pub media_type: String,
    /// Media ID — can be nil UUID when `auto_create` is provided.
    #[serde(default)]
    pub media_id: Option<Uuid>,
    pub release_title: String,
    pub debrid_provider: String,
    #[serde(default)]
    pub quality: Option<serde_json::Value>,
    /// Resume position in seconds. When set, the transcode starts directly
    /// at this position instead of from the beginning.
    pub start_time: Option<f64>,
    /// Client playback capabilities — used to decide direct play vs transcode.
    pub capabilities: Option<crate::streaming::decision::ClientCapabilities>,
    /// Estimated duration in seconds from metadata (TMDB runtime).
    /// Used to create the session immediately before probe completes.
    pub estimated_duration: Option<f64>,
    /// When set, auto-create the movie/show if it doesn't exist (discover page flow).
    #[serde(default)]
    pub auto_create: Option<AutoCreateMedia>,
    /// This viewer's audio language, over the server-wide preference.
    #[serde(default)]
    pub preferred_audio_language: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ResolveStatusQuery {
    pub provider_id: String,
    pub torrent_id: String,
}

/// GET /api/v1/admin/stream/resolve/status?provider_id={}&torrent_id={}
///
/// Proxies to Kobload's resolve status endpoint for polling uncached torrent progress.
pub async fn stream_resolve_status(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<ResolveStatusQuery>,
) -> impl IntoResponse {
    let runtime = state.runtime_config.get().await;
    let client = match build_kobload_client(&runtime) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    match client
        .stream_resolve_status(&query.provider_id, &query.torrent_id)
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => {
            error!(error = %e, "stream resolve status failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("stream resolve status failed: {e}")})),
            )
                .into_response()
        }
    }
}

/// Sessions are addressed by an opaque id, so every read and write of one must
/// still carry the owner predicate. `None` means admin — free to act on any
/// session, matching the device-ownership rule elsewhere.
pub(crate) fn session_owner(user_id: Uuid, role: &str) -> Option<Uuid> {
    (role != "admin").then_some(user_id)
}

/// GET /api/v1/admin/stream/:id/status — SSE endpoint for processing progress
pub async fn stream_status(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Path(session_id): Path<Uuid>,
) -> impl IntoResponse {
    // Verify the session exists and belongs to the caller
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS(
             SELECT 1 FROM debrid_stream_sessions
             WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)
           ) AS "exists!""#,
        session_id,
        session_owner(auth.user_id, &auth.role),
    )
    .fetch_one(&state.pool)
    .await
    .unwrap_or(false);

    if !exists {
        return (StatusCode::NOT_FOUND, "session not found").into_response();
    }

    let pool = state.pool.clone();
    let event_stream = async_stream::stream! {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        let mut sent_probe = false;
        // Track the last emitted skip-segments JSON so we re-emit on value
        // change. The debrid_process pipeline now writes twice: once with
        // chapter-derived segments immediately after probe, then again with
        // the merged (chapter + cross-episode) result after analyze.
        let mut last_skip: Option<serde_json::Value> = None;
        let mut sent_thumbnails = false;
        let mut sent_download_complete = false;
        // Timeout after 10 minutes to prevent leaked connections
        let deadline = tokio::time::Instant::now() + Duration::from_secs(600);

        // Initial connected event
        yield Ok::<_, Infallible>(
            axum::response::sse::Event::default()
                .data(serde_json::to_string(&json!({"type": "connected"})).unwrap_or_default()),
        );

        loop {
            interval.tick().await;
            if tokio::time::Instant::now() > deadline {
                break;
            }

            let row = sqlx::query_as::<_, DebridStatusRow>(
                r#"
                SELECT probe_data, skip_segments, cache_path, filesize
                FROM debrid_stream_sessions
                WHERE id = $1
                "#,
            )
            .bind(session_id)
            .fetch_optional(&pool)
            .await;

            let row = match row {
                Ok(Some(r)) => r,
                Ok(None) => break, // session deleted
                Err(_) => continue,
            };

            if !sent_probe
                && let Some(ref probe_data) = row.probe_data
            {
                let mut event = json!({"type": "probe_complete"});
                if let Some(obj) = probe_data.as_object() {
                    if let Some(duration) = obj.get("duration") {
                        event["duration"] = duration.clone();
                    }
                    if let Some(method) = obj.get("method") {
                        event["method"] = method.clone();
                    }
                    if let Some(preferred) = obj.get("preferred_audio_stream_index") {
                        event["preferred_audio_stream_index"] = preferred.clone();
                    }
                    // Enrich audio_tracks with is_default from the decision
                    let selected_idx = obj
                        .get("selected_audio_stream_index")
                        .and_then(|v| v.as_i64());
                    if let Some(audio) = obj.get("audio_streams")
                        && let Some(tracks) = audio.as_array()
                    {
                        let enriched: Vec<serde_json::Value> = tracks
                            .iter()
                            .map(|t| {
                                let mut track = t.clone();
                                if let Some(obj) = track.as_object_mut() {
                                    let idx = obj.get("stream_index").and_then(|v| v.as_i64());
                                    obj.insert(
                                        "is_default".to_string(),
                                        json!(selected_idx.is_some() && idx == selected_idx),
                                    );
                                }
                                track
                            })
                            .collect();
                        event["audio_tracks"] = json!(enriched);
                    }
                    if let Some(subs) = obj.get("subtitle_streams") {
                        event["subtitles"] = subs.clone();
                    }
                }
                yield Ok(
                    axum::response::sse::Event::default()
                        .data(serde_json::to_string(&event).unwrap_or_default()),
                );
                sent_probe = true;
            }

            if let Some(ref skip_data) = row.skip_segments
                && last_skip.as_ref() != Some(skip_data)
            {
                let event = json!({
                    "type": "skip_segments_ready",
                    "segments": skip_data,
                });
                yield Ok(
                    axum::response::sse::Event::default()
                        .data(serde_json::to_string(&event).unwrap_or_default()),
                );
                last_skip = Some(skip_data.clone());
            }

            // `cache_path` is set in the DB as soon as the progressive
            // downloader creates the target file — the file existing is NOT
            // proof the download finished. Only emit `download_complete` once
            // the on-disk size reaches the expected `filesize`, and guard so
            // it fires exactly once per SSE connection.
            if let Some(ref cache_path) = row.cache_path {
                let path = PathBuf::from(cache_path);

                if !sent_download_complete
                    && let Ok(meta) = tokio::fs::metadata(&path).await
                    && row.filesize > 0
                    && meta.len() as i64 >= row.filesize
                {
                    let event = json!({
                        "type": "download_complete",
                        "cache_path": cache_path,
                    });
                    yield Ok(
                        axum::response::sse::Event::default()
                            .data(serde_json::to_string(&event).unwrap_or_default()),
                    );
                    sent_download_complete = true;
                }

                // Check if thumbnails have been generated
                if !sent_thumbnails {
                    let thumb_dir = path
                        .parent()
                        .unwrap_or(&path)
                        .join("media_cache")
                        .join("trickplay");
                    let vtt_path = thumb_dir.join("thumbnails.vtt");
                    if vtt_path.exists() {
                        let url = format!(
                            "/api/v1/admin/playback/sessions/{session_id}/thumbnails.vtt"
                        );
                        let event = json!({
                            "type": "thumbnails_ready",
                            "thumbnails_url": url,
                        });
                        yield Ok(
                            axum::response::sse::Event::default()
                                .data(serde_json::to_string(&event).unwrap_or_default()),
                        );
                        sent_thumbnails = true;
                    }
                }
            }

            // Close when all features have been sent
            if sent_probe && sent_thumbnails && sent_download_complete {
                break;
            }
        }
    };

    Sse::new(event_stream).into_response()
}

/// POST /api/v1/admin/stream/:id/progress — update watch progress
pub async fn stream_progress(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(session_id): Path<Uuid>,
    Json(body): Json<ProgressBody>,
) -> impl IntoResponse {
    let result = sqlx::query!(
        r#"
        UPDATE debrid_stream_sessions
        SET progress_seconds = $2, updated_at = NOW()
        WHERE id = $1 AND ($3::uuid IS NULL OR user_id = $3)
        "#,
        session_id,
        body.progress_seconds,
        session_owner(auth.user_id, &auth.role),
    )
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() == 0 => {
            return (StatusCode::NOT_FOUND, "session not found").into_response();
        }
        Err(e) => {
            error!(error = %e, "failed to update stream progress");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
        Ok(_) => {}
    }

    if debrid_outcomes::progress_means_played(body.progress_seconds) {
        debrid_outcomes::record_played(&state, session_id).await;
    }

    // Always save position to watch state (enables resume).
    // Mark as "watched" only when >= 90% complete.
    if let Ok(Some(session)) = sqlx::query!(
        r#"
        SELECT media_type, media_id, probe_data
        FROM debrid_stream_sessions
        WHERE id = $1
        "#,
        session_id,
    )
    .fetch_optional(&state.pool)
    .await
    {
        let duration = session
            .probe_data
            .as_ref()
            .and_then(|p| p.get("duration"))
            .and_then(|d| d.as_f64());

        let is_watched = duration
            .map(|dur| dur > 0.0 && body.progress_seconds / dur >= 0.90)
            .unwrap_or(false);
        let position = body.progress_seconds as i32;

        // Save position directly — use a lightweight query that won't
        // overwrite `watched=true` if the user already completed this content.
        match session.media_type.as_str() {
            "movie" => {
                if is_watched {
                    let input = movie_watch_states::WatchStateInput {
                        movie_id: session.media_id,
                        user_id: auth.user_id,
                        watched: true,
                        watched_at: Some(OffsetDateTime::now_utc()),
                        position_seconds: Some(position),
                        rating: None,
                    };
                    if let Err(e) = movie_watch_states::upsert(&state.pool, input).await {
                        warn!(error = %e, "failed to mark movie as watched from stream");
                    }
                } else {
                    // Position-only update: preserve existing watched state
                    let _ = sqlx::query!(
                        r#"
                        INSERT INTO movie_watch_states (id, movie_id, user_id, watched, position_seconds)
                        VALUES ($1, $2, $3, FALSE, $4)
                        ON CONFLICT (movie_id, user_id)
                        DO UPDATE SET position_seconds = $4, source = 'local', updated_at = NOW()
                        "#,
                        Uuid::now_v7(),
                        session.media_id,
                        auth.user_id,
                        position,
                    )
                    .execute(&state.pool)
                    .await;
                }
            }
            "episode" => {
                if is_watched {
                    let input = tv_watch_states::TvWatchStateInput {
                        episode_id: session.media_id,
                        user_id: auth.user_id,
                        watched: true,
                        watched_at: Some(OffsetDateTime::now_utc()),
                        position_seconds: Some(position),
                        rating: None,
                    };
                    if let Err(e) = tv_watch_states::upsert(&state.pool, input).await {
                        warn!(error = %e, "failed to mark episode as watched from stream");
                    }
                } else {
                    let _ = sqlx::query!(
                        r#"
                        INSERT INTO tv_watch_states (id, episode_id, user_id, watched, position_seconds)
                        VALUES ($1, $2, $3, FALSE, $4)
                        ON CONFLICT (episode_id, user_id)
                        DO UPDATE SET position_seconds = $4, source = 'local', updated_at = NOW()
                        "#,
                        Uuid::now_v7(),
                        session.media_id,
                        auth.user_id,
                        position,
                    )
                    .execute(&state.pool)
                    .await;
                }
            }
            _ => {}
        }
    }

    StatusCode::NO_CONTENT.into_response()
}

/// DELETE /api/v1/admin/stream/:id — end a stream session
pub async fn stream_delete(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(session_id): Path<Uuid>,
) -> impl IntoResponse {
    let result = sqlx::query!(
        "DELETE FROM debrid_stream_sessions WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
        session_id,
        session_owner(auth.user_id, &auth.role),
    )
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() == 0 => {
            (StatusCode::NOT_FOUND, "session not found").into_response()
        }
        Ok(_) => {
            crate::routes::admin::streaming::end_session(&state, session_id).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            error!(error = %e, "failed to delete stream session");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// POST /api/v1/admin/stream/:id/pin — pin a stream session
pub async fn stream_pin(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(session_id): Path<Uuid>,
) -> impl IntoResponse {
    let result = sqlx::query!(
        r#"
        UPDATE debrid_stream_sessions
        SET pinned = true, updated_at = NOW()
        WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)
        "#,
        session_id,
        session_owner(auth.user_id, &auth.role),
    )
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() == 0 => {
            (StatusCode::NOT_FOUND, "session not found").into_response()
        }
        Ok(_) => {
            // Also pin associated cache files
            let _ = cache_files::set_pinned_for_session(&state.pool, session_id, true).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            error!(error = %e, "failed to pin stream session");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// DELETE /api/v1/admin/stream/:id/pin — unpin a stream session
pub async fn stream_unpin(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(session_id): Path<Uuid>,
) -> impl IntoResponse {
    let result = sqlx::query!(
        r#"
        UPDATE debrid_stream_sessions
        SET pinned = false, updated_at = NOW()
        WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)
        "#,
        session_id,
        session_owner(auth.user_id, &auth.role),
    )
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() == 0 => {
            (StatusCode::NOT_FOUND, "session not found").into_response()
        }
        Ok(_) => {
            // Also unpin associated cache files
            let _ = cache_files::set_pinned_for_session(&state.pool, session_id, false).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            error!(error = %e, "failed to unpin stream session");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// GET /api/v1/admin/stream/recent — recently streamed sessions with media info
pub async fn stream_recent(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    #[derive(Serialize)]
    struct RecentSession {
        id: Uuid,
        media_type: String,
        media_id: Uuid,
        release_title: String,
        debrid_provider: String,
        filename: String,
        filesize: i64,
        pinned: bool,
        progress_seconds: Option<f64>,
        created_at: String,
        updated_at: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        media_title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        media_image_url: Option<String>,
    }

    let rows = sqlx::query!(
        r#"
        SELECT
            s.id, s.media_type, s.media_id, s.release_title,
            s.debrid_provider, s.filename, s.filesize, s.pinned,
            s.progress_seconds, s.created_at, s.updated_at,
            COALESCE(m.title, e.title) AS "media_title?",
            COALESCE(m.images->>'poster', sh.images->>'poster') AS "media_image_url?"
        FROM debrid_stream_sessions s
        LEFT JOIN movies m ON s.media_type = 'movie' AND s.media_id = m.id
        LEFT JOIN tv_episodes e ON s.media_type = 'episode' AND s.media_id = e.id
        LEFT JOIN tv_seasons sn ON e.season_id = sn.id
        LEFT JOIN tv_shows sh ON sn.show_id = sh.id
        WHERE s.user_id = $1
        ORDER BY s.updated_at DESC
        LIMIT 10
        "#,
        auth.user_id,
    )
    .fetch_all(&state.pool)
    .await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "failed to fetch recent streams");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "database error"})),
            )
                .into_response();
        }
    };

    let items: Vec<RecentSession> = rows
        .into_iter()
        .map(|r| RecentSession {
            id: r.id,
            media_type: r.media_type,
            media_id: r.media_id,
            release_title: r.release_title,
            debrid_provider: r.debrid_provider,
            filename: r.filename,
            filesize: r.filesize,
            pinned: r.pinned,
            progress_seconds: r.progress_seconds,
            created_at: r
                .created_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
            updated_at: r
                .updated_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
            media_title: r.media_title,
            media_image_url: r.media_image_url,
        })
        .collect();

    Json(json!({ "sessions": items })).into_response()
}

// =============================================================================
// Stremio API Key CRUD
// =============================================================================

/// GET /api/v1/admin/settings/stremio-keys — list all keys for current user
pub async fn list_stremio_keys(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    let rows = sqlx::query!(
        r#"
        SELECT id, api_key, name, created_at
        FROM stremio_api_keys
        WHERE user_id = $1
        ORDER BY created_at DESC
        "#,
        auth.user_id,
    )
    .fetch_all(&state.pool)
    .await;

    match rows {
        Ok(rows) => {
            let keys: Vec<StremioKeyResponse> = rows
                .into_iter()
                .map(|r| StremioKeyResponse {
                    id: r.id,
                    api_key: r.api_key,
                    name: r.name,
                    created_at: r
                        .created_at
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default(),
                })
                .collect();
            Json(json!({ "keys": keys })).into_response()
        }
        Err(e) => {
            error!(error = %e, "failed to list stremio keys");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "database error"})),
            )
                .into_response()
        }
    }
}

/// POST /api/v1/admin/settings/stremio-keys — create a new key
pub async fn create_stremio_key(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<CreateStremioKeyBody>,
) -> impl IntoResponse {
    let api_key = Uuid::new_v4().to_string();
    let id = Uuid::now_v7();

    match sqlx::query!(
        r#"
        INSERT INTO stremio_api_keys (id, user_id, api_key, name)
        VALUES ($1, $2, $3, $4)
        RETURNING created_at
        "#,
        id,
        auth.user_id,
        api_key,
        body.name,
    )
    .fetch_one(&state.pool)
    .await
    {
        Ok(row) => Json(StremioKeyResponse {
            id,
            api_key,
            name: body.name,
            created_at: row
                .created_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
        })
        .into_response(),
        Err(e) => {
            error!(error = %e, "failed to create stremio key");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "database error"})),
            )
                .into_response()
        }
    }
}

/// DELETE /api/v1/admin/settings/stremio-keys/:id — revoke a key
pub async fn delete_stremio_key(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(key_id): Path<Uuid>,
) -> impl IntoResponse {
    let result = sqlx::query!(
        "DELETE FROM stremio_api_keys WHERE id = $1 AND user_id = $2",
        key_id,
        auth.user_id,
    )
    .execute(&state.pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() == 0 => (StatusCode::NOT_FOUND, "key not found").into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            error!(error = %e, "failed to delete stremio key");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================
