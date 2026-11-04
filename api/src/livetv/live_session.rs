//! Live TV playback. A tuner is a LAN device connected to an antenna, so this
//! path deliberately has none of `iptv/live_session.rs`'s insurance against a
//! slow remote server: no probe and no health scoring. The one failover is to the
//! other services of the same channel's stack, for a mux the antenna cannot lock.

use anyhow::{Result, anyhow};
use bytes::Bytes;
use dashmap::DashMap;
use futures_util::{Stream, StreamExt};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use crate::ffmpeg::transcode::{TranscodeBackend, TranscodeProcess};
use crate::repositories::{
    livetv_channels, livetv_guide, livetv_sessions, livetv_tuners, livetv_watch_events,
};
use crate::streaming::decision::{
    self, AudioTrackMeta, ClientCapabilities, FileMeta, PlaybackMethod, ViewerPrefs,
};

use super::fanout::Fanout;
use super::stacking::StackCache;
use super::trickplay;
use super::tuner_lease::{
    HolderKind, LEASE_LINGER, LeaseError, LeaseRequest, LeaseToken, TunerLeases,
};
use super::tuner_source::{self, FIRST_BYTE_DEADLINE, SourceCandidate, TuneHealth};

/// One GOP. The tuner's GOP is ~0.96 s, so this is the smallest segment that
/// never straddles two keyframes — the whole point of Decision 12.
const SEGMENT_SECONDS: f64 = 1.0;
const TUNER_STREAM_PORT: u16 = 5004;
/// A LAN device either has a lock or it does not; a long backoff is latency
/// spent guarding a risk that does not exist.
const RECONNECT_DELAYS_MS: [u64; 5] = [250, 500, 1000, 2000, 4000];
/// Both players heartbeat every 15 s while a session is open, paused or not.
const SESSION_IDLE: Duration = Duration::from_secs(300);
const EMPTY_TUNE_GRACE: time::Duration = time::Duration::seconds(30);

pub type ByteStream = Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Direct,
    Transcode,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Transcode => "transcode",
        }
    }
}

pub struct LiveTune {
    pub id: Uuid,
    pub key: String,
    pub channel_id: Uuid,
    pub tuner_id: Uuid,
    pub lease: LeaseToken,
    pub buffer_dir: PathBuf,
    pub started_at: OffsetDateTime,
    pub attached: HashSet<String>,
    pub recording_id: Option<Uuid>,
    pub transcode: Option<TranscodeProcess>,
    pub cancel: CancellationToken,
    /// Raw TS fan-out. One tuner connection feeds both the segment buffer and
    /// every DirectPlay client, so a direct viewer costs no extra tuner and the
    /// buffer exists from the moment we tune whichever way the client plays it.
    pub fanout: Arc<Fanout>,
    pub health: Arc<TuneHealth>,
}

#[derive(Debug, Clone)]
pub struct SessionState {
    pub tune_key: String,
    pub channel_id: Uuid,
    pub user_id: Uuid,
    pub started_at: OffsetDateTime,
    pub guide_id: Option<Uuid>,
    pub title: Option<String>,
    pub series_id: Option<String>,
    pub episode_key: Option<String>,
    pub source: String,
    pub last_seen: std::time::Instant,
    /// Why the tune behind this session died. The session outlives it so the viewer
    /// can be told, until they tune again or it is reaped.
    pub failure: Option<String>,
}

pub struct LivetvState {
    pub leases: Arc<TunerLeases>,
    pub tunes: DashMap<String, LiveTune>,
    pub sessions: DashMap<String, SessionState>,
    pub stacks: StackCache,
}

impl LivetvState {
    pub fn new(leases: Arc<TunerLeases>) -> Arc<Self> {
        Arc::new(Self {
            leases,
            tunes: DashMap::new(),
            sessions: DashMap::new(),
            stacks: StackCache::default(),
        })
    }
}

pub struct StreamFacts {
    pub video_codec: String,
    pub audio_codec: String,
    pub width: u32,
    pub height: u32,
    pub hd: bool,
    pub interlaced: bool,
}

/// Everything the decision engine needs, straight off the row the scan wrote.
/// Australian DVB-T is field-coded end to end: 1080i25 for HD, 576i25 for SD.
pub fn stream_facts(row: &livetv_channels::ChannelRow) -> StreamFacts {
    let video_codec = match row
        .video_codec
        .as_deref()
        .unwrap_or("")
        .to_ascii_uppercase()
        .as_str()
    {
        "MPEG2" => "mpeg2video",
        "H264" | "AVC" => "h264",
        "HEVC" | "H265" => "hevc",
        _ if row.hd => "h264",
        _ => "mpeg2video",
    }
    .to_string();
    let audio_codec = match row
        .audio_codec
        .as_deref()
        .unwrap_or("")
        .to_ascii_uppercase()
        .as_str()
    {
        "AC3" => "ac3",
        "EAC3" => "eac3",
        "AAC" => "aac",
        _ => "mp2",
    }
    .to_string();
    let (width, height) = if row.hd { (1920, 1080) } else { (720, 576) };
    StreamFacts {
        video_codec,
        audio_codec,
        width,
        height,
        hd: row.hd,
        interlaced: row.kind != "radio",
    }
}

/// Postgres stores TIMESTAMPTZ at microsecond resolution, so an in-memory
/// timestamp and its round-tripped copy must agree or the buffer start the
/// client is given will not equal the recording start it is promoted to.
fn now_pg() -> OffsetDateTime {
    let t = OffsetDateTime::now_utc();
    t.replace_nanosecond((t.nanosecond() / 1_000) * 1_000)
        .unwrap_or(t)
}

fn host_of(base_url: &str) -> String {
    base_url
        .trim()
        .trim_end_matches('/')
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_string()
}

pub fn tuner_stream_url(base_url: &str, lcn: i32) -> String {
    format!(
        "http://{}:{TUNER_STREAM_PORT}/auto/v{lcn}",
        host_of(base_url)
    )
}

fn candidate(row: &livetv_channels::ChannelRow, base_url: &str, lcn: i32) -> SourceCandidate {
    SourceCandidate {
        channel_id: row.id,
        name: super::classify::display_name(
            row.display_name.as_deref(),
            row.scan_name.as_deref(),
            row.callsign.as_deref(),
        ),
        url: tuner_stream_url(base_url, lcn),
        frequency: row.frequency,
    }
}

/// The channel itself, then the rest of its stack best first — those on another mux
/// ahead of those sharing its mux, which fail together when the mux has no signal. Only
/// members on the same device: the lease is per device, so a member on another one
/// would tune a tuner nobody holds.
pub async fn source_candidates(
    state: &crate::AppState,
    channel: &livetv_channels::ChannelRow,
    tuner: &livetv_tuners::TunerRow,
) -> Vec<SourceCandidate> {
    let Some(lcn) = channel.lcn else {
        return Vec::new();
    };
    let mut out = vec![candidate(channel, &tuner.base_url, lcn)];
    let fallbacks = match state.livetv.stacks.get(&state.pool).await {
        Ok(stacks) => stacks.fallbacks(channel.id),
        Err(e) => {
            warn!(error = %e, "live TV stacks unavailable, tuning without fallbacks");
            return out;
        }
    };
    if fallbacks.is_empty() {
        return out;
    }
    let rows =
        match livetv_channels::list_for_guide(&state.pool, Some(fallbacks.clone()), true).await {
            Ok(rows) => rows,
            Err(e) => {
                warn!(error = %e, "loading live TV stack members failed");
                return out;
            }
        };
    let mut same_mux = Vec::new();
    for id in fallbacks {
        if let Some(row) = rows.iter().find(|r| r.id == id && r.tuner_id == tuner.id)
            && let Some(lcn) = row.lcn
        {
            let c = candidate(row, &tuner.base_url, lcn);
            if row.frequency == channel.frequency {
                same_mux.push(c);
            } else {
                out.push(c);
            }
        }
    }
    out.extend(same_mux);
    out
}

/// Cap the ladder rather than running open-ended CRF: measured 720p50 bwdif
/// output at crf 23 was ~6 Mbit/s, four times the storage the buffer budget
/// assumed.
fn video_bitrate_for(height: u32) -> i64 {
    match height {
        0..=480 => 1_500_000,
        481..=576 => 2_000_000,
        577..=720 => 3_000_000,
        _ => 5_000_000,
    }
}

pub struct LiveSessionResult {
    pub session_id: String,
    pub channel_id: Uuid,
    pub mode: Mode,
    pub playlist_url: Option<String>,
    pub direct_url: Option<String>,
    pub facts: StreamFacts,
    pub buffer_start_time: OffsetDateTime,
    pub lease: super::tuner_lease::LeaseInfo,
    pub tuners_in_use: u32,
    pub tuner_count: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("channel not found")]
    ChannelNotFound,
    #[error("tuner not found")]
    TunerNotFound,
    #[error("channel has no logical channel number, so the device cannot tune it")]
    NotTunable,
    #[error("{0}")]
    Exhausted(String),
    #[error("session not found")]
    SessionNotFound,
    #[error("{0}")]
    TuneFailed(String),
    #[error(transparent)]
    Repository(#[from] crate::repositories::RepositoryError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<LeaseError> for SessionError {
    fn from(e: LeaseError) -> Self {
        SessionError::Exhausted(e.to_string())
    }
}

pub fn buffer_root(
    config: &crate::Config,
    runtime: &crate::runtime_config::RuntimeConfig,
) -> PathBuf {
    runtime
        .transcode_cache_dir
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&config.storage_root).join("cache"))
        .join("livetv")
}

fn decide_mode(facts: &StreamFacts, capabilities: &ClientCapabilities) -> Mode {
    let file = FileMeta {
        container: "mpegts".to_string(),
        video_codec: facts.video_codec.clone(),
        video_width: facts.width,
        video_height: facts.height,
        interlaced: facts.interlaced,
    };
    let audio = [AudioTrackMeta {
        stream_index: 0,
        codec: facts.audio_codec.clone(),
        channels: 2,
        language: None,
        title: None,
        is_default: true,
    }];
    // A client that reads MPEG-TS natively is libVLC or ExoPlayer, which present
    // field-coded video themselves; no browser does either. So that is the signal for a
    // client that says nothing — one that says either way is believed.
    let mut capabilities = capabilities.clone();
    if capabilities.supports_interlaced.is_none() {
        capabilities.supports_interlaced = Some(
            capabilities
                .containers
                .iter()
                .any(|c| c == "mpegts" || c == "ts"),
        );
    }
    let decision = decision::decide(
        &file,
        &audio,
        &[],
        &capabilities,
        ViewerPrefs::default(),
        &["h264"],
    );
    if decision.method == PlaybackMethod::DirectPlay {
        Mode::Direct
    } else {
        Mode::Transcode
    }
}

pub async fn create_session(
    state: &crate::AppState,
    channel_id: Uuid,
    user_id: Uuid,
    capabilities: &ClientCapabilities,
) -> std::result::Result<LiveSessionResult, SessionError> {
    let session_id = Uuid::now_v7().to_string();
    tune(state, &session_id, channel_id, user_id, capabilities, false).await
}

/// Inline channel switch. The old tune lingers, still tuned, so flicking back is
/// instant; a waiting acquire reaps it early.
///
/// Lingering is also what lets the player stop the old channel cleanly: it does so only
/// once this has answered, and a stream that ended in that same moment hung libVLC's
/// stop for as long as fifteen seconds.
///
/// The new channel is tuned before the old one is let go. Letting it go first left the
/// channel the viewer is still watching with no tuner behind it for the length of a tune,
/// and the player had nothing to show but the tail of a stream that had stopped — so the
/// change needed a caption or a black frame over it to be bearable. Tuned in this order
/// there is a live picture right up to the cut.
pub async fn switch_channel(
    state: &crate::AppState,
    session_id: &str,
    channel_id: Uuid,
    user_id: Uuid,
    capabilities: &ClientCapabilities,
) -> std::result::Result<LiveSessionResult, SessionError> {
    let Some(previous) = state.livetv.sessions.get(session_id).map(|s| s.clone()) else {
        return Err(SessionError::SessionNotFound);
    };
    write_watch_event(state, &previous).await;

    // Re-tuning the channel already on is a no-op that must not then tear down the tune
    // it just re-attached to.
    let leaving = if previous.channel_id == channel_id {
        None
    } else {
        Some(previous.tune_key.as_str())
    };

    match tune(state, session_id, channel_id, user_id, capabilities, true).await {
        Ok(result) => {
            if let Some(key) = leaving {
                leave_lingering(state, session_id, key);
            }
            Ok(result)
        }
        // Nothing to overlap with: on a device whose tuners are all busy the outgoing
        // tune is the only free slot there is, so it goes first and the switch takes the
        // slower path rather than failing.
        Err(SessionError::Exhausted(_)) if leaving.is_some() => {
            detach_from_tune(state, session_id, previous.tune_key.as_str());
            tune(state, session_id, channel_id, user_id, capabilities, true).await
        }
        Err(e) => Err(e),
    }
}

async fn tune(
    state: &crate::AppState,
    session_id: &str,
    channel_id: Uuid,
    user_id: Uuid,
    capabilities: &ClientCapabilities,
    retune: bool,
) -> std::result::Result<LiveSessionResult, SessionError> {
    let channel = livetv_channels::get_by_id(&state.pool, channel_id)
        .await?
        .ok_or(SessionError::ChannelNotFound)?;
    let tuner = livetv_tuners::get_by_id(&state.pool, channel.tuner_id)
        .await?
        .ok_or(SessionError::TunerNotFound)?;
    if channel.lcn.is_none() {
        return Err(SessionError::NotTunable);
    }

    let facts = stream_facts(&channel);
    let mode = decide_mode(&facts, capabilities);
    // Mode-independent: two DirectPlay viewers, a web viewer and a recording on
    // one channel are one physical tune, so they share one lease and one buffer.
    let key = format!("c:{channel_id}");

    let runtime = state.runtime_config.get().await;
    let root = buffer_root(&state.config, &runtime);

    let stale = state
        .livetv
        .tunes
        .get(&key)
        .filter(|t| t.health.is_dead())
        .map(|t| t.id);
    if let Some(id) = stale {
        retire_tune(state, &key, id);
    }
    end_reclaimed_lingerers(state);
    let existing = state.livetv.tunes.contains_key(&key);
    if !existing {
        let lease = state.livetv.leases.acquire(LeaseRequest {
            tuner_id: tuner.id,
            channel_id,
            share_key: key.clone(),
            capacity: tuner.tuner_count.max(1) as u32,
            holder_kind: HolderKind::Viewer,
            session_id: Some(session_id.to_string()),
            recording_id: None,
            user_id: Some(user_id),
        })?;
        // An acquire that needed a lingering tune's slot has just reclaimed it, and the
        // device only frees that tuner once its connection closes.
        end_reclaimed_lingerers(state);
        if lease.fresh {
            TunerLeases::persist_new(&state.pool, &lease.info).await?;
        }
        // Per tune, not per lease: a flick back to the channel just left revives that
        // channel's lingering lease and would land the returning tune on the directory
        // the outgoing one's discard is still deleting — with the segmenter being killed
        // free to drop a stray segment into it on the way out. Nothing reads these names,
        // and an abandoned one is swept as an orphan.
        let buffer_dir = root.join(format!("{}-{}", lease.id(), Uuid::now_v7()));
        tokio::fs::create_dir_all(&buffer_dir)
            .await
            .map_err(|e| SessionError::Other(anyhow!("creating buffer dir: {e}")))?;
        let candidates = source_candidates(state, &channel, &tuner).await;
        let cancel = CancellationToken::new();
        let health = Arc::new(TuneHealth::default());

        // Decision 12.4: the segmenter starts on tune, so the player attaches to a
        // buffer that is already filling — and it runs for DirectPlay too, which
        // is what makes pause, Restart and a retroactive Record work there.
        let fanout = Arc::new(Fanout::default());
        let transcode = start_segmenter(
            state,
            &buffer_dir,
            candidates,
            &facts,
            &runtime,
            cancel.clone(),
            fanout.clone(),
            health.clone(),
        )
        .await?;

        // Scrub thumbnails over the buffer, generated as it fills. The tuner is the
        // only source of this footage, so anything that wants to look at what has
        // already aired has to be fed from here — trickplay is just the first.
        if runtime.livetv_buffer_thumbnails {
            tokio::spawn(trickplay::run(
                buffer_dir.clone(),
                buffer_dir.join("trickplay"),
                cancel.clone(),
            ));
        }

        let tune_id = Uuid::now_v7();
        {
            let state = state.clone();
            let key = key.clone();
            let health = health.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move {
                tokio::select! {
                    _ = health.wait_dead() => retire_tune(&state, &key, tune_id),
                    _ = cancel.cancelled() => {}
                }
            });
        }
        state.livetv.tunes.insert(
            key.clone(),
            LiveTune {
                id: tune_id,
                key: key.clone(),
                channel_id,
                tuner_id: tuner.id,
                lease,
                buffer_dir,
                started_at: now_pg(),
                attached: HashSet::new(),
                recording_id: None,
                transcode: Some(transcode),
                cancel,
                fanout,
                health,
            },
        );
    }

    let health = state.livetv.tunes.get(&key).map(|t| t.health.clone());
    let now =
        livetv_guide::now_and_next(&state.pool, channel_id, OffsetDateTime::now_utc()).await?;
    let guide_now = now.0;

    let session_state = SessionState {
        tune_key: key.clone(),
        channel_id,
        user_id,
        started_at: now_pg(),
        guide_id: guide_now.as_ref().map(|g| g.id),
        title: guide_now.as_ref().map(|g| g.title.clone()),
        series_id: guide_now.as_ref().and_then(|g| g.series_id.clone()),
        episode_key: guide_now.as_ref().and_then(|g| g.episode_key.clone()),
        source: "live".to_string(),
        last_seen: std::time::Instant::now(),
        failure: None,
    };

    // Attached and recorded together, with nothing awaited between: the sweeper lets go
    // of a tune whose attached sessions it cannot find.
    let (lease_info, buffer_dir, started_at) = {
        let Some(mut tune) = state.livetv.tunes.get_mut(&key) else {
            return Err(match health.and_then(|h| h.reason()) {
                Some(reason) => SessionError::TuneFailed(reason),
                None => SessionError::Other(anyhow!("tune vanished")),
            });
        };
        if !tune.lease.revive() {
            warn!(key = %key, "a lingering tune's slot was reclaimed as it was rejoined");
        }
        tune.attached.insert(session_id.to_string());
        state
            .livetv
            .sessions
            .insert(session_id.to_string(), session_state);
        (
            tune.lease.info.clone(),
            tune.buffer_dir.clone(),
            tune.started_at,
        )
    };

    let buffer_dir_s = buffer_dir.to_string_lossy().to_string();
    if retune {
        livetv_sessions::retune(
            &state.pool,
            session_id,
            channel_id,
            Some(lease_info.id),
            mode.as_str(),
            &buffer_dir_s,
        )
        .await?;
    } else {
        livetv_sessions::create(
            &state.pool,
            livetv_sessions::CreateSession {
                session_id: session_id.to_string(),
                channel_id,
                lease_id: Some(lease_info.id),
                user_id: Some(user_id),
                mode: mode.as_str().to_string(),
                buffer_dir: buffer_dir_s,
            },
        )
        .await?;
    }

    // The segmenter runs for a direct tune too, which is what makes pause, Restart
    // and a retroactive Record work there — but withholding the playlist left the
    // client with nothing to switch to, so those controls sat there permanently
    // disabled. A direct session gets both: the raw stream for the live edge, the
    // playlist for anything behind it.
    let playlist_url = Some(format!(
        "/api/v1/admin/livetv/sessions/{session_id}/stream.m3u8"
    ));
    let direct_url = match mode {
        Mode::Transcode => None,
        Mode::Direct => Some(format!(
            "/api/v1/admin/livetv/channels/{channel_id}/stream.ts?session={session_id}"
        )),
    };

    Ok(LiveSessionResult {
        session_id: session_id.to_string(),
        channel_id,
        mode,
        playlist_url,
        direct_url,
        facts,
        buffer_start_time: started_at,
        lease: lease_info,
        tuners_in_use: state.livetv.leases.in_use(tuner.id),
        tuner_count: tuner.tuner_count.max(1) as u32,
    })
}

/// Output size for the web transcode, capped by the configured height.
///
/// The aspect comes from the source: SD DVB-T is 720x576, and forcing it to 16:9
/// stretches every 4:3 channel. Widths are kept even because the encoders reject
/// odd ones on yuv420p.
fn transcode_dimensions(facts: &StreamFacts, cap: i64) -> (u32, u32) {
    let height = (cap.clamp(240, 2160) as u32).min(facts.height.max(2));
    let width = if facts.height == 0 {
        height * 16 / 9
    } else {
        (height as f64 * facts.width as f64 / facts.height as f64).round() as u32
    };
    (width + (width % 2), height + (height % 2))
}

/// Where the feeder hands bytes to the segmenter. Swapped when a segmenter is restarted,
/// and emptied when the feeder stops so a segmenter waiting on input sees the end of it.
type SegmenterSink = Arc<std::sync::Mutex<Option<std::sync::mpsc::Sender<Vec<u8>>>>>;

/// A broadcast that jumps its timestamps back — a programme change, a splice — makes the
/// muxer refuse the next packet and the segmenter die. It is restarted into the same
/// directory, where the DVR playlist is appended to behind a discontinuity, so the buffer
/// carries on. One that keeps dying is not a glitch and takes the tune down.
const SEGMENTER_RESTARTS: usize = 3;
const SEGMENTER_RESTART_WINDOW: Duration = Duration::from_secs(60);

type SegmenterRun = tokio::task::JoinHandle<kob_ffmpeg::Result<()>>;

#[allow(clippy::too_many_arguments)]
async fn start_segmenter(
    state: &crate::AppState,
    buffer_dir: &Path,
    candidates: Vec<SourceCandidate>,
    facts: &StreamFacts,
    runtime: &crate::runtime_config::RuntimeConfig,
    cancel: CancellationToken,
    fanout: Arc<Fanout>,
    health: Arc<TuneHealth>,
) -> Result<TranscodeProcess> {
    let playlist_path = buffer_dir.join("stream.m3u8");
    let (width, height) = transcode_dimensions(facts, runtime.livetv_transcode_height);

    let encoder = state
        .encoder_caps
        .encoder_for("h264")
        .unwrap_or(&state.encoder_caps.h264)
        .to_string();
    let deinterlace = runtime.livetv_deinterlace_web;
    let hw = state.hw_device.get();
    let dir = buffer_dir.to_path_buf();

    let launch = {
        let dir = dir.clone();
        let cancel = cancel.clone();
        move || -> (std::sync::mpsc::Sender<Vec<u8>>, SegmenterRun) {
            let mut config = kob_ffmpeg::transcode::HlsConfig::new(&dir)
                .segment_duration(SEGMENT_SECONDS)
                .video_encoder(&encoder)
                .video_bitrate(video_bitrate_for(height))
                .audio_encoder(crate::streaming::decision::aac_encoder())
                .audio_bitrate(128_000)
                .hls_window(kob_ffmpeg::transcode::HlsWindow::Dvr);
            // x264's default `medium` spends rc_lookahead=40 frames — 0.8 s at 50 fps —
            // before the first segment exists, which is most of Decision 12's budget.
            // The strings are passed through verbatim, so NVENC (which wants p1-p7) must
            // keep its own defaults.
            if encoder == "libx264" {
                config = config.preset("veryfast").tune("zerolatency");
            }
            if deinterlace {
                config = config.deinterlace(kob_ffmpeg::filter::graph::Deinterlace::Auto);
            }
            config.scale_width = Some(width as i32);
            config.scale_height = Some(height as i32);
            config.live_source = true;
            config.run = Some(kob_ffmpeg::progress::RunContext::cancellable(
                cancel.clone(),
            ));
            if let Some(hw) = hw.clone() {
                config = config.hw_device(hw);
            }

            let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
            let t0 = std::time::Instant::now();
            let run = tokio::task::spawn_blocking(move || {
                let mut io = kob_ffmpeg::format::io_context::CustomIoContext::from_channel(rx)?;
                let mut ctx = kob_ffmpeg::format::input::InputContext::open_custom(&mut io)?;
                // Measured: this returns in ~1.1 s of stream on a real DVB-T service and
                // capping probesize/analyzeduration below that buys no latency at all but
                // silently drops the AC-3 track, so the ffmpeg defaults stand.
                ctx.find_stream_info()?;
                info!(
                    analysed_ms = t0.elapsed().as_millis(),
                    "live TV input analysed"
                );
                kob_ffmpeg::transcode::transcode_to_hls_from_ctx(ctx, &config).map(|_| ())
            });
            (tx, run)
        }
    };

    let (tx, first) = launch();
    let sink: SegmenterSink = Arc::new(std::sync::Mutex::new(Some(tx)));
    spawn_feeder(
        sink.clone(),
        fanout,
        candidates,
        cancel.clone(),
        std::time::Instant::now(),
        health.clone(),
    );

    let supervisor_cancel = cancel.clone();
    let handle = tokio::spawn(async move {
        let mut running = first;
        let mut restarts: std::collections::VecDeque<std::time::Instant> = Default::default();
        loop {
            let outcome = running.await;
            if supervisor_cancel.is_cancelled() || health.is_dead() {
                return outcome.unwrap_or(Ok(()));
            }
            // A segmenter that dies leaves the buffer frozen and nothing else, so its
            // error has to reach the log or the failure is invisible.
            match &outcome {
                Ok(Err(e)) => warn!(error = %e, dir = %dir.display(), "live TV segmenter failed"),
                Err(e) => warn!(error = %e, dir = %dir.display(), "live TV segmenter panicked"),
                Ok(Ok(())) => warn!(dir = %dir.display(), "live TV segmenter stopped"),
            }
            let now = std::time::Instant::now();
            restarts.retain(|t| now.duration_since(*t) < SEGMENTER_RESTART_WINDOW);
            if restarts.len() >= SEGMENTER_RESTARTS {
                health.die("The live buffer for this channel kept failing".to_string());
                return outcome.unwrap_or(Ok(()));
            }
            restarts.push_back(now);
            let (tx, next) = launch();
            if let Ok(mut current) = sink.lock() {
                *current = Some(tx);
            }
            info!(dir = %dir.display(), restarts = restarts.len(), "live TV segmenter restarted");
            running = next;
        }
    });

    info!(dir = %buffer_dir.display(), "live TV segmenter started");
    Ok(TranscodeProcess {
        backend: TranscodeBackend::Library { handle, cancel },
        segment_dir: buffer_dir.to_path_buf(),
        playlist_path,
        stream_health: None,
        started_at: std::time::Instant::now(),
    })
}

/// HTTP bytes -> both sinks. Neither consumer can stall the other: no subscribers is
/// the normal case for a web-only tune, and a dead segmenter is restarted behind it.
/// The feeder stops on cancellation, an exhausted reconnect budget or a mux with no
/// signal; the last two kill the tune, which is torn down rather than left for the
/// next viewer of the channel to join.
///
/// Until the first byte arrives each attempt walks every candidate; after it, only the
/// one that delivered is reconnected to, because another member can be a different
/// codec and the segmenter has already analysed this one.
fn spawn_feeder(
    sink: SegmenterSink,
    fanout: Arc<Fanout>,
    candidates: Vec<SourceCandidate>,
    cancel: CancellationToken,
    t0: std::time::Instant,
    health: Arc<TuneHealth>,
) {
    tokio::spawn(async move {
        let death = feed(&sink, &fanout, &candidates, &cancel, t0, &health).await;
        if let Some(reason) = death {
            health.die(reason);
        }
        if let Ok(mut current) = sink.lock() {
            *current = None;
        }
    });
}

/// `None` when cancelled, otherwise why the feed stopped for good.
async fn feed(
    sink: &SegmenterSink,
    fanout: &Fanout,
    candidates: &[SourceCandidate],
    cancel: &CancellationToken,
    t0: std::time::Instant,
    health: &TuneHealth,
) -> Option<String> {
    let Some(requested) = candidates.first().map(|c| c.name.clone()) else {
        return Some("This channel has no number the tuner can tune".to_string());
    };
    let http = crate::http::new_client();
    let mut attempt = 0usize;
    let mut locked: Option<usize> = None;
    loop {
        if cancel.is_cancelled() {
            return None;
        }
        let pool = match locked {
            Some(i) => &candidates[i..=i],
            None => candidates,
        };
        match tuner_source::open_first(&http, pool, FIRST_BYTE_DEADLINE, cancel).await {
            Ok(opened) => {
                attempt = 0;
                if locked.is_none() {
                    let index = opened.index;
                    locked = Some(index);
                    health.mark_delivered();
                    info!(ttfb_ms = t0.elapsed().as_millis(), "live TV tuner open");
                    if index > 0 {
                        info!(
                            requested = %requested,
                            alternate = %candidates[index].name,
                            alternate_id = %candidates[index].channel_id,
                            url = %candidates[index].url,
                            "live TV tuned a stacked alternate"
                        );
                    }
                }
                let mut chunk = Some(opened.first);
                let mut body = opened.body;
                loop {
                    if let Some(chunk) = chunk.take() {
                        let packets = fanout.send(&chunk);
                        if !packets.is_empty()
                            && let Ok(mut current) = sink.lock()
                        {
                            let gone = current
                                .as_ref()
                                .is_some_and(|tx| tx.send(packets.to_vec()).is_err());
                            if gone {
                                *current = None;
                            }
                        }
                    }
                    let next = tokio::select! {
                        _ = cancel.cancelled() => return None,
                        n = body.next() => n,
                    };
                    match next {
                        Some(Ok(c)) => chunk = Some(c),
                        Some(Err(e)) => {
                            warn!(error = %e, "live TV source read error");
                            break;
                        }
                        None => break,
                    }
                }
            }
            Err(errors) => {
                if cancel.is_cancelled() {
                    return None;
                }
                if locked.is_none() {
                    health.fail(tuner_source::failure_message(&requested, &errors));
                }
                // The device already spent ~10 s looking for a lock before it said
                // so; dialling the same mux again only makes the viewer wait longer.
                if !errors.is_empty() && errors.iter().all(|e| e.is_no_signal()) {
                    warn!(channel = %requested, "live TV source has no signal, giving up");
                    return Some(match locked {
                        Some(_) => format!("{requested} has lost its signal"),
                        None => tuner_source::failure_message(&requested, &errors),
                    });
                }
            }
        }

        // Never re-probe on reconnect: the parameters are the ones the scan
        // already recorded.
        let Some(delay) = RECONNECT_DELAYS_MS.get(attempt).copied() else {
            warn!(channel = %requested, "live TV source reconnect budget exhausted");
            return Some(
                health
                    .failure()
                    .unwrap_or_else(|| format!("The tuner stopped sending {requested}")),
            );
        };
        attempt += 1;
        tokio::select! {
            _ = cancel.cancelled() => return None,
            _ = tokio::time::sleep(Duration::from_millis(delay)) => {}
        }
    }
}

/// How long a direct client riding a tune waits for its first byte, before which the
/// response status cannot yet say whether the tune worked.
const DIRECT_FIRST_BYTE_WAIT: Duration = Duration::from_secs(15);

/// Subscribe to the tune's byte fan-out. The tuner connection is already open and
/// already flowing, so this is the live edge with no second connection and no
/// probe; a session with no tune left falls back to opening the device itself.
///
/// Errors are the viewer-facing reason the tune never produced a byte.
pub async fn take_direct_stream(
    state: &crate::AppState,
    session_id: Option<&str>,
    candidates: &[SourceCandidate],
) -> std::result::Result<(ByteStream, String), String> {
    let riding = session_id
        .and_then(|sid| tune_for_session(state, sid))
        .and_then(|key| {
            state
                .livetv
                .tunes
                .get(&key)
                .map(|t| (t.fanout.clone(), t.health.clone()))
        });
    if let Some((fanout, health)) = riding {
        // From the latest keyframe rather than the live edge: the decoder can show
        // nothing until one arrives, and a join lands anywhere in a GOP. Joined before
        // the wait, so a tune still opening is had from its first byte.
        let (keyframe, rx) = fanout.join();
        health.first_byte(DIRECT_FIRST_BYTE_WAIT).await?;
        let catch_up = futures_util::stream::iter((!keyframe.is_empty()).then_some(Ok(keyframe)));
        let live = tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(|item| async {
            match item {
                Ok(chunk) => Some(Ok(chunk)),
                // A client that fell behind resyncs at the live edge rather than
                // holding the tune back for everyone else.
                Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                    warn!(dropped = n, "live TV direct client lagged");
                    None
                }
            }
        });
        return Ok((
            Box::pin(catch_up.chain(live)) as ByteStream,
            "video/mp2t".to_string(),
        ));
    }

    let requested = candidates
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_default();
    match tuner_source::open_first(
        &crate::http::new_client(),
        candidates,
        FIRST_BYTE_DEADLINE,
        &CancellationToken::new(),
    )
    .await
    {
        Ok(opened) => {
            if opened.index > 0 {
                info!(
                    requested = %requested,
                    alternate = %candidates[opened.index].name,
                    "live TV direct stream tuned a stacked alternate"
                );
            }
            let first = futures_util::stream::once(async move { Ok(opened.first) });
            Ok((
                Box::pin(first.chain(opened.body)) as ByteStream,
                opened.content_type,
            ))
        }
        Err(errors) => Err(tuner_source::failure_message(&requested, &errors)),
    }
}

fn detach(state: &crate::AppState, session_id: &str) {
    let Some((_, session)) = state.livetv.sessions.remove(session_id) else {
        return;
    };
    detach_from_tune(state, session_id, &session.tune_key);
}

/// Drop [session_id]'s claim on [tune_key], tearing the tune down if it was the last.
///
/// Nothing here is awaited. Both halves of a teardown are unbounded from the caller's
/// point of view — the segmenter only notices its cancellation between frames, and the
/// buffer is up to three hours of them — and a channel change was waiting on the pair
/// before it could ask for the next tune.
fn detach_from_tune(state: &crate::AppState, session_id: &str, tune_key: &str) {
    let mut teardown: Option<LiveTune> = None;
    if let Some(mut tune) = state.livetv.tunes.get_mut(tune_key) {
        tune.attached.remove(session_id);
        // A promoted buffer outlives its viewer: the segments are the recording.
        if tune.attached.is_empty() && tune.recording_id.is_none() {
            drop(tune);
            teardown = state.livetv.tunes.remove(tune_key).map(|(_, t)| t);
        }
    }
    if let Some(tune) = teardown {
        dispose(tune, false);
    }
}

/// [detach_from_tune] for a channel change: a tune left with nobody on it stays up for
/// the lease's linger rather than coming down under the player still reading it.
fn leave_lingering(state: &crate::AppState, session_id: &str, tune_key: &str) {
    let lingering = {
        let Some(mut tune) = state.livetv.tunes.get_mut(tune_key) else {
            return;
        };
        tune.attached.remove(session_id);
        if !tune.attached.is_empty() || tune.recording_id.is_some() {
            return;
        }
        let since = tune.lease.linger();
        (tune.id, since, tune.lease.flags.clone())
    };
    let (id, since, flags) = lingering;
    let state = state.clone();
    let key = tune_key.to_string();
    tokio::spawn(async move {
        tokio::select! {
            _ = flags.ended() => {}
            _ = tokio::time::sleep(LEASE_LINGER) => {}
        }
        end_linger(&state, &key, id, since);
    });
}

fn end_linger(state: &crate::AppState, key: &str, id: Uuid, since: std::time::Instant) {
    let ended = state.livetv.tunes.remove_if(key, |_, t| {
        t.id == id && t.attached.is_empty() && t.lease.dormant_since() == Some(since)
    });
    if let Some((_, tune)) = ended {
        dispose(tune, false);
    }
}

/// Lingering tunes whose slot an acquire has taken back. Ended here rather than left to
/// their own timers, so the tuner is free before the acquirer dials the device.
fn end_reclaimed_lingerers(state: &crate::AppState) {
    let reclaimed: Vec<(String, Uuid, std::time::Instant)> = state
        .livetv
        .tunes
        .iter()
        .filter(|t| t.lease.flags.is_ended())
        .filter_map(|t| Some((t.key.clone(), t.id, t.lease.dormant_since()?)))
        .collect();
    for (key, id, since) in reclaimed {
        end_linger(state, &key, id, since);
    }
}

/// Stop a tune already taken out of the map, keeping its segments only for a recording.
fn dispose(mut tune: LiveTune, keep_buffer: bool) {
    tune.cancel.cancel();
    let transcode = tune.transcode.take();
    let dir = tune.buffer_dir.clone();
    let key = tune.key.clone();
    // Before the handoff, and synchronously: the lease releases with the tune, and the
    // acquire the next tune makes can only reap a lingering lease that nothing holds.
    drop(tune);
    tokio::spawn(async move {
        if let Some(mut t) = transcode {
            t.kill().await;
        }
        if !keep_buffer {
            discard_buffer(dir, key).await;
        }
    });
}

/// Take a dead tune out of service: nothing may join it, its tuner is released, and
/// every session on it is told why.
fn retire_tune(state: &crate::AppState, key: &str, id: Uuid) {
    let Some((_, tune)) = state.livetv.tunes.remove_if(key, |_, t| t.id == id) else {
        return;
    };
    let reason = tune
        .health
        .reason()
        .unwrap_or_else(|| "The tuner stopped sending this channel".to_string());
    for sid in &tune.attached {
        if let Some(mut session) = state.livetv.sessions.get_mut(sid)
            && session.tune_key == key
        {
            session.failure = Some(reason.clone());
        }
    }
    warn!(key = %key, reason = %reason, "live TV tune died, tuner released");
    let keep = tune.recording_id.is_some();
    dispose(tune, keep);
}

/// Ends sessions nobody has heartbeated for `idle`, then lets go of every tune left
/// holding a session that no longer exists.
pub async fn sweep_sessions(state: &crate::AppState, idle: Duration) {
    let now = std::time::Instant::now();
    let abandoned: Vec<String> = state
        .livetv
        .sessions
        .iter()
        .filter(|s| now.duration_since(s.last_seen) >= idle)
        .map(|s| s.key().clone())
        .collect();
    for sid in abandoned {
        info!(session_id = %sid, "live TV session abandoned, ending it");
        destroy_session(state, &sid).await;
    }

    let orphaned: Vec<(String, Vec<String>)> = state
        .livetv
        .tunes
        .iter()
        .filter(|t| t.recording_id.is_none())
        // A lingering tune's own timer, or the acquire that needs its slot, ends it.
        .filter(|t| t.lease.dormant_since().is_none())
        // A tune is inserted a moment before its first session attaches.
        .filter(|t| !t.attached.is_empty() || t.started_at < now_pg() - EMPTY_TUNE_GRACE)
        .map(|t| {
            let gone: Vec<String> = t
                .attached
                .iter()
                .filter(|sid| {
                    !state
                        .livetv
                        .sessions
                        .get(*sid)
                        .is_some_and(|s| s.tune_key == t.key)
                })
                .cloned()
                .collect();
            (t.key.clone(), gone)
        })
        .collect();
    for (key, gone) in orphaned {
        if gone.is_empty() {
            detach_from_tune(state, "", &key);
        }
        for sid in gone {
            detach_from_tune(state, &sid, &key);
        }
    }
}

pub fn session_failure(state: &crate::AppState, session_id: &str) -> Option<String> {
    state
        .livetv
        .sessions
        .get(session_id)
        .and_then(|s| s.failure.clone())
}

/// Discard a torn-down tune's segments.
///
/// Decision 4 default: the buffer goes on a channel change unless it was promoted to a
/// recording. Up to three hours of segments plus their trickplay, and nothing reads them
/// again — which is why the caller hands this to a task rather than waiting on it.
async fn discard_buffer(dir: PathBuf, key: String) {
    let _ = tokio::fs::remove_dir_all(&dir).await;
    info!(key = %key, "live TV tune torn down");
}

pub async fn destroy_session(state: &crate::AppState, session_id: &str) -> bool {
    let Some(session) = state.livetv.sessions.get(session_id).map(|s| s.clone()) else {
        return false;
    };
    write_watch_event(state, &session).await;
    detach(state, session_id);
    let _ = livetv_sessions::end(&state.pool, session_id).await;
    true
}

/// One row per viewing, written on session end and on channel change.
/// Takes the session by value: the switch has to read it before the new tune overwrites
/// it, which is a beat before this can run.
async fn write_watch_event(state: &crate::AppState, session: &SessionState) {
    let ended_at = now_pg();
    let watched = (ended_at - session.started_at)
        .whole_seconds()
        .clamp(0, i32::MAX as i64) as i32;
    let programme_secs = match session.guide_id {
        Some(id) => livetv_guide::get_by_id(&state.pool, id)
            .await
            .ok()
            .flatten()
            .map(|g| (g.end_time - g.start_time).whole_seconds() as i32),
        None => None,
    };
    if let Err(e) = livetv_watch_events::create(
        &state.pool,
        livetv_watch_events::CreateWatchEvent {
            user_id: Some(session.user_id),
            channel_id: session.channel_id,
            guide_id: session.guide_id,
            series_id: session.series_id.clone(),
            title: session.title.clone(),
            episode_key: session.episode_key.clone(),
            started_at: session.started_at,
            ended_at: Some(ended_at),
            watched_secs: watched,
            programme_secs,
            source: session.source.clone(),
        },
    )
    .await
    {
        warn!(error = %e, "writing live TV watch event failed");
    }
}

pub async fn heartbeat(state: &crate::AppState, session_id: &str, source: Option<&str>) -> bool {
    let Some(mut session) = state.livetv.sessions.get_mut(session_id) else {
        return false;
    };
    session.last_seen = std::time::Instant::now();
    if let Some(src) = source
        && matches!(src, "live" | "buffer" | "recording")
    {
        session.source = src.to_string();
    }
    let tune_key = session.tune_key.clone();
    drop(session);
    if let Some(tune) = state.livetv.tunes.get(&tune_key) {
        state.livetv.leases.heartbeat(tune.lease.id());
    }
    let _ = livetv_sessions::touch(&state.pool, session_id).await;
    true
}

// =============================================================================
// The segment directory IS the buffer
// =============================================================================

#[derive(Debug, Default)]
pub struct BufferStatus {
    pub segments: usize,
    pub duration_seconds: f64,
    pub retained_seconds: f64,
    pub first_segment_index: u64,
    /// Playing time retention has already reaped off the front. The served playlist
    /// is rebased onto the first surviving segment, so anything else describing the
    /// buffer on the same timeline — trickplay's VTT — has to subtract this too.
    pub dropped_seconds: f64,
}

/// Parse ffmpeg's DVR playlist, dropping entries whose files retention already
/// reaped and rebasing MEDIA-SEQUENCE so what we serve stays self-consistent.
///
/// The header is rebuilt rather than copied: ffmpeg's live output declares
/// `TARGETDURATION:0`, opens with a `DISCONTINUITY` that precedes every segment
/// and orders `MEDIA-SEQUENCE` after `EXT-X-MAP`. Browsers tolerate all three
/// and a strict player does not.
pub async fn read_buffer(dir: &Path) -> Option<(String, BufferStatus)> {
    let raw = tokio::fs::read_to_string(dir.join("stream.m3u8"))
        .await
        .ok()?;
    let mut version = 7u32;
    let mut extra: Vec<String> = Vec::new();
    let mut map: Option<String> = None;
    let mut entries: Vec<(String, f64, String, bool)> = Vec::new();
    let mut pending: Option<(String, f64)> = None;
    let mut pending_discontinuity = false;
    let mut base_sequence: u64 = 0;
    let mut seen_first = false;

    for line in raw.lines() {
        if let Some(v) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            base_sequence = v.trim().parse().unwrap_or(0);
            continue;
        }
        if let Some(v) = line.strip_prefix("#EXT-X-VERSION:") {
            version = v.trim().parse().unwrap_or(7);
            continue;
        }
        if line.starts_with("#EXT-X-TARGETDURATION")
            || line.starts_with("#EXT-X-PLAYLIST-TYPE")
            || line.starts_with("#EXT-X-ENDLIST")
            || line == "#EXTM3U"
        {
            continue;
        }
        if line.starts_with("#EXT-X-MAP:") {
            map = Some(line.replace("URI=\"init.mp4\"", "URI=\"segments/init.mp4\""));
            continue;
        }
        if line == "#EXT-X-DISCONTINUITY" {
            pending_discontinuity = true;
            continue;
        }
        if let Some(v) = line.strip_prefix("#EXTINF:") {
            let secs = v.trim_end_matches(',').trim().parse::<f64>().unwrap_or(0.0);
            pending = Some((line.to_string(), secs));
            continue;
        }
        if line.starts_with('#') {
            if !seen_first {
                extra.push(line.to_string());
            }
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        seen_first = true;
        if let Some((extinf, secs)) = pending.take() {
            entries.push((
                extinf,
                secs,
                line.trim().to_string(),
                std::mem::take(&mut pending_discontinuity),
            ));
        }
    }

    let mut kept: Vec<(String, f64, String, bool)> = Vec::new();
    let mut dropped: u64 = 0;
    let mut dropped_seconds = 0.0f64;
    for (i, entry) in entries.into_iter().enumerate() {
        if dir.join(&entry.2).exists() {
            kept.push(entry);
        } else if kept.is_empty() {
            dropped = i as u64 + 1;
            dropped_seconds += entry.1;
        }
    }

    let duration: f64 = kept.iter().map(|e| e.1).sum();
    let target = kept.iter().fold(1.0f64, |acc, e| acc.max(e.1)).ceil() as u64;
    let mut out = vec![
        "#EXTM3U".to_string(),
        format!("#EXT-X-VERSION:{version}"),
        format!("#EXT-X-TARGETDURATION:{target}"),
        // Retention reaps from the front, which EXT-X-PLAYLIST-TYPE:EVENT forbids —
        // a plain live playlist with a rebased sequence says the same thing legally.
        format!("#EXT-X-MEDIA-SEQUENCE:{}", base_sequence + dropped),
    ];
    out.extend(extra);
    out.extend(map);
    for (i, (extinf, _, file, discontinuity)) in kept.iter().enumerate() {
        if *discontinuity && i > 0 {
            out.push("#EXT-X-DISCONTINUITY".to_string());
        }
        out.push(extinf.clone());
        out.push(format!("segments/{file}"));
    }
    let status = BufferStatus {
        segments: kept.len(),
        duration_seconds: duration,
        retained_seconds: duration,
        first_segment_index: base_sequence + dropped,
        dropped_seconds,
    };
    Some((out.join("\n") + "\n", status))
}

/// Keep-from-tune-in, capped. Segments belonging to a promoted buffer are never
/// reaped — they are the recording.
pub async fn enforce_retention(dir: &Path, cap: Duration) -> usize {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return 0;
    };
    let cutoff = std::time::SystemTime::now() - cap;
    let mut removed = 0;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("seg_") {
            continue;
        }
        let Ok(meta) = entry.metadata().await else {
            continue;
        };
        if meta.modified().is_ok_and(|m| m < cutoff) {
            let _ = tokio::fs::remove_file(&path).await;
            removed += 1;
        }
    }
    removed
}

/// `livetv_buffer_policy`: `tune_in` and `rolling` are the same thing once a cap
/// exists — you can always go back `cap` hours. `programme` is the one that
/// differs: it keeps only back to the start of what is on now.
async fn retention_cap(
    state: &crate::AppState,
    runtime: &crate::runtime_config::RuntimeConfig,
    channel_id: Uuid,
    hour_cap: Duration,
) -> Duration {
    if runtime.livetv_buffer_policy != "programme" {
        return hour_cap;
    }
    let now = OffsetDateTime::now_utc();
    match livetv_guide::now_and_next(&state.pool, channel_id, now).await {
        Ok((Some(current), _)) => {
            let since = (now - current.start_time).unsigned_abs();
            since.min(hour_cap)
        }
        _ => hour_cap,
    }
}

/// Periodic sweeper: reap lingering leases, apply the retention cap, and delete
/// buffer directories nothing references.
pub fn spawn_sweeper(state: crate::AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        loop {
            tick.tick().await;
            sweep_sessions(&state, SESSION_IDLE).await;
            state.livetv.leases.reap_expired();

            let runtime = state.runtime_config.get().await;
            let hour_cap =
                Duration::from_secs((runtime.livetv_buffer_cap_hours.clamp(1, 24) as u64) * 3600);
            let tunes: Vec<(PathBuf, bool, Uuid)> = state
                .livetv
                .tunes
                .iter()
                .map(|t| (t.buffer_dir.clone(), t.recording_id.is_some(), t.channel_id))
                .collect();
            let live: HashSet<PathBuf> = tunes.iter().map(|(d, _, _)| d.clone()).collect();
            for (dir, promoted, channel_id) in tunes {
                if promoted {
                    continue;
                }
                let cap = retention_cap(&state, &runtime, channel_id, hour_cap).await;
                enforce_retention(&dir, cap).await;
            }

            let root = buffer_root(&state.config, &runtime);
            super::tuner_lease::sweep_orphan_buffers(&state.pool, &root, &live).await;
        }
    });
}

pub fn tune_for_session(state: &crate::AppState, session_id: &str) -> Option<String> {
    state
        .livetv
        .sessions
        .get(session_id)
        .map(|s| s.tune_key.clone())
}

pub async fn promote_to_recording(
    state: &crate::AppState,
    session_id: &str,
    recording_id: Uuid,
) -> std::result::Result<(), SessionError> {
    let key = tune_for_session(state, session_id).ok_or(SessionError::SessionNotFound)?;
    let (tuner_id, channel_id) = {
        let mut tune = state
            .livetv
            .tunes
            .get_mut(&key)
            .ok_or(SessionError::SessionNotFound)?;
        tune.recording_id = Some(recording_id);
        (tune.tuner_id, tune.channel_id)
    };
    let tuner = livetv_tuners::get_by_id(&state.pool, tuner_id)
        .await?
        .ok_or(SessionError::TunerNotFound)?;
    // Recording outranks viewer: re-acquire the same share key so the slot stops
    // being evictable, then drop the extra holder immediately.
    let token = state.livetv.leases.acquire(LeaseRequest {
        tuner_id,
        channel_id,
        share_key: key.clone(),
        capacity: tuner.tuner_count.max(1) as u32,
        holder_kind: HolderKind::Recording,
        session_id: Some(session_id.to_string()),
        recording_id: Some(recording_id),
        user_id: None,
    })?;
    if token.fresh {
        TunerLeases::persist_new(&state.pool, &token.info).await?;
    }
    drop(token);
    livetv_sessions::set_recording(&state.pool, session_id, recording_id).await?;
    Ok(())
}

/// When the buffer begins == when we tuned. Record is retroactive to this.
pub fn buffer_start_of(state: &crate::AppState, session_id: &str) -> Option<OffsetDateTime> {
    let key = tune_for_session(state, session_id)?;
    state.livetv.tunes.get(&key).map(|t| t.started_at)
}

pub fn buffer_dir_of(state: &crate::AppState, session_id: &str) -> Option<PathBuf> {
    let key = tune_for_session(state, session_id)?;
    state.livetv.tunes.get(&key).map(|t| t.buffer_dir.clone())
}

pub fn tune_health_of(state: &crate::AppState, session_id: &str) -> Option<Arc<TuneHealth>> {
    let key = tune_for_session(state, session_id)?;
    state.livetv.tunes.get(&key).map(|t| t.health.clone())
}

/// Whether [session_id] names a tune that is already open.
///
/// This is the same lookup [take_direct_stream] makes before it decides to ride an
/// existing tune rather than open a tuner, and callers gate on it to tell the two
/// apart — keep them agreeing.
pub fn has_open_tune(state: &crate::AppState, session_id: &str) -> bool {
    tune_for_session(state, session_id).is_some_and(|key| state.livetv.tunes.contains_key(&key))
}

/// Gives up early once `health` says the tune will never produce a segment.
pub async fn wait_for_first_segment(
    dir: &Path,
    timeout: Duration,
    health: Option<&TuneHealth>,
) -> Option<(String, BufferStatus)> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some((playlist, status)) = read_buffer(dir).await
            && status.segments > 0
        {
            return Some((playlist, status));
        }
        if std::time::Instant::now() >= deadline || health.is_some_and(|h| h.failure().is_some()) {
            return read_buffer(dir).await;
        }
        // Poll fast: the whole budget from tune to first frame is 1.2-1.8 s.
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(width: u32, height: u32) -> StreamFacts {
        StreamFacts {
            video_codec: "mpeg2video".to_string(),
            audio_codec: "ac3".to_string(),
            width,
            height,
            hd: height >= 720,
            interlaced: true,
        }
    }

    /// The discard is handed to a task so a channel change does not wait on it. That is
    /// only sound if it still happens: the buffer is gigabytes of segments per tune, and
    /// a tuner that leaks one per channel press fills the disk in an evening.
    #[tokio::test]
    async fn a_buffer_handed_off_for_discard_is_still_deleted() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("lease-1");
        tokio::fs::create_dir_all(dir.join("trickplay"))
            .await
            .unwrap();
        tokio::fs::write(dir.join("seg_00000.m4s"), b"segment")
            .await
            .unwrap();

        discard_buffer(dir.clone(), "c:test".to_string()).await;

        assert!(
            !dir.exists(),
            "the tune's buffer outlived the channel change"
        );
    }

    /// SD DVB-T is 720x576. Deriving the width from a hardcoded 16:9 stretched
    /// every 4:3 channel to 1024 wide, and re-scaled a stream that needed no scale.
    #[test]
    fn the_transcode_keeps_the_source_aspect() {
        assert_eq!(transcode_dimensions(&facts(720, 576), 1080), (720, 576));
        assert_eq!(transcode_dimensions(&facts(1920, 1080), 1080), (1920, 1080));
    }

    #[test]
    fn the_height_cap_scales_both_axes() {
        assert_eq!(transcode_dimensions(&facts(1920, 1080), 720), (1280, 720));
        assert_eq!(transcode_dimensions(&facts(720, 576), 480), (600, 480));
    }

    /// Never upscale: the cap is a ceiling, not a target.
    #[test]
    fn a_cap_above_the_source_leaves_it_alone() {
        assert_eq!(transcode_dimensions(&facts(720, 576), 2160), (720, 576));
    }

    #[test]
    fn every_dimension_stays_even() {
        for h in [241, 359, 483, 577, 721, 1081] {
            let (w, out_h) = transcode_dimensions(&facts(1920, 1080), h);
            assert_eq!(w % 2, 0, "width {w} for cap {h}");
            assert_eq!(out_h % 2, 0, "height {out_h} for cap {h}");
        }
    }

    /// kobapp's list, verbatim — it asks for everything libVLC can open.
    fn tuner_client() -> ClientCapabilities {
        ClientCapabilities {
            video_codecs: ["h264", "hevc", "mpeg2video", "vc1"]
                .map(str::to_string)
                .to_vec(),
            audio_codecs: ["aac", "ac3", "eac3", "mp3"].map(str::to_string).to_vec(),
            containers: ["mp4", "mkv", "ts"].map(str::to_string).to_vec(),
            supports_interlaced: None,
            max_height: None,
        }
    }

    fn tuner_facts(video: &str, audio: &str, hd: bool) -> StreamFacts {
        StreamFacts {
            video_codec: video.to_string(),
            audio_codec: audio.to_string(),
            width: if hd { 1920 } else { 720 },
            height: if hd { 1080 } else { 576 },
            hd,
            interlaced: true,
        }
    }

    /// The direct path exists for a client that reads MPEG-TS natively, and a
    /// field-coded H.264 service is exactly what it is for.
    #[test]
    fn a_client_that_reads_mpegts_takes_an_h264_service_raw() {
        let caps = tuner_client();
        assert_eq!(
            decide_mode(&tuner_facts("h264", "ac3", true), &caps),
            Mode::Direct
        );
        assert_eq!(
            decide_mode(&tuner_facts("h264", "aac", false), &caps),
            Mode::Direct
        );
    }

    /// MPEG-TS says the client can read the broadcast, not that its decoder can show it.
    /// The Tensor VPU errors on every field picture, so kobapp on one says so — and is
    /// handed the GPU's deinterlaced encode, which its hardware decodes, rather than a
    /// raw stream it could only decode on the CPU.
    #[test]
    fn a_client_that_cannot_decode_fields_gets_a_deinterlaced_encode() {
        let caps = ClientCapabilities {
            supports_interlaced: Some(false),
            ..tuner_client()
        };
        assert_eq!(
            decide_mode(&tuner_facts("h264", "ac3", true), &caps),
            Mode::Transcode
        );
        let progressive = StreamFacts {
            interlaced: false,
            ..tuner_facts("h264", "ac3", true)
        };
        assert_eq!(decide_mode(&progressive, &caps), Mode::Direct);
    }

    /// MPEG-2 direct play was blocked for a while on the theory that libVLC could
    /// not take it. It can — the shipped Android AAR carries a full FFmpeg with a
    /// registered mpeg2video decoder — and the real fault was kobapp's auth proxy
    /// relaying `Transfer-Encoding: chunked` over a body it had already decoded,
    /// which broke every direct stream regardless of codec. Transcoding SD MPEG-2
    /// is pure overhead, so the direct path keeps it.
    #[test]
    fn an_mpeg2_service_still_direct_plays_for_a_client_that_asked_for_it() {
        let caps = tuner_client();
        assert_eq!(
            decide_mode(&tuner_facts("mpeg2video", "aac", false), &caps),
            Mode::Direct
        );
        // MP2 is not in that client's list, so this one still has to be encoded.
        assert_eq!(
            decide_mode(&tuner_facts("mpeg2video", "mp2", false), &caps),
            Mode::Transcode
        );
    }

    /// A browser is the reason the transcode path exists; nothing about the
    /// carve-out may hand it a raw tuner stream either.
    #[test]
    fn a_browser_still_transcodes_everything() {
        let browser = ClientCapabilities {
            video_codecs: vec!["h264".into()],
            audio_codecs: vec!["aac".into()],
            containers: vec!["mp4".into()],
            supports_interlaced: None,
            max_height: None,
        };
        assert_eq!(
            decide_mode(&tuner_facts("h264", "aac", true), &browser),
            Mode::Transcode
        );
        assert_eq!(
            decide_mode(&tuner_facts("mpeg2video", "mp2", false), &browser),
            Mode::Transcode
        );
    }
}
