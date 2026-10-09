//! Playback session config for HLS segment management.
//!
//! Segment state (runs, index, resolution, eviction) and keyframe
//! prediction live in `kob_ffmpeg::session::TranscodeSession`. This
//! module holds only per-session config fields.

use std::path::PathBuf;

/// Per-session config for HLS segment management.
///
/// All segment state queries go through `TranscodeProcess` →
/// `kob_ffmpeg::session::TranscodeSession`.
pub struct SegmentCache {
    /// Duration of each HLS segment in seconds.
    pub segment_duration: u32,
    /// Total number of segments for the full media file.
    pub total_segments: u32,
    /// Total media duration in seconds (from ffprobe).
    pub duration_seconds: f64,
    /// Session directory containing `gen_N/` subdirectories.
    pub base_dir: PathBuf,
    /// Where the playlist should start emitting from (set on seek).
    pub playback_start_segment: u32,
    /// The media time that segment actually begins at.
    ///
    /// Not `playback_start_segment * segment_duration`: a copy-mode stream splits at
    /// keyframes, so its segments are only nominally [segment_duration] long and the
    /// grid drifts from real time by a fraction of a segment per keyframe. FFmpeg has
    /// to be started at the real time — an ABR variant restarted at the grid's idea of
    /// it opens somewhere else entirely from the playlist it is meant to be
    /// interchangeable with. Kept in step with [Self::playback_start_segment] by
    /// [Self::set_playback_start].
    pub playback_start_secs: f64,
}

impl SegmentCache {
    /// Create a new segment cache for a session.
    pub fn new(base_dir: PathBuf, segment_duration: u32, duration_seconds: f64) -> Self {
        // Cap at 24 hours worth of segments to prevent OOM from absurd durations
        const MAX_SEGMENTS: u32 = 24 * 3600 / 2; // ~43200 segments at 2s each
        let total_segments =
            ((duration_seconds / f64::from(segment_duration)).ceil() as u32).min(MAX_SEGMENTS);
        Self {
            segment_duration,
            total_segments,
            duration_seconds,
            base_dir,
            playback_start_segment: 0,
            playback_start_secs: 0.0,
        }
    }

    /// Move the playlist start to `segment`, which begins at `secs`.
    pub fn set_playback_start(&mut self, segment: u32, secs: f64) {
        self.playback_start_segment = segment;
        self.playback_start_secs = secs;
    }

    /// Adopt another cache's playlist start, so an ABR variant opens where its
    /// primary did rather than where the segment grid says that segment sits.
    pub fn follow_playback_start(&mut self, other: &Self) {
        self.set_playback_start(other.playback_start_segment, other.playback_start_secs);
    }

    /// The output directory for a given generation.
    pub fn gen_dir(&self, generation: u32) -> PathBuf {
        self.base_dir.join(format!("gen_{generation}"))
    }

    /// Convert a time in seconds to an absolute segment index.
    pub fn time_to_segment(&self, secs: f64) -> u32 {
        let seg = (secs / f64::from(self.segment_duration)).floor() as u32;
        seg.min(self.total_segments.saturating_sub(1))
    }

    /// Convert an absolute segment index to a time in seconds.
    pub fn segment_to_time(&self, idx: u32) -> f64 {
        f64::from(idx) * f64::from(self.segment_duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_cache(duration: f64) -> (SegmentCache, TempDir) {
        let tmp = TempDir::new().unwrap();
        let cache = SegmentCache::new(tmp.path().to_path_buf(), 6, duration);
        (cache, tmp)
    }

    #[test]
    fn time_to_segment_basics() {
        let (cache, _tmp) = make_cache(120.0);
        assert_eq!(cache.time_to_segment(0.0), 0);
        assert_eq!(cache.time_to_segment(5.9), 0);
        assert_eq!(cache.time_to_segment(6.0), 1);
        assert_eq!(cache.time_to_segment(12.0), 2);
        // Clamp to max
        assert_eq!(cache.time_to_segment(999.0), cache.total_segments - 1);
    }

    /// The playlist start is a real media time, not the grid's idea of one: a copy-mode
    /// stream's segments are only nominally `segment_duration` long, so the two diverge
    /// as the title goes on — and a variant restarted on the grid opens somewhere the
    /// primary's playlist never was.
    #[test]
    fn the_playback_start_keeps_the_time_it_was_given() {
        let (mut cache, _tmp) = make_cache(3600.0);
        assert_eq!(cache.playback_start_secs, 0.0);

        // Segment 230 of a keyframe-split stream, which the 6s grid puts at 1380s.
        cache.set_playback_start(230, 1337.5);
        assert_eq!(cache.playback_start_segment, 230);
        assert_eq!(cache.playback_start_secs, 1337.5);
        assert_eq!(cache.segment_to_time(230), 1380.0);

        let (mut variant, _tmp2) = make_cache(3600.0);
        variant.follow_playback_start(&cache);
        assert_eq!(variant.playback_start_segment, 230);
        assert_eq!(variant.playback_start_secs, 1337.5);
    }

    #[test]
    fn segment_to_time() {
        let (cache, _tmp) = make_cache(120.0);
        assert_eq!(cache.segment_to_time(0), 0.0);
        assert_eq!(cache.segment_to_time(1), 6.0);
        assert_eq!(cache.segment_to_time(5), 30.0);
    }
}
