mod atomic;
mod basic;
mod kobload;

// Re-export handlers used in lib.rs
pub use atomic::atomic_ingest;
pub use basic::{create_entitlement, upload_cover, upload_file, upsert_book};
pub use kobload::kobload_ingest;

// Re-export types used externally
pub use kobload::{
    AUDIOBOOK_EXTENSIONS, EBOOK_EXTENSIONS, FileInfoExt, IngestOutcome, KobloadCallbackRequest,
    KobloadFileGroup, KobloadFileInfo, parse_subtitle_filename_hints, process_kobload_ingest,
};
