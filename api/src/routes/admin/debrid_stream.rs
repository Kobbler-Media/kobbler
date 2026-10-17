//! Debrid stream route handlers.
//!
//! Provides endpoints for searching, resolving, and proxying debrid streams.
//! Stream sessions are persisted in `debrid_stream_sessions` for resume/replay.

use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response, Sse},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::ffmpeg::mkv_cue_cache::{
    CueTrimPolicy, patch_for_local_file, patch_for_remote, patched_stream,
};
use crate::ffmpeg::mkv_cue_trim::CuePatch;
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::jobs::JobContext;
use crate::repositories::{
    movie_files, movie_watch_states, movies, tv_episode_files, tv_episodes, tv_seasons, tv_shows,
    tv_watch_states,
};
use crate::routes::admin::streaming::{AudioTrackResponse, SessionResponse};
use crate::routes::helpers::build_kobload_client;
use crate::routes::{AuthUser, PlaybackAuth};
use crate::streaming::decision::{AAC_ENCODER, PlaybackMethod, ViewerPrefs};
use crate::streaming::progressive::{ProgressiveDownloadConfig, ProgressiveDownloadManager};
use crate::video_metadata::thesportsdb::parse_sport_event_details;

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

    let request = if let Some(movie_id) = body.movie_id {
        match build_movie_search_request(&state, movie_id).await {
            Ok(r) => r,
            Err(resp) => return resp,
        }
    } else if let Some(episode_id) = body.episode_id {
        match build_episode_search_request(&state, episode_id).await {
            Ok(r) => r,
            Err(resp) => return resp,
        }
    } else if let Some(title) = body.title {
        // Raw metadata search — discover page, no DB entity yet.
        // TV requires an explicit season+episode: show-level imdb_id searches
        // can never return a specific playable file, so we reject them here
        // rather than silently downgrading to a movie-style search.
        let media_type = body.media_type.as_deref().unwrap_or("movie");
        let media_want = if media_type == "tv" {
            let (Some(season), Some(episode)) = (body.season, body.episode) else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": "tv stream search requires both 'season' and 'episode' — show-level searches are not playable"
                    })),
                )
                    .into_response();
            };
            let identifiers = nats_jobs::TvIdentifiers {
                tmdb_id: body.tmdb_id.map(|id| id.to_string()),
                tvdb_id: None,
                imdb_id: body.imdb_id.clone(),
            };
            nats_jobs::MediaWant::TvEpisode {
                show_name: title,
                season: season.to_string(),
                episode: episode.to_string(),
                year: body.year.map(|y| y as u32),
                air_date: None,
                identifiers: Some(identifiers),
                alternate_titles: None,
                episode_title: None,
                quality_profile_id: None,
                quality_overrides: None,
            }
        } else {
            let identifiers = nats_jobs::MovieIdentifiers {
                tmdb_id: body.tmdb_id.map(|id| id.to_string()),
                imdb_id: body.imdb_id.clone(),
            };
            nats_jobs::MediaWant::Movie {
                title,
                year: body.year.map(|y| y as u32),
                release_date: None,
                identifiers: Some(identifiers),
                alternate_titles: None,
                quality_profile_id: None,
                quality_overrides: None,
                in_library: None,
                library_video_height: None,
            }
        };
        nats_jobs::StreamSearchRequest {
            media_want: Some(media_want),
            query: String::new(),
            media_type: Some(media_type.to_string()),
            quality_profile_id: None,
            limit: None,
            include_uncached: None,
        }
    } else if let Some(raw) = body.raw {
        raw
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "provide movie_id, episode_id, title, or a raw search request"})),
        )
            .into_response();
    };

    // Determine media_type + media_id for preferred stream lookup
    let preferred = if let Some(movie_id) = body.movie_id {
        find_preferred_stream(&state.pool, "movie", movie_id).await
    } else if let Some(episode_id) = body.episode_id {
        find_preferred_stream(&state.pool, "episode", episode_id).await
    } else {
        None
    };

    match client.stream_search(&request).await {
        Ok(resp) => {
            let mut value = serde_json::to_value(resp).unwrap_or_default();
            if let Some((hash, file_id)) = preferred
                && let Some(obj) = value.as_object_mut()
            {
                obj.insert("preferred_info_hash".to_string(), json!(hash));
                if let Some(fid) = file_id {
                    obj.insert("preferred_file_id".to_string(), json!(fid));
                }
            }
            Json(value).into_response()
        }
        Err(e) => {
            error!(error = %e, "stream search failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("stream search failed: {e}")})),
            )
                .into_response()
        }
    }
}

/// Find the preferred (last successfully used) stream for a media item.
///
/// Prioritizes streams with a local cache file, then falls back to the most
/// recent session. Returns `(info_hash, file_id)`.
pub async fn find_preferred_stream(
    pool: &sqlx::PgPool,
    media_type: &str,
    media_id: Uuid,
) -> Option<(String, Option<i32>)> {
    #[derive(sqlx::FromRow)]
    struct PrefRow {
        info_hash: Option<String>,
        file_id: Option<i32>,
        cache_path: Option<String>,
    }

    let rows: Vec<PrefRow> = sqlx::query_as(
        r#"SELECT info_hash, file_id, cache_path
           FROM debrid_stream_sessions
           WHERE media_type = $1 AND media_id = $2 AND info_hash IS NOT NULL
           ORDER BY
             CASE WHEN cache_path IS NOT NULL THEN 0 ELSE 1 END,
             created_at DESC
           LIMIT 5"#,
    )
    .bind(media_type)
    .bind(media_id)
    .fetch_all(pool)
    .await
    .ok()?;

    for row in rows {
        let hash = row.info_hash?;
        // If it claims a local cache, verify the file still exists
        if let Some(ref path) = row.cache_path {
            if tokio::fs::metadata(path).await.is_ok() {
                return Some((hash, row.file_id));
            }
            // Cache file gone — try next row
            continue;
        }
        // No cache_path — use as fallback (most recent session)
        return Some((hash, row.file_id));
    }

    None
}

/// Build the dedup key for a debrid session from (info_hash, file_id).
fn debrid_cache_key(info_hash: Option<&str>, file_id: Option<i32>) -> Option<String> {
    let hash = info_hash?;
    Some(match file_id {
        Some(fid) => format!("{hash}_f{fid}"),
        None => hash.to_string(),
    })
}

/// Build a SessionResponse for an existing live session (dedup hit).
fn existing_session_response(
    session_id: Uuid,
    state: &AppState,
) -> Option<axum::response::Response> {
    let mut session = state.playback_sessions.get_mut(&session_id)?;

    // Don't reuse sessions with errors (e.g. crashed FFmpeg)
    if session.error.is_some() {
        return None;
    }

    // Don't reuse sessions where FFmpeg died and the decision requires transcoding.
    // The file_path is empty until probe completes — that's fine (still pending).
    // But if probe is done (file_path set) and no transcode is running for a
    // non-DirectPlay session, FFmpeg crashed and segments will never appear.
    let probe_done = !session.file_path.as_os_str().is_empty();
    let needs_transcode = session.decision.method != PlaybackMethod::DirectPlay;
    let ffmpeg_dead = session
        .transcode
        .as_mut()
        .map(|t| !t.is_running())
        .unwrap_or(true); // None = no transcode at all
    if probe_done && needs_transcode && ffmpeg_dead {
        return None;
    }

    let base_url = format!("/api/v1/admin/playback/sessions/{session_id}");
    let is_direct_play = session.decision.method == PlaybackMethod::DirectPlay;
    let direct_url = if is_direct_play {
        Some(format!("/api/v1/admin/stream/{session_id}/play"))
    } else {
        None
    };
    let playlist_url = if is_direct_play {
        None
    } else {
        Some(format!("{base_url}/master.m3u8"))
    };
    let audio_tracks = crate::routes::admin::streaming::build_audio_responses(
        &session.audio_metas,
        session.decision.audio_stream_index,
    );
    let selected_audio_index = Some(session.decision.audio_stream_index);
    let (sw, sh) = session.source_resolution;
    Some(
        Json(SessionResponse {
            session_id: session_id.to_string(),
            method: session.decision.method,
            playlist_url,
            direct_url,
            duration_seconds: session.duration_seconds,
            subtitles: vec![],
            audio_tracks,
            thumbnails_url: None,
            lufs: None,
            chapters: vec![],
            skip_segments: vec![],
            start_time: None,
            media_id: None,
            media_type: None,
            file_id: None,
            selected_audio_index,
            selected_subtitle_index: None,
            video_width: if sw > 0 { Some(sw) } else { None },
            video_height: if sh > 0 { Some(sh) } else { None },
        })
        .into_response(),
    )
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
    let runtime = state.runtime_config.get().await;

    // === Session dedup: prevent duplicate sessions for the same torrent file ===
    let cache_key = debrid_cache_key(body.info_hash.as_deref(), body.file_id);

    // Hold the resolve lock across the entire session creation to prevent races.
    // The guard must live until after the index insert, so it's in the function scope.
    let _debrid_resolve_guard = if let Some(ref key) = cache_key {
        // Fast path: check without locking
        if let Some(existing_id) = state.debrid_session_index.get(key).map(|e| *e) {
            if let Some(mut session) = state.playback_sessions.get_mut(&existing_id) {
                session.touch();
                drop(session);
                info!(session_id = %existing_id, cache_key = %key, "debrid session dedup hit (fast path)");
                if let Some(resp) = existing_session_response(existing_id, &state) {
                    return resp;
                }
            }
            // Session gone from memory — stale index entry
            state.debrid_session_index.remove(key);
        }

        // Serialize concurrent resolves for the same key.
        // `lock_owned()` captures the Arc, so `guard` is self-contained.
        let guard = state
            .debrid_resolve_locks
            .entry(key.clone())
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone()
            .lock_owned()
            .await;

        // Double-check after acquiring lock
        if let Some(existing_id) = state.debrid_session_index.get(key).map(|e| *e) {
            if let Some(mut session) = state.playback_sessions.get_mut(&existing_id) {
                session.touch();
                drop(session);
                info!(session_id = %existing_id, cache_key = %key, "debrid session dedup hit (after lock)");
                if let Some(resp) = existing_session_response(existing_id, &state) {
                    return resp;
                }
            }
            state.debrid_session_index.remove(key);
        }

        Some(guard)
    } else {
        None
    };

    // Check for a fully cached file before hitting debrid at all.
    // Uses the DB to look up (info_hash, file_id) → cache_path from a previous session,
    // then verifies the file exists and is complete on disk.
    if let Some(ref info_hash) = body.info_hash {
        // Find a previous session for this exact torrent file
        let cached: Option<(String, String, i64)> = sqlx::query_as(
            r#"SELECT cache_path, filename, filesize
               FROM debrid_stream_sessions
               WHERE info_hash = $1
                 AND ($2::INT IS NULL AND file_id IS NULL OR file_id = $2)
                 AND cache_path IS NOT NULL
               ORDER BY created_at DESC LIMIT 1"#,
        )
        .bind(info_hash)
        .bind(body.file_id)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten();

        if let Some((cache_path, filename, filesize)) = cached {
            let path = PathBuf::from(&cache_path);
            if let Ok(meta) = tokio::fs::metadata(&path).await {
                let size = meta.len();
                if filesize > 0 && size >= filesize as u64 {
                    info!(
                        info_hash = %info_hash,
                        file = %filename,
                        size_mb = size / (1024 * 1024),
                        "Using fully cached file — skipping debrid"
                    );
                    // This path bypasses resolve_or_create_media_id entirely, so
                    // enrichment has to be reached from here too — otherwise a
                    // title whose file happens to be on disk never catches up.
                    if let Some(media_id) = body.media_id.filter(|id| !id.is_nil()) {
                        enqueue_stream_enrich_for_media(&state, media_id, &body.media_type).await;
                    }
                    return create_local_cached_session(
                        &state, &runtime, auth, &body, path, filename, filesize,
                    )
                    .await
                    .into_response();
                }
            }
        }
    }

    let client = match build_kobload_client(&runtime) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let resolve_req = nats_jobs::StreamResolveRequest {
        debrid_provider_id: body.debrid_provider_id,
        info_hash: body.info_hash.clone(),
        magnet_url: body.magnet_url.clone(),
        nzb_url: body.nzb_url.clone(),
        file_id: body.file_id,
    };

    let resolved = match client.stream_resolve(&resolve_req).await {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "stream resolve failed");
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("stream resolve failed: {e}")})),
            )
                .into_response();
        }
    };

    // If not cached, return immediately so frontend can poll for progress
    if !resolved.cached {
        info!(
            torrent_id = ?resolved.torrent_id,
            "Stream not cached, returning caching status for client polling"
        );
        return Json(json!({
            "session_id": serde_json::Value::Null,
            "status": "caching",
            "debrid_provider_id": body.debrid_provider_id,
            "torrent_id": resolved.torrent_id,
        }))
        .into_response();
    }

    // Resolve the media_id — auto-create entity if needed (discover page flow)
    // Returns (media_id, effective_media_type) — media_type may be upgraded
    // from "tv" to "episode" when an episode entity is auto-created.
    let (media_id, effective_media_type) = match resolve_or_create_media_id(&state, &body).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };

    enqueue_stream_enrich_for_media(&state, media_id, &effective_media_type).await;

    // Set streamed_at on the entity (first stream only)
    match effective_media_type.as_str() {
        "movie" => {
            let _ = sqlx::query!(
                "UPDATE movies SET streamed_at = COALESCE(streamed_at, NOW()) WHERE id = $1",
                media_id,
            )
            .execute(&state.pool)
            .await;
        }
        "episode" => {
            // Look up show via episode→season→show
            let _ = sqlx::query!(
                r#"
                UPDATE tv_shows SET streamed_at = COALESCE(streamed_at, NOW())
                WHERE id = (
                    SELECT ts.show_id FROM tv_episodes te
                    JOIN tv_seasons ts ON ts.id = te.season_id
                    WHERE te.id = $1
                )
                "#,
                media_id,
            )
            .execute(&state.pool)
            .await;
        }
        _ => {}
    }

    // Persist the session (use effective_media_type so TV episodes get "episode" not "tv")
    let session_id = match sqlx::query_scalar::<_, Uuid>(
        r#"INSERT INTO debrid_stream_sessions
            (user_id, media_type, media_id, release_title, info_hash,
             debrid_provider, quality, filename, filesize, file_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        RETURNING id"#,
    )
    .bind(auth.user_id)
    .bind(&effective_media_type)
    .bind(media_id)
    .bind(&body.release_title)
    .bind(&body.info_hash)
    .bind(&body.debrid_provider)
    .bind(&body.quality)
    .bind(&resolved.filename)
    .bind(resolved.filesize)
    .bind(body.file_id)
    .fetch_one(&state.pool)
    .await
    {
        Ok(id) => id,
        Err(e) => {
            error!(error = %e, "failed to create stream session");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to create session"})),
            )
                .into_response();
        }
    };

    // Cache the resolved debrid URL on the session
    let expires_at = resolved.expires_at.as_deref().and_then(|s| {
        time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
    });
    let _ = sqlx::query!(
        r#"
        UPDATE debrid_stream_sessions
        SET debrid_url = $1, debrid_url_expires_at = $2, updated_at = NOW()
        WHERE id = $3
        "#,
        resolved.stream_url,
        expires_at,
        session_id,
    )
    .execute(&state.pool)
    .await;

    let container = resolved
        .filename
        .rsplit('.')
        .next()
        .unwrap_or("mkv")
        .to_lowercase();

    // Reject non-video files (e.g. .exe fake releases) so the frontend fallback
    // loop tries the next stream option instead of attempting to probe garbage.
    const VIDEO_CONTAINERS: &[&str] = &[
        "mkv", "mp4", "avi", "wmv", "webm", "m4v", "flv", "mov", "mpg", "mpeg", "ts", "m2ts", "vob",
    ];
    if !VIDEO_CONTAINERS.contains(&container.as_str()) {
        warn!(
            session_id = %session_id,
            filename = %resolved.filename,
            container = %container,
            "Rejected non-video file from debrid resolve"
        );
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "error": format!("Resolved file is not a video (.{container})")
            })),
        )
            .into_response();
    }

    info!(session_id = %session_id, filename = %resolved.filename, "created debrid stream session");

    // Use estimated duration from metadata (TMDB runtime) so we can return
    // immediately. The probe will update with the real duration later.
    let estimated_duration = body.estimated_duration.unwrap_or(0.0);

    // Create a "pending" playback session — no transcode yet, just enough
    // for the playlist handler to return an EVENT playlist while we probe.
    let rt_config = state.runtime_config.get().await;
    let segment_dir = rt_config
        .transcode_cache_dir
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(&state.config.storage_root)
                .join("cache")
                .join("transcode")
        })
        .join(session_id.to_string());

    let mut segment_cache = crate::streaming::segment_cache::SegmentCache::new(
        segment_dir.clone(),
        6,
        estimated_duration,
    );
    if let Some(st) = body.start_time {
        segment_cache.set_playback_start(segment_cache.time_to_segment(st), st);
    }

    let download_cancel = CancellationToken::new();

    // Optimistic initial decision: if the client's capabilities include the
    // container, assume DirectPlay. The background probe refines this once
    // codecs are known. Clients that can play anything (Stremio, native apps)
    // get DirectPlay immediately without waiting for the probe.
    let initial_method = if let Some(ref caps) = body.capabilities {
        use crate::streaming::decision::is_container_compatible;
        if is_container_compatible(&container, &caps.containers) {
            PlaybackMethod::DirectPlay
        } else {
            PlaybackMethod::Remux
        }
    } else {
        PlaybackMethod::Remux
    };

    let playback_session = crate::streaming::session::PlaybackSession {
        id: session_id,
        user_id: auth.user_id,
        media_type: crate::streaming::session::MediaType::Movie,
        file_id: session_id,
        file_path: PathBuf::new(), // will be updated after probe
        decision: crate::streaming::decision::PlaybackDecision {
            method: initial_method,
            video_mode: crate::streaming::decision::VideoMode::Copy,
            audio_mode: crate::streaming::decision::AudioMode::Transcode {
                target_codec: AAC_ENCODER.to_string(),
                bitrate_kbps: 192,
            },
            audio_stream_index: 0,
            subtitle_handling: crate::streaming::decision::SubtitleHandling::None,
        },
        transcode: None, // will be set after probe+decision
        segment_cache,
        created_at: std::time::Instant::now(),
        last_activity: std::time::Instant::now(),
        seek_generation: 0,
        duration_seconds: estimated_duration,
        container: container.clone(),
        video_codec: String::new(),
        subtitle_files: vec![],
        hw_accel: (*state.hw_accel).clone(),
        source_resolution: (1920, 1080),
        variants: None,
        is_live: false,
        provider_id: None,
        max_bitrate_kbps: None,
        keyframes: None,
        error: None,
        pending_segment: None,
        download_cancel: Some(download_cancel.clone()),
        thumbnail_dir: None, // set once cache_path is known after download
        debrid_cache_key: cache_key.clone(),
        audio_metas: vec![], // populated after background probe
        force_video_transcode: false,
        corrupt_ranges: Vec::new(),
        overlay_spawned_starts: std::collections::HashSet::new(),
        highest_served_segment: None,
    };

    state.playback_sessions.insert(session_id, playback_session);

    // Register in the dedup index so concurrent requests reuse this session
    if let Some(ref key) = cache_key {
        state.debrid_session_index.insert(key.clone(), session_id);
    }

    // Return immediately — the player can start loading the playlist.
    // The EVENT playlist will have estimated segments; serve_segment will
    // block waiting for the actual files to appear on disk.
    let base_url = format!("/api/v1/admin/playback/sessions/{session_id}");
    let start_time = body.start_time; // captured by background task for FFmpeg seek
    let (direct_url, response_start_time) = if initial_method == PlaybackMethod::DirectPlay {
        // DirectPlay: no server-side seek; frontend resumes via initialPosition
        (
            Some(format!("/api/v1/admin/stream/{session_id}/play")),
            None,
        )
    } else {
        (None, body.start_time)
    };
    let playlist_url = if initial_method == PlaybackMethod::DirectPlay {
        None
    } else {
        Some(format!("{base_url}/master.m3u8"))
    };
    let response = Json(SessionResponse {
        session_id: session_id.to_string(),
        method: initial_method,
        playlist_url,
        direct_url,
        duration_seconds: estimated_duration,
        subtitles: vec![],
        audio_tracks: vec![],
        thumbnails_url: None,
        lufs: None,
        chapters: vec![],
        skip_segments: vec![],
        start_time: response_start_time,
        media_id: Some(media_id.to_string()),
        media_type: Some(effective_media_type),
        file_id: None,
        selected_audio_index: None,
        selected_subtitle_index: None,
        video_width: None,
        video_height: None,
    })
    .into_response();

    // Background: download + probe + decide + start transcode.
    // Updates the playback session in-place when ready.
    let cache_dir = rt_config
        .debrid_cache_dir
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(&state.config.storage_root).join("debrid_cache")
        });
    // Cache by info_hash so replaying the same torrent resumes the partial download
    // rather than starting fresh. Falls back to session_id if no hash.
    let session_id_str = session_id.to_string();
    let cache_key = body.info_hash.as_deref().unwrap_or(&session_id_str);
    let local_cache_path = cache_dir.join(cache_key).join(&resolved.filename);
    let download_progress = Arc::new(kob_ffmpeg::hybrid_io::DownloadProgress::new(
        if resolved.filesize > 0 {
            Some(resolved.filesize as u64)
        } else {
            None
        },
    ));
    // Seed progress with existing partial download so HybridIo reads from disk
    if let Ok(meta) = tokio::fs::metadata(&local_cache_path).await {
        let existing = meta.len();
        if existing > 0 {
            download_progress.update(existing);
            if resolved.filesize > 0 && existing >= resolved.filesize as u64 {
                download_progress.mark_complete();
                info!(session_id = %session_id, size_mb = existing / (1024 * 1024), "Using completed cached file");
            } else {
                info!(session_id = %session_id, cached_mb = existing / (1024 * 1024), "Resuming from partial cache");
            }
        }
    }
    let capabilities = body.capabilities.unwrap_or_default();
    let bg_state = state.clone();
    let bg_stream_url = resolved.stream_url.clone();
    let bg_container = container;
    let bg_segment_dir = segment_dir;

    tokio::spawn(async move {
        // Start progressive download
        let dl_config = ProgressiveDownloadConfig {
            cache_dir: cache_dir.clone(),
            default_ttl_secs: rt_config.debrid_cache_ttl_secs,
            max_cache_bytes: rt_config.debrid_cache_max_bytes,
        };
        let dl_client = crate::http::builder()
            .timeout(std::time::Duration::from_secs(3600))
            .build()
            .expect("download client");
        let manager = Arc::new(ProgressiveDownloadManager::new(
            dl_config,
            bg_state.pool.clone(),
            dl_client,
            bg_state.nats.publisher().clone(),
        ));
        let cancel = download_cancel.clone();
        // Stops the size poller when the downloader finishes, whatever the
        // outcome. `complete` is only set on success, so without this a failed
        // download leaves the poller spinning a stat() every 500ms forever.
        let poll_stop = CancellationToken::new();
        let poll_stop_setter = poll_stop.clone();
        let dl_url = bg_stream_url.clone();
        let dl_cache_path = local_cache_path.clone();
        let dl_filesize = resolved.filesize;
        let dl_progress = download_progress.clone();
        tokio::spawn({
            let manager = manager.clone();
            let cancel_check = cancel.clone();
            async move {
                let outcome = manager
                    .start_download(session_id, dl_url, dl_cache_path, dl_filesize, cancel)
                    .await;
                // Only mark complete if the file actually landed in full. Marking
                // complete after a failed download makes HybridIo read the short
                // (or empty) cache file and return a hard EOF instead of falling
                // back to HTTP — the probe then dies on a perfectly good link.
                match outcome {
                    Ok(()) if !cancel_check.is_cancelled() => dl_progress.mark_complete(),
                    Ok(()) => {}
                    Err(e) => {
                        warn!(
                            session_id = %session_id,
                            error = %e,
                            "Download did not complete; leaving HybridIo on HTTP fallback"
                        );
                    }
                }
                poll_stop_setter.cancel();
            }
        });
        let poll_progress = download_progress.clone();
        let poll_path = local_cache_path.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = poll_stop.cancelled() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
                }
                if poll_progress.is_complete() {
                    break;
                }
                if let Ok(meta) = tokio::fs::metadata(&poll_path).await {
                    poll_progress.update(meta.len());
                }
            }
            // Final read: a download that stopped short still leaves valid bytes
            // on disk, and HybridIo should serve those locally rather than
            // re-fetching them over HTTP.
            if let Ok(meta) = tokio::fs::metadata(&poll_path).await {
                poll_progress.update(meta.len());
            }
        });

        // Check for cached probe sidecar (only valid for fully-downloaded files)
        let sidecar_path = probe_sidecar_path(&local_cache_path);
        let cached_probe = if download_progress.is_complete() {
            match tokio::fs::read_to_string(&sidecar_path).await {
                Ok(json) => {
                    match serde_json::from_str::<crate::ffmpeg::transcode::HybridProbeInfo>(&json) {
                        Ok(probe) => {
                            info!(session_id = %session_id, "Using cached probe sidecar");
                            Some(probe)
                        }
                        Err(e) => {
                            warn!(session_id = %session_id, error = %e, "Invalid probe sidecar, re-probing");
                            None
                        }
                    }
                }
                Err(_) => None,
            }
        } else {
            None
        };

        // Either use the cached probe or probe via HybridIo
        let (probe_info, probe_result) = if let Some(cached) = cached_probe {
            (cached, None)
        } else {
            let result = match crate::ffmpeg::transcode::TranscodeProcess::probe_hybrid(
                local_cache_path.clone(),
                bg_stream_url.clone(),
                download_progress.clone(),
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    error!(session_id = %session_id, error = %e, "Background probe failed");
                    if let Some(mut session) = bg_state.playback_sessions.get_mut(&session_id) {
                        session.error = Some(crate::streaming::session::SessionError::new(
                            "probe_failed",
                            format!("Probe failed: {e}"),
                        ));
                    }
                    return;
                }
            };

            // Write probe sidecar for future plays of this cached file
            if download_progress.is_complete() {
                match serde_json::to_string(&result.probe) {
                    Ok(json) => {
                        if let Err(e) = tokio::fs::write(&sidecar_path, json).await {
                            warn!(session_id = %session_id, error = %e, "Failed to write probe sidecar");
                        }
                    }
                    Err(e) => {
                        warn!(session_id = %session_id, error = %e, "Failed to serialize probe sidecar");
                    }
                }
            }

            let info = crate::ffmpeg::transcode::HybridProbeInfo {
                duration_seconds: result.probe.duration_seconds,
                video_codec: result.probe.video_codec.clone(),
                audio_codec: result.probe.audio_codec.clone(),
                audio_stream_index: result.probe.audio_stream_index,
                video_width: result.probe.video_width,
                video_height: result.probe.video_height,
                audio_tracks: result
                    .probe
                    .audio_tracks
                    .iter()
                    .map(|a| crate::ffmpeg::transcode::HybridAudioTrack {
                        stream_index: a.stream_index,
                        codec: a.codec.clone(),
                        channels: a.channels,
                        language: a.language.clone(),
                    })
                    .collect(),
            };
            (info, Some(result))
        };

        let duration = probe_info.duration_seconds;
        let video_codec = probe_info.video_codec.clone();
        let video_width = probe_info.video_width;
        let video_height = probe_info.video_height;

        info!(
            session_id = %session_id,
            duration,
            video_codec = %video_codec,
            "Debrid stream probed (background)"
        );

        // Decision engine
        let file_meta = crate::streaming::decision::FileMeta {
            container: bg_container,
            video_codec: video_codec.clone(),
            video_width,
            video_height,
            interlaced: false,
        };
        let audio_metas: Vec<crate::streaming::decision::AudioTrackMeta> = probe_info
            .audio_tracks
            .iter()
            .map(|a| crate::streaming::decision::AudioTrackMeta {
                stream_index: a.stream_index,
                codec: a.codec.clone(),
                channels: a.channels,
                language: a.language.clone(),
                title: None,
                is_default: false,
            })
            .collect();
        // Resolve preferred audio from language settings
        let bg_config = bg_state.runtime_config.get().await;
        let preferred_audio = crate::streaming::decision::find_preferred_audio(
            &audio_metas,
            &bg_config.preferred_audio_language,
        );

        let decision = crate::streaming::decision::decide(
            &file_meta,
            &audio_metas,
            &[],
            &capabilities,
            ViewerPrefs {
                preferred_audio_index: preferred_audio,
                preferred_subtitle_index: None,
                understood_language: &bg_config.preferred_audio_language,
            },
            &bg_state.encoder_caps.preferred_codecs(),
        );

        info!(session_id = %session_id, method = ?decision.method, "Debrid playback decision (background)");

        // Stamp probe-derived state onto the in-memory session IMMEDIATELY so
        // that master.m3u8 / stream.m3u8 handlers stop returning 503 as soon as
        // the probe finishes. Previously these fields were only set after
        // ffmpeg passed a health check (see below), opening a long window where
        // `probe_complete` SSE had already fired but the playlist handler
        // still rejected the first hls.js request.
        //
        // Remux/Transcode path sets `session.transcode` further down once the
        // encoder is healthy; the playlist handler's estimated path works fine
        // with `transcode = None` (3 placeholder segments, serve_segment
        // block-waits for the files to appear).
        {
            let total_segments = (duration / 6.0).ceil() as u32;
            if let Some(mut session) = bg_state.playback_sessions.get_mut(&session_id) {
                session.decision = decision.clone();
                session.duration_seconds = duration;
                session.segment_cache.duration_seconds = duration;
                session.segment_cache.total_segments = total_segments;
                session.video_codec = video_codec.clone();
                session.source_resolution = (video_width, video_height);
                session.thumbnail_dir = Some(
                    local_cache_path
                        .parent()
                        .unwrap_or(&local_cache_path)
                        .join("media_cache")
                        .join("trickplay"),
                );
                session.audio_metas = audio_metas.clone();
                session.file_path = local_cache_path.clone();
            }
        }

        // Store probe data in DB so the SSE status endpoint can emit probe_complete.
        // Done after the decision so we can include the selected audio stream index
        // for is_default marking on the frontend.
        //
        // `method` is the REFINED decision from the probe. The initial session
        // response carries an optimistic method based on client capabilities,
        // and if the probe refines to something else (e.g. DirectPlay ->
        // AudioTranscode because the audio codec isn't actually supported),
        // the frontend needs to tear down its direct-play setup and switch
        // to HLS. Expose it here so the frontend can compare against the
        // initial method it received.
        let method_str = match decision.method {
            PlaybackMethod::DirectPlay => "direct_play",
            PlaybackMethod::Remux => "remux",
            PlaybackMethod::AudioTranscode => "audio_transcode",
            PlaybackMethod::VideoTranscode => "video_transcode",
        };
        let probe_json = serde_json::json!({
            "duration": duration,
            "video_codec": &video_codec,
            "video_width": video_width,
            "video_height": video_height,
            "method": method_str,
            "selected_audio_stream_index": decision.audio_stream_index,
            "audio_streams": probe_info.audio_tracks.iter().map(|a| serde_json::json!({
                "stream_index": a.stream_index,
                "codec": a.codec,
                "channels": a.channels,
                "language": a.language,
            })).collect::<Vec<_>>(),
        });
        let _ = sqlx::query(
            "UPDATE debrid_stream_sessions SET probe_data = $1, updated_at = NOW() WHERE id = $2",
        )
        .bind(&probe_json)
        .bind(session_id)
        .execute(&bg_state.pool)
        .await;

        // DirectPlay: client handles the container natively — no FFmpeg needed.
        // All session fields were set above; nothing more to do.
        if decision.method == PlaybackMethod::DirectPlay {
            info!(session_id = %session_id, "DirectPlay — skipping FFmpeg, serving raw file");
            return;
        }

        // Remux/Transcode: start FFmpeg to produce HLS segments.
        // Uses the same encoder fallback chain as the cached-file path:
        // primary (e.g. h264_nvenc) → vaapi → libx264.
        let primary_encoder = match &decision.video_mode {
            crate::streaming::decision::VideoMode::Transcode { target_codec } => bg_state
                .encoder_caps
                .encoder_for(target_codec)
                .unwrap_or(&bg_state.encoder_caps.h264)
                .to_string(),
            crate::streaming::decision::VideoMode::Copy => bg_state.encoder_caps.h264.clone(),
        };

        let needs_transcode = matches!(
            &decision.video_mode,
            crate::streaming::decision::VideoMode::Transcode { .. }
        );

        // Determine hw_device for the primary encoder
        let primary_hw_device: Option<kob_ffmpeg::hw::device::SharedHwDevice> =
            if primary_encoder == "libx264" {
                None
            } else {
                bg_state.hw_device.get()
            };

        // Try primary encoder via hybrid IO (only when we have the full probe result,
        // i.e. not using cached sidecar probe)
        let mut transcode = None;
        let mut fail_reason: Option<&'static str> = None;
        if let Some(probe_result) = probe_result {
            match crate::ffmpeg::transcode::TranscodeProcess::start_hybrid_from_probe(
                probe_result,
                &decision,
                bg_segment_dir.clone(),
                6,
                &primary_encoder,
                start_time,
                primary_hw_device,
            )
            .await
            {
                Ok(mut process) => {
                    // Health check: detect immediate encoder crashes (e.g. nvenc 10-bit issues)
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    match process.check_health() {
                        Ok(true) => {
                            transcode = Some(process);
                        }
                        Err(e) => {
                            warn!(
                                session_id = %session_id,
                                encoder = %primary_encoder,
                                error = %e,
                                "Hybrid encoder crashed immediately, will retry from cache"
                            );
                            fail_reason = Some("hybrid_health_fail");
                            process.kill().await;
                        }
                        Ok(false) => {
                            warn!(
                                session_id = %session_id,
                                encoder = %primary_encoder,
                                "Hybrid encoder exited immediately, will retry from cache"
                            );
                            fail_reason = Some("hybrid_health_fail");
                            process.kill().await;
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        session_id = %session_id,
                        encoder = %primary_encoder,
                        error = %e,
                        "Failed to start hybrid encoder, will retry from cache"
                    );
                    fail_reason = Some("hybrid_start_failed");
                }
            }
        } else {
            fail_reason = Some("no_probe_result");
        }

        // If the primary encoder failed, fall back using the cached-file path.
        // The hybrid IO is consumed, but the local cache file has enough data
        // for fallback encoders to start producing segments.
        //
        // For VideoMode::Transcode we walk the h264 fallback encoder chain.
        // For VideoMode::Copy (Remux / AudioTranscode) the encoder choice is
        // irrelevant — video is copied — so we make a single attempt with the
        // same encoder value (it's unused by ffmpeg in Copy mode).
        if transcode.is_none() {
            let (video_mode, encoder_chain): (crate::ffmpeg::transcode::VideoMode, Vec<String>) =
                if needs_transcode {
                    let mut chain: Vec<String> = bg_state
                        .encoder_caps
                        .h264_fallbacks()
                        .into_iter()
                        .map(String::from)
                        .collect();
                    // Don't retry the primary — it already failed
                    chain.retain(|e| e != &primary_encoder);
                    (crate::ffmpeg::transcode::VideoMode::Transcode, chain)
                } else {
                    // Video copy: one shot from cached file. target_encoder is
                    // unused by ffmpeg in Copy mode but we pass primary_encoder
                    // for logging consistency.
                    (
                        crate::ffmpeg::transcode::VideoMode::Copy,
                        vec![primary_encoder.clone()],
                    )
                };

            let audio_mode = match &decision.audio_mode {
                crate::streaming::decision::AudioMode::Transcode {
                    target_codec,
                    bitrate_kbps,
                } => crate::ffmpeg::transcode::AudioMode::Transcode {
                    target_codec: target_codec.clone(),
                    bitrate_kbps: *bitrate_kbps,
                },
                crate::streaming::decision::AudioMode::Copy => {
                    crate::ffmpeg::transcode::AudioMode::Copy
                }
            };

            for encoder in &encoder_chain {
                let _ = tokio::fs::remove_dir_all(&bg_segment_dir).await;

                let hw_device = if encoder == "libx264" {
                    None
                } else if encoder.contains("vaapi")
                    && matches!(*bg_state.hw_accel, crate::ffmpeg::detect::HwAccel::Nvenc)
                {
                    if let Some(ref dev) = bg_state.vaapi_fallback_device {
                        Some(dev.clone())
                    } else {
                        warn!(
                            session_id = %session_id,
                            "No VAAPI fallback device available, skipping VAAPI encoder"
                        );
                        continue;
                    }
                } else {
                    bg_state.hw_device.get()
                };

                let args = crate::ffmpeg::transcode::TranscodeArgs {
                    input_path: local_cache_path.clone(),
                    output_dir: bg_segment_dir.clone(),
                    video_mode,
                    audio_mode: audio_mode.clone(),
                    audio_stream_index: decision.audio_stream_index as i32,
                    start_time,
                    segment_duration: 6,
                    hw_accel: (*bg_state.hw_accel).clone(),
                    video_codec: video_codec.clone(),
                    target_encoder: encoder.clone(),
                    burn_in_subtitle: None,
                    scale: None,
                    target_bitrate_kbps: None,
                    duration_seconds: duration,
                    initial_generation: 0,
                    hw_device,
                };

                match crate::ffmpeg::transcode::TranscodeProcess::start(args).await {
                    Ok(mut process) => {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        match process.check_health() {
                            Ok(true) => {
                                info!(
                                    session_id = %session_id,
                                    encoder = %encoder,
                                    "Encoder fallback succeeded (hybrid→cached)"
                                );
                                transcode = Some(process);
                                break;
                            }
                            Err(e) => {
                                warn!(
                                    session_id = %session_id,
                                    encoder = %encoder,
                                    error = %e,
                                    "Fallback encoder crashed immediately, trying next"
                                );
                                process.kill().await;
                            }
                            Ok(false) => {
                                warn!(
                                    session_id = %session_id,
                                    encoder = %encoder,
                                    "Fallback encoder exited immediately, trying next"
                                );
                                process.kill().await;
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            session_id = %session_id,
                            encoder = %encoder,
                            error = %e,
                            "Failed to start fallback encoder, trying next"
                        );
                    }
                }
            }
        }

        let transcode = match transcode {
            Some(t) => t,
            None => {
                let reason = fail_reason.unwrap_or("unknown");
                error!(
                    session_id = %session_id,
                    primary_encoder = %primary_encoder,
                    fail_reason = %reason,
                    video_mode = ?decision.video_mode,
                    audio_mode = ?decision.audio_mode,
                    "All encoders failed"
                );
                if let Some(mut session) = bg_state.playback_sessions.get_mut(&session_id) {
                    session.error = Some(crate::streaming::session::SessionError::new(
                        "no_fallback_encoder",
                        format!("All encoders failed ({reason})"),
                    ));
                }
                return;
            }
        };

        // Attach the running transcode process to the session. All other
        // probe-derived fields were stamped immediately after the decision
        // above, so the playlist handler has been serving the estimated
        // playlist while ffmpeg was starting.
        if let Some(mut session) = bg_state.playback_sessions.get_mut(&session_id) {
            session.transcode = Some(transcode);
        }
    });

    response
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

async fn kids_may_resolve(pool: &sqlx::PgPool, body: &StreamResolveBody) -> bool {
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
#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
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

/// 5-minute buffer before considering a cached URL expired.
const URL_EXPIRY_BUFFER_SECS: i64 = 300;

/// Sessions are addressed by an opaque id, so every read and write of one must
/// still carry the owner predicate. `None` means admin — free to act on any
/// session, matching the device-ownership rule elsewhere.
fn session_owner(user_id: Uuid, role: &str) -> Option<Uuid> {
    (role != "admin").then_some(user_id)
}

/// GET /api/v1/admin/stream/:id/play
///
/// Proxies the debrid stream URL with Range request forwarding.
/// Uses a cached URL if still valid; otherwise re-resolves via Kobload.
pub async fn stream_play(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let session = match sqlx::query!(
        r#"
        SELECT id, info_hash, debrid_provider, filename, filesize,
               cache_path AS "cache_path?",
               debrid_url AS "debrid_url?",
               debrid_url_expires_at AS "debrid_url_expires_at?"
        FROM debrid_stream_sessions
        WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)
        "#,
        session_id,
        session_owner(auth.user_id, &auth.role),
    )
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(s)) => {
            info!(session_id = %session_id, cache_path = ?s.cache_path, has_debrid_url = s.debrid_url.is_some(), "stream_play: session found in DB");
            s
        }
        Ok(None) => {
            warn!(session_id = %session_id, "stream_play: session not found in DB");
            return (StatusCode::NOT_FOUND, "session not found").into_response();
        }
        Err(e) => {
            error!(session_id = %session_id, error = %e, "stream_play: database error");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Touch the in-memory session to keep it alive (idle detection uses this).
    // Check if download needs resuming (stopped by idle timeout).
    let needs_resume = if let Some(mut ps) = state.playback_sessions.get_mut(&session_id) {
        ps.touch();
        // Check if download was stopped and file is incomplete
        if let Some(ref cancel) = ps.download_cancel
            && cancel.is_cancelled()
        {
            let total = session.filesize.max(0) as u64;
            let downloaded = session
                .cache_path
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .map(|m| m.len())
                .unwrap_or(0);
            if downloaded < total {
                let new_cancel = CancellationToken::new();
                ps.download_cancel = Some(new_cancel.clone());
                Some(new_cancel)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };
    // DashMap guard is dropped here — safe to await below

    if let Some(new_cancel) = needs_resume {
        let cache_path = PathBuf::from(session.cache_path.as_deref().unwrap_or(""));
        let filesize = session.filesize;
        let pool = state.pool.clone();
        let nats_pub = state.nats.publisher().clone();
        let rt_config = state.runtime_config.get().await;
        let debrid_url = session.debrid_url.clone();
        let info_hash = session.info_hash.clone();
        let debrid_provider = session.debrid_provider.clone();
        let bg_state = state.clone();

        info!(session_id = %session_id, "Resuming stopped download");
        tokio::spawn(async move {
            let url = match resolve_fresh_debrid_url(
                &bg_state,
                &debrid_url,
                info_hash.as_deref(),
                &debrid_provider,
                session_id,
            )
            .await
            {
                Some(u) => u,
                None => return,
            };

            let dl_config = ProgressiveDownloadConfig {
                cache_dir: cache_path
                    .parent()
                    .unwrap_or(&cache_path)
                    .parent()
                    .unwrap_or(&cache_path)
                    .to_path_buf(),
                default_ttl_secs: rt_config.debrid_cache_ttl_secs,
                max_cache_bytes: rt_config.debrid_cache_max_bytes,
            };
            let dl_client = crate::http::builder()
                .timeout(std::time::Duration::from_secs(3600))
                .build()
                .expect("download client");
            let manager = Arc::new(ProgressiveDownloadManager::new(
                dl_config, pool, dl_client, nats_pub,
            ));
            // Fire-and-forget cache warm; playback is already being served from
            // the hybrid path, so a failure here only costs the local cache.
            let _ = manager
                .start_download(session_id, url, cache_path, filesize, new_cancel)
                .await;
        });
    }

    let is_direct_play = state
        .playback_sessions
        .get(&session_id)
        .is_some_and(|s| s.decision.method == PlaybackMethod::DirectPlay);

    let rt_cfg = state.runtime_config.get().await;
    let cue_policy = CueTrimPolicy {
        // Debrid sessions extract no sidecars, so the client renders subtitles from
        // the container and the language it will pick has to keep its index.
        sidecars_available: state
            .playback_sessions
            .get(&session_id)
            .is_some_and(|s| !s.subtitle_files.is_empty()),
        preferred_languages: vec![rt_cfg.preferred_subtitle_language.clone()],
    };

    // Hybrid serving: use local file where downloaded, proxy from debrid where not.
    // Same concept as HybridIo but at the HTTP response level.
    if let Some(ref cache_path) = session.cache_path {
        let path = PathBuf::from(cache_path);
        if let Ok(meta) = tokio::fs::metadata(&path).await {
            let downloaded = meta.len();
            let total = session.filesize.max(0) as u64;
            let fully_downloaded = downloaded >= total && total > 0;

            if fully_downloaded {
                info!(session_id = %session_id, "serving from complete local cache");
                let _ = sqlx::query!(
                    "UPDATE debrid_cache_files SET last_accessed_at = NOW() WHERE session_id = $1",
                    session_id,
                )
                .execute(&state.pool)
                .await;

                if !is_direct_play && needs_remux(&session.filename) {
                    return remux_local_file(&path).await;
                }
                let patch = patch_for_local_file(
                    &state.cue_trim_cache,
                    &path,
                    &session.filename,
                    &cue_policy,
                    rt_cfg.mkv_cue_trim_enabled,
                )
                .await;
                return serve_local_file(
                    &path,
                    session.filesize,
                    &session.filename,
                    &headers,
                    patch,
                )
                .await;
            }

            // Partial file — serve from local if the requested range is within
            // the downloaded portion, otherwise fall through to debrid proxy.
            // DirectPlay clients accept the raw container, so they can use
            // partial MKV/AVI bytes directly (no remux needed).
            if downloaded > 0 && (is_direct_play || !needs_remux(&session.filename)) {
                if let Some(range_header) = headers.get(header::RANGE)
                    && let Ok(range_str) = range_header.to_str()
                    && let Some(range) = parse_byte_range(range_str, total)
                {
                    // If the entire requested range is within the downloaded bytes,
                    // serve from the local file (fast, no network)
                    if range.end < downloaded {
                        // No trim while the download is in flight: the cue index sits
                        // at the tail, so it is not here yet and every attempt would
                        // re-read the head against a size that keeps changing.
                        return serve_local_file(
                            &path,
                            session.filesize,
                            &session.filename,
                            &headers,
                            None,
                        )
                        .await;
                    }
                } else if headers.get(header::RANGE).is_none() {
                    // No Range header (full file request) — if we have some data,
                    // serve what we have from the local file. The browser will
                    // see Content-Length = total and make Range requests for the
                    // rest, which we'll handle on subsequent requests.
                    // Only safe for MP4 where the moov atom is at the start.
                    // Skip this for now — the browser needs proper Content-Length.
                }
            }
        }
    }

    // Check if we have a cached URL that's still valid (with 5-minute buffer).
    // URLs without an expiry (debrid_url_expires_at IS NULL) are treated as
    // non-expiring — Real-Debrid often omits the expiry field.
    let now = OffsetDateTime::now_utc();
    let cached_url = session
        .debrid_url
        .as_deref()
        .filter(|url| !url.is_empty())
        .filter(|_| {
            session
                .debrid_url_expires_at
                .is_none_or(|exp| exp > now + time::Duration::seconds(URL_EXPIRY_BUFFER_SECS))
        })
        .map(String::from);

    let stream_url = if let Some(url) = cached_url {
        url
    } else {
        // Re-resolve to get a fresh streaming URL
        let runtime = state.runtime_config.get().await;
        let client = match build_kobload_client(&runtime) {
            Ok(c) => c,
            Err(e) => return e.into_response(),
        };

        let debrid_provider_id = match Uuid::parse_str(&session.debrid_provider) {
            Ok(id) => id,
            Err(_) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "session has invalid debrid_provider (not a UUID)",
                )
                    .into_response();
            }
        };

        let resolve_req = nats_jobs::StreamResolveRequest {
            debrid_provider_id,
            info_hash: session.info_hash.clone(),
            magnet_url: None,
            nzb_url: None,
            file_id: None,
        };

        let resolved = match client.stream_resolve(&resolve_req).await {
            Ok(r) => r,
            Err(e) => {
                error!(error = %e, "failed to re-resolve stream URL");
                return (StatusCode::BAD_GATEWAY, "failed to resolve stream URL").into_response();
            }
        };

        // Kobload returns cached=false with an empty stream_url when the
        // torrent is still being fetched by the debrid provider.  Don't
        // try to proxy an empty URL — tell the client to retry.
        if resolved.stream_url.is_empty() {
            warn!(session_id = %session_id, "re-resolve returned empty URL (torrent not yet cached on provider)");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "stream not ready — torrent is being fetched by debrid provider",
            )
                .into_response();
        }

        // Parse expires_at from the response and cache the URL
        let expires_at = resolved.expires_at.as_deref().and_then(|s| {
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
        });

        let _ = sqlx::query!(
            r#"
            UPDATE debrid_stream_sessions
            SET debrid_url = $1, debrid_url_expires_at = $2, updated_at = NOW()
            WHERE id = $3
            "#,
            resolved.stream_url,
            expires_at,
            session_id,
        )
        .execute(&state.pool)
        .await;

        if expires_at.is_some() {
            info!(session_id = %session_id, "Cached fresh debrid URL");
        } else {
            warn!(session_id = %session_id, "Resolved debrid URL has no expiry — caching without TTL");
        }

        resolved.stream_url
    };

    // Update the session timestamp
    let _ = sqlx::query!(
        "UPDATE debrid_stream_sessions SET updated_at = NOW() WHERE id = $1",
        session_id,
    )
    .execute(&state.pool)
    .await;

    if !is_direct_play && needs_remux(&session.filename) {
        return remux_proxy_stream(&stream_url).await;
    }

    let patch = match crate::http::builder()
        .timeout(Duration::from_secs(60))
        .build()
    {
        Ok(client) => {
            patch_for_remote(
                &state.cue_trim_cache,
                client,
                &stream_url,
                &format!(
                    "{}:{}:{}",
                    session.info_hash.as_deref().unwrap_or(""),
                    session.filename,
                    session.filesize
                ),
                &session.filename,
                &cue_policy,
                rt_cfg.mkv_cue_trim_enabled,
            )
            .await
        }
        Err(_) => None,
    };

    proxy_stream(&stream_url, session.filesize, &headers, patch).await
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
                        DO UPDATE SET position_seconds = $4, updated_at = NOW()
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
                        DO UPDATE SET position_seconds = $4, updated_at = NOW()
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

/// POST /api/v1/admin/stream/:id/ingest — ingest cached file into library
pub async fn stream_ingest(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(session_id): Path<Uuid>,
    Json(body): Json<IngestBody>,
) -> impl IntoResponse {
    // 1. Look up the session
    let session = match sqlx::query!(
        r#"
        SELECT media_type, media_id, cache_path AS "cache_path?", filename, filesize
        FROM debrid_stream_sessions
        WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)
        "#,
        session_id,
        session_owner(auth.user_id, &auth.role),
    )
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(s)) => s,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "session not found"})),
            )
                .into_response();
        }
        Err(e) => {
            error!(error = %e, "failed to fetch stream session for ingest");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "database error"})),
            )
                .into_response();
        }
    };

    // 2. Check cache_path exists and file is complete
    let cache_path = match session.cache_path {
        Some(ref p) if PathBuf::from(p).exists() => p.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "cached file not available — stream must be fully downloaded first"})),
            )
                .into_response();
        }
    };

    // Verify the cached file is complete (not a partial download)
    match tokio::fs::metadata(&PathBuf::from(&cache_path)).await {
        Ok(meta) => {
            if (meta.len() as i64) < session.filesize - 1024 {
                warn!(
                    session_id = %session_id,
                    expected = session.filesize,
                    actual = meta.len(),
                    "cached file appears incomplete"
                );
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "cached file appears incomplete — file size is smaller than expected"})),
                )
                    .into_response();
            }
        }
        Err(e) => {
            error!(error = %e, path = %cache_path, "failed to stat cached file");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to verify cached file"})),
            )
                .into_response();
        }
    }

    // 3. Look up media root path
    let media_root_path = match sqlx::query_scalar!(
        "SELECT path FROM media_roots WHERE id = $1",
        body.media_root_id,
    )
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(p)) => p,
        Ok(None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "media root not found"})),
            )
                .into_response();
        }
        Err(e) => {
            error!(error = %e, "failed to look up media root");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "database error"})),
            )
                .into_response();
        }
    };

    // 4. Move file to media root
    let dest_dir = PathBuf::from(&media_root_path);
    let dest_path = dest_dir.join(&session.filename);
    let dest_str = dest_path.to_string_lossy().to_string();

    if let Err(e) = tokio::fs::create_dir_all(&dest_dir).await {
        error!(error = %e, path = %dest_dir.display(), "failed to create media root directory");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "failed to create directory"})),
        )
            .into_response();
    }

    // Try rename first (same filesystem), fall back to copy+remove
    let src = PathBuf::from(&cache_path);
    if let Err(_rename_err) = tokio::fs::rename(&src, &dest_path).await {
        if let Err(e) = tokio::fs::copy(&src, &dest_path).await {
            error!(error = %e, "failed to copy cached file to media root");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to move file"})),
            )
                .into_response();
        }
        if let Err(e) = tokio::fs::remove_file(&src).await {
            warn!(error = %e, "failed to remove source cache file after copy");
        }
    }

    // 5. Create the file entry
    let file_size = session.filesize;
    let format = session
        .filename
        .rsplit('.')
        .next()
        .unwrap_or("mkv")
        .to_lowercase();

    match session.media_type.as_str() {
        "movie" => {
            let input = movie_files::MovieFileInput {
                movie_id: session.media_id,
                file_path: dest_str.clone(),
                file_size: Some(file_size),
                file_fingerprint: None,
                format,
                quality: None,
                languages: None,
                subtitles: None,
                source_type: Some("debrid".to_string()),
                source_metadata: None,
                video_codec: None,
                video_width: None,
                video_height: None,
                video_bitrate_kbps: None,
                framerate: None,
                bit_depth: None,
                is_hdr: false,
                hdr_format: None,
                is_interlaced: false,
                duration_seconds: None,
                container: None,
                overall_bitrate_kbps: None,
            };
            if let Err(e) = movie_files::create(&state.pool, input).await {
                error!(error = %e, "failed to create movie file record");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "failed to create file record"})),
                )
                    .into_response();
            }
        }
        "episode" => {
            let input = tv_episode_files::EpisodeFileInput {
                file_path: dest_str.clone(),
                file_size: Some(file_size),
                file_fingerprint: None,
                format,
                quality: None,
                languages: None,
                subtitles: None,
                source_type: Some("debrid".to_string()),
                source_metadata: None,
                video_codec: None,
                video_width: None,
                video_height: None,
                video_bitrate_kbps: None,
                framerate: None,
                bit_depth: None,
                is_hdr: false,
                is_interlaced: false,
                hdr_format: None,
                duration_seconds: None,
                container: None,
                overall_bitrate_kbps: None,
            };
            match tv_episode_files::create(&state.pool, input).await {
                Ok(file_id) => {
                    // Link the file to the episode via junction table
                    if let Err(e) =
                        tv_episode_files::link_episodes(&state.pool, file_id, &[session.media_id])
                            .await
                    {
                        error!(error = %e, "failed to link episode file");
                    }
                }
                Err(e) => {
                    error!(error = %e, "failed to create episode file record");
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": "failed to create file record"})),
                    )
                        .into_response();
                }
            }
        }
        other => {
            warn!(media_type = %other, "unsupported media type for ingest");
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("unsupported media_type: {other}")})),
            )
                .into_response();
        }
    }

    // 6. Clean up: delete cache file record, update session
    let _ = sqlx::query!(
        "DELETE FROM debrid_cache_files WHERE session_id = $1",
        session_id,
    )
    .execute(&state.pool)
    .await;

    let _ = sqlx::query!(
        "UPDATE debrid_stream_sessions SET cache_path = NULL, updated_at = NOW() WHERE id = $1",
        session_id,
    )
    .execute(&state.pool)
    .await;

    info!(session_id = %session_id, dest = %dest_str, "ingested debrid stream into library");

    Json(json!({"file_path": dest_str})).into_response()
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
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
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
            let _ = sqlx::query!(
                "UPDATE debrid_cache_files SET pinned = true WHERE session_id = $1",
                session_id,
            )
            .execute(&state.pool)
            .await;
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
            let _ = sqlx::query!(
                "UPDATE debrid_cache_files SET pinned = false WHERE session_id = $1",
                session_id,
            )
            .execute(&state.pool)
            .await;
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

/// Stream entities carry only the thin discover payload (title, year, poster,
/// genres). Enrichment is what fills in cast, crew, tagline, content rating,
/// logos, and the full season/episode list, so a streamed title's detail page
/// is as complete as a library one's.
///
/// Runs on every resolve, not just on create: titles streamed before this was
/// wired have `enriched_at IS NULL` forever, and pressing Stream on one is the
/// natural moment to catch it up. Enqueued rather than awaited — stream resolve
/// is latency-sensitive and the page picks the data up over SSE.
async fn enqueue_stream_enrich(state: &AppState, entity_type: &str, entity_id: Uuid) {
    let (job_type, payload_key) = match entity_type {
        "movie" => ("movie_enrich", "movie_id"),
        "tv_show" => ("tv_show_enrich", "show_id"),
        _ => return,
    };

    // Unenriched, and identifiable by a provider the enrich job can actually use.
    let title = match entity_type {
        "movie" => {
            sqlx::query_scalar!(
                "SELECT title FROM movies
             WHERE id = $1 AND enriched_at IS NULL
               AND (identifiers->>'tmdb_id' IS NOT NULL OR identifiers->>'imdb_id' IS NOT NULL)",
                entity_id,
            )
            .fetch_optional(&state.pool)
            .await
        }
        _ => {
            sqlx::query_scalar!(
                "SELECT title FROM tv_shows
             WHERE id = $1 AND enriched_at IS NULL
               AND (identifiers->>'tmdb_id' IS NOT NULL OR identifiers->>'imdb_id' IS NOT NULL)",
                entity_id,
            )
            .fetch_optional(&state.pool)
            .await
        }
    };
    let Ok(Some(title)) = title else {
        return;
    };

    if crate::jobs::has_active_job(&state.pool, job_type, payload_key, &entity_id.to_string())
        .await
        .unwrap_or(false)
    {
        return;
    }
    let display = crate::jobs::fetch_job_display(&state.pool, entity_type, entity_id).await;
    let context = JobContext::for_entity(entity_type, entity_id, format!("Enriching '{title}'"))
        .with_display_opt(display);
    if let Err(e) = crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        job_type,
        json!({ payload_key: entity_id }),
        context,
    )
    .await
    {
        warn!(error = %e, entity_id = %entity_id, "Failed to enqueue stream enrichment");
    }
}

/// Map a resolved stream target onto the entity enrichment actually applies to:
/// an episode's metadata comes from its show, not from the episode row.
async fn enqueue_stream_enrich_for_media(state: &AppState, media_id: Uuid, media_type: &str) {
    match media_type {
        "movie" => enqueue_stream_enrich(state, "movie", media_id).await,
        "tv" => enqueue_stream_enrich(state, "tv_show", media_id).await,
        "episode" => {
            let show_id = sqlx::query_scalar!(
                "SELECT s.show_id FROM tv_episodes e
                 JOIN tv_seasons s ON s.id = e.season_id
                 WHERE e.id = $1",
                media_id,
            )
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten();
            if let Some(show_id) = show_id {
                enqueue_stream_enrich(state, "tv_show", show_id).await;
            }
        }
        _ => {}
    }
}

/// Resolve the media_id from the request. If `auto_create` is set and no
/// `media_id` is provided, find-or-create the movie/show/episode from metadata.
/// Returns `(media_id, effective_media_type)` — media_type may be upgraded
/// from "tv" to "episode" when an episode entity is auto-created.
async fn resolve_or_create_media_id(
    state: &AppState,
    body: &StreamResolveBody,
) -> Result<(Uuid, String), axum::response::Response> {
    // If a media_id was provided directly, use it
    if let Some(id) = body.media_id
        && id != Uuid::nil()
    {
        return Ok((id, body.media_type.clone()));
    }

    // Need auto_create metadata to proceed
    let ac = match &body.auto_create {
        Some(ac) => ac,
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "media_id or auto_create required"})),
            )
                .into_response());
        }
    };

    let identifiers = ac
        .identifiers
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));

    match body.media_type.as_str() {
        "movie" => {
            // Check if movie already exists by TMDB or IMDB ID
            if let Some(tmdb_id) = identifiers.get("tmdb_id").and_then(|v| v.as_str())
                && let Ok(Some(existing)) =
                    movies::get_by_identifier(&state.pool, "tmdb_id", tmdb_id).await
            {
                return Ok((existing.id, "movie".to_string()));
            }
            if let Some(imdb_id) = identifiers.get("imdb_id").and_then(|v| v.as_str())
                && let Ok(Some(existing)) =
                    movies::get_by_identifier(&state.pool, "imdb_id", imdb_id).await
            {
                return Ok((existing.id, "movie".to_string()));
            }

            // Build images from poster/backdrop
            let mut images = crate::images::ImageMap::new();
            if let Some(ref url) = ac.poster_url {
                images.insert(
                    "poster".to_string(),
                    crate::images::ImageEntry {
                        source_url: Some(url.clone()),
                        cached_at: None,
                    },
                );
            }
            if let Some(ref url) = ac.backdrop_url {
                images.insert(
                    "backdrop".to_string(),
                    crate::images::ImageEntry {
                        source_url: Some(url.clone()),
                        cached_at: None,
                    },
                );
            }

            let input = movies::MovieInput {
                title: ac.title.clone(),
                year: ac.year,
                description: ac.description.clone(),
                images: Some(images),
                identifiers: Some(identifiers),
                genres: ac.genres.clone(),
                rating: ac.rating,
                rating_count: ac.rating_count,
                runtime: ac.runtime,
                streamed_at: Some(time::OffsetDateTime::now_utc()),
                ..Default::default()
            };
            match movies::create(&state.pool, input).await {
                Ok(id) => {
                    info!(movie_id = %id, title = %ac.title, "Auto-created movie from stream");
                    Ok((id, "movie".to_string()))
                }
                Err(e) => {
                    error!(error = %e, "Failed to auto-create movie");
                    Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": "failed to create movie"})),
                    )
                        .into_response())
                }
            }
        }
        "tv" | "episode" => {
            // Find or create the TV show by TMDB/IMDB ID
            let show_id = if let Some(tmdb_id) = identifiers.get("tmdb_id").and_then(|v| v.as_str())
                && let Ok(Some(existing)) =
                    tv_shows::get_by_identifier(&state.pool, "tmdb_id", tmdb_id).await
            {
                existing.id
            } else if let Some(imdb_id) = identifiers.get("imdb_id").and_then(|v| v.as_str())
                && let Ok(Some(existing)) =
                    tv_shows::get_by_identifier(&state.pool, "imdb_id", imdb_id).await
            {
                existing.id
            } else {
                let mut images = crate::images::ImageMap::new();
                if let Some(ref url) = ac.poster_url {
                    images.insert(
                        "poster".to_string(),
                        crate::images::ImageEntry {
                            source_url: Some(url.clone()),
                            cached_at: None,
                        },
                    );
                }
                if let Some(ref url) = ac.backdrop_url {
                    images.insert(
                        "backdrop".to_string(),
                        crate::images::ImageEntry {
                            source_url: Some(url.clone()),
                            cached_at: None,
                        },
                    );
                }
                let input = tv_shows::TvShowInput {
                    title: ac.title.clone(),
                    year: ac.year,
                    description: ac.description.clone(),
                    images: Some(images),
                    identifiers: Some(identifiers.clone()),
                    genres: ac.genres.clone(),
                    rating: ac.rating,
                    rating_count: ac.rating_count,
                    network: ac.network.clone(),
                    is_monitored: Some(true),
                    auto_want_mode: Some("none".to_string()),
                    runtime: ac.runtime,
                    streamed_at: Some(time::OffsetDateTime::now_utc()),
                    ..Default::default()
                };
                match tv_shows::create(&state.pool, input).await {
                    Ok(id) => {
                        info!(show_id = %id, title = %ac.title, "Auto-created TV show from stream");
                        id
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to auto-create TV show");
                        return Err((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({"error": "failed to create TV show"})),
                        )
                            .into_response());
                    }
                }
            };

            // If season/episode info is provided, create the full chain and
            // return the episode ID so the session uses media_type="episode".
            if let Some(season_num) = ac.season_number
                && let Some(episode_num) = ac.episode_number
            {
                let pool = &state.pool;

                // Find or create the season
                let season_str = season_num.to_string();
                let season_id = match tv_seasons::list_by_show(pool, show_id).await {
                    Ok(seasons) => {
                        if let Some(s) = seasons.iter().find(|s| s.season_identifier == season_str)
                        {
                            s.id
                        } else {
                            match tv_seasons::create(
                                pool,
                                tv_seasons::SeasonInput {
                                    show_id,
                                    season_identifier: season_str.clone(),
                                    season_sort_order: season_num as i32,
                                    title: None,
                                    description: None,
                                    images: None,
                                    air_date: None,
                                    episode_count: None,
                                    is_monitored: true,
                                    is_special: false,
                                    auto_want_mode: "none".to_string(),
                                },
                            )
                            .await
                            {
                                Ok(id) => id,
                                Err(e) => {
                                    warn!(error = %e, "Failed to create season, falling back to show ID");
                                    return Ok((show_id, "tv".to_string()));
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to list seasons, falling back to show ID");
                        return Ok((show_id, "tv".to_string()));
                    }
                };

                // Sync episode data from metadata provider
                let provider_episodes = if let Some(ref provider) = ac.provider
                    && let Some(ref provider_id) = ac.provider_id
                {
                    let registry =
                        crate::video_metadata::VideoProviderRegistry::new(state.pool.clone());
                    match registry.get_season(provider, provider_id, season_num).await {
                        Ok(details) => details.episodes,
                        Err(e) => {
                            warn!(error = %e, "Failed to fetch season from provider");
                            vec![]
                        }
                    }
                } else {
                    vec![]
                };

                // Find or create the episode
                let ep_str = episode_num.to_string();
                let provider_ep = provider_episodes
                    .iter()
                    .find(|e| e.episode_number == episode_num);

                let episode_id = match tv_episodes::get_by_season_and_identifier(
                    pool, season_id, &ep_str,
                )
                .await
                {
                    Ok(Some(ep)) => ep.id,
                    Ok(None) => {
                        let air_date =
                            provider_ep
                                .and_then(|e| e.air_date.as_deref())
                                .and_then(|d| {
                                    time::Date::parse(
                                        d,
                                        &time::format_description::well_known::Iso8601::DEFAULT,
                                    )
                                    .ok()
                                });
                        let input = tv_episodes::EpisodeInput {
                            season_id,
                            episode_identifier: ep_str,
                            episode_sort_order: episode_num as i32,
                            title: provider_ep.and_then(|e| e.title.clone()),
                            description: provider_ep.and_then(|e| e.description.clone()),
                            images: None,
                            air_date,
                            air_time: None,
                            runtime: provider_ep.and_then(|e| e.runtime.map(|r| r as i32)),
                            identifiers: serde_json::json!({}),
                            country: None,
                        };
                        match tv_episodes::create(pool, input).await {
                            Ok(id) => id,
                            Err(e) => {
                                warn!(error = %e, "Failed to create episode, falling back to show ID");
                                return Ok((show_id, "tv".to_string()));
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to look up episode, falling back to show ID");
                        return Ok((show_id, "tv".to_string()));
                    }
                };

                info!(show_id = %show_id, episode_id = %episode_id, "Auto-created TV episode for stream");
                Ok((episode_id, "episode".to_string()))
            } else {
                // No episode info — return show ID (fallback, won't track per-episode progress)
                Ok((show_id, "tv".to_string()))
            }
        }
        _ => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("unsupported media_type: {}", body.media_type)})),
        )
            .into_response()),
    }
}

async fn build_movie_search_request(
    state: &AppState,
    movie_id: Uuid,
) -> Result<nats_jobs::StreamSearchRequest, axum::response::Response> {
    let movie = match movies::get(&state.pool, movie_id).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(json!({"error": "movie not found"})),
            )
                .into_response());
        }
        Err(e) => {
            error!(error = %e, "failed to fetch movie");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "database error"})),
            )
                .into_response());
        }
    };

    let identifiers = nats_jobs::MovieIdentifiers {
        tmdb_id: movie
            .identifiers
            .get("tmdb_id")
            .and_then(|v| v.as_str())
            .map(String::from),
        imdb_id: movie
            .identifiers
            .get("imdb_id")
            .and_then(|v| v.as_str())
            .map(String::from),
    };

    let media_want = nats_jobs::MediaWant::Movie {
        title: movie.title,
        year: movie.year.map(|y| y as u32),
        release_date: movie.release_date.map(|d| d.to_string()),
        identifiers: Some(identifiers),
        alternate_titles: movie.alternate_titles,
        quality_profile_id: movie.quality_profile_id,
        quality_overrides: movie.preferred_languages.as_ref().map(|langs| {
            nats_jobs::QualityOverrides {
                preferred_languages: Some(langs.clone()),
                ..Default::default()
            }
        }),
        in_library: None,
        library_video_height: None,
    };

    Ok(nats_jobs::StreamSearchRequest {
        media_want: Some(media_want),
        query: String::new(),
        media_type: Some("movie".to_string()),
        quality_profile_id: movie.quality_profile_id,
        limit: None,
        include_uncached: None,
    })
}

async fn build_episode_search_request(
    state: &AppState,
    episode_id: Uuid,
) -> Result<nats_jobs::StreamSearchRequest, axum::response::Response> {
    let episode = match tv_episodes::get(&state.pool, episode_id).await {
        Ok(Some(e)) => e,
        Ok(None) => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(json!({"error": "episode not found"})),
            )
                .into_response());
        }
        Err(e) => {
            error!(error = %e, "failed to fetch episode");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "database error"})),
            )
                .into_response());
        }
    };

    let season = match tv_seasons::get(&state.pool, episode.season_id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(json!({"error": "season not found"})),
            )
                .into_response());
        }
        Err(e) => {
            error!(error = %e, "failed to fetch season");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "database error"})),
            )
                .into_response());
        }
    };

    let show = match tv_shows::get(&state.pool, season.show_id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(json!({"error": "show not found"})),
            )
                .into_response());
        }
        Err(e) => {
            error!(error = %e, "failed to fetch show");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "database error"})),
            )
                .into_response());
        }
    };

    let media_want = if show.content_category == "sport" {
        // Sports leagues store events as episodes, but a bare TV-episode search
        // degrades to the league name ("MotoGP") and matches unrelated releases —
        // the 2026 San Marino race search returned a MotoGP video-game repack.
        // The structured SportEvent want searches "MotoGP San Marino Race" instead,
        // the same shape the want pipeline sends.
        let mut details = parse_sport_event_details(
            episode.title.as_deref(),
            &episode.episode_identifier,
            episode.episode_sort_order,
        );

        // Prefer structured country from provider over title-parsed location
        if let Some(ref country) = episode.country
            && !country.is_empty()
        {
            details.location = Some(country.clone());
        }

        nats_jobs::MediaWant::SportEvent {
            league_name: show.title,
            season: season.season_identifier,
            round: details.round,
            location: details.location,
            session_type: details.session_type,
            event_title: details.event_title,
            event_date: episode.air_date.map(|d| d.to_string()),
            quality_profile_id: show.quality_profile_id,
            quality_overrides: show.preferred_languages.as_ref().map(|langs| {
                nats_jobs::QualityOverrides {
                    preferred_languages: Some(langs.clone()),
                    ..Default::default()
                }
            }),
            in_library: None,
            library_video_height: None,
        }
    } else {
        let identifiers = nats_jobs::TvIdentifiers {
            tmdb_id: show
                .identifiers
                .get("tmdb_id")
                .and_then(|v| v.as_str())
                .map(String::from),
            tvdb_id: show
                .identifiers
                .get("tvdb_id")
                .and_then(|v| v.as_str())
                .map(String::from),
            imdb_id: show
                .identifiers
                .get("imdb_id")
                .and_then(|v| v.as_str())
                .map(String::from),
        };

        nats_jobs::MediaWant::TvEpisode {
            show_name: show.title,
            season: season.season_identifier,
            episode: episode.episode_identifier,
            year: show.year.map(|y| y as u32),
            air_date: episode.air_date.map(|d| d.to_string()),
            identifiers: Some(identifiers),
            alternate_titles: show.alternate_titles,
            episode_title: episode.title,
            quality_profile_id: show.quality_profile_id,
            quality_overrides: show.preferred_languages.as_ref().map(|langs| {
                nats_jobs::QualityOverrides {
                    preferred_languages: Some(langs.clone()),
                    ..Default::default()
                }
            }),
        }
    };

    Ok(nats_jobs::StreamSearchRequest {
        media_want: Some(media_want),
        query: String::new(),
        media_type: Some("tv".to_string()),
        quality_profile_id: show.quality_profile_id,
        limit: None,
        include_uncached: None,
    })
}

/// Get a valid debrid streaming URL, re-resolving if the cached one expired.
/// Used when resuming a stopped download.
async fn resolve_fresh_debrid_url(
    state: &AppState,
    cached_url: &Option<String>,
    info_hash: Option<&str>,
    debrid_provider: &str,
    session_id: Uuid,
) -> Option<String> {
    // Try the cached URL first
    if let Some(url) = cached_url
        && !url.is_empty()
    {
        return Some(url.clone());
    }

    // Re-resolve via Kobload
    let runtime = state.runtime_config.get().await;
    let client = match build_kobload_client(&runtime) {
        Ok(c) => c,
        Err(_) => {
            error!(session_id = %session_id, "Failed to build kobload client for download resume");
            return None;
        }
    };

    let debrid_provider_id = match Uuid::parse_str(debrid_provider) {
        Ok(id) => id,
        Err(_) => {
            error!(session_id = %session_id, "Invalid debrid_provider UUID for download resume");
            return None;
        }
    };

    let resolve_req = nats_jobs::StreamResolveRequest {
        debrid_provider_id,
        info_hash: info_hash.map(String::from),
        magnet_url: None,
        nzb_url: None,
        file_id: None,
    };

    match client.stream_resolve(&resolve_req).await {
        Ok(resolved) if resolved.stream_url.is_empty() => {
            warn!(session_id = %session_id, "re-resolve returned empty URL for download resume (torrent not cached)");
            None
        }
        Ok(resolved) => {
            // Cache the new URL
            let expires_at = resolved.expires_at.as_deref().and_then(|s| {
                time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
            });
            let _ = sqlx::query(
                "UPDATE debrid_stream_sessions SET debrid_url = $1, debrid_url_expires_at = $2, updated_at = NOW() WHERE id = $3",
            )
            .bind(&resolved.stream_url)
            .bind(expires_at)
            .bind(session_id)
            .execute(&state.pool)
            .await;
            Some(resolved.stream_url)
        }
        Err(e) => {
            error!(session_id = %session_id, error = %e, "Failed to re-resolve debrid URL for download resume");
            None
        }
    }
}

/// Proxy a remote URL, forwarding Range headers and streaming the response body.
async fn proxy_stream(
    url: &str,
    filesize: i64,
    incoming_headers: &HeaderMap,
    patch: Option<Arc<CuePatch>>,
) -> axum::response::Response {
    // A range that lies wholly inside the trimmed index needs no upstream request at
    // all — which is the request the demuxer makes before it decodes anything.
    if let Some(p) = patch.as_ref()
        && let Some(range) = incoming_headers
            .get(header::RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| parse_byte_range(v, filesize.max(0) as u64))
        && let Some(bytes) = p.whole_range(range.start, (range.end - range.start + 1) as usize)
    {
        return Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_TYPE, "video/x-matroska")
            .header(header::CONTENT_LENGTH, bytes.len().to_string())
            .header(
                header::CONTENT_RANGE,
                format!("bytes {}-{}/{}", range.start, range.end, filesize),
            )
            .header(header::ACCEPT_RANGES, "bytes")
            .body(Body::from(bytes.to_vec()))
            .unwrap()
            .into_response();
    }

    // Dedicated client for streaming: no total timeout (transfers are large),
    // but read_timeout catches debrid servers that hang mid-stream.
    let http_client = crate::http::builder()
        .timeout(Duration::MAX)
        .read_timeout(Duration::from_secs(10))
        .build()
        .expect("proxy stream client");

    let mut req = http_client.get(url);

    // Forward Range header if present
    if let Some(range) = incoming_headers.get(header::RANGE) {
        req = req.header(header::RANGE, range);
    }

    let upstream = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "failed to connect to debrid stream");
            return (StatusCode::BAD_GATEWAY, "upstream connection failed").into_response();
        }
    };

    let upstream_status = upstream.status();
    let upstream_headers = upstream.headers().clone();

    let mut builder = Response::builder();

    // Map upstream status
    let status = if upstream_status == reqwest::StatusCode::PARTIAL_CONTENT {
        StatusCode::PARTIAL_CONTENT
    } else if upstream_status.is_success() {
        StatusCode::OK
    } else {
        error!(status = %upstream_status, "debrid upstream returned error");
        return (StatusCode::BAD_GATEWAY, "upstream error").into_response();
    };
    builder = builder.status(status);

    // Forward relevant headers
    for key in &[
        header::CONTENT_TYPE,
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
    ] {
        if let Some(val) = upstream_headers.get(key.as_str()) {
            builder = builder.header(key, val);
        }
    }

    // If no content-type from upstream, guess from filesize context
    if upstream_headers
        .get(header::CONTENT_TYPE.as_str())
        .is_none()
    {
        builder = builder.header(header::CONTENT_TYPE, "video/mp4");
    }

    builder = builder.header(header::ACCEPT_RANGES, "bytes");

    // If upstream didn't send content-length and this is a full request, use session filesize
    if status == StatusCode::OK
        && upstream_headers
            .get(header::CONTENT_LENGTH.as_str())
            .is_none()
    {
        builder = builder.header(header::CONTENT_LENGTH, filesize.to_string());
    }

    let body_stream = upstream.bytes_stream();
    let body = match patch {
        // Where the response actually starts, which for a 206 is whatever upstream
        // decided to give us rather than whatever we asked for.
        Some(p) => {
            let start = upstream_headers
                .get(header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(content_range_start)
                .unwrap_or(0);
            Body::from_stream(patched_stream(body_stream, start, p))
        }
        None => Body::from_stream(body_stream),
    };

    builder.body(body).unwrap().into_response()
}

/// First byte offset of a `bytes 123-456/789` response.
fn content_range_start(value: &str) -> Option<u64> {
    value
        .trim()
        .strip_prefix("bytes ")?
        .split('-')
        .next()?
        .trim()
        .parse()
        .ok()
}

/// Serve a local cached file with full HTTP Range support.
async fn serve_local_file(
    path: &std::path::Path,
    filesize: i64,
    filename: &str,
    incoming_headers: &HeaderMap,
    patch: Option<Arc<CuePatch>>,
) -> axum::response::Response {
    let file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) => {
            error!(error = %e, path = %path.display(), "failed to open cached stream file");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to open cache file",
            )
                .into_response();
        }
    };

    let file_size = match file.metadata().await {
        Ok(m) => m.len(),
        Err(_) => filesize as u64,
    };

    let content_type = guess_video_content_type(filename);

    // Parse Range header for seeking support
    if let Some(range_header) = incoming_headers.get(header::RANGE)
        && let Ok(range_str) = range_header.to_str()
        && let Some(range) = parse_byte_range(range_str, file_size)
    {
        return serve_byte_range(file, file_size, content_type, range, patch).await;
    }

    // No range — serve full file
    let stream = ReaderStream::new(file);
    let body = match patch {
        Some(p) => Body::from_stream(patched_stream(stream, 0, p)),
        None => Body::from_stream(stream),
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, file_size.to_string())
        .header(header::ACCEPT_RANGES, "bytes")
        .body(body)
        .unwrap()
        .into_response()
}

struct ByteRange {
    start: u64,
    end: u64,
}

fn parse_byte_range(header_val: &str, file_size: u64) -> Option<ByteRange> {
    // No byte of an empty file is satisfiable, and every branch below indexes the
    // last one.
    if file_size == 0 {
        return None;
    }
    let range = header_val.strip_prefix("bytes=")?;
    let (start_str, end_str) = range.split_once('-')?;

    if start_str.is_empty() {
        let suffix: u64 = end_str.parse().ok()?;
        if suffix == 0 {
            return None;
        }
        let start = file_size.saturating_sub(suffix);
        Some(ByteRange {
            start,
            end: file_size - 1,
        })
    } else {
        let start: u64 = start_str.parse().ok()?;
        let end = if end_str.is_empty() {
            file_size - 1
        } else {
            end_str.parse().ok()?
        };
        if start <= end && start < file_size {
            Some(ByteRange {
                start,
                end: end.min(file_size - 1),
            })
        } else {
            None
        }
    }
}

async fn serve_byte_range(
    mut file: tokio::fs::File,
    file_size: u64,
    content_type: &str,
    range: ByteRange,
    patch: Option<Arc<CuePatch>>,
) -> axum::response::Response {
    let length = range.end - range.start + 1;
    let partial = |body: Body| {
        Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::CONTENT_LENGTH, length.to_string())
            .header(
                header::CONTENT_RANGE,
                format!("bytes {}-{}/{}", range.start, range.end, file_size),
            )
            .header(header::ACCEPT_RANGES, "bytes")
            .body(body)
            .unwrap()
            .into_response()
    };

    if let Some(p) = patch.as_ref()
        && let Some(bytes) = p.whole_range(range.start, length as usize)
    {
        return partial(Body::from(bytes.to_vec()));
    }

    if let Err(e) = file.seek(std::io::SeekFrom::Start(range.start)).await {
        error!(error = %e, "failed to seek in cached stream file");
        return (StatusCode::INTERNAL_SERVER_ERROR, "seek error").into_response();
    }

    let limited = file.take(length);
    let stream = ReaderStream::new(limited);
    let body = match patch {
        Some(p) => Body::from_stream(patched_stream(stream, range.start, p)),
        None => Body::from_stream(stream),
    };

    partial(body)
}

fn guess_video_content_type(filename: &str) -> &'static str {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",
        "ts" => "video/mp2t",
        "wmv" => "video/x-ms-wmv",
        _ => "video/mp4",
    }
}

/// Returns true for container formats that browsers cannot natively play
/// and need on-the-fly remux to fragmented MP4.
fn needs_remux(filename: &str) -> bool {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    matches!(ext.as_str(), "mkv" | "avi" | "ts" | "m2ts" | "flv" | "wmv")
}

/// Pipe a reqwest response body through ffmpeg for on-the-fly remux to fMP4.
///
/// Video is copied as-is (zero CPU), audio is transcoded to AAC for browser
/// compatibility. Output is fragmented MP4 with `frag_keyframe+empty_moov`
/// so it's streamable without needing the moov atom at the end.
async fn remux_proxy_stream(url: &str) -> axum::response::Response {
    let http_client = crate::http::new_client();

    // Fetch full file from debrid — no range for remux (ffmpeg needs sequential input)
    let upstream = match http_client.get(url).send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            error!(status = %r.status(), "debrid upstream returned error for remux");
            return (StatusCode::BAD_GATEWAY, "upstream error").into_response();
        }
        Err(e) => {
            error!(error = %e, "failed to connect to debrid stream for remux");
            return (StatusCode::BAD_GATEWAY, "upstream connection failed").into_response();
        }
    };

    let mut ffmpeg = match tokio::process::Command::new("ffmpeg")
        .args([
            "-i",
            "pipe:0",
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-f",
            "mp4",
            "-movflags",
            "frag_keyframe+empty_moov",
            "pipe:1",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            error!(error = %e, "failed to spawn ffmpeg for remux");
            return (StatusCode::INTERNAL_SERVER_ERROR, "remux unavailable").into_response();
        }
    };

    let mut stdin = ffmpeg.stdin.take().expect("ffmpeg stdin was piped");
    let stdout = ffmpeg.stdout.take().expect("ffmpeg stdout was piped");

    // Spawn a task to pipe the upstream body into ffmpeg stdin
    tokio::spawn(async move {
        use futures::StreamExt;
        let mut stream = upstream.bytes_stream();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    if tokio::io::AsyncWriteExt::write_all(&mut stdin, &bytes)
                        .await
                        .is_err()
                    {
                        break; // ffmpeg closed stdin (e.g. output consumer disconnected)
                    }
                }
                Err(e) => {
                    warn!(error = %e, "upstream read error during remux");
                    break;
                }
            }
        }
        drop(stdin); // Close stdin to signal EOF to ffmpeg
    });

    // Stream ffmpeg stdout back to the client
    let reader_stream = ReaderStream::new(tokio::io::BufReader::new(stdout));

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "video/mp4")
        // No Content-Length — chunked transfer since we don't know remuxed size
        .body(Body::from_stream(reader_stream))
        .unwrap()
        .into_response()
}

/// Remux a local file through ffmpeg to fragmented MP4.
async fn remux_local_file(path: &std::path::Path) -> axum::response::Response {
    let path_str = path.to_string_lossy().to_string();

    let mut ffmpeg = match tokio::process::Command::new("ffmpeg")
        .args([
            "-i",
            &path_str,
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-f",
            "mp4",
            "-movflags",
            "frag_keyframe+empty_moov",
            "pipe:1",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            error!(error = %e, "failed to spawn ffmpeg for local remux");
            return (StatusCode::INTERNAL_SERVER_ERROR, "remux unavailable").into_response();
        }
    };

    let stdout = ffmpeg.stdout.take().expect("ffmpeg stdout was piped");
    let reader_stream = ReaderStream::new(tokio::io::BufReader::new(stdout));

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "video/mp4")
        .body(Body::from_stream(reader_stream))
        .unwrap()
        .into_response()
}

/// Create a playback session from a fully cached local file.
/// Skips all debrid API calls — probes and transcodes the local file directly.
async fn create_local_cached_session(
    state: &AppState,
    runtime: &crate::runtime_config::RuntimeConfig,
    auth: PlaybackAuth,
    body: &StreamResolveBody,
    file_path: PathBuf,
    filename: String,
    _filesize: i64,
) -> impl IntoResponse {
    let session_id = Uuid::new_v4();

    // Probe the local file (fast — it's on disk)
    let probe_path = file_path.clone();
    let probe =
        match tokio::task::spawn_blocking(move || kob_ffmpeg::probe::probe(&probe_path)).await {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                error!(error = %e, "Failed to probe cached file");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("probe failed: {e}")})),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("probe task: {e}")})),
                )
                    .into_response();
            }
        };

    let duration = probe.format.duration_seconds.unwrap_or(0.0);
    let video_codec = probe
        .video_streams
        .first()
        .map(|v| v.codec.clone())
        .unwrap_or_default();
    let audio_codec = probe
        .audio_streams
        .first()
        .map(|a| a.codec.clone())
        .unwrap_or_default();
    let container = filename.rsplit('.').next().unwrap_or("mkv").to_lowercase();

    // If the client can direct play this container, skip transcoding entirely.
    // Create a lightweight session that serves the raw file.
    let can_direct_play = body.capabilities.as_ref().is_some_and(|caps| {
        use crate::streaming::decision::is_container_compatible;
        is_container_compatible(&container, &caps.containers)
    });

    if can_direct_play {
        info!(
            session_id = %session_id,
            file = %filename,
            duration,
            video_codec = %video_codec,
            "Direct play from local cache (no debrid, no transcode)"
        );

        let segment_dir = runtime
            .transcode_cache_dir
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(&state.config.storage_root)
                    .join("cache")
                    .join("transcode")
            })
            .join(session_id.to_string());
        let mut segment_cache =
            crate::streaming::segment_cache::SegmentCache::new(segment_dir, 6, duration);
        if let Some(st) = body.start_time {
            segment_cache.set_playback_start(segment_cache.time_to_segment(st), st);
        }
        let debrid_thumb_dir = file_path
            .parent()
            .unwrap_or(&file_path)
            .join("media_cache")
            .join("trickplay");
        let has_thumbnails = debrid_thumb_dir.join("thumbnails.vtt").exists();

        let cached_audio_metas: Vec<crate::streaming::decision::AudioTrackMeta> = probe
            .audio_streams
            .iter()
            .map(|a| crate::streaming::decision::AudioTrackMeta {
                stream_index: a.index,
                codec: a.codec.clone(),
                channels: a.channels,
                language: a.language.clone(),
                title: a.title.clone(),
                is_default: a.is_default,
            })
            .collect();
        let preferred_audio_idx = crate::streaming::decision::find_preferred_audio(
            &cached_audio_metas,
            &runtime.preferred_audio_language,
        )
        .unwrap_or_else(|| probe.audio_streams.first().map(|a| a.index).unwrap_or(0));

        let playback_session = crate::streaming::session::PlaybackSession {
            id: session_id,
            user_id: auth.user_id,
            media_type: crate::streaming::session::MediaType::Movie,
            file_id: session_id,
            file_path,
            decision: crate::streaming::decision::PlaybackDecision {
                method: PlaybackMethod::DirectPlay,
                video_mode: crate::streaming::decision::VideoMode::Copy,
                audio_mode: crate::streaming::decision::AudioMode::Copy,
                audio_stream_index: preferred_audio_idx,
                subtitle_handling: crate::streaming::decision::SubtitleHandling::None,
            },
            transcode: None,
            segment_cache,
            created_at: std::time::Instant::now(),
            last_activity: std::time::Instant::now(),
            seek_generation: 0,
            duration_seconds: duration,
            container: container.clone(),
            video_codec: video_codec.clone(),
            subtitle_files: vec![],
            hw_accel: (*state.hw_accel).clone(),
            source_resolution: (
                probe.video_streams.first().map(|v| v.width).unwrap_or(1920) as u32,
                probe
                    .video_streams
                    .first()
                    .map(|v| v.height)
                    .unwrap_or(1080) as u32,
            ),
            variants: None,
            is_live: false,
            provider_id: None,
            max_bitrate_kbps: None,
            keyframes: None,
            error: None,
            pending_segment: None,
            download_cancel: None,
            thumbnail_dir: Some(debrid_thumb_dir.clone()),
            debrid_cache_key: debrid_cache_key(body.info_hash.as_deref(), body.file_id),
            audio_metas: cached_audio_metas,
            force_video_transcode: false,
            corrupt_ranges: Vec::new(),
            overlay_spawned_starts: std::collections::HashSet::new(),
            highest_served_segment: None,
        };

        state.playback_sessions.insert(session_id, playback_session);

        if let Some(key) = debrid_cache_key(body.info_hash.as_deref(), body.file_id) {
            state.debrid_session_index.insert(key, session_id);
        }

        // Insert DB row so stream_play can find this session
        let cache_path_str = state
            .playback_sessions
            .get(&session_id)
            .map(|s| s.file_path.to_string_lossy().to_string())
            .unwrap_or_default();
        let _ = sqlx::query(
            r#"INSERT INTO debrid_stream_sessions
                (id, user_id, media_type, media_id, release_title, info_hash,
                 debrid_provider, quality, filename, filesize, cache_path, file_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT (id) DO NOTHING"#,
        )
        .bind(session_id)
        .bind(auth.user_id)
        .bind(&body.media_type)
        .bind(body.media_id)
        .bind(&body.release_title)
        .bind(&body.info_hash)
        .bind(&body.debrid_provider)
        .bind(&body.quality)
        .bind(&filename)
        .bind(_filesize)
        .bind(&cache_path_str)
        .bind(body.file_id)
        .execute(&state.pool)
        .await;

        let base_url = format!("/api/v1/admin/playback/sessions/{session_id}");
        let thumbnails_url = if has_thumbnails {
            Some(format!("{base_url}/thumbnails.vtt"))
        } else {
            None
        };

        let audio_tracks: Vec<AudioTrackResponse> = probe
            .audio_streams
            .iter()
            .map(|a| AudioTrackResponse {
                stream_index: a.index,
                codec: a.codec.clone(),
                channels: a.channels,
                language: a.language.clone(),
                title: a.title.clone(),
                is_default: a.index == preferred_audio_idx,
            })
            .collect();

        let (probe_w, probe_h) = probe
            .video_streams
            .first()
            .map(|v| (v.width as u32, v.height as u32))
            .unwrap_or((0, 0));

        return Json(SessionResponse {
            session_id: session_id.to_string(),
            method: PlaybackMethod::DirectPlay,
            playlist_url: None,
            direct_url: Some(format!("/api/v1/admin/stream/{session_id}/play")),
            duration_seconds: duration,
            subtitles: vec![],
            audio_tracks,
            thumbnails_url,
            lufs: None,
            chapters: vec![],
            skip_segments: vec![],
            start_time: None, // DirectPlay: no server-side seek; frontend resumes via initialPosition
            media_id: body.media_id.map(|id| id.to_string()),
            media_type: None,
            file_id: None,
            selected_audio_index: Some(preferred_audio_idx),
            selected_subtitle_index: None,
            video_width: if probe_w > 0 { Some(probe_w) } else { None },
            video_height: if probe_h > 0 { Some(probe_h) } else { None },
        })
        .into_response();
    }

    info!(
        session_id = %session_id,
        file = %filename,
        duration,
        video_codec = %video_codec,
        "Playing from local cache (no debrid)"
    );

    // Build audio metas early so we can find the preferred track
    let cached_audio_metas: Vec<crate::streaming::decision::AudioTrackMeta> = probe
        .audio_streams
        .iter()
        .map(|a| crate::streaming::decision::AudioTrackMeta {
            stream_index: a.index,
            codec: a.codec.clone(),
            channels: a.channels,
            language: a.language.clone(),
            title: a.title.clone(),
            is_default: a.is_default,
        })
        .collect();
    let preferred_audio_idx = crate::streaming::decision::find_preferred_audio(
        &cached_audio_metas,
        &runtime.preferred_audio_language,
    )
    .unwrap_or_else(|| probe.audio_streams.first().map(|a| a.index).unwrap_or(0));
    let preferred_audio_codec = cached_audio_metas
        .iter()
        .find(|a| a.stream_index == preferred_audio_idx)
        .map(|a| a.codec.as_str())
        .unwrap_or(&audio_codec);

    let needs_transcode = video_codec == "hevc" || video_codec == "av1";
    let video_mode = if needs_transcode {
        crate::ffmpeg::transcode::VideoMode::Transcode
    } else {
        crate::ffmpeg::transcode::VideoMode::Copy
    };
    let audio_mode = if preferred_audio_codec == "aac" {
        crate::ffmpeg::transcode::AudioMode::Copy
    } else {
        crate::ffmpeg::transcode::AudioMode::Transcode {
            target_codec: AAC_ENCODER.to_string(),
            bitrate_kbps: crate::streaming::decision::suggest_audio_bitrate(
                cached_audio_metas
                    .iter()
                    .find(|a| a.stream_index == preferred_audio_idx),
            ),
        }
    };

    let segment_dir = runtime
        .transcode_cache_dir
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(&state.config.storage_root)
                .join("cache")
                .join("transcode")
        })
        .join(session_id.to_string());

    let primary_encoder = if needs_transcode {
        state.encoder_caps.h264.clone()
    } else {
        state
            .encoder_caps
            .encoder_for(&video_codec)
            .unwrap_or(&state.encoder_caps.h264)
            .to_string()
    };

    // Build encoder fallback chain: primary → vaapi → libx264
    let mut encoder_chain = vec![primary_encoder.clone()];
    encoder_chain.extend(
        state
            .encoder_caps
            .h264_fallbacks()
            .into_iter()
            .map(String::from),
    );

    let mut transcode = None;
    let mut last_error = String::new();
    for encoder in &encoder_chain {
        // Clean up segment dir from prior failed attempt
        if !last_error.is_empty() {
            let _ = tokio::fs::remove_dir_all(&segment_dir).await;
        }

        let args = crate::ffmpeg::transcode::TranscodeArgs {
            input_path: file_path.clone(),
            output_dir: segment_dir.clone(),
            video_mode,
            audio_mode: audio_mode.clone(),
            audio_stream_index: preferred_audio_idx,
            start_time: body.start_time,
            segment_duration: 6,
            hw_accel: (*state.hw_accel).clone(),
            video_codec: video_codec.clone(),
            target_encoder: encoder.clone(),
            burn_in_subtitle: None,
            scale: None,
            target_bitrate_kbps: None,
            duration_seconds: duration,
            initial_generation: 0,
            hw_device: if encoder == "libx264" {
                None
            } else if encoder.contains("vaapi")
                && matches!(*state.hw_accel, crate::ffmpeg::detect::HwAccel::Nvenc)
            {
                if let Some(ref dev) = state.vaapi_fallback_device {
                    Some(dev.clone())
                } else {
                    warn!(
                        session_id = %session_id,
                        "No VAAPI fallback device available, skipping VAAPI encoder"
                    );
                    last_error = "No VAAPI fallback device available".to_string();
                    continue;
                }
            } else {
                state.hw_device.get()
            },
        };

        match crate::ffmpeg::transcode::TranscodeProcess::start(args).await {
            Ok(mut process) => {
                // Wait briefly to detect immediate encoder crashes (e.g. nvenc pixel format issues)
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                match process.check_health() {
                    Ok(true) => {
                        if encoder != &primary_encoder {
                            info!(
                                session_id = %session_id,
                                encoder = %encoder,
                                "Encoder fallback succeeded"
                            );
                        }
                        transcode = Some(process);
                        break;
                    }
                    Err(e) => {
                        warn!(
                            session_id = %session_id,
                            encoder = %encoder,
                            error = %e,
                            "Encoder crashed immediately, trying fallback"
                        );
                        process.kill().await;
                        last_error = e;
                    }
                    Ok(false) => {
                        warn!(
                            session_id = %session_id,
                            encoder = %encoder,
                            "Encoder exited immediately, trying fallback"
                        );
                        process.kill().await;
                        last_error = "encoder exited without error".to_string();
                    }
                }
            }
            Err(e) => {
                warn!(
                    session_id = %session_id,
                    encoder = %encoder,
                    error = %e,
                    "Failed to start encoder, trying fallback"
                );
                last_error = e.to_string();
            }
        }
    }

    let transcode = match transcode {
        Some(t) => t,
        None => {
            error!(
                session_id = %session_id,
                error = %last_error,
                "All encoders failed for cached file"
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("transcode failed: {last_error}")})),
            )
                .into_response();
        }
    };

    let mut segment_cache =
        crate::streaming::segment_cache::SegmentCache::new(segment_dir, 6, duration);
    if let Some(st) = body.start_time {
        segment_cache.set_playback_start(segment_cache.time_to_segment(st), st);
    }

    let debrid_thumb_dir = file_path
        .parent()
        .unwrap_or(&file_path)
        .join("media_cache")
        .join("trickplay");
    let has_thumbnails = debrid_thumb_dir.join("thumbnails.vtt").exists();

    let method = if needs_transcode {
        PlaybackMethod::VideoTranscode
    } else if matches!(
        audio_mode,
        crate::ffmpeg::transcode::AudioMode::Transcode { .. }
    ) {
        PlaybackMethod::AudioTranscode
    } else {
        PlaybackMethod::Remux
    };

    let playback_session = crate::streaming::session::PlaybackSession {
        id: session_id,
        user_id: auth.user_id,
        media_type: crate::streaming::session::MediaType::Movie,
        file_id: session_id,
        file_path,
        decision: crate::streaming::decision::PlaybackDecision {
            method,
            video_mode: if needs_transcode {
                crate::streaming::decision::VideoMode::Transcode {
                    target_codec: "h264".to_string(),
                }
            } else {
                crate::streaming::decision::VideoMode::Copy
            },
            audio_mode: if preferred_audio_codec == "aac" {
                crate::streaming::decision::AudioMode::Copy
            } else {
                crate::streaming::decision::AudioMode::Transcode {
                    target_codec: AAC_ENCODER.to_string(),
                    bitrate_kbps: crate::streaming::decision::suggest_audio_bitrate(
                        cached_audio_metas
                            .iter()
                            .find(|a| a.stream_index == preferred_audio_idx),
                    ),
                }
            },
            audio_stream_index: preferred_audio_idx,
            subtitle_handling: crate::streaming::decision::SubtitleHandling::None,
        },
        transcode: Some(transcode),
        segment_cache,
        created_at: std::time::Instant::now(),
        last_activity: std::time::Instant::now(),
        seek_generation: 0,
        duration_seconds: duration,
        container,
        video_codec,
        subtitle_files: vec![],
        hw_accel: (*state.hw_accel).clone(),
        source_resolution: (
            probe.video_streams.first().map(|v| v.width).unwrap_or(1920) as u32,
            probe
                .video_streams
                .first()
                .map(|v| v.height)
                .unwrap_or(1080) as u32,
        ),
        variants: None,
        is_live: false,
        provider_id: None,
        max_bitrate_kbps: None,
        keyframes: None,
        error: None,
        pending_segment: None,
        download_cancel: None,
        thumbnail_dir: Some(debrid_thumb_dir),
        debrid_cache_key: debrid_cache_key(body.info_hash.as_deref(), body.file_id),
        audio_metas: cached_audio_metas,
        force_video_transcode: false,
        corrupt_ranges: Vec::new(),
        overlay_spawned_starts: std::collections::HashSet::new(),
        highest_served_segment: None,
    };

    state.playback_sessions.insert(session_id, playback_session);

    // Register in dedup index
    if let Some(key) = debrid_cache_key(body.info_hash.as_deref(), body.file_id) {
        state.debrid_session_index.insert(key, session_id);
    }

    // Insert DB row so stream_play can find this session
    let cache_path_str = state
        .playback_sessions
        .get(&session_id)
        .map(|s| s.file_path.to_string_lossy().to_string())
        .unwrap_or_default();
    let _ = sqlx::query(
        r#"INSERT INTO debrid_stream_sessions
            (id, user_id, media_type, media_id, release_title, info_hash,
             debrid_provider, quality, filename, filesize, cache_path, file_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        ON CONFLICT (id) DO NOTHING"#,
    )
    .bind(session_id)
    .bind(auth.user_id)
    .bind(&body.media_type)
    .bind(body.media_id)
    .bind(&body.release_title)
    .bind(&body.info_hash)
    .bind(&body.debrid_provider)
    .bind(&body.quality)
    .bind(&filename)
    .bind(_filesize)
    .bind(&cache_path_str)
    .bind(body.file_id)
    .execute(&state.pool)
    .await;

    // Touch cache file so cleanup knows it's in use
    if let Some(ref info_hash) = body.info_hash {
        let pool = state.pool.clone();
        let pattern = format!("%{info_hash}%");
        tokio::spawn(async move {
            let _ = sqlx::query!(
                "UPDATE debrid_cache_files SET last_accessed_at = NOW() WHERE file_path LIKE $1",
                pattern,
            )
            .execute(&pool)
            .await;
        });
    }

    let base_url = format!("/api/v1/admin/playback/sessions/{session_id}");

    let audio_tracks: Vec<AudioTrackResponse> = probe
        .audio_streams
        .iter()
        .map(|a| AudioTrackResponse {
            stream_index: a.index,
            codec: a.codec.clone(),
            channels: a.channels,
            language: a.language.clone(),
            title: a.title.clone(),
            is_default: a.index == preferred_audio_idx,
        })
        .collect();

    let thumbnails_url = if has_thumbnails {
        Some(format!("{base_url}/thumbnails.vtt"))
    } else {
        None
    };

    let (probe_w, probe_h) = probe
        .video_streams
        .first()
        .map(|v| (v.width as u32, v.height as u32))
        .unwrap_or((0, 0));

    Json(SessionResponse {
        session_id: session_id.to_string(),
        method,
        playlist_url: Some(format!("{base_url}/master.m3u8")),
        direct_url: None,
        duration_seconds: duration,
        subtitles: vec![],
        audio_tracks,
        thumbnails_url,
        lufs: None,
        chapters: vec![],
        skip_segments: vec![],
        start_time: body.start_time,
        media_id: None,
        media_type: None,
        file_id: None,
        selected_audio_index: Some(preferred_audio_idx),
        selected_subtitle_index: None,
        video_width: if probe_w > 0 { Some(probe_w) } else { None },
        video_height: if probe_h > 0 { Some(probe_h) } else { None },
    })
    .into_response()
}

#[cfg(test)]
mod range_parsing {
    use super::parse_byte_range;

    /// `filesize` here comes from the session row, which is zero until the debrid
    /// resolve has reported one — and a suffix range then underflowed `file_size - 1`.
    #[test]
    fn a_zero_length_file_has_no_satisfiable_range() {
        for header in ["bytes=0-", "bytes=-500", "bytes=0-99", "bytes=10-20"] {
            assert!(
                parse_byte_range(header, 0).is_none(),
                "{header} parsed against an empty file"
            );
        }
    }

    #[test]
    fn ordinary_ranges_still_parse() {
        let r = parse_byte_range("bytes=0-99", 1000).unwrap();
        assert_eq!((r.start, r.end), (0, 99));
        let r = parse_byte_range("bytes=500-", 1000).unwrap();
        assert_eq!((r.start, r.end), (500, 999));
        let r = parse_byte_range("bytes=-100", 1000).unwrap();
        assert_eq!((r.start, r.end), (900, 999));
        let r = parse_byte_range("bytes=900-99999", 1000).unwrap();
        assert_eq!((r.start, r.end), (900, 999));
        assert!(parse_byte_range("bytes=1000-", 1000).is_none());
        assert!(parse_byte_range("bytes=-0", 1000).is_none());
        assert!(parse_byte_range("nonsense", 1000).is_none());
    }
}
