//! Wanted item handling - books/items users want to acquire
//!
//! This module handles:
//! - Keying and describing wanted books for Kobload
//! - Receiving callbacks from Kobload when watches match

pub mod acquisition;
pub mod alternate_titles;
pub mod book_source;
pub mod book_titles;

pub use acquisition::*;

pub const SOURCE_SYSTEM: &str = "kobbler";
