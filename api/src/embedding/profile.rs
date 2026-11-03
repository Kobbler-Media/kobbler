//! Every number that depends on the embedding model's score scale, in one place.
//!
//! Zero-shot scores are cosines between an image vector and a prompt vector, and
//! each model puts them on its own scale: EmbeddingGemma-2's text↔image cosine
//! differences are about 10× narrower than SigLIP2's sigmoid scores were, so a
//! threshold carried over unchanged silently stops firing. Consumers read
//! [`active`] (or [`for_model`] with a stored model id) instead of bare numbers.
//!
//! The EG2 values below are provisional pending tuning against the
//! chapter-confirmed credit segments and the real library; how each was reasoned
//! from its SigLIP2 predecessor is noted on it.

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
    /// Raw mean pairwise image↔image cosine mapped linearly from
    /// `[floor, ceiling]` onto the 0–1 `visual_score` band fusion's cutoffs
    /// (0.2 / 0.5 / 0.7 / 0.9) were set on.
    pub consistency_floor: f64,
    pub consistency_ceiling: f64,

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
        let span = self.consistency_ceiling - self.consistency_floor;
        if span <= 0.0 {
            return raw_consistency.clamp(0.0, 1.0);
        }
        ((raw_consistency - self.consistency_floor) / span).clamp(0.0, 1.0)
    }
}

/// Tuned against Invincible (white-on-black credits) and Scrubs 2026 (small text
/// over an active scene) under SigLIP2. Under EG2, with no task prefix, the same
/// set separated credits at per-episode AUC 0.998–1.0.
const CREDIT_PROMPTS: &[&str] = &[
    "white text on a black background",
    "end credits of a TV show",
    "names and titles on a dark screen",
    "production company logo",
    "credits text overlaid on a scene",
];

const NON_CREDIT_PROMPTS: &[&str] = &[
    "a scene from a TV show",
    "animated characters",
    "people in a room",
];

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
    // Measured on 857 frames from 4 episodes: 0.85 recall at 0 false positives at
    // 0.04, against SigLIP2's 0.90 at 0.10, prompts untuned.
    credit_threshold: 0.04,
    // SigLIP2 used 0.5, five times its threshold, against credit scores of
    // 0.15–0.80. EG2's scores sit about 10× closer together, so full confidence
    // moves to 0.10: a region averaging just over the threshold still reads as
    // credit text (above fusion's 0.3) rather than clamping to the floor.
    credit_confidence_full: 0.10,
    credit_prompts: CREDIT_PROMPTS,
    non_credit_prompts: NON_CREDIT_PROMPTS,
    // Same value as SigLIP2. The 10× narrowing is the text↔image gap; this is
    // image↔image cosine between unit vectors of one modality, which a dual
    // encoder spreads over a similar range for "same shot" vs "new scene".
    cluster_break_cosine: 0.5,
    // Identity, as under SigLIP2, for the same reason.
    consistency_floor: 0.0,
    consistency_ceiling: 1.0,
    ad_prompts: AD_PROMPTS,
    programme_prompts: PROGRAMME_PROMPTS,
    // SigLIP2 used a sigmoid ratio (ad ≥ 1.5× programme). Cosines are all similar
    // positive values, so a ratio sits near 0.5 for every frame; a margin is the
    // cosine equivalent, put just under the credit threshold because it compares
    // best-against-best rather than averages.
    ad_margin: 0.03,
    // Query↔summary cosines for an asymmetric retrieval model run roughly
    // 0.2–0.35 unrelated and 0.5–0.6 for a good match: Meili scale ≈0.66 ± 0.04.
    description_distribution: Distribution {
        mean: 0.66,
        sigma: 0.04,
    },
    // SigLIP2's raw text→image cosines ran 0.0–0.18 (0.53 ± 0.01). EG2's
    // cross-modal cosines run a little higher with a similarly narrow spread.
    poster_distribution: Distribution {
        mean: 0.57,
        sigma: 0.02,
    },
    // On the corrected scale, so it carries over from SigLIP2.
    semantic_hit_floor: 0.3,
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
    fn visual_score_maps_the_consistency_band_onto_zero_to_one() {
        let p = ModelProfile {
            consistency_floor: 0.4,
            consistency_ceiling: 0.9,
            ..EG2
        };
        assert!((p.visual_score(0.65) - 0.5).abs() < 1e-9);
        assert_eq!(p.visual_score(0.1), 0.0);
        assert_eq!(p.visual_score(0.99), 1.0);
        assert!((EG2.visual_score(0.42) - 0.42).abs() < 1e-9);
    }
}
