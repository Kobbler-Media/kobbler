//! Recording capture (Decisions 5 and 13).
//!
//! A recording is HLS segments, never `_partN.mp4`: in-progress playback,
//! seek-to-start and incremental analysis all fall out of that for free. Each
//! capture attempt writes its own `part_NNN/` directory, so a reboot mid-window
//! appends a part rather than starting over, and the served playlist is the
//! parts stitched together with `EXT-X-DISCONTINUITY`.

use std::path::{Path, PathBuf};
use std::time::Duration;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use crate::repositories::livetv_recordings;

/// Longer than the live buffer's 1 s: nothing here is chasing first-frame
/// latency, and four-second segments quarter the playlist and directory churn
/// over a three-hour capture.
const SEGMENT_SECONDS: f64 = 4.0;

/// A recording row is only safe to append to once its previous worker is gone.
/// The handler heartbeats every 30 s, so three missed beats means dead.
pub const RESUME_STALE_SECS: i64 = 90;

/// Live TV writes `{root}/{uuid}/` and IPTV writes `{root}/{uuid}.mp4`, so
/// sharing a root is safe — and one bind-mounted recordings disk is what an
/// operator actually has.
pub async fn recording_root(
    pool: &sqlx::PgPool,
    config: &crate::Config,
    runtime: &crate::runtime_config::RuntimeConfig,
) -> crate::repositories::Result<PathBuf> {
    if let Some(path) = runtime
        .livetv_recording_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Ok(PathBuf::from(path));
    }
    let iptv = crate::repositories::iptv_recordings::get_recording_path(pool).await?;
    Ok(iptv
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&config.storage_root).join("livetv")))
}

pub fn recording_dir(root: &Path, recording_id: Uuid) -> PathBuf {
    root.join(recording_id.to_string())
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// The one failure that aborts loudly instead of retrying: if the disk will
    /// not take bytes, another attempt writes nothing either.
    #[error("recording storage is not writable: {0}")]
    Disk(String),
    #[error("{0}")]
    Source(String),
}

/// Deliberately not a packet count: a pure remux never touches the video encoder,
/// so ffmpeg's `video_packets_written` is always zero on this path and reads as a
/// failed capture.
#[derive(Debug, Default)]
pub struct CaptureOutcome {
    pub segments: usize,
}

async fn probe_writable(dir: &Path) -> Result<(), CaptureError> {
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| CaptureError::Disk(format!("creating {}: {e}", dir.display())))?;
    let probe = dir.join(".writable");
    tokio::fs::write(&probe, b"1")
        .await
        .map_err(|e| CaptureError::Disk(format!("writing to {}: {e}", dir.display())))?;
    let _ = tokio::fs::remove_file(&probe).await;
    Ok(())
}

/// The next unused `part_NNN` under a recording directory.
pub async fn next_part_dir(dir: &Path) -> PathBuf {
    let mut highest: Option<u32> = None;
    if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(n) = name
                .strip_prefix("part_")
                .and_then(|n| n.parse::<u32>().ok())
            {
                highest = Some(highest.map_or(n, |h: u32| h.max(n)));
            }
        }
    }
    let next = highest.map_or(0, |h| h + 1);
    dir.join(format!("part_{next:03}"))
}

/// The teletext data-unit rate is the strongest ad-break signal we have, and it
/// lives on a DVB PID that the fMP4 remux cannot carry — so it is metered off
/// the raw transport stream while the recording is being written, and only the
/// per-second counts survive to disk.
pub const TELETEXT_SIDECAR: &str = "teletext.csv";

/// Full re-opens of the tuner inside one part. ffmpeg is no longer the thing
/// holding the socket, so the reconnect budget is ours.
const RECONNECT_DELAYS_MS: [u64; 5] = [250, 500, 1000, 2000, 4000];

/// Capture one part. A pure remux — the tuner already hands us H.264, and a
/// re-encode would spend a core per recording to lose quality.
pub async fn capture_part(
    stream_url: &str,
    part_dir: &Path,
    max_duration: Duration,
    cancel: CancellationToken,
) -> Result<CaptureOutcome, CaptureError> {
    probe_writable(part_dir).await?;

    let stop = cancel.child_token();
    let finished = CancellationToken::new();
    {
        let stop = stop.clone();
        let finished = finished.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(max_duration) => stop.cancel(),
                _ = finished.cancelled() => {}
            }
        });
    }

    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let (tele_tx, tele_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let meter = spawn_teletext_meter(part_dir.join(TELETEXT_SIDECAR), tele_rx);
    spawn_feeder(tx, tele_tx, stream_url.to_string(), stop.clone());

    let dir = part_dir.to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        let mut config = kob_ffmpeg::transcode::HlsConfig::new(&dir)
            .segment_duration(SEGMENT_SECONDS)
            .hls_window(kob_ffmpeg::transcode::HlsWindow::Dvr);
        config.live_source = true;
        config.run = Some(kob_ffmpeg::progress::RunContext::cancellable(stop));
        let mut io = kob_ffmpeg::format::io_context::CustomIoContext::from_channel(rx)?;
        let mut ctx = kob_ffmpeg::format::input::InputContext::open_custom(&mut io)?;
        ctx.find_stream_info()?;
        kob_ffmpeg::transcode::transcode_to_hls_from_ctx(ctx, &config)
    })
    .await;
    finished.cancel();
    let _ = meter.await;

    match result {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            probe_writable(part_dir).await?;
            return Err(CaptureError::Source(e.to_string()));
        }
        Err(e) => {
            probe_writable(part_dir).await?;
            return Err(CaptureError::Source(format!("capture task panicked: {e}")));
        }
    };

    Ok(CaptureOutcome {
        segments: count_segments(part_dir).await,
    })
}

fn spawn_teletext_meter(
    sidecar: PathBuf,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut meter = crate::livetv::ad_detect::teletext::TeletextMeter::new();
        while let Some(chunk) = rx.recv().await {
            meter.push(&chunk);
        }
        meter.finish();
        let counts = meter.counts();
        if counts.is_empty() {
            return;
        }
        if let Err(e) = crate::livetv::ad_detect::teletext::write_sidecar(&sidecar, counts).await {
            warn!(error = %e, "writing the teletext sidecar failed");
        }
    })
}

/// Tuner bytes -> the remux and the teletext meter. Neither consumer can stall
/// the other, and the feeder owns the reconnect budget that ffmpeg used to.
fn spawn_feeder(
    tx: std::sync::mpsc::Sender<Vec<u8>>,
    tele: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    url: String,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        use futures::StreamExt;
        let http = crate::http::new_client();
        let mut attempt = 0usize;
        loop {
            if cancel.is_cancelled() {
                return;
            }
            let resp = tokio::select! {
                _ = cancel.cancelled() => return,
                r = http.get(&url).timeout(Duration::from_secs(86400)).send() => r,
            };
            match resp.and_then(|r| r.error_for_status()) {
                Ok(resp) => {
                    attempt = 0;
                    let mut body = resp.bytes_stream();
                    loop {
                        let next = tokio::select! {
                            _ = cancel.cancelled() => return,
                            n = body.next() => n,
                        };
                        match next {
                            Some(Ok(chunk)) => {
                                let _ = tele.send(chunk.to_vec());
                                if tx.send(chunk.to_vec()).is_err() {
                                    return;
                                }
                            }
                            Some(Err(e)) => {
                                warn!(error = %e, "live TV capture read error");
                                break;
                            }
                            None => break,
                        }
                    }
                }
                Err(e) => warn!(error = %e, url = %url, "live TV capture connect failed"),
            }
            let Some(delay) = RECONNECT_DELAYS_MS.get(attempt).copied() else {
                warn!(url = %url, "live TV capture reconnect budget exhausted");
                return;
            };
            attempt += 1;
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_millis(delay)) => {}
            }
        }
    });
}

/// Counted from the playlist, not the directory: a capture stopped before its first
/// segment boundary still leaves a `seg_*.m4s` on disk carrying no samples and
/// `#EXTINF:0`. Counting the file would report an empty resumed part as a successful
/// one and let a zero-duration segment into the recording's timeline.
async fn count_segments(dir: &Path) -> usize {
    let Ok(raw) = tokio::fs::read_to_string(dir.join("stream.m3u8")).await else {
        return 0;
    };
    let (entries, _) = parse_part(&raw);
    let mut n = 0;
    for (secs, file) in entries {
        if secs > 0.0 && dir.join(&file).exists() {
            n += 1;
        }
    }
    n
}

#[derive(Debug, Default, Clone)]
pub struct PlaylistStatus {
    pub segments: usize,
    pub duration_seconds: f64,
    pub parts: usize,
}

pub(crate) async fn part_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut parts: Vec<PathBuf> = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("part_") && entry.path().is_dir() {
                parts.push(entry.path());
            }
        }
    }
    parts.sort();
    parts
}

/// Parts already on disk. A resumed capture starts from this, so a resume that
/// captures nothing new still completes on what the previous worker wrote.
pub async fn part_count(dir: &Path) -> usize {
    part_dirs(dir).await.len()
}

pub(crate) fn parse_part(raw: &str) -> (Vec<(f64, String)>, Option<String>) {
    let mut entries = Vec::new();
    let mut pending: Option<f64> = None;
    let mut init: Option<String> = None;
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("#EXT-X-MAP:URI=\"") {
            init = rest.split('"').next().map(str::to_string);
            continue;
        }
        if let Some(v) = line.strip_prefix("#EXTINF:") {
            pending = v.trim_end_matches(',').trim().parse::<f64>().ok();
            continue;
        }
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if let Some(secs) = pending.take() {
            entries.push((secs, line.trim().to_string()));
        }
    }
    (entries, init)
}

/// Stitch every captured part into one playlist. A resume is a new part with its
/// own `init.mp4`, which is exactly what `EXT-X-DISCONTINUITY` plus a fresh
/// `EXT-X-MAP` describes; a promoted live buffer has no parts at all and is read
/// as a single flat one.
pub async fn read_playlist(
    dir: &Path,
    finished: bool,
    uri_prefix: &str,
) -> Option<(String, PlaylistStatus)> {
    let mut parts = part_dirs(dir).await;
    if parts.is_empty() && dir.join("stream.m3u8").exists() {
        parts.push(dir.to_path_buf());
    }
    if parts.is_empty() {
        return None;
    }

    let mut body: Vec<String> = Vec::new();
    let mut status = PlaylistStatus::default();
    let mut target: f64 = SEGMENT_SECONDS;

    for part in &parts {
        let Ok(raw) = tokio::fs::read_to_string(part.join("stream.m3u8")).await else {
            continue;
        };
        let (entries, init) = parse_part(&raw);
        let prefix = match part.strip_prefix(dir).ok().map(|p| p.to_string_lossy()) {
            Some(p) if !p.is_empty() => format!("{uri_prefix}{p}/"),
            _ => uri_prefix.to_string(),
        };
        let mut wrote_any = false;
        for (secs, file) in entries {
            if secs <= 0.0 || !part.join(&file).exists() {
                continue;
            }
            if !wrote_any {
                if status.segments > 0 {
                    body.push("#EXT-X-DISCONTINUITY".to_string());
                }
                if let Some(init) = &init {
                    body.push(format!("#EXT-X-MAP:URI=\"{prefix}{init}\""));
                }
                status.parts += 1;
                wrote_any = true;
            }
            target = target.max(secs);
            status.duration_seconds += secs;
            status.segments += 1;
            body.push(format!("#EXTINF:{secs:.6},"));
            body.push(format!("{prefix}{file}"));
        }
    }

    if status.segments == 0 {
        return None;
    }

    let mut out = vec![
        "#EXTM3U".to_string(),
        "#EXT-X-VERSION:7".to_string(),
        format!("#EXT-X-TARGETDURATION:{}", target.ceil() as u64),
        "#EXT-X-MEDIA-SEQUENCE:0".to_string(),
        "#EXT-X-INDEPENDENT-SEGMENTS".to_string(),
    ];
    out.extend(body);
    if finished {
        out.push("#EXT-X-ENDLIST".to_string());
    }
    Some((out.join("\n") + "\n", status))
}

pub async fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&path).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            match entry.metadata().await {
                Ok(m) if m.is_dir() => stack.push(entry.path()),
                Ok(m) => total += m.len(),
                Err(_) => {}
            }
        }
    }
    total
}

#[derive(Debug, Default)]
pub struct ReconcileReport {
    pub resumed: usize,
    pub closed: usize,
    pub missed: usize,
}

/// Decision 13. Nothing in memory survives a restart, so every row still marked
/// `recording` is an orphan: resume the ones whose window is still open by
/// re-enqueueing the capture job (it appends a part), close the rest.
pub async fn reconcile_on_boot(
    pool: &sqlx::PgPool,
    publisher: &nats_jobs::Publisher,
) -> Result<ReconcileReport, crate::repositories::RepositoryError> {
    let now = OffsetDateTime::now_utc();
    let mut report = ReconcileReport::default();
    livetv_recordings::release_recording_claims(pool, RESUME_STALE_SECS).await?;

    for row in livetv_recordings::list_by_status(pool, "recording").await? {
        let end = row.scheduled_end + time::Duration::seconds(row.padding_after_secs as i64);
        if end > now {
            match publisher
                .enqueue_job(
                    "kobbler",
                    "livetv_record",
                    serde_json::json!({ "recording_id": row.id, "resume": true }),
                )
                .await
            {
                Ok(_) => report.resumed += 1,
                Err(e) => warn!(recording_id = %row.id, error = %e, "resuming a recording failed"),
            }
        } else {
            let has_media = row.output_dir.is_some();
            livetv_recordings::set_status(
                pool,
                row.id,
                if has_media { "completed" } else { "failed" },
                if has_media {
                    None
                } else {
                    Some("the server restarted before anything was captured")
                },
            )
            .await?;
            report.closed += 1;
        }
    }

    for row in livetv_recordings::list_by_status(pool, "scheduled").await? {
        if row.scheduled_end + time::Duration::seconds(row.padding_after_secs as i64) <= now {
            livetv_recordings::set_status(
                pool,
                row.id,
                "failed",
                Some("the airing passed while the server was down"),
            )
            .await?;
            report.missed += 1;
        }
    }

    info!(
        resumed = report.resumed,
        closed = report.closed,
        missed = report.missed,
        "live TV recording reconcile complete"
    );
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A capture stopped before any segment boundary flushes a segment with no
    /// samples and `#EXTINF:0`. It is a file on disk but not a second of media, so
    /// counting it desynchronises a recording's duration from its content — and
    /// makes an empty resumed part look like a successful one.
    #[tokio::test]
    async fn a_zero_duration_segment_is_not_a_segment() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("part_000");
        tokio::fs::create_dir_all(&part).await.unwrap();
        tokio::fs::write(part.join("init.mp4"), b"x").await.unwrap();
        tokio::fs::write(part.join("seg_00000.m4s"), b"real")
            .await
            .unwrap();
        tokio::fs::write(part.join("seg_00001.m4s"), b"empty")
            .await
            .unwrap();
        tokio::fs::write(
            part.join("stream.m3u8"),
            "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4.000000,\nseg_00000.m4s\n#EXTINF:0.000000,\nseg_00001.m4s\n",
        )
        .await
        .unwrap();

        assert_eq!(
            count_segments(&part).await,
            1,
            "the empty segment must not be counted"
        );

        let (playlist, status) = read_playlist(dir.path(), true, "segments/")
            .await
            .expect("a playlist");
        assert_eq!(status.segments, 1);
        assert!((status.duration_seconds - 4.0).abs() < 1e-9);
        assert!(!playlist.contains("#EXTINF:0.000000"));
    }
}
