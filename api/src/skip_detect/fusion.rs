//! Fusion layer: combines multi-signal candidates into final detected segments.
//!
//! Three independent detection passes feed candidates:
//! 1. Chromaprint audio fingerprinting (repeated audio across episodes)
//! 2. dHash visual sequence matching (repeated frame sequences across episodes)
//! 3. Zero-shot credit text detection (text/names overlaid on video)
//!
//! Confidence is assigned based on signal agreement. Vetoes suppress false positives.

use super::{
    AudioProfile, Candidate, CandidateSignals, DetectedSegment, MAX_CREDITS_DURATION_ABS,
    MAX_CREDITS_DURATION_PCT, MAX_INTRO_DURATION, MAX_POST_CREDITS_DURATION, MIN_CREDITS_START_PCT,
    MIN_EFFECTIVE_CONFIDENCE, MIN_INTRO_DURATION, SegmentType, max_credits_duration,
};

/// Fusion output including both surviving segments and rejected candidates
/// (with a per-candidate reason for rejection). Used by the skip_detect
/// handler to populate the `skip_detect_candidates` table as a queryable
/// audit trail — callers that only want the winners can use [`score_candidates`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct FusionOutcome {
    pub segments: Vec<DetectedSegment>,
    pub rejected: Vec<RejectedCandidate>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RejectedCandidate {
    pub candidate: Candidate,
    pub reason: String,
    /// The scored segment when rejection happened post-scoring (so callers
    /// see the confidence/veto that pushed it below threshold). `None` when
    /// the candidate was filtered before `score_one` ran.
    pub scored: Option<DetectedSegment>,
}

/// Score a set of candidates and return only segments that pass the effective confidence threshold.
pub fn score_candidates(candidates: Vec<Candidate>) -> Vec<DetectedSegment> {
    score_candidates_with_rejections(candidates).segments
}

/// Same scoring pipeline as [`score_candidates`] but also returns every
/// candidate that was dropped along the way, tagged with the reason it was
/// dropped. Reasons are drawn from a small fixed vocabulary so downstream
/// queries can group/filter on them.
pub fn score_candidates_with_rejections(candidates: Vec<Candidate>) -> FusionOutcome {
    let mut segments: Vec<DetectedSegment> = Vec::new();
    let mut rejected: Vec<RejectedCandidate> = Vec::new();

    for c in candidates {
        if let Some(reason) = gate_by_bounds(&c) {
            rejected.push(RejectedCandidate {
                candidate: c,
                reason,
                scored: None,
            });
            continue;
        }
        let scored = score_one(c.clone());
        if scored.should_persist() {
            segments.push(scored);
        } else {
            rejected.push(RejectedCandidate {
                candidate: c,
                reason: format!(
                    "effective_confidence {:.3} < {:.3} (conf {:.3} × (1 - veto {:.3}))",
                    scored.effective_confidence(),
                    MIN_EFFECTIVE_CONFIDENCE,
                    scored.confidence,
                    scored.veto_score,
                ),
                scored: Some(scored),
            });
        }
    }

    // Remove orphaned post-credits scenes (no surviving credits segment).
    // Demote them to rejected so the audit trail still shows they existed.
    let has_credits = segments
        .iter()
        .any(|s| s.segment_type == SegmentType::Credits);
    if !has_credits {
        let (kept, orphans): (Vec<_>, Vec<_>) = segments
            .into_iter()
            .partition(|s| s.segment_type != SegmentType::PostCreditsScene);
        segments = kept;
        for seg in orphans {
            rejected.push(RejectedCandidate {
                candidate: Candidate {
                    segment_type: seg.segment_type,
                    start_seconds: seg.start_seconds,
                    end_seconds: seg.end_seconds,
                    signals: CandidateSignals::default(),
                    file_duration: None,
                },
                reason: "orphaned post_credits_scene (no surviving credits segment)".into(),
                scored: Some(seg),
            });
        }
    }

    FusionOutcome { segments, rejected }
}

/// Returns `Some(reason)` if the candidate should be dropped before scoring
/// on duration / position grounds; `None` if it passes.
///
/// Public so candidate-construction code in `cache_detect` can filter
/// unsurvivable candidates out of the pool *before* fusion — keeps them from
/// absorbing signals (e.g. vision credit-text score) meant for the real
/// credits region elsewhere in the file.
pub fn pre_fusion_hard_reject(c: &Candidate) -> Option<String> {
    gate_by_bounds(c)
}

fn gate_by_bounds(c: &Candidate) -> Option<String> {
    let duration = c.end_seconds - c.start_seconds;
    match c.segment_type {
        SegmentType::Intro => {
            if duration < MIN_INTRO_DURATION {
                Some(format!(
                    "intro duration {:.1}s < MIN_INTRO_DURATION {:.1}s",
                    duration, MIN_INTRO_DURATION
                ))
            } else if duration > MAX_INTRO_DURATION {
                Some(format!(
                    "intro duration {:.1}s > MAX_INTRO_DURATION {:.1}s",
                    duration, MAX_INTRO_DURATION
                ))
            } else {
                None
            }
        }
        SegmentType::Credits => {
            if let Some(file_dur) = c.file_duration
                && file_dur > 0.0
            {
                let start_pct = c.start_seconds / file_dur;
                let max_dur = max_credits_duration(file_dur);
                if start_pct < MIN_CREDITS_START_PCT {
                    Some(format!(
                        "credits start_pct {:.2} < MIN_CREDITS_START_PCT {:.2}",
                        start_pct, MIN_CREDITS_START_PCT
                    ))
                } else if duration > max_dur {
                    Some(format!(
                        "credits duration {:.1}s > cap {:.1}s ({}× file or {}s abs)",
                        duration, max_dur, MAX_CREDITS_DURATION_PCT, MAX_CREDITS_DURATION_ABS
                    ))
                } else {
                    None
                }
            } else if duration > MAX_CREDITS_DURATION_ABS {
                Some(format!(
                    "credits duration {:.1}s > MAX_CREDITS_DURATION_ABS {:.1}s (no file_duration)",
                    duration, MAX_CREDITS_DURATION_ABS
                ))
            } else {
                None
            }
        }
        SegmentType::PostCreditsScene => {
            if duration > MAX_POST_CREDITS_DURATION {
                Some(format!(
                    "post_credits duration {:.1}s > MAX_POST_CREDITS_DURATION {:.1}s",
                    duration, MAX_POST_CREDITS_DURATION
                ))
            } else {
                None
            }
        }
        SegmentType::Recap => {
            if duration > 120.0 {
                Some(format!("recap duration {:.1}s > 120.0s", duration))
            } else {
                None
            }
        }
    }
}

fn score_one(candidate: Candidate) -> DetectedSegment {
    let confidence = compute_confidence(&candidate.segment_type, &candidate.signals);
    let veto_score = compute_veto(&candidate.segment_type, &candidate.signals);

    let mut sources = Vec::new();
    if candidate.signals.chromaprint_score.is_some() {
        sources.push("audio");
    }
    if candidate.signals.dhash_score.is_some() {
        sources.push("dhash");
    }
    if candidate.signals.visual_score.is_some() {
        sources.push("visual");
    }
    if candidate.signals.credit_text_score.is_some() {
        sources.push("text");
    }
    if candidate.signals.boundary_match.is_some() {
        sources.push("boundary");
    }
    if candidate.signals.chapter_agreement.is_some_and(|s| s > 0.0) {
        sources.push("chapter_confirmed");
    }

    DetectedSegment {
        segment_type: candidate.segment_type,
        start_seconds: candidate.start_seconds,
        end_seconds: candidate.end_seconds,
        confidence,
        veto_score,
        source: sources.join("+"),
    }
}

/// Compute base confidence from signal agreement.
///
/// | Signals | Confidence | Rationale |
/// |---------|-----------|-----------|
/// | Audio + dHash agree | 0.95 | Two independent methods found same thing |
/// | Audio high density (>0.85) | 0.80 | Near-perfect theme song match |
/// | dHash only, long sequence | 0.75 | Consistent title sequence, no audio theme |
/// | Credit text (vision) | 0.70 | Text overlay detected |
/// | Audio moderate (0.7-0.85) | 0.50 | Possible theme, needs more evidence |
/// | Chapter agreement | +0.05–0.10 | Chapter marker confirms detection |
fn compute_confidence(segment_type: &SegmentType, signals: &CandidateSignals) -> f64 {
    let audio_density = signals.chromaprint_score.unwrap_or(0.0);
    let has_audio = audio_density > 0.5;
    let has_dhash = signals.dhash_score.is_some_and(|s| s > 0.5);
    let has_credit_text = signals.credit_text_score.is_some_and(|s| s > 0.3);
    let has_visual = signals.visual_score.is_some_and(|s| s > 0.5);
    let has_boundary = signals.boundary_match.is_some();

    let mut confidence: f64 = if has_audio && has_dhash {
        // Two independent methods agree — highest confidence
        0.95
    } else if has_audio && audio_density > 0.85 {
        // Near-perfect audio match alone
        0.80
    } else if has_dhash && !has_audio {
        // Visual sequence only (title card without audio theme).
        // Use actual density as confidence — the vhash pair-count cap
        // already limits density for few-episode seasons.
        signals.dhash_score.unwrap_or(0.75).min(0.75)
    } else if *segment_type == SegmentType::Credits && has_credit_text {
        // Vision detected credit text overlay
        0.70
    } else if has_audio && has_visual {
        // Audio + visual agreement
        0.85
    } else if has_audio && audio_density > 0.7 {
        // Moderate audio match
        0.50
    } else if has_visual && !has_audio {
        // Visual-only
        0.65
    } else if has_boundary {
        // Boundary only
        0.35
    } else {
        0.0
    };

    // Boundary confirmation bonus
    if let Some(bm) = &signals.boundary_match
        && (has_audio || has_dhash || has_visual)
    {
        if bm.has_black_frame && bm.has_silence {
            confidence += 0.10;
        } else if bm.has_black_frame || bm.has_silence {
            confidence += 0.05;
        }
    }

    // Credit text detection bonus for credit candidates
    if *segment_type == SegmentType::Credits && has_credit_text && has_audio {
        confidence += 0.10;
    }

    // Chapter agreement bonus/penalty
    if let Some(chapter_score) = signals.chapter_agreement {
        confidence += chapter_score;
    }

    confidence.min(1.0)
}

fn compute_veto(segment_type: &SegmentType, signals: &CandidateSignals) -> f64 {
    let audio_density = signals.chromaprint_score.unwrap_or(0.0);
    let has_audio = audio_density > 0.5;
    let has_dhash = signals.dhash_score.is_some_and(|s| s > 0.5);
    let dhash_absent = signals.dhash_score.is_some_and(|s| s < 0.2);

    let mut veto: f64 = 0.0;

    // Audio match with NO dHash match AND low density → ambient audio coincidence
    if has_audio && dhash_absent && audio_density < 0.8 {
        veto = veto.max(0.8);
    }

    // Audio YES but legacy visual explicitly NO
    if has_audio && signals.visual_score.is_some_and(|s| s < 0.2) && !has_dhash {
        veto = veto.max(0.7);
    }

    // Dialogue detected in intro/credits region — but not when strong signals confirm it
    // (many shows roll credits over final scene dialogue)
    let has_strong_confirmation = (has_audio && audio_density > 0.85)
        || (has_dhash && signals.dhash_score.unwrap_or(0.0) > 0.85)
        || signals.credit_text_score.is_some_and(|s| s > 0.2);
    if signals.audio_profile == Some(AudioProfile::Dialogue)
        && matches!(segment_type, SegmentType::Intro | SegmentType::Credits)
        && !has_strong_confirmation
    {
        veto = veto.max(0.5);
    }

    // "Credits" with high visual variance (not static credits)
    if *segment_type == SegmentType::Credits
        && let Some(vs) = signals.visual_score
        && vs > 0.5
        && vs < 0.7
    {
        veto = veto.max(0.8);
    }

    // Post-credits in same visual cluster as credits
    if *segment_type == SegmentType::PostCreditsScene
        && let Some(vs) = signals.visual_score
        && vs > 0.9
    {
        veto = veto.max(0.9);
    }

    // Credits with no evidence at all — no boundary, no audio, no text,
    // no visual match. Historically this veto ignored dhash, which punished
    // shows like The Leftovers whose credits have byte-identical visuals
    // across episodes but no shared music cue (per-season composers vary).
    // A strong cross-episode vhash match IS evidence.
    if *segment_type == SegmentType::Credits {
        let no_boundary = signals.boundary_match.is_none()
            || signals
                .boundary_match
                .as_ref()
                .is_some_and(|bm| !bm.has_black_frame && !bm.has_silence);
        let no_audio = !has_audio;
        let no_text = !signals.credit_text_score.is_some_and(|s| s > 0.3);
        if no_boundary && no_audio && no_text && !has_dhash {
            veto = veto.max(0.4);
        }
    }

    veto.min(1.0)
}

#[cfg(test)]
mod tests {
    use super::super::BoundaryMatch;
    use super::*;

    fn make_candidate(seg_type: SegmentType, signals: CandidateSignals) -> Candidate {
        Candidate {
            segment_type: seg_type,
            start_seconds: 0.0,
            end_seconds: 90.0,
            file_duration: None,
            signals,
        }
    }

    #[test]
    fn audio_and_dhash_gives_highest_confidence() {
        let seg = score_one(make_candidate(
            SegmentType::Intro,
            CandidateSignals {
                chromaprint_score: Some(0.8),
                dhash_score: Some(0.8),
                ..Default::default()
            },
        ));
        assert!(
            (seg.confidence - 0.95).abs() < 0.01,
            "expected 0.95, got {}",
            seg.confidence
        );
        assert!(seg.veto_score < 0.01);
        assert!(seg.should_persist());
    }

    #[test]
    fn audio_high_density_gives_good_confidence() {
        let seg = score_one(make_candidate(
            SegmentType::Intro,
            CandidateSignals {
                chromaprint_score: Some(0.9),
                ..Default::default()
            },
        ));
        assert!(
            (seg.confidence - 0.80).abs() < 0.01,
            "expected 0.80, got {}",
            seg.confidence
        );
        assert!(seg.should_persist());
    }

    #[test]
    fn audio_moderate_density_gives_low_confidence() {
        let seg = score_one(make_candidate(
            SegmentType::Intro,
            CandidateSignals {
                chromaprint_score: Some(0.75),
                ..Default::default()
            },
        ));
        assert!(
            (seg.confidence - 0.50).abs() < 0.01,
            "expected 0.50, got {}",
            seg.confidence
        );
        // Moderate audio-only (0.50 confidence) is below MIN_EFFECTIVE_CONFIDENCE (0.55)
        assert!(!seg.should_persist());
    }

    #[test]
    fn dhash_only_gives_moderate_confidence() {
        let seg = score_one(make_candidate(
            SegmentType::Intro,
            CandidateSignals {
                dhash_score: Some(0.8),
                ..Default::default()
            },
        ));
        assert!(
            (seg.confidence - 0.75).abs() < 0.01,
            "expected 0.75, got {}",
            seg.confidence
        );
    }

    #[test]
    fn credit_text_detection_gives_confidence() {
        let seg = score_one(make_candidate(
            SegmentType::Credits,
            CandidateSignals {
                credit_text_score: Some(0.7),
                ..Default::default()
            },
        ));
        assert!(
            (seg.confidence - 0.70).abs() < 0.01,
            "expected 0.70, got {}",
            seg.confidence
        );
        assert!(seg.should_persist());
    }

    #[test]
    fn boundary_only_gives_low_confidence() {
        let seg = score_one(make_candidate(
            SegmentType::Intro,
            CandidateSignals {
                boundary_match: Some(BoundaryMatch {
                    has_black_frame: true,
                    has_silence: true,
                    refined_start: 0.0,
                    refined_end: 90.0,
                }),
                ..Default::default()
            },
        ));
        assert!(
            (seg.confidence - 0.35).abs() < 0.01,
            "expected 0.35, got {}",
            seg.confidence
        );
    }

    #[test]
    fn boundary_confirmation_adds_bonus() {
        let seg = score_one(make_candidate(
            SegmentType::Intro,
            CandidateSignals {
                chromaprint_score: Some(0.9),
                dhash_score: Some(0.8),
                boundary_match: Some(BoundaryMatch {
                    has_black_frame: true,
                    has_silence: true,
                    refined_start: 0.0,
                    refined_end: 90.0,
                }),
                ..Default::default()
            },
        ));
        assert!(
            (seg.confidence - 1.0).abs() < 0.06,
            "expected ~1.0 (0.95+0.10 capped), got {}",
            seg.confidence
        );
    }

    #[test]
    fn audio_no_dhash_low_density_vetoed() {
        let seg = score_one(make_candidate(
            SegmentType::Intro,
            CandidateSignals {
                chromaprint_score: Some(0.75),
                dhash_score: Some(0.1), // dHash explicitly absent
                ..Default::default()
            },
        ));
        assert!(
            seg.veto_score >= 0.8,
            "expected veto >= 0.8, got {}",
            seg.veto_score
        );
        assert!(!seg.should_persist());
    }

    #[test]
    fn audio_yes_visual_no_triggers_veto() {
        let seg = score_one(make_candidate(
            SegmentType::Intro,
            CandidateSignals {
                chromaprint_score: Some(0.8),
                visual_score: Some(0.1),
                ..Default::default()
            },
        ));
        assert!(
            seg.veto_score >= 0.7,
            "expected veto >= 0.7, got {}",
            seg.veto_score
        );
    }

    #[test]
    fn dialogue_in_intro_triggers_veto() {
        // Moderate signals without strong confirmation — dialogue veto should fire
        let seg = score_one(make_candidate(
            SegmentType::Intro,
            CandidateSignals {
                chromaprint_score: Some(0.75),
                dhash_score: Some(0.6),
                audio_profile: Some(AudioProfile::Dialogue),
                ..Default::default()
            },
        ));
        assert!(
            (seg.veto_score - 0.5).abs() < 0.01,
            "expected veto 0.5, got {}",
            seg.veto_score
        );
    }

    #[test]
    fn dialogue_veto_suppressed_by_strong_audio() {
        // Invincible S3 scenario: dialogue over credits, but chroma 0.999 confirms credits
        let seg = score_one(make_candidate(
            SegmentType::Credits,
            CandidateSignals {
                chromaprint_score: Some(0.999),
                dhash_score: Some(1.0),
                credit_text_score: Some(0.3),
                audio_profile: Some(AudioProfile::Dialogue),
                ..Default::default()
            },
        ));
        assert!(
            seg.veto_score < 0.01,
            "dialogue veto should be suppressed when strong signals confirm credits, got veto {}",
            seg.veto_score
        );
        assert!(
            seg.should_persist(),
            "credits with strong confirmation should persist, effective={}",
            seg.effective_confidence()
        );
    }

    #[test]
    fn dialogue_veto_suppressed_by_strong_dhash() {
        // Use Intro to isolate dialogue veto (Credits has additional veto rules)
        let seg = score_one(make_candidate(
            SegmentType::Intro,
            CandidateSignals {
                dhash_score: Some(0.9),
                audio_profile: Some(AudioProfile::Dialogue),
                ..Default::default()
            },
        ));
        assert!(
            seg.veto_score < 0.01,
            "dialogue veto should be suppressed by strong dhash, got veto {}",
            seg.veto_score
        );
    }

    #[test]
    fn dialogue_veto_suppressed_by_credit_text() {
        // Use Intro to isolate dialogue veto
        let seg = score_one(make_candidate(
            SegmentType::Intro,
            CandidateSignals {
                chromaprint_score: Some(0.6),
                credit_text_score: Some(0.5),
                audio_profile: Some(AudioProfile::Dialogue),
                ..Default::default()
            },
        ));
        assert!(
            seg.veto_score < 0.01,
            "dialogue veto should be suppressed by credit text, got veto {}",
            seg.veto_score
        );
    }

    #[test]
    fn dialogue_veto_fires_without_strong_confirmation() {
        // Dialogue with only moderate signals — veto should still apply
        let seg = score_one(make_candidate(
            SegmentType::Intro,
            CandidateSignals {
                chromaprint_score: Some(0.75),
                audio_profile: Some(AudioProfile::Dialogue),
                ..Default::default()
            },
        ));
        assert!(
            (seg.veto_score - 0.5).abs() < 0.01,
            "dialogue veto should fire without strong confirmation, got veto {}",
            seg.veto_score
        );
    }

    #[test]
    fn orphaned_post_credits_removed() {
        // Post-credits scene exists but credits were vetoed — post-credits should be removed
        let candidates = vec![
            // Credits candidate that will be vetoed (low audio, dhash absent)
            Candidate {
                segment_type: SegmentType::Credits,
                start_seconds: 1188.0,
                end_seconds: 1320.0,
                file_duration: Some(1400.0),
                signals: CandidateSignals {
                    chromaprint_score: Some(0.75),
                    dhash_score: Some(0.1),
                    ..Default::default()
                },
            },
            // Post-credits scene that would otherwise survive
            Candidate {
                segment_type: SegmentType::PostCreditsScene,
                start_seconds: 1320.0,
                end_seconds: 1400.0,
                file_duration: Some(1400.0),
                signals: CandidateSignals {
                    credit_text_score: Some(0.5),
                    boundary_match: Some(BoundaryMatch {
                        has_black_frame: true,
                        has_silence: true,
                        refined_start: 1320.0,
                        refined_end: 1400.0,
                    }),
                    ..Default::default()
                },
            },
        ];
        let results = score_candidates(candidates);
        assert!(
            !results
                .iter()
                .any(|s| s.segment_type == SegmentType::PostCreditsScene),
            "post-credits should be removed when credits are absent"
        );
    }

    #[test]
    fn post_credits_kept_when_credits_exist() {
        let candidates = vec![
            // Credits that will survive
            Candidate {
                segment_type: SegmentType::Credits,
                start_seconds: 1200.0,
                end_seconds: 1320.0,
                file_duration: Some(1400.0),
                signals: CandidateSignals {
                    chromaprint_score: Some(0.9),
                    dhash_score: Some(0.9),
                    ..Default::default()
                },
            },
            // Post-credits scene
            Candidate {
                segment_type: SegmentType::PostCreditsScene,
                start_seconds: 1320.0,
                end_seconds: 1400.0,
                file_duration: Some(1400.0),
                signals: CandidateSignals {
                    boundary_match: Some(BoundaryMatch {
                        has_black_frame: true,
                        has_silence: true,
                        refined_start: 1320.0,
                        refined_end: 1400.0,
                    }),
                    chromaprint_score: Some(0.8),
                    ..Default::default()
                },
            },
        ];
        let results = score_candidates(candidates);
        assert!(
            results
                .iter()
                .any(|s| s.segment_type == SegmentType::Credits),
            "credits should survive"
        );
        assert!(
            results
                .iter()
                .any(|s| s.segment_type == SegmentType::PostCreditsScene),
            "post-credits should be kept when credits exist"
        );
    }

    #[test]
    fn post_credits_same_cluster_triggers_high_veto() {
        let seg = score_one(make_candidate(
            SegmentType::PostCreditsScene,
            CandidateSignals {
                visual_score: Some(0.95),
                ..Default::default()
            },
        ));
        assert!(
            (seg.veto_score - 0.9).abs() < 0.01,
            "expected veto 0.9, got {}",
            seg.veto_score
        );
    }

    #[test]
    fn credits_no_signals_triggers_veto() {
        let seg = score_one(make_candidate(
            SegmentType::Credits,
            CandidateSignals {
                visual_score: Some(0.8),
                ..Default::default()
            },
        ));
        assert!(
            seg.veto_score >= 0.4,
            "expected veto >= 0.4, got {}",
            seg.veto_score
        );
    }

    /// Regression: vhash-only credits (strong cross-episode visual match, no
    /// audio theme, no vision text) must not be vetoed by the "no evidence"
    /// rule. Reproduces the Leftovers S01 case where credits had 9-pair vhash
    /// agreement at density 0.75+ yet fusion rejected them.
    #[test]
    fn vhash_only_credits_survives_no_evidence_veto() {
        let seg = score_one(Candidate {
            segment_type: SegmentType::Credits,
            start_seconds: 3306.0,
            end_seconds: 3367.0,
            file_duration: Some(3369.0),
            signals: CandidateSignals {
                dhash_score: Some(0.75),
                ..Default::default()
            },
        });
        assert!(
            seg.veto_score < 0.1,
            "vhash-only credits should not be vetoed, got veto {}",
            seg.veto_score
        );
        assert!(
            seg.should_persist(),
            "vhash-only credits with dhash 0.75 should persist (conf {} eff {})",
            seg.confidence,
            seg.effective_confidence()
        );
    }

    #[test]
    fn no_signals_produces_zero_confidence() {
        let seg = score_one(make_candidate(
            SegmentType::Intro,
            CandidateSignals::default(),
        ));
        assert!(seg.confidence < 0.01);
        assert!(!seg.should_persist());
    }

    #[test]
    fn score_candidates_filters_below_threshold() {
        let candidates = vec![
            make_candidate(
                SegmentType::Intro,
                CandidateSignals {
                    chromaprint_score: Some(0.9),
                    dhash_score: Some(0.8),
                    ..Default::default()
                },
            ),
            // Low audio, dHash explicitly absent → vetoed
            make_candidate(
                SegmentType::Credits,
                CandidateSignals {
                    chromaprint_score: Some(0.75),
                    dhash_score: Some(0.1),
                    ..Default::default()
                },
            ),
        ];
        let results = score_candidates(candidates);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].segment_type, SegmentType::Intro);
    }

    #[test]
    fn effective_confidence_formula() {
        let seg = DetectedSegment {
            segment_type: SegmentType::Intro,
            start_seconds: 0.0,
            end_seconds: 90.0,
            confidence: 0.8,
            veto_score: 0.5,
            source: "test".to_string(),
        };
        let eff = seg.effective_confidence();
        assert!((eff - 0.4).abs() < 0.001, "expected 0.4, got {}", eff);
        assert!(!seg.should_persist());
    }

    #[test]
    fn source_string_reflects_signals() {
        let seg = score_one(make_candidate(
            SegmentType::Intro,
            CandidateSignals {
                chromaprint_score: Some(0.8),
                dhash_score: Some(0.7),
                boundary_match: Some(BoundaryMatch {
                    has_black_frame: true,
                    has_silence: false,
                    refined_start: 0.0,
                    refined_end: 90.0,
                }),
                ..Default::default()
            },
        ));
        assert_eq!(seg.source, "audio+dhash+boundary");
    }

    #[test]
    fn max_intro_duration_rejects_long_intros() {
        let candidates = vec![
            Candidate {
                segment_type: SegmentType::Intro,
                start_seconds: 0.0,
                end_seconds: 180.0,
                file_duration: None,
                signals: CandidateSignals {
                    chromaprint_score: Some(0.9),
                    dhash_score: Some(0.8),
                    ..Default::default()
                },
            },
            Candidate {
                segment_type: SegmentType::Intro,
                start_seconds: 0.0,
                end_seconds: 90.0,
                file_duration: None,
                signals: CandidateSignals {
                    chromaprint_score: Some(0.9),
                    dhash_score: Some(0.8),
                    ..Default::default()
                },
            },
        ];
        let results = score_candidates(candidates);
        assert_eq!(results.len(), 1);
        assert!(
            (results[0].end_seconds - 90.0).abs() < 0.01,
            "Only the 90s intro should survive"
        );
    }

    #[test]
    fn credits_too_early_rejected() {
        let candidates = vec![Candidate {
            segment_type: SegmentType::Credits,
            start_seconds: 660.0, // 50%
            end_seconds: 760.0,
            file_duration: Some(1320.0),
            signals: CandidateSignals {
                chromaprint_score: Some(0.9),
                dhash_score: Some(0.8),
                ..Default::default()
            },
        }];
        let results = score_candidates(candidates);
        assert_eq!(
            results.len(),
            0,
            "Credits starting at 50% should be rejected"
        );
    }

    #[test]
    fn credits_at_normal_position_accepted() {
        let candidates = vec![Candidate {
            segment_type: SegmentType::Credits,
            start_seconds: 1188.0, // 90%
            end_seconds: 1320.0,
            file_duration: Some(1320.0),
            signals: CandidateSignals {
                chromaprint_score: Some(0.9),
                dhash_score: Some(0.8),
                ..Default::default()
            },
        }];
        let results = score_candidates(candidates);
        assert_eq!(results.len(), 1, "Credits at 90% should be accepted");
    }

    #[test]
    fn credits_too_long_rejected() {
        let candidates = vec![Candidate {
            segment_type: SegmentType::Credits,
            start_seconds: 990.0,
            end_seconds: 1320.0,
            file_duration: Some(1320.0),
            signals: CandidateSignals {
                chromaprint_score: Some(0.9),
                dhash_score: Some(0.8),
                ..Default::default()
            },
        }];
        let results = score_candidates(candidates);
        assert_eq!(
            results.len(),
            0,
            "Credits lasting 25% of episode should be rejected"
        );
    }

    #[test]
    fn post_credits_too_long_rejected() {
        let candidates = vec![Candidate {
            segment_type: SegmentType::PostCreditsScene,
            start_seconds: 1000.0,
            end_seconds: 1200.0,
            file_duration: None,
            signals: CandidateSignals {
                boundary_match: Some(BoundaryMatch {
                    has_black_frame: false,
                    has_silence: true,
                    refined_start: 1000.0,
                    refined_end: 1200.0,
                }),
                ..Default::default()
            },
        }];
        let results = score_candidates(candidates);
        assert_eq!(results.len(), 0, "Post-credits > 120s should be rejected");
    }
}
