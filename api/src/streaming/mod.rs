//! Media streaming and playback.
//!
//! Handles the decision engine, session management, and subtitle processing
//! for browser-based media playback.

pub mod activity;
pub mod byte_range;
pub mod cache_download;
pub mod debrid_session;
pub mod decision;
pub mod download_meter;
pub mod feed;
pub mod language;
pub mod loudness;
pub mod mpegts;
pub mod offline_export;
pub mod persist;
pub mod presumed_audio;
pub mod progressive;
pub mod segment_cache;
pub mod session;
pub mod session_gate;
pub mod srt;
pub mod subtitle;
pub mod subtitle_insight;
pub mod variant;
