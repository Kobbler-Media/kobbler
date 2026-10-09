//! Library routes module.
//!
//! This module contains all route handlers for the library API endpoints.

mod activity;
mod audiobooks;
mod authors;
mod availability;
mod books;
mod covers;
mod devices;
mod genres;
pub mod helpers;
mod images;
mod monitoring;
pub mod search_download;
mod series;
mod shelves;
mod stats;
pub mod types;
mod wanted;

// Re-export all handlers used in the router

// Activity
pub use activity::list_activity;

// Audiobooks (koblem LibraryProvider)
pub use audiobooks::{get_audiobook, get_audiobook_clip, search_audiobooks};

// Authors
pub use authors::{
    create_author, delete_author, get_author, list_author_books, list_authors, merge_authors,
    rename_author, update_author,
};

// Availability
pub use availability::{
    check_book_availability, delete_unavailable_books, get_book_recommendations,
    list_unavailable_books,
};

// Books
pub use books::{
    available_letters, create_book, delete_book, delete_book_file, facets, first_by_letter,
    get_book, get_book_files, list_books, move_book_file, set_book_primary_series, touch_books,
    update_book,
};

// Covers
pub use covers::{cover, cover_from_url, extracted_cover, upload_cover, use_extracted_cover};

// Devices
pub use devices::{list_devices, sync_books};

// Genres
pub use genres::{list_genre_books, list_genres};

// Images
pub use images::{get_author_image, get_series_image};

// Monitoring
pub use monitoring::{
    apply_author_metadata, apply_series_metadata, refresh_author_metadata, refresh_series_metadata,
    update_author_monitoring, update_series_monitoring,
};

// Search & Download
pub use search_download::{download_to_library, search_library_releases};

// Series
pub use series::{
    create_series, delete_series, get_series, list_series, list_series_books, merge_series,
    rename_series, reorder_series, update_series,
};

// Shelves
pub use shelves::{
    add_shelf_items, create_shelf, delete_shelf, list_shelf_items, list_shelves,
    remove_shelf_items, rename_shelf,
};

// Stats
pub use stats::get_stats;

// Wanted
pub use wanted::{
    skip_book, unwant_all_for_author, unwant_all_for_series, want_all_pending_for_author,
    want_all_pending_for_series, want_book,
};
