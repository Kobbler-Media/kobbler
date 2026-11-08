//! Full-cast audiobooks: dialogue attributed per chapter by the resident LLM,
//! merged into one cast across the book by beefy, given voice designs in small
//! batches, voiced once the cast is vetted.

pub mod bible;
pub mod carry;
pub mod chapters;
pub mod eta;
pub mod llm;
pub mod page;
pub mod quotes;
pub mod resolve;
pub mod voices;

use std::time::Duration;

/// A speaker with fewer lines than this shares a pooled voice instead of
/// getting one of their own to audition.
pub const POOL_BELOW: i32 = 5;
/// ~12k tokens a window: room in gemma's context for the instructions and a
/// reply that lists every quote.
pub const WINDOW_CHARS: usize = 48_000;

/// Outlasts an engine crash: the exl3 supervisor's kill, restart and model
/// reload took minutes on .208, far past the job queue's own ~30 s of retries.
pub const LLM_RETRY: &[Duration] = &[
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(120),
    Duration::from_secs(240),
];
