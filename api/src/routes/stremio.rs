//! Stremio addon endpoints.
//!
//! Implements the Stremio addon protocol with API key auth embedded in the URL path.
//! Routes: manifest, stream search, and play (resolve + redirect).

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{error, info, warn};
use uuid::Uuid;

use nats_jobs::StreamOption;

use crate::AppState;
use crate::routes::helpers::build_kobload_client;

/// Per-phase wall-clock timings for a single Stremio stream request so we can
/// see kobbler-side overhead vs the kobload round-trip at a glance.
#[derive(Debug, Default)]
struct StremioStreamTimings {
    auth_ms: u64,
    build_search_ms: u64,
    kobload_call_ms: u64,
    preferred_lookup_ms: u64,
    assemble_ms: u64,
    total_ms: u64,
}

impl StremioStreamTimings {
    fn log_summary(&self, stream_count: usize) {
        info!(
            stream_count,
            total_ms = self.total_ms,
            auth_ms = self.auth_ms,
            build_search_ms = self.build_search_ms,
            kobload_call_ms = self.kobload_call_ms,
            preferred_lookup_ms = self.preferred_lookup_ms,
            assemble_ms = self.assemble_ms,
            "Stremio stream request complete"
        );
    }
}

// =============================================================================
// Stremio Play Cache
// =============================================================================

const STREMIO_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(3600);

pub struct CachedStremioRedirect {
    pub session_id: Uuid,
    pub redirect_url: String,
    pub last_accessed_at: std::time::Instant,
    /// Fallback option IDs to try if this option fails (uncached, DMCA, etc).
    pub fallback_option_ids: Vec<String>,
}

/// Check cache for a live session. Returns the redirect URL if found.
fn check_stremio_cache(state: &AppState, option_id: &str) -> Option<String> {
    if let Some(mut entry) = state.stremio_play_cache.get_mut(option_id) {
        let cached = entry.value_mut();
        // Skip entries that only have fallbacks (no resolved session yet)
        if cached.session_id.is_nil() {
            return None;
        }
        if state.playback_sessions.contains_key(&cached.session_id) {
            cached.last_accessed_at = std::time::Instant::now();
            let url = cached.redirect_url.clone();
            drop(entry);
            info!(option_id = %option_id, "stremio play cache hit — reusing session");
            return Some(url);
        }
        // Session is gone from memory — clear the resolved session but keep fallbacks
        let cached = entry.value_mut();
        cached.session_id = Uuid::nil();
        cached.redirect_url.clear();
    }
    None
}

/// Remove entries older than TTL. Called opportunistically on cache access.
fn sweep_stremio_cache(
    cache: &dashmap::DashMap<String, CachedStremioRedirect>,
    locks: &dashmap::DashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>,
) {
    let now = std::time::Instant::now();
    cache.retain(|_, v| now.duration_since(v.last_accessed_at) < STREMIO_CACHE_TTL);
    // Sweep orphaned locks (no corresponding cache entry)
    locks.retain(|k, _| cache.contains_key(k));
}
use crate::video_metadata::VideoProviderRegistry;

// =============================================================================
// Stremio Protocol Types
// =============================================================================

#[derive(Serialize)]
struct StremioManifest {
    id: &'static str,
    version: &'static str,
    name: &'static str,
    description: &'static str,
    resources: Vec<&'static str>,
    types: Vec<&'static str>,
    catalogs: Vec<()>,
    #[serde(rename = "idPrefixes")]
    id_prefixes: Vec<&'static str>,
}

#[derive(Serialize)]
struct StremioStreamResponse {
    streams: Vec<StremioStream>,
}

#[derive(Serialize)]
struct StremioStream {
    name: String,
    title: String,
    url: String,
    #[serde(rename = "behaviorHints")]
    behavior_hints: StemioBehaviorHints,
}

#[derive(Serialize)]
struct StemioBehaviorHints {
    #[serde(rename = "bingeGroup")]
    binge_group: String,
}

// =============================================================================
// Stremio Client Capabilities
// =============================================================================

/// Stremio/mpv plays virtually everything natively — broad capabilities
/// so the decision engine chooses DirectPlay whenever possible.
fn stremio_capabilities() -> crate::streaming::decision::ClientCapabilities {
    crate::streaming::decision::ClientCapabilities {
        video_codecs: vec![
            "h264".into(),
            "hevc".into(),
            "av1".into(),
            "vp9".into(),
            "mpeg2video".into(),
            "mpeg4".into(),
            "vp8".into(),
            "theora".into(),
        ],
        audio_codecs: vec![
            "aac".into(),
            "ac3".into(),
            "eac3".into(),
            "dts".into(),
            "opus".into(),
            "flac".into(),
            "vorbis".into(),
            "mp3".into(),
            "truehd".into(),
            "pcm_s16le".into(),
            "pcm_s24le".into(),
        ],
        containers: vec![
            "matroska,webm".into(),
            "mkv".into(),
            "mp4".into(),
            "avi".into(),
            "mov".into(),
            "webm".into(),
            "ts".into(),
            "m2ts".into(),
        ],
        supports_interlaced: true,
    }
}

// =============================================================================
// Auth Helper
// =============================================================================

/// The library movie or episode a Stremio id names, when the library has it.
async fn library_target(
    pool: &crate::DbPool,
    media_type: &str,
    id: &str,
) -> (Option<Uuid>, Option<Uuid>) {
    match media_type {
        "movie" => {
            let movie_id: Option<Uuid> =
                sqlx::query_scalar("SELECT id FROM movies WHERE identifiers->>'imdb_id' = $1")
                    .bind(id)
                    .fetch_optional(pool)
                    .await
                    .ok()
                    .flatten();
            (movie_id, None)
        }
        "series" => {
            // "tt1234567:season:episode"
            let parts: Vec<&str> = id.splitn(3, ':').collect();
            let [imdb_id, season, episode] = parts[..] else {
                return (None, None);
            };
            let (Ok(sn), Ok(en)) = (season.parse::<i32>(), episode.parse::<i32>()) else {
                return (None, None);
            };
            let episode_id: Option<Uuid> = sqlx::query_scalar(
                r#"SELECT e.id FROM tv_episodes e
                   JOIN tv_seasons s ON s.id = e.season_id
                   JOIN tv_shows sh ON sh.id = s.show_id
                   WHERE sh.identifiers->>'imdb_id' = $1
                     AND s.season_identifier = $2
                     AND e.episode_identifier = $3"#,
            )
            .bind(imdb_id)
            .bind(sn.to_string())
            .bind(en.to_string())
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
            (None, episode_id)
        }
        _ => (None, None),
    }
}

async fn apply_preference(
    state: &AppState,
    user_id: Uuid,
    media_type: &str,
    id: &str,
    search_request: &mut nats_jobs::StreamSearchRequest,
) {
    let (movie_id, episode_id) = library_target(&state.pool, media_type, id).await;
    search_request.prefer =
        crate::debrid_outcomes::search_preference(&state.pool, user_id, movie_id, episode_id)
            .await
            .prefer;
    search_request.audio_languages = crate::streaming::decision::search_audio_languages(
        &state.runtime_config.get().await.preferred_audio_language,
    );
}

/// Options that failed for this user before are listed but never fallen back to.
fn fallback_ids<'a>(options: impl IntoIterator<Item = &'a StreamOption>) -> Vec<String> {
    options
        .into_iter()
        .filter(|o| o.playback != Some(nats_jobs::StreamPlayback::Failed))
        .take(5)
        .map(|o| o.id.clone())
        .collect()
}

async fn validate_stremio_key(pool: &crate::DbPool, api_key: &str) -> Option<Uuid> {
    sqlx::query_scalar!(
        r#"SELECT user_id FROM stremio_api_keys WHERE api_key = $1"#,
        api_key,
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

// =============================================================================
// Handlers
// =============================================================================

/// GET /stremio/{api_key}/manifest.json
pub async fn manifest(
    State(state): State<AppState>,
    Path(api_key): Path<String>,
) -> impl IntoResponse {
    if validate_stremio_key(&state.pool, &api_key).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    Json(StremioManifest {
        id: "app.kobbler.stremio",
        version: "1.0.0",
        name: "Kobbler",
        description: "Smart debrid streaming with quality profiles and release scoring",
        resources: vec!["stream"],
        types: vec!["movie", "series"],
        catalogs: vec![],
        id_prefixes: vec!["tt"],
    })
    .into_response()
}

/// GET /stremio/{api_key}/stream/{type}/{id}.json
pub async fn stream(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path((api_key, media_type, id)): Path<(String, String, String)>,
) -> impl IntoResponse {
    let stream_start = std::time::Instant::now();
    let mut timings = StremioStreamTimings::default();

    let auth_start = std::time::Instant::now();
    let Some(user_id) = validate_stremio_key(&state.pool, &api_key).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    timings.auth_ms = auth_start.elapsed().as_millis() as u64;

    // Strip trailing .json from the id if the router matched it that way
    let id = id.strip_suffix(".json").unwrap_or(&id);

    let build_search_start = std::time::Instant::now();
    let mut search_request = match media_type.as_str() {
        "movie" => match build_movie_search(id, &state).await {
            Ok(r) => r,
            Err(resp) => return resp,
        },
        "series" => match build_series_search(id, &state).await {
            Ok(r) => r,
            Err(resp) => return resp,
        },
        _ => {
            return Json(StremioStreamResponse { streams: vec![] }).into_response();
        }
    };
    timings.build_search_ms = build_search_start.elapsed().as_millis() as u64;

    let preferred_lookup_start = std::time::Instant::now();
    apply_preference(&state, user_id, &media_type, id, &mut search_request).await;
    timings.preferred_lookup_ms = preferred_lookup_start.elapsed().as_millis() as u64;

    let runtime = state.runtime_config.get().await;
    let client = match build_kobload_client(&runtime) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let kobload_call_start = std::time::Instant::now();
    let search_result = match client.stream_search(&search_request).await {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "stremio stream search failed");
            return Json(StremioStreamResponse { streams: vec![] }).into_response();
        }
    };
    timings.kobload_call_ms = kobload_call_start.elapsed().as_millis() as u64;

    let base_url = derive_base_url(&headers, &runtime);

    let assemble_start = std::time::Instant::now();
    let options = search_result.options;

    let streams: Vec<StremioStream> = options
        .iter()
        .map(|opt| {
            let resolution = opt
                .quality
                .as_ref()
                .and_then(|q| q.get("resolution").and_then(|r| r.as_str()))
                .unwrap_or("SD");

            // Name: provider badge + resolution (like Torrentio does)
            let provider_short = match opt.debrid_provider.as_str() {
                "realdebrid" => "RD",
                "alldebrid" => "AD",
                "torbox" => "TB",
                "premiumize" => "PM",
                "debridlink" => "DL",
                other => other,
            };
            let name = format!("Kobbler {provider_short}\n{resolution}");

            // Title: release name + metadata line
            let mut meta_parts: Vec<String> = Vec::new();

            // Size
            if opt.size > 0 {
                let gb = opt.size as f64 / 1_073_741_824.0;
                if gb >= 1.0 {
                    meta_parts.push(format!("\u{1F4BE} {gb:.1} GB"));
                } else {
                    meta_parts.push(format!("\u{1F4BE} {} MB", opt.size / 1_048_576));
                }
            }

            // Quality badges
            let quality_badges = build_quality_badges(opt);
            if !quality_badges.is_empty() {
                meta_parts.push(quality_badges);
            }

            // Source (codec + audio)
            let codec_audio = build_codec_audio_line(opt);
            if !codec_audio.is_empty() {
                meta_parts.push(codec_audio);
            }

            // Release group
            if let Some(group) = opt
                .quality
                .as_ref()
                .and_then(|q| q.get("release_group").and_then(|v| v.as_str()))
            {
                meta_parts.push(format!("\u{2699}\u{FE0F} {group}"));
            }

            let title = format!("{}\n{}", opt.release_title, meta_parts.join(" \u{00b7} "));

            StremioStream {
                name,
                title,
                // type/id let play() re-run the search for fallbacks when its
                // in-memory chain is gone (API restart, cache TTL) but Stremio
                // is still replaying a stream list it cached earlier.
                url: format!(
                    "{}/stremio/{}/play/{}?type={}&id={}",
                    base_url, api_key, opt.id, media_type, id
                ),
                behavior_hints: StemioBehaviorHints {
                    binge_group: format!("kobbler-{}-{}", opt.debrid_provider, resolution),
                },
            }
        })
        .collect();

    // Pre-populate fallback chains so play() can retry on failure
    for (i, opt) in options.iter().enumerate() {
        state.stremio_play_cache.insert(
            opt.id.clone(),
            CachedStremioRedirect {
                session_id: Uuid::nil(),
                redirect_url: String::new(),
                last_accessed_at: std::time::Instant::now(),
                fallback_option_ids: fallback_ids(&options[i + 1..]),
            },
        );
    }
    timings.assemble_ms = assemble_start.elapsed().as_millis() as u64;
    timings.total_ms = stream_start.elapsed().as_millis() as u64;
    timings.log_summary(streams.len());

    Json(StremioStreamResponse { streams }).into_response()
}

/// Parsed option_id components: (info_hash, nzb_url, debrid_provider_id, file_id).
type ParsedOptionId = (Option<String>, Option<String>, Uuid, Option<i32>);

/// Parse an option_id into its components for resolve.
fn parse_option_id(option_id: &str) -> Option<ParsedOptionId> {
    if let Some(rest) = option_id.strip_prefix("torrent_") {
        let (hash_part, provider_str) = rest.rsplit_once('_')?;
        let provider_id = Uuid::parse_str(provider_str).ok()?;
        let (hash, file_idx) = if let Some(f_pos) = hash_part.rfind("_f") {
            let idx_str = &hash_part[f_pos + 2..];
            if let Ok(idx) = idx_str.parse::<i32>() {
                (&hash_part[..f_pos], Some(idx))
            } else {
                (hash_part, None)
            }
        } else {
            (hash_part, None)
        };
        Some((Some(hash.to_string()), None, provider_id, file_idx))
    } else if let Some(rest) = option_id.strip_prefix("usenet_") {
        let (guid, provider_str) = rest.rsplit_once('_')?;
        let provider_id = Uuid::parse_str(provider_str).ok()?;
        Some((None, Some(guid.to_string()), provider_id, None))
    } else {
        None
    }
}

/// Try to resolve a single option_id. Returns (session_id, redirect_url) on success.
async fn try_resolve_option(
    state: &AppState,
    user_id: Uuid,
    option_id: &str,
    base_url: &str,
    api_key: &str,
) -> Option<(Uuid, String)> {
    let (info_hash, nzb_url, debrid_provider_id, file_id) = parse_option_id(option_id)?;

    let auth = crate::routes::PlaybackAuth {
        user_id,
        role: "admin".to_string(),
        is_kids: false,
    };
    let body = crate::routes::admin::debrid_stream::StreamResolveBody {
        debrid_provider_id,
        info_hash,
        magnet_url: None,
        nzb_url,
        file_id,
        media_type: "movie".to_string(),
        media_id: Some(Uuid::new_v4()),
        release_title: option_id.to_string(),
        debrid_provider: debrid_provider_id.to_string(),
        quality: None,
        capabilities: Some(stremio_capabilities()),
        estimated_duration: None,
        start_time: None,
        auto_create: None,
        preferred_audio_language: None,
    };

    let resolve_resp =
        crate::routes::admin::debrid_stream::stream_resolve(State(state.clone()), auth, Json(body))
            .await
            .into_response();

    let (parts, body) = resolve_resp.into_parts();
    if !parts.status.is_success() {
        warn!(option_id, status = %parts.status, "stremio resolve failed");
        return None;
    }

    let body_bytes = axum::body::to_bytes(body, 1024 * 1024).await.ok()?;
    let session_resp: serde_json::Value = serde_json::from_slice(&body_bytes).ok()?;

    let sid = session_resp
        .get("session_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())?;

    let redirect_url = format!("{base_url}/stremio/{api_key}/file/{sid}");

    // Cache the successful resolution, preserving fallbacks
    let existing_fallbacks = state
        .stremio_play_cache
        .get(option_id)
        .map(|e| e.fallback_option_ids.clone())
        .unwrap_or_default();
    state.stremio_play_cache.insert(
        option_id.to_string(),
        CachedStremioRedirect {
            session_id: sid,
            redirect_url: redirect_url.clone(),
            last_accessed_at: std::time::Instant::now(),
            fallback_option_ids: existing_fallbacks,
        },
    );

    Some((sid, redirect_url))
}

/// Content the option was listed for, carried on the play URL.
#[derive(Deserialize)]
pub struct PlayQuery {
    #[serde(rename = "type")]
    media_type: Option<String>,
    id: Option<String>,
}

/// Re-run the search stream() listed the option from, ranked the same way, and
/// return the fallback chain stream() would have cached for `option_id`.
async fn rebuild_fallbacks(
    state: &AppState,
    user_id: Uuid,
    media_type: &str,
    id: &str,
    option_id: &str,
) -> Vec<String> {
    let search_request = match media_type {
        "movie" => build_movie_search(id, state).await,
        "series" => build_series_search(id, state).await,
        _ => return vec![],
    };
    let Ok(mut search_request) = search_request else {
        return vec![];
    };
    apply_preference(state, user_id, media_type, id, &mut search_request).await;
    let runtime = state.runtime_config.get().await;
    let Ok(client) = build_kobload_client(&runtime) else {
        return vec![];
    };
    match client.stream_search(&search_request).await {
        Ok(r) => fallback_ids(r.options.iter().filter(|o| o.id != option_id)),
        Err(e) => {
            warn!(error = %e, media_type, id, "stremio fallback search failed");
            vec![]
        }
    }
}

/// GET /stremio/{api_key}/play/{option_id}
///
/// Resolves the stream via our debrid proxy — Stremio gets a direct play URL
/// backed by HybridIo (local cache + HTTP fallback). Downloads cache locally
/// for instant seeks on replay. Stremio never touches the debrid CDN directly.
///
/// On failure (uncached, DMCA, network error), automatically tries fallback
/// options from the same search results.
pub async fn play(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path((api_key, option_id)): Path<(String, String)>,
    Query(query): Query<PlayQuery>,
) -> impl IntoResponse {
    let user_id = match validate_stremio_key(&state.pool, &api_key).await {
        Some(id) => id,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    // Fast path: check cache without locking
    if let Some(url) = check_stremio_cache(&state, &option_id) {
        return Redirect::temporary(&url).into_response();
    }

    // Serialize concurrent resolves for the same option_id
    let resolve_lock = state
        .stremio_resolve_locks
        .entry(option_id.clone())
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = resolve_lock.lock().await;

    // Double-check: another request may have resolved while we waited
    if let Some(url) = check_stremio_cache(&state, &option_id) {
        return Redirect::temporary(&url).into_response();
    }

    // Opportunistically sweep expired entries
    sweep_stremio_cache(&state.stremio_play_cache, &state.stremio_resolve_locks);

    let base_url = derive_base_url(&headers, &state.runtime_config.get().await);

    // Try the requested option first
    if let Some((_sid, redirect_url)) =
        try_resolve_option(&state, user_id, &option_id, &base_url, &api_key).await
    {
        return Redirect::temporary(&redirect_url).into_response();
    }

    // Primary failed — try fallback options
    let mut fallbacks = state
        .stremio_play_cache
        .get(&option_id)
        .map(|e| e.fallback_option_ids.clone())
        .unwrap_or_default();

    if fallbacks.is_empty()
        && let (Some(media_type), Some(id)) = (&query.media_type, &query.id)
    {
        fallbacks = rebuild_fallbacks(&state, user_id, media_type, id, &option_id).await;
        info!(option_id = %option_id, count = fallbacks.len(), "stremio play: rebuilt fallbacks from a fresh search");
    }

    for alt_id in &fallbacks {
        // Check if this fallback already has a cached session
        if let Some(url) = check_stremio_cache(&state, alt_id) {
            info!(option_id = %option_id, fallback = %alt_id, "stremio fallback cache hit");
            return Redirect::temporary(&url).into_response();
        }

        if let Some((_sid, redirect_url)) =
            try_resolve_option(&state, user_id, alt_id, &base_url, &api_key).await
        {
            info!(option_id = %option_id, fallback = %alt_id, "stremio fallback succeeded");
            return Redirect::temporary(&redirect_url).into_response();
        }
    }

    warn!(option_id = %option_id, fallback_count = fallbacks.len(), "stremio play: all options exhausted");
    StatusCode::SERVICE_UNAVAILABLE.into_response()
}

/// GET /stremio/{api_key}/file/{session_id}
///
/// Stremio direct play endpoint — serves the debrid file with Range support.
/// Validates the stremio API key and delegates to the same logic as stream_play.
pub async fn stream_file(
    State(state): State<AppState>,
    Path((api_key, session_id)): Path<(String, Uuid)>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    info!(session_id = %session_id, "stremio stream_file request");

    let Some(user_id) = validate_stremio_key(&state.pool, &api_key).await else {
        warn!(session_id = %session_id, "stremio stream_file: invalid API key");
        return StatusCode::UNAUTHORIZED.into_response();
    };

    info!(session_id = %session_id, "stremio stream_file: key valid, calling stream_play");

    // stream_play scopes the session lookup to its owner; the session was
    // created under this key's user in try_resolve_option.
    let auth = crate::routes::PlaybackAuth {
        user_id,
        role: "stremio".to_string(),
        is_kids: false,
    };
    let resp = crate::routes::admin::debrid_play::stream_play(
        State(state),
        auth,
        axum::extract::Path(session_id),
        headers,
    )
    .await
    .into_response();

    info!(session_id = %session_id, status = %resp.status(), "stremio stream_file: stream_play returned");
    resp
}

// =============================================================================
// Search Request Builders
// =============================================================================

async fn build_movie_search(
    imdb_id: &str,
    state: &AppState,
) -> Result<nats_jobs::StreamSearchRequest, axum::response::Response> {
    let row = sqlx::query!(
        r#"
        SELECT id, title, year, identifiers, alternate_titles, quality_profile_id, preferred_languages
        FROM movies
        WHERE identifiers->>'imdb_id' = $1
        "#,
        imdb_id,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        error!(error = %e, "stremio movie lookup failed");
        (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
    })?;

    if let Some(movie) = row {
        // Movie is in library — use its full metadata for best search results
        let identifiers = nats_jobs::MovieIdentifiers {
            tmdb_id: movie
                .identifiers
                .get("tmdb_id")
                .and_then(|v| v.as_str())
                .map(String::from),
            imdb_id: Some(imdb_id.to_string()),
        };

        let quality_overrides =
            movie
                .preferred_languages
                .as_ref()
                .map(|langs| nats_jobs::QualityOverrides {
                    preferred_languages: Some(langs.clone()),
                    ..Default::default()
                });

        let media_want = nats_jobs::MediaWant::Movie {
            title: movie.title,
            year: movie.year.map(|y| y as u32),
            release_date: None,
            identifiers: Some(identifiers),
            alternate_titles: Some(movie.alternate_titles),
            quality_profile_id: movie.quality_profile_id,
            quality_overrides,
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
            prefer: None,
            audio_languages: Vec::new(),
        })
    } else {
        // Not in library — resolve IMDB ID to metadata via providers
        resolve_imdb_movie(imdb_id, state).await
    }
}

async fn build_series_search(
    stremio_id: &str,
    state: &AppState,
) -> Result<nats_jobs::StreamSearchRequest, axum::response::Response> {
    // Parse "tt1234567:season:episode"
    let parts: Vec<&str> = stremio_id.splitn(3, ':').collect();
    if parts.len() != 3 {
        return Err(Json(json!({"streams": []})).into_response());
    }
    let imdb_id = parts[0];
    let season_num: i32 = parts[1]
        .parse()
        .map_err(|_| Json(StremioStreamResponse { streams: vec![] }).into_response())?;
    let episode_num: i32 = parts[2]
        .parse()
        .map_err(|_| Json(StremioStreamResponse { streams: vec![] }).into_response())?;

    let show = sqlx::query!(
        r#"
        SELECT id, title, year, identifiers, alternate_titles, quality_profile_id, preferred_languages
        FROM tv_shows
        WHERE identifiers->>'imdb_id' = $1
        "#,
        imdb_id,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        error!(error = %e, "stremio show lookup failed");
        (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
    })?;

    let season_identifier = season_num.to_string();
    let episode_identifier = episode_num.to_string();

    if let Some(s) = show {
        // Show is in library — use its full metadata
        let identifiers = nats_jobs::TvIdentifiers {
            tmdb_id: s
                .identifiers
                .get("tmdb_id")
                .and_then(|v| v.as_str())
                .map(String::from),
            tvdb_id: s
                .identifiers
                .get("tvdb_id")
                .and_then(|v| v.as_str())
                .map(String::from),
            imdb_id: Some(imdb_id.to_string()),
        };

        // Try to find the specific episode for richer metadata
        let ep = sqlx::query!(
            r#"
            SELECT e.title, e.air_date
            FROM tv_episodes e
            JOIN tv_seasons s ON s.id = e.season_id
            WHERE s.show_id = $1
              AND s.season_identifier = $2
              AND e.episode_identifier = $3
            "#,
            s.id,
            season_identifier,
            episode_identifier,
        )
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten();

        let quality_overrides =
            s.preferred_languages
                .as_ref()
                .map(|langs| nats_jobs::QualityOverrides {
                    preferred_languages: Some(langs.clone()),
                    ..Default::default()
                });

        let media_want = nats_jobs::MediaWant::TvEpisode {
            show_name: s.title,
            season: season_identifier,
            episode: episode_identifier,
            year: s.year.map(|y| y as u32),
            air_date: ep.as_ref().and_then(|e| e.air_date.map(|d| d.to_string())),
            identifiers: Some(identifiers),
            alternate_titles: Some(s.alternate_titles),
            episode_title: ep.as_ref().and_then(|e| e.title.clone()),
            quality_profile_id: s.quality_profile_id,
            quality_overrides,
        };

        Ok(nats_jobs::StreamSearchRequest {
            media_want: Some(media_want),
            query: String::new(),
            media_type: Some("tv".to_string()),
            quality_profile_id: s.quality_profile_id,
            limit: None,
            include_uncached: None,
            prefer: None,
            audio_languages: Vec::new(),
        })
    } else {
        // Not in library — resolve IMDB ID to metadata via providers
        resolve_imdb_tv_episode(imdb_id, season_num as u32, episode_num as u32, state).await
    }
}

// =============================================================================
// IMDB Resolution
// =============================================================================

/// Resolve an IMDB movie ID to a search request via metadata providers.
///
/// Tries TMDB first (richest metadata: alternate titles, release date, all IDs),
/// falls back to Cinemeta (title + year only) if TMDB fails.
async fn resolve_imdb_movie(
    imdb_id: &str,
    state: &AppState,
) -> Result<nats_jobs::StreamSearchRequest, axum::response::Response> {
    let registry = VideoProviderRegistry::new(state.pool.clone());

    // Try TMDB: IMDB ID → TMDB ID → full movie details + alternate titles
    if let Ok((movie_ids, _)) = registry.find_by_imdb_id(imdb_id).await
        && let Some(tmdb_id) = movie_ids.first()
        && let Ok(details) = registry.get_movie("tmdb", tmdb_id).await
    {
        let alternate_titles = registry
            .get_movie_alternate_titles(tmdb_id, &details.title)
            .await
            .unwrap_or_default();

        let identifiers = nats_jobs::MovieIdentifiers {
            tmdb_id: Some(tmdb_id.clone()),
            imdb_id: Some(imdb_id.to_string()),
        };

        let media_want = nats_jobs::MediaWant::Movie {
            title: details.title,
            year: details.year.map(|y| y as u32),
            release_date: details.release_date,
            identifiers: Some(identifiers),
            alternate_titles: if alternate_titles.is_empty() {
                None
            } else {
                Some(alternate_titles)
            },
            quality_profile_id: None,
            quality_overrides: None,
            in_library: None,
            library_video_height: None,
        };

        return Ok(nats_jobs::StreamSearchRequest {
            media_want: Some(media_want),
            query: String::new(),
            media_type: Some("movie".to_string()),
            quality_profile_id: None,
            limit: None,
            include_uncached: None,
            prefer: None,
            audio_languages: Vec::new(),
        });
    }

    // Fallback: Cinemeta (Stremio's own metadata service)
    let http_client = crate::http::new_client();
    let cinemeta = crate::video_metadata::cinemeta::CinemetaClient::new(http_client);
    if let Ok(result) = cinemeta.lookup_movie(imdb_id).await {
        let identifiers = nats_jobs::MovieIdentifiers {
            tmdb_id: None,
            imdb_id: Some(imdb_id.to_string()),
        };

        let media_want = nats_jobs::MediaWant::Movie {
            title: result.title,
            year: result.year,
            release_date: None,
            identifiers: Some(identifiers),
            alternate_titles: None,
            quality_profile_id: None,
            quality_overrides: None,
            in_library: None,
            library_video_height: None,
        };

        return Ok(nats_jobs::StreamSearchRequest {
            media_want: Some(media_want),
            query: String::new(),
            media_type: Some("movie".to_string()),
            quality_profile_id: None,
            limit: None,
            include_uncached: None,
            prefer: None,
            audio_languages: Vec::new(),
        });
    }

    // All providers failed — bare minimum search with just the IMDB ID
    warn!(imdb_id, "all providers failed to resolve movie IMDB ID");
    let identifiers = nats_jobs::MovieIdentifiers {
        tmdb_id: None,
        imdb_id: Some(imdb_id.to_string()),
    };

    let media_want = nats_jobs::MediaWant::Movie {
        title: imdb_id.to_string(),
        year: None,
        release_date: None,
        identifiers: Some(identifiers),
        alternate_titles: None,
        quality_profile_id: None,
        quality_overrides: None,
        in_library: None,
        library_video_height: None,
    };

    Ok(nats_jobs::StreamSearchRequest {
        media_want: Some(media_want),
        query: String::new(),
        media_type: Some("movie".to_string()),
        quality_profile_id: None,
        limit: None,
        include_uncached: None,
        prefer: None,
        audio_languages: Vec::new(),
    })
}

/// Resolve an IMDB TV show ID + season/episode to a search request via metadata providers.
///
/// Tries TMDB first (show details + season/episode metadata), then TVMaze IMDB lookup,
/// falls back to Cinemeta if both fail.
async fn resolve_imdb_tv_episode(
    imdb_id: &str,
    season_num: u32,
    episode_num: u32,
    state: &AppState,
) -> Result<nats_jobs::StreamSearchRequest, axum::response::Response> {
    let registry = VideoProviderRegistry::new(state.pool.clone());
    let season_str = season_num.to_string();
    let episode_str = episode_num.to_string();

    // Try TMDB: IMDB ID → TMDB show ID → show details + season details
    if let Ok((_, tv_ids)) = registry.find_by_imdb_id(imdb_id).await
        && let Some(tmdb_id) = tv_ids.first()
        && let Ok(show) = registry.get_tv_show("tmdb", tmdb_id).await
    {
        let alternate_titles = registry
            .get_tv_alternate_titles(tmdb_id, &show.title)
            .await
            .unwrap_or_default();

        // Try to get episode details for air date and title
        let (episode_title, air_date) =
            if let Ok(season) = registry.get_season("tmdb", tmdb_id, season_num).await {
                let ep = season
                    .episodes
                    .iter()
                    .find(|e| e.episode_number == episode_num);
                (
                    ep.and_then(|e| e.title.clone()),
                    ep.and_then(|e| e.air_date.clone()),
                )
            } else {
                (None, None)
            };

        let identifiers = nats_jobs::TvIdentifiers {
            tmdb_id: Some(tmdb_id.clone()),
            tvdb_id: show.identifiers.tvdb_id.clone(),
            imdb_id: Some(imdb_id.to_string()),
        };

        let media_want = nats_jobs::MediaWant::TvEpisode {
            show_name: show.title,
            season: season_str,
            episode: episode_str,
            year: show.year.map(|y| y as u32),
            air_date,
            identifiers: Some(identifiers),
            alternate_titles: if alternate_titles.is_empty() {
                None
            } else {
                Some(alternate_titles)
            },
            episode_title,
            quality_profile_id: None,
            quality_overrides: None,
        };

        return Ok(nats_jobs::StreamSearchRequest {
            media_want: Some(media_want),
            query: String::new(),
            media_type: Some("tv".to_string()),
            quality_profile_id: None,
            limit: None,
            include_uncached: None,
            prefer: None,
            audio_languages: Vec::new(),
        });
    }

    // Fallback: TVMaze IMDB lookup
    let http_client = crate::http::new_client();
    let tvmaze = crate::video_metadata::tvmaze::TvMazeClient::new(http_client.clone());
    if let Ok(result) = tvmaze.lookup_by_imdb(imdb_id).await {
        let identifiers = nats_jobs::TvIdentifiers {
            tmdb_id: None,
            tvdb_id: result.tvdb_id,
            imdb_id: Some(imdb_id.to_string()),
        };

        let media_want = nats_jobs::MediaWant::TvEpisode {
            show_name: result.title,
            season: season_str,
            episode: episode_str,
            year: result.year,
            air_date: None,
            identifiers: Some(identifiers),
            alternate_titles: None,
            episode_title: None,
            quality_profile_id: None,
            quality_overrides: None,
        };

        return Ok(nats_jobs::StreamSearchRequest {
            media_want: Some(media_want),
            query: String::new(),
            media_type: Some("tv".to_string()),
            quality_profile_id: None,
            limit: None,
            include_uncached: None,
            prefer: None,
            audio_languages: Vec::new(),
        });
    }

    // Fallback: Cinemeta
    let cinemeta = crate::video_metadata::cinemeta::CinemetaClient::new(http_client);
    if let Ok(result) = cinemeta.lookup_series(imdb_id).await {
        let identifiers = nats_jobs::TvIdentifiers {
            tmdb_id: None,
            tvdb_id: None,
            imdb_id: Some(imdb_id.to_string()),
        };

        let media_want = nats_jobs::MediaWant::TvEpisode {
            show_name: result.title,
            season: season_str,
            episode: episode_str,
            year: result.year,
            air_date: None,
            identifiers: Some(identifiers),
            alternate_titles: None,
            episode_title: None,
            quality_profile_id: None,
            quality_overrides: None,
        };

        return Ok(nats_jobs::StreamSearchRequest {
            media_want: Some(media_want),
            query: String::new(),
            media_type: Some("tv".to_string()),
            quality_profile_id: None,
            limit: None,
            include_uncached: None,
            prefer: None,
            audio_languages: Vec::new(),
        });
    }

    // All providers failed — bare minimum search with IMDB ID
    warn!(
        imdb_id,
        season_num, episode_num, "all providers failed to resolve TV IMDB ID"
    );
    let identifiers = nats_jobs::TvIdentifiers {
        tmdb_id: None,
        tvdb_id: None,
        imdb_id: Some(imdb_id.to_string()),
    };

    let media_want = nats_jobs::MediaWant::TvEpisode {
        show_name: imdb_id.to_string(),
        season: season_str,
        episode: episode_str,
        year: None,
        air_date: None,
        identifiers: Some(identifiers),
        alternate_titles: None,
        episode_title: None,
        quality_profile_id: None,
        quality_overrides: None,
    };

    Ok(nats_jobs::StreamSearchRequest {
        media_want: Some(media_want),
        query: String::new(),
        media_type: Some("tv".to_string()),
        quality_profile_id: None,
        limit: None,
        include_uncached: None,
        prefer: None,
        audio_languages: Vec::new(),
    })
}

// =============================================================================
// Helpers
// =============================================================================

/// Build HDR/DV/Atmos badges from quality data.
fn build_quality_badges(opt: &StreamOption) -> String {
    let Some(quality) = &opt.quality else {
        return String::new();
    };
    let mut badges = Vec::new();

    if quality
        .get("dolby_vision")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        badges.push("DV");
    }
    if quality
        .get("hdr10plus")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        badges.push("HDR10+");
    } else if quality
        .get("hdr10")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        badges.push("HDR10");
    } else if quality
        .get("hdr")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        badges.push("HDR");
    }
    if quality
        .get("atmos")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        badges.push("Atmos");
    }

    badges.join(" ")
}

/// Build codec + audio description from quality data.
fn build_codec_audio_line(opt: &StreamOption) -> String {
    let Some(quality) = &opt.quality else {
        return String::new();
    };
    let mut parts = Vec::new();

    if let Some(source) = quality.get("source").and_then(|v| v.as_str()) {
        parts.push(source.to_uppercase());
    }

    if let Some(codec) = quality.get("codec").and_then(|v| v.as_str()) {
        let display = match codec {
            "avc" => "H.264",
            "hevc" => "H.265",
            "av1" => "AV1",
            other => other,
        };
        parts.push(display.to_string());
    }

    if let Some(audio) = quality.get("audio").and_then(|v| v.as_str()) {
        parts.push(audio.to_string());
    }

    parts.join(" ")
}

/// Derive the external base URL for Stremio play links.
///
/// Priority:
/// 1. `stremio_base_url` from runtime config (if set)
/// 2. `X-Forwarded-Proto` + `Host` headers (reverse proxy)
/// 3. `Host` header with https
/// 4. Fallback to empty string (relative URLs)
fn derive_base_url(
    headers: &axum::http::HeaderMap,
    _runtime: &crate::runtime_config::RuntimeConfig,
) -> String {
    // TODO: add stremio_base_url to runtime config for explicit override
    // For now, derive from request headers

    let host = headers
        .get("host")
        .or_else(|| headers.get("x-forwarded-host"))
        .and_then(|v| v.to_str().ok());

    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("https");

    match host {
        Some(h) => format!("{proto}://{h}"),
        None => String::new(), // relative URLs as fallback
    }
}
