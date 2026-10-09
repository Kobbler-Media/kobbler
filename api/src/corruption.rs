//! Corrupt-range tracking for video files.
//!
//! `CorruptRange` describes a contiguous span of decoder errors observed by
//! [`kob_ffmpeg::single_pass::CorruptionSink`]. Stored on `tv_episode_files`
//! and `movie_files` as a JSONB array (`corrupt_ranges` column).
//!
//! The streaming layer reads these at session creation to spawn overlay
//! transcode generations covering the corrupt windows so clean segments can
//! still be `Copy` while corrupt segments get re-encoded — papering over
//! mid-stream corruption that would otherwise crash a browser HW decoder.
//!
//! Tier 3 (`POST /sessions/:id/decode-error`) calls [`merge`] to extend the
//! list with ranges discovered at runtime so future plays of the same file
//! pre-emptively transcode the same window without waiting for the player to
//! crash again.
//!
//! Wire format (one entry per JSON object in the JSONB array):
//! ```json
//! { "start_secs": 1839.92, "end_secs": 1839.92, "frame_count": 1,
//!   "concealment": true, "missing_reference": false, "invalid_bitstream": false,
//!   "decode_slices_failed": true, "other": false }
//! ```

use serde::{Deserialize, Serialize};

/// Default gap (seconds) used by [`merge`] when deciding whether two ranges
/// belong together. Wider than the per-frame coalescer in kob-ffmpeg because
/// stored ranges are already coalesced internally; this only handles overlap
/// at write time across separate analysis runs / Tier-3 events.
pub const DEFAULT_MERGE_GAP_SECS: f64 = 1.0;

/// A contiguous corrupt video span. `start_secs..=end_secs` is inclusive on
/// both ends — both ends are timestamps of an observed corrupt frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CorruptRange {
    pub start_secs: f64,
    pub end_secs: f64,
    pub frame_count: u32,
    pub concealment: bool,
    pub missing_reference: bool,
    pub invalid_bitstream: bool,
    pub decode_slices_failed: bool,
    pub other: bool,
}

impl From<kob_ffmpeg::single_pass::CorruptRange> for CorruptRange {
    fn from(r: kob_ffmpeg::single_pass::CorruptRange) -> Self {
        Self {
            start_secs: r.start_secs,
            end_secs: r.end_secs,
            frame_count: r.frame_count,
            concealment: r.concealment,
            missing_reference: r.missing_reference,
            invalid_bitstream: r.invalid_bitstream,
            decode_slices_failed: r.decode_slices_failed,
            other: r.other,
        }
    }
}

impl CorruptRange {
    /// True if the strongest decoder signal fired for this range — a
    /// playback-affecting error rather than e.g. an isolated CORRUPT-flag
    /// hint without a specific decode_error_flags bit.
    pub fn has_decoder_signal(&self) -> bool {
        self.concealment
            || self.missing_reference
            || self.invalid_bitstream
            || self.decode_slices_failed
    }

    fn overlaps_or_adjacent(&self, other: &Self, gap_secs: f64) -> bool {
        let lo = self.start_secs.min(other.start_secs);
        let hi = self.end_secs.max(other.end_secs);
        let combined = hi - lo;
        let span_self = self.end_secs - self.start_secs;
        let span_other = other.end_secs - other.start_secs;
        combined <= span_self + span_other + gap_secs
    }

    fn absorb(&mut self, other: &Self) {
        self.start_secs = self.start_secs.min(other.start_secs);
        self.end_secs = self.end_secs.max(other.end_secs);
        self.frame_count = self.frame_count.saturating_add(other.frame_count);
        self.concealment |= other.concealment;
        self.missing_reference |= other.missing_reference;
        self.invalid_bitstream |= other.invalid_bitstream;
        self.decode_slices_failed |= other.decode_slices_failed;
        self.other |= other.other;
    }
}

/// Merge two range lists, coalescing entries whose intervals overlap or are
/// within `gap_secs` of each other. Result is sorted by `start_secs`.
///
/// Used when appending Tier-3 runtime discoveries to existing analyze-time
/// data, and inside `set_corrupt_ranges` to canonicalise a fresh list.
pub fn merge(
    existing: impl IntoIterator<Item = CorruptRange>,
    new: impl IntoIterator<Item = CorruptRange>,
    gap_secs: f64,
) -> Vec<CorruptRange> {
    let mut all: Vec<CorruptRange> = existing.into_iter().chain(new).collect();
    all.sort_by(|a, b| {
        a.start_secs
            .partial_cmp(&b.start_secs)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut out: Vec<CorruptRange> = Vec::with_capacity(all.len());
    for r in all {
        match out.last_mut() {
            Some(prev) if prev.overlaps_or_adjacent(&r, gap_secs) => prev.absorb(&r),
            _ => out.push(r),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(start: f64, end: f64) -> CorruptRange {
        CorruptRange {
            start_secs: start,
            end_secs: end,
            frame_count: 1,
            concealment: true,
            missing_reference: false,
            invalid_bitstream: false,
            decode_slices_failed: false,
            other: false,
        }
    }

    #[test]
    fn merge_disjoint_keeps_both() {
        let m = merge([r(10.0, 11.0)], [r(100.0, 101.0)], 1.0);
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn merge_overlapping_coalesces() {
        let m = merge([r(10.0, 12.0)], [r(11.0, 15.0)], 1.0);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].start_secs, 10.0);
        assert_eq!(m[0].end_secs, 15.0);
        assert_eq!(m[0].frame_count, 2);
    }

    #[test]
    fn merge_within_gap_coalesces() {
        let m = merge([r(10.0, 11.0)], [r(11.5, 12.0)], 1.0);
        assert_eq!(m.len(), 1, "ranges 0.5s apart should merge with gap=1.0");
    }

    #[test]
    fn merge_outside_gap_keeps_both() {
        let m = merge([r(10.0, 11.0)], [r(13.0, 14.0)], 1.0);
        assert_eq!(m.len(), 2, "ranges 2s apart should NOT merge with gap=1.0");
    }

    #[test]
    fn merge_unions_severity_flags() {
        let mut a = r(10.0, 11.0);
        a.concealment = true;
        a.missing_reference = false;
        let mut b = r(11.0, 12.0);
        b.concealment = false;
        b.missing_reference = true;
        let m = merge([a], [b], 1.0);
        assert!(m[0].concealment);
        assert!(m[0].missing_reference);
    }

    #[test]
    fn has_decoder_signal_distinguishes_other() {
        let mut bare = r(10.0, 11.0);
        bare.concealment = false;
        bare.missing_reference = false;
        bare.invalid_bitstream = false;
        bare.decode_slices_failed = false;
        bare.other = true;
        assert!(!bare.has_decoder_signal());

        let mut concealed = r(10.0, 11.0);
        concealed.concealment = true;
        concealed.missing_reference = false;
        concealed.invalid_bitstream = false;
        concealed.decode_slices_failed = false;
        assert!(concealed.has_decoder_signal());
    }
}
