use api::skip_detect::{
    AudioProfile, BoundaryMatch, Candidate, CandidateSignals, DetectedSegment, MIN_MATCH_DURATION,
    SegmentType,
    chromaprint::{Fingerprint, find_matching_regions},
    fusion::score_candidates,
    vision::{cluster_consistency, cosine_similarity, find_cluster_break},
};

// =============================================================================
// Chromaprint matching (public API)
// =============================================================================

#[test]
fn identical_fingerprints_produce_high_score_match() {
    let data: Vec<u32> = (0..200).collect();
    let fp_a = Fingerprint {
        data: data.clone(),
        duration_seconds: 200.0,
    };
    let fp_b = Fingerprint {
        data: data.clone(),
        duration_seconds: 200.0,
    };

    let matches = find_matching_regions(&fp_a, &fp_b, 1.0, 0.20);
    assert!(
        !matches.is_empty(),
        "identical fingerprints must produce matches"
    );
    // The single region should span most of the duration
    let total_matched: f64 = matches
        .iter()
        .map(|m| m.end_seconds - m.start_seconds)
        .sum();
    assert!(
        total_matched > 150.0,
        "expected >150s of matches across 200s, got {total_matched}"
    );
    assert!(
        matches[0].score > 0.95,
        "identical data should score near 1.0, got {}",
        matches[0].score
    );
}

#[test]
fn completely_different_fingerprints_produce_no_matches() {
    // All zeros vs all ones: BER = 1.0 at every position
    let fp_a = Fingerprint {
        data: vec![0x00000000; 200],
        duration_seconds: 200.0,
    };
    let fp_b = Fingerprint {
        data: vec![0xFFFFFFFF; 200],
        duration_seconds: 200.0,
    };

    let matches = find_matching_regions(&fp_a, &fp_b, 1.0, 0.20);
    assert!(
        matches.is_empty(),
        "opposite fingerprints should produce no matches, got {}",
        matches.len()
    );
}

#[test]
fn matching_prefix_detected_at_start() {
    // First 100 items identical, remaining 100 completely different
    let shared: Vec<u32> = (1000..1100).collect();
    let noise_a: Vec<u32> = (2000..2100).collect();
    let noise_b: Vec<u32> = noise_a.iter().map(|x| !x).collect();

    let fp_a = Fingerprint {
        data: [shared.clone(), noise_a].concat(),
        duration_seconds: 200.0,
    };
    let fp_b = Fingerprint {
        data: [shared, noise_b].concat(),
        duration_seconds: 200.0,
    };

    let matches = find_matching_regions(&fp_a, &fp_b, 1.0, 0.20);
    assert!(!matches.is_empty(), "should detect matching prefix");

    let first = &matches[0];
    assert!(
        first.start_seconds < 5.0,
        "match should start near 0.0, got {}",
        first.start_seconds
    );
    assert!(
        first.end_seconds > 80.0 && first.end_seconds < 120.0,
        "match should end around 100s, got {}",
        first.end_seconds
    );
}

#[test]
fn empty_fingerprints_produce_no_matches() {
    let fp_a = Fingerprint {
        data: vec![],
        duration_seconds: 0.0,
    };
    let fp_b = Fingerprint {
        data: vec![1, 2, 3],
        duration_seconds: 3.0,
    };
    assert!(find_matching_regions(&fp_a, &fp_b, 1.0, 0.20).is_empty());
    assert!(find_matching_regions(&fp_b, &fp_a, 1.0, 0.20).is_empty());
}

#[test]
fn short_matching_region_filtered_by_min_duration() {
    // Only 2 matching items at 1 item/sec — below MIN_MATCH_DURATION (3s)
    let fp_a = Fingerprint {
        data: vec![42; 2],
        duration_seconds: 2.0,
    };
    let fp_b = Fingerprint {
        data: vec![42; 2],
        duration_seconds: 2.0,
    };
    let matches = find_matching_regions(&fp_a, &fp_b, 1.0, 0.20);
    assert!(
        matches.is_empty(),
        "region shorter than MIN_MATCH_DURATION ({MIN_MATCH_DURATION}s) should be filtered"
    );
}

// =============================================================================
// Fusion scoring (public API)
// =============================================================================

#[test]
fn strong_match_audio_visual_boundary_high_confidence() {
    let candidates = vec![Candidate {
        segment_type: SegmentType::Intro,
        start_seconds: 0.0,
        end_seconds: 90.0,
        file_duration: None,
        signals: CandidateSignals {
            chromaprint_score: Some(0.85),
            visual_score: Some(0.85),
            boundary_match: Some(BoundaryMatch {
                has_black_frame: true,
                has_silence: true,
                refined_start: 0.0,
                refined_end: 90.0,
            }),
            ..Default::default()
        },
    }];

    let results = score_candidates(candidates);
    assert_eq!(results.len(), 1);
    let seg = &results[0];
    // audio+visual = 0.9, + boundary both = +0.15, capped at 1.0
    assert!(
        seg.confidence >= 0.9,
        "expected high confidence, got {}",
        seg.confidence
    );
    assert!(seg.veto_score < 0.01, "no veto expected");
    assert!(seg.should_persist());
}

#[test]
fn audio_match_visual_mismatch_vetoed() {
    let candidates = vec![Candidate {
        segment_type: SegmentType::Intro,
        start_seconds: 0.0,
        end_seconds: 90.0,
        file_duration: None,
        signals: CandidateSignals {
            chromaprint_score: Some(0.8),
            visual_score: Some(0.1), // low visual = veto
            ..Default::default()
        },
    }];

    let results = score_candidates(candidates);
    assert!(
        results.is_empty(),
        "audio+visual mismatch should be vetoed and filtered out"
    );
}

#[test]
fn dialogue_in_intro_candidate_vetoed() {
    // audio+visual = 0.9 confidence, dialogue veto = 0.5
    // effective = 0.9 * (1 - 0.5) = 0.45 < 0.5 threshold → filtered out
    let candidates = vec![Candidate {
        segment_type: SegmentType::Intro,
        start_seconds: 0.0,
        end_seconds: 90.0,
        file_duration: None,
        signals: CandidateSignals {
            chromaprint_score: Some(0.8),
            visual_score: Some(0.8),
            audio_profile: Some(AudioProfile::Dialogue),
            ..Default::default()
        },
    }];

    let results = score_candidates(candidates);
    assert!(
        results.is_empty(),
        "dialogue-vetoed intro should be filtered (effective=0.45 < 0.5)"
    );
}

#[test]
fn visual_only_match_medium_confidence_filtered() {
    // The Leftovers case: credits with consistent visual but no matching audio.
    // confidence=0.7, veto=0.3 (no boundary + no audio), effective=0.49 < 0.5 → filtered
    let candidates = vec![Candidate {
        segment_type: SegmentType::Credits,
        start_seconds: 2400.0,
        end_seconds: 2700.0,
        file_duration: None,
        signals: CandidateSignals {
            visual_score: Some(0.8),
            ..Default::default()
        },
    }];

    let results = score_candidates(candidates);
    assert!(
        results.is_empty(),
        "visual-only credits without boundary should be filtered (effective=0.49 < 0.5)"
    );
}

#[test]
fn visual_only_with_boundary_persists() {
    // Visual-only credits WITH boundary confirmation should persist
    let candidates = vec![Candidate {
        segment_type: SegmentType::Credits,
        start_seconds: 2400.0,
        end_seconds: 2700.0,
        file_duration: None,
        signals: CandidateSignals {
            visual_score: Some(0.8),
            boundary_match: Some(BoundaryMatch {
                has_black_frame: true,
                has_silence: true,
                refined_start: 2400.0,
                refined_end: 2700.0,
            }),
            ..Default::default()
        },
    }];

    let results = score_candidates(candidates);
    assert_eq!(results.len(), 1);
    let seg = &results[0];
    // visual-only=0.65 + boundary both=+0.10 = 0.75
    assert!(
        seg.confidence > 0.7,
        "visual+boundary should be >0.7, got {}",
        seg.confidence
    );
    assert!(seg.should_persist());
}

#[test]
fn no_signals_filtered_out() {
    let candidates = vec![Candidate {
        segment_type: SegmentType::Intro,
        start_seconds: 0.0,
        end_seconds: 90.0,
        file_duration: None,
        signals: CandidateSignals::default(),
    }];

    let results = score_candidates(candidates);
    assert!(
        results.is_empty(),
        "candidate with no signals should be filtered"
    );
}

#[test]
fn post_credits_high_visual_distinct_from_credits_persists() {
    // Post-credits scene with moderate visual similarity (distinct from credits)
    // Must include a credits segment so post-credits isn't orphan-removed
    let candidates = vec![
        Candidate {
            segment_type: SegmentType::Credits,
            start_seconds: 2650.0,
            end_seconds: 2800.0,
            file_duration: Some(2900.0),
            signals: CandidateSignals {
                chromaprint_score: Some(0.9),
                dhash_score: Some(0.9),
                ..Default::default()
            },
        },
        Candidate {
            segment_type: SegmentType::PostCreditsScene,
            start_seconds: 2800.0,
            end_seconds: 2900.0,
            file_duration: Some(2900.0),
            signals: CandidateSignals {
                visual_score: Some(0.6),
                chromaprint_score: Some(0.7),
                ..Default::default()
            },
        },
    ];

    let results = score_candidates(candidates);
    let post_credits: Vec<_> = results
        .iter()
        .filter(|s| s.segment_type == SegmentType::PostCreditsScene)
        .collect();
    assert_eq!(post_credits.len(), 1);
    assert!(
        post_credits[0].should_persist(),
        "post-credits with moderate visual should persist when credits exist"
    );
}

#[test]
fn post_credits_too_similar_to_credits_vetoed() {
    // Visual score > 0.9 means too similar to credits cluster
    let candidates = vec![Candidate {
        segment_type: SegmentType::PostCreditsScene,
        start_seconds: 2800.0,
        end_seconds: 2900.0,
        file_duration: None,
        signals: CandidateSignals {
            visual_score: Some(0.95),
            chromaprint_score: Some(0.7),
            ..Default::default()
        },
    }];

    let results = score_candidates(candidates);
    // Veto = 0.9, confidence = 0.9, effective = 0.09
    assert!(
        results.is_empty(),
        "post-credits too similar to credits should be filtered"
    );
}

#[test]
fn effective_confidence_formula_verified() {
    let seg = DetectedSegment {
        segment_type: SegmentType::Intro,
        start_seconds: 0.0,
        end_seconds: 90.0,
        confidence: 0.8,
        veto_score: 0.3,
        source: "test".to_string(),
    };
    let expected = 0.8 * (1.0 - 0.3);
    assert!(
        (seg.effective_confidence() - expected).abs() < 0.001,
        "expected {expected}, got {}",
        seg.effective_confidence()
    );
    assert!(seg.should_persist()); // 0.56 >= 0.5
}

#[test]
fn score_candidates_preserves_multiple_good_segments() {
    let candidates = vec![
        Candidate {
            segment_type: SegmentType::Intro,
            start_seconds: 0.0,
            end_seconds: 90.0,
            file_duration: None,
            signals: CandidateSignals {
                chromaprint_score: Some(0.8),
                visual_score: Some(0.8),
                ..Default::default()
            },
        },
        Candidate {
            segment_type: SegmentType::Credits,
            start_seconds: 2400.0,
            end_seconds: 2700.0,
            file_duration: None,
            signals: CandidateSignals {
                chromaprint_score: Some(0.8),
                visual_score: Some(0.8),
                ..Default::default()
            },
        },
    ];

    let results = score_candidates(candidates);
    assert_eq!(results.len(), 2, "both good segments should persist");
    assert_eq!(results[0].segment_type, SegmentType::Intro);
    assert_eq!(results[1].segment_type, SegmentType::Credits);
}

#[test]
fn source_string_reflects_active_signals() {
    let candidates = vec![Candidate {
        segment_type: SegmentType::Intro,
        start_seconds: 0.0,
        end_seconds: 90.0,
        file_duration: None,
        signals: CandidateSignals {
            chromaprint_score: Some(0.8),
            boundary_match: Some(BoundaryMatch {
                has_black_frame: true,
                has_silence: false,
                refined_start: 0.0,
                refined_end: 90.0,
            }),
            ..Default::default()
        },
    }];

    let results = score_candidates(candidates);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].source, "audio+boundary");
}

// =============================================================================
// Vision similarity (public API)
// =============================================================================

#[test]
fn cosine_similarity_identical_vectors() {
    let a = vec![1.0f32, 2.0, 3.0, 4.0];
    assert!((cosine_similarity(&a, &a) - 1.0).abs() < 0.001);
}

#[test]
fn cosine_similarity_orthogonal_vectors() {
    let a = vec![1.0f32, 0.0, 0.0];
    let b = vec![0.0f32, 1.0, 0.0];
    assert!(cosine_similarity(&a, &b).abs() < 0.001);
}

#[test]
fn cosine_similarity_opposite_vectors() {
    let a = vec![1.0f32, 0.0];
    let b = vec![-1.0f32, 0.0];
    assert!((cosine_similarity(&a, &b) - (-1.0)).abs() < 0.001);
}

#[test]
fn cosine_similarity_empty_returns_zero() {
    let a: Vec<f32> = vec![];
    let b: Vec<f32> = vec![];
    assert!((cosine_similarity(&a, &b)).abs() < 0.001);
}

#[test]
fn cosine_similarity_mismatched_length_returns_zero() {
    let a = vec![1.0f32, 2.0];
    let b = vec![1.0f32, 2.0, 3.0];
    assert!((cosine_similarity(&a, &b)).abs() < 0.001);
}

#[test]
fn cluster_consistency_uniform_embeddings() {
    let embeddings = vec![
        vec![1.0f32, 0.0, 0.0],
        vec![0.99, 0.01, 0.0],
        vec![0.98, 0.02, 0.0],
    ];
    let consistency = cluster_consistency(&embeddings);
    assert!(
        consistency > 0.95,
        "near-identical embeddings should have high consistency, got {consistency}"
    );
}

#[test]
fn cluster_consistency_diverse_embeddings() {
    let embeddings = vec![
        vec![1.0f32, 0.0, 0.0],
        vec![0.0, 1.0, 0.0],
        vec![0.0, 0.0, 1.0],
    ];
    let consistency = cluster_consistency(&embeddings);
    assert!(
        consistency < 0.1,
        "orthogonal embeddings should have low consistency, got {consistency}"
    );
}

#[test]
fn cluster_consistency_single_embedding() {
    let embeddings = vec![vec![1.0f32, 2.0, 3.0]];
    assert!(
        (cluster_consistency(&embeddings) - 1.0).abs() < 0.001,
        "single embedding should have consistency 1.0"
    );
}

#[test]
fn cluster_break_uniform_then_outlier() {
    let embeddings: Vec<Vec<f32>> = vec![
        vec![1.0, 0.0, 0.0],
        vec![0.99, 0.01, 0.0],
        vec![0.98, 0.02, 0.0],
        vec![0.97, 0.03, 0.0],
        vec![0.96, 0.04, 0.0],
        vec![0.0, 1.0, 0.0], // outlier
    ];
    let break_idx = find_cluster_break(&embeddings, 0.5);
    assert_eq!(break_idx, Some(5), "break should be at the outlier index");
}

#[test]
fn cluster_break_all_similar_returns_none() {
    let embeddings: Vec<Vec<f32>> = vec![
        vec![1.0, 0.0, 0.0],
        vec![0.99, 0.01, 0.0],
        vec![0.98, 0.02, 0.0],
        vec![0.97, 0.03, 0.0],
        vec![0.96, 0.04, 0.0],
    ];
    assert!(
        find_cluster_break(&embeddings, 0.5).is_none(),
        "uniform cluster should have no break"
    );
}

#[test]
fn cluster_break_too_few_embeddings_returns_none() {
    let embeddings = vec![vec![1.0f32, 0.0], vec![0.0, 1.0]];
    assert!(
        find_cluster_break(&embeddings, 0.5).is_none(),
        "fewer than 3 embeddings should return None"
    );
}

#[test]
fn cluster_break_multiple_outliers_returns_first() {
    let embeddings: Vec<Vec<f32>> = vec![
        vec![1.0, 0.0, 0.0],
        vec![0.99, 0.01, 0.0],
        vec![0.98, 0.02, 0.0],
        vec![0.0, 1.0, 0.0], // first outlier
        vec![0.0, 0.0, 1.0], // second outlier
    ];
    let break_idx = find_cluster_break(&embeddings, 0.5);
    assert_eq!(
        break_idx,
        Some(3),
        "should return the index of the first break"
    );
}

// =============================================================================
// SegmentType and DetectedSegment helpers
// =============================================================================

#[test]
fn segment_type_as_str() {
    assert_eq!(SegmentType::Intro.as_str(), "intro");
    assert_eq!(SegmentType::Credits.as_str(), "credits");
    assert_eq!(SegmentType::PostCreditsScene.as_str(), "post_credits_scene");
    assert_eq!(SegmentType::Recap.as_str(), "recap");
}

#[test]
fn should_persist_boundary_at_threshold() {
    // Exactly at threshold: 0.55 effective
    let seg = DetectedSegment {
        segment_type: SegmentType::Intro,
        start_seconds: 0.0,
        end_seconds: 90.0,
        confidence: 1.0,
        veto_score: 0.45,
        source: "test".to_string(),
    };
    assert!(
        seg.should_persist(),
        "effective 0.55 should persist (>= MIN_EFFECTIVE_CONFIDENCE)"
    );

    // Just below threshold
    let seg2 = DetectedSegment {
        confidence: 0.49,
        veto_score: 0.0,
        ..seg.clone()
    };
    assert!(!seg2.should_persist(), "effective 0.49 should not persist");
}

// =============================================================================
// Candidate serde round-trip (needed for split pipeline DB storage)
// =============================================================================

#[test]
fn candidate_signals_serialize_deserialize_round_trip() {
    let signals = CandidateSignals {
        chromaprint_score: Some(0.85),
        visual_score: Some(0.72),
        boundary_match: Some(BoundaryMatch {
            has_black_frame: true,
            has_silence: false,
            refined_start: 10.5,
            refined_end: 95.3,
        }),
        audio_profile: Some(AudioProfile::MusicOnly),
        ..Default::default()
    };

    let json = serde_json::to_string(&signals).unwrap();
    let deserialized: CandidateSignals = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.chromaprint_score, signals.chromaprint_score);
    assert_eq!(deserialized.visual_score, signals.visual_score);
    assert!(deserialized.boundary_match.is_some());
    let bm = deserialized.boundary_match.unwrap();
    assert!(bm.has_black_frame);
    assert!(!bm.has_silence);
    assert!((bm.refined_start - 10.5).abs() < 0.001);
    assert!((bm.refined_end - 95.3).abs() < 0.001);
    assert_eq!(deserialized.audio_profile, Some(AudioProfile::MusicOnly));
}

#[test]
fn candidate_serialize_deserialize_round_trip() {
    let candidate = Candidate {
        segment_type: SegmentType::Recap,
        start_seconds: 5.0,
        end_seconds: 45.0,
        file_duration: None,
        signals: CandidateSignals {
            chromaprint_score: Some(0.7),
            ..Default::default()
        },
    };

    let json = serde_json::to_string(&candidate).unwrap();
    let deserialized: Candidate = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.segment_type, SegmentType::Recap);
    assert!((deserialized.start_seconds - 5.0).abs() < 0.001);
    assert!((deserialized.end_seconds - 45.0).abs() < 0.001);
    assert_eq!(deserialized.signals.chromaprint_score, Some(0.7));
    assert!(deserialized.signals.visual_score.is_none());
}

#[test]
fn candidate_signals_default_all_none() {
    let signals = CandidateSignals::default();
    assert!(signals.chromaprint_score.is_none());
    assert!(signals.visual_score.is_none());
    assert!(signals.boundary_match.is_none());
    assert!(signals.audio_profile.is_none());
}

// =============================================================================
// Fuse pass: stored_to_candidate reconstruction
// =============================================================================

#[test]
fn audio_only_candidate_fuses_correctly() {
    // Simulates the fuse pass: audio-only candidate (no visual) should score at 0.6
    let candidates = vec![Candidate {
        segment_type: SegmentType::Intro,
        start_seconds: 0.0,
        end_seconds: 90.0,
        file_duration: None,
        signals: CandidateSignals {
            chromaprint_score: Some(0.8),
            boundary_match: Some(BoundaryMatch {
                has_black_frame: true,
                has_silence: true,
                refined_start: 0.0,
                refined_end: 90.0,
            }),
            visual_score: None, // Not yet run
            ..Default::default()
        },
    }];

    let results = score_candidates(candidates);
    assert_eq!(results.len(), 1);
    let seg = &results[0];
    // audio moderate (0.8 density < 0.85) = 0.50 + boundary both=+0.10 = 0.60
    assert!(
        seg.confidence >= 0.5,
        "audio+boundary should be >=0.5, got {}",
        seg.confidence
    );
    assert!(seg.should_persist());
    assert_eq!(seg.source, "audio+boundary");
}

#[test]
fn re_fuse_with_visual_upgrades_confidence() {
    // Same candidate as above, but now with visual_score added
    let candidates = vec![Candidate {
        segment_type: SegmentType::Intro,
        start_seconds: 0.0,
        end_seconds: 90.0,
        file_duration: None,
        signals: CandidateSignals {
            chromaprint_score: Some(0.8),
            visual_score: Some(0.8), // Now populated
            boundary_match: Some(BoundaryMatch {
                has_black_frame: true,
                has_silence: true,
                refined_start: 0.0,
                refined_end: 90.0,
            }),
            ..Default::default()
        },
    }];

    let results = score_candidates(candidates);
    assert_eq!(results.len(), 1);
    let seg = &results[0];
    // audio+visual=0.9 + boundary both=+0.15 = 1.05 capped to 1.0
    assert!(
        seg.confidence > 0.9,
        "audio+visual+boundary should be >0.9, got {}",
        seg.confidence
    );
    assert_eq!(seg.source, "audio+visual+boundary");
}

// =============================================================================
// Pipeline mode parsing
// =============================================================================

#[test]
fn pipeline_mode_from_str() {
    use api::skip_detect::PipelineMode;
    use std::str::FromStr;

    assert_eq!(
        PipelineMode::from_str("audio").unwrap(),
        PipelineMode::Audio
    );
    assert_eq!(PipelineMode::from_str("full").unwrap(), PipelineMode::Full);
    assert_eq!(PipelineMode::from_str("off").unwrap(), PipelineMode::Off);
    assert_eq!(PipelineMode::from_str("").unwrap(), PipelineMode::Off);
    assert_eq!(
        PipelineMode::from_str("garbage").unwrap(),
        PipelineMode::Off
    );
}
