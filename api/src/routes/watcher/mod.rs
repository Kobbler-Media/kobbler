//! Routes for ingest queue management.

mod metadata;
mod queue;

// Re-export queue handlers
pub use queue::{
    approve_ingest_item, batch_reject_ingest_items, delete_ingest_queue_item,
    get_ingest_queue_item, list_ingest_queue, reject_ingest_item, restore_ingest_item, upload_book,
    upload_ingest_cover,
};

// Re-export metadata handlers
pub use metadata::{extract_metadata, get_ingest_queue_cover};
