//! Live IPTV playback sessions.
//!
//! Creates playback sessions for live IPTV channels, handling stream probing,
//! decision engine integration, and ffmpeg transcode for live HLS output.

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::ffmpeg::transcode::TranscodeProcess;
use crate::repositories;
use crate::streaming::decision::{
    self, AudioTrackMeta, ClientCapabilities, FileMeta, PlaybackDecision, PlaybackMethod,
    ViewerPrefs,
};
use crate::streaming::segment_cache::SegmentCache;
use crate::streaming::session::{MediaType, PlaybackSession};

use super::provider_tracker;

/// Response from creating a live IPTV playback session.
pub struct LiveSessionResult {
    pub session_id: Uuid,
    pub method: PlaybackMethod,
    pub playlist_url: Option<String>,
    pub direct_url: Option<String>,
    pub provider_id: Uuid,
    pub audio_tracks: Vec<AudioTrackMeta>,
}

/// Create a live playback session for an IPTV channel.
///
/// Finds an available source, probes the stream, runs the decision engine,
/// and either sets up direct play (proxy) or launches an ffmpeg live transcode.
pub async fn create_live_session(
    pool: &PgPool,
    state: &AppState,
    channel_id: Uuid,
    user_id: Uuid,
    capabilities: ClientCapabilities,
    source_id: Option<Uuid>,
) -> Result<LiveSessionResult> {
    // Reuse an existing live session for this channel only when the caller did
    // NOT pin a specific source. With a pinned source, the user is explicitly
    // asking to try that source — returning a stale session tied to a
    // different (possibly broken) source would silently ignore the choice.
    if source_id.is_none() {
        for mut entry in state.playback_sessions.iter_mut() {
            let session = entry.value_mut();
            if session.file_id == channel_id && session.is_live {
                session.last_activity = std::time::Instant::now();
                let (playlist_url, direct_url) = match session.decision.method {
                    PlaybackMethod::DirectPlay => (
                        None,
                        Some(format!("/api/v1/admin/iptv/channels/{channel_id}/play.ts")),
                    ),
                    _ => (
                        Some(format!(
                            "/api/v1/admin/playback/sessions/{}/stream.m3u8",
                            session.id
                        )),
                        None,
                    ),
                };
                info!(
                    session_id = %session.id,
                    channel_id = %channel_id,
                    "reusing existing live session"
                );
                return Ok(LiveSessionResult {
                    session_id: session.id,
                    method: session.decision.method,
                    playlist_url,
                    direct_url,
                    provider_id: Uuid::nil(),
                    audio_tracks: vec![],
                });
            }
        }
    }

    let config = state.runtime_config.get().await;
    let http = crate::http::new_client();

    // Get sources ordered by priority/health
    let mut sources = repositories::iptv_sources::list_by_channel(pool, channel_id)
        .await
        .context("listing sources for channel")?;

    // If a specific source was requested, filter to just that one
    if let Some(sid) = source_id {
        sources.retain(|s| s.id == sid);
    }

    if sources.is_empty() {
        bail!("no sources available for channel {channel_id}");
    }

    // Track per-provider connection failures to detect exhaustion
    let mut provider_conn_failures: HashMap<Uuid, u32> = HashMap::new();
    let mut provider_source_counts: HashMap<Uuid, u32> = HashMap::new();

    // Count sources per provider
    for source in &sources {
        *provider_source_counts
            .entry(source.provider_id)
            .or_default() += 1;
    }

    // Try each source until one works
    for source in &sources {
        if source.health_score <= 0.0 {
            continue;
        }

        // Check provider capacity
        if !provider_tracker::has_capacity(source.provider_id) {
            warn!(
                provider_id = %source.provider_id,
                "provider at connection limit, skipping"
            );
            continue;
        }

        let provider =
            match repositories::iptv_providers::get_by_id(pool, source.provider_id).await? {
                Some(p) => p,
                None => continue,
            };

        let client = super::xtream::XtreamClient::new(
            http.clone(),
            &provider.url,
            &provider.username,
            &provider.password,
        );
        let stream_url = client.stream_url(source.remote_stream_id, "ts");

        // Probe the stream via single-connection: open HTTP, buffer, probe with kob-ffmpeg
        let probe_result = probe_and_hold_stream(&http, &stream_url).await;

        let (
            LiveProbe {
                video_codec,
                interlaced,
                audio_tracks,
            },
            live_stream,
        ) = match probe_result {
            Ok(info) => info,
            Err(e) => {
                let err_msg = e.to_string();
                let is_conn_limit = is_connection_limit_error(&err_msg);

                if is_conn_limit {
                    warn!(
                        source_id = %source.id,
                        provider_id = %source.provider_id,
                        error = %e,
                        "probe failed due to provider connection limit, not penalizing source health"
                    );
                    let count = provider_conn_failures
                        .entry(source.provider_id)
                        .or_default();
                    *count += 1;

                    // Check if all sources from this provider have failed with connection errors
                    if let Some(&total) = provider_source_counts.get(&source.provider_id)
                        && *count >= total
                    {
                        warn!(
                            provider_id = %source.provider_id,
                            provider_name = %provider.name,
                            "provider {} appears to have no available connections (external usage?)",
                            provider.name,
                        );
                    }
                } else {
                    warn!(
                        source_id = %source.id,
                        error = %e,
                        "failed to probe stream, trying next source"
                    );
                    let new_score = (source.health_score - 0.2).max(0.0);
                    let _ =
                        repositories::iptv_sources::update_health(pool, source.id, new_score).await;
                }
                continue;
            }
        };

        // Build file metadata for the decision engine
        let file_meta = FileMeta {
            container: "mpegts".to_string(),
            video_codec: video_codec.clone(),
            video_width: 1920, // Assume 1080p for live streams
            video_height: 1080,
            interlaced,
        };

        let preferred_codecs = state.encoder_caps.preferred_codecs();
        let decision = decision::decide(
            &file_meta,
            &audio_tracks,
            &[], // No subtitle tracks for live streams
            &capabilities,
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: &config.preferred_audio_language,
                subtitle_language: "",
            },
            &preferred_codecs,
        );

        // Acquire provider connection
        if !provider_tracker::acquire(source.provider_id) {
            warn!(
                provider_id = %source.provider_id,
                "failed to acquire connection slot"
            );
            // Drop stream and wait for provider to release the connection slot
            drop(live_stream);
            tokio::time::sleep(Duration::from_secs(3)).await;
            continue;
        }

        let session_id = Uuid::new_v4();

        match build_session(
            state,
            &config,
            session_id,
            user_id,
            channel_id,
            source.provider_id,
            &stream_url,
            &video_codec,
            &audio_tracks,
            &decision,
            live_stream,
            &http,
        )
        .await
        {
            Ok(result) => {
                info!(
                    session_id = %session_id,
                    channel_id = %channel_id,
                    provider_id = %source.provider_id,
                    method = ?decision.method,
                    "created live IPTV session"
                );
                return Ok(LiveSessionResult {
                    session_id,
                    method: decision.method,
                    playlist_url: result.0,
                    direct_url: result.1,
                    provider_id: source.provider_id,
                    audio_tracks: audio_tracks.clone(),
                });
            }
            Err(e) => {
                provider_tracker::release(source.provider_id);
                error!(error = %e, "failed to build live session");
                // live_stream was moved into build_session and dropped on Err —
                // wait for provider to release the connection slot before trying next source
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
        }
    }

    bail!("all sources exhausted for channel {channel_id}")
}

/// Check if a probe error indicates provider connection limits rather than a source issue.
///
/// HTTP 458 is commonly returned by IPTV providers when max connections are exceeded.
/// Other 4xx errors on stream URLs are also likely connection/auth issues, not source problems.
pub fn is_connection_limit_error(error_msg: &str) -> bool {
    let lower = error_msg.to_lowercase();
    // HTTP 458 is the canonical "max connections exceeded" error from xtream providers
    if lower.contains("458") {
        return true;
    }
    // Other HTTP errors that suggest provider-level capacity/auth issues
    if lower.contains("http error 4") || lower.contains("403") || lower.contains("429") {
        return true;
    }
    // reqwest error_for_status() format: "HTTP status client error (458) for url ..."
    if lower.contains("client error") {
        return true;
    }
    // Common connection-related messages
    if lower.contains("max connections")
        || lower.contains("too many connections")
        || lower.contains("connection refused")
    {
        return true;
    }
    false
}

/// Probe a live stream URL to get codec info using kob-ffmpeg.
pub async fn probe_live_stream(url: &str) -> Result<LiveProbe> {
    let url = url.to_string();
    tokio::task::spawn_blocking(move || {
        let result = kob_ffmpeg::iptv::probe_stream(&url)
            .map_err(|e| anyhow::anyhow!("probe failed: {e}"))?;
        probe_result_to_meta(&result)
    })
    .await
    .context("probe task join error")?
}

#[derive(Debug)]
pub struct LiveProbe {
    pub video_codec: String,
    pub interlaced: bool,
    pub audio_tracks: Vec<AudioTrackMeta>,
}

pub fn probe_result_to_meta(result: &kob_ffmpeg::probe::ProbeResult) -> Result<LiveProbe> {
    let video = result
        .video_streams
        .first()
        .context("no video stream found in probe output")?;

    let audio_tracks: Vec<AudioTrackMeta> = result
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

    Ok(LiveProbe {
        video_codec: video.codec.clone(),
        interlaced: video.is_interlaced,
        audio_tracks,
    })
}

/// Boxed byte stream from an HTTP response body.
type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

/// A pre-opened live stream ready for consumption.
/// Created during probing, consumed once by either direct-play proxy or ffmpeg transcode.
pub struct LiveStream {
    pub probe_buf: Vec<u8>,
    /// The remaining byte stream after the probe buffer. Wrapped in Mutex because
    /// `dyn Stream + Send` is not `Sync`, but DashMap requires `Sync` values.
    /// The Mutex is never contended — the stream is inserted once and removed once.
    pub byte_stream: std::sync::Mutex<ByteStream>,
    pub content_type: String,
}

/// Open an HTTP connection to a stream, buffer enough data to probe codecs,
/// and return the probe results along with the still-open stream.
///
/// This avoids opening two connections (one for probe, one for playback) which fails
/// with `max_connections: 1` providers. The buffer is written to a temp file for
/// kob-ffmpeg to probe, while the original HTTP stream remains open.
async fn probe_and_hold_stream(
    http: &reqwest::Client,
    stream_url: &str,
) -> Result<(LiveProbe, LiveStream)> {
    // Open the stream — override the shared client's 30s timeout for live streams
    let resp = http
        .get(stream_url)
        .timeout(Duration::from_secs(86400))
        .send()
        .await
        .context("connecting to stream")?
        .error_for_status()
        .context("stream HTTP status")?;

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("video/mp2t")
        .to_string();

    // Buffer the first ~128KB of MPEG-TS data for probing.
    let mut byte_stream = Box::pin(resp.bytes_stream());
    let mut probe_buf = Vec::with_capacity(128 * 1024);
    while probe_buf.len() < 128 * 1024 {
        match byte_stream.next().await {
            Some(Ok(chunk)) => probe_buf.extend_from_slice(&chunk),
            Some(Err(e)) => bail!("error buffering stream for probe: {e}"),
            None => break,
        }
    }

    // Write buffer to temp file for kob-ffmpeg to probe
    let probe_data = probe_buf.clone();
    let probe = tokio::task::spawn_blocking(move || {
        let tmp = tempfile::Builder::new()
            .suffix(".ts")
            .tempfile()
            .map_err(|e| anyhow::anyhow!("creating temp file for probe: {e}"))?;
        std::fs::write(tmp.path(), &probe_data)
            .map_err(|e| anyhow::anyhow!("writing probe buffer: {e}"))?;

        let result = kob_ffmpeg::probe::probe_with_options(
            tmp.path(),
            kob_ffmpeg::probe::ProbeOptions {
                probesize: 131072,
                max_analyze_duration: 2_000_000,
            },
        )
        .map_err(|e| anyhow::anyhow!("probe failed: {e}"))?;

        probe_result_to_meta(&result)
    })
    .await
    .context("probe task join error")??;

    Ok((
        probe,
        LiveStream {
            probe_buf,
            byte_stream: std::sync::Mutex::new(byte_stream),
            content_type,
        },
    ))
}

/// Build the session and optionally start ffmpeg transcode.
///
/// Returns (playlist_url, direct_url).
#[allow(clippy::too_many_arguments)]
async fn build_session(
    state: &AppState,
    config: &crate::runtime_config::RuntimeConfig,
    session_id: Uuid,
    user_id: Uuid,
    channel_id: Uuid,
    provider_id: Uuid,
    stream_url: &str,
    video_codec: &str,
    audio_tracks: &[AudioTrackMeta],
    decision: &PlaybackDecision,
    live_stream: LiveStream,
    http: &reqwest::Client,
) -> Result<(Option<String>, Option<String>)> {
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

    let (playlist_url, direct_url, transcode) = if decision.method == PlaybackMethod::DirectPlay {
        // Direct play: store the pre-opened stream for play_ts() to consume
        let proxy_url = format!("/api/v1/admin/iptv/channels/{channel_id}/play.ts");
        state.live_streams.insert(channel_id, live_stream);
        (None, Some(proxy_url), None)
    } else {
        // Need transcode: spawn ffmpeg for live HLS, feeding from the open stream
        tokio::fs::create_dir_all(&segment_dir)
            .await
            .context("creating segment dir")?;

        let process =
            start_live_transcode(live_stream, &segment_dir, state, decision, stream_url, http)
                .await
                .context("starting live transcode")?;

        let url = format!("/api/v1/admin/playback/sessions/{session_id}/stream.m3u8");
        (Some(url), None, Some(process))
    };

    // Build a minimal SegmentCache for the session
    // Duration 0: live sessions serve ffmpeg's on-disk stream.m3u8 directly,
    // so the SegmentCache is unused — avoid allocating segments for infinite duration.
    let segment_cache = SegmentCache::new(segment_dir.clone(), 4, 0.0);

    // Get first audio track for metadata
    let first_audio = audio_tracks.first();

    let session = PlaybackSession {
        id: session_id,
        user_id,
        media_type: MediaType::Episode, // Reuse Episode for live TV
        file_id: channel_id,            // Store channel_id as file_id
        file_path: PathBuf::from(stream_url),
        decision: decision.clone(),
        transcode,
        segment_cache,
        created_at: std::time::Instant::now(),
        last_activity: std::time::Instant::now(),
        seek_generation: 0,
        is_live: true,
        provider_id: Some(provider_id),
        duration_seconds: 0.0,
        container: "mpegts".to_string(),
        video_codec: video_codec.to_string(),
        subtitle_files: vec![],
        hw_accel: (*state.hw_accel).clone(),
        source_resolution: (1920, 1080),
        variants: None,
        max_bitrate_kbps: None,
        max_height: None,
        keyframes: None,
        error: None,
        pending_segment: None,
        download_cancel: None,
        thumbnail_dir: None,
        debrid_cache_key: None,
        audio_metas: vec![],
        preferred_audio_index: None,
        force_video_transcode: false,
        corrupt_ranges: Vec::new(),
        overlay_spawned_starts: std::collections::HashSet::new(),
        highest_served_segment: None,
        feed: None,
        download: None,
        cue_gate: None,
        persisted: false,
    };

    state.playback_sessions.insert(session_id, session);

    info!(
        session_id = %session_id,
        method = ?decision.method,
        video_codec = %video_codec,
        audio_codec = first_audio.map(|a| a.codec.as_str()).unwrap_or("unknown"),
        "live session ready"
    );

    Ok((playlist_url, direct_url))
}

/// Start a live HLS transcode, feeding from a pre-opened stream via kob-ffmpeg.
///
/// Architecture: async HTTP bytes → mpsc channel → CustomIoContext → transcode_to_hls.
/// The transcode runs in spawn_blocking; the feeder runs as a tokio task with reconnect.
async fn start_live_transcode(
    live_stream: LiveStream,
    hls_dir: &std::path::Path,
    state: &AppState,
    decision: &PlaybackDecision,
    stream_url: &str,
    http: &reqwest::Client,
) -> Result<TranscodeProcess> {
    let playlist_path = hls_dir.join("stream.m3u8");
    let hls_dir_owned = hls_dir.to_path_buf();

    // Build HlsConfig from the playback decision
    let video_encoder = match &decision.video_mode {
        decision::VideoMode::Copy => None,
        decision::VideoMode::Transcode { target_codec } => {
            let encoder = state
                .encoder_caps
                .encoder_for(target_codec)
                .unwrap_or(&state.encoder_caps.h264);
            Some(encoder.to_string())
        }
    };
    let (audio_encoder, audio_bitrate) = match &decision.audio_mode {
        decision::AudioMode::Copy => (None, None),
        decision::AudioMode::Transcode {
            target_codec,
            bitrate_kbps,
        } => {
            let encoder = match target_codec.as_str() {
                // The decision engine uses AAC_ENCODER (libfdk_aac) directly,
                // but accept legacy "aac" for backward compat with any older
                // session payloads still in flight.
                "aac" => crate::streaming::decision::AAC_ENCODER,
                "opus" => "libopus",
                other => other,
            };
            (Some(encoder.to_string()), Some(*bitrate_kbps as i64 * 1000))
        }
    };

    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();

    let mut config = kob_ffmpeg::transcode::HlsConfig::new(&hls_dir_owned).segment_duration(4.0);
    if let Some(enc) = video_encoder {
        config = config.video_encoder(&enc);
        if decision.deinterlace {
            config = config.deinterlace(kob_ffmpeg::filter::graph::Deinterlace::Auto);
        }
    }
    if let Some(enc) = audio_encoder {
        config = config.audio_encoder(&enc);
    }
    if let Some(br) = audio_bitrate {
        config = config.audio_bitrate(br);
    }
    if decision.audio_stream_index > 0 {
        config.audio_stream_index = Some(decision.audio_stream_index as u32);
    }
    config.run = Some(kob_ffmpeg::progress::RunContext::cancellable(cancel_clone));
    config.live_source = true;
    if let Some(hw) = state.hw_device.get() {
        config = config.hw_device(hw);
    }

    // Channel bridge: async HTTP bytes → sync channel → CustomIoContext.
    // Uses a standard mpsc (unbounded receiver for the blocking side) with a
    // bounded async sender that yields back to tokio on backpressure instead
    // of blocking the runtime thread.
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();

    // Spawn the transcode in a blocking thread, reading from channel-backed I/O
    let transcode_handle = tokio::task::spawn_blocking(move || {
        let mut io = kob_ffmpeg::format::io_context::CustomIoContext::from_channel(rx)?;
        let mut ctx = kob_ffmpeg::format::input::InputContext::open_custom(&mut io)?;
        ctx.find_stream_info()?;
        kob_ffmpeg::transcode::transcode_to_hls_from_ctx(ctx, &config).map(|_| ())
    });

    // Spawn the async feeder task: HTTP bytes → channel sender
    let probe_buf = live_stream.probe_buf;
    let mut byte_stream = live_stream
        .byte_stream
        .into_inner()
        .expect("mutex not poisoned");
    let reconnect_url = stream_url.to_string();
    let reconnect_http = http.clone();
    let original_content_type = live_stream.content_type.clone();
    // Generous reconnect budget for live streams that may run for hours.
    // Exponential backoff: 2s, 4s, 8s, 16s, 30s, 30s, 30s... (capped at 30s)
    const MAX_RECONNECTS: u32 = 10;

    let health = std::sync::Arc::new(std::sync::Mutex::new(
        crate::ffmpeg::transcode::StreamHealth::new(),
    ));
    let feeder_health = health.clone();

    tokio::spawn(async move {
        // Feed probe buffer first — mpsc::Sender::send is non-blocking (unbounded)
        {
            let len = probe_buf.len() as u64;
            if tx.send(probe_buf).is_err() {
                return;
            }
            if let Ok(mut h) = feeder_health.lock() {
                h.source_bytes_received += len;
            }
        }

        /// Feed bytes from an async stream to the channel, updating health counters.
        /// Returns `true` if the channel closed (transcode ended).
        /// Uses a 10-second timeout per chunk — if the source stalls longer than that,
        /// we treat it as a disconnect and trigger reconnect rather than waiting for
        /// the full HTTP timeout (30s+) which leaves the player frozen.
        async fn feed_stream(
            tx: &std::sync::mpsc::Sender<Vec<u8>>,
            byte_stream: &mut ByteStream,
            health: &std::sync::Arc<std::sync::Mutex<crate::ffmpeg::transcode::StreamHealth>>,
        ) -> bool {
            const STALL_TIMEOUT: Duration = Duration::from_secs(10);

            loop {
                match tokio::time::timeout(STALL_TIMEOUT, byte_stream.next()).await {
                    Ok(Some(Ok(data))) => {
                        let len = data.len() as u64;
                        if tx.send(data.to_vec()).is_err() {
                            return true; // channel closed
                        }
                        if let Ok(mut h) = health.lock() {
                            h.source_bytes_received += len;
                            h.last_data_at = Some(std::time::Instant::now());
                        }
                    }
                    Ok(Some(Err(e))) => {
                        warn!(error = %e, "stream read error while feeding transcode");
                        if let Ok(mut h) = health.lock() {
                            h.is_source_connected = false;
                            h.error_message = Some(e.to_string());
                        }
                        return false; // source error, can retry
                    }
                    Ok(None) => {
                        return false; // stream ended, can retry
                    }
                    Err(_) => {
                        warn!(
                            "source stream stalled (no data for {STALL_TIMEOUT:?}), treating as disconnect"
                        );
                        if let Ok(mut h) = health.lock() {
                            h.is_source_connected = false;
                            h.error_message = Some("source stalled".to_string());
                        }
                        return false; // stall → reconnect
                    }
                }
            }
        }

        if feed_stream(&tx, &mut byte_stream, &feeder_health).await {
            return;
        }

        // Mark source as disconnected before reconnect attempts
        if let Ok(mut h) = feeder_health.lock() {
            h.is_source_connected = false;
        }

        // Source stream ended — attempt reconnects with exponential backoff.
        // After a successful reconnect + feed, the counter resets so long-running
        // streams get fresh reconnect budgets for each new disconnect event.
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            if attempt > MAX_RECONNECTS {
                break;
            }
            warn!(
                attempt,
                max = MAX_RECONNECTS,
                url = %reconnect_url,
                "source stream ended, attempting reconnect"
            );
            // Exponential backoff capped at 30 seconds
            let delay = Duration::from_secs((2u64.pow(attempt)).min(30));
            tokio::time::sleep(delay).await;

            let resp = match reconnect_http
                .get(&reconnect_url)
                .timeout(Duration::from_secs(86400))
                .send()
                .await
            {
                Ok(r) => match r.error_for_status() {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(error = %e, attempt, "reconnect failed (HTTP error)");
                        continue;
                    }
                },
                Err(e) => {
                    warn!(error = %e, attempt, "reconnect failed (connection error)");
                    continue;
                }
            };

            // Check if the content type changed (e.g., TS → m3u8 or vice versa).
            // Don't abort — FFmpeg is resilient to minor format changes — but log it.
            let new_content_type = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown");
            if new_content_type != original_content_type {
                warn!(
                    attempt,
                    original = %original_content_type,
                    new = %new_content_type,
                    "source stream content-type changed after reconnect"
                );
            }

            // Mark source as reconnected
            if let Ok(mut h) = feeder_health.lock() {
                h.is_source_connected = true;
                h.error_message = None;
                h.reconnect_count += 1;
                h.last_data_at = Some(std::time::Instant::now());
            }

            info!(attempt, "reconnected to source stream");
            let mut new_stream: ByteStream = Box::pin(resp.bytes_stream());
            if feed_stream(&tx, &mut new_stream, &feeder_health).await {
                return; // transcode ended
            }

            // Stream disconnected again — but since we successfully fed data,
            // reset the attempt counter so long-running streams get fresh budgets.
            attempt = 0;

            if let Ok(mut h) = feeder_health.lock() {
                h.is_source_connected = false;
            }
        }

        warn!("all reconnect attempts exhausted, giving up");
        if let Ok(mut h) = feeder_health.lock() {
            h.is_source_connected = false;
            h.error_message = Some("all reconnect attempts exhausted".to_string());
        }
        // tx dropped here → channel closes → CustomIoContext returns EOF
    });

    // Wait briefly for the transcode to start (format detection)
    tokio::time::sleep(Duration::from_millis(100)).await;

    info!(
        output_dir = %hls_dir.display(),
        "started live kob-ffmpeg transcode"
    );

    Ok(TranscodeProcess {
        backend: crate::ffmpeg::transcode::TranscodeBackend::Library {
            handle: transcode_handle,
            cancel,
        },
        segment_dir: hls_dir_owned,
        playlist_path,
        stream_health: Some(health),
        started_at: std::time::Instant::now(),
    })
}

/// Destroy a live session: kill ffmpeg, release provider connection, clean up.
pub async fn destroy_live_session(state: &AppState, session_id: Uuid, provider_id: Uuid) {
    let channel_id = state.playback_sessions.get(&session_id).map(|s| s.file_id);

    if let Some((_, mut session)) = state.playback_sessions.remove(&session_id) {
        session.kill_all_transcodes().await;

        let dir = session.segment_dir().to_path_buf();
        if dir.exists() {
            let _ = tokio::fs::remove_dir_all(&dir).await;
        }
    }

    // Drop any unconsumed pre-opened stream for this channel
    if let Some(channel_id) = channel_id {
        state.live_streams.remove(&channel_id);
    }

    provider_tracker::release(provider_id);
    info!(session_id = %session_id, "destroyed live IPTV session");
}
