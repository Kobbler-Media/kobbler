//! Kobo sync protocol implementation - shared core logic.
//!
//! This module contains the core business logic for the Kobo sync protocol.
//! It is used by both:
//! - The axum route handlers in `routes/kobo.rs`
//! - The proxy handler in `kobo_handler.rs`

pub mod device;
pub mod entitlement;
pub mod files;
pub mod metadata;
pub mod position;
pub mod progress;
pub mod reading_state;
pub mod recommendations;
pub mod resources;
pub mod sync;
pub mod sync_token;
pub mod tags;
pub mod types;

// Re-export commonly used items
pub use device::{DeviceContext, DeviceLookupError, lookup_device};
pub use entitlement::{book_exists, remove_book_entitlement, series_exists, tag_exists};
pub use files::{
    CoverVariant, is_greyscale_flag, load_cover_original, load_or_build_cover_variant,
    parse_dimension, parse_quality, prepare_download,
};
pub use metadata::{build_metadata, fetch_book_metadata};
pub use reading_state::{
    ReadingStateUpdate, ResolvedState, build_reading_state_response, ensure_reading_state,
    reading_state_sync_json, resolve_state_for_device, update_reading_state,
};
pub use recommendations::{build_product_recommendation, get_recommendations};
pub use resources::{build_initialization_resources, merge_resources};
pub use sync::{SyncResponse, build_sync_response, commit_sync_state};
pub use sync_token::SyncToken;
pub use tags::{TagItemInput, add_tag_items, create_tag, delete_tag, remove_tag_items, rename_tag};
