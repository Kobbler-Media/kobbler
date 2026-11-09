//! Streaming and playback route handlers.
//!
//! Manages playback sessions, serves HLS playlists/segments, and handles
//! direct play with HTTP range requests. Uses Plex-style segment caching:
//! segments are kept across seeks and served from cache when possible.

use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Response, StatusCode, header},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::ffmpeg::mkv_cue_cache::{CueTrimPolicy, patch_for_local_file, patched_stream};
use crate::ffmpeg::mkv_splice::IndexPatch;
use crate::ffmpeg::transcode::{self, AudioMode, TranscodeArgs, VideoMode};
use crate::jobs::handlers::SessionCleanupPayload;
use crate::repositories::{
    audio_tracks, episode_skip_segments, iptv_recordings, movie_files, movie_skip_segments,
    subtitle_tracks, tv_episode_files, video_chapters,
};
use crate::routes::{AuthUser, PlaybackAuth};
use crate::streaming::byte_range::{ByteRange, parse_byte_range};
use crate::streaming::decision::{
    self, AAC_ENCODER, AudioTrackMeta, ClientCapabilities, FileMeta, PlaybackMethod,
    SubtitleTrackMeta, ViewerPrefs,
};
use crate::streaming::feed::FeedStatus;
use crate::streaming::persist::Progress;
use crate::streaming::segment_cache::SegmentCache;
use crate::streaming::session::{
    MediaType, PendingSegmentRequest, PlaybackSession, SessionError, SubtitleFile,
};
use crate::streaming::subtitle;
use crate::streaming::variant::{self, QualityVariant, VariantSpec};
use crate::types::MediaScope;

// =============================================================================
// Stall detection thresholds
// =============================================================================

/// How long the segment handler will wait for a single requested segment to
/// appear on disk before declaring the session stalled. Matches HLS live-edge
/// tolerances and gives ffmpeg a realistic catch-up window.
const STALL_THRESHOLD: std::time::Duration = std::time::Duration::from_secs(30);

/// How long after `TranscodeProcess::started_at` the playlist handler waits
/// before declaring an empty playlist a startup stall. Long enough for slow
/// debrid + HybridIo probe + ffmpeg first-segment emission.
const STARTUP_THRESHOLD: std::time::Duration = std::time::Duration::from_secs(30);

// =============================================================================
// Terminal error responses (HTTP 500 + machine-readable code)
// =============================================================================

/// Build a terminal error response for the player. hls.js retries 5xx a
/// bounded number of times then fires a fatal `Hls.Events.ERROR` carrying the
/// JSON body in `data.response.text`. The frontend switches on the `error`
/// code and shows a user-facing message.
///
/// 500 (not 503) is deliberate: 503 means "retry me", which is the opposite
/// of the signal we want for terminal errors.
fn error_response(code: &'static str, message: &str) -> Response<Body> {
    let body = json!({ "error": code, "message": message }).to_string();
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .header("X-Stream-Error", code)
        .header("X-Stream-Error-Message", message)
        .body(Body::from(body))
        .unwrap()
}

/// Debrid session probe is still running: return 503 + Retry-After so hls.js
/// retries instead of failing fatally (it treats 4xx on manifest load as
/// terminal, skipping the retry budget).
fn session_pending_response() -> Response<Body> {
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header(header::RETRY_AFTER, "2")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from("session not ready — probe still in progress"))
        .unwrap()
}

// =============================================================================
// Request/Response Types
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub media_type: MediaType,
    /// The exact file to play. Optional: a client that only knows what the viewer
    /// picked sends [media_id] instead and lets the server choose, which is one
    /// fewer round trip between pressing play and the first byte.
    pub file_id: Option<String>,
    /// Movie or episode id, resolved to its preferred available file server-side.
    /// Ignored when [file_id] is given; one of the two is required.
    pub media_id: Option<String>,
    pub capabilities: ClientCapabilities,
    pub audio_track_index: Option<i32>,
    pub subtitle_track_index: Option<i32>,
    /// Force HLS transcode even when direct play is possible.
    /// Enables ABR quality variants so clients can adapt to bandwidth changes.
    #[serde(default)]
    pub force_transcode: bool,
    /// Maximum video bitrate in kbps. When set, the primary stream is
    /// bitrate-capped and ABR variants above this limit are excluded.
    /// Implies force_transcode.
    pub max_bitrate_kbps: Option<u32>,
    /// Start transcoding from this position (seconds). When resuming playback,
    /// pass the saved position so FFmpeg starts directly there instead of
    /// requiring a separate seek after session creation.
    pub start_time: Option<f64>,
    /// Per-user preferred audio language (3-letter ISO code, e.g. "eng"). Used
    /// only when [audio_track_index] is not set; takes precedence over the
    /// server-wide config default so each profile can override.
    pub preferred_audio_language: Option<String>,
    /// Title of the audio track the viewer last picked by hand for this show or movie.
    /// Wins over [preferred_audio_language] when a track of that title is present.
    pub preferred_audio_title: Option<String>,
    /// Per-user preferred subtitle language. See [preferred_audio_language].
    pub preferred_subtitle_language: Option<String>,
    /// When false, the user has disabled subtitles by default — skip
    /// subtitle selection entirely (no burn-in, no track exposed as selected).
    /// When None, treated as true.
    pub subtitles_enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionResponse {
    pub session_id: String,
    pub method: PlaybackMethod,
    pub playlist_url: Option<String>,
    pub direct_url: Option<String>,
    pub duration_seconds: f64,
    pub subtitles: Vec<SubtitleTrackResponse>,
    pub audio_tracks: Vec<AudioTrackResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnails_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lufs: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub chapters: Vec<ChapterResponse>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skip_segments: Vec<SkipSegmentResponse>,
    /// Where this session's timeline begins, when it is not the start of the title.
    /// Set only for a transcode, whose playlist is produced from the resume point and
    /// serves it as its own zero; null on direct play, where the whole file is served
    /// and the position is the client's to seek to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<f64>,
    /// The resolved media entity ID (populated when auto-creating from discover).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_id: Option<String>,
    /// The file this session plays. Echoed back because a client that asked by
    /// media_id never saw it, and skip-segment events are addressed to a file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    /// The effective media type after resolution (e.g. "episode" when auto-creating
    /// a TV episode from discover). Helps frontend navigate to the correct page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Audio stream index actually selected for playback. Lets the client show
    /// the correct track as the active selection without guessing from is_default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_audio_index: Option<i32>,
    /// Debrid sessions only: the audio track the language preference picks, which a
    /// direct-play client should switch to when it is not the file's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_audio_stream_index: Option<i32>,
    /// Subtitle stream index actually selected for playback (None when subtitles disabled).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_subtitle_index: Option<i32>,
    /// Source video width in pixels. Lets the client pick a saved aspect-ratio
    /// preference (FIT/CROP/STRETCH) keyed by video framing before the first
    /// frame is decoded. None when not yet probed (e.g. debrid pre-resolve).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_width: Option<u32>,
    /// Source video height in pixels. See [video_width].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_height: Option<u32>,
    /// Where the picture sits inside the frame when the encode has bars baked in, as
    /// fractions of it: what a client keys an aspect choice on, rather than the frame.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picture: Option<crate::media_cache::picture_area::PictureArea>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterResponse {
    pub index: i32,
    pub title: Option<String>,
    pub start_seconds: f64,
    pub end_seconds: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkipSegmentResponse {
    pub segment_type: String,
    pub start_seconds: f64,
    pub end_seconds: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    pub source: String,
}

/// A file's skip segments in the exact shape the player consumes.
///
/// Shared by session creation and by the `skip_segments_ready` library event,
/// so a player handed segments mid-playback gets the same rows, in the same
/// shape, it would have received at session start. A second copy of this
/// mapping would drift from the first without anything failing.
pub async fn load_skip_segments(
    pool: &sqlx::PgPool,
    media_type: MediaType,
    file_id: Uuid,
) -> Vec<SkipSegmentResponse> {
    match media_type {
        MediaType::Episode => episode_skip_segments::list_by_episode_file(pool, file_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|s| SkipSegmentResponse {
                segment_type: s.segment_type,
                start_seconds: s.start_seconds,
                end_seconds: s.end_seconds,
                confidence: s.confidence,
                source: s.source,
            })
            .collect(),
        MediaType::Movie => movie_skip_segments::list_by_movie_file(pool, file_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|s| SkipSegmentResponse {
                segment_type: s.segment_type,
                start_seconds: s.start_seconds,
                end_seconds: s.end_seconds,
                confidence: s.confidence,
                source: s.source,
            })
            .collect(),
        MediaType::Recording => vec![],
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SubtitleTrackResponse {
    pub track_id: String,
    pub stream_index: i32,
    pub codec: String,
    pub language: Option<String>,
    pub language_name: Option<&'static str>,
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub format: String,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_hearing_impaired: bool,
    pub requires_transcode: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioTrackResponse {
    pub stream_index: i32,
    pub codec: String,
    pub channels: i32,
    pub language: Option<String>,
    pub language_name: Option<&'static str>,
    pub title: Option<String>,
    pub is_default: bool,
}

#[derive(Debug, Deserialize)]
pub struct SeekRequest {
    pub time_seconds: f64,
}

#[derive(Debug, Deserialize)]
pub struct DecodeErrorRequest {
    /// Playback time at which the player observed `MEDIA_ERR_DECODE`. The
    /// server snaps this to a keyframe boundary and restarts FFmpeg in
    /// transcode mode from there so the browser HW decoder doesn't trip
    /// on the same corrupt bitstream again.
    pub at_seconds: f64,
}

#[derive(Debug, Deserialize)]
pub struct SwitchAudioRequest {
    /// Audio stream index to switch to. If null/absent, keeps the current track
    /// (useful for subtitle burn-in changes that don't need an audio switch).
    #[serde(default)]
    pub audio_stream_index: Option<i32>,
    /// Current playback position to resume from after switch.
    #[serde(default)]
    pub resume_time: Option<f64>,
    /// Optional: also switch burn-in subtitle. Set to -1 to disable burn-in.
    /// When provided, the subtitle handling is updated before restarting FFmpeg.
    #[serde(default)]
    pub burn_in_subtitle_index: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct SubtitleQuery {
    /// Seconds into the title the player's timeline begins at.
    #[serde(default)]
    pub offset: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct StreamPlaylistQuery {
    /// Seek generation cache buster.
    #[serde(default)]
    pub g: Option<u32>,
    /// ABR variant ID (e.g., "720p", "480p"). Omit for primary/high variant.
    #[serde(default)]
    pub v: Option<String>,
}

/// The resume position a session can actually be started at.
///
/// The position is keyed on the episode or movie and the user rather than on the file,
/// so it survives a content replace and can land past the end of a shorter replacement.
/// Starting there dumps the viewer at EOF and instantly marks the title watched, so it
/// is dropped and the title opens at the top instead.
fn accepted_resume(requested: Option<f64>, duration: f64) -> Option<f64> {
    requested.filter(|&st| st > 0.0 && (duration <= 0.0 || st < duration))
}

/// The rebase to report back, which is a transcoded playlist's alone.
///
/// A transcode is produced from the resume point and serves it as its own zero, so the
/// client has only the residual to seek and has to know where that zero sits. Direct
/// play hands over the whole file and the playhead is the client's — reporting a start
/// there tells it that zero is 23 minutes in when it is not, which reads the clock right
/// while the picture sits at the top, skips the seek that would have moved it, and leaves
/// every later seek short by the same amount.
fn reported_rebase(method: PlaybackMethod, start: Option<f64>) -> Option<f64> {
    start.filter(|_| method != PlaybackMethod::DirectPlay)
}

#[cfg(test)]
mod resume_rebase {
    use super::*;

    #[test]
    fn a_direct_played_file_is_never_rebased() {
        assert_eq!(
            reported_rebase(PlaybackMethod::DirectPlay, Some(1380.0)),
            None,
            "the whole file is served from zero, so the client owns the seek"
        );
    }

    #[test]
    fn a_transcode_reports_where_its_playlist_begins() {
        for method in [
            PlaybackMethod::Remux,
            PlaybackMethod::AudioTranscode,
            PlaybackMethod::VideoTranscode,
        ] {
            assert_eq!(reported_rebase(method, Some(1380.0)), Some(1380.0));
        }
    }

    #[test]
    fn a_resume_inside_the_file_is_kept() {
        assert_eq!(accepted_resume(Some(1380.0), 3600.0), Some(1380.0));
        // An unmeasured container is no reason to refuse one.
        assert_eq!(accepted_resume(Some(1380.0), 0.0), Some(1380.0));
    }

    #[test]
    fn a_resume_past_a_replaced_shorter_file_is_dropped() {
        assert_eq!(accepted_resume(Some(1380.0), 900.0), None);
        assert_eq!(accepted_resume(Some(0.0), 3600.0), None);
        assert_eq!(accepted_resume(None, 3600.0), None);
    }
}

// =============================================================================
// Cross-device handoff
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct HandoffQuery {
    pub media_type: String,
    pub media_id: Uuid,
}

/// GET /api/v1/admin/playback/handoff?media_type=episode&media_id=…
///
/// The newest position any of the caller's devices saved for this show (or movie), or
/// null. The live path is the `watch_progress` event; this is for a player whose event
/// stream was down when it went out — asked when it comes back, not when play is pressed.
pub async fn latest_handoff(
    State(state): State<AppState>,
    auth: AuthUser,
    axum::extract::Query(q): axum::extract::Query<HandoffQuery>,
) -> impl IntoResponse {
    Json(crate::watch_handoff::latest_for(&state, auth.user_id, &q.media_type, q.media_id).await)
}

// =============================================================================
// Create Session
// =============================================================================

pub async fn create_session(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    if auth.is_kids && !kids_may_play(&state.pool, &req).await {
        return (StatusCode::NOT_FOUND, "not available on a Kids profile").into_response();
    }

    let file_id = match resolve_session_file_id(&state, &req).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let session_id = Uuid::new_v4();
    match open_file_session(&state, auth.user_id, &req, file_id, session_id, None).await {
        Ok(response) => {
            super::session_restore::remember(&state, session_id, auth.user_id, file_id, &req).await;
            Json(response).into_response()
        }
        Err(resp) => resp,
    }
}

/// Build a file session under [session_id] and put it in the session map.
///
/// [restore] carries how far the session had got before the API restarted under it;
/// the session is then rebuilt at that point, its playlist and segments as they were,
/// rather than opened at [CreateSessionRequest::start_time].
pub(crate) async fn open_file_session(
    state: &AppState,
    user_id: Uuid,
    req: &CreateSessionRequest,
    file_id: Uuid,
    session_id: Uuid,
    restore: Option<Progress>,
) -> Result<SessionResponse, Response<Body>> {
    let config = state.runtime_config.get().await;

    let (
        full_path,
        StoredFileMeta {
            container,
            video_codec,
            duration,
            audio_metas,
            sub_metas,
            keyframes,
            video_width,
            video_height,
            interlaced,
            dynamic_range,
            ..
        },
    ) = load_playable_file(
        state,
        req.media_type,
        file_id,
        req.capabilities.supports_interlaced == Some(false),
    )
    .await?;

    // Resolve preferred audio/subtitle from language settings when the client
    // didn't explicitly request a track. Per-profile prefs (sent by the client)
    // override the server-wide config default.
    let audio_lang_pref = decision::audio_language_pref(
        req.preferred_audio_language.as_deref(),
        &config.preferred_audio_language,
    );
    let preferred_audio = if req.audio_track_index.is_some() {
        req.audio_track_index
    } else {
        decision::preferred_audio(
            &audio_metas,
            req.preferred_audio_title.as_deref(),
            audio_lang_pref,
        )
    };
    let subtitles_enabled = req.subtitles_enabled.unwrap_or(true);
    let subtitle_lang_pref = req
        .preferred_subtitle_language
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&config.preferred_subtitle_language);
    let preferred_subtitle = req.subtitle_track_index.filter(|_| subtitles_enabled);

    // Run decision engine
    let file_meta = FileMeta {
        container: container.clone(),
        video_codec: video_codec.clone(),
        video_width,
        video_height,
        interlaced: interlaced.unwrap_or(false),
        dynamic_range,
    };
    let preferred_codecs = state.encoder_caps.preferred_codecs();
    let mut decision = decision::decide(
        &file_meta,
        &audio_metas,
        &sub_metas,
        &req.capabilities,
        ViewerPrefs {
            preferred_audio_index: preferred_audio,
            preferred_subtitle_index: preferred_subtitle,
            understood_language: audio_lang_pref,
            subtitle_language: if subtitles_enabled {
                subtitle_lang_pref
            } else {
                ""
            },
        },
        &preferred_codecs,
    );

    // Force transcode override: when the client requests HLS with ABR
    // (for adaptive bitrate switching) or sets a bitrate cap, override
    // the decision to VideoTranscode so we get the HLS pipeline + variants.
    let force = req.force_transcode || req.max_bitrate_kbps.is_some();
    if force && decision.method != PlaybackMethod::VideoTranscode {
        let target_codec = decision::best_transcode_codec(&preferred_codecs, &req.capabilities);
        decision.method = PlaybackMethod::VideoTranscode;
        decision.video_mode = decision::VideoMode::Transcode { target_codec };
        // If audio wasn't already being transcoded but needs it, the original
        // decision already handled that. We only change the video path.
    }

    let cache_dir = config
        .transcode_cache_dir
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(&state.config.storage_root)
                .join("cache")
                .join("transcode")
        });
    let segment_dir = cache_dir.join(session_id.to_string());

    // Build subtitle metadata — uses pre-extracted cache when available,
    // falls back to lazy extraction on first request
    let subtitle_files = build_subtitle_metadata(
        &sub_metas,
        &segment_dir,
        &state.config.storage_root,
        file_id,
    );

    // Initialize segment cache
    let mut segment_cache = SegmentCache::new(segment_dir.clone(), 6, duration);
    // The one answer for all three places a resume has to reach: where the playlist
    // starts, where FFmpeg starts, and what the client is told its zero is.
    let resume_start = accepted_resume(req.start_time, duration);
    if let Some(ref progress) = restore {
        segment_cache.set_playback_start(
            progress.playback_start_segment,
            progress.playback_start_secs,
        );
    } else if let Some(st) = resume_start {
        segment_cache.set_playback_start(segment_cache.time_to_segment(st), st);
    }

    // Extract burn-in subtitle index from decision
    let burn_in_subtitle = match &decision.subtitle_handling {
        decision::SubtitleHandling::BurnIn { stream_index } => Some(*stream_index),
        _ => None,
    };

    // Force video transcode if keyframes are abnormally dense (e.g. DCP rips
    // where every frame is a keyframe). Copy-mode HLS splits at keyframes, so
    // all-intra content produces per-frame micro-segments that break playback.
    // Threshold: avg interval < 0.5s catches all-intra (~0.04s) but not
    // sports/action with normal 2-4s GOPs.
    if matches!(decision.video_mode, decision::VideoMode::Copy)
        && let Some(ref kfs) = keyframes
        && kfs.len() >= 2
    {
        let avg_interval = kfs.last().copied().unwrap_or(0.0) / kfs.len() as f64;
        if avg_interval < 0.5 {
            info!(
                avg_keyframe_interval = avg_interval,
                keyframe_count = kfs.len(),
                "Dense keyframes detected — forcing video transcode for proper HLS segmentation"
            );
            let target_codec = decision::best_transcode_codec(&preferred_codecs, &req.capabilities);
            decision.video_mode = decision::VideoMode::Transcode { target_codec };
            decision.method = PlaybackMethod::VideoTranscode;
        }
    }
    if let Some(ref progress) = restore {
        decision = progress.decision.clone();
    }

    // Build transcode args if not direct play — FFmpeg startup is deferred to a
    // background task so the session response returns immediately (~35ms vs ~2s
    // over SMB cold cache).
    let deferred_transcode = if decision.method != PlaybackMethod::DirectPlay && restore.is_none() {
        let (video_mode, target_encoder) = match &decision.video_mode {
            decision::VideoMode::Copy => (VideoMode::Copy, state.encoder_caps.h264.clone()),
            decision::VideoMode::Transcode { target_codec } => {
                let encoder = state
                    .encoder_caps
                    .encoder_for(target_codec)
                    .unwrap_or(&state.encoder_caps.h264)
                    .to_string();
                (VideoMode::Transcode, encoder)
            }
        };
        let audio_mode = match &decision.audio_mode {
            decision::AudioMode::Copy => AudioMode::Copy,
            decision::AudioMode::Transcode {
                target_codec,
                bitrate_kbps,
            } => AudioMode::Transcode {
                target_codec: target_codec.clone(),
                bitrate_kbps: *bitrate_kbps,
            },
        };

        let args = TranscodeArgs {
            input_path: full_path.clone(),
            output_dir: segment_dir.clone(),
            video_mode,
            audio_mode,
            audio_stream_index: decision.audio_stream_index,
            start_time: resume_start,
            segment_duration: 6,
            hw_accel: (*state.hw_accel).clone(),
            video_codec: video_codec.clone(),
            target_encoder,
            burn_in_subtitle,
            scale: primary_scale(
                (file_meta.video_width, file_meta.video_height),
                req.capabilities.max_height,
            ),
            target_bitrate_kbps: req.max_bitrate_kbps,
            duration_seconds: duration,
            initial_generation: 0,
            hw_device: state.hw_device.get(),
            prior_runs: Vec::new(),
            deinterlace: decision.deinterlace,
            tone_map_hdr: decision.tone_map_hdr,
        };

        // Ensure segment dir exists for subtitle extraction before response
        let _ = tokio::fs::create_dir_all(&segment_dir).await;

        Some((args, keyframes.clone()))
    } else {
        // Ensure segment dir exists even for direct play (for extracted subtitles)
        if !subtitle_files.is_empty() || restore.is_some() {
            let _ = tokio::fs::create_dir_all(&segment_dir).await;
        }
        None
    };
    let transcode_process = None;

    let base_url = format!("/api/v1/admin/playback/sessions/{session_id}");

    // Which track the decision engine actually activated. The client marks the
    // `is_default` track active, so this must be the decision's choice — the
    // file's own default disposition is exactly what auto-selection now declines
    // to follow for understood audio.
    let selected_subtitle_index = decision.subtitle_handling.stream_index();

    // Build subtitle responses: text subs (extractable) + bitmap subs (burn-in)
    let mut subtitle_responses: Vec<SubtitleTrackResponse> = subtitle_files
        .iter()
        .map(|sf| {
            let (format, ext) = match sf.codec.as_str() {
                "ass" | "ssa" => ("ass", "ass"),
                _ => ("vtt", "vtt"),
            };
            SubtitleTrackResponse {
                track_id: sf.track_id.to_string(),
                stream_index: sf.stream_index,
                codec: sf.codec.clone(),
                language_name: crate::streaming::language::display_name(sf.language.as_deref()),
                language: sf.language.clone(),
                title: sf.title.clone(),
                url: Some(format!("{base_url}/subtitles/{}.{ext}", sf.track_id)),
                format: format.to_string(),
                is_default: Some(sf.stream_index) == selected_subtitle_index,
                is_forced: sf.is_forced,
                is_hearing_impaired: sf.is_hearing_impaired,
                requires_transcode: false,
            }
        })
        .collect();

    // Add bitmap subtitle tracks (PGS/DVDSub) — burned into video, no URL
    for sub in sub_metas
        .iter()
        .filter(|s| decision::is_bitmap_codec(&s.codec))
    {
        subtitle_responses.push(SubtitleTrackResponse {
            track_id: format!("burn_{}", sub.stream_index),
            stream_index: sub.stream_index,
            codec: sub.codec.clone(),
            language_name: crate::streaming::language::display_name(sub.language.as_deref()),
            language: sub.language.clone(),
            title: sub.title.clone(),
            url: None,
            format: "burn_in".to_string(),
            is_default: Some(sub.stream_index) == selected_subtitle_index,
            is_forced: sub.is_forced,
            is_hearing_impaired: sub.is_hearing_impaired,
            requires_transcode: true,
        });
    }

    // Build audio track responses
    let audio_responses: Vec<AudioTrackResponse> = audio_metas
        .iter()
        .map(|a| AudioTrackResponse {
            stream_index: a.stream_index,
            codec: a.codec.clone(),
            channels: a.channels,
            language_name: crate::streaming::language::display_name(a.language.as_deref()),
            language: a.language.clone(),
            title: a.title.clone(),
            is_default: a.is_default,
        })
        .collect();

    // Load video chapters
    let chapters = match req.media_type {
        MediaType::Movie => video_chapters::list_by_movie_file(&state.pool, file_id)
            .await
            .unwrap_or_default(),
        MediaType::Episode => video_chapters::list_by_episode_file(&state.pool, file_id)
            .await
            .unwrap_or_default(),
        MediaType::Recording => vec![],
    };

    // Load corrupt_ranges so the lazy overlay scheduler (per-segment transcode
    // for known-bad windows) can work without a per-frame DB roundtrip. The
    // session struct caches these and the playlist-request hook spawns
    // overlays as the playhead approaches each range. Recordings aren't
    // tracked in tv_episode_files / movie_files so they get an empty list.
    let corrupt_ranges: Vec<crate::corruption::CorruptRange> = match req.media_type {
        MediaType::Episode => {
            crate::repositories::tv_episode_files::get_corrupt_ranges(&state.pool, file_id)
                .await
                .unwrap_or_default()
                .unwrap_or_default()
        }
        MediaType::Movie => {
            crate::repositories::movie_files::get_corrupt_ranges(&state.pool, file_id)
                .await
                .unwrap_or_default()
                .unwrap_or_default()
        }
        MediaType::Recording => Vec::new(),
    };
    if !corrupt_ranges.is_empty() {
        info!(
            session_id = %session_id,
            count = corrupt_ranges.len(),
            "session has corrupt ranges — overlay scheduler will pre-emptively transcode them"
        );
    }
    let chapter_responses: Vec<ChapterResponse> = chapters
        .into_iter()
        .map(|ch| ChapterResponse {
            index: ch.chapter_index,
            title: ch.title,
            start_seconds: ch.start_seconds,
            end_seconds: ch.end_seconds,
        })
        .collect();

    // Load skip segments for episodes and movies
    let skip_segment_responses = load_skip_segments(&state.pool, req.media_type, file_id).await;

    // Derive thumbnail availability from the media_analyze manifest, falling
    // back to a filesystem VTT check so legacy thumbnails (pre-manifest era)
    // keep serving until media_analyze runs on the file. Lufs stays in the DB
    // for movies/episodes (written by media_analyze's loudness branch
    // post-processing) — recordings don't have LUFS.
    let thumbnails_generated = match req.media_type {
        MediaType::Movie | MediaType::Episode => {
            let from_manifest =
                crate::media_cache::readers::read_manifest(&state.config.storage_root, file_id)
                    .and_then(|m| m.artefacts.trickplay)
                    .map(|t| t.present)
                    .unwrap_or(false);
            from_manifest
                || crate::media_cache::trickplay_dir(&state.config.storage_root, file_id)
                    .join("thumbnails.vtt")
                    .exists()
        }
        MediaType::Recording => {
            let file_path_opt = sqlx::query_scalar!(
                r#"SELECT file_path AS "file_path!" FROM iptv_recordings WHERE id = $1"#,
                file_id,
            )
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten();
            file_path_opt
                .map(|p| {
                    let parent = std::path::Path::new(&p)
                        .parent()
                        .map(std::path::Path::to_path_buf)
                        .unwrap_or_else(|| std::path::PathBuf::from(&p));
                    // Prefer the new media_cache/trickplay/ layout, fall back
                    // to the legacy `<parent>/thumbnails/` path so pre-deploy
                    // recordings keep their scrubber previews.
                    let v2_manifest_present =
                        crate::media_cache::readers::read_manifest_at(&parent.join("media_cache"))
                            .and_then(|m| m.artefacts.trickplay)
                            .map(|t| t.present)
                            .unwrap_or(false);
                    v2_manifest_present
                        || parent.join("media_cache/trickplay/thumbnails.vtt").exists()
                        || parent.join("thumbnails/thumbnails.vtt").exists()
                })
                .unwrap_or(false)
        }
    };
    let lufs_value: Option<f64> = match req.media_type {
        MediaType::Movie => {
            sqlx::query_scalar!(r#"SELECT lufs FROM movie_files WHERE id = $1"#, file_id,)
                .fetch_optional(&state.pool)
                .await
                .ok()
                .flatten()
                .flatten()
        }
        MediaType::Episode => sqlx::query_scalar!(
            r#"SELECT lufs FROM tv_episode_files WHERE id = $1"#,
            file_id,
        )
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten()
        .flatten(),
        MediaType::Recording => None,
    };

    let picture = crate::repositories::file_pictures::get(&state.pool, req.media_type, file_id)
        .await
        .ok()
        .flatten();

    let response = SessionResponse {
        session_id: session_id.to_string(),
        method: decision.method,
        playlist_url: if decision.method != PlaybackMethod::DirectPlay {
            Some(format!("{base_url}/master.m3u8"))
        } else {
            None
        },
        direct_url: if decision.method == PlaybackMethod::DirectPlay {
            Some(format!("{base_url}/direct"))
        } else {
            None
        },
        duration_seconds: duration,
        subtitles: subtitle_responses,
        audio_tracks: audio_responses,
        thumbnails_url: if thumbnails_generated {
            Some(format!(
                "/api/v1/admin/playback/sessions/{session_id}/thumbnails.vtt"
            ))
        } else {
            None
        },
        lufs: lufs_value,
        chapters: chapter_responses,
        skip_segments: skip_segment_responses,
        start_time: reported_rebase(decision.method, resume_start),
        media_id: None,
        media_type: None,
        file_id: Some(file_id.to_string()),
        selected_audio_index: preferred_audio,
        preferred_audio_stream_index: None,
        selected_subtitle_index,
        video_width: if video_width > 0 {
            Some(video_width)
        } else {
            None
        },
        video_height: if video_height > 0 {
            Some(video_height)
        } else {
            None
        },
        picture,
    };

    // Compute ABR variants for VideoTranscode sessions with enough resolution
    let variants = if decision.method == PlaybackMethod::VideoTranscode {
        let (out_w, out_h) = variant::output_resolution(
            file_meta.video_width,
            file_meta.video_height,
            req.capabilities.max_height,
        );
        let mut variant_specs = variant::compute_variants(out_w, out_h);
        if let Some(max_br) = req.max_bitrate_kbps {
            variant_specs = variant::filter_by_max_bitrate(variant_specs, max_br);
        }
        if variant_specs.is_empty() {
            None
        } else {
            let mut map = std::collections::HashMap::new();
            for spec in variant_specs {
                let variant_dir = segment_dir.join(&spec.id);
                let mut variant_cache = SegmentCache::new(variant_dir, 6, duration);
                // Match the primary's start position so variants start from
                // the same resume position (not from 0).
                variant_cache.follow_playback_start(&segment_cache);
                let prior_runs = restore
                    .as_ref()
                    .map(|p| p.variant_prior_runs(&spec.id))
                    .unwrap_or_default();
                map.insert(
                    spec.id.clone(),
                    QualityVariant {
                        spec,
                        transcode: None,
                        segment_cache: variant_cache,
                        prior_runs,
                    },
                );
            }
            Some(map)
        }
    } else {
        None
    };

    let mut session = PlaybackSession {
        id: session_id,
        user_id,
        media_type: req.media_type,
        file_id,
        file_path: full_path,
        decision,
        transcode: transcode_process,
        segment_cache,
        created_at: Instant::now(),
        last_activity: Instant::now(),
        seek_generation: restore.as_ref().map_or(0, Progress::next_seek_generation),
        duration_seconds: duration,
        container,
        video_codec,
        subtitle_files,
        hw_accel: (*state.hw_accel).clone(),
        source_resolution: (file_meta.video_width, file_meta.video_height),
        is_live: false,
        provider_id: None,
        variants,
        max_bitrate_kbps: req.max_bitrate_kbps,
        max_height: req.capabilities.max_height,
        keyframes,
        error: None,
        pending_segment: None,
        download_cancel: None,
        thumbnail_dir: None,
        debrid_cache_key: None,
        audio_metas: audio_metas.clone(),
        preferred_audio_index: None,
        force_video_transcode: restore.as_ref().is_some_and(|p| p.force_video_transcode),
        corrupt_ranges,
        chapters: Vec::new(),
        overlay_spawned_starts: std::collections::HashSet::new(),
        highest_served_segment: restore.as_ref().and_then(|p| p.highest_served_segment),
        feed: None,
        download: None,
        cue_gate: None,
        persisted: true,
    };

    if let Some(ref progress) = restore
        && session.decision.method != PlaybackMethod::DirectPlay
    {
        let process = start_ffmpeg_for_session(
            &session,
            segment_dir.clone(),
            Some(progress.resume_secs(&session.segment_cache)),
            state,
            None,
            progress.next_generation(),
            progress.prior_runs(),
        )
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to restart transcode: {e}"),
            )
                .into_response()
        })?;
        if let Some(ref kf) = session.keyframes {
            process.set_keyframes(kf.clone());
        }
        session.transcode = Some(process);
    }

    // Claimed before the session is visible, so the player's first seek or restart
    // queues behind the background start instead of racing it.
    let start_gate = if deferred_transcode.is_some() {
        Some(state.session_gates.claim(session_id).await)
    } else {
        None
    };
    state.playback_sessions.insert(session_id, session);

    info!(
        session_id = %session_id,
        method = ?response.method,
        "Playback session created"
    );

    // Start FFmpeg in background — the playlist handler serves estimated segments
    // until the transcode process is ready, then switches to real segments.
    if let Some((args, deferred_keyframes)) = deferred_transcode {
        let bg_state = state.clone();
        tokio::spawn(async move {
            let _start_gate = start_gate;
            match transcode::TranscodeProcess::start(args).await {
                Ok(process) => {
                    if let Some(kf) = deferred_keyframes {
                        process.set_keyframes(kf);
                    }
                    if let Some(mut session) = bg_state.playback_sessions.get_mut(&session_id) {
                        session.transcode = Some(process);
                    }
                    info!(session_id = %session_id, "Deferred transcode started");
                }
                Err(e) => {
                    error!(session_id = %session_id, error = %e, "Deferred transcode failed");
                    if let Some(mut session) = bg_state.playback_sessions.get_mut(&session_id) {
                        session.error = Some(SessionError::new(
                            "transcode_failed",
                            format!("Failed to start transcode: {e}"),
                        ));
                    }
                }
            }
        });
    }

    Ok(response)
}

// =============================================================================
// Destroy Session
// =============================================================================

pub async fn destroy_session(
    State(state): State<AppState>,
    _auth: PlaybackAuth,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let session_id = match Uuid::parse_str(&session_id) {
        Ok(id) => id,
        Err(_) => return StatusCode::BAD_REQUEST,
    };
    if end_session(&state, session_id).await {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

/// Tear down an in-memory playback session: its transcodes, its debrid download and
/// its dedup entry. A fully downloaded cache file stays on disk for the cache layer.
pub(crate) async fn end_session(state: &AppState, session_id: Uuid) -> bool {
    let Some((_, mut session)) = state.playback_sessions.remove(&session_id) else {
        // Lost to a restart and never asked for since: only its row and segments remain.
        let Some(dir) = super::session_restore::forget(state, session_id).await else {
            return false;
        };
        enqueue_session_cleanup(state, std::path::Path::new(&dir)).await;
        return true;
    };
    if session.persisted {
        super::session_restore::forget(state, session_id).await;
    }
    if let Some(provider_id) = session.provider_id {
        crate::iptv::provider_tracker::release(provider_id);
        state.live_streams.remove(&session.file_id);
        info!(session_id = %session_id, provider_id = %provider_id, "released IPTV provider connection");
    }
    if let Some(cancel) = &session.download_cancel {
        cancel.cancel();
    }
    if let Some(ref key) = session.debrid_cache_key {
        state.debrid_session_index.remove(key);
    }
    session.kill_all_transcodes().await;
    enqueue_session_cleanup(state, session.segment_dir()).await;
    info!(session_id = %session_id, "Playback session destroyed");
    true
}

// =============================================================================
// Heartbeat
// =============================================================================

/// What a session looks like from the outside, gathered under the map lock.
#[derive(Debug, Clone)]
pub struct SessionFacts {
    pub is_live: bool,
    /// A debrid session holds no file path until its probe lands. That is a wait, not
    /// a fault, and it is the one state where an unreadable source means nothing.
    pub probing: bool,
    pub source_readable: bool,
    /// None for direct play, which has no process behind it.
    pub transcode_running: Option<bool>,
    pub segments_ready: Option<u32>,
    pub session_error: Option<(&'static str, String)>,
    pub source_label: String,
}

/// What the facts add up to.
pub struct SessionVerdict {
    pub state: &'static str,
    pub error: Option<&'static str>,
    pub message: Option<String>,
}

/// Decide whether a session can still deliver bytes.
///
/// The one rule that is worth being careful about is a transcode that has exited: for a
/// finite file that is what finishing looks like, and the segments on disk are the proof
/// of it. Only an exit with nothing to show for it — or any exit at all on a live stream,
/// which should never end — is a fault.
pub fn session_verdict(facts: &SessionFacts) -> SessionVerdict {
    let transcode_gone = matches!(facts.transcode_running, Some(false))
        && (facts.is_live || facts.segments_ready.unwrap_or(0) == 0);
    let source_readable = facts.probing || facts.source_readable;

    let (error, message) = match &facts.session_error {
        // The server's own diagnosis knows why; everything below only knows that.
        Some((code, msg)) => (Some(*code), Some(msg.clone())),
        None if !source_readable => (
            Some("source_unreadable"),
            Some(format!(
                "The media file is no longer readable: {}",
                facts.source_label
            )),
        ),
        None if transcode_gone => (
            Some("transcode_exited"),
            Some("The transcode process exited before producing anything".to_string()),
        ),
        None => (None, None),
    };

    let state = if error.is_some() {
        "failed"
    } else if facts.probing || facts.segments_ready == Some(0) {
        "preparing"
    } else {
        "ready"
    };

    SessionVerdict {
        state,
        error,
        message,
    }
}

#[cfg(test)]
mod session_verdict_tests {
    use super::*;

    fn facts() -> SessionFacts {
        SessionFacts {
            is_live: false,
            probing: false,
            source_readable: true,
            transcode_running: None,
            segments_ready: None,
            session_error: None,
            source_label: "/media/film.mkv".to_string(),
        }
    }

    #[test]
    fn a_direct_played_file_has_nothing_to_wait_for() {
        assert_eq!(session_verdict(&facts()).state, "ready");
    }

    #[test]
    fn a_transcode_with_no_segments_yet_is_still_coming() {
        let v = session_verdict(&SessionFacts {
            transcode_running: Some(true),
            segments_ready: Some(0),
            ..facts()
        });
        assert_eq!(v.state, "preparing");
        assert!(v.error.is_none());
    }

    #[test]
    fn a_transcode_that_exited_with_nothing_is_a_fault() {
        let v = session_verdict(&SessionFacts {
            transcode_running: Some(false),
            segments_ready: Some(0),
            ..facts()
        });
        assert_eq!(v.state, "failed");
        assert_eq!(v.error, Some("transcode_exited"));
    }

    /// A finite file's transcode reaching the end of it is not a failure.
    #[test]
    fn a_finished_vod_transcode_is_not_a_fault() {
        let v = session_verdict(&SessionFacts {
            transcode_running: Some(false),
            segments_ready: Some(240),
            ..facts()
        });
        assert_eq!(v.state, "ready");
    }

    /// A live transcode has no end to reach, so it exiting means the source is gone.
    #[test]
    fn a_live_transcode_that_exits_has_lost_its_source() {
        let v = session_verdict(&SessionFacts {
            is_live: true,
            transcode_running: Some(false),
            segments_ready: Some(240),
            ..facts()
        });
        assert_eq!(v.state, "failed");
        assert_eq!(v.error, Some("transcode_exited"));
    }

    #[test]
    fn an_unreadable_source_names_itself() {
        let v = session_verdict(&SessionFacts {
            source_readable: false,
            ..facts()
        });
        assert_eq!(v.error, Some("source_unreadable"));
        assert!(v.message.unwrap().contains("/media/film.mkv"));
    }

    /// A debrid session has no path to read until its probe lands.
    #[test]
    fn a_session_still_probing_is_not_unreadable() {
        let v = session_verdict(&SessionFacts {
            probing: true,
            source_readable: false,
            ..facts()
        });
        assert_eq!(v.state, "preparing");
        assert!(v.error.is_none());
    }

    #[test]
    fn the_servers_own_diagnosis_wins() {
        let v = session_verdict(&SessionFacts {
            source_readable: false,
            transcode_running: Some(false),
            segments_ready: Some(0),
            session_error: Some(("startup_stalled", "no segments in 30s".to_string())),
            ..facts()
        });
        assert_eq!(v.error, Some("startup_stalled"));
        assert_eq!(v.message.unwrap(), "no segments in 30s");
    }
}

/// Everything the server knows about whether a session can still deliver bytes.
#[derive(Debug, Serialize)]
pub struct SessionStatusResponse {
    pub session_id: String,
    /// `ready` | `preparing` | `failed`. Derived here so the client does not have to
    /// re-implement the rule for what the other fields add up to.
    pub state: &'static str,
    pub method: PlaybackMethod,
    pub is_live: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub source_readable: bool,
    /// None for direct play, which has no process behind it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcode_running: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segments_ready: Option<u32>,
    /// Debrid sessions only: the direct-play byte feed and the download behind it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feed: Option<FeedStatus>,
    /// Debrid sessions only, once probed: see [`SessionResponse`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_audio_stream_index: Option<i32>,
}

/// Report whether this session can actually deliver bytes, in one cheap request.
///
/// A player that has been handed a URL cannot tell a slow start from a dead one — a
/// reaped session, a transcode that crashed on its first frame and a source that has
/// been unmounted all look identical to it, which is to say they all look like a
/// spinner. Everything here is already known the moment it goes wrong, so a client
/// that polls this while it waits never has to discover a failure by timing out.
///
/// Deliberately separate from [`heartbeat`], whose bare status code is the whole of
/// its contract and which existing clients read as a liveness bit.
pub async fn status(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Response<Body> {
    let session_id = match Uuid::parse_str(&session_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "bad session id").into_response(),
    };

    let (
        method,
        is_live,
        session_error,
        probing,
        file_path,
        transcode_running,
        segments_ready,
        feed,
        preferred_audio_stream_index,
    ) = {
        let Some(mut session) = state.playback_sessions.get_mut(&session_id) else {
            return (StatusCode::NOT_FOUND, "session not found").into_response();
        };
        session.touch();
        let start_segment = session.segment_cache.playback_start_segment;
        let running = session.transcode.as_mut().map(|t| t.is_running());
        let ready = session
            .transcode
            .as_ref()
            .map(|t| t.playlist_segments_from(start_segment).len() as u32);
        (
            session.decision.method,
            session.is_live,
            session.error.as_ref().map(|e| (e.code, e.message.clone())),
            // A debrid session holds no path until its probe lands. That is a wait,
            // not a fault.
            session.file_path.as_os_str().is_empty(),
            session.file_path.clone(),
            running,
            ready,
            session
                .feed
                .as_ref()
                .map(|f| f.status(session.download.as_deref())),
            session.preferred_audio_index,
        )
    };

    let facts = SessionFacts {
        is_live,
        probing,
        // A live session's file_path is its upstream url, which the filesystem never has.
        source_readable: is_live || tokio::fs::metadata(&file_path).await.is_ok(),
        transcode_running,
        segments_ready,
        session_error,
        source_label: file_path.display().to_string(),
    };
    let verdict = session_verdict(&facts);

    Json(SessionStatusResponse {
        session_id: session_id.to_string(),
        state: verdict.state,
        method,
        is_live,
        error: verdict.error,
        message: verdict.message,
        source_readable: facts.source_readable,
        transcode_running,
        segments_ready,
        feed,
        preferred_audio_stream_index,
    })
    .into_response()
}

pub async fn heartbeat(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let session_id = match Uuid::parse_str(&session_id) {
        Ok(id) => id,
        Err(_) => return StatusCode::BAD_REQUEST,
    };

    if let Some(mut session) = state.playback_sessions.get_mut(&session_id) {
        session.touch();
        // Keep debrid cache file alive during playback
        if session.download_cancel.is_some() {
            let pool = state.pool.clone();
            tokio::spawn(async move {
                let _ =
                    crate::repositories::debrid_cache_files::touch_for_session(&pool, session_id)
                        .await;
            });
        }
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

// =============================================================================
// HLS Playlists
// =============================================================================

/// A session lifted out of the map is mid-restart: wait for it to come back rather
/// than answer for a session that is about to exist again.
async fn wait_out_restart(state: &AppState, session_id: Uuid) {
    if !state.playback_sessions.contains_key(&session_id) {
        state.session_gates.wait(session_id).await;
    }
}

fn session_missing(state: &AppState, session_id: Uuid) -> axum::response::Response {
    let status = if state.session_gates.is_held(session_id) {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::NOT_FOUND
    };
    (status, "session not found").into_response()
}

pub async fn master_playlist(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let session_id = match Uuid::parse_str(&session_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid session_id").into_response(),
    };
    wait_out_restart(&state, session_id).await;

    let mut session = match state.playback_sessions.get_mut(&session_id) {
        Some(s) => s,
        None => {
            return session_missing(&state, session_id);
        }
    };
    session.touch();

    if session.file_path.as_os_str().is_empty() {
        return session_pending_response();
    }

    // Check FFmpeg health. On crash, persist the error onto the session so
    // the next playlist/segment request returns a terminal HTTP 500 with a
    // machine-readable `X-Stream-Error` code instead of looking healthy.
    if let Some(ref mut transcode) = session.transcode
        && let Err(msg) = transcode.check_health()
    {
        warn!(session_id = %session_id, error = %msg, "FFmpeg unhealthy during master playlist request");
        if session.error.is_none() {
            session.error = Some(SessionError::new("transcode_failed", msg));
        }
    }

    let base_url = format!("/api/v1/admin/playback/sessions/{session_id}");
    let seek_gen = session.seek_generation;
    let (src_w, src_h) = session.source_resolution;

    let mut playlist = String::from("#EXTM3U\n#EXT-X-VERSION:7\n\n");

    if session.is_abr() {
        // ABR: list primary (high quality) first, then lower variants.
        // hls.js picks based on bandwidth — order determines tie-breaking.
        let (out_w, out_h) = variant::output_resolution(src_w, src_h, session.max_height);
        let primary = variant::primary_variant_spec(out_w, out_h);
        // When max_bitrate_kbps is set, the primary is bitrate-capped —
        // advertise the capped bandwidth so hls.js makes correct decisions.
        let primary_bw = if let Some(max_br) = session.max_bitrate_kbps {
            variant::estimate_bandwidth(Some(max_br))
        } else {
            primary.bandwidth
        };
        playlist.push_str(&format!(
            "#EXT-X-STREAM-INF:BANDWIDTH={primary_bw},RESOLUTION={}x{},CODECS=\"avc1.640028,mp4a.40.2\"\n\
             {base_url}/stream.m3u8?g={seek_gen}\n",
            primary.width, primary.height,
        ));

        if let Some(ref variants) = session.variants {
            // Sort variants by bandwidth descending (highest quality first)
            let mut specs: Vec<&QualityVariant> = variants.values().collect();
            specs.sort_by_key(|b| std::cmp::Reverse(b.spec.bandwidth));
            for qv in specs {
                playlist.push_str(&format!(
                    "#EXT-X-STREAM-INF:BANDWIDTH={},RESOLUTION={}x{},CODECS=\"avc1.640028,mp4a.40.2\"\n\
                     {base_url}/stream.m3u8?g={seek_gen}&v={}\n",
                    qv.spec.bandwidth, qv.spec.width, qv.spec.height, qv.spec.id,
                ));
            }
        }
    } else {
        // Non-ABR: single stream with seek generation cache buster.
        playlist.push_str(&format!(
            "#EXT-X-STREAM-INF:BANDWIDTH=5000000\n{base_url}/stream.m3u8?g={seek_gen}\n"
        ));
    }

    // Subtitles are NOT included in the HLS master playlist. In HLS,
    // #EXT-X-MEDIA:TYPE=SUBTITLES URIs must point to media playlists,
    // not raw VTT/ASS files. hls.js tries to parse them as HLS and fails
    // with "Missing format identifier #EXTM3U". The frontend handles
    // subtitles independently via JASSUB (ASS) or native <track> (VTT)
    // using the session's subtitle list and dedicated /subtitles/ endpoints.

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(playlist))
        .unwrap()
        .into_response()
}

/// Server-generated HLS stream playlist.
///
/// Serves immediately without waiting for FFmpeg to produce segments:
/// - If cached segments exist: uses actual durations from FFmpeg's playlist
/// - If predicted segments are available (from keyframes): serves EVENT
///   playlist with keyframe-derived durations — hls.js polls for updates
///   and subsequent calls serve actual cached segments
/// - Otherwise: generates an EVENT playlist with estimated durations using
///   `segment_duration` — the `serve_segment` handler waits for files to appear
///
/// For ABR sessions, `?v=variant_id` selects a lower-quality variant. The
/// primary (high quality) variant omits the `?v=` param. On first request
/// for a variant, FFmpeg is lazy-started for that quality level.
pub async fn stream_playlist(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<StreamPlaylistQuery>,
) -> impl IntoResponse {
    let session_id = match Uuid::parse_str(&session_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid session_id").into_response(),
    };
    wait_out_restart(&state, session_id).await;

    let variant_id = query.v.clone();

    // Debrid sessions before probe completes have empty file_path
    if state
        .playback_sessions
        .get(&session_id)
        .is_some_and(|s| s.file_path.as_os_str().is_empty())
    {
        return session_pending_response();
    }

    // If this is a variant request, lazy-start FFmpeg for that variant
    if let Some(ref vid) = variant_id
        && let Err(resp) = lazy_start_variant(&state, session_id, vid).await
    {
        return resp;
    }

    // Check if the background probe/transcode failed. Return a terminal HTTP
    // 500 with a machine-readable code so the frontend's hls.js error handler
    // can show a real error UI — never ENDLIST, which the player would treat
    // as "stream finished successfully" and dispatch `ended`.
    if let Some(session) = state.playback_sessions.get(&session_id)
        && let Some(ref err) = session.error
    {
        return error_response(err.code, &err.message).into_response();
    }

    // Live IPTV sessions: serve ffmpeg's on-disk stream.m3u8 directly.
    // These sessions have no meaningful SegmentCache (duration is infinite),
    // and ffmpeg manages its own HLS playlist with fMP4 segments.
    {
        let mut session = match state.playback_sessions.get_mut(&session_id) {
            Some(s) => s,
            None => {
                return session_missing(&state, session_id);
            }
        };
        if session.is_live {
            session.touch();
            let ffmpeg_dead = session
                .transcode
                .as_mut()
                .map(|t| !t.is_running())
                .unwrap_or(true);
            let source_disconnected = session
                .transcode
                .as_ref()
                .and_then(|t| t.stream_health.as_ref())
                .map(|h| {
                    let health = h.lock().unwrap();
                    !health.is_source_connected
                })
                .unwrap_or(false);
            if ffmpeg_dead {
                tracing::warn!(session_id = %session_id, "live ffmpeg process has exited");
            }
            let playlist_path = session.segment_cache.base_dir.join("stream.m3u8");
            drop(session);

            // If both FFmpeg and the source are dead, return a terminal HTTP
            // 500 with `source_disconnected`. The frontend surfaces this as a
            // proper error UI rather than the player thinking the stream
            // ended successfully.
            if ffmpeg_dead && source_disconnected {
                return error_response("source_disconnected", "Live source disconnected")
                    .into_response();
            }

            // Wait for ffmpeg to produce the initial playlist (up to ~10s)
            for _ in 0..50 {
                if let Ok(content) = tokio::fs::read_to_string(&playlist_path).await
                    && !content.is_empty()
                {
                    // Rewrite segment URLs and sanitize for live playback:
                    // - ffmpeg writes "seg_00005.m4s" / "init.mp4" but our route expects "segments/..." prefix
                    // - Rewrite EXT-X-MAP URI for the fMP4 init segment
                    // - Strip #EXT-X-ENDLIST (tells player stream is over — never correct for live)
                    // - Strip final segment if duration < 1.0s (ffmpeg flush fragment, likely corrupt)
                    let mut lines: Vec<String> = Vec::new();
                    let mut has_endlist = false;
                    for line in content.lines() {
                        if line.starts_with("#EXT-X-ENDLIST") {
                            has_endlist = true;
                            continue;
                        }
                        if line.starts_with("seg_") && line.ends_with(".m4s") {
                            lines.push(format!("segments/{line}"));
                        } else if line.contains("URI=\"init.mp4\"") {
                            lines.push(
                                line.replace("URI=\"init.mp4\"", "URI=\"segments/init.mp4\""),
                            );
                        } else {
                            lines.push(line.to_string());
                        }
                    }
                    // If ENDLIST was present, also strip the final segment + its EXTINF
                    // if the duration is suspiciously short (ffmpeg flush fragment)
                    if has_endlist && lines.len() >= 2 {
                        tracing::warn!("live playlist contains #EXT-X-ENDLIST — ffmpeg has exited");
                        // Check if last segment's EXTINF is < 1.0s
                        let last_is_segment = lines
                            .last()
                            .map(|l| l.starts_with("segments/seg_"))
                            .unwrap_or(false);
                        if last_is_segment {
                            let prev = &lines[lines.len() - 2];
                            let short_duration = prev
                                .strip_prefix("#EXTINF:")
                                .and_then(|s| s.trim_end_matches(',').parse::<f64>().ok())
                                .map(|d| d < 1.0)
                                .unwrap_or(false);
                            if short_duration {
                                lines.pop(); // segment filename
                                lines.pop(); // EXTINF
                            }
                        }
                    }
                    let content = lines.join("\n");
                    return Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
                        .header(header::CACHE_CONTROL, "no-store")
                        .body(Body::from(content))
                        .unwrap()
                        .into_response();
                }

                // If FFmpeg is already dead and no playlist exists, don't wait
                if ffmpeg_dead {
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "stream source disconnected",
                    )
                        .into_response();
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            return (StatusCode::SERVICE_UNAVAILABLE, "playlist not ready").into_response();
        }
    }

    // Update segment counts from disk via the library, then check health.
    let (ffmpeg_running, ffmpeg_crashed) = {
        let mut session = match state.playback_sessions.get_mut(&session_id) {
            Some(s) => s,
            None => {
                return session_missing(&state, session_id);
            }
        };
        session.touch();

        // Update segment state from disk via TranscodeSession
        if let Some(ref vid) = variant_id {
            if let Some(qv) = session
                .variants
                .as_mut()
                .and_then(|v| v.get_mut(vid.as_str()))
                && let Some(ref transcode) = qv.transcode
            {
                transcode.update_segments();
            }
        } else if let Some(ref transcode) = session.transcode {
            transcode.update_segments();
        }

        // Lazy overlay scheduler: pre-emptively transcode known-corrupt ranges
        // that are within preroll of the current playback position.
        maybe_spawn_overlays(&mut session, &state);

        // Check FFmpeg health for the relevant transcode process.
        // On Err we persist onto session.error so subsequent requests hit
        // the `session.error` ENDLIST branch at the top of this handler.
        let mut persist_err: Option<String> = None;
        let result = if let Some(ref vid) = variant_id {
            match session
                .variants
                .as_mut()
                .and_then(|v| v.get_mut(vid.as_str()))
            {
                Some(qv) => match &mut qv.transcode {
                    Some(transcode) => match transcode.check_health() {
                        Ok(running) => (running, false),
                        Err(msg) => {
                            warn!(
                                session_id = %session_id,
                                variant = %vid,
                                error = %msg,
                                "FFmpeg unhealthy during stream playlist request"
                            );
                            persist_err = Some(msg);
                            (false, true)
                        }
                    },
                    None => (false, false),
                },
                None => (false, false),
            }
        } else {
            match &mut session.transcode {
                Some(transcode) => match transcode.check_health() {
                    Ok(running) => (running, false),
                    Err(msg) => {
                        warn!(
                            session_id = %session_id,
                            error = %msg,
                            "FFmpeg unhealthy during stream playlist request"
                        );
                        persist_err = Some(msg);
                        (false, true)
                    }
                },
                None => (false, false),
            }
        };
        if let Some(msg) = persist_err
            && session.error.is_none()
        {
            session.error = Some(SessionError::new("transcode_failed", msg));
        }
        result
    };

    // Build the playlist, scoped to playback_start_segment
    let session = match state.playback_sessions.get(&session_id) {
        Some(s) => s,
        None => {
            return session_missing(&state, session_id);
        }
    };

    let cache = if let Some(ref vid) = variant_id {
        match session.variants.as_ref().and_then(|v| v.get(vid.as_str())) {
            Some(qv) => &qv.segment_cache,
            None => return (StatusCode::NOT_FOUND, "variant not found").into_response(),
        }
    } else {
        &session.segment_cache
    };

    // Segment path prefix: variants use "segments/{variant_id}/gen_N/..."
    let seg_prefix = variant_id
        .as_ref()
        .map(|v| format!("segments/{v}"))
        .unwrap_or_else(|| "segments".to_string());

    // Get cached segments from the TranscodeSession (source of truth)
    let cached_segments = if let Some(ref vid) = variant_id {
        session
            .variants
            .as_ref()
            .and_then(|v| v.get(vid.as_str()))
            .and_then(|qv| qv.transcode.as_ref())
            .map(|t| t.playlist_segments_from(cache.playback_start_segment))
            .unwrap_or_default()
    } else {
        session
            .transcode
            .as_ref()
            .map(|t| t.playlist_segments_from(cache.playback_start_segment))
            .unwrap_or_default()
    };

    if cached_segments.is_empty() {
        // Demand-driven startup-stall check: if ffmpeg has been running longer
        // than `STARTUP_THRESHOLD` and still has nothing to serve, the player
        // would otherwise spin on estimated segments forever. Surface a real
        // error instead. Within the grace window we fall through to the
        // estimated/predicted path and let hls.js retry.
        let started_at = if let Some(ref vid) = variant_id {
            session
                .variants
                .as_ref()
                .and_then(|v| v.get(vid.as_str()))
                .and_then(|qv| qv.transcode.as_ref())
                .map(|t| t.started_at)
        } else {
            session.transcode.as_ref().map(|t| t.started_at)
        };
        let elapsed = started_at.map(|t| t.elapsed()).unwrap_or_default();
        if elapsed > STARTUP_THRESHOLD {
            let msg = format!(
                "Transcode produced no segments within {:.0}s of startup",
                STARTUP_THRESHOLD.as_secs_f64()
            );
            warn!(
                session_id = %session_id,
                variant = ?variant_id,
                elapsed_secs = elapsed.as_secs_f64(),
                "Startup stall — surfacing terminal error"
            );
            drop(session);
            if let Some(mut session) = state.playback_sessions.get_mut(&session_id)
                && session.error.is_none()
            {
                session.error = Some(SessionError::new("startup_stalled", msg.clone()));
            }
            return error_response("startup_stalled", &msg).into_response();
        }
        tracing::warn!(
            session_id = %session_id,
            playback_start = cache.playback_start_segment,
            elapsed_secs = elapsed.as_secs_f64(),
            "No cached segments found — within startup grace, falling through to estimated path"
        );
    }

    // Get predicted segments from keyframes via the transcode session
    let predicted = if let Some(ref vid) = variant_id {
        session
            .variants
            .as_ref()
            .and_then(|v| v.get(vid.as_str()))
            .and_then(|qv| qv.transcode.as_ref())
            .map(|t| {
                let generation = t.current_generation();
                t.predicted_segments(generation, cache.playback_start_secs)
            })
            .unwrap_or_default()
    } else {
        session
            .transcode
            .as_ref()
            .map(|t| {
                let generation = t.current_generation();
                t.predicted_segments(generation, cache.playback_start_secs)
            })
            .unwrap_or_default()
    };

    // No generation filtering: segment_index maps absolute segment positions
    // to on-disk files, so every entry returned by playlist_segments_from()
    // is valid content for that position regardless of which generation
    // produced it. Filtering by current_generation removes valid cached
    // segments from older generations after seeks, causing the playlist to
    // show only fragments from the latest generation.

    // Use cached segments if available, otherwise fall back to predicted/estimated
    if !cached_segments.is_empty() {
        let max_dur = cached_segments
            .iter()
            .map(|s| s.duration)
            .fold(0.0_f64, f64::max);
        let target_duration = (max_dur.ceil() as u32).max(cache.segment_duration);

        let mut playlist = String::new();
        playlist.push_str("#EXTM3U\n");
        playlist.push_str("#EXT-X-VERSION:7\n");
        playlist.push_str(&format!("#EXT-X-TARGETDURATION:{target_duration}\n"));
        playlist.push_str("#EXT-X-MEDIA-SEQUENCE:0\n");
        playlist.push_str("#EXT-X-PLAYLIST-TYPE:EVENT\n");
        playlist.push_str("#EXT-X-INDEPENDENT-SEGMENTS\n");

        let mut current_generation: Option<u32> = None;
        for seg in &cached_segments {
            if current_generation.is_none() || seg.is_generation_boundary {
                // DISCONTINUITY only between generations, not before the first segment
                if current_generation.is_some() && seg.is_generation_boundary {
                    playlist.push_str("#EXT-X-DISCONTINUITY\n");
                }
                playlist.push_str(&format!(
                    "#EXT-X-MAP:URI=\"{seg_prefix}/gen_{}/init.mp4\"\n",
                    seg.generation
                ));
            }
            current_generation = Some(seg.generation);

            playlist.push_str(&format!("#EXTINF:{:.6},\n", seg.duration));
            playlist.push_str(&format!(
                "{seg_prefix}/gen_{}/seg_{:05}.m4s\n",
                seg.generation, seg.local_index
            ));
        }

        // Pad sparse playlists so hls.js always has enough segments to start
        // playback immediately. serve_segment block-waits for files to appear.
        if cached_segments.len() < 3 && ffmpeg_running {
            let last_seg = cached_segments.last().unwrap();
            let last_gen = last_seg.generation;
            let next_local = last_seg.local_index + 1;
            let abs_emitted = cache.playback_start_segment + cached_segments.len() as u32;
            let remaining = cache.total_segments.saturating_sub(abs_emitted);
            let pad_count = (3 - cached_segments.len()).min(remaining as usize);
            let seg_dur = f64::from(cache.segment_duration);
            for i in 0..pad_count {
                let local_idx = next_local + i as u32;
                playlist.push_str(&format!("#EXTINF:{seg_dur:.6},\n"));
                playlist.push_str(&format!(
                    "{seg_prefix}/gen_{last_gen}/seg_{local_idx:05}.m4s\n"
                ));
            }
        }

        let emitted_duration: f64 = cached_segments.iter().map(|s| s.duration).sum();
        let start_time = cache.playback_start_secs;
        let all_emitted = (start_time + emitted_duration + f64::from(cache.segment_duration))
            >= cache.duration_seconds;
        // Only finalize when the emitted content math confirms we've
        // covered the full duration. Never trust FFmpeg process state —
        // it can exit 0 on truncated/corrupt files with content missing.
        // If all_emitted barely misses due to rounding, hls.js polls once
        // more and naturally finishes when no new segments appear.
        if all_emitted {
            playlist.push_str("#EXT-X-ENDLIST\n");
        }

        if !ffmpeg_running && !all_emitted {
            let remaining = cache.duration_seconds - (start_time + emitted_duration);
            let seg_count = cached_segments.len();
            warn!(
                session_id = %session_id,
                variant = ?variant_id,
                crashed = ffmpeg_crashed,
                segments = seg_count,
                emitted_secs = format_args!("{:.1}", start_time + emitted_duration),
                total_secs = format_args!("{:.1}", cache.duration_seconds),
                remaining_secs = format_args!("{:.1}", remaining),
                "FFmpeg stopped with incomplete content"
            );
        }

        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::from(playlist))
            .unwrap()
            .into_response();
    }

    let generation = if let Some(ref vid) = variant_id {
        session
            .variants
            .as_ref()
            .and_then(|v| v.get(vid.as_str()))
            .and_then(|qv| qv.transcode.as_ref())
            .map(|t| t.current_generation())
            .unwrap_or(0)
    } else {
        session
            .transcode
            .as_ref()
            .map(|t| t.current_generation())
            .unwrap_or(0)
    };

    // NOTE: predicted segments from keyframes are intentionally skipped.
    // Predicted durations don't exactly match FFmpeg's actual output, causing
    // MSE decode errors (VOD) or timeline confusion on transition (EVENT).
    // The estimated path below gives hls.js enough to start, and serve_segment
    // waits for FFmpeg. The cached path takes over once real segments exist.
    //
    // The `predicted` variable is still computed above for potential future use
    // (e.g. synthetic full-manifest mode) but not served.
    let _ = &predicted;

    let target_duration = cache.segment_duration;
    let start = cache.playback_start_segment;

    // Playback position is at or past the end — return a finished playlist
    // with no segments so hls.js fires "ended" instead of 404-ing on init.mp4.
    if start >= cache.total_segments {
        let playlist = format!(
            "#EXTM3U\n\
             #EXT-X-VERSION:7\n\
             #EXT-X-TARGETDURATION:{target_duration}\n\
             #EXT-X-MEDIA-SEQUENCE:0\n\
             #EXT-X-PLAYLIST-TYPE:VOD\n\
             #EXT-X-ENDLIST\n"
        );
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::from(playlist))
            .unwrap()
            .into_response();
    }

    // No cached or predicted segments yet — FFmpeg is still working.
    // Serve an EVENT playlist with a small number of estimated segments.
    // hls.js needs at least one segment to start buffering — serve_segment
    // will wait up to 10s for each segment file to appear on disk.
    // Only list a few segments (not all 500+) so hls.js doesn't queue
    // hundreds of requests for files that don't exist yet.
    let seg_dur = f64::from(cache.segment_duration);
    let estimated_count = 3u32; // Enough for hls.js to start, not so many it times out

    let mut playlist = String::new();
    playlist.push_str("#EXTM3U\n");
    playlist.push_str("#EXT-X-VERSION:7\n");
    playlist.push_str(&format!("#EXT-X-TARGETDURATION:{target_duration}\n"));
    playlist.push_str("#EXT-X-MEDIA-SEQUENCE:0\n");
    playlist.push_str("#EXT-X-PLAYLIST-TYPE:EVENT\n");
    playlist.push_str("#EXT-X-INDEPENDENT-SEGMENTS\n");
    playlist.push_str(&format!(
        "#EXT-X-MAP:URI=\"{seg_prefix}/gen_{generation}/init.mp4\"\n"
    ));

    for i in 0..estimated_count.min(cache.total_segments - start) {
        playlist.push_str(&format!("#EXTINF:{seg_dur:.6},\n"));
        playlist.push_str(&format!("{seg_prefix}/gen_{generation}/seg_{i:05}.m4s\n"));
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(playlist))
        .unwrap()
        .into_response()
}

// =============================================================================
// Segment Serving (wildcard route: segments/*path)
// =============================================================================

pub async fn serve_segment(
    State(state): State<AppState>,
    Path((session_id, seg_path)): Path<(String, String)>,
) -> impl IntoResponse {
    let session_id = match Uuid::parse_str(&session_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid session_id").into_response(),
    };
    wait_out_restart(&state, session_id).await;

    // Debrid sessions before probe completes have no segments yet
    {
        let session_state = state.playback_sessions.get(&session_id);
        if session_state.is_some_and(|s| s.file_path.as_os_str().is_empty()) {
            return session_pending_response();
        }
    }

    // Live IPTV sessions: ffmpeg writes flat .ts files (no gen_N/ prefix).
    // Serve directly from base_dir with simple filename validation.
    {
        let is_live = state
            .playback_sessions
            .get(&session_id)
            .is_some_and(|s| s.is_live);
        if is_live {
            return serve_live_segment(&state, session_id, &seg_path).await;
        }
    }

    // Sanitize path: only allow gen_N/filename or variant_id/gen_N/filename
    let sanitized = match sanitize_segment_path(&seg_path) {
        Some(s) => s,
        None => return (StatusCode::BAD_REQUEST, "invalid segment path").into_response(),
    };

    // Detect if this is a variant segment (3 components: variant_id/gen_N/file)
    let variant_id = {
        let parts: Vec<&str> = sanitized.split('/').collect();
        if parts.len() == 3 {
            Some(parts[0].to_string())
        } else {
            None
        }
    };

    let segment_path = {
        let mut session = match state.playback_sessions.get_mut(&session_id) {
            Some(s) => s,
            None => {
                return session_missing(&state, session_id);
            }
        };
        session.touch();
        // If the session has already been marked terminal (stall, crash,
        // probe failure), short-circuit with the same 500 the playlist would
        // return. The frontend's hls.js handler treats both surfaces equally.
        if let Some(ref err) = session.error {
            return error_response(err.code, &err.message).into_response();
        }
        session.segment_dir().join(&sanitized)
    };

    // Demand-driven stall detection: a stall is "the player asked for a
    // segment that doesn't exist yet, and `STALL_THRESHOLD` later it still
    // doesn't exist". Producer-side polling (count of segments written) was
    // wrong because legitimate cache-ahead / cached-seek looks identical to a
    // stuck ffmpeg from the producer's POV.
    //
    // For media segments, wait for the NEXT segment to exist as proof that
    // FFmpeg has finished writing the current one (Jellyfin's approach). For
    // init.mp4, just check existence.
    let next_segment_path = next_media_segment_path(&segment_path);
    let requested_index = parse_segment_index(&segment_path);

    // Track the real playhead so the lazy overlay scheduler can pre-emptively
    // spawn overlays as the player moves forward. `playback_start_segment`
    // only updates on seek; this catches continuous playback. Skip variants —
    // they share the same physical timeline as the primary, and we only want
    // one signal per session.
    if variant_id.is_none()
        && let Some(idx) = requested_index
        && let Some(mut session) = state.playback_sessions.get_mut(&session_id)
    {
        // The file name counts from its own generation's first segment.
        let run_start = parse_generation(&sanitized)
            .and_then(|g| {
                session
                    .transcode
                    .as_ref()?
                    .runs()
                    .into_iter()
                    .find(|r| r.generation == g)
            })
            .map_or(0, |r| r.run_start);
        let abs = run_start + idx;
        session.highest_served_segment = Some(
            session
                .highest_served_segment
                .map_or(abs, |prev| prev.max(abs)),
        );
    }

    let wait_started = std::time::Instant::now();
    loop {
        if segment_path.exists()
            && let Ok(meta) = tokio::fs::metadata(&segment_path).await
            && meta.len() > 0
        {
            let next_ok = match next_segment_path.as_ref() {
                None => true,
                Some(p) => p.exists(),
            };
            if next_ok {
                break;
            }
            // Check if the relevant FFmpeg process is done — if so, the
            // current segment is the last one we'll ever see.
            if let Some(mut session) = state.playback_sessions.get_mut(&session_id) {
                let is_done = if let Some(ref vid) = variant_id {
                    session
                        .variants
                        .as_mut()
                        .and_then(|v| v.get_mut(vid.as_str()))
                        .and_then(|qv| qv.transcode.as_mut())
                        .is_some_and(|t| !t.is_running())
                } else {
                    session.transcode.as_mut().is_some_and(|t| !t.is_running())
                };
                if is_done {
                    break;
                }
            }
        }

        // Update the per-session pending request and check for stall.
        // Variant requests share the same `pending_segment` slot — variants
        // and the primary stream don't usually overlap from one player, so
        // collapsing them is fine. (TODO: per-variant pending tracking.)
        if let Some(idx) = requested_index
            && let Some(mut session) = state.playback_sessions.get_mut(&session_id)
        {
            let now = std::time::Instant::now();
            let first_at = match session.pending_segment {
                Some(pending) if pending.segment_index == idx => pending.first_requested_at,
                _ => {
                    session.pending_segment = Some(PendingSegmentRequest {
                        segment_index: idx,
                        first_requested_at: now,
                    });
                    now
                }
            };
            if first_at.elapsed() > STALL_THRESHOLD {
                let msg = format!(
                    "Stalled waiting for segment {idx} for {:.1}s",
                    first_at.elapsed().as_secs_f64()
                );
                warn!(
                    session_id = %session_id,
                    variant = ?variant_id,
                    segment_index = idx,
                    "Demand-driven stall detected"
                );
                if session.error.is_none() {
                    session.error = Some(SessionError::new("transcode_stalled", msg.clone()));
                }
                return error_response("transcode_stalled", &msg).into_response();
            }
        }

        // Check FFmpeg health for the relevant process. Now that check_health
        // is just "is process alive", an Err here only fires for a real crash.
        if let Some(mut session) = state.playback_sessions.get_mut(&session_id) {
            let health_err = if let Some(ref vid) = variant_id {
                session
                    .variants
                    .as_mut()
                    .and_then(|v| v.get_mut(vid.as_str()))
                    .and_then(|qv| qv.transcode.as_mut())
                    .and_then(|t| t.check_health().err())
            } else {
                session
                    .transcode
                    .as_mut()
                    .and_then(|t| t.check_health().err())
            };
            if let Some(msg) = health_err {
                if session.error.is_none() {
                    session.error = Some(SessionError::new("transcode_failed", msg.clone()));
                }
                return error_response("transcode_failed", &msg).into_response();
            }
        }

        // Belt-and-braces: in case `requested_index` is None (init.mp4) and
        // the file never appears, still bound the wait so we don't loop
        // forever. STALL_THRESHOLD is the same budget used for the index path.
        if requested_index.is_none() && wait_started.elapsed() > STALL_THRESHOLD {
            break;
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    if !segment_path.exists() {
        return (StatusCode::NOT_FOUND, "segment not found").into_response();
    }

    // The requested segment was served — clear any pending stall tracker.
    if let Some(mut session) = state.playback_sessions.get_mut(&session_id)
        && let Some(pending) = session.pending_segment
        && requested_index == Some(pending.segment_index)
    {
        session.pending_segment = None;
    }

    let filename = sanitized.rsplit('/').next().unwrap_or(&sanitized);
    let content_type = if filename.ends_with(".m4s") || filename.ends_with(".mp4") {
        "video/mp4"
    } else if filename.ends_with(".ts") {
        "video/mp2t"
    } else {
        "application/octet-stream"
    };

    match tokio::fs::File::open(&segment_path).await {
        Ok(file) => {
            let stream = ReaderStream::new(file);
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, content_type)
                .header(header::CACHE_CONTROL, "no-store")
                .body(Body::from_stream(stream))
                .unwrap()
                .into_response()
        }
        Err(e) => {
            error!(error = %e, path = %segment_path.display(), "Failed to open segment");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to read segment").into_response()
        }
    }
}

/// Serve a segment for a live IPTV session.
///
/// Live ffmpeg writes flat fMP4 files directly in the session directory
/// (no gen_N/ subdirectory). Accepts `init.mp4` (fMP4 init segment) and
/// `seg_NNNNN.m4s` (fMP4 media segments).
async fn serve_live_segment(
    state: &AppState,
    session_id: Uuid,
    filename: &str,
) -> axum::response::Response {
    // Validate: must be seg_NNNNN.m4s or init.mp4 with no path separators or traversal
    let is_valid = !filename.contains('/')
        && !filename.contains('\\')
        && !filename.contains("..")
        && (filename.starts_with("seg_") && filename.ends_with(".m4s") || filename == "init.mp4");
    if !is_valid {
        return (StatusCode::BAD_REQUEST, "invalid live segment path").into_response();
    }

    let segment_path = {
        let mut session = match state.playback_sessions.get_mut(&session_id) {
            Some(s) => s,
            None => {
                return session_missing(state, session_id);
            }
        };
        session.touch();
        session.segment_cache.base_dir.join(filename)
    };

    if !segment_path.exists() {
        return (StatusCode::NOT_FOUND, "segment not found").into_response();
    }

    let content_type = if filename == "init.mp4" {
        "video/mp4"
    } else {
        "video/iso.segment"
    };

    match tokio::fs::File::open(&segment_path).await {
        Ok(file) => {
            let stream = ReaderStream::new(file);
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, content_type)
                .header(header::CACHE_CONTROL, "no-store")
                .body(Body::from_stream(stream))
                .unwrap()
                .into_response()
        }
        Err(e) => {
            error!(error = %e, path = %segment_path.display(), "Failed to open live segment");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to read segment").into_response()
        }
    }
}

/// Validate and sanitize a segment path.
///
/// Accepts two forms:
/// - `gen_N/filename` — primary variant segments
/// - `variant_id/gen_N/filename` — ABR variant segments (e.g., `720p/gen_0/seg_00005.m4s`)
///
/// Returns None if the path is invalid or contains traversal attempts.
fn sanitize_segment_path(path: &str) -> Option<String> {
    if path.contains("..") || path.starts_with('/') || path.starts_with('\\') {
        return None;
    }

    let parts: Vec<&str> = path.split('/').collect();

    let (dir, file) = match parts.len() {
        // gen_N/filename
        2 => (parts[0], parts[1]),
        // variant_id/gen_N/filename — validate variant_id is alphanumeric
        3 => {
            let variant = parts[0];
            if !variant
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                return None;
            }
            // Return the full 3-component path
            let dir = parts[1];
            let file = parts[2];
            if !dir.starts_with("gen_") || dir[4..].parse::<u32>().is_err() {
                return None;
            }
            let valid_file = file == "init.mp4"
                || (file.starts_with("seg_") && (file.ends_with(".m4s") || file.ends_with(".ts")));
            if !valid_file {
                return None;
            }
            return Some(format!("{variant}/{dir}/{file}"));
        }
        _ => return None,
    };

    // Dir must be gen_N where N is a number
    if !dir.starts_with("gen_") || dir[4..].parse::<u32>().is_err() {
        return None;
    }

    // File must be a known pattern
    let valid_file = file == "init.mp4"
        || (file.starts_with("seg_") && (file.ends_with(".m4s") || file.ends_with(".ts")));
    if !valid_file {
        return None;
    }

    Some(format!("{dir}/{file}"))
}

/// Parse the segment index from a path like `.../gen_0/seg_00005.m4s`.
/// Returns `None` for init.mp4 or unparseable names.
fn parse_generation(segment_path: &str) -> Option<u32> {
    segment_path
        .split('/')
        .find_map(|part| part.strip_prefix("gen_"))?
        .parse()
        .ok()
}

fn parse_segment_index(path: &std::path::Path) -> Option<u32> {
    let filename = path.file_name()?.to_str()?;
    let stem = filename.strip_prefix("seg_")?;
    let num_str = stem
        .strip_suffix(".m4s")
        .or_else(|| stem.strip_suffix(".ts"))?;
    num_str.parse().ok()
}

/// Given a segment path like `.../gen_0/seg_00005.m4s`, return the path to the
/// next segment (`seg_00006.m4s`). Returns `None` for init.mp4 or unparseable names.
/// Used to verify a segment is fully written (Jellyfin's approach: if the next
/// segment exists, the current one is complete).
fn next_media_segment_path(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let filename = path.file_name()?.to_str()?;
    let ext = if filename.ends_with(".m4s") {
        ".m4s"
    } else if filename.ends_with(".ts") {
        ".ts"
    } else {
        return None;
    };
    let num = parse_segment_index(path)?;
    let next_name = format!("seg_{:05}{ext}", num + 1);
    Some(path.with_file_name(next_name))
}

#[cfg(test)]
mod path_parsing_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_segment_index_extracts_number() {
        assert_eq!(
            parse_segment_index(&PathBuf::from("/x/gen_0/seg_00005.m4s")),
            Some(5)
        );
        assert_eq!(
            parse_segment_index(&PathBuf::from("/x/gen_2/seg_01234.ts")),
            Some(1234)
        );
        assert_eq!(parse_segment_index(&PathBuf::from("/x/init.mp4")), None);
        assert_eq!(parse_segment_index(&PathBuf::from("/x/seg_abc.m4s")), None);
    }

    #[test]
    fn next_media_segment_path_round_trips_with_parser() {
        let p = PathBuf::from("/x/gen_0/seg_00005.m4s");
        let next = next_media_segment_path(&p).unwrap();
        assert_eq!(parse_segment_index(&next), Some(6));
        assert_eq!(next.file_name().unwrap().to_str().unwrap(), "seg_00006.m4s");
    }
}

// =============================================================================
// Seek
// =============================================================================

pub async fn seek(
    State(state): State<AppState>,
    _auth: PlaybackAuth,
    Path(session_id): Path<String>,
    Json(req): Json<SeekRequest>,
) -> impl IntoResponse {
    let session_id = match Uuid::parse_str(&session_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid session_id").into_response(),
    };

    let _gate = state.session_gates.claim(session_id).await;
    // Out of the map so the FFmpeg restart below holds no shard lock; readers wait on
    // the gate until it is back.
    let (_, mut session) = match state.playback_sessions.remove(&session_id) {
        Some(pair) => pair,
        None => {
            return (StatusCode::NOT_FOUND, "session not found").into_response();
        }
    };
    session.touch();

    // Debrid sessions before probe completes have empty file_path
    if session.file_path.as_os_str().is_empty() {
        state.playback_sessions.insert(session_id, session);
        return session_pending_response();
    }

    // Seeking at or past the end of the file — nothing to transcode.
    // Kill FFmpeg, bump the generation so the frontend gets a fresh playlist,
    // and return a response that produces a finished (zero-segment) HLS playlist.
    if req.time_seconds >= session.duration_seconds {
        if let Some(ref mut transcode) = session.transcode {
            transcode.kill().await;
        }
        session.transcode = None;
        if let Some(ref mut variants) = session.variants {
            for variant in variants.values_mut() {
                if let Some(ref mut transcode) = variant.transcode {
                    transcode.kill().await;
                }
                variant.transcode = None;
            }
        }
        session.seek_generation += 1;
        let end = session.segment_cache.total_segments;
        session.move_playlist_start(end, session.duration_seconds);
        let generation = session.seek_generation;
        let start_time = session.duration_seconds;
        state.playback_sessions.insert(session_id, session);
        return Json(json!({
            "seek_generation": generation,
            "from_cache": false,
            "start_time": start_time,
        }))
        .into_response();
    }

    // Use actual segment durations when available for accurate seek positioning.
    let (target_segment, aligned_start_time) = if let Some(ref transcode) = session.transcode {
        transcode.update_segments();
        let seg = transcode.time_to_segment_accurate(req.time_seconds);
        (seg, transcode.segment_to_time_accurate(seg))
    } else {
        let seg = session.segment_cache.time_to_segment(req.time_seconds);
        (seg, session.segment_cache.segment_to_time(seg))
    };

    // If the seek target is already cached (segment exists in the index),
    // skip the FFmpeg restart — just update the playback position. This
    // prevents the "create session then immediately seek to resume position"
    // pattern from killing FFmpeg and creating a new generation.
    // Also handles seeks within already-produced content during transcoding.
    if let Some(ref transcode) = session.transcode {
        transcode.update_segments();
        let segments = transcode.playlist_segments_from(target_segment);
        if !segments.is_empty() {
            session.move_playlist_start(target_segment, aligned_start_time);
            session.seek_generation += 1;
            let generation = session.seek_generation;
            info!(
                "Seek to {:.1}s — {} cached segments from target, no FFmpeg restart needed",
                req.time_seconds,
                segments.len()
            );
            state.playback_sessions.insert(session_id, session);
            return Json(json!({
                "seek_generation": generation,
                "from_cache": true,
                "start_time": aligned_start_time,
                "sub_segment_offset": (req.time_seconds - aligned_start_time).max(0.0),
            }))
            .into_response();
        }
    }

    // Kill all variant transcodes — they'll lazy-restart on next request
    if let Some(ref mut variants) = session.variants {
        for variant in variants.values_mut() {
            if let Some(ref mut transcode) = variant.transcode {
                transcode.kill().await;
            }
            variant.transcode = None;
        }
    }

    // Update playback start
    session.move_playlist_start(target_segment, aligned_start_time);
    session.seek_generation += 1;

    // Update variant caches with the new playback start
    if let Some(ref mut variants) = session.variants {
        for variant in variants.values_mut() {
            variant
                .segment_cache
                .follow_playback_start(&session.segment_cache);
        }
    }

    // Use the TranscodeSession's seek (handles cache check + new generation internally)
    let mut from_cache = false;
    if let Some(ref mut transcode) = session.transcode {
        // First update segment state so the seek cache check is current
        transcode.update_segments();

        if let Some(seek_result) = transcode.seek(req.time_seconds) {
            match &seek_result {
                kob_ffmpeg::session::SeekResult::Cached {
                    restart_from_secs, ..
                } => {
                    from_cache = true;
                    let restart_secs = *restart_from_secs;
                    info!(
                        from_cache = true,
                        "In-process seek (cached) to {:.1}s, restart from {:.1}s",
                        req.time_seconds,
                        restart_secs
                    );

                    // Don't redirect for cached seeks — redirecting bumps the
                    // generation and register_run() clears the segment_index,
                    // creating permanent gaps. The thread continues producing
                    // from wherever it is, filling remaining gaps naturally.
                    if restart_secs >= session.duration_seconds {
                        // File fully remuxed — kill the FFmpeg thread but keep the
                        // TranscodeSession alive so cached segment state (runs,
                        // segment_index) remains accessible for playlist generation.
                        transcode.kill().await;
                    }
                }
                kob_ffmpeg::session::SeekResult::NewGeneration { generation } => {
                    info!(
                        generation,
                        "In-process seek (new generation) to {:.1}s", req.time_seconds
                    );
                }
            }
        } else {
            // Session seek failed (e.g. thread died) — fall back to kill+restart
            transcode.kill().await;
            let base_dir = session.segment_cache.base_dir.clone();
            match start_ffmpeg_for_session(
                &session,
                base_dir,
                Some(aligned_start_time),
                &state,
                None,
                session.seek_generation,
                Vec::new(),
            )
            .await
            {
                Ok(process) => {
                    if let Some(ref kf) = session.keyframes {
                        process.set_keyframes(kf.clone());
                    }
                    session.transcode = Some(process);
                }
                Err(e) => {
                    error!(error = %e, "Failed to restart FFmpeg after seek");
                    state.playback_sessions.insert(session_id, session);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Seek failed: {e}"),
                    )
                        .into_response();
                }
            }
        }
    } else {
        // No existing transcode — start fresh
        let base_dir = session.segment_cache.base_dir.clone();
        match start_ffmpeg_for_session(
            &session,
            base_dir,
            Some(aligned_start_time),
            &state,
            None,
            session.seek_generation,
            Vec::new(),
        )
        .await
        {
            Ok(process) => {
                if let Some(ref kf) = session.keyframes {
                    process.set_keyframes(kf.clone());
                }
                session.transcode = Some(process);
            }
            Err(e) => {
                error!(error = %e, "Failed to start FFmpeg for seek");
                state.playback_sessions.insert(session_id, session);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Seek failed: {e}"),
                )
                    .into_response();
            }
        }
    }

    // Evict old generations if cache exceeds budget
    if let Some(ref transcode) = session.transcode {
        transcode.evict_if_needed(session.segment_cache.playback_start_segment);
    }

    let generation = session.seek_generation;

    // Re-insert session into the map
    state.playback_sessions.insert(session_id, session);

    Json(json!({
        "seek_generation": generation,
        "from_cache": from_cache,
        "start_time": aligned_start_time,
        "sub_segment_offset": (req.time_seconds - aligned_start_time).max(0.0),
    }))
    .into_response()
}

// =============================================================================
// Decode Error (Tier-3 corruption fallback)
// =============================================================================

/// Player POSTs here when it observes `MEDIA_ERR_DECODE` (Chrome HW decoder
/// gave up on the source bitstream). Server kills the current FFmpeg, flips
/// the session into transcode mode, and restarts from the snapped keyframe
/// before `at_seconds` so the player can reload the manifest and resume on
/// a clean re-encoded stream.
///
/// Also persists the discovered range to the file row's `corrupt_ranges` so
/// future plays of the same file get pre-emptive transcode (or, in the next
/// iteration, an overlay generation) without waiting for the player to crash
/// again.
///
/// V1: full-session restart — proves the player→server flow and validates
/// that hls.js handles the cross-generation transition cleanly. The follow-up
/// task switches native sessions to `start_overlay()` (overlay-only re-encode
/// of the corrupt window); debrid first-play stays on full restart because
/// the cached source bytes may not extend past the corrupt point yet.
pub async fn decode_error(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(session_id): Path<String>,
    Json(req): Json<DecodeErrorRequest>,
) -> impl IntoResponse {
    let session_id = match Uuid::parse_str(&session_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid session_id").into_response(),
    };

    let _gate = state.session_gates.claim(session_id).await;
    let (_, mut session) = match state.playback_sessions.remove(&session_id) {
        Some(pair) => pair,
        None => return (StatusCode::NOT_FOUND, "session not found").into_response(),
    };
    session.touch();

    if session.file_path.as_os_str().is_empty() {
        state.playback_sessions.insert(session_id, session);
        return session_pending_response();
    }

    let at = req.at_seconds.clamp(0.0, session.duration_seconds.max(0.0));
    let already_transcoding = session.force_video_transcode
        || matches!(
            session.decision.video_mode,
            decision::VideoMode::Transcode { .. }
        );
    if already_transcoding {
        // Player still hit a decode error after we already forced transcode —
        // either the encoder is mis-set up or the corruption is in the
        // post-encode delivery path. Don't loop; return 409 so the client
        // surfaces the error instead of POSTing again.
        info!(
            %session_id, at,
            "decode-error received but session is already in transcode mode — refusing to loop"
        );
        state.playback_sessions.insert(session_id, session);
        return (
            StatusCode::CONFLICT,
            "session is already transcoding — corruption is downstream of the encoder",
        )
            .into_response();
    }

    // Snap to a source keyframe at-or-before `at` so we land on an IDR.
    let snapped_start = session
        .keyframes
        .as_ref()
        .and_then(|kf| kf.iter().rev().find(|&&t| t <= at).copied())
        .unwrap_or(at);

    // Path A: native VOD with reopenable file path → spawn an overlay
    // generation covering the corrupt window. Primary copy stream stays alive
    // and post-overlay segments resume on the original generation.
    //
    // Path B: debrid (cached bytes may not extend past playhead yet) / IPTV
    // (custom IO, no reopenable path) → fall back to v1 full-restart in
    // transcode mode for the rest of the session.
    let overlay_window_end = (at + 30.0).min(session.duration_seconds.max(0.0));
    let prefer_overlay = !session.is_live && session.debrid_cache_key.is_none();
    let mut overlay_succeeded = false;
    let mut overlay_generation: Option<u32> = None;
    if prefer_overlay {
        let target_codec = match &session.decision.video_mode {
            decision::VideoMode::Transcode { target_codec } => target_codec.clone(),
            decision::VideoMode::Copy => decision::best_transcode_codec(
                &state.encoder_caps.preferred_codecs(),
                &decision::ClientCapabilities::default(),
            ),
        };
        let encoder = state
            .encoder_caps
            .encoder_for(&target_codec)
            .unwrap_or(&state.encoder_caps.h264)
            .to_string();
        let hw_device = state.hw_device.get();

        let (audio_encoder, audio_bitrate) = match &session.decision.audio_mode {
            decision::AudioMode::Copy => (None, None),
            decision::AudioMode::Transcode {
                target_codec,
                bitrate_kbps,
            } => (
                Some(target_codec.clone()),
                Some(i64::from(*bitrate_kbps) * 1000),
            ),
        };
        if let Some(ref transcode) = session.transcode {
            match transcode.start_overlay(
                snapped_start..overlay_window_end,
                kob_ffmpeg::session::OverlayConfig {
                    video_encoder: encoder,
                    video_bitrate: None,
                    audio_encoder,
                    audio_bitrate,
                    hw_device,
                },
            ) {
                Ok(overlay_gen) => {
                    info!(
                        %session_id, at, snapped_start, overlay_window_end, overlay_generation = overlay_gen,
                        "Tier-3 decode-error: spawned overlay (no full restart)"
                    );
                    overlay_succeeded = true;
                    overlay_generation = Some(overlay_gen);
                    session
                        .overlay_spawned_starts
                        .insert(snapped_start.to_bits());
                    session.seek_generation += 1;
                }
                Err(e) => {
                    warn!(
                        %session_id, error = %e,
                        "Tier-3 decode-error: start_overlay failed, falling back to full restart"
                    );
                }
            }
        }
    }

    if !overlay_succeeded {
        info!(
            %session_id,
            at,
            media_type = ?session.media_type,
            "Tier-3 decode-error: full-session restart in transcode mode"
        );

        // Kill current transcode + variants — same dance as `seek` does.
        if let Some(ref mut transcode) = session.transcode {
            transcode.kill().await;
        }
        session.transcode = None;
        if let Some(ref mut variants) = session.variants {
            for variant in variants.values_mut() {
                if let Some(ref mut transcode) = variant.transcode {
                    transcode.kill().await;
                }
                variant.transcode = None;
            }
        }

        session.force_video_transcode = true;
        session.seek_generation += 1;

        let target_segment = session.segment_cache.time_to_segment(snapped_start);
        session.move_playlist_start(target_segment, snapped_start);
        if let Some(ref mut variants) = session.variants {
            for variant in variants.values_mut() {
                variant
                    .segment_cache
                    .follow_playback_start(&session.segment_cache);
            }
        }

        let generation = session.seek_generation;
        let base_dir = session.segment_cache.base_dir.clone();
        let restart_result = start_ffmpeg_for_session(
            &session,
            base_dir,
            Some(snapped_start),
            &state,
            None,
            generation,
            Vec::new(),
        )
        .await;
        match restart_result {
            Ok(process) => {
                if let Some(ref kf) = session.keyframes {
                    process.set_keyframes(kf.clone());
                }
                session.transcode = Some(process);
            }
            Err(e) => {
                error!(error = %e, "Failed to restart FFmpeg after decode-error");
                state.playback_sessions.insert(session_id, session);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Restart failed: {e}"),
                )
                    .into_response();
            }
        }
    }
    let _ = overlay_generation; // logged above; no further use needed

    // Persist the discovered range so subsequent plays pre-emptively transcode
    // the affected window. Best-effort — failures here don't block recovery.
    // IPTV (`is_live`) uses `MediaType::Episode` as a placeholder with a
    // channel_id in `file_id`, so persistence is skipped — corrupt_ranges
    // tracking only makes sense for finite VOD sources.
    if !session.is_live {
        let pool = state.pool.clone();
        let media_type = session.media_type;
        let file_id = session.file_id;
        let observed_range = crate::corruption::CorruptRange {
            start_secs: (at - 2.0).max(0.0),
            end_secs: at + 10.0,
            frame_count: 0,
            concealment: false,
            missing_reference: false,
            invalid_bitstream: false,
            decode_slices_failed: false,
            other: true,
        };
        tokio::spawn(async move {
            use crate::repositories::{movie_files, tv_episode_files};
            use crate::streaming::session::MediaType;
            let res = match media_type {
                MediaType::Episode => {
                    tv_episode_files::append_corrupt_range(&pool, file_id, observed_range).await
                }
                MediaType::Movie => {
                    movie_files::append_corrupt_range(&pool, file_id, observed_range).await
                }
                MediaType::Recording => Ok(false), // recordings aren't tracked in either table
            };
            if let Err(e) = res {
                warn!(%file_id, error = %e, "decode-error: append_corrupt_range failed");
            }
        });
    }

    let response_generation = session.seek_generation;
    let is_live = session.is_live;
    let is_debrid = session.debrid_cache_key.is_some();
    state.playback_sessions.insert(session_id, session);

    // One structured emission per decode-error so a log query can count
    // tier3_path:overlay vs tier3_path:full_restart and see real-world split.
    info!(
        target: "tier3",
        %session_id,
        tier3_path = if overlay_succeeded { "overlay" } else { "full_restart" },
        outcome = "succeeded",
        at_seconds = at,
        snapped_start,
        is_live,
        is_debrid,
        overlay_generation,
        "Tier-3 decode-error handled"
    );

    Json(json!({
        "seek_generation": response_generation,
        "start_time": snapped_start,
        "force_video_transcode": !overlay_succeeded,
        "overlay_generation": overlay_generation,
    }))
    .into_response()
}

// =============================================================================
// Switch Audio Track
// =============================================================================

/// Switch the audio track on an active session without destroying it.
///
/// Kills the current FFmpeg, re-evaluates the playback decision for the new
/// audio track, starts a fresh transcode, and returns an updated session
/// response. The session ID and segment directory are preserved.
pub async fn switch_audio(
    State(state): State<AppState>,
    _auth: PlaybackAuth,
    Path(session_id): Path<String>,
    Json(req): Json<SwitchAudioRequest>,
) -> impl IntoResponse {
    let session_id = match Uuid::parse_str(&session_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid session_id").into_response(),
    };

    let _gate = state.session_gates.claim(session_id).await;
    let (_, mut session) = match state.playback_sessions.remove(&session_id) {
        Some(pair) => pair,
        None => {
            return (StatusCode::NOT_FOUND, "session not found").into_response();
        }
    };
    session.touch();

    // Validate: debrid sessions before probe completes have empty file_path
    if session.file_path.as_os_str().is_empty() {
        state.playback_sessions.insert(session_id, session);
        return session_pending_response();
    }

    // Resolve audio stream index: if None, keep the current track.
    let audio_stream_index = req
        .audio_stream_index
        .unwrap_or(session.decision.audio_stream_index);

    // Validate: audio track exists (clone to avoid borrow conflict with session mutation)
    let new_track = match session
        .audio_metas
        .iter()
        .find(|a| a.stream_index == audio_stream_index)
        .cloned()
    {
        Some(t) => t,
        None => {
            state.playback_sessions.insert(session_id, session);
            return (
                StatusCode::BAD_REQUEST,
                "audio stream index not found in session",
            )
                .into_response();
        }
    };

    let session_file_id = session.file_id;

    // A debrid direct-play client cannot pick a track from the raw file itself; asking for
    // one, even the one the decision already names, means it wants a stream carrying it.
    let debrid_direct_play =
        session.download.is_some() && session.decision.method == PlaybackMethod::DirectPlay;
    if session.decision.audio_stream_index == audio_stream_index
        && req.burn_in_subtitle_index.is_none()
        && !debrid_direct_play
    {
        let base_url = format!("/api/v1/admin/playback/sessions/{session_id}");
        let audio_responses = build_audio_responses(&session.audio_metas, audio_stream_index);
        let method = session.decision.method;
        let duration = session.duration_seconds;
        let (sw, sh) = session.source_resolution;
        state.playback_sessions.insert(session_id, session);
        return Json(SessionResponse {
            session_id: session_id.to_string(),
            method,
            playlist_url: if method != PlaybackMethod::DirectPlay {
                Some(format!("{base_url}/master.m3u8"))
            } else {
                None
            },
            direct_url: if method == PlaybackMethod::DirectPlay {
                Some(format!("{base_url}/direct"))
            } else {
                None
            },
            duration_seconds: duration,
            subtitles: vec![],
            audio_tracks: audio_responses,
            thumbnails_url: None,
            lufs: None,
            chapters: vec![],
            skip_segments: vec![],
            start_time: None,
            media_id: None,
            media_type: None,
            file_id: Some(session_file_id.to_string()),
            selected_audio_index: Some(audio_stream_index),
            preferred_audio_stream_index: None,
            selected_subtitle_index: None,
            video_width: if sw > 0 { Some(sw) } else { None },
            video_height: if sh > 0 { Some(sh) } else { None },
            picture: None,
        })
        .into_response();
    }

    // Kill all current transcodes (primary + ABR variants)
    session.kill_all_transcodes().await;
    session.transcode = None;
    if let Some(ref mut variants) = session.variants {
        for variant in variants.values_mut() {
            variant.transcode = None;
        }
    }

    // Re-evaluate audio mode for the new track.
    // HLS always supports AAC; copy if the track is already AAC, otherwise transcode.
    let audio_codec_norm = decision::normalize_codec(&new_track.codec);
    let audio_mode = if audio_codec_norm == "aac" {
        decision::AudioMode::Copy
    } else {
        decision::AudioMode::Transcode {
            target_codec: AAC_ENCODER.to_string(),
            bitrate_kbps: decision::suggest_audio_bitrate(Some(&new_track)),
        }
    };

    // Determine playback method: if video is copy and audio is copy and container
    // is compatible, we could direct play. But switching audio mid-session always
    // needs at least a remux to produce new HLS segments with the new audio.
    let needs_audio_transcode = matches!(audio_mode, decision::AudioMode::Transcode { .. });
    let method = match session.decision.video_mode {
        decision::VideoMode::Transcode { .. } => PlaybackMethod::VideoTranscode,
        decision::VideoMode::Copy => {
            if needs_audio_transcode {
                PlaybackMethod::AudioTranscode
            } else {
                PlaybackMethod::Remux
            }
        }
    };

    // Update decision
    session.decision.audio_stream_index = audio_stream_index;
    session.decision.audio_mode = audio_mode;
    session.decision.method = method;

    // Handle burn-in subtitle change if requested
    if let Some(burn_idx) = req.burn_in_subtitle_index {
        if burn_idx < 0 {
            // Disable burn-in
            session.decision.subtitle_handling = decision::SubtitleHandling::None;
        } else {
            // Enable burn-in — forces video transcode
            session.decision.subtitle_handling = decision::SubtitleHandling::BurnIn {
                stream_index: burn_idx,
            };
            let target_codec = decision::best_transcode_codec(
                &state.encoder_caps.preferred_codecs(),
                &decision::ClientCapabilities::default(),
            );
            session.decision.video_mode = decision::VideoMode::Transcode { target_codec };
            session.decision.method = PlaybackMethod::VideoTranscode;
        }
    }

    // Reset segment cache — old segments have the wrong audio baked in.
    // Use seek_generation+1 as initial_generation to avoid segment dir collisions.
    session.seek_generation += 1;
    let new_generation = session.seek_generation;
    session.move_playlist_start(0, 0.0);

    // Reset variant caches too
    if let Some(ref mut variants) = session.variants {
        for variant in variants.values_mut() {
            variant.segment_cache.set_playback_start(0, 0.0);
        }
    }

    // Start new FFmpeg with resume position
    let base_dir = session.segment_cache.base_dir.clone();
    let start_time = req.resume_time.filter(|&t| t > 1.0);
    if let Some(st) = start_time {
        let target_segment = session.segment_cache.time_to_segment(st);
        session.move_playlist_start(target_segment, st);
        if let Some(ref mut variants) = session.variants {
            for variant in variants.values_mut() {
                variant
                    .segment_cache
                    .follow_playback_start(&session.segment_cache);
            }
        }
    }

    match start_ffmpeg_for_session(
        &session,
        base_dir,
        start_time,
        &state,
        None,
        new_generation,
        Vec::new(),
    )
    .await
    {
        Ok(process) => {
            if let Some(ref kf) = session.keyframes {
                process.set_keyframes(kf.clone());
            }
            session.transcode = Some(process);
        }
        Err(e) => {
            error!(error = %e, "Failed to start FFmpeg for audio switch");
            state.playback_sessions.insert(session_id, session);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Audio switch failed: {e}"),
            )
                .into_response();
        }
    }

    let base_url = format!("/api/v1/admin/playback/sessions/{session_id}");
    let audio_responses = build_audio_responses(&session.audio_metas, audio_stream_index);

    let method = session.decision.method;
    let (sw, sh) = session.source_resolution;
    // The burn-in request above already folded into session.decision, so read
    // the active subtitle from there rather than from the request.
    let switched_subtitle_index = session.decision.subtitle_handling.stream_index();
    let response = SessionResponse {
        session_id: session_id.to_string(),
        method,
        playlist_url: if method != PlaybackMethod::DirectPlay {
            Some(format!("{base_url}/master.m3u8"))
        } else {
            None
        },
        direct_url: if method == PlaybackMethod::DirectPlay {
            Some(format!("{base_url}/direct"))
        } else {
            None
        },
        duration_seconds: session.duration_seconds,
        subtitles: session
            .subtitle_files
            .iter()
            .map(|sf| {
                let (format, ext) = match sf.codec.as_str() {
                    "ass" | "ssa" => ("ass", "ass"),
                    _ => ("vtt", "vtt"),
                };
                SubtitleTrackResponse {
                    track_id: sf.track_id.to_string(),
                    stream_index: sf.stream_index,
                    codec: sf.codec.clone(),
                    language_name: crate::streaming::language::display_name(sf.language.as_deref()),
                    language: sf.language.clone(),
                    title: sf.title.clone(),
                    url: Some(format!("{base_url}/subtitles/{}.{ext}", sf.track_id)),
                    format: format.to_string(),
                    is_default: Some(sf.stream_index) == switched_subtitle_index,
                    is_forced: sf.is_forced,
                    is_hearing_impaired: sf.is_hearing_impaired,
                    requires_transcode: false,
                }
            })
            .collect(),
        audio_tracks: audio_responses,
        thumbnails_url: None,
        lufs: None,
        chapters: vec![],
        skip_segments: vec![],
        start_time,
        media_id: None,
        media_type: None,
        file_id: Some(session_file_id.to_string()),
        selected_audio_index: Some(audio_stream_index),
        preferred_audio_stream_index: None,
        selected_subtitle_index: switched_subtitle_index,
        video_width: if sw > 0 { Some(sw) } else { None },
        video_height: if sh > 0 { Some(sh) } else { None },
        picture: None,
    };

    info!(
        session_id = %session_id,
        audio_stream_index,
        method = ?session.decision.method,
        "Audio track switched"
    );

    state.playback_sessions.insert(session_id, session);
    Json(response).into_response()
}

// =============================================================================
// Download Subtitle to Session
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct SessionSubtitleDownloadRequest {
    pub provider: String,
    pub subtitle_id: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub is_forced: bool,
    #[serde(default)]
    pub is_hearing_impaired: bool,
}

/// Download a subtitle from a provider and attach it to an active session.
///
/// Works for all session types (local, debrid, IPTV recordings). The subtitle
/// file is saved to the session's segment directory and added to the in-memory
/// subtitle list. Returns the new subtitle track info so the player can select it.
pub async fn download_subtitle_to_session(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(session_id): Path<String>,
    Json(req): Json<SessionSubtitleDownloadRequest>,
) -> impl IntoResponse {
    let session_id = match Uuid::parse_str(&session_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid session_id").into_response(),
    };

    // Download the subtitle content first (before locking the session)
    let registry = crate::subtitle_providers::SubtitleProviderRegistry::new(state.pool.clone());
    let download = match registry.download(&req.provider, &req.subtitle_id).await {
        Ok(d) => d,
        Err(e) => {
            error!(error = %e, "Subtitle download failed");
            return (
                StatusCode::BAD_GATEWAY,
                format!("Subtitle download failed: {e}"),
            )
                .into_response();
        }
    };

    // Now lock the session to save the file and update subtitle_files
    let mut session = match state.playback_sessions.get_mut(&session_id) {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, "session not found").into_response(),
    };
    session.touch();

    let track_id = Uuid::new_v4();
    let sub_dir = session.segment_dir().to_path_buf();
    let _ = tokio::fs::create_dir_all(&sub_dir).await;

    // Determine format and file extension
    let (codec, ext) = match download.format.as_str() {
        "ass" | "ssa" => ("ass", "ass"),
        "sub" => ("subrip", "srt"),
        _ => ("subrip", "vtt"),
    };

    let sub_filename = format!("{track_id}.{ext}");
    let sub_path = sub_dir.join(&sub_filename);

    // For SRT, convert to VTT
    let content = if download.format == "srt" && ext == "vtt" {
        let srt_text = String::from_utf8_lossy(&download.content);
        crate::streaming::subtitle::srt_to_vtt(&srt_text).into_bytes()
    } else {
        download.content
    };

    if let Err(e) = tokio::fs::write(&sub_path, &content).await {
        error!(error = %e, "Failed to write subtitle file");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to save subtitle: {e}"),
        )
            .into_response();
    }

    // Use a stream_index that won't collide with embedded tracks
    // (external subs use negative indices as a convention)
    let stream_index = -(session.subtitle_files.len() as i32 + 1);

    let sub_file = SubtitleFile {
        track_id,
        stream_index,
        codec: codec.to_string(),
        language: req.language.clone(),
        title: Some(format!(
            "{}{}",
            req.language.as_deref().unwrap_or("External"),
            if req.is_forced { " (Forced)" } else { "" }
        )),
        file_path: sub_path,
        extracted: true,
        is_forced: req.is_forced,
        is_hearing_impaired: false,
    };

    session.subtitle_files.push(sub_file);

    let base_url = format!("/api/v1/admin/playback/sessions/{session_id}");
    let response = SubtitleTrackResponse {
        track_id: track_id.to_string(),
        stream_index,
        codec: codec.to_string(),
        language_name: crate::streaming::language::display_name(req.language.as_deref()),
        language: req.language,
        title: Some("Downloaded subtitle".to_string()),
        url: Some(format!("{base_url}/subtitles/{track_id}.{ext}")),
        format: ext.to_string(),
        is_default: false,
        is_forced: req.is_forced,
        is_hearing_impaired: false,
        requires_transcode: false,
    };

    info!(session_id = %session_id, track_id = %track_id, "Subtitle downloaded to session");
    Json(response).into_response()
}

/// Build audio track responses with `is_default` set for the active track.
pub fn build_audio_responses(
    audio_metas: &[AudioTrackMeta],
    active_stream_index: i32,
) -> Vec<AudioTrackResponse> {
    audio_metas
        .iter()
        .map(|a| AudioTrackResponse {
            stream_index: a.stream_index,
            codec: a.codec.clone(),
            channels: a.channels,
            language_name: crate::streaming::language::display_name(a.language.as_deref()),
            language: a.language.clone(),
            title: a.title.clone(),
            is_default: a.stream_index == active_stream_index,
        })
        .collect()
}

/// Start FFmpeg with the session's transcode settings.
///
/// When `variant_spec` is provided, the output is scaled and bitrate-limited
/// for an ABR lower-quality variant. `None` means primary (highest quality).
/// Preroll window: start an overlay encode this many seconds before the
/// playhead reaches a corrupt range. Generous default — the encoder is much
/// faster than realtime, but giving it slack means the overlay's segments
/// always land before the player asks for them, even on weaker boxes.
const OVERLAY_PREROLL_SECS: f64 = 60.0;

/// Lazy overlay scheduler: spawn `start_overlay` for any corrupt range whose
/// start is within `OVERLAY_PREROLL_SECS` of the current playback position
/// and that hasn't been spawned yet this session. Idempotent — re-callable
/// on every playlist request without re-spawning.
///
/// IPTV (`is_live`) and pre-probe debrid sessions are skipped because
/// `start_overlay` requires a reopenable file path. Tier-3 full-restart
/// covers those instead.
fn maybe_spawn_overlays(session: &mut PlaybackSession, state: &AppState) {
    if session.corrupt_ranges.is_empty() {
        return;
    }
    if session.is_live || session.file_path.as_os_str().is_empty() {
        return;
    }
    let Some(transcode) = session.transcode.as_ref() else {
        return;
    };
    // Effective playhead: max(playlist start, last segment the player asked for).
    // `playback_start_segment` only updates on seek, so during continuous
    // playback `highest_served_segment` is the only signal that advances.
    let playhead_segment = session
        .highest_served_segment
        .unwrap_or(session.segment_cache.playback_start_segment)
        .max(session.segment_cache.playback_start_segment);
    let playhead_secs = session.segment_cache.segment_to_time(playhead_segment);
    let preroll_horizon = playhead_secs + OVERLAY_PREROLL_SECS;

    let target_codec = match &session.decision.video_mode {
        decision::VideoMode::Transcode { target_codec } => target_codec.clone(),
        decision::VideoMode::Copy => decision::best_transcode_codec(
            &state.encoder_caps.preferred_codecs(),
            &decision::ClientCapabilities::default(),
        ),
    };
    let encoder = state
        .encoder_caps
        .encoder_for(&target_codec)
        .unwrap_or(&state.encoder_caps.h264)
        .to_string();

    let to_spawn: Vec<crate::corruption::CorruptRange> = session
        .corrupt_ranges
        .iter()
        .filter(|r| r.has_decoder_signal())
        .filter(|r| {
            let key = r.start_secs.to_bits();
            !session.overlay_spawned_starts.contains(&key) && r.start_secs <= preroll_horizon
        })
        .cloned()
        .collect();

    let hw_device = state.hw_device.get();
    let (audio_encoder, audio_bitrate) = match &session.decision.audio_mode {
        decision::AudioMode::Copy => (None, None),
        decision::AudioMode::Transcode {
            target_codec,
            bitrate_kbps,
        } => (
            Some(target_codec.clone()),
            Some(i64::from(*bitrate_kbps) * 1000),
        ),
    };
    for range in to_spawn {
        let key = range.start_secs.to_bits();
        match transcode.start_overlay(
            range.start_secs..range.end_secs,
            kob_ffmpeg::session::OverlayConfig {
                video_encoder: encoder.clone(),
                video_bitrate: None,
                audio_encoder: audio_encoder.clone(),
                audio_bitrate,
                hw_device: hw_device.clone(),
            },
        ) {
            Ok(overlay_gen) => {
                info!(
                    session_id = %session.id,
                    range_start = range.start_secs,
                    range_end = range.end_secs,
                    overlay_generation = overlay_gen,
                    "spawned overlay for corrupt range"
                );
                session.overlay_spawned_starts.insert(key);
            }
            Err(e) => {
                warn!(
                    session_id = %session.id,
                    range_start = range.start_secs,
                    error = %e,
                    "start_overlay failed — playback will rely on Tier-3 fallback if user reaches this range"
                );
                // Mark anyway so we don't loop spawning a failing overlay every playlist request.
                session.overlay_spawned_starts.insert(key);
            }
        }
    }
}

async fn start_ffmpeg_for_session(
    session: &PlaybackSession,
    output_dir: PathBuf,
    start_time: Option<f64>,
    state: &AppState,
    variant_spec: Option<&VariantSpec>,
    initial_generation: u32,
    prior_runs: Vec<kob_ffmpeg::session::PriorRun>,
) -> Result<transcode::TranscodeProcess, transcode::TranscodeError> {
    if session.file_path.as_os_str().is_empty() {
        return Err(transcode::TranscodeError::StartFailed(
            "file_path is empty — probe still in progress".to_string(),
        ));
    }

    // Variants always need actual transcoding (for scaling), even if the
    // primary stream copies. Pick the best encoder for the variant.
    let (video_mode, target_encoder) = if variant_spec.is_some() {
        let target_codec = match &session.decision.video_mode {
            decision::VideoMode::Transcode { target_codec } => target_codec.clone(),
            decision::VideoMode::Copy => {
                // Primary is copy but variant needs encoding — pick best codec
                decision::best_transcode_codec(
                    &state.encoder_caps.preferred_codecs(),
                    // Use conservative H.264-only caps for variants to maximize compat
                    &decision::ClientCapabilities::default(),
                )
            }
        };
        let encoder = state
            .encoder_caps
            .encoder_for(&target_codec)
            .unwrap_or(&state.encoder_caps.h264)
            .to_string();
        (VideoMode::Transcode, encoder)
    } else if session.force_video_transcode {
        // Tier-3 / corrupt_ranges override: ignore the decision engine's
        // Copy choice and emit a clean re-encoded H.264 stream so browser
        // HW decoders don't trip on source bitstream errors.
        let target_codec = match &session.decision.video_mode {
            decision::VideoMode::Transcode { target_codec } => target_codec.clone(),
            decision::VideoMode::Copy => decision::best_transcode_codec(
                &state.encoder_caps.preferred_codecs(),
                &decision::ClientCapabilities::default(),
            ),
        };
        let encoder = state
            .encoder_caps
            .encoder_for(&target_codec)
            .unwrap_or(&state.encoder_caps.h264)
            .to_string();
        (VideoMode::Transcode, encoder)
    } else {
        match &session.decision.video_mode {
            decision::VideoMode::Copy => (VideoMode::Copy, state.encoder_caps.h264.clone()),
            decision::VideoMode::Transcode { target_codec } => {
                let encoder = state
                    .encoder_caps
                    .encoder_for(target_codec)
                    .unwrap_or(&state.encoder_caps.h264)
                    .to_string();
                (VideoMode::Transcode, encoder)
            }
        }
    };
    let audio_mode = match &session.decision.audio_mode {
        decision::AudioMode::Copy => AudioMode::Copy,
        decision::AudioMode::Transcode {
            target_codec,
            bitrate_kbps,
        } => AudioMode::Transcode {
            target_codec: target_codec.clone(),
            bitrate_kbps: *bitrate_kbps,
        },
    };

    let burn_in_subtitle = match &session.decision.subtitle_handling {
        decision::SubtitleHandling::BurnIn { stream_index } => Some(*stream_index),
        _ => None,
    };

    let (scale, target_bitrate_kbps) = match variant_spec {
        Some(spec) => (Some((spec.width, spec.height)), spec.target_bitrate_kbps),
        // Primary stream: apply session's max_bitrate_kbps if set
        None => (
            primary_scale(session.source_resolution, session.max_height),
            session.max_bitrate_kbps,
        ),
    };

    let args = TranscodeArgs {
        input_path: session.file_path.clone(),
        output_dir,
        video_mode,
        audio_mode,
        audio_stream_index: session.decision.audio_stream_index,
        start_time,
        segment_duration: 6,
        hw_accel: session.hw_accel.clone(),
        video_codec: session.video_codec.clone(),
        target_encoder,
        burn_in_subtitle,
        scale,
        target_bitrate_kbps,
        duration_seconds: session.duration_seconds,
        initial_generation,
        hw_device: state.hw_device.get(),
        prior_runs,
        deinterlace: session.decision.deinterlace,
        tone_map_hdr: session.decision.tone_map_hdr,
    };

    transcode::TranscodeProcess::start(args).await
}

fn primary_scale((w, h): (u32, u32), max_height: Option<u32>) -> Option<(u32, u32)> {
    let out = variant::output_resolution(w, h, max_height);
    (out != (w, h)).then_some(out)
}

fn variant_needs_start(
    state: &AppState,
    session_id: Uuid,
    variant_id: &str,
) -> Result<bool, axum::response::Response> {
    let mut session = state
        .playback_sessions
        .get_mut(&session_id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "session not found").into_response())?;
    let variant = session
        .variants
        .as_mut()
        .and_then(|v| v.get_mut(variant_id))
        .ok_or_else(|| (StatusCode::NOT_FOUND, "variant not found").into_response())?;
    Ok(variant.transcode.as_mut().is_none_or(|t| !t.is_running()))
}

/// Lazy-start FFmpeg for an ABR variant if not already running.
///
/// Called on the first `stream.m3u8?v=variant_id` request for a variant.
/// Creates the variant's output directory and starts FFmpeg with the
/// appropriate scale and bitrate settings.
async fn lazy_start_variant(
    state: &AppState,
    session_id: Uuid,
    variant_id: &str,
) -> Result<(), axum::response::Response> {
    if !variant_needs_start(state, session_id, variant_id)? {
        return Ok(());
    }
    let _gate = state.session_gates.claim(session_id).await;
    if !variant_needs_start(state, session_id, variant_id)? {
        return Ok(());
    }

    // Remove session from map for async FFmpeg start
    let (_, mut session) = match state.playback_sessions.remove(&session_id) {
        Some(pair) => pair,
        None => return Err((StatusCode::NOT_FOUND, "session not found").into_response()),
    };

    if session.file_path.as_os_str().is_empty() {
        state.playback_sessions.insert(session_id, session);
        return Err(session_pending_response());
    }

    // Extract variant info needed for FFmpeg start
    let start = {
        let variant = match session
            .variants
            .as_mut()
            .and_then(|v| v.get_mut(variant_id))
        {
            Some(v) => v,
            None => {
                state.playback_sessions.insert(session_id, session);
                return Err((StatusCode::NOT_FOUND, "variant not found").into_response());
            }
        };

        let start_time_secs = variant.segment_cache.playback_start_secs;
        let start_time = if start_time_secs > 0.0 {
            Some(start_time_secs)
        } else {
            None
        };

        // The TranscodeSession registers the initial run internally on start
        let base_dir = variant.segment_cache.base_dir.clone();
        let spec = variant.spec.clone();
        let prior_runs = std::mem::take(&mut variant.prior_runs);

        (start_time, base_dir, spec, prior_runs)
    };
    let (start_time, base_dir, spec, prior_runs) = start;
    let generation = prior_runs
        .iter()
        .map(|r| r.generation + 1)
        .fold(session.seek_generation, u32::max);

    match start_ffmpeg_for_session(
        &session,
        base_dir,
        start_time,
        state,
        Some(&spec),
        generation,
        prior_runs,
    )
    .await
    {
        Ok(process) => {
            if let Some(ref kf) = session.keyframes {
                process.set_keyframes(kf.clone());
            }
            let variant = session
                .variants
                .as_mut()
                .and_then(|v| v.get_mut(variant_id))
                .unwrap();
            variant.transcode = Some(process);
            info!(
                session_id = %session_id,
                variant = variant_id,
                "Lazy-started FFmpeg for ABR variant"
            );
        }
        Err(e) => {
            error!(error = %e, variant = variant_id, "Failed to start FFmpeg for variant");
            state.playback_sessions.insert(session_id, session);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to start variant transcode: {e}"),
            )
                .into_response());
        }
    }

    state.playback_sessions.insert(session_id, session);
    Ok(())
}

// =============================================================================
// Direct Play (Range Request Support)
// =============================================================================

pub async fn direct_play(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let session_id = match Uuid::parse_str(&session_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid session_id").into_response(),
    };

    let mut session = match state.playback_sessions.get_mut(&session_id) {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, "session not found").into_response(),
    };
    session.touch();

    if session.file_path.as_os_str().is_empty() {
        return session_pending_response();
    }

    let file_path = session.file_path.clone();
    let container = session.container.clone();
    // Extracted sidecars mean the client never selects an embedded subtitle track,
    // so none of them need to keep their cue points.
    let sidecars_available = !session.subtitle_files.is_empty();
    drop(session);

    let file = match tokio::fs::File::open(&file_path).await {
        Ok(f) => f,
        Err(e) => {
            error!(error = %e, path = %file_path.display(), "Failed to open file for direct play");
            return (StatusCode::NOT_FOUND, "file not found").into_response();
        }
    };

    let file_size = match file.metadata().await {
        Ok(m) => m.len(),
        Err(e) => {
            error!(error = %e, "Failed to get file metadata");
            return (StatusCode::INTERNAL_SERVER_ERROR, "metadata error").into_response();
        }
    };

    let content_type = match container.as_str() {
        "mp4" | "mov,mp4,m4a,3gp,3g2,mj2" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "matroska,webm" | "matroska" | "mkv" => "video/x-matroska",
        _ => "video/mp4",
    };

    let rt = state.runtime_config.get().await;
    let patch = patch_for_local_file(
        &state.cue_trim_cache,
        &file_path,
        &container,
        &CueTrimPolicy {
            sidecars_available,
            preferred_languages: vec![rt.preferred_subtitle_language.clone()],
        },
        rt.mkv_cue_trim_enabled,
    )
    .await;

    // Parse Range header
    if let Some(range_header) = headers.get(header::RANGE)
        && let Ok(range_str) = range_header.to_str()
        && let Some(range) = parse_byte_range(range_str, file_size)
    {
        return serve_range(file, file_size, content_type, range, patch).await;
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

/// Serve a byte range from a file.
async fn serve_range(
    mut file: tokio::fs::File,
    file_size: u64,
    content_type: &str,
    range: ByteRange,
    patch: Option<Arc<IndexPatch>>,
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
    };

    // The demuxer asks for the cue index as one range; answering it from the patch
    // skips reading bytes that would be overwritten in full anyway.
    if let Some(p) = patch.as_ref()
        && let Some(bytes) = p.whole_range(range.start, length as usize)
    {
        return partial(Body::from(bytes.to_vec())).into_response();
    }

    if let Err(e) = file.seek(std::io::SeekFrom::Start(range.start)).await {
        error!(error = %e, "Failed to seek in file");
        return (StatusCode::INTERNAL_SERVER_ERROR, "seek error").into_response();
    }

    let limited = file.take(length);
    let stream = ReaderStream::new(limited);
    let body = match patch {
        Some(p) => Body::from_stream(patched_stream(stream, range.start, p)),
        None => Body::from_stream(stream),
    };

    partial(body).into_response()
}

// =============================================================================
// Subtitle Serving
// =============================================================================

pub async fn serve_subtitle(
    State(state): State<AppState>,
    Path((session_id, track_filename)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<SubtitleQuery>,
) -> impl IntoResponse {
    let session_id = match Uuid::parse_str(&session_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid session_id").into_response(),
    };

    let mut session = match state.playback_sessions.get_mut(&session_id) {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, "session not found").into_response(),
    };
    session.touch();

    // Parse track_id from filename (e.g., "uuid.vtt" or "uuid.ass")
    let track_id_str = track_filename
        .rsplit_once('.')
        .map(|(id, _)| id)
        .unwrap_or(&track_filename);

    let track_id = match Uuid::parse_str(track_id_str) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid track_id").into_response(),
    };

    let (sub_file, needs_extract, file_id) = match session
        .subtitle_files
        .iter()
        .find(|sf| sf.track_id == track_id)
    {
        Some(sf) => (sf.clone(), !sf.extracted, session.file_id),
        None => return (StatusCode::NOT_FOUND, "subtitle track not found").into_response(),
    };

    if session.file_path.as_os_str().is_empty() {
        return session_pending_response();
    }

    let input_path = session.file_path.clone();
    drop(session);

    // Extract on-demand if not yet extracted — write to persistent cache so
    // future sessions find it pre-extracted.
    let serve_path = if needs_extract {
        let cache_path = subtitle::cache_path(
            &state.config.storage_root,
            file_id,
            sub_file.stream_index,
            &sub_file.codec,
        );

        if let Some(parent) = cache_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        if let Err(e) =
            transcode::extract_subtitle(&input_path, sub_file.stream_index, &cache_path).await
        {
            error!(
                stream_index = sub_file.stream_index,
                error = %e,
                "Failed to extract subtitle on demand"
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "subtitle extraction failed",
            )
                .into_response();
        }

        // Update session: point to cache path and mark extracted
        if let Some(mut session) = state.playback_sessions.get_mut(&session_id)
            && let Some(sf) = session
                .subtitle_files
                .iter_mut()
                .find(|sf| sf.track_id == track_id)
        {
            sf.file_path = cache_path.clone();
            sf.extracted = true;
        }

        cache_path
    } else {
        sub_file.file_path.clone()
    };

    // Determine if we need VTT conversion
    let requested_vtt = track_filename.ends_with(".vtt");
    let is_srt = matches!(sub_file.codec.as_str(), "srt" | "subrip" | "mov_text");
    let is_vtt = matches!(sub_file.codec.as_str(), "webvtt" | "vtt");

    if requested_vtt && (is_srt || is_vtt) {
        match tokio::fs::read_to_string(&serve_path).await {
            Ok(content) => {
                let vtt = if is_srt {
                    subtitle::srt_to_vtt(&content)
                } else {
                    content
                };
                let vtt_content = subtitle::shift_vtt(&vtt, query.offset.unwrap_or(0.0));
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/vtt; charset=utf-8")
                    .header(header::CACHE_CONTROL, "max-age=3600")
                    .body(Body::from(vtt_content))
                    .unwrap()
                    .into_response()
            }
            Err(e) => {
                error!(error = %e, "Failed to read subtitle file");
                (StatusCode::INTERNAL_SERVER_ERROR, "failed to read subtitle").into_response()
            }
        }
    } else {
        // Serve raw subtitle file (ASS, or SRT without conversion)
        let content_type = match sub_file.codec.as_str() {
            "ass" | "ssa" => "text/x-ssa; charset=utf-8",
            "srt" => "application/x-subrip; charset=utf-8",
            _ => "text/plain; charset=utf-8",
        };

        match tokio::fs::File::open(&serve_path).await {
            Ok(file) => {
                let stream = ReaderStream::new(file);
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, content_type)
                    .header(header::CACHE_CONTROL, "max-age=3600")
                    .body(Body::from_stream(stream))
                    .unwrap()
                    .into_response()
            }
            Err(e) => {
                error!(error = %e, "Failed to open subtitle file");
                (StatusCode::INTERNAL_SERVER_ERROR, "failed to read subtitle").into_response()
            }
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Enqueue a background session cleanup job via NATS.
async fn enqueue_session_cleanup(state: &AppState, dir: &std::path::Path) {
    let payload = SessionCleanupPayload {
        dir: dir.to_string_lossy().to_string(),
    };
    let context = crate::jobs::JobContext::new("Cleaning up playback session");
    if let Err(e) = crate::jobs::enqueue_routine(
        &state.pool,
        state.nats.publisher(),
        "session_cleanup",
        serde_json::to_value(&payload).unwrap(),
        context,
    )
    .await
    {
        warn!(error = %e, dir = %dir.display(), "Failed to enqueue session cleanup, removing directly");
        let _ = tokio::fs::remove_dir_all(dir).await;
    }
}

/// The file behind [file_id] as a transcode needs it: its path on disk and its stream
/// metadata, probed inline when nothing has recorded it yet.
///
/// [field_order_needed] probes a file whose field order is unrecorded, for a viewer that
/// cannot show interlaced video.
pub(crate) async fn load_playable_file(
    state: &AppState,
    media_type: MediaType,
    file_id: Uuid,
    field_order_needed: bool,
) -> Result<(PathBuf, StoredFileMeta), Response<Body>> {
    let stored = match media_type {
        MediaType::Movie => load_movie_file_meta(state, file_id).await?,
        MediaType::Episode => load_episode_file_meta(state, file_id).await?,
        MediaType::Recording => load_recording_file_meta(state, file_id).await?,
    };

    let full_path =
        crate::audio::path::resolve_file_path(&state.config.storage_root, &stored.file_path);
    if !full_path.exists() {
        return Err((StatusCode::NOT_FOUND, "file not found on disk").into_response());
    }

    // If the file hasn't been probed yet (no video_codec in DB), or its cached
    // duration is missing/zero (e.g. just content-replaced at the same path, so
    // create_or_update reset duration → reprobe pending), probe it now so the
    // decision engine + SegmentCache have real data instead of a 0-length
    // session (which would map every seek to segment 0 and instant-ENDLIST).
    // An unrecorded field order only matters to a client that has said it cannot show fields.
    let interlace_unknown = stored.interlaced.is_none() && field_order_needed;
    let meta = if stored.video_codec.is_empty()
        || stored.container.is_empty()
        || stored.duration <= 0.0
        || interlace_unknown
    {
        match crate::ffmpeg::probe::probe(&full_path).await {
            Ok(probe) => {
                let probed_container = probe.format.format_name.clone();
                let probed_video_codec = probe
                    .video_streams
                    .first()
                    .map(|v| v.codec.clone())
                    .unwrap_or_default();
                let probed_duration = probe.format.duration_seconds.unwrap_or(stored.duration);
                let probed_audio: Vec<AudioTrackMeta> = probe
                    .audio_streams
                    .iter()
                    .map(|a| AudioTrackMeta {
                        stream_index: a.index,
                        codec: a.codec.clone(),
                        channels: a.channels,
                        language: a.language.clone(),
                        title: a.title.clone(),
                        is_default: a.is_default,
                    })
                    .collect();
                let probed_subs: Vec<SubtitleTrackMeta> = probe
                    .subtitle_streams
                    .iter()
                    .map(|s| SubtitleTrackMeta {
                        stream_index: s.index,
                        codec: s.codec.clone(),
                        language: s.language.clone(),
                        title: s.title.clone(),
                        is_default: s.is_default,
                        is_forced: s.is_forced,
                        is_hearing_impaired: s.is_hearing_impaired,
                    })
                    .collect();
                info!(
                    video_codec = %probed_video_codec,
                    container = %probed_container,
                    audio_tracks = probed_audio.len(),
                    duration = probed_duration,
                    "Inline probe for unprobed file"
                );

                // Save probe results back to DB (video metadata + audio/subtitle tracks)
                // so future sessions skip the probe and have correct track data.
                // Persist probe results for movie/episode (recordings store
                // metadata via update_media_metadata in the record job).
                if matches!(media_type, MediaType::Movie | MediaType::Episode)
                    && let Err(e) = crate::ffmpeg::persist::persist_probe_result(
                        &state.pool,
                        file_id,
                        media_type.as_str(),
                        &probe,
                    )
                    .await
                {
                    warn!(error = %e, "Failed to save inline probe results to DB");
                }

                let (probed_width, probed_height) = probe
                    .video_streams
                    .first()
                    .map(|v| (v.width as u32, v.height as u32))
                    .unwrap_or((0, 0));

                StoredFileMeta {
                    file_path: stored.file_path,
                    container: probed_container,
                    video_codec: probed_video_codec,
                    duration: probed_duration,
                    audio_metas: probed_audio,
                    sub_metas: probed_subs,
                    keyframes: None,
                    video_width: probed_width,
                    video_height: probed_height,
                    interlaced: Some(probe.video_streams.first().is_some_and(|v| v.is_interlaced)),
                    dynamic_range: probe
                        .video_streams
                        .first()
                        .map(decision::probed_dynamic_range)
                        .unwrap_or_default(),
                    overall_bitrate_kbps: probe
                        .format
                        .bit_rate_kbps
                        .and_then(|k| u32::try_from(k).ok()),
                }
            }
            Err(e) => {
                warn!(error = %e, "Inline probe failed, using DB metadata");
                stored
            }
        }
    } else {
        stored
    };
    Ok((full_path, meta))
}

pub(crate) struct StoredFileMeta {
    pub(crate) file_path: String,
    pub(crate) container: String,
    pub(crate) video_codec: String,
    pub(crate) duration: f64,
    pub(crate) audio_metas: Vec<AudioTrackMeta>,
    pub(crate) sub_metas: Vec<SubtitleTrackMeta>,
    pub(crate) keyframes: Option<Vec<f64>>,
    pub(crate) video_width: u32,
    pub(crate) video_height: u32,
    /// `None` where nothing has recorded it, so a probe has to say.
    pub(crate) interlaced: Option<bool>,
    pub(crate) dynamic_range: decision::DynamicRange,
    pub(crate) overall_bitrate_kbps: Option<u32>,
}

/// The file a create-session request names, directly or by media id.
///
/// A client that has just been handed a movie or an episode knows the media it wants and
/// nothing about the files behind it, and making it go and ask put a whole round trip
/// between the tap and the first byte. The choice is the server's anyway — the newest
/// available file — so it is made here rather than mirrored in every client.
async fn kids_may_play(pool: &sqlx::PgPool, req: &CreateSessionRequest) -> bool {
    let parse = |id: Option<&str>| id.and_then(|id| Uuid::parse_str(id).ok());
    let allowed = match (
        req.media_type,
        parse(req.file_id.as_deref()),
        parse(req.media_id.as_deref()),
    ) {
        (MediaType::Movie, Some(file_id), _) => {
            crate::kids::movie_file_allowed(pool, file_id).await
        }
        (MediaType::Episode, Some(file_id), _) => {
            crate::kids::episode_file_allowed(pool, file_id).await
        }
        (MediaType::Movie, None, Some(movie_id)) => {
            crate::kids::movie_allowed(pool, movie_id).await
        }
        (MediaType::Episode, None, Some(episode_id)) => {
            crate::kids::episode_allowed(pool, episode_id).await
        }
        _ => Ok(false),
    };
    allowed.unwrap_or(false)
}

async fn resolve_session_file_id(
    state: &AppState,
    req: &CreateSessionRequest,
) -> Result<Uuid, axum::response::Response> {
    if let Some(file_id) = req.file_id.as_deref() {
        return Uuid::parse_str(file_id)
            .map_err(|_| (StatusCode::BAD_REQUEST, "invalid file_id").into_response());
    }

    let media_id = req.media_id.as_deref().ok_or_else(|| {
        (StatusCode::BAD_REQUEST, "file_id or media_id is required").into_response()
    })?;
    let media_uuid = Uuid::parse_str(media_id)
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid media_id").into_response())?;
    preferred_file_id(state, req.media_type, media_uuid).await
}

/// The newest available file of a movie or episode — the one playback picks.
pub(crate) async fn preferred_file_id(
    state: &AppState,
    media_type: MediaType,
    media_uuid: Uuid,
) -> Result<Uuid, axum::response::Response> {
    let file_id = match media_type {
        MediaType::Movie => movie_files::get_preferred_available(&state.pool, media_uuid)
            .await
            .map_err(|e| {
                error!(error = %e, "DB error resolving movie file");
                (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
            })?
            .map(|f| f.id),
        MediaType::Episode => tv_episode_files::get_preferred_available(&state.pool, media_uuid)
            .await
            .map_err(|e| {
                error!(error = %e, "DB error resolving episode file");
                (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
            })?
            .map(|f| f.id),
        // A recording is reached through its own playback route, which already knows
        // the file. Nothing asks for one by media id.
        MediaType::Recording => None,
    };

    file_id.ok_or_else(|| (StatusCode::NOT_FOUND, "no available file for media").into_response())
}

async fn load_movie_file_meta(
    state: &AppState,
    file_id: Uuid,
) -> Result<StoredFileMeta, axum::response::Response> {
    let file = movie_files::get(&state.pool, file_id)
        .await
        .map_err(|e| {
            error!(error = %e, "DB error loading movie file");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        })?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "movie file not found").into_response())?;

    let audio = audio_tracks::list_by_movie_file(&state.pool, file_id)
        .await
        .unwrap_or_default();
    let subs = subtitle_tracks::list_by_movie_file(&state.pool, file_id)
        .await
        .unwrap_or_default();

    let audio_metas: Vec<AudioTrackMeta> = audio
        .iter()
        .map(|a| AudioTrackMeta {
            stream_index: a.track_index,
            codec: a.codec.clone(),
            channels: i32::from(a.channels),
            language: a.language.clone(),
            title: a.title.clone(),
            is_default: a.is_default,
        })
        .collect();

    let sub_metas: Vec<SubtitleTrackMeta> = subs
        .iter()
        .map(|s| SubtitleTrackMeta {
            stream_index: s.track_index,
            codec: s.codec.clone(),
            language: s.language.clone(),
            title: s.title.clone(),
            is_default: s.is_default,
            is_forced: s.is_forced,
            is_hearing_impaired: s.is_hearing_impaired,
        })
        .collect();

    let keyframes: Option<Vec<f64>> = file.keyframes.and_then(|v| serde_json::from_value(v).ok());

    Ok(StoredFileMeta {
        file_path: file.file_path,
        container: file.container.unwrap_or_default(),
        video_codec: file.video_codec.unwrap_or_default(),
        duration: file.duration_seconds.unwrap_or(0.0),
        audio_metas,
        sub_metas,
        keyframes,
        video_width: file.video_width.unwrap_or(0) as u32,
        video_height: file.video_height.unwrap_or(0) as u32,
        interlaced: Some(file.is_interlaced),
        dynamic_range: decision::dynamic_range(
            file.is_hdr,
            file.hdr_format.as_deref(),
            file.dv_bl_compat_id,
        ),
        overall_bitrate_kbps: file
            .overall_bitrate_kbps
            .and_then(|k| u32::try_from(k).ok()),
    })
}

async fn load_episode_file_meta(
    state: &AppState,
    file_id: Uuid,
) -> Result<StoredFileMeta, axum::response::Response> {
    let file = tv_episode_files::get(&state.pool, file_id)
        .await
        .map_err(|e| {
            error!(error = %e, "DB error loading episode file");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        })?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "episode file not found").into_response())?;

    let audio = audio_tracks::list_by_episode_file(&state.pool, file_id)
        .await
        .unwrap_or_default();
    let subs = subtitle_tracks::list_by_episode_file(&state.pool, file_id)
        .await
        .unwrap_or_default();

    let audio_metas: Vec<AudioTrackMeta> = audio
        .iter()
        .map(|a| AudioTrackMeta {
            stream_index: a.track_index,
            codec: a.codec.clone(),
            channels: i32::from(a.channels),
            language: a.language.clone(),
            title: a.title.clone(),
            is_default: a.is_default,
        })
        .collect();

    let sub_metas: Vec<SubtitleTrackMeta> = subs
        .iter()
        .map(|s| SubtitleTrackMeta {
            stream_index: s.track_index,
            codec: s.codec.clone(),
            language: s.language.clone(),
            title: s.title.clone(),
            is_default: s.is_default,
            is_forced: s.is_forced,
            is_hearing_impaired: s.is_hearing_impaired,
        })
        .collect();

    let keyframes: Option<Vec<f64>> = file.keyframes.and_then(|v| serde_json::from_value(v).ok());

    Ok(StoredFileMeta {
        file_path: file.file_path,
        container: file.container.unwrap_or_default(),
        video_codec: file.video_codec.unwrap_or_default(),
        duration: file.duration_seconds.unwrap_or(0.0),
        audio_metas,
        sub_metas,
        keyframes,
        video_width: file.video_width.unwrap_or(0) as u32,
        video_height: file.video_height.unwrap_or(0) as u32,
        interlaced: Some(file.is_interlaced),
        dynamic_range: decision::dynamic_range(
            file.is_hdr,
            file.hdr_format.as_deref(),
            file.dv_bl_compat_id,
        ),
        overall_bitrate_kbps: file
            .overall_bitrate_kbps
            .and_then(|k| u32::try_from(k).ok()),
    })
}

async fn load_recording_file_meta(
    state: &AppState,
    file_id: Uuid,
) -> Result<StoredFileMeta, axum::response::Response> {
    let rec = iptv_recordings::get_by_id(&state.pool, file_id)
        .await
        .map_err(|e| {
            error!(error = %e, "DB error loading recording");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        })?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "recording not found").into_response())?;

    if rec.status != "completed" {
        return Err((StatusCode::BAD_REQUEST, "recording not yet completed").into_response());
    }

    let file_path = rec
        .file_path
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "recording has no file").into_response())?;

    // Recordings store absolute paths and have no separate audio/subtitle track
    // tables — the inline probe fallback in create_session handles discovery.
    Ok(StoredFileMeta {
        file_path,
        container: rec.container.unwrap_or_default(),
        video_codec: rec.video_codec.unwrap_or_default(),
        duration: rec.duration_seconds.unwrap_or(0.0),
        audio_metas: vec![],
        sub_metas: vec![],
        keyframes: None,
        video_width: 0,
        video_height: 0,
        interlaced: None,
        dynamic_range: decision::DynamicRange::Sdr,
        overall_bitrate_kbps: None,
    })
}

/// Build subtitle metadata for a session.
///
/// Checks the persistent subtitle cache for pre-extracted files (from the
/// media_probe job). If a cached file exists, uses it directly with
/// `extracted: true`. Otherwise, falls back to the session temp dir with
/// `extracted: false` — extraction happens lazily on first request in
/// `serve_subtitle`.
fn build_subtitle_metadata(
    sub_metas: &[SubtitleTrackMeta],
    segment_dir: &std::path::Path,
    storage_root: &str,
    file_id: Uuid,
) -> Vec<SubtitleFile> {
    sub_metas
        .iter()
        .filter(|sub| !subtitle::is_bitmap_subtitle(&sub.codec))
        .map(|sub| {
            let track_id = Uuid::new_v4();
            // Check if the media_probe job already extracted this track
            let cached = subtitle::cache_path(storage_root, file_id, sub.stream_index, &sub.codec);
            let (file_path, extracted) = if cached.exists() {
                (cached, true)
            } else {
                let ext = subtitle::subtitle_extension(&sub.codec).unwrap_or("srt");
                (segment_dir.join(format!("{track_id}.{ext}")), false)
            };
            SubtitleFile {
                track_id,
                stream_index: sub.stream_index,
                codec: sub.codec.clone(),
                language: sub.language.clone(),
                title: sub.title.clone(),
                file_path,
                extracted,
                is_forced: sub.is_forced,
                is_hearing_impaired: sub.is_hearing_impaired,
            }
        })
        .collect()
}

/// 410 rather than 404: a 404 here means "not generated yet", which a player keeps
/// polling for. A session that no longer exists will never produce any.
fn session_gone() -> Response<Body> {
    (StatusCode::GONE, "Session not found").into_response()
}

/// Serve the WebVTT thumbnails file for a session.
pub async fn serve_thumbnails_vtt(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
) -> impl IntoResponse {
    let session = match state.playback_sessions.get(&session_id) {
        Some(s) => s,
        None => return session_gone(),
    };
    let thumb_dir = match session.thumbnail_dir {
        Some(ref dir) => dir.clone(),
        None => PathBuf::from(&state.config.storage_root)
            .join("thumbnails")
            .join(session.file_id.to_string()),
    };
    drop(session);

    let vtt_path = thumb_dir.join("thumbnails.vtt");

    match tokio::fs::read_to_string(&vtt_path).await {
        Ok(content) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/vtt"),
                (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            ],
            content,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "Thumbnails not generated yet").into_response(),
    }
}

/// Serve a thumbnail sprite image for a session.
pub async fn serve_thumbnail_sprite(
    State(state): State<AppState>,
    Path((session_id, filename)): Path<(Uuid, String)>,
) -> impl IntoResponse {
    // Sanitize filename
    if filename.contains("..") || filename.contains('/') {
        return (StatusCode::BAD_REQUEST, "Invalid filename").into_response();
    }

    let session = match state.playback_sessions.get(&session_id) {
        Some(s) => s,
        None => return session_gone(),
    };
    let thumb_dir = match session.thumbnail_dir {
        Some(ref dir) => dir.clone(),
        None => PathBuf::from(&state.config.storage_root)
            .join("thumbnails")
            .join(session.file_id.to_string()),
    };
    drop(session);

    let sprite_path = thumb_dir.join(&filename);

    match tokio::fs::read(&sprite_path).await {
        Ok(data) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "image/jpeg"),
                (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            ],
            data,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "Sprite not found").into_response(),
    }
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct EnrichmentBackfillRequest {
    /// Re-enqueue every available video file instead of only those below
    /// `PIPELINE_VERSION`. Warm caches short-circuit the decode, supplement
    /// `embeddings` when vision was configured after the first pass, and always
    /// re-run `skip_detect` behind it.
    #[serde(default)]
    pub all_video: bool,
    /// Restrict the walk to one kind of media; absent covers every kind.
    ///
    /// This pass decodes each file in full, so the kinds differ by more than an
    /// order of magnitude in cost. Widening a window that only applies to one
    /// kind does not justify re-analysing the rest.
    #[serde(default)]
    pub media_type: Option<MediaScope>,
}

/// Trigger enrichment backfill for existing library content.
///
/// Enqueues `media_analyze_backfill` which walks every file with
/// `media_cache_version < PIPELINE_VERSION` and queues `media_analyze` for
/// each. Safe to call multiple times — the scheduler fires this on a 30-minute
/// cadence and gates on `enrichment_enabled` at tick time.
pub async fn trigger_enrichment_backfill(
    State(state): State<AppState>,
    auth: AuthUser,
    body: Option<Json<EnrichmentBackfillRequest>>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let EnrichmentBackfillRequest {
        all_video,
        media_type,
    } = body.map(|Json(b)| b).unwrap_or_default();
    let scope = media_type.map(|m| m.as_str()).unwrap_or("all media");
    let context = crate::jobs::JobContext::new(format!("Manual media-cache backfill — {scope}"));
    match crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "media_analyze_backfill",
        json!({ "all_video": all_video, "media_type": media_type }),
        context,
    )
    .await
    {
        Ok(job_id) => (StatusCode::OK, Json(json!({ "job_id": job_id }))).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to enqueue media-cache backfill");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct IntegrityCheckRequest {
    /// Skip the content sample and only compare each file against the size and
    /// mtime its cache was built from. Free, and what the daily schedule runs.
    #[serde(default)]
    pub stamp_only: bool,
    /// Delete what is confirmed bad and mark the episode wanted. Off by
    /// default: this removes media on the strength of a heuristic.
    #[serde(default)]
    pub remediate: bool,
    /// Act on files that are simply gone from disk, dropping the row so the
    /// episode stops reading as in-library and becomes wantable again. Safe
    /// enough to run unattended (the daily schedule sets it), and unlike
    /// `remediate` it never acts on the content-sample heuristic.
    #[serde(default)]
    pub remediate_missing: bool,
}

/// Run the media integrity check now.
///
/// The daily schedule only runs the stamp tier, which reads nothing. The
/// sampling tier reads ~768 KB per file across the library, so it is a
/// deliberate, off-peak decision rather than something to run unattended.
pub async fn run_integrity_check(
    State(state): State<AppState>,
    auth: AuthUser,
    body: Option<Json<IntegrityCheckRequest>>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let context = crate::jobs::JobContext::new(if req.stamp_only {
        "Media integrity check (stamp only)"
    } else {
        "Media integrity check (deep)"
    });
    match crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "media_integrity_check",
        json!({
            "stamp_only": req.stamp_only,
            "remediate": req.remediate,
            "remediate_missing": req.remediate_missing,
        }),
        context,
    )
    .await
    {
        Ok(job_id) => (StatusCode::OK, Json(json!({ "job_id": job_id }))).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to enqueue media integrity check");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct RerunSkipDetectRequest {
    /// Re-detect everything, including content already detected against the
    /// current cache. Needed when the detection *logic* changes: the normal
    /// gate compares cache time to detection time and cannot see that the code
    /// in between is different.
    #[serde(default)]
    pub force: bool,
    /// Restrict the run to one kind of media; absent covers both.
    ///
    /// Forcing both is not the cheap option it looks like. The season path runs
    /// a credit-text classify per episode exactly as the movie path
    /// does, and the library holds ~16× as many episode files as movie files —
    /// so a forced library-wide run is a full-library GPU pass. A change that
    /// only moves the movie path should queue only movies.
    #[serde(default)]
    pub media_type: Option<MediaScope>,
}

/// Re-run skip detection (intro / credits / post-credits) across the library.
///
/// This is the escape hatch for "I turned vision / Full mode on after the
/// library was already scanned".
///
/// One job per season, not per file. Detection is season-scoped — a single job
/// matches every episode in a season against every other and persists the whole
/// set — so a per-file fan-out recomputes each season once per episode it
/// contains. Movies are genuinely per file.
///
/// This deliberately does not go through `media_analyze_backfill`. That rebuilds
/// the media cache (decode, fingerprints, embeddings) for every file, which is
/// the expensive pass and is not what "re-run detection" means; detection reads
/// the cache that already exists.
///
/// Reading the cache is not free either, though: both paths run a zero-shot
/// credit-text classify per file, and the library holds ~16x as many episode
/// files as movie files. `media_type` narrows the run to the kind whose
/// detection actually changed.
///
/// Files whose frame embeddings come from a previous model are not detected
/// without vision: the job queues their embeddings-only re-embed and the season
/// is detected when the last one lands (see `skip_detect::run_episode`).
pub async fn rerun_skip_detect(
    State(state): State<AppState>,
    auth: AuthUser,
    body: Option<Json<RerunSkipDetectRequest>>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let RerunSkipDetectRequest { force, media_type } = body.map(|Json(b)| b).unwrap_or_default();
    // Audiobooks carry no skip segments — accepting the value and queueing
    // nothing would look like a successful no-op run.
    if media_type == Some(MediaScope::Audiobook) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Audiobooks have no skip segments to detect" })),
        )
            .into_response();
    }
    // Non-forced runs only touch what is actually out of date.
    let stale_only = !force;

    let seasons = if MediaScope::Episode.covered_by(media_type) {
        match crate::repositories::tv_episode_files::list_season_detect_targets(
            &state.pool,
            stale_only,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to list season detect targets");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    } else {
        Vec::new()
    };
    let movies = if MediaScope::Movie.covered_by(media_type) {
        match crate::repositories::movie_files::list_detect_targets(&state.pool, stale_only).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to list movie detect targets");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    } else {
        Vec::new()
    };

    let scope_label = match media_type {
        Some(MediaScope::Movie) => "movies",
        Some(MediaScope::Episode) => "TV",
        _ => "library",
    };
    let label = if force {
        format!("Re-run skip detection — {scope_label} (forced)")
    } else {
        format!("Re-run skip detection — {scope_label} (out of date only)")
    };

    let mut queued = 0usize;
    let mut failed = 0usize;
    for (season_id, file_id) in &seasons {
        let context = crate::jobs::JobContext::new(label.as_str());
        if let Err(e) = crate::jobs::enqueue_routine(
            &state.pool,
            state.nats.publisher(),
            "skip_detect",
            json!({
                "file_id": file_id,
                "media_type": "episode",
                // No media_cache run backs a manual re-run; the handler's
                // finalize is a no-op UPDATE against a row that isn't there.
                "run_id": Uuid::nil(),
                "season_id": season_id,
                "force": force,
            }),
            context,
        )
        .await
        {
            tracing::warn!(error = %e, season_id = %season_id, "Failed to enqueue skip_detect");
            failed += 1;
        } else {
            queued += 1;
        }
    }
    for file_id in &movies {
        let context = crate::jobs::JobContext::new(label.as_str());
        if let Err(e) = crate::jobs::enqueue_routine(
            &state.pool,
            state.nats.publisher(),
            "skip_detect",
            json!({
                "file_id": file_id,
                "media_type": "movie",
                "run_id": Uuid::nil(),
                "force": force,
            }),
            context,
        )
        .await
        {
            tracing::warn!(error = %e, file_id = %file_id, "Failed to enqueue skip_detect");
            failed += 1;
        } else {
            queued += 1;
        }
    }

    (
        StatusCode::OK,
        Json(json!({
            "queued": queued,
            "failed": failed,
            "seasons": seasons.len(),
            "movies": movies.len(),
            "forced": force,
            "media_type": media_type.map(|m| m.as_str()),
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scope field is additive — clients that predate it, and the
    /// bare-`{}`/no-body forms the endpoint already accepted, still mean
    /// "everything".
    #[test]
    fn rerun_skip_detect_defaults_to_the_whole_library() {
        let req: RerunSkipDetectRequest = serde_json::from_str("{}").unwrap();
        assert!(!req.force);
        assert_eq!(req.media_type, None);
        assert!(MediaScope::Movie.covered_by(req.media_type));
        assert!(MediaScope::Episode.covered_by(req.media_type));
    }

    /// Forcing a movies-only re-run must not pull in the ~10,000 episode files,
    /// each of which costs a credit-text classify.
    #[test]
    fn rerun_skip_detect_scopes_to_movies() {
        let req: RerunSkipDetectRequest =
            serde_json::from_str(r#"{"force": true, "media_type": "movie"}"#).unwrap();
        assert!(req.force);
        assert_eq!(req.media_type, Some(MediaScope::Movie));
        assert!(MediaScope::Movie.covered_by(req.media_type));
        assert!(!MediaScope::Episode.covered_by(req.media_type));
    }

    #[test]
    fn enrichment_backfill_request_defaults_are_unscoped_and_stale_only() {
        let req: EnrichmentBackfillRequest = serde_json::from_str("{}").unwrap();
        assert!(!req.all_video);
        assert_eq!(req.media_type, None);
    }

    #[test]
    fn switch_audio_request_accepts_null_audio_stream_index() {
        let json = r#"{"audio_stream_index": null, "resume_time": 5.3}"#;
        let req: SwitchAudioRequest = serde_json::from_str(json).unwrap();
        assert!(req.audio_stream_index.is_none());
        assert_eq!(req.resume_time, Some(5.3));
    }

    #[test]
    fn switch_audio_request_accepts_present_audio_stream_index() {
        let json = r#"{"audio_stream_index": 1, "resume_time": 5.3}"#;
        let req: SwitchAudioRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.audio_stream_index, Some(1));
    }

    #[test]
    fn switch_audio_request_accepts_missing_audio_stream_index() {
        let json = r#"{"resume_time": 5.3}"#;
        let req: SwitchAudioRequest = serde_json::from_str(json).unwrap();
        assert!(req.audio_stream_index.is_none());
    }
}
