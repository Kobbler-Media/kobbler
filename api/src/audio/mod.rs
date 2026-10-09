//! Audio processing for audiobook support.
//!
//! Provides ffprobe-based metadata/chapter extraction, chapter resolution
//! fallback chain, and M4B-to-MP3Z conversion for Kobo devices.

pub mod auto_chapters;
pub mod chapter_detect;
pub mod chapters;
pub mod clip;
pub mod convert;
pub mod path;
pub mod peaks;
pub mod peaky;
pub mod probe;
pub mod repair;
pub mod stt;
