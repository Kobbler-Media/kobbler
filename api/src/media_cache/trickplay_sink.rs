//! Progressive trickplay sink: emit sprite sheets + thumbnails.vtt to disk
//! per chunk so the player sees scrubbable thumbnails before the full decode
//! finishes.
//!
//! Used by [`super::analyze::run_analysis`] in place of the historic
//! buffer-then-tile path. See `kob_ffmpeg::single_pass::TrickplaySink` for the
//! contract: each completed trickplay JPEG is dispatched to `on_frame` in
//! capture order, and `finish` flushes any trailing partial chunk.
//!
//! On every full chunk (default 25 frames = one 5×5 sprite) we:
//!   1. Tile the chunk into `sprite_NNN.jpg` via `tile_chunk_to_sprite`.
//!   2. Atomically rewrite `thumbnails.vtt` covering every sprite emitted so
//!      far, so the player polling the VTT mid-decode never reads a torn or
//!      empty file.
//!
//! The trailing partial chunk (1–24 frames) is flushed by `finish()` with a
//! final VTT rewrite. Sprite indices are 1-based to match the historical
//! `sprite_001.jpg` naming used by both the on-disk and VTT layouts.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use kob_ffmpeg::single_pass::TrickplaySink;
use kob_ffmpeg::thumbnails::JpegFrame;

use crate::ffmpeg::transcode::{ThumbnailResult, generate_thumbnails_vtt, tile_chunk_to_sprite};
use crate::media_cache::writers::write_atomic;

/// Default sprite tile dimensions — 5×5 = 25 frames per sprite, matching the
/// dimensions used by the legacy buffered tiler.
pub const TILE_COLS: u32 = 5;
pub const TILE_ROWS: u32 = 5;
pub const FRAMES_PER_SPRITE: usize = (TILE_COLS * TILE_ROWS) as usize;

/// Streaming trickplay sink that writes sprite sheets + a partial VTT to disk
/// as frames arrive. Construct with [`Self::new`] and pass the boxed sink via
/// `kob_ffmpeg::single_pass::CustomSinks::trickplay`.
pub struct IncrementalSpriteSink {
    trickplay_dir: PathBuf,
    tile_cols: u32,
    tile_rows: u32,
    frames_per_sprite: usize,
    /// Best-effort total duration used for the LAST entry's end-time before
    /// `finish()` runs. Comes from `AnalyzeInputs.known_duration` at the call
    /// site; falls back to the last captured timestamp when unset.
    expected_duration_secs: f64,
    /// Frames buffered for the next sprite. Drained when full.
    buffer: Vec<JpegFrame>,
    /// Capture timestamps for every frame seen so far (across all sprites).
    /// Drives VTT entry start/end times.
    all_timestamps: Vec<f64>,
    /// Number of sprite sheets already written (i.e. `sprite_001.jpg`
    /// through `sprite_{sprites_written:03}.jpg` exist on disk).
    sprites_written: u32,
    /// Tile dimensions detected from the first emitted sprite. Cached because
    /// the VTT generator needs them on every rewrite.
    thumb_dims: Option<(u32, u32)>,
}

impl IncrementalSpriteSink {
    pub fn new(trickplay_dir: PathBuf, expected_duration_secs: f64) -> Self {
        Self::with_tile_dims(trickplay_dir, expected_duration_secs, TILE_COLS, TILE_ROWS)
    }

    pub fn with_tile_dims(
        trickplay_dir: PathBuf,
        expected_duration_secs: f64,
        tile_cols: u32,
        tile_rows: u32,
    ) -> Self {
        let frames_per_sprite = (tile_cols * tile_rows) as usize;
        Self {
            trickplay_dir,
            tile_cols,
            tile_rows,
            frames_per_sprite,
            expected_duration_secs,
            buffer: Vec::with_capacity(frames_per_sprite),
            all_timestamps: Vec::new(),
            sprites_written: 0,
            thumb_dims: None,
        }
    }

    /// Snapshot of what's on disk: number of sprites written, total thumb
    /// count across them, tile dims, captured timestamps. Used by
    /// `analyze.rs` to fill the manifest after `finish()`.
    pub fn summary(&self) -> SpriteSummary {
        let (thumb_w, thumb_h) = self.thumb_dims.unwrap_or((0, 0));
        SpriteSummary {
            sprite_count: self.sprites_written,
            thumb_width: thumb_w,
            thumb_height: thumb_h,
            tile_cols: self.tile_cols,
            tile_rows: self.tile_rows,
            total_thumbs: self.all_timestamps.len() as u32,
            timestamps: self.all_timestamps.clone(),
        }
    }

    fn flush_chunk(&mut self) -> Result<(), kob_ffmpeg::FfmpegError> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.trickplay_dir).map_err(io_to_ffmpeg_err)?;
        let next_idx = self.sprites_written + 1;
        let sprite_path = self.trickplay_dir.join(format!("sprite_{next_idx:03}.jpg"));

        let (thumb_w, thumb_h) =
            tile_chunk_to_sprite(&self.buffer, &sprite_path, self.tile_cols, self.tile_rows)
                .map_err(|e| {
                    kob_ffmpeg::FfmpegError::new(
                        kob_ffmpeg::ffi::AVERROR_EXTERNAL,
                        format!("incremental tile failed: {e}"),
                    )
                })?;

        self.thumb_dims.get_or_insert((thumb_w, thumb_h));
        self.buffer.clear();
        self.sprites_written = next_idx;

        self.rewrite_vtt()?;
        Ok(())
    }

    fn rewrite_vtt(&self) -> Result<(), kob_ffmpeg::FfmpegError> {
        let Some((thumb_w, thumb_h)) = self.thumb_dims else {
            return Ok(());
        };
        let result = ThumbnailResult {
            sprite_count: self.sprites_written,
            thumb_width: thumb_w,
            thumb_height: thumb_h,
            tile_cols: self.tile_cols,
            tile_rows: self.tile_rows,
            total_thumbs: self.all_timestamps.len() as u32,
            timestamps: self.all_timestamps.clone(),
        };
        let last_ts = self.all_timestamps.last().copied().unwrap_or(0.0);
        let dur_for_vtt = if self.expected_duration_secs > last_ts {
            self.expected_duration_secs
        } else {
            // No declared duration (or duration < last seen timestamp): pad
            // the final entry by one inter-frame interval so it has a non-zero span.
            let interval = if self.all_timestamps.len() >= 2 {
                let n = self.all_timestamps.len();
                (self.all_timestamps[n - 1] - self.all_timestamps[n - 2]).max(0.1)
            } else {
                1.0
            };
            last_ts + interval
        };
        let vtt = generate_thumbnails_vtt(&result, dur_for_vtt);
        let vtt_path = self.trickplay_dir.join("thumbnails.vtt");
        write_atomic(&vtt_path, vtt.as_bytes()).map_err(io_to_ffmpeg_err)?;
        Ok(())
    }
}

impl TrickplaySink for IncrementalSpriteSink {
    fn on_frame(
        &mut self,
        _frame_idx: usize,
        time_secs: f64,
        jpeg: JpegFrame,
    ) -> Result<(), kob_ffmpeg::FfmpegError> {
        self.all_timestamps.push(time_secs);
        self.buffer.push(jpeg);
        if self.buffer.len() >= self.frames_per_sprite {
            self.flush_chunk()?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), kob_ffmpeg::FfmpegError> {
        if !self.buffer.is_empty() {
            self.flush_chunk()?;
        }
        // VTT rewrite covers the case where every chunk filled exactly and
        // the last `flush_chunk` already wrote VTT — that's idempotent.
        self.rewrite_vtt()?;
        Ok(())
    }
}

fn io_to_ffmpeg_err(e: std::io::Error) -> kob_ffmpeg::FfmpegError {
    kob_ffmpeg::FfmpegError::new(
        kob_ffmpeg::ffi::AVERROR_EXTERNAL,
        format!("incremental trickplay io: {e}"),
    )
}

/// Wrap an [`IncrementalSpriteSink`] in an `Arc<Mutex<_>>` so the caller can
/// read [`IncrementalSpriteSink::summary`] after `run_single_pass_with_sinks`
/// has consumed the boxed `TrickplaySink`.
///
/// The returned tuple is `(handle, boxed_sink)`. Pass `boxed_sink` via
/// `CustomSinks::trickplay`; after the run completes, take the inner sink out
/// of `handle` to read its summary.
pub fn shared(
    sink: IncrementalSpriteSink,
) -> (Arc<Mutex<IncrementalSpriteSink>>, Box<dyn TrickplaySink>) {
    let arc = Arc::new(Mutex::new(sink));
    let adapter = SharedTrickplayAdapter { inner: arc.clone() };
    (arc, Box::new(adapter))
}

struct SharedTrickplayAdapter {
    inner: Arc<Mutex<IncrementalSpriteSink>>,
}

impl TrickplaySink for SharedTrickplayAdapter {
    fn on_frame(
        &mut self,
        idx: usize,
        time_secs: f64,
        jpeg: JpegFrame,
    ) -> Result<(), kob_ffmpeg::FfmpegError> {
        self.inner.lock().unwrap().on_frame(idx, time_secs, jpeg)
    }

    fn finish(&mut self) -> Result<(), kob_ffmpeg::FfmpegError> {
        self.inner.lock().unwrap().finish()
    }
}

/// Report from [`IncrementalSpriteSink::summary`], used by analyze.rs to
/// populate the manifest's trickplay artefact info after `finish()`.
#[derive(Debug, Clone)]
pub struct SpriteSummary {
    pub sprite_count: u32,
    pub thumb_width: u32,
    pub thumb_height: u32,
    pub tile_cols: u32,
    pub tile_rows: u32,
    pub total_thumbs: u32,
    pub timestamps: Vec<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};
    use kob_ffmpeg::single_pass::TrickplaySink;
    use std::io::Cursor;
    use tempfile::TempDir;

    fn synth_jpeg(timestamp: f64, color: [u8; 3]) -> JpegFrame {
        // 16x16 solid-color JPEG. Cheap to encode/decode in tests.
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(16, 16, |_, _| Rgb(color));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
            .expect("encode jpeg");
        JpegFrame {
            timestamp_secs: timestamp,
            jpeg_data: bytes,
            width: 16,
            height: 16,
        }
    }

    fn count_sprites(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir)
            .map(|it| {
                it.filter_map(|e| e.ok())
                    .filter(|e| {
                        e.path().extension().map(|x| x == "jpg").unwrap_or(false)
                            && e.file_name().to_string_lossy().starts_with("sprite_")
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    fn read_vtt(dir: &std::path::Path) -> String {
        std::fs::read_to_string(dir.join("thumbnails.vtt")).expect("read vtt")
    }

    #[test]
    fn emits_sprite_at_each_full_chunk() {
        let tmp = TempDir::new().unwrap();
        let mut sink = IncrementalSpriteSink::new(tmp.path().to_path_buf(), 100.0);

        // Push 24 frames — no sprite yet.
        for i in 0..24 {
            sink.on_frame(i, i as f64, synth_jpeg(i as f64, [0, 0, 0]))
                .unwrap();
        }
        assert_eq!(count_sprites(tmp.path()), 0, "no sprite before chunk full");
        assert!(
            !tmp.path().join("thumbnails.vtt").exists(),
            "no VTT before first sprite"
        );

        // 25th frame triggers sprite_001.jpg + first VTT.
        sink.on_frame(24, 24.0, synth_jpeg(24.0, [0, 0, 0]))
            .unwrap();
        assert_eq!(count_sprites(tmp.path()), 1);
        let vtt = read_vtt(tmp.path());
        assert!(vtt.starts_with("WEBVTT"));
        assert!(vtt.contains("sprite_001.jpg"));

        // Next 25 frames → sprite_002.jpg.
        for i in 25..50 {
            sink.on_frame(i, i as f64, synth_jpeg(i as f64, [0, 0, 0]))
                .unwrap();
        }
        assert_eq!(count_sprites(tmp.path()), 2);
        let vtt = read_vtt(tmp.path());
        assert!(vtt.contains("sprite_002.jpg"));
    }

    #[test]
    fn finish_flushes_partial_trailing_chunk() {
        let tmp = TempDir::new().unwrap();
        let mut sink = IncrementalSpriteSink::new(tmp.path().to_path_buf(), 100.0);

        // 30 frames: one full sprite + 5 trailing.
        for i in 0..30 {
            sink.on_frame(i, i as f64, synth_jpeg(i as f64, [0, 0, 0]))
                .unwrap();
        }
        assert_eq!(count_sprites(tmp.path()), 1);

        sink.finish().unwrap();
        assert_eq!(
            count_sprites(tmp.path()),
            2,
            "finish() emits trailing sprite"
        );
        let vtt = read_vtt(tmp.path());
        assert!(vtt.contains("sprite_002.jpg"));
    }

    #[test]
    fn vtt_only_lists_sprites_already_on_disk() {
        // After 25 frames the VTT must list exactly 25 entries — no phantom
        // rows pointing at sprite_002 before it has been written.
        let tmp = TempDir::new().unwrap();
        let mut sink = IncrementalSpriteSink::new(tmp.path().to_path_buf(), 100.0);
        for i in 0..25 {
            sink.on_frame(i, i as f64, synth_jpeg(i as f64, [0, 0, 0]))
                .unwrap();
        }
        let vtt = read_vtt(tmp.path());
        let sprite_001_count = vtt.matches("sprite_001.jpg").count();
        let sprite_002_count = vtt.matches("sprite_002.jpg").count();
        assert_eq!(sprite_001_count, 25, "all 25 entries point at sprite_001");
        assert_eq!(sprite_002_count, 0, "no phantom sprite_002 entries");
    }

    #[test]
    fn empty_sink_finish_is_noop() {
        let tmp = TempDir::new().unwrap();
        let mut sink = IncrementalSpriteSink::new(tmp.path().to_path_buf(), 100.0);
        sink.finish().unwrap();
        assert_eq!(count_sprites(tmp.path()), 0);
        // No VTT written either — nothing to describe.
        assert!(!tmp.path().join("thumbnails.vtt").exists());
    }

    #[test]
    fn vtt_rewrite_is_atomic() {
        // After every sprite the VTT is rewritten via write_atomic. Confirm
        // there's no leftover .tmp sibling and the on-disk file always parses
        // as a complete VTT.
        let tmp = TempDir::new().unwrap();
        let mut sink = IncrementalSpriteSink::new(tmp.path().to_path_buf(), 100.0);
        for i in 0..50 {
            sink.on_frame(i, i as f64, synth_jpeg(i as f64, [0, 0, 0]))
                .unwrap();
            // Mid-stream: at every 25-multiple the VTT must be readable.
            if (i + 1) % 25 == 0 {
                let vtt = read_vtt(tmp.path());
                assert!(vtt.starts_with("WEBVTT\n"));
                assert!(vtt.ends_with("\n"));
            }
        }
        sink.finish().unwrap();
        // No torn .tmp left behind.
        let stragglers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "tmp").unwrap_or(false))
            .collect();
        assert!(stragglers.is_empty(), "left .tmp files: {stragglers:?}");
    }

    #[test]
    fn summary_reports_disk_state() {
        let tmp = TempDir::new().unwrap();
        let mut sink = IncrementalSpriteSink::new(tmp.path().to_path_buf(), 100.0);
        for i in 0..30 {
            sink.on_frame(i, i as f64, synth_jpeg(i as f64, [0, 0, 0]))
                .unwrap();
        }
        sink.finish().unwrap();
        let s = sink.summary();
        assert_eq!(s.sprite_count, 2);
        assert_eq!(s.total_thumbs, 30);
        assert_eq!(s.tile_cols, 5);
        assert_eq!(s.tile_rows, 5);
        assert_eq!(s.thumb_width, 16);
        assert_eq!(s.thumb_height, 16);
        assert_eq!(s.timestamps.len(), 30);
    }
}
