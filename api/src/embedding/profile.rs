//! Every number that depends on the embedding model's score scale, in one place.
//!
//! Zero-shot scores are cosines between an image vector and a prompt vector, and
//! each model puts them on its own scale: EmbeddingGemma-2's text↔image cosine
//! differences are about 10× narrower than SigLIP2's sigmoid scores were, so a
//! threshold carried over unchanged silently stops firing. Consumers read
//! [`active`] (or [`for_model`] with a stored model id) instead of bare numbers.
//!
//! The EG2 values were tuned on 233 episodes (73 shows) + 15 movies against the
//! chapter-confirmed credit segments, held out by show, and on the real library
//! for search; the method and evidence are in the owner's
//! RESULTS-eg2-tuning.md (2026-11-04). The live-TV values are still reasoned, not
//! tuned: there is no ad ground truth to tune them against.

use super::EMBED_MODEL;

/// Meilisearch's `distribution` for a `userProvided` embedder: the typical raw
/// score (`mean`, on Meili's `(1 + cos) / 2` scale) and its spread (`sigma`).
/// Meili stretches scores around these so a good semantic match can compete with
/// keyword hits and an unrelated one falls towards zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Distribution {
    pub mean: f64,
    pub sigma: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct ModelProfile {
    pub model: &'static str,

    // ── Skip detect: credit text ────────────────────────────────────────────
    /// Per-frame `mean(credit prompts) - mean(non-credit prompts)` below which a
    /// frame is not a credit-text hit.
    pub credit_threshold: f64,
    /// The region-average score that counts as full confidence; confidence is
    /// `avg / credit_confidence_full`, clamped to 0.3–0.95.
    pub credit_confidence_full: f64,
    pub credit_prompts: &'static [&'static str],
    /// Contrast prompts: subtracting their mean cancels the per-frame offset every
    /// prompt shares.
    pub non_credit_prompts: &'static [&'static str],

    // ── Skip detect: frame self-similarity ──────────────────────────────────
    /// Image↔image cosine to the opening frames' centroid below which a credits
    /// region "breaks" into a post-credits scene.
    pub cluster_break_cosine: f64,
    /// Knots `(raw mean pairwise image↔image cosine, visual_score)`, ascending,
    /// mapping this model's consistency onto the 0–1 band fusion's cutoffs
    /// (0.2 / 0.5 / 0.7 / 0.9) were set on. Piecewise-linear between knots, with
    /// (0, 0) and (1, 1) implied at the ends, so each cutoff lands exactly on its
    /// tuned raw value; an affine map can't, and the 0.7 one matters.
    pub consistency_knots: &'static [(f64, f64)],

    // ── Live TV ad detection ────────────────────────────────────────────────
    pub ad_prompts: &'static [&'static str],
    pub programme_prompts: &'static [&'static str],
    /// `best ad cosine - best programme cosine` at or above which a frame is an ad.
    pub ad_margin: f64,

    // ── Semantic search ─────────────────────────────────────────────────────
    /// Query (text) ↔ summary (text).
    pub description_distribution: Distribution,
    /// Query (text) ↔ poster (image).
    pub poster_distribution: Distribution,
    /// Semantic-only hits below this corrected score are dropped: every query has
    /// *some* nearest title, and without a floor an unrelated one takes its
    /// type's top slot.
    pub semantic_hit_floor: f64,
}

impl ModelProfile {
    /// [`cluster_consistency`](crate::skip_detect::primitives::cosine::cluster_consistency)
    /// output on the scale fusion's `visual_score` cutoffs expect.
    pub fn visual_score(&self, raw_consistency: f64) -> f64 {
        let x = raw_consistency.clamp(0.0, 1.0);
        let mut prev = (0.0, 0.0);
        for &knot in self
            .consistency_knots
            .iter()
            .chain(std::iter::once(&(1.0, 1.0)))
        {
            if x <= knot.0 {
                let span = knot.0 - prev.0;
                if span <= 0.0 {
                    return knot.1;
                }
                return prev.1 + (x - prev.0) / span * (knot.1 - prev.1);
            }
            prev = knot;
        }
        1.0
    }
}

/// Greedy forward/backward selection over 33 credit and 24 contrast candidates
/// on pooled per-frame AUC (train split only), plain text: a task prefix scored
/// worse. SigLIP2's five-and-three set reached 0.956 under EG2; this one 0.969.
const CREDIT_PROMPTS: &[&str] = &[
    "white text on a black background",
    "names and titles on a dark screen",
    "production company logo",
    "a title card",
];

const NON_CREDIT_PROMPTS: &[&str] = &["people in a room", "an image"];

const AD_PROMPTS: &[&str] = &[
    "a television advertisement",
    "a commercial break",
    "a product advertisement with on-screen pricing",
    "a station identification bumper",
];

const PROGRAMME_PROMPTS: &[&str] = &[
    "a television programme",
    "a live sports broadcast",
    "a news broadcast",
    "a scene from a television show",
];

pub const EG2: ModelProfile = ModelProfile {
    model: EMBED_MODEL,
    // Held-out (82 episodes, 29 shows): 0 misses and start/end median 0.67 / 0.14 s
    // (p90 3.0 / 5.6), against SigLIP2's 4 misses and 0.92 / 0.16 (3.4 / 6.2) at
    // 0.10. The loss is flat from 0.0175 to 0.0375. A per-episode normalisation
    // (subtracting the episode's own p10/p25/median) was worse: the window is
    // often a third credits, so its "background" includes them.
    credit_threshold: 0.0275,
    // EG2 region averages sit around 0.06–0.08. SigLIP2's 0.5 here would clamp
    // every EG2 detection to the 0.3 floor, and fusion's credit-text test is
    // strictly `> 0.3`: credit text would silently stop counting.
    credit_confidence_full: 0.07,
    credit_prompts: CREDIT_PROMPTS,
    non_credit_prompts: NON_CREDIT_PROMPTS,
    // EG2 image↔image cosines are compressed upward (credit windows p10/p50/p90
    // 0.751 / 0.773 / 0.814, scenes 0.767 / 0.815 / 0.879); 0.68 is SigLIP2's 0.5
    // by quantile.
    cluster_break_cosine: 0.68,
    // Fusion cutoff ← raw EG2 consistency, measured on 246 credit windows, 225
    // pre-credit scenes and 1,413 random windows:
    //   0.2 ← 0.41, 0.5 ← 0.61: below every window either model produced (dead
    //   branches); affine extrapolation.
    //   0.7 ← 0.73: the high-variance credits veto's upper edge. The quantile map
    //   says 0.747, which vetoes 4.9 % of real credit windows; 0.73 vetoes none
    //   (SigLIP2 at 0.7: none either).
    //   0.9 ← 0.88: the post-credits same-cluster veto, by quantile.
    consistency_knots: &[(0.41, 0.2), (0.61, 0.5), (0.73, 0.7), (0.88, 0.9)],
    ad_prompts: AD_PROMPTS,
    programme_prompts: PROGRAMME_PROMPTS,
    // Not tuned (no ad ground truth). SigLIP2 used a sigmoid ratio (ad ≥ 1.5×
    // programme); cosines are all similar positive values, so a ratio sits near
    // 0.5 for every frame. A best-against-best margin is the cosine equivalent,
    // set near the tuned credit threshold.
    ad_margin: 0.03,
    // Set so EG2 behaves as SigLIP2's constants did on the same 52 natural and 15
    // off-topic queries: the same corrected score for the median off-topic top hit
    // and the median 20th on-topic hit, and the same off-topic pass rate at the
    // floor. Meili's correction formula was taken from its source from memory:
    // check one query against a live index before switching semantic search on.
    description_distribution: Distribution {
        mean: 0.815,
        sigma: 0.0205,
    },
    poster_distribution: Distribution {
        mean: 0.819,
        sigma: 0.0144,
    },
    // Tuned: summaries 0.2, posters 0.31. One floor serves both; summaries are
    // the primary embedder, and the poster query already carries a lower weight.
    semantic_hit_floor: 0.2,
};

pub const PROFILES: &[ModelProfile] = &[EG2];

/// The profile for a model id as recorded next to stored vectors.
pub fn for_model(model: &str) -> Option<&'static ModelProfile> {
    PROFILES.iter().find(|p| p.model == model)
}

/// The profile of [`EMBED_MODEL`], the model everything is embedded with now.
pub fn active() -> &'static ModelProfile {
    &EG2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_active_profile_is_the_embed_model() {
        assert_eq!(active().model, EMBED_MODEL);
        assert_eq!(for_model(EMBED_MODEL).map(|p| p.model), Some(EMBED_MODEL));
        assert!(for_model("google/siglip2-so400m-patch16-naflex").is_none());
    }

    #[test]
    fn visual_score_puts_each_fusion_cutoff_on_its_tuned_raw_value() {
        for (raw, cutoff) in [(0.41, 0.2), (0.61, 0.5), (0.73, 0.7), (0.88, 0.9)] {
            assert!((EG2.visual_score(raw) - cutoff).abs() < 1e-9, "{raw}");
        }
        assert_eq!(EG2.visual_score(0.0), 0.0);
        assert_eq!(EG2.visual_score(1.0), 1.0);
        assert_eq!(EG2.visual_score(1.5), 1.0);
        let mid = EG2.visual_score(0.67);
        assert!(mid > 0.5 && mid < 0.7);
    }

    #[test]
    fn no_knots_is_the_identity() {
        let p = ModelProfile {
            consistency_knots: &[],
            ..EG2
        };
        assert!((p.visual_score(0.42) - 0.42).abs() < 1e-9);
    }
}
