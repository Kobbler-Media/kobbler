//! Cross-episode math shared between V1 (removed) and V2 cache-based
//! skip detect. After the V1 purge this module holds only the pure
//! aggregation helpers that [`super::cache_detect`] calls with data read
//! from the `media_cache` sidecars — no live ffmpeg probes, no DB writes.

use std::collections::HashMap;

use tracing::debug;
use uuid::Uuid;

use super::chromaprint::{Fingerprint, MatchedRegion, find_pairwise_matches};
use super::vhash;
use super::{INTRO_MAX_BER, MIN_MATCH_DENSITY};

/// An episode file to analyze. Both the file_id for DB writes and the path
/// for ffmpeg are carried — callers choose which to use. `episode_id` is the
/// canonical TV-episode UUID when known; the cache_detect resolver prefers
/// `episode_cache/{episode_id}/` over `media_cache/{file_id}/` when set.
/// Native ingest callers that don't carry the canonical id pass `None` and
/// fall through to the file-keyed reader.
#[derive(Debug, Clone)]
pub struct EpisodeFile {
    pub file_id: Uuid,
    pub file_path: String,
    pub duration_seconds: f64,
    pub episode_id: Option<Uuid>,
}

/// Per-episode matched position derived from pairwise chromaprint comparisons.
#[derive(Debug, Clone)]
pub(crate) struct PerEpisodePosition {
    pub(crate) start_seconds: f64,
    pub(crate) end_seconds: f64,
    pub(crate) avg_score: f64,
    /// Pairwise match details (vs_ep index, density, positions) for debug
    /// logging + chapter override.
    pub(crate) pairwise: Vec<PairDetail>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct PairDetail {
    pub(crate) vs_ep: usize,
    pub(crate) density: f64,
    pub(crate) lhs_start: f64,
    pub(crate) lhs_end: f64,
    pub(crate) rhs_start: f64,
    pub(crate) rhs_end: f64,
}

#[cfg(test)]
pub(crate) fn find_per_episode_positions(
    fps: &[(usize, Fingerprint)],
    min_duration: f64,
) -> HashMap<usize, PerEpisodePosition> {
    per_episode_positions(fps, min_duration, false).0
}

/// Pairwise credits region aggregation over a set of chromaprint
/// fingerprints, plus every pairwise region of each episode whatever its
/// sibling count: footage shared with even one sibling is not unique to the
/// episode, which is what a post-credits scene has to be.
pub(crate) fn find_per_episode_positions_and_shared(
    fps: &[(usize, Fingerprint)],
    min_duration: f64,
) -> (HashMap<usize, PerEpisodePosition>, SharedSpans) {
    let (positions, regions) = per_episode_positions(fps, min_duration, false);
    let shared = regions
        .into_iter()
        .map(|(ep, rs)| {
            let spans = rs
                .iter()
                .map(|(mine, partner, theirs)| SharedSpan {
                    start_seconds: mine.start_seconds,
                    end_seconds: mine.end_seconds,
                    partner: *partner,
                    partner_end_seconds: theirs.end_seconds,
                })
                .collect();
            (ep, spans)
        })
        .collect();
    (positions, shared)
}

/// Pairwise intro region aggregation. An episode with only two partners
/// is kept when those two partners also match each other at the same place:
/// a theme shared by just three episodes of a season (IASIP S15's Ireland
/// run). A recap matches its source scene, which is not where the other
/// recap's source sits, so it rarely closes the triangle.
pub(crate) fn find_per_episode_intro_positions(
    fps: &[(usize, Fingerprint)],
    min_duration: f64,
) -> HashMap<usize, PerEpisodePosition> {
    per_episode_positions(fps, min_duration, true).0
}

/// One stretch of an episode that a sibling also carries, with where the
/// sibling's copy ends, both in the fingerprints' own coordinates.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SharedSpan {
    pub(crate) start_seconds: f64,
    pub(crate) end_seconds: f64,
    pub(crate) partner: usize,
    pub(crate) partner_end_seconds: f64,
}

/// Per episode, every span it shares with some sibling.
pub(crate) type SharedSpans = HashMap<usize, Vec<SharedSpan>>;

type EpRegions = HashMap<usize, Vec<(MatchedRegion, usize, MatchedRegion)>>;

fn overlaps(a: &MatchedRegion, b: &MatchedRegion) -> bool {
    a.start_seconds < b.end_seconds && b.start_seconds < a.end_seconds
}

/// Two of `ep`'s regions, against different partners, that overlap in `ep`
/// and whose partners match each other on the same stretch.
fn closed_triangle(
    ep_regions: &EpRegions,
    regions: &[(MatchedRegion, usize, MatchedRegion)],
) -> Option<Vec<(MatchedRegion, usize, MatchedRegion)>> {
    for (i, (mine_a, p, theirs_a)) in regions.iter().enumerate() {
        for (mine_b, q, theirs_b) in &regions[i + 1..] {
            if p == q || !overlaps(mine_a, mine_b) {
                continue;
            }
            let closes = ep_regions.get(p).is_some_and(|rs| {
                rs.iter().any(|(p_side, partner, q_side)| {
                    partner == q && overlaps(p_side, theirs_a) && overlaps(q_side, theirs_b)
                })
            });
            if closes {
                return Some(vec![
                    (mine_a.clone(), *p, theirs_a.clone()),
                    (mine_b.clone(), *q, theirs_b.clone()),
                ]);
            }
        }
    }
    None
}

fn per_episode_positions(
    fps: &[(usize, Fingerprint)],
    min_duration: f64,
    accept_closed_triangles: bool,
) -> (HashMap<usize, PerEpisodePosition>, EpRegions) {
    if fps.len() < 2 {
        return (HashMap::new(), HashMap::new());
    }

    // Require ≥3 siblings to agree on a match when the season has enough
    // episodes to support that (4+ episodes). Short seasons (2-3 episodes)
    // fall back to requiring "everyone else" because that's the best we can
    // do. 2-pair matches on a 4+ ep season are almost always recaps or
    // mid-episode recurring scenes, not intros/credits — Leftovers S2x01's
    // 2-pair chromaprint "credits" at 1656s is the canonical example.
    let min_matches_per_ep = 3.min(fps.len() - 1);

    // Collect all positions per episode from pairwise matches.
    let mut ep_regions: EpRegions = HashMap::new();

    for i in 0..fps.len() {
        for j in (i + 1)..fps.len() {
            let pairwise = find_pairwise_matches(&fps[i].1, &fps[j].1, 1.0, INTRO_MAX_BER);
            for pm in pairwise {
                if pm.lhs.end_seconds - pm.lhs.start_seconds >= min_duration {
                    ep_regions.entry(fps[i].0).or_default().push((
                        pm.lhs.clone(),
                        fps[j].0,
                        pm.rhs.clone(),
                    ));
                }
                if pm.rhs.end_seconds - pm.rhs.start_seconds >= min_duration {
                    ep_regions
                        .entry(fps[j].0)
                        .or_default()
                        .push((pm.rhs, fps[i].0, pm.lhs));
                }
            }
        }
    }

    // Aggregate per episode: cluster overlapping matches, pick best cluster, then median.
    let mut result = HashMap::new();
    for (&ep_idx, regions) in &ep_regions {
        let triangle;
        let regions = if regions.len() >= min_matches_per_ep {
            regions
        } else if accept_closed_triangles
            && min_matches_per_ep == 3
            && let Some(t) = closed_triangle(&ep_regions, regions)
        {
            triangle = t;
            &triangle
        } else {
            continue;
        };

        let pairwise: Vec<PairDetail> = regions
            .iter()
            .map(|(mine, vs_ep, theirs)| PairDetail {
                vs_ep: *vs_ep,
                density: mine.score,
                lhs_start: mine.start_seconds,
                lhs_end: mine.end_seconds,
                rhs_start: theirs.start_seconds,
                rhs_end: theirs.end_seconds,
            })
            .collect();

        // Cluster overlapping regions: sort by start, merge if within 10 s gap.
        let mut sorted_indices: Vec<usize> = (0..regions.len()).collect();
        sorted_indices.sort_by(|&a, &b| {
            regions[a]
                .0
                .start_seconds
                .partial_cmp(&regions[b].0.start_seconds)
                .unwrap()
        });

        let mut clusters: Vec<Vec<usize>> = Vec::new();
        for idx in sorted_indices {
            let fits_last = clusters.last().is_some_and(|cluster| {
                let last_idx = *cluster.last().unwrap();
                regions[idx].0.start_seconds <= regions[last_idx].0.end_seconds + 10.0
            });
            if fits_last {
                clusters.last_mut().unwrap().push(idx);
            } else {
                clusters.push(vec![idx]);
            }
        }

        // Pick the cluster with the longest maximum match duration.
        let best_cluster = clusters
            .into_iter()
            .max_by(|a, b| {
                let max_dur_a = a
                    .iter()
                    .map(|&i| regions[i].0.end_seconds - regions[i].0.start_seconds)
                    .fold(0.0f64, f64::max);
                let max_dur_b = b
                    .iter()
                    .map(|&i| regions[i].0.end_seconds - regions[i].0.start_seconds)
                    .fold(0.0f64, f64::max);
                max_dur_a.partial_cmp(&max_dur_b).unwrap()
            })
            .unwrap();

        let mut starts: Vec<f64> = best_cluster
            .iter()
            .map(|&i| regions[i].0.start_seconds)
            .collect();
        let mut ends: Vec<f64> = best_cluster
            .iter()
            .map(|&i| regions[i].0.end_seconds)
            .collect();
        starts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ends.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let avg_score = best_cluster
            .iter()
            .map(|&i| regions[i].0.score)
            .sum::<f64>()
            / best_cluster.len() as f64;

        result.insert(
            ep_idx,
            PerEpisodePosition {
                start_seconds: starts[starts.len() / 2],
                end_seconds: ends[ends.len() / 2],
                avg_score,
                pairwise,
            },
        );
    }

    (result, ep_regions)
}

/// Override the chromaprint position with a chapter-aligned pairwise match
/// if the cluster-selected position contradicts a chapter marker (delta > 30 s)
/// and a better pairwise match exists near the chapter.
pub(crate) fn chapter_override_position(
    pos: &PerEpisodePosition,
    chapter_start: f64,
) -> Option<(f64, f64)> {
    let delta = (pos.start_seconds - chapter_start).abs();
    if delta <= 30.0 {
        return None;
    }

    let aligned = pos
        .pairwise
        .iter()
        .filter(|p| (p.lhs_start - chapter_start).abs() <= 30.0)
        .max_by(|a, b| {
            let dur_a = a.lhs_end - a.lhs_start;
            let dur_b = b.lhs_end - b.lhs_start;
            dur_a.partial_cmp(&dur_b).unwrap()
        });

    aligned.map(|a| {
        debug!(
            old_start = pos.start_seconds,
            new_start = a.lhs_start,
            chapter_start,
            "Overriding chromaprint position with chapter-aligned pairwise match"
        );
        (a.lhs_start, a.lhs_end)
    })
}

/// A vHash match with pairwise debugging detail.
pub struct VHashMatchWithDetail {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub density: f64,
    /// Mean per-frame similarity across surviving frames in the matched run,
    /// averaged across pairs. See [`vhash::VHashMatch::mean_similarity`] for
    /// why this differs from `density`.
    pub mean_similarity: f64,
    pub pair_count: usize,
    pub pairwise: Vec<VHashPairDetail>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct VHashPairDetail {
    pub vs_ep: usize,
    pub density: f64,
    pub mean_similarity: f64,
    pub lhs_start: f64,
    pub lhs_end: f64,
    pub rhs_start: f64,
    pub rhs_end: f64,
}

/// Maximum gap between consecutive sorted pair-starts that still counts as
/// "same cluster". Pairs whose starts wander further apart than this belong
/// to different on-screen content (different recurring snippets in the cold
/// open) and must not be aggregated together.
const START_CLUSTER_GAP_SECS: f64 = 10.0;

/// A pair shorter than this fraction of its cluster's median pair length is
/// a sliver of shared lead-in, not a partner match for the intro.
const SLIVER_PAIR_FRACTION: f64 = 0.25;

fn matched_secs(m: &vhash::VHashMatch) -> f64 {
    m.density * m.mean_similarity * (m.end_seconds - m.start_seconds)
}

/// Aggregated result of [`pick_winning_start_cluster`].
struct ClusterPick {
    start_seconds: f64,
    end_seconds: f64,
    density: f64,
    mean_similarity: f64,
    pair_count: usize,
}

/// Group `pairs` into clusters (a pair joins the running cluster when its
/// start is within `gap_secs` of the previous start, or when it overlaps the
/// cluster's span), then return the cluster with the most matched seconds,
/// `Σ density × mean_similarity × length`. Picked endpoints are the second-earliest
/// start and `median(end)` over the **winning cluster only** — guaranteeing both come
/// from pairs that agree on the same content, instead of being synthesised
/// from independent statistics across unrelated pairs (which previously
/// stretched a 9s logo flash plus 60s of unrelated cold-open into a
/// 76s "intro"; see harness output for Paradise S02E06).
fn pick_winning_start_cluster(pairs: &[vhash::VHashMatch], gap_secs: f64) -> Option<ClusterPick> {
    if pairs.is_empty() {
        return None;
    }
    let mut sorted: Vec<&vhash::VHashMatch> = pairs.iter().collect();
    sorted.sort_by(|a, b| {
        a.start_seconds
            .partial_cmp(&b.start_seconds)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // Overlap joins too: when the montage is re-cut per episode, a partner
    // whose opening shots differ only starts matching tens of seconds in, yet
    // it covers the same title sequence. Split off by start alone, those
    // shorter (hence denser) pairs can outscore the full-length ones.
    let mut clusters: Vec<(Vec<&vhash::VHashMatch>, f64)> = Vec::new();
    for m in sorted {
        let extend = clusters.last().is_some_and(|(c, span_end)| {
            c.last()
                .is_some_and(|prev| m.start_seconds - prev.start_seconds <= gap_secs)
                || m.start_seconds < *span_end
        });
        if extend {
            let (c, span_end) = clusters.last_mut().unwrap();
            c.push(m);
            *span_end = span_end.max(m.end_seconds);
        } else {
            clusters.push((vec![m], m.end_seconds));
        }
    }
    // Length counts: a network ident or title card is shared by more
    // partners than the titles are (IASIP S15), so a count-like score hands
    // it the intro. `reduce` with strict `>` keeps the earlier cluster on a
    // tie — clusters were appended in start-ascending order above.
    let score = |c: &[&vhash::VHashMatch]| -> f64 { c.iter().map(|m| matched_secs(m)).sum() };
    let winner = clusters
        .into_iter()
        .map(|(c, _)| c)
        .reduce(|best, candidate| {
            if score(&candidate) > score(&best) {
                candidate
            } else {
                best
            }
        })?;
    // A sliver of shared lead-in (3x01 has no intro, yet matches 3x05's black
    // opening for 5.6s) sits inside the cluster's span but is not a partner
    // for the intro, and as the earliest start it would pick the start.
    let mut lengths: Vec<f64> = winner
        .iter()
        .map(|m| m.end_seconds - m.start_seconds)
        .collect();
    lengths.sort_by(|a, b| a.total_cmp(b));
    let floor = lengths[lengths.len() / 2] * SLIVER_PAIR_FRACTION;
    let winner: Vec<&vhash::VHashMatch> = winner
        .into_iter()
        .filter(|m| m.end_seconds - m.start_seconds >= floor)
        .collect();
    let mut starts: Vec<f64> = winner.iter().map(|m| m.start_seconds).collect();
    let mut ends: Vec<f64> = winner.iter().map(|m| m.end_seconds).collect();
    starts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ends.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = winner.len() as f64;
    // Second-earliest start: a single outlier pair whose lhs drifts earlier
    // than the rest can't drag the start before the real intro (Paradise
    // S02E08), but partners that only matched the back half of the titles
    // can't drag it into them either, as a median does (Leftovers S3).
    Some(ClusterPick {
        start_seconds: starts[1.min(starts.len() - 1)],
        end_seconds: ends[ends.len() / 2],
        density: winner.iter().map(|m| m.density).sum::<f64>() / n,
        mean_similarity: winner.iter().map(|m| m.mean_similarity).sum::<f64>() / n,
        pair_count: winner.len(),
    })
}

/// Find per-episode vHash positions by collecting both sides from all pairwise
/// visual matches, clustering outliers away, and cross-validating that each
/// episode's pair-rhs positions land within the partner's match envelope.
pub fn find_per_episode_vhash(
    sequences: &[(usize, Vec<vhash::FrameDescriptor>)],
    min_duration: f64,
) -> HashMap<usize, VHashMatchWithDetail> {
    if sequences.len() < 2 {
        return HashMap::new();
    }

    let mut ep_matches: HashMap<usize, Vec<(vhash::VHashMatch, usize, vhash::VHashMatch)>> =
        HashMap::new();

    let total_pairs = sequences.len() * (sequences.len() - 1) / 2;
    let mut matched_pairs = 0usize;
    for i in 0..sequences.len() {
        for j in (i + 1)..sequences.len() {
            if let Some(pair) = vhash::find_visual_matches(
                &sequences[i].1,
                &sequences[j].1,
                min_duration,
                MIN_MATCH_DENSITY,
            ) {
                matched_pairs += 1;
                ep_matches.entry(sequences[i].0).or_default().push((
                    pair.lhs.clone(),
                    sequences[j].0,
                    pair.rhs.clone(),
                ));
                ep_matches.entry(sequences[j].0).or_default().push((
                    pair.rhs,
                    sequences[i].0,
                    pair.lhs,
                ));
            }
        }
    }
    debug!(
        sequences = sequences.len(),
        total_pairs,
        matched_pairs,
        episodes_with_matches = ep_matches.len(),
        "find_per_episode_vhash pairwise summary"
    );

    let mut result = HashMap::new();
    for (ep_idx, matches) in ep_matches {
        if matches.is_empty() {
            continue;
        }

        let clustered = cluster_matches(&matches);
        let lhs_only: Vec<vhash::VHashMatch> = clustered.iter().map(|m| m.0.clone()).collect();
        let Some(pick) = pick_winning_start_cluster(&lhs_only, START_CLUSTER_GAP_SECS) else {
            continue;
        };

        let pairwise = matches
            .iter()
            .map(|(mine, vs_ep, theirs)| VHashPairDetail {
                vs_ep: *vs_ep,
                density: mine.density,
                mean_similarity: mine.mean_similarity,
                lhs_start: mine.start_seconds,
                lhs_end: mine.end_seconds,
                rhs_start: theirs.start_seconds,
                rhs_end: theirs.end_seconds,
            })
            .collect();

        result.insert(
            ep_idx,
            VHashMatchWithDetail {
                start_seconds: pick.start_seconds,
                end_seconds: pick.end_seconds,
                density: pick.density,
                mean_similarity: pick.mean_similarity,
                pair_count: pick.pair_count,
                pairwise,
            },
        );
    }

    // Cross-validate each episode's pair-rhs positions against the partner's
    // pair envelope. Using the envelope (min lhs_start, max lhs_end across the
    // partner's pairwise detail) avoids cascade tightening versus using the
    // median aggregate.
    let partner_envelope: HashMap<usize, (f64, f64)> = result
        .iter()
        .map(|(&ep_idx, detail)| {
            let min_start = detail
                .pairwise
                .iter()
                .map(|p| p.lhs_start)
                .fold(f64::INFINITY, f64::min);
            let max_end = detail
                .pairwise
                .iter()
                .map(|p| p.lhs_end)
                .fold(f64::NEG_INFINITY, f64::max);
            (ep_idx, (min_start, max_end))
        })
        .collect();
    let to_remove: Vec<usize> = result
        .iter()
        .filter_map(|(&ep_idx, detail)| {
            let valid_pairs = detail
                .pairwise
                .iter()
                .filter(|pair| {
                    if let Some(&(p_start, p_end)) = partner_envelope.get(&pair.vs_ep) {
                        pair.rhs_start < p_end && pair.rhs_end > p_start
                    } else {
                        true
                    }
                })
                .count();
            if valid_pairs * 2 < detail.pair_count {
                Some(ep_idx)
            } else {
                None
            }
        })
        .collect();

    for ep_idx in to_remove {
        result.remove(&ep_idx);
    }

    result
}

/// Cluster pairwise matches by midpoint proximity.
/// Returns only the matches in the heaviest cluster. Prevents distant false
/// matches from contaminating the aggregation of the real intro region.
fn cluster_matches(
    matches: &[(vhash::VHashMatch, usize, vhash::VHashMatch)],
) -> Vec<&(vhash::VHashMatch, usize, vhash::VHashMatch)> {
    if matches.len() <= 1 {
        return matches.iter().collect();
    }

    let mut indexed: Vec<(usize, f64)> = matches
        .iter()
        .enumerate()
        .map(|(i, m)| (i, (m.0.start_seconds + m.0.end_seconds) / 2.0))
        .collect();
    indexed.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let max_gap = 120.0;
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    let mut current_cluster = vec![indexed[0].0];

    for i in 1..indexed.len() {
        if indexed[i].1 - indexed[i - 1].1 <= max_gap {
            current_cluster.push(indexed[i].0);
        } else {
            clusters.push(current_cluster);
            current_cluster = vec![indexed[i].0];
        }
    }
    clusters.push(current_cluster);

    // By matched seconds, not pair count: a recurring act-break eyecatch is
    // shared by more partners than the opening (Death Note) but is a tenth
    // of its length. `max_by` keeps the last maximum; ties are not expected.
    let weight = |c: &[usize]| -> f64 { c.iter().map(|&i| matched_secs(&matches[i].0)).sum() };
    let best = clusters
        .into_iter()
        .max_by(|a, b| weight(a).total_cmp(&weight(b)))
        .unwrap();
    best.iter().map(|&i| &matches[i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skip_detect::MIN_MATCH_DURATION;
    use crate::skip_detect::chromaprint::Fingerprint;

    fn m(start: f64, end: f64, density: f64, mean_similarity: f64) -> vhash::VHashMatch {
        vhash::VHashMatch {
            start_seconds: start,
            end_seconds: end,
            density,
            mean_similarity,
        }
    }

    #[test]
    fn pick_cluster_paradise_s02e06_picks_strong_pair_cluster_not_envelope() {
        // Reproduces the harness output for Paradise S02E06's intro pass.
        // Every per-pair lhs span is 7-16s (consistent with a logo flash),
        // but the pre-fix aggregator stitched min(start)=280.6 (from one
        // pair) onto median(end)=356.6 (from a different pair) and produced
        // a 76s "intro". The fix scores by Σ density × mean_sim per
        // start-cluster and picks the strongest cluster's own endpoints.
        let pairs = vec![
            m(280.9, 289.6, 1.00, 0.95), // ep0
            m(451.9, 467.6, 0.78, 0.80), // ep1
            m(450.9, 457.6, 0.90, 0.75), // ep3
            m(300.3, 307.3, 0.90, 0.71), // ep4
            m(348.3, 356.6, 1.00, 0.74), // ep6
            m(280.6, 289.6, 1.00, 0.99), // ep7
        ];
        let pick = pick_winning_start_cluster(&pairs, 10.0).expect("expected a winning cluster");
        // Second-earliest start of the 2-pair winner [{280.6, 280.9}] →
        // 280.9. Span 280.9..289.6 = 8.7s falls below the 10s vhash-only
        // intro gate at the call site → S02E06 ends up with no intro picked.
        assert!(
            (pick.start_seconds - 280.9).abs() < 0.01,
            "start should come from the strongest pair cluster, got {}",
            pick.start_seconds
        );
        assert!(
            (pick.end_seconds - 289.6).abs() < 0.01,
            "end must come from the same pairs as the start, got {}",
            pick.end_seconds
        );
        assert_eq!(pick.pair_count, 2, "winning cluster has 2 pairs");
    }

    #[test]
    fn pick_cluster_paradise_s02e08_outlier_lhs_doesnt_drag_start_early() {
        // S02E08 had one pair (ep2) whose lhs_start=569.2 was 6.4s earlier
        // than the cluster majority (575.6..575.9) — still inside the 10s
        // gap so it landed in the same start-cluster, but min(start) handed
        // back 569.2 and the extracted clip began ~6s before the actual
        // intro. The second-earliest start pulls back to 575.6.
        let pairs = vec![
            m(575.6, 585.3, 1.00, 0.97), // ep0
            m(575.6, 583.6, 1.00, 1.00), // ep1
            m(569.2, 584.9, 1.00, 0.89), // ep2 — outlier lhs_start
            m(604.9, 611.2, 1.00, 0.90), // ep3 (separate sub-cluster)
            m(575.9, 584.9, 1.00, 0.99), // ep5
            m(575.6, 584.6, 0.96, 0.98), // ep6
        ];
        let pick = pick_winning_start_cluster(&pairs, 10.0).unwrap();
        assert!(
            (pick.start_seconds - 575.6).abs() < 0.01,
            "picked start must reject the 569.2 outlier; got {}",
            pick.start_seconds
        );
        assert_eq!(
            pick.pair_count, 5,
            "winner is the 5-pair cluster around 575s, not the lone 604.9"
        );
    }

    #[test]
    fn pick_cluster_paradise_s02e05_marginal_repeat_collapses_below_gate() {
        // S02E05 had three pairs with mean_similarity ~0.74 (marginal
        // repeat content per VHashMatch::mean_similarity comment) drifting
        // 16s apart at the 880-900s mark. Pre-fix min(start)/median(end)
        // emitted a 16s "intro" candidate that wasn't an intro at all.
        // The second-earliest start collapses it to ~7s — below the 10s
        // vhash-only gate.
        let pairs = vec![
            m(883.6, 890.9, 1.00, 0.77), // ep2
            m(892.3, 899.3, 0.90, 0.71), // ep5
            m(899.6, 907.2, 1.00, 0.74), // ep7
        ];
        let pick = pick_winning_start_cluster(&pairs, 10.0).unwrap();
        let span = pick.end_seconds - pick.start_seconds;
        assert!(
            span < 10.0,
            "marginal repeat should collapse below the 10s gate; got {span:.1}s"
        );
    }

    #[test]
    fn pick_cluster_single_pair() {
        let pick = pick_winning_start_cluster(&[m(100.0, 110.0, 1.0, 0.9)], 10.0).unwrap();
        assert!((pick.start_seconds - 100.0).abs() < 0.01);
        assert!((pick.end_seconds - 110.0).abs() < 0.01);
        assert_eq!(pick.pair_count, 1);
    }

    #[test]
    fn pick_cluster_empty_returns_none() {
        assert!(pick_winning_start_cluster(&[], 10.0).is_none());
    }

    #[test]
    fn pick_cluster_tied_score_breaks_to_earliest() {
        // Two single-pair clusters with identical density × mean_sim. The
        // earlier one wins because `max_by` is stable and we sort by start
        // ascending before clustering — preferring earlier intro candidates
        // when pair strength is tied.
        let pairs = vec![m(50.0, 60.0, 1.0, 0.9), m(500.0, 510.0, 1.0, 0.9)];
        let pick = pick_winning_start_cluster(&pairs, 10.0).unwrap();
        assert!((pick.start_seconds - 50.0).abs() < 0.01);
    }

    #[test]
    fn pick_cluster_leftovers_s3e04_partial_pairs_dont_split_the_intro() {
        // The Leftovers 3x04 intro pairs, as matched against the rest of S3.
        // The montage is re-cut per episode, so partners whose opening shots
        // differ (5, 6, 8) only start matching ~30s in, while 2, 3 and 7
        // match from the titles' first frame. All six describe the same
        // 0..94s title sequence. Clustered by start alone, the three late
        // pairs (denser because they are shorter) outscored the three full
        // ones and the intro started at 30.91, dropping 28s of titles.
        let pairs = vec![
            m(6.92, 89.59, 0.90, 0.988),  // vs 2
            m(1.92, 86.25, 0.91, 0.999),  // vs 3
            m(30.91, 94.26, 0.99, 0.990), // vs 5
            m(30.91, 90.59, 1.00, 0.993), // vs 6
            m(0.00, 94.26, 0.90, 0.991),  // vs 7
            m(30.91, 94.26, 1.00, 0.984), // vs 8
        ];
        let pick = pick_winning_start_cluster(&pairs, START_CLUSTER_GAP_SECS).unwrap();
        assert_eq!(pick.pair_count, 6, "all six pairs are one intro");
        assert!(
            pick.start_seconds <= 2.5,
            "titles begin at ~2.5s; got start {}",
            pick.start_seconds
        );
        assert!(
            (pick.end_seconds - 94.26).abs() < 0.01,
            "end should stay at 94.26; got {}",
            pick.end_seconds
        );
    }

    #[test]
    fn pick_cluster_iasip_s15e04_full_titles_beat_the_network_ident() {
        // It's Always Sunny S15E04's intro pairs. Four partners share the
        // ~11s FX ident + "11:00 AM" time card at the top of the file; three
        // share the 20s title sequence at 117s, which the theme's chroma match
        // confirms. Scored without length, four short pairs outweighed three
        // full ones and the ident became a second "intro".
        let pairs = vec![
            m(2.02, 13.57, 1.00, 0.99),    // vs 6
            m(2.02, 13.57, 1.00, 0.99),    // vs 7
            m(5.94, 13.24, 0.95, 0.88),    // vs 4
            m(5.94, 13.24, 0.95, 0.89),    // vs 5
            m(117.59, 137.57, 0.93, 0.95), // vs 0
            m(117.59, 137.57, 1.00, 0.97), // vs 1
            m(117.59, 137.57, 0.95, 0.93), // vs 2
        ];
        let pick = pick_winning_start_cluster(&pairs, START_CLUSTER_GAP_SECS).unwrap();
        assert!(
            (pick.start_seconds - 117.59).abs() < 0.01,
            "the title sequence wins; got {}..{}",
            pick.start_seconds,
            pick.end_seconds
        );
        assert_eq!(pick.pair_count, 3);
    }

    #[test]
    fn pick_cluster_leftovers_s3e05_start_ignores_a_sliver_pair() {
        // The Leftovers 3x05 pairs. 3x01 has no intro; its only "match" is
        // 5.6s of the black lead-in, yet as the earliest start it decided the
        // second-earliest pick (3.25, inside the black). The black runs to
        // 7.51s and the titles open there, which the full-length pairs say.
        let pairs = vec![
            m(0.00, 5.59, 1.00, 1.00),   // vs 0 (3x01, no intro)
            m(3.25, 99.93, 0.93, 0.99),  // vs 7
            m(7.59, 101.60, 0.87, 0.99), // vs 5
            m(8.93, 99.93, 0.90, 1.00),  // vs 6
            m(12.93, 95.60, 0.83, 0.98), // vs 1
            m(12.93, 98.93, 0.87, 0.99), // vs 2
            m(34.91, 99.60, 0.99, 0.99), // vs 3
        ];
        let pick = pick_winning_start_cluster(&pairs, START_CLUSTER_GAP_SECS).unwrap();
        assert!(
            (pick.start_seconds - 7.59).abs() < 0.01,
            "start must come from intro-length pairs; got {}",
            pick.start_seconds
        );
        assert!((pick.end_seconds - 99.60).abs() < 0.5);
        assert_eq!(pick.pair_count, 6, "the sliver is not a partner");
    }

    #[test]
    fn cluster_matches_death_note_bumper_does_not_outvote_the_opening() {
        // Death Note 1x21: 16 partners share the 80s opening, 18 share the
        // ~10s "How to use it" eyecatch at the act break. Picking the
        // midpoint cluster by pair count handed the eyecatch to the intro
        // pass, so its density landed on the chroma-matched opening.
        let mut matches = Vec::new();
        for _ in 0..16 {
            let o = m(0.0, 80.91, 0.97, 0.97);
            matches.push((o.clone(), 0, o));
        }
        for _ in 0..18 {
            let b = m(780.24, 789.91, 1.00, 0.97);
            matches.push((b.clone(), 0, b));
        }
        let kept = cluster_matches(&matches);
        assert_eq!(kept.len(), 16);
        assert!(kept.iter().all(|k| k.0.start_seconds == 0.0));
    }

    /// Real vHash descriptors for The Leftovers S3E02–E08 title sequences
    /// (0–110s; E06 440–570s, where its intro follows a cold open).
    fn leftovers_s3_title_fixtures() -> Vec<(usize, Vec<vhash::FrameDescriptor>)> {
        let raw: [(usize, &[u8]); 7] = [
            (
                2,
                include_bytes!("../../tests/fixtures/vhash/leftovers_s3e02_titles.bin"),
            ),
            (
                3,
                include_bytes!("../../tests/fixtures/vhash/leftovers_s3e03_titles.bin"),
            ),
            (
                4,
                include_bytes!("../../tests/fixtures/vhash/leftovers_s3e04_titles.bin"),
            ),
            (
                5,
                include_bytes!("../../tests/fixtures/vhash/leftovers_s3e05_titles.bin"),
            ),
            (
                6,
                include_bytes!("../../tests/fixtures/vhash/leftovers_s3e06_titles.bin"),
            ),
            (
                7,
                include_bytes!("../../tests/fixtures/vhash/leftovers_s3e07_titles.bin"),
            ),
            (
                8,
                include_bytes!("../../tests/fixtures/vhash/leftovers_s3e08_titles.bin"),
            ),
        ];
        raw.iter()
            .map(|(ep, b)| {
                (
                    *ep,
                    vhash::deserialize_descriptors(b).expect("decode fixture"),
                )
            })
            .collect()
    }

    #[test]
    fn leftovers_s3_intro_starts_at_the_titles() {
        // Regression for the 2026-11-03 re-run: 3x04's intro moved from 0.00
        // to 30.91, and every other episode's start drifted 2–9s into the
        // titles. Frame 0 is black and the titles open at ~2s in each of
        // these episodes (3x04 at ~2.5s, "Starring Justin Theroux" ~7.5s).
        let res = find_per_episode_vhash(&leftovers_s3_title_fixtures(), MIN_MATCH_DURATION);

        let e04 = res.get(&4).expect("3x04 intro");
        assert!(
            e04.start_seconds <= 2.5,
            "3x04 start {} drops the titles",
            e04.start_seconds
        );
        assert!(
            e04.end_seconds >= 91.5,
            "3x04 end {} cuts the 'Directed by' card",
            e04.end_seconds
        );
        assert!(e04.pair_count >= 5, "3x04 pair_count {}", e04.pair_count);

        for ep in [2, 3, 7, 8] {
            let m = res.get(&ep).unwrap_or_else(|| panic!("3x0{ep} intro"));
            assert!(
                m.start_seconds <= 2.5,
                "3x0{ep} start {} drops the opening shot",
                m.start_seconds
            );
        }
        let e06 = res.get(&6).expect("3x06 intro");
        assert!(
            (460.0..=464.0).contains(&e06.start_seconds),
            "3x06 start {}",
            e06.start_seconds
        );
    }

    #[test]
    fn per_episode_from_identical_fingerprints() {
        let fp = Fingerprint {
            data: vec![100; 20],
            duration_seconds: 20.0,
        };
        let fps = vec![(0, fp.clone()), (1, fp)];
        let result = find_per_episode_positions(&fps, 3.0);
        assert!(result.contains_key(&0), "Ep 0 should have position");
        assert!(result.contains_key(&1), "Ep 1 should have position");
        assert!(result[&0].avg_score > 0.9);
    }

    #[test]
    fn per_episode_needs_at_least_two() {
        let fp = Fingerprint {
            data: vec![100; 20],
            duration_seconds: 20.0,
        };
        assert!(find_per_episode_positions(&[(0, fp)], 3.0).is_empty());
        assert!(find_per_episode_positions(&[], 3.0).is_empty());
    }

    #[test]
    fn per_episode_rejects_short_matches() {
        let fp = Fingerprint {
            data: vec![100; 5],
            duration_seconds: 5.0,
        };
        let fps = vec![(0, fp.clone()), (1, fp)];
        let result = find_per_episode_positions(&fps, 15.0);
        assert!(
            result.is_empty(),
            "Should reject matches shorter than min_duration"
        );
    }

    #[test]
    fn per_episode_no_match_for_different_fingerprints() {
        let fp_a = Fingerprint {
            data: vec![0x00000000; 20],
            duration_seconds: 20.0,
        };
        let fp_b = Fingerprint {
            data: vec![0xFFFFFFFF; 20],
            duration_seconds: 20.0,
        };
        let fps = vec![(0, fp_a), (1, fp_b)];
        let result = find_per_episode_positions(&fps, 3.0);
        assert!(
            result.is_empty(),
            "Completely different fingerprints should not match"
        );
    }

    #[test]
    fn per_episode_noise_filtered_in_large_season() {
        let matching_fp = Fingerprint {
            data: vec![100; 20],
            duration_seconds: 20.0,
        };
        let fps: Vec<(usize, Fingerprint)> = vec![
            (0, matching_fp.clone()),
            (1, matching_fp),
            (
                2,
                Fingerprint {
                    data: vec![0xAAAAAAAA; 20],
                    duration_seconds: 20.0,
                },
            ),
            (
                3,
                Fingerprint {
                    data: vec![0xBBBBBBBB; 20],
                    duration_seconds: 20.0,
                },
            ),
            (
                4,
                Fingerprint {
                    data: vec![0xCCCCCCCC; 20],
                    duration_seconds: 20.0,
                },
            ),
            (
                5,
                Fingerprint {
                    data: vec![0xDDDDDDDD; 20],
                    duration_seconds: 20.0,
                },
            ),
        ];
        let result = find_per_episode_positions(&fps, 3.0);
        assert!(
            result.is_empty(),
            "Should reject episodes with only 1 matching pair in large season"
        );
    }

    #[test]
    fn per_episode_accepts_strong_agreement() {
        let fp = Fingerprint {
            data: vec![100; 20],
            duration_seconds: 20.0,
        };
        let fps = vec![(0, fp.clone()), (1, fp.clone()), (2, fp.clone()), (3, fp)];
        let result = find_per_episode_positions(&fps, 3.0);
        assert_eq!(
            result.len(),
            4,
            "All 4 episodes should have positions when they all agree"
        );
    }

    #[test]
    fn per_episode_variable_offsets() {
        let theme: Vec<u32> = vec![
            0xA5A5A5A5, 0x5A5A5A5A, 0x12345678, 0x87654321, 0xDEADBEEF, 0xCAFEBABE, 0xF00DBABE,
            0xBAADF00D, 0x1337C0DE, 0xDECAFBAD, 0xFACEFEED, 0x0BADC0DE, 0xC0FFEE00, 0xBEEFCAFE,
            0xFEEDFACE, 0xBABECAFE,
        ];

        let mut data0 = theme.clone();
        data0.extend(vec![0x11111111; 8]);

        let mut data1 = vec![0x22222222; 5];
        data1.extend(theme.clone());
        data1.extend(vec![0x33333333; 3]);

        let mut data2 = vec![0x44444444; 10];
        data2.extend(theme.clone());

        let fp0 = Fingerprint {
            data: data0,
            duration_seconds: 24.0,
        };
        let fp1 = Fingerprint {
            data: data1,
            duration_seconds: 24.0,
        };
        let fp2 = Fingerprint {
            data: data2,
            duration_seconds: 26.0,
        };

        let fps = vec![(0, fp0), (1, fp1), (2, fp2)];
        let result = find_per_episode_positions(&fps, 3.0);

        assert!(result.contains_key(&0), "Ep 0 should have position");
        assert!(result.contains_key(&1), "Ep 1 should have position");
        assert!(result.contains_key(&2), "Ep 2 should have position");

        assert!(
            result[&0].start_seconds < 2.0,
            "Ep0 start={} should be near 0",
            result[&0].start_seconds
        );
        assert!(
            result[&1].start_seconds > 2.0,
            "Ep1 start={} should be offset",
            result[&1].start_seconds
        );
        assert!(
            result[&2].start_seconds > result[&1].start_seconds,
            "Ep2 start={} should be after Ep1 start={}",
            result[&2].start_seconds,
            result[&1].start_seconds
        );
    }

    #[test]
    fn short_matches_dont_outvote_long_intro() {
        let ident: Vec<u32> = vec![0xAAAAAAAA; 6];
        let intro: Vec<u32> = vec![
            0xA5A5A5A5, 0x5A5A5A5A, 0x12345678, 0x87654321, 0xDEADBEEF, 0xCAFEBABE, 0xF00DBABE,
            0xBAADF00D, 0x1337C0DE, 0xDECAFBAD, 0xFACEFEED, 0x0BADC0DE, 0xC0FFEE00, 0xBEEFCAFE,
            0xFEEDFACE, 0xBABECAFE,
        ];

        let mut data0 = ident.clone();
        data0.extend(vec![0x11111111; 24]);
        data0.extend(intro.clone());
        data0.extend(vec![0x22222222; 10]);

        let mut data1 = ident.clone();
        data1.extend(vec![0x33333333; 50]);

        let mut data2 = ident.clone();
        data2.extend(vec![0x44444444; 50]);

        let mut data3 = ident.clone();
        data3.extend(vec![0x55555555; 24]);
        data3.extend(intro.clone());
        data3.extend(vec![0x66666666; 10]);

        let fps: Vec<(usize, Fingerprint)> = vec![
            (
                0,
                Fingerprint {
                    data: data0,
                    duration_seconds: 56.0,
                },
            ),
            (
                1,
                Fingerprint {
                    data: data1,
                    duration_seconds: 56.0,
                },
            ),
            (
                2,
                Fingerprint {
                    data: data2,
                    duration_seconds: 56.0,
                },
            ),
            (
                3,
                Fingerprint {
                    data: data3,
                    duration_seconds: 56.0,
                },
            ),
        ];

        let result = find_per_episode_positions(&fps, 5.0);

        if let Some(pos) = result.get(&0) {
            assert!(
                pos.start_seconds > 20.0,
                "Ep0: start_seconds={} should be near intro offset (~30), not near 0",
                pos.start_seconds
            );
        }
        if let Some(pos) = result.get(&3) {
            assert!(
                pos.start_seconds > 20.0,
                "Ep3: start_seconds={} should be near intro offset (~30), not near 0",
                pos.start_seconds
            );
        }
    }

    #[test]
    fn chapter_override_selects_aligned_match() {
        let pos = PerEpisodePosition {
            start_seconds: 0.0,
            end_seconds: 40.0,
            avg_score: 0.85,
            pairwise: vec![
                PairDetail {
                    vs_ep: 1,
                    density: 0.9,
                    lhs_start: 0.0,
                    lhs_end: 40.0,
                    rhs_start: 0.0,
                    rhs_end: 40.0,
                },
                PairDetail {
                    vs_ep: 2,
                    density: 0.9,
                    lhs_start: 0.0,
                    lhs_end: 38.0,
                    rhs_start: 0.0,
                    rhs_end: 38.0,
                },
                PairDetail {
                    vs_ep: 3,
                    density: 0.85,
                    lhs_start: 310.0,
                    lhs_end: 395.0,
                    rhs_start: 300.0,
                    rhs_end: 385.0,
                },
            ],
        };

        let result = chapter_override_position(&pos, 300.0);
        assert!(result.is_some(), "Should override when chapter contradicts");
        let (start, end) = result.unwrap();
        assert!(
            (start - 310.0).abs() < 0.01,
            "Start should be 310.0, got {start}"
        );
        assert!((end - 395.0).abs() < 0.01, "End should be 395.0, got {end}");
    }

    #[test]
    fn chapter_override_noop_when_close() {
        let pos = PerEpisodePosition {
            start_seconds: 305.0,
            end_seconds: 390.0,
            avg_score: 0.9,
            pairwise: vec![PairDetail {
                vs_ep: 1,
                density: 0.9,
                lhs_start: 305.0,
                lhs_end: 390.0,
                rhs_start: 300.0,
                rhs_end: 385.0,
            }],
        };

        let result = chapter_override_position(&pos, 300.0);
        assert!(
            result.is_none(),
            "Should not override when position is within 30s of chapter"
        );
    }

    #[test]
    fn chapter_override_noop_when_no_aligned_match() {
        let pos = PerEpisodePosition {
            start_seconds: 0.0,
            end_seconds: 40.0,
            avg_score: 0.85,
            pairwise: vec![
                PairDetail {
                    vs_ep: 1,
                    density: 0.9,
                    lhs_start: 0.0,
                    lhs_end: 40.0,
                    rhs_start: 0.0,
                    rhs_end: 40.0,
                },
                PairDetail {
                    vs_ep: 2,
                    density: 0.9,
                    lhs_start: 0.0,
                    lhs_end: 38.0,
                    rhs_start: 0.0,
                    rhs_end: 38.0,
                },
            ],
        };

        let result = chapter_override_position(&pos, 300.0);
        assert!(
            result.is_none(),
            "Should not override when no pairwise match aligns with chapter"
        );
    }

    fn iasip_s15_intro_fingerprints() -> Vec<(usize, Fingerprint)> {
        let raw: [&[u8]; 8] = [
            include_bytes!("../../tests/fixtures/chromaprint/iasip_s15e01_intro.bin"),
            include_bytes!("../../tests/fixtures/chromaprint/iasip_s15e02_intro.bin"),
            include_bytes!("../../tests/fixtures/chromaprint/iasip_s15e03_intro.bin"),
            include_bytes!("../../tests/fixtures/chromaprint/iasip_s15e04_intro.bin"),
            include_bytes!("../../tests/fixtures/chromaprint/iasip_s15e05_intro.bin"),
            include_bytes!("../../tests/fixtures/chromaprint/iasip_s15e06_intro.bin"),
            include_bytes!("../../tests/fixtures/chromaprint/iasip_s15e07_intro.bin"),
            include_bytes!("../../tests/fixtures/chromaprint/iasip_s15e08_intro.bin"),
        ];
        raw.iter()
            .enumerate()
            .map(|(idx, bytes)| {
                let data: Vec<u32> = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| u32::from_le_bytes(*c))
                    .collect();
                // The fixtures are the first 160 s at the file's own ~8.06 samples/s.
                let duration_seconds = data.len() as f64 / 8.06;
                (
                    idx,
                    Fingerprint {
                        data,
                        duration_seconds,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn iasip_s15_ireland_titles_agree_across_three_episodes() {
        // 5x05–5x07 (the Ireland episodes) open on a variant theme that only
        // they share, at ~84–135 s. Each has two partners, which on its own is
        // the recap shape the 3-sibling rule exists for; the three agreeing
        // with one another is not.
        let res =
            find_per_episode_intro_positions(&iasip_s15_intro_fingerprints(), MIN_MATCH_DURATION);
        let e05 = res.get(&4).expect("5x05 titles");
        assert!(
            (115.0..=120.0).contains(&e05.start_seconds),
            "5x05 start {}",
            e05.start_seconds
        );
        let e07 = res.get(&6).expect("5x07 titles");
        assert!(
            (106.0..=110.0).contains(&e07.start_seconds) && e07.end_seconds >= 124.0,
            "5x07 {}..{}",
            e07.start_seconds,
            e07.end_seconds
        );
        // The standard titles in 5x01–5x04 are unchanged.
        let e01 = res.get(&0).expect("5x01 titles");
        assert!((114.0..=118.0).contains(&e01.start_seconds));
    }

    #[test]
    fn credits_keep_the_three_sibling_rule() {
        // The same two-partner shape on the credits side stays dropped
        // (Leftovers 2x01's mid-episode "credits").
        let res = find_per_episode_positions(&iasip_s15_intro_fingerprints(), MIN_MATCH_DURATION);
        assert!(!res.contains_key(&4), "5x05 should need three siblings");
    }
}
