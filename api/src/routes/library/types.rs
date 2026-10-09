//! Shared types for library routes.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Pagination Types
// =============================================================================

/// Query parameter to enable debug fields in responses.
#[derive(Debug, Deserialize, Default)]
pub struct DebugQuery {
    #[serde(default)]
    pub debug: Option<bool>,
}

#[derive(Deserialize)]
pub struct PaginationParams {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub search: Option<String>,
    pub filter: Option<String>, // all | synced | pending
}

#[derive(Serialize)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_count: Option<i64>,
}

// =============================================================================
// Book Types
// =============================================================================

#[derive(Deserialize)]
pub struct ListBooksQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub search: Option<String>,
    pub filter: Option<String>, // all | synced | pending (for entitlement status)
    /// Filter by in_library status. If None, show all books. If true, only library books. If false, only non-library books.
    pub in_library: Option<bool>,
    /// Filter by want_status (e.g., "wanted", "pending", "queued", "downloading", "needs_review", "completed", "failed", "skipped")
    /// Prefix with ! to exclude (e.g., "!skipped" excludes skipped books)
    pub want_status: Option<String>,
    /// Center results around this book ID (for deep linking)
    pub around: Option<String>,
    /// Number of items to show before the "around" target. Defaults to half of limit (centered).
    /// Set to a small value (e.g., 3) for letter navigation to show target near top.
    pub around_before: Option<i64>,
    /// Pagination direction: "forward" (default) or "backward"
    pub direction: Option<String>,
    /// Sort order: "title" (alphabetical, default) or "created_at" (recent first)
    pub sort: Option<String>,
    /// Offset for search pagination (0-indexed). When searching, use this instead of cursor
    /// to page through MeiliSearch results. Each page should increment by limit.
    pub search_offset: Option<usize>,
    /// Filter to books that have audiobook files
    pub has_audiobook: Option<bool>,
    /// Filter by media format: "ebook" (has ebook files or wants ebook),
    /// "audiobook" (has audiobook files or wants audiobook)
    pub format: Option<String>,
    /// Filter to books with no media root (orphaned)
    pub orphaned: Option<bool>,
}

/// Query parameters for finding first book by letter (alphabet bar jump)
#[derive(Deserialize)]
pub struct FirstByLetterQuery {
    /// Letter to find (A-Z or # for non-alphabetic)
    pub letter: String,
    /// Filter type (all | synced | pending)
    pub filter: Option<String>,
    /// Filter by in_library status
    pub in_library: Option<bool>,
    /// Filter by media format: "ebook" or "audiobook"
    pub format: Option<String>,
}

/// Response for first-by-letter endpoint
#[derive(Serialize)]
pub struct FirstByLetterResponse {
    pub id: Uuid,
    /// Title of the book (needed to create a cursor for forward pagination)
    pub title: String,
}

/// Query parameters for available letters endpoint
#[derive(Deserialize)]
pub struct AvailableLettersQuery {
    /// Filter type (all | synced | pending)
    pub filter: Option<String>,
    /// Filter by in_library status
    pub in_library: Option<bool>,
    /// Filter by media format: "ebook" or "audiobook"
    pub format: Option<String>,
}

/// Response for available letters endpoint
#[derive(Serialize)]
pub struct AvailableLettersResponse {
    /// Letters that have at least one book (A-Z, #)
    pub letters: Vec<String>,
}

/// Author reference with ID and name paired together.
/// For pending authors (ingest items not yet approved), `id` is `None`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorRef {
    pub id: Option<Uuid>,
    pub name: String,
}

/// Shelf reference with ID and name for book-level shelf membership.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShelfRef {
    pub id: Uuid,
    pub name: String,
}

#[derive(Serialize)]
pub struct LibraryBook {
    pub id: Uuid,
    pub title: String,
    pub authors: Vec<AuthorRef>,
    pub description: Option<String>,
    pub publisher: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub pubdate: Option<OffsetDateTime>,
    pub series_id: Option<Uuid>,
    pub series_name: Option<String>,
    pub series_index: Option<f64>,
    pub language: Option<String>,
    pub formats: Vec<String>,
    pub synced: bool,
    pub pending_sync: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
    /// URL to the extracted cover (from ebook file), if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extracted_cover_url: Option<String>,
    pub identifiers: serde_json::Value,
    /// Whether the book is in the user's library
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_in_library: Option<bool>,
    /// Want status for non-library books (none, pending, wanted, skipped)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub want_status: Option<String>,
    /// Page count
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_count: Option<i32>,
    /// Average rating (0.0 - 5.0 scale)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    /// Number of ratings
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating_count: Option<i32>,
    /// Genres/subjects
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    /// Shelves this book belongs to (user-scoped)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shelves: Vec<ShelfRef>,
    /// Raw provider metadata (only included when ?debug=true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub narrator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<i32>,
    pub is_abridged: bool,
    pub want_format: String,
    pub has_audiobook: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_root_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_memberships: Option<Vec<crate::repositories::book_series::BookSeriesEntry>>,
}

/// Author input with optional external ID for linking.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthorInput {
    pub name: String,
    #[serde(default)]
    pub hardcover_id: Option<String>,
}

#[derive(Deserialize)]
pub struct BookUpdate {
    pub title: Option<String>,
    pub authors: Option<Vec<AuthorInput>>,
    pub description: Option<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<String>,
    pub series_id: Option<Uuid>,
    pub series_name: Option<String>,
    pub series_hardcover_id: Option<String>,
    pub series_index: Option<f64>,
    pub identifiers: Option<serde_json::Value>,
    pub genres: Option<Vec<String>>,
    pub page_count: Option<i32>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    /// Source metadata (ingest items only): provenance info
    pub source_metadata: Option<serde_json::Value>,
    /// Cover image URL to fetch and store (ingest items only).
    /// For library books, cover_url is set directly on the books table.
    /// For ingest items, the image is downloaded and stored as extracted_cover bytes.
    pub cover_url: Option<String>,
    /// Audiobook narrator
    pub narrator: Option<String>,
    /// Whether this is an abridged edition
    pub is_abridged: Option<bool>,
    /// Preferred acquisition format (e.g., "any", "ebook", "audiobook")
    pub want_format: Option<String>,
    /// Bulk-replace all series memberships. Each entry needs series_id, optional series_index, and is_primary.
    pub series_memberships: Option<Vec<SeriesMembershipInput>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeriesMembershipInput {
    pub series_id: Uuid,
    pub series_index: Option<f64>,
    pub is_primary: bool,
}

/// Request for creating a new book (wanted or library book without files)
#[derive(Deserialize)]
pub struct BookCreateRequest {
    pub title: String,
    pub authors: Option<Vec<AuthorInput>>,
    pub description: Option<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<String>,
    /// Series ID (preferred over series_name when selecting from dropdown)
    pub series_id: Option<Uuid>,
    pub series_name: Option<String>,
    pub series_hardcover_id: Option<String>,
    pub series_index: Option<f64>,
    pub identifiers: Option<serde_json::Value>,
    pub genres: Option<Vec<String>>,
    pub page_count: Option<i32>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub cover_url: Option<String>,
    /// Audiobook narrator
    pub narrator: Option<String>,
    /// Whether this is an abridged edition
    pub is_abridged: Option<bool>,
    /// Preferred acquisition format (e.g., "any", "ebook", "audiobook")
    pub want_format: Option<String>,
    /// If true, creates as a wanted book (is_in_library=false, want_status='wanted')
    /// If false or not set, creates as a library book without files
    #[serde(default)]
    pub want: bool,
}

#[derive(Serialize)]
pub struct BookFileInfo {
    pub id: Uuid,
    pub format: String,
    pub size_bytes: Option<i64>,
    pub filename: String,
    pub file_path: String,
    pub source_type: Option<String>,
    pub source_metadata: Option<serde_json::Value>,
    #[serde(with = "time::serde::rfc3339")]
    pub added_at: OffsetDateTime,
    pub available: bool,
}

#[derive(Serialize)]
pub struct AudioFileInfo {
    pub id: Uuid,
    pub file_index: i32,
    pub format: String,
    pub filename: String,
    pub file_size: i64,
    pub duration_seconds: Option<f64>,
    pub available: bool,
}

#[derive(Serialize)]
pub struct BookFilesResponse {
    pub files: Vec<BookFileInfo>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub audio_files: Vec<AudioFileInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_summary: Option<AudioSummary>,
}

#[derive(Serialize)]
pub struct AudioSummary {
    pub count: usize,
    pub total_size: i64,
    pub total_duration_seconds: Option<f64>,
    pub format: String,
}

// =============================================================================
// Series Types
// =============================================================================

#[derive(Serialize)]
pub struct SeriesSummary {
    pub id: Uuid,
    pub name: String,
    pub book_count: i64,
    pub wanted_count: i64,
    pub authors: Vec<AuthorRef>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    pub cover_urls: Vec<String>,
    // Monitoring fields
    pub identifiers: serde_json::Value,
    pub image_url: Option<String>,
    pub description: Option<String>,
    pub auto_want_mode: String,
    pub total_book_count: Option<i32>,
    pub is_monitored: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub monitoring_started_at: Option<OffsetDateTime>,
    /// Raw provider metadata (only included when ?debug=true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct ListSeriesQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub search: Option<String>,
    /// If true, include series with no library books
    #[serde(default)]
    pub include_no_books: bool,
    /// Filter by is_monitored status
    pub is_monitored: Option<bool>,
    /// Center results around this series ID (for deep linking)
    pub around: Option<String>,
    /// Pagination direction: "forward" (default) or "backward"
    pub direction: Option<String>,
    /// Offset for search pagination (0-indexed). When searching, use this instead of cursor
    /// to page through MeiliSearch results. Each page should increment by limit.
    pub search_offset: Option<usize>,
    /// Filter by media format: "ebook" or "audiobook" (series with at least one matching book)
    pub format: Option<String>,
}

#[derive(Deserialize)]
pub struct SeriesRenameRequest {
    pub name: String,
}

#[derive(Deserialize)]
pub struct SeriesCreateRequest {
    pub name: String,
    pub identifiers: Option<serde_json::Value>,
    pub image_url: Option<String>,
    pub description: Option<String>,
    pub total_book_count: Option<i32>,
    pub is_monitored: Option<bool>,
    pub auto_want_mode: Option<String>,
}

#[derive(Deserialize)]
pub struct SeriesUpdateRequest {
    pub name: Option<String>,
    pub identifiers: Option<serde_json::Value>,
    pub image_url: Option<String>,
    pub description: Option<String>,
    pub total_book_count: Option<i32>,
    pub is_monitored: Option<bool>,
    pub auto_want_mode: Option<String>,
}

#[derive(Deserialize)]
pub struct SeriesMergeRequest {
    pub source_id: Uuid,
    pub target_id: Uuid,
}

#[derive(Deserialize)]
pub struct SeriesReorderRequest {
    pub book_id: Uuid,
    pub series_index: Option<f64>,
}

// =============================================================================
// Author Types
// =============================================================================

#[derive(Serialize)]
pub struct AuthorSummary {
    pub id: Uuid,
    pub name: String,
    pub book_count: i64,
    pub series_count: i64,
    /// Total books in DB for this author (regardless of is_in_library)
    pub total_book_count: i64,
    /// Total series in DB for this author (regardless of is_in_library)
    pub total_series_count: i64,
    pub wanted_count: i64,
    pub cover_urls: Vec<String>,
    // Monitoring fields
    pub identifiers: serde_json::Value,
    pub image_url: Option<String>,
    pub bio: Option<String>,
    pub is_monitored: bool,
    pub auto_want_mode: String,
    pub known_book_count: Option<i32>,
    /// Top series names for disambiguation (e.g., "Stormlight Archive, Mistborn")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_series_names: Option<String>,
    /// Raw provider metadata (only included when ?debug=true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct ListAuthorsQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub search: Option<String>,
    /// If true, include authors with no library books
    #[serde(default)]
    pub include_no_books: bool,
    /// Filter by is_monitored status
    pub is_monitored: Option<bool>,
    /// Center results around this author ID (for deep linking)
    pub around: Option<String>,
    /// Pagination direction: "forward" (default) or "backward"
    pub direction: Option<String>,
    /// Offset for search pagination (0-indexed). When searching, use this instead of cursor
    /// to page through MeiliSearch results. Each page should increment by limit.
    pub search_offset: Option<usize>,
    /// Filter by media format: "ebook" or "audiobook" (authors with at least one matching book)
    pub format: Option<String>,
}

#[derive(Deserialize)]
pub struct AuthorRenameRequest {
    pub name: String,
}

#[derive(Deserialize)]
pub struct AuthorCreateRequest {
    pub name: String,
    pub identifiers: Option<serde_json::Value>,
    pub image_url: Option<String>,
    pub bio: Option<String>,
    pub is_monitored: Option<bool>,
    pub auto_want_mode: Option<String>,
}

#[derive(Deserialize)]
pub struct AuthorUpdateRequest {
    pub name: Option<String>,
    pub identifiers: Option<serde_json::Value>,
    pub image_url: Option<String>,
    pub bio: Option<String>,
    pub is_monitored: Option<bool>,
    pub auto_want_mode: Option<String>,
}

#[derive(Deserialize)]
pub struct AuthorMergeRequest {
    pub source_id: Uuid,
    pub target_id: Uuid,
}

// =============================================================================
// Shelf Types
// =============================================================================

#[derive(Serialize)]
pub struct ShelfSummary {
    pub id: Uuid,
    pub name: String,
    pub user_id: Uuid,
    pub book_count: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub last_modified: OffsetDateTime,
    pub cover_urls: Vec<String>,
}

#[derive(Deserialize)]
pub struct ShelfCreateRequest {
    pub name: String,
}

#[derive(Deserialize)]
pub struct ShelfUpdateRequest {
    pub name: String,
}

#[derive(Deserialize)]
pub struct ShelfItemsRequest {
    pub book_ids: Vec<Uuid>,
}

#[derive(Deserialize)]
pub struct ShelfQuery {
    // user_id removed - no cross-user access until permissions are implemented
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

// =============================================================================
// Genre Types
// =============================================================================

#[derive(Serialize)]
pub struct GenreSummary {
    pub id: Uuid,
    pub name: String,
    pub book_count: i64,
}

// =============================================================================
// Activity Types
// =============================================================================

#[derive(Serialize)]
pub struct ActivityEvent {
    pub id: Uuid,
    pub event_kind: String,
    pub entity_type: Option<String>,
    pub entity_id: Option<Uuid>,
    pub title: String,
    pub subtitle: Option<String>,
    pub image_url: Option<String>,
    pub url: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

// =============================================================================
// Cover Types
// =============================================================================

#[derive(Deserialize)]
pub struct CoverQuery {
    pub token: Option<String>,
}

#[derive(Deserialize)]
pub struct CoverFromUrlRequest {
    pub url: String,
}

// =============================================================================
// Monitoring Types
// =============================================================================

#[derive(Deserialize)]
pub struct UpdateMonitoringRequest {
    pub is_monitored: bool,
    pub auto_want_mode: Option<String>,
}

#[derive(Deserialize)]
pub struct ApplyMetadataRequest {
    pub apply_to_books: Option<bool>,
}

// =============================================================================
// Search & Download Types
// =============================================================================

#[derive(Deserialize)]
pub struct SearchReleasesQuery {
    pub query: Option<String>,
    pub book_id: Option<Uuid>,
}

#[derive(Serialize)]
pub struct SearchReleaseResult {
    pub title: String,
    pub indexer: String,
    pub size: i64,
    pub protocol: String,
    pub download_url: String,
    pub info_url: Option<String>,
    pub seeders: Option<i32>,
    pub leechers: Option<i32>,
    pub age_days: Option<i32>,
    pub categories: Vec<String>,
    pub guid: String,
}

#[derive(Deserialize)]
pub struct DownloadRequest {
    pub download_url: String,
    pub title: String,
    pub indexer: String,
    pub protocol: String,
    pub book_id: Uuid,
}

// =============================================================================
// Availability Types
// =============================================================================

/// Book with unavailable files summary
#[derive(Serialize)]
pub struct UnavailableBook {
    pub id: Uuid,
    pub title: String,
    pub authors: Vec<String>,
    pub unavailable_files: i64,
    pub total_files: i64,
    #[serde(with = "time::serde::rfc3339::option")]
    pub unavailable_since: Option<OffsetDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
}

/// Response for unavailable books list
#[derive(Serialize)]
pub struct UnavailableBooksResponse {
    pub items: Vec<UnavailableBook>,
    pub total_count: i64,
}

/// Recommendation for a book
#[derive(Serialize)]
pub struct BookRecommendation {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub series_name: Option<String>,
    pub series_index: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
    pub reason: String,
    /// Reading progress (0-100), None if never opened.
    pub progress_percent: Option<f64>,
}
