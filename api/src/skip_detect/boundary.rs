//! Shared boundary types for the skip-detect pipeline.
//!
//! The V1 async orchestration (live ffmpeg probes, audio-profile classifier,
//! vision frame extractor) was removed when V1 went away. What's left is the
//! `BoundaryResult` wire type consumed by `super::cache_detect` and
//! `super::primitives::boundary` — both read black-frame + silence intervals
//! from the cached sidecars and refine via the pure primitive.

/// Result of boundary analysis around a candidate region edge.
#[derive(Debug, Clone)]
pub struct BoundaryResult {
    pub refined_time: f64,
    pub has_black_frame: bool,
    pub has_silence: bool,
}
