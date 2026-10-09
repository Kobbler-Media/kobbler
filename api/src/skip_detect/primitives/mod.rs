//! Pure analysis primitives for skip detection.
//!
//! Each submodule contains side-effect-free functions over slices that the
//! higher-level detectors (`vision`, `chromaprint`, `vhash`, `boundary`) call
//! into after performing their I/O. Keeping the math separate from FFmpeg /
//! HTTP makes it cheap to test and reason about.

pub mod boundary;
pub mod cluster;
pub mod cosine;
pub mod credit_scoring;
pub mod fingerprint_match;
pub mod vhash_align;
