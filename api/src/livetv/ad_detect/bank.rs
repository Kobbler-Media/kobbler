//! Chromaprint matching against a rolling bank of known ads.
//!
//! The bank replaces skip-detect's season-sibling comparison: the same 30 s ad
//! airs dozens of times a week and is bit-identical every time, so the question
//! is "have I seen this exact audio before?" rather than "does this look like an
//! ad?".
//!
//! The alignment scan is local rather than `fingerprint_match::find_matching_regions`
//! because that one caps the scan at `min(len_a, len_b) - |shift|`, which is zero
//! for a 12 s ad sitting an hour into a recording — it was written for whole
//! episode against whole episode.

use std::collections::HashMap;

use uuid::Uuid;

use crate::skip_detect::primitives::fingerprint_match::{
    Fingerprint, find_longest_contiguous_run_pair,
};

const DEFAULT_SAMPLES_TO_SECONDS: f64 = 0.1238;
const MAX_TIME_SKIP: f64 = 3.0;
const INDEX_SHIFT_TOLERANCE: i32 = 2;
/// A point that recurs more often than this is ambience, not a landmark.
const MAX_POSTINGS_PER_POINT: usize = 8;
const MAX_CANDIDATE_SHIFTS: usize = 64;

#[derive(Debug, Clone)]
pub struct BankEntry {
    pub id: Uuid,
    pub fingerprint: Vec<u32>,
    pub duration_secs: f64,
    pub origin: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BankHit {
    pub entry_id: Uuid,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub score: f64,
}

#[derive(Debug, Clone)]
pub struct BankConfig {
    pub min_region_secs: f64,
    pub max_ber: f64,
}

impl Default for BankConfig {
    fn default() -> Self {
        Self {
            min_region_secs: 10.0,
            // 0.1875 * 32 = 6 bits, the popcount threshold validated on the
            // Seven capture (12.13 s true match against 0.00 s on programme).
            max_ber: 0.1875,
        }
    }
}

fn samples_to_seconds(fp: &Fingerprint) -> f64 {
    if fp.data.is_empty() || fp.duration_seconds <= 0.0 {
        DEFAULT_SAMPLES_TO_SECONDS
    } else {
        fp.duration_seconds / fp.data.len() as f64
    }
}

fn index_of(points: &[u32]) -> HashMap<u32, Vec<usize>> {
    let mut index: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, point) in points.iter().enumerate() {
        let postings = index.entry(*point).or_default();
        if postings.len() < MAX_POSTINGS_PER_POINT {
            postings.push(i);
        }
    }
    index
}

fn candidate_shifts(index: &HashMap<u32, Vec<usize>>, ad: &[u32]) -> Vec<i32> {
    let mut votes: HashMap<i32, u32> = HashMap::new();
    for (a, point) in ad.iter().enumerate() {
        for delta in -INDEX_SHIFT_TOLERANCE..=INDEX_SHIFT_TOLERANCE {
            let Some(postings) = index.get(&point.wrapping_add(delta as u32)) else {
                continue;
            };
            for r in postings {
                *votes.entry(a as i32 - *r as i32).or_insert(0) += 1;
            }
        }
    }
    let mut shifts: Vec<(i32, u32)> = votes.into_iter().collect();
    shifts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    shifts.truncate(MAX_CANDIDATE_SHIFTS);
    shifts.into_iter().map(|(s, _)| s).collect()
}

/// `shift` aligns recording index `r` with ad index `r + shift`.
fn scan_at_shift(
    recording: &[u32],
    ad: &[u32],
    shift: i32,
    max_bit_diff: u32,
    spp: f64,
) -> Option<(f64, f64, f64)> {
    let lo = (-shift).max(0) as usize;
    let hi = recording
        .len()
        .min((ad.len() as i64 - shift as i64).max(0) as usize);
    if hi <= lo {
        return None;
    }
    let mut pairs: Vec<(f64, f64)> = Vec::new();
    for (r, point) in recording.iter().enumerate().take(hi).skip(lo) {
        let a = (r as i64 + shift as i64) as usize;
        if (point ^ ad[a]).count_ones() <= max_bit_diff {
            pairs.push((r as f64 * spp, a as f64 * spp));
        }
    }
    if pairs.is_empty() {
        return None;
    }
    let (lhs, _) = find_longest_contiguous_run_pair(&pairs, MAX_TIME_SKIP, spp)?;
    Some((lhs.start_seconds, lhs.end_seconds, lhs.score))
}

pub fn match_bank(recording: &Fingerprint, bank: &[BankEntry], cfg: &BankConfig) -> Vec<BankHit> {
    if recording.data.is_empty() || bank.is_empty() {
        return Vec::new();
    }
    let spp = samples_to_seconds(recording);
    let max_bit_diff = (cfg.max_ber * 32.0) as u32;
    let index = index_of(&recording.data);

    let mut hits: Vec<BankHit> = Vec::new();
    for entry in bank {
        if entry.fingerprint.is_empty() {
            continue;
        }
        let mut best: Option<BankHit> = None;
        for shift in candidate_shifts(&index, &entry.fingerprint) {
            let Some((start, end, score)) = scan_at_shift(
                &recording.data,
                &entry.fingerprint,
                shift,
                max_bit_diff,
                spp,
            ) else {
                continue;
            };
            if end - start < cfg.min_region_secs {
                continue;
            }
            if best.is_none_or(|b| score > b.score) {
                best = Some(BankHit {
                    entry_id: entry.id,
                    start_seconds: start,
                    end_seconds: end,
                    score,
                });
            }
        }
        if let Some(hit) = best {
            hits.push(hit);
        }
    }

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut kept: Vec<BankHit> = Vec::new();
    for hit in hits {
        let overlaps = kept
            .iter()
            .any(|k| hit.start_seconds < k.end_seconds && k.start_seconds < hit.end_seconds);
        if !overlaps {
            kept.push(hit);
        }
    }
    kept.sort_by(|a, b| {
        a.start_seconds
            .partial_cmp(&b.start_seconds)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    kept
}

/// Known ads that sit back to back are one pod, not several breaks. The count is
/// how many bank entries the span covers, which becomes `ads_known`.
pub fn merge_hits_into_breaks(hits: &[BankHit], max_gap_secs: f64) -> Vec<(f64, f64, usize)> {
    let mut sorted: Vec<BankHit> = hits.to_vec();
    sorted.sort_by(|a, b| {
        a.start_seconds
            .partial_cmp(&b.start_seconds)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut out: Vec<(f64, f64, usize)> = Vec::new();
    for hit in sorted {
        match out.last_mut() {
            Some(last) if hit.start_seconds - last.1 <= max_gap_secs => {
                last.1 = last.1.max(hit.end_seconds);
                last.2 += 1;
            }
            _ => out.push((hit.start_seconds, hit.end_seconds, 1)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(seed: u64, n: usize) -> Vec<u32> {
        let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (0..n)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (state >> 32) as u32
            })
            .collect()
    }

    fn fingerprint(data: Vec<u32>) -> Fingerprint {
        let duration_seconds = data.len() as f64 * DEFAULT_SAMPLES_TO_SECONDS;
        Fingerprint {
            data,
            duration_seconds,
        }
    }

    fn entry(id_byte: u8, data: Vec<u32>) -> BankEntry {
        let duration_secs = data.len() as f64 * DEFAULT_SAMPLES_TO_SECONDS;
        BankEntry {
            id: Uuid::from_bytes([id_byte; 16]),
            fingerprint: data,
            duration_secs,
            origin: "confirmed".to_string(),
        }
    }

    #[test]
    fn an_implanted_known_ad_is_found_at_its_offset() {
        let ad = noise(7, 120);
        let mut recording = noise(1, 2000);
        recording[500..620].copy_from_slice(&ad);

        let hits = match_bank(
            &fingerprint(recording),
            &[entry(1, ad)],
            &BankConfig::default(),
        );
        assert_eq!(hits.len(), 1, "{hits:?}");
        let expected = 500.0 * DEFAULT_SAMPLES_TO_SECONDS;
        assert!(
            (hits[0].start_seconds - expected).abs() < 1.0,
            "{hits:?} expected ~{expected}"
        );
        assert!(
            hits[0].end_seconds - hits[0].start_seconds >= 10.0,
            "{hits:?}"
        );
    }

    #[test]
    fn unrelated_audio_matches_nothing() {
        let hits = match_bank(
            &fingerprint(noise(1, 2000)),
            &[entry(2, noise(99, 120))],
            &BankConfig::default(),
        );
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn two_adjacent_ads_merge_into_one_break() {
        let first = noise(11, 130);
        let second = noise(12, 130);
        let mut recording = noise(3, 3000);
        recording[1000..1130].copy_from_slice(&first);
        recording[1130..1260].copy_from_slice(&second);

        let hits = match_bank(
            &fingerprint(recording),
            &[entry(3, first), entry(4, second)],
            &BankConfig::default(),
        );
        assert_eq!(hits.len(), 2, "{hits:?}");

        let merged = merge_hits_into_breaks(&hits, 2.0);
        assert_eq!(merged.len(), 1, "{merged:?}");
        assert_eq!(merged[0].2, 2);
        assert!(merged[0].1 - merged[0].0 >= 25.0, "{merged:?}");
    }

    #[test]
    fn a_match_shorter_than_the_minimum_region_is_dropped() {
        let ad = noise(21, 40); // ~4.95 s, under the 10 s floor
        let mut recording = noise(5, 2000);
        recording[300..340].copy_from_slice(&ad);
        let hits = match_bank(
            &fingerprint(recording),
            &[entry(5, ad)],
            &BankConfig::default(),
        );
        assert!(hits.is_empty(), "{hits:?}");
    }
}
