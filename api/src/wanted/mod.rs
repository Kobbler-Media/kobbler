//! Wanted item handling - books/items users want to acquire
//!
//! This module handles:
//! - Syncing wanted items to Kobload watch patterns
//! - Receiving callbacks from Kobload when watches match

pub mod acquisition;
pub mod watch_sync;

pub use acquisition::*;
pub use watch_sync::*;
