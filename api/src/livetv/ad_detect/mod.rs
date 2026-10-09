//! Ad detection (Decision 9). Marks only, never cuts.
//!
//! Precision-first: a false positive is worth roughly 5-10 false negatives, so a
//! break is only marked when two independent signals agree, the emitted span is
//! biased inwards, and wildly disagreeing detectors mark nothing at all.

pub mod bank;
pub mod blocks;
pub mod teletext;
pub mod vision_signal;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdBreak {
    pub start_seconds: f64,
    pub end_seconds: f64,
    /// Constrained by the `livetv_ad_breaks.method` CHECK.
    pub method: &'static str,
    pub confidence: f64,
    pub ads_known: usize,
    pub ads_total: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Signals {
    pub teletext: Vec<(f64, f64)>,
    pub blocks: Vec<(f64, f64, f64)>,
    pub bank: Vec<(f64, f64, usize)>,
    pub vision: Vec<(f64, f64)>,
}

#[derive(Debug, Clone)]
pub struct FuseConfig {
    pub edge_margin_secs: f64,
    pub min_overlap_fraction: f64,
    pub max_disagreement_ratio: f64,
    pub min_break_secs: f64,
    pub max_break_secs: f64,
    pub typical_ad_secs: f64,
}

impl Default for FuseConfig {
    fn default() -> Self {
        Self {
            edge_margin_secs: 0.5,
            min_overlap_fraction: 0.5,
            max_disagreement_ratio: 2.0,
            min_break_secs: 15.0,
            max_break_secs: 600.0,
            typical_ad_secs: 30.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Teletext,
    Blocks,
    Bank,
    Vision,
}

#[derive(Debug, Clone, Copy)]
struct Span {
    kind: Kind,
    start: f64,
    end: f64,
    ads_known: usize,
}

impl Span {
    fn len(&self) -> f64 {
        self.end - self.start
    }
}

fn collect(signals: &Signals) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut push = |kind: Kind, start: f64, end: f64, ads_known: usize| {
        if end > start {
            spans.push(Span {
                kind,
                start,
                end,
                ads_known,
            });
        }
    };
    for (s, e) in &signals.teletext {
        push(Kind::Teletext, *s, *e, 0);
    }
    for (s, e, _) in &signals.blocks {
        push(Kind::Blocks, *s, *e, 0);
    }
    for (s, e, known) in &signals.bank {
        push(Kind::Bank, *s, *e, *known);
    }
    for (s, e) in &signals.vision {
        push(Kind::Vision, *s, *e, 0);
    }
    spans.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    spans
}

fn agrees(a: &Span, b: &Span, cfg: &FuseConfig) -> bool {
    let overlap = a.end.min(b.end) - a.start.max(b.start);
    if overlap <= 0.0 {
        return false;
    }
    overlap >= cfg.min_overlap_fraction * a.len().min(b.len())
}

pub fn fuse(signals: &Signals, cfg: &FuseConfig) -> Vec<AdBreak> {
    let spans = collect(signals);
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, span) in spans.iter().enumerate() {
        let joined = groups
            .iter()
            .position(|g| g.iter().any(|j| agrees(&spans[*j], span, cfg)));
        match joined {
            Some(gi) => groups[gi].push(i),
            None => groups.push(vec![i]),
        }
    }

    let mut out: Vec<AdBreak> = Vec::new();
    for group in groups {
        let members: Vec<Span> = group.iter().map(|i| spans[*i]).collect();
        let mut kinds: Vec<Kind> = members.iter().map(|m| m.kind).collect();
        kinds.sort_by_key(|k| *k as u8);
        kinds.dedup();

        let ads_known: usize = members.iter().map(|m| m.ads_known).sum();
        // A byte-identical known ad is not a heuristic, so a bank-only span is
        // allowed to stand alone where a single heuristic signal never is.
        let bank_only = kinds.len() == 1 && kinds[0] == Kind::Bank && ads_known > 0;
        if kinds.len() < 2 && !bank_only {
            continue;
        }

        let inter_start = members
            .iter()
            .map(|m| m.start)
            .fold(f64::NEG_INFINITY, f64::max);
        let inter_end = members.iter().map(|m| m.end).fold(f64::INFINITY, f64::min);
        let union_start = members
            .iter()
            .map(|m| m.start)
            .fold(f64::INFINITY, f64::min);
        let union_end = members
            .iter()
            .map(|m| m.end)
            .fold(f64::NEG_INFINITY, f64::max);
        let inter_len = inter_end - inter_start;
        let union_len = union_end - union_start;
        if inter_len <= 0.0 || union_len <= 0.0 {
            continue;
        }
        if union_len > cfg.max_disagreement_ratio * inter_len {
            continue;
        }

        let start = inter_start + cfg.edge_margin_secs;
        let end = inter_end - cfg.edge_margin_secs;
        let length = end - start;
        if length < cfg.min_break_secs || length > cfg.max_break_secs {
            continue;
        }

        let agreement = (inter_len / union_len).clamp(0.0, 1.0);
        let breadth = ((kinds.len() as f64 - 1.0) / 3.0).clamp(0.0, 1.0);
        let known_bonus = if ads_known > 0 { 0.15 } else { 0.0 };
        let confidence = (0.45 + 0.30 * breadth + 0.25 * agreement + known_bonus).clamp(0.0, 1.0);

        let estimated = (length / cfg.typical_ad_secs).round().max(1.0) as usize;
        out.push(AdBreak {
            start_seconds: start,
            end_seconds: end,
            method: if bank_only { "fingerprint" } else { "combined" },
            confidence,
            ads_known,
            ads_total: ads_known.max(estimated),
        });
    }

    out.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut kept: Vec<AdBreak> = Vec::new();
    for candidate in out {
        let clashes = kept.iter().any(|k| {
            candidate.start_seconds < k.end_seconds && k.start_seconds < candidate.end_seconds
        });
        if !clashes {
            kept.push(candidate);
        }
    }
    kept.sort_by(|a, b| {
        a.start_seconds
            .partial_cmp(&b.start_seconds)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_agreeing_signals_produce_one_inward_biased_break() {
        let signals = Signals {
            teletext: vec![(164.0, 374.0)],
            blocks: vec![(164.5, 373.0, 0.9)],
            ..Default::default()
        };
        let breaks = fuse(&signals, &FuseConfig::default());
        assert_eq!(breaks.len(), 1, "{breaks:?}");
        assert_eq!(breaks[0].method, "combined");
        assert!(breaks[0].start_seconds > 164.5, "{breaks:?}");
        assert!(breaks[0].end_seconds < 373.0, "{breaks:?}");
        assert!(breaks[0].confidence > 0.5 && breaks[0].confidence <= 1.0);
        assert_eq!(breaks[0].ads_known, 0);
        assert!(breaks[0].ads_total >= 6, "{breaks:?}");
    }

    #[test]
    fn one_heuristic_signal_alone_marks_nothing() {
        let signals = Signals {
            teletext: vec![(164.0, 374.0)],
            ..Default::default()
        };
        assert!(fuse(&signals, &FuseConfig::default()).is_empty());

        let signals = Signals {
            blocks: vec![(164.0, 374.0, 1.0)],
            ..Default::default()
        };
        assert!(fuse(&signals, &FuseConfig::default()).is_empty());
    }

    #[test]
    fn wildly_disagreeing_signals_mark_nothing() {
        let signals = Signals {
            teletext: vec![(100.0, 400.0)],
            blocks: vec![(340.0, 400.0, 0.9)],
            ..Default::default()
        };
        assert!(fuse(&signals, &FuseConfig::default()).is_empty());
    }

    #[test]
    fn a_confirmed_bank_span_stands_on_its_own() {
        let signals = Signals {
            bank: vec![(1254.0, 1364.0, 3)],
            ..Default::default()
        };
        let breaks = fuse(&signals, &FuseConfig::default());
        assert_eq!(breaks.len(), 1, "{breaks:?}");
        assert_eq!(breaks[0].method, "fingerprint");
        assert_eq!(breaks[0].ads_known, 3);
    }

    #[test]
    fn a_bank_hit_that_knows_no_ads_is_not_enough() {
        let signals = Signals {
            bank: vec![(1254.0, 1364.0, 0)],
            ..Default::default()
        };
        assert!(fuse(&signals, &FuseConfig::default()).is_empty());
    }

    #[test]
    fn a_break_under_the_minimum_length_is_dropped() {
        let signals = Signals {
            teletext: vec![(100.0, 112.0)],
            blocks: vec![(100.0, 112.0, 0.9)],
            ..Default::default()
        };
        assert!(fuse(&signals, &FuseConfig::default()).is_empty());
    }

    #[test]
    fn four_agreeing_signals_score_higher_than_two() {
        let two = fuse(
            &Signals {
                teletext: vec![(164.0, 374.0)],
                blocks: vec![(164.0, 374.0, 0.9)],
                ..Default::default()
            },
            &FuseConfig::default(),
        );
        let four = fuse(
            &Signals {
                teletext: vec![(164.0, 374.0)],
                blocks: vec![(164.0, 374.0, 0.9)],
                bank: vec![(164.0, 374.0, 2)],
                vision: vec![(164.0, 374.0)],
            },
            &FuseConfig::default(),
        );
        assert!(
            four[0].confidence > two[0].confidence,
            "{four:?} vs {two:?}"
        );
    }
}

#[cfg(test)]
mod end_to_end_tests {
    use super::*;

    /// Every number here is measured on `scratchpad/gt.ts`: the teletext run is
    /// what `teletext::ad_runs` returns over the real capture, and the black and
    /// silence spans are what `blackdetect`/`silencedetect` report at the
    /// boundaries of the break verified by eye at 164.2 s -> 374.2 s.
    #[test]
    fn the_seven_capture_break_survives_fusion() {
        let teletext = vec![(175.0, 374.0)];
        let blocks: Vec<(f64, f64, f64)> = blocks::candidates(
            &[(163.0, 163.16), (373.04, 373.24)],
            &[(162.6, 164.6), (372.9, 373.82)],
            &blocks::BlockConfig::default(),
        )
        .into_iter()
        .map(|c| (c.start_seconds, c.end_seconds, c.score))
        .collect();
        assert!(!blocks.is_empty(), "the boundary cuts must yield a pod");

        let breaks = fuse(
            &Signals {
                teletext,
                blocks,
                ..Default::default()
            },
            &FuseConfig::default(),
        );
        assert_eq!(breaks.len(), 1, "one break, not two");
        let b = &breaks[0];
        assert_eq!(b.method, "combined");
        // Started late and ended early: a skip must never eat programme.
        assert!(b.start_seconds >= 175.0, "{b:?}");
        assert!(b.end_seconds <= 374.0, "{b:?}");
        assert!(b.confidence > 0.6, "{b:?}");
    }

    /// The same capture with only the teletext meter available marks nothing:
    /// one signal is never enough, whatever its measured precision.
    #[test]
    fn teletext_alone_still_marks_nothing_on_the_real_capture() {
        let breaks = fuse(
            &Signals {
                teletext: vec![(175.0, 374.0), (1254.0, 1364.0)],
                ..Default::default()
            },
            &FuseConfig::default(),
        );
        assert!(breaks.is_empty());
    }
}
