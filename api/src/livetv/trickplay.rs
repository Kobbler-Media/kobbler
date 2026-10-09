//! Trickplay for a recording that is still being written (Decision 7).
//!
//! `IncrementalSpriteSink` already emits `sprite_NNN.jpg` every 25 frames and
//! atomically rewrites `thumbnails.vtt`, so a scrubbing player never reads a
//! torn file. What was missing was an input: a recording is HLS segments, so
//! the pass runs over the segments that have already closed and picks up where
//! it left off on the next tick.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kob_ffmpeg::single_pass::{CustomSinks, SinglePassRequest, TrickplayParams, TrickplaySink};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::livetv::recorder;
use crate::media_cache::trickplay_sink::IncrementalSpriteSink;

/// Matches `media_cache::analyze`'s trickplay grid so the in-progress sprites
/// and the final ones describe the same thing.
const TRICKPLAY_WIDTH: i32 = 320;
const TRICKPLAY_FPS: f64 = 0.2;
const TRICKPLAY_QUALITY: i32 = 5;

/// One tick per segment batch. Four 4 s segments is ~16 s of programme, which
/// is three trickplay frames — enough to be worth a decode.
const TICK: Duration = Duration::from_secs(20);
const MIN_BATCH_SEGMENTS: usize = 4;

/// The first frame the pass ever saw, and what every later one is measured from.
///
/// Frames are reported on the source's own clock: a broadcast's PTS, which for one
/// Friday evening's tune was six and a half hours in, and which each batch simply
/// continues. The buffer a viewer scrubs starts at zero, so a VTT written in that clock
/// describes a timeline that does not exist — every cue sat hours past the end of the
/// one being scrubbed, which is why a live rewind never showed a single thumbnail.
#[derive(Default)]
struct Origin(Option<f64>);

impl Origin {
    fn rebase(&mut self, time_secs: f64) -> f64 {
        let base = *self.0.get_or_insert(time_secs);
        (time_secs - base).max(0.0)
    }
}

/// Puts every batch on the timeline the buffer is played on before it reaches the
/// shared sprite state.
struct RebasedSink {
    inner: Arc<Mutex<IncrementalSpriteSink>>,
    origin: Arc<Mutex<Origin>>,
    frames: Arc<Mutex<usize>>,
}

impl TrickplaySink for RebasedSink {
    fn on_frame(
        &mut self,
        idx: usize,
        time_secs: f64,
        jpeg: kob_ffmpeg::thumbnails::JpegFrame,
    ) -> Result<(), kob_ffmpeg::FfmpegError> {
        *self.frames.lock().expect("trickplay frame counter") += 1;
        let at = self
            .origin
            .lock()
            .expect("trickplay origin")
            .rebase(time_secs);
        self.inner
            .lock()
            .expect("trickplay sink")
            .on_frame(idx, at, jpeg)
    }

    // Deliberately not forwarded: flushing a partial chunk mid-recording would
    // misalign every sprite that follows it.
    fn finish(&mut self) -> Result<(), kob_ffmpeg::FfmpegError> {
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct Batch {
    pub playlist: String,
    pub segments: Vec<PathBuf>,
    pub duration_seconds: f64,
}

/// Closed segments the pass has not seen yet. The newest listed segment is left
/// alone: ffmpeg rewrites the playlist as it closes each one, and the tail of
/// the file may still be in flight.
async fn next_batch(dir: &Path, consumed: &HashSet<PathBuf>) -> Option<Batch> {
    let mut parts = recorder::part_dirs(dir).await;
    if parts.is_empty() && dir.join("stream.m3u8").exists() {
        parts.push(dir.to_path_buf());
    }
    for part in parts {
        let Ok(raw) = tokio::fs::read_to_string(part.join("stream.m3u8")).await else {
            continue;
        };
        let (entries, init) = recorder::parse_part(&raw);
        if entries.len() < 2 {
            continue;
        }
        let closed = &entries[..entries.len() - 1];
        let fresh: Vec<(f64, String)> = closed
            .iter()
            .filter(|(_, f)| !consumed.contains(&part.join(f)) && part.join(f).exists())
            .cloned()
            .collect();
        if fresh.len() < MIN_BATCH_SEGMENTS {
            continue;
        }

        let target = fresh.iter().fold(1.0f64, |a, (s, _)| a.max(*s)).ceil() as u64;
        let mut lines = vec![
            "#EXTM3U".to_string(),
            "#EXT-X-VERSION:7".to_string(),
            format!("#EXT-X-TARGETDURATION:{target}"),
            "#EXT-X-MEDIA-SEQUENCE:0".to_string(),
        ];
        if let Some(init) = &init {
            lines.push(format!("#EXT-X-MAP:URI=\"{init}\""));
        }
        let mut duration = 0.0;
        let mut segments = Vec::new();
        for (secs, file) in &fresh {
            duration += secs;
            lines.push(format!("#EXTINF:{secs:.6},"));
            lines.push(file.clone());
            segments.push(part.join(file));
        }
        lines.push("#EXT-X-ENDLIST".to_string());

        let playlist_path = part.join("trickplay_batch.m3u8");
        if tokio::fs::write(&playlist_path, lines.join("\n") + "\n")
            .await
            .is_err()
        {
            continue;
        }
        return Some(Batch {
            playlist: playlist_path.to_string_lossy().to_string(),
            segments,
            duration_seconds: duration,
        });
    }
    None
}

/// Run trickplay over a recording for as long as it is being captured. The
/// completed recording's own analysis pass regenerates everything from the
/// stitched manifest, so this only has to be good enough to scrub with.
pub async fn run(dir: PathBuf, trickplay_dir: PathBuf, cancel: CancellationToken) {
    if tokio::fs::create_dir_all(&trickplay_dir).await.is_err() {
        return;
    }
    let sink = Arc::new(Mutex::new(IncrementalSpriteSink::new(
        trickplay_dir.clone(),
        0.0,
    )));
    let mut consumed: HashSet<PathBuf> = HashSet::new();
    let origin = Arc::new(Mutex::new(Origin::default()));

    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(TICK) => {}
        }
        let Some(batch) = next_batch(&dir, &consumed).await else {
            continue;
        };
        let mut req = SinglePassRequest::new(&batch.playlist);
        req.trickplay = Some(TrickplayParams {
            width: TRICKPLAY_WIDTH,
            fps: TRICKPLAY_FPS,
            jpeg_quality: TRICKPLAY_QUALITY,
        });
        let frames = Arc::new(Mutex::new(0usize));
        let boxed: Box<dyn TrickplaySink> = Box::new(RebasedSink {
            inner: sink.clone(),
            origin: origin.clone(),
            frames: frames.clone(),
        });
        let sinks = CustomSinks {
            trickplay: Some(boxed),
        };
        let outcome = tokio::task::spawn_blocking(move || {
            kob_ffmpeg::single_pass::run_single_pass_with_sinks(req, None, sinks)
        })
        .await;
        match outcome {
            // Deliberately no per-batch offset: the clock the frames arrive on already
            // advances across batches, so adding the durations on top of it ran the VTT
            // at twice real time — 25 minutes of tuning described as 50.
            Ok(Ok(_)) => consumed.extend(batch.segments),
            Ok(Err(e)) => warn!(error = %e, "in-progress trickplay pass failed"),
            Err(e) => warn!(error = %e, "in-progress trickplay pass panicked"),
        }
    }
}

/// Re-express an in-progress trickplay VTT onto the buffer the client is actually
/// playing.
///
/// The sink counts from the first segment it ever saw, but retention reaps off the
/// front and `read_buffer` rebases the playlist onto the first surviving segment. Left
/// alone the two drift apart by exactly the reaped playing time, so every thumbnail
/// would land further and further ahead of the frame it belongs to.
pub fn rebase_vtt(vtt: &str, dropped_seconds: f64) -> String {
    let mut out = String::from("WEBVTT\n\n");
    let mut lines = vtt.lines().peekable();
    while let Some(line) = lines.next() {
        let Some((from, to)) = line.split_once("-->") else {
            continue;
        };
        let (Some(start), Some(end)) = (parse_vtt_time(from.trim()), parse_vtt_time(to.trim()))
        else {
            continue;
        };
        let Some(cue) = lines.next() else { break };
        let end = end - dropped_seconds;
        if end <= 0.0 {
            continue;
        }
        let start = (start - dropped_seconds).max(0.0);
        out.push_str(&format!(
            "{} --> {}\n{}\n\n",
            format_vtt_time(start),
            format_vtt_time(end),
            cue.trim(),
        ));
    }
    out
}

fn parse_vtt_time(v: &str) -> Option<f64> {
    let mut secs = 0.0f64;
    for part in v.split(':') {
        secs = secs * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(secs)
}

fn format_vtt_time(secs: f64) -> String {
    let secs = secs.max(0.0);
    let h = (secs / 3600.0).floor() as u64;
    let m = ((secs % 3600.0) / 60.0).floor() as u64;
    let s = secs % 60.0;
    format!("{h:02}:{m:02}:{s:06.3}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "WEBVTT\n\n00:00:00.000 --> 00:00:05.000\nsprite_001.jpg#xywh=0,0,320,180\n\n00:00:05.000 --> 00:00:10.000\nsprite_001.jpg#xywh=320,0,320,180\n\n00:00:10.000 --> 00:00:15.000\nsprite_001.jpg#xywh=640,0,320,180\n\n";

    /// The source clock is whatever the broadcaster's encoder says; all that matters is
    /// that the buffer's own timeline starts at zero and advances in real time.
    #[test]
    fn frames_are_written_on_the_buffers_timeline_not_the_broadcasts() {
        let mut origin = Origin::default();
        assert_eq!(0.0, origin.rebase(23_522.679));
        assert_eq!(5.0, origin.rebase(23_527.679));
        // A later batch continues the same clock, and is not offset a second time.
        assert_eq!(20.0, origin.rebase(23_542.679));
    }

    /// A batch whose first frame predates the origin — a resumed capture rewinding into
    /// what was already seen — lands at the start rather than before it.
    #[test]
    fn a_frame_before_the_origin_lands_at_the_start() {
        let mut origin = Origin::default();
        origin.rebase(100.0);
        assert_eq!(0.0, origin.rebase(90.0));
    }

    #[test]
    fn a_buffer_that_has_reaped_nothing_keeps_every_cue() {
        let out = rebase_vtt(SAMPLE, 0.0);
        assert_eq!(out.matches("-->").count(), 3);
        assert!(out.contains("00:00:00.000 --> 00:00:05.000"), "{out}");
    }

    #[test]
    fn cues_behind_the_retained_window_are_dropped_and_the_rest_shift_back() {
        // Retention reaped the first 5s, so the second cue is now the buffer's start.
        let out = rebase_vtt(SAMPLE, 5.0);
        assert_eq!(out.matches("-->").count(), 2, "{out}");
        assert!(out.contains("00:00:00.000 --> 00:00:05.000"), "{out}");
        assert!(out.contains("00:00:05.000 --> 00:00:10.000"), "{out}");
        assert!(
            out.contains("xywh=320,0"),
            "the surviving cue keeps its tile"
        );
    }

    #[test]
    fn a_cue_straddling_the_cut_starts_at_the_buffer_edge() {
        let out = rebase_vtt(SAMPLE, 7.5);
        assert!(out.contains("00:00:00.000 --> 00:00:02.500"), "{out}");
    }

    /// Both clients resolve a cue by stripping `.vtt` and treating the stem as a
    /// directory, so a served cue has to stay the bare filename the sprite route
    /// takes. A path segment here resolves one level too deep and 404s in silence.
    #[test]
    fn sprite_names_stay_bare_filenames() {
        let out = rebase_vtt(SAMPLE, 0.0);
        assert!(out.contains("\nsprite_001.jpg#xywh=0,0,320,180"), "{out}");
    }

    async fn write_part(part: &Path, count: usize) {
        tokio::fs::create_dir_all(part).await.unwrap();
        tokio::fs::write(part.join("init.mp4"), b"init")
            .await
            .unwrap();
        let mut lines = vec![
            "#EXTM3U".to_string(),
            "#EXT-X-VERSION:7".to_string(),
            "#EXT-X-TARGETDURATION:4".to_string(),
            "#EXT-X-MEDIA-SEQUENCE:0".to_string(),
            "#EXT-X-MAP:URI=\"init.mp4\"".to_string(),
        ];
        for i in 0..count {
            let name = format!("seg_{i:05}.m4s");
            tokio::fs::write(part.join(&name), b"x").await.unwrap();
            lines.push("#EXTINF:4.000000,".to_string());
            lines.push(name);
        }
        tokio::fs::write(part.join("stream.m3u8"), lines.join("\n") + "\n")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn the_newest_segment_is_never_analysed() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        write_part(&dir.join("part_000"), 5).await;
        let batch = next_batch(dir, &HashSet::new()).await.expect("a batch");
        assert_eq!(batch.segments.len(), 4);
        assert!(!batch.segments.iter().any(|p| p.ends_with("seg_00004.m4s")));
        assert!((batch.duration_seconds - 16.0).abs() < 0.001);
        let playlist = tokio::fs::read_to_string(&batch.playlist).await.unwrap();
        assert!(playlist.contains("#EXT-X-MAP:URI=\"init.mp4\""));
        assert!(playlist.contains("#EXT-X-ENDLIST"));
    }

    #[tokio::test]
    async fn a_batch_smaller_than_the_floor_waits() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        write_part(&dir.join("part_000"), 3).await;
        assert!(next_batch(dir, &HashSet::new()).await.is_none());
    }

    #[tokio::test]
    async fn already_analysed_segments_are_not_analysed_again() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let part = dir.join("part_000");
        write_part(&part, 9).await;
        let first = next_batch(dir, &HashSet::new()).await.expect("a batch");
        assert_eq!(first.segments.len(), 8);
        let mut consumed: HashSet<PathBuf> = first.segments.iter().cloned().collect();
        assert!(
            next_batch(dir, &consumed).await.is_none(),
            "only the still-open newest segment is left"
        );

        write_part(&part, 14).await;
        let second = next_batch(dir, &consumed).await.expect("a second batch");
        assert_eq!(second.segments.len(), 5);
        assert!(second.segments.iter().all(|p| !consumed.contains(p)));
        consumed.extend(second.segments);
        assert!(next_batch(dir, &consumed).await.is_none());
    }
}
