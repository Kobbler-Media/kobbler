//! What detection weighed for a file and why it kept or dropped each candidate, stored
//! in `skip_detect_candidates` and logged one line per file, alike for library files
//! and debrid downloads.

use serde_json::json;
use tracing::info;
use uuid::Uuid;

use super::cache_detect::EpisodeDiagnostics;
use super::{AudioProfile, Candidate};
use crate::repositories::skip_detect_candidates::CandidateRow;

/// Build a flat list of `CandidateRow`s for a single file from its
/// diagnostics — both fusion winners (selected=true) and rejected
/// candidates (selected=false with `rejection_reason` set).
///
/// Match winners back to their pre-fusion `Candidate` by (segment_type,
/// start_seconds) within a 0.5s epsilon so the stored row carries the full
/// signal breakdown, not just the post-score summary.
pub fn candidate_rows(file_id: Uuid, diag: &EpisodeDiagnostics) -> Vec<CandidateRow> {
    const MATCH_EPSILON: f64 = 0.5;
    let mut rows: Vec<CandidateRow> = Vec::new();

    for seg in &diag.final_segments {
        let matched = diag.candidates_pre_fusion.iter().find(|c| {
            c.segment_type == seg.segment_type
                && (c.start_seconds - seg.start_seconds).abs() < MATCH_EPSILON
        });
        match matched {
            Some(c) => rows.push(candidate_to_row(file_id, c, true, None)),
            None => {
                // Fallback: fusion produced a segment we can't tie back to a
                // pre-fusion candidate (e.g. synthesized downstream). Store a
                // minimal row so the audit still shows the winner.
                rows.push(CandidateRow {
                    file_id,
                    segment_type: seg.segment_type.as_str().to_string(),
                    start_seconds: seg.start_seconds,
                    end_seconds: seg.end_seconds,
                    chromaprint_score: None,
                    dhash_score: None,
                    boundary_has_black_frame: None,
                    boundary_has_silence: None,
                    boundary_refined_start: None,
                    boundary_refined_end: None,
                    audio_profile: None,
                    credit_text_score: None,
                    chapter_agreement: None,
                    file_duration: Some(diag.duration),
                    selected: true,
                    rejection_reason: None,
                });
            }
        }
    }

    for rej in &diag.rejected_candidates {
        rows.push(candidate_to_row(
            file_id,
            &rej.candidate,
            false,
            Some(rej.reason.clone()),
        ));
    }

    rows
}

fn candidate_to_row(
    file_id: Uuid,
    c: &Candidate,
    selected: bool,
    rejection_reason: Option<String>,
) -> CandidateRow {
    let (boundary_black, boundary_silence, boundary_start, boundary_end) =
        match &c.signals.boundary_match {
            Some(b) => (
                Some(b.has_black_frame),
                Some(b.has_silence),
                Some(b.refined_start),
                Some(b.refined_end),
            ),
            None => (None, None, None, None),
        };
    CandidateRow {
        file_id,
        segment_type: c.segment_type.as_str().to_string(),
        start_seconds: c.start_seconds,
        end_seconds: c.end_seconds,
        chromaprint_score: c.signals.chromaprint_score,
        dhash_score: c.signals.dhash_score,
        boundary_has_black_frame: boundary_black,
        boundary_has_silence: boundary_silence,
        boundary_refined_start: boundary_start,
        boundary_refined_end: boundary_end,
        audio_profile: c
            .signals
            .audio_profile
            .map(audio_profile_to_str)
            .map(String::from),
        credit_text_score: c.signals.credit_text_score,
        chapter_agreement: c.signals.chapter_agreement,
        file_duration: c.file_duration,
        selected,
        rejection_reason,
    }
}

fn audio_profile_to_str(p: AudioProfile) -> &'static str {
    match p {
        AudioProfile::MusicOnly => "music_only",
        AudioProfile::Dialogue => "dialogue",
        AudioProfile::NarrationLoop => "narration_loop",
        AudioProfile::Mixed => "mixed",
        AudioProfile::Unknown => "unknown",
    }
}

/// Emit one compact structured log line per file so operators can answer
/// "why didn't this episode get a segment?" from `docker logs` without
/// querying the candidates table. Mirrors the `"media_analyze timing"`
/// shape — one line, JSON in a tracing field.
pub fn log(anchor_file_id: Uuid, scope: &str, diags: &[EpisodeDiagnostics]) {
    for diag in diags {
        let rejected_summary: Vec<serde_json::Value> = diag
            .rejected_candidates
            .iter()
            .map(|r| {
                json!({
                    "segment_type": r.candidate.segment_type.as_str(),
                    "start_seconds": r.candidate.start_seconds,
                    "end_seconds": r.candidate.end_seconds,
                    "reason": r.reason,
                })
            })
            .collect();
        info!(
            anchor_file_id = %anchor_file_id,
            file_id = %diag.file_id,
            scope = scope,
            duration = diag.duration,
            chromaprint_samples = diag.chromaprint_samples,
            vhash_frames = diag.vhash_frames,
            chapter_count = diag.chapter_count,
            pre_fusion = diag.candidates_pre_fusion.len(),
            persisted = diag.final_segments.len(),
            rejected = diag.rejected_candidates.len(),
            notes = %serde_json::to_string(&diag.notes).unwrap_or_default(),
            rejected_detail = %serde_json::to_string(&rejected_summary).unwrap_or_default(),
            "skip_detect diag"
        );
    }
}
