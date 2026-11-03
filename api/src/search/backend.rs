//! Search backend trait for abstracting search implementations.

use async_trait::async_trait;
use uuid::Uuid;

use serde::Serialize;

use super::semantic::SemanticQuery;
use super::{
    AuthorDocument, BookDocument, CommandDocument, GenreDocument, IptvChannelDocument,
    IptvEpgDocument, LivetvChannelDocument, LivetvGuideDocument, MediaRootFileDocument,
    MovieDocument, PageDocument, PersonDocument, SeriesDocument, SettingDocument,
    TranscriptSegmentDocument, TvEpisodeDocument, TvSeasonDocument, TvShowDocument,
    UnifiedSearchResponse, UnifiedSearchResult,
};

/// Error type for search operations.
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("Backend error: {0}")]
    Backend(String),

    #[error("Connection error: {0}")]
    Connection(String),

    #[error("Entity not found: {0}")]
    NotFound(String),
}

impl From<meilisearch_sdk::errors::Error> for SearchError {
    fn from(e: meilisearch_sdk::errors::Error) -> Self {
        SearchError::Backend(e.to_string())
    }
}

impl From<sqlx::Error> for SearchError {
    fn from(e: sqlx::Error) -> Self {
        SearchError::Backend(e.to_string())
    }
}

/// Status information about the search backend.
#[derive(Debug, Clone, Serialize)]
pub struct SearchBackendStatus {
    /// Backend type (e.g., "meilisearch", "sql_fallback").
    pub backend_type: String,
    /// Whether the backend is connected and healthy.
    pub connected: bool,
    /// Connection URL (masked for security).
    pub url: String,
    /// Number of indexed books.
    pub book_count: Option<i64>,
    /// Number of indexed authors.
    pub author_count: Option<i64>,
    /// Number of indexed series.
    pub series_count: Option<i64>,
    /// Number of indexed genres.
    pub genre_count: Option<i64>,
    /// Number of indexed movies.
    pub movie_count: Option<i64>,
    /// Number of indexed TV shows.
    pub tv_show_count: Option<i64>,
    /// Number of indexed TV seasons.
    pub tv_season_count: Option<i64>,
    /// Number of indexed TV episodes.
    pub tv_episode_count: Option<i64>,
    /// Number of indexed people.
    pub person_count: Option<i64>,
    /// Titles carrying a summary or artwork vector across the movie, show and book indexes.
    pub embedded_document_count: Option<i64>,
    /// Error message if not connected.
    pub error: Option<String>,
}

/// Filter options for book searches.
#[derive(Debug, Default, Clone)]
pub struct BookSearchFilters {
    /// Filter by in_library status.
    pub is_in_library: Option<bool>,
    /// Filter by want_status (e.g., "wanted", "none").
    pub want_status: Option<String>,
    /// If true, exclude items with ingest_status (only show library books).
    pub exclude_ingest_queue: bool,
    /// Filter by media format: "ebook" or "audiobook".
    pub format: Option<String>,
}

/// Filter options for author searches.
#[derive(Debug, Default, Clone)]
pub struct AuthorSearchFilters {
    /// Filter by monitored status.
    pub is_monitored: Option<bool>,
}

/// Filter options for series searches.
#[derive(Debug, Default, Clone)]
pub struct SeriesSearchFilters {
    /// Filter by monitored status.
    pub is_monitored: Option<bool>,
}

/// Filter options for movie searches.
#[derive(Debug, Default, Clone)]
pub struct MovieSearchFilters {
    /// Filter by in_library status.
    pub is_in_library: Option<bool>,
    /// Filter by want_status (e.g., "wanted", "none").
    pub want_status: Option<String>,
}

/// Filter options for TV show searches.
#[derive(Debug, Default, Clone)]
pub struct TvShowSearchFilters {
    /// Filter by monitored status.
    pub is_monitored: Option<bool>,
    /// Filter by show status (e.g., "continuing", "ended").
    pub status: Option<String>,
}

/// Filter options for people searches (currently no filters, but kept for consistency).
#[derive(Debug, Default, Clone)]
pub struct PersonSearchFilters {}

/// Filter options for IPTV channel searches.
#[derive(Debug, Default)]
pub struct IptvChannelSearchFilters {
    pub category: Option<String>,
    pub group: Option<String>,
    pub languages: Option<Vec<String>>,
    pub enabled: Option<bool>,
}

/// Filter options for IPTV EPG searches.
#[derive(Debug, Default)]
pub struct IptvEpgSearchFilters {
    pub channel_id: Option<String>,
    pub categories: Option<Vec<String>>,
    pub is_live: Option<bool>,
    pub start_after: Option<i64>,
    pub start_before: Option<i64>,
}

/// Filter options for Live TV channel searches.
#[derive(Debug, Default)]
pub struct LivetvChannelSearchFilters {
    pub network: Option<String>,
    pub region: Option<String>,
    pub kind: Option<String>,
    pub enabled: Option<bool>,
    pub favourite: Option<bool>,
    pub hd: Option<bool>,
}

/// Filter options for Live TV guide searches.
#[derive(Debug, Default)]
pub struct LivetvGuideSearchFilters {
    pub channel_id: Option<String>,
    pub genres: Option<Vec<String>>,
    pub start_after: Option<i64>,
    pub start_before: Option<i64>,
}

/// A single facet value and the number of matching documents, for drill-down
/// filter UIs (e.g. genre "Fantasy" → 42 books under the current filters).
#[derive(Debug, Clone, Serialize)]
pub struct FacetBucket {
    pub value: String,
    pub count: usize,
}

/// Contextual facet counts for a library list. `status` carries the entity's
/// status-like facet — `want_status` for books/movies, show `status` for TV.
#[derive(Debug, Clone, Default, Serialize)]
pub struct LibraryFacets {
    pub genres: Vec<FacetBucket>,
    pub status: Vec<FacetBucket>,
}

/// Trait for search backend implementations.
///
/// This trait abstracts the search backend (e.g., MeiliSearch, PostgreSQL)
/// allowing for different implementations while maintaining a consistent API.
#[async_trait]
pub trait SearchBackend: Send + Sync {
    // =========================================================================
    // Unified Search (Command Palette) - Federated
    // =========================================================================

    /// Search across all entity types using federated search.
    ///
    /// Returns results ranked by relevance across all indexes, with `top_hits`
    /// containing the best hit of each type and `remaining` containing everything else.
    /// `semantic` adds an artwork-vector half to the movie, show and book queries;
    /// a backend that cannot run it answers by keyword alone rather than failing.
    async fn unified_search(
        &self,
        query: &str,
        limit: usize,
        semantic: Option<&SemanticQuery>,
    ) -> Result<UnifiedSearchResponse, SearchError>;

    /// Legacy search across all entity types (grouped by type).
    /// Deprecated: Use `unified_search` for relevance-ranked results.
    async fn search(&self, query: &str, limit: usize) -> Result<UnifiedSearchResult, SearchError>;

    // =========================================================================
    // Entity-Specific Search (List Filtering)
    // =========================================================================

    /// Search books with filters, returning IDs only.
    ///
    /// Results are pre-filtered by text query AND attribute filters.
    /// The returned IDs can then be used in SQL queries for full data retrieval.
    /// Use `offset` for pagination - when loading page N, pass offset = (N-1) * limit.
    async fn search_books(
        &self,
        query: &str,
        filters: BookSearchFilters,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Uuid>, SearchError>;

    /// Search authors with filters, returning IDs only.
    /// Use `offset` for pagination - when loading page N, pass offset = (N-1) * limit.
    async fn search_authors(
        &self,
        query: &str,
        filters: AuthorSearchFilters,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Uuid>, SearchError>;

    /// Search series with filters, returning IDs only.
    /// Use `offset` for pagination - when loading page N, pass offset = (N-1) * limit.
    async fn search_series(
        &self,
        query: &str,
        filters: SeriesSearchFilters,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Uuid>, SearchError>;

    /// Search movies with filters, returning IDs only.
    /// Use `offset` for pagination - when loading page N, pass offset = (N-1) * limit.
    async fn search_movies(
        &self,
        query: &str,
        filters: MovieSearchFilters,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Uuid>, SearchError>;

    /// Search TV shows with filters, returning IDs only.
    /// Use `offset` for pagination - when loading page N, pass offset = (N-1) * limit.
    async fn search_tv_shows(
        &self,
        query: &str,
        filters: TvShowSearchFilters,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Uuid>, SearchError>;

    /// Search people, returning IDs only.
    /// Use `offset` for pagination - when loading page N, pass offset = (N-1) * limit.
    async fn search_people(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Uuid>, SearchError>;

    /// Get available first letters for books (for alphabet bar).
    ///
    /// Returns letters (A-Z, #) that have at least one matching book.
    async fn get_available_book_letters(
        &self,
        filters: BookSearchFilters,
    ) -> Result<Vec<String>, SearchError>;

    /// Get available first letters for movies (for alphabet bar).
    ///
    /// Returns letters (A-Z, #) that have at least one matching movie.
    async fn get_available_movie_letters(
        &self,
        filters: MovieSearchFilters,
    ) -> Result<Vec<String>, SearchError>;

    /// Get available first letters for TV shows (for alphabet bar).
    ///
    /// Returns letters (A-Z, #) that have at least one matching TV show.
    async fn get_available_tv_show_letters(
        &self,
        filters: TvShowSearchFilters,
    ) -> Result<Vec<String>, SearchError>;

    /// Contextual facet counts for the book library (genres + want_status),
    /// scoped to the supplied filters, for drill-down filter UIs.
    async fn get_book_facets(
        &self,
        filters: BookSearchFilters,
    ) -> Result<LibraryFacets, SearchError>;

    /// Contextual facet counts for the movie library (genres + want_status).
    async fn get_movie_facets(
        &self,
        filters: MovieSearchFilters,
    ) -> Result<LibraryFacets, SearchError>;

    /// Contextual facet counts for the TV show library (genres + show status).
    async fn get_tv_show_facets(
        &self,
        filters: TvShowSearchFilters,
    ) -> Result<LibraryFacets, SearchError>;

    // =========================================================================
    // Indexing Operations
    // =========================================================================

    /// Index a book document.
    async fn index_book(&self, doc: BookDocument) -> Result<(), SearchError>;

    /// Index an author document.
    async fn index_author(&self, doc: AuthorDocument) -> Result<(), SearchError>;

    /// Index a series document.
    async fn index_series(&self, doc: SeriesDocument) -> Result<(), SearchError>;

    /// Delete a book from the index.
    async fn delete_book(&self, id: Uuid) -> Result<(), SearchError>;

    /// Delete an author from the index.
    async fn delete_author(&self, id: Uuid) -> Result<(), SearchError>;

    /// Delete a series from the index.
    async fn delete_series(&self, id: Uuid) -> Result<(), SearchError>;

    /// Index a genre document.
    async fn index_genre(&self, doc: GenreDocument) -> Result<(), SearchError>;

    /// Delete a genre from the index.
    async fn delete_genre(&self, id: Uuid) -> Result<(), SearchError>;

    /// Index a movie document.
    async fn index_movie(&self, doc: MovieDocument) -> Result<(), SearchError>;

    /// Index a TV show document.
    async fn index_tv_show(&self, doc: TvShowDocument) -> Result<(), SearchError>;

    /// Delete a movie from the index.
    async fn delete_movie(&self, id: Uuid) -> Result<(), SearchError>;

    /// Delete a TV show from the index.
    async fn delete_tv_show(&self, id: Uuid) -> Result<(), SearchError>;

    /// Index a TV season document.
    async fn index_tv_season(&self, doc: TvSeasonDocument) -> Result<(), SearchError>;

    /// Index a TV episode document.
    async fn index_tv_episode(&self, doc: TvEpisodeDocument) -> Result<(), SearchError>;

    /// Index multiple TV season documents in a single batch.
    async fn index_tv_seasons_batch(&self, docs: Vec<TvSeasonDocument>) -> Result<(), SearchError>;

    /// Index multiple TV episode documents in a single batch.
    async fn index_tv_episodes_batch(
        &self,
        docs: Vec<TvEpisodeDocument>,
    ) -> Result<(), SearchError>;

    /// Delete a TV season from the index.
    async fn delete_tv_season(&self, id: Uuid) -> Result<(), SearchError>;

    /// Delete a TV episode from the index.
    async fn delete_tv_episode(&self, id: Uuid) -> Result<(), SearchError>;

    /// Index a person document.
    async fn index_person(&self, doc: PersonDocument) -> Result<(), SearchError>;

    /// Delete a person from the index.
    async fn delete_person(&self, id: Uuid) -> Result<(), SearchError>;

    /// Index a media root file document.
    async fn index_media_root_file(&self, doc: MediaRootFileDocument) -> Result<(), SearchError>;

    /// Index multiple media root file documents in a single batch.
    async fn index_media_root_files_batch(
        &self,
        docs: Vec<MediaRootFileDocument>,
    ) -> Result<(), SearchError>;

    /// Delete a media root file from the index.
    async fn delete_media_root_file(&self, id: Uuid) -> Result<(), SearchError>;

    /// Search media root files scoped to a specific root.
    async fn search_media_root_files(
        &self,
        query: &str,
        media_root_id: Uuid,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<MediaRootFileDocument>, SearchError>;

    /// Index a page document.
    async fn index_page(&self, doc: PageDocument) -> Result<(), SearchError>;

    /// Index a command document.
    async fn index_command(&self, doc: CommandDocument) -> Result<(), SearchError>;

    // =========================================================================
    // IPTV Operations
    // =========================================================================

    /// Search IPTV channels with filters, returning IDs only.
    async fn search_iptv_channels(
        &self,
        query: &str,
        filters: &IptvChannelSearchFilters,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<String>, SearchError>;

    /// Index IPTV channel documents.
    async fn index_iptv_channels(&self, docs: &[IptvChannelDocument]) -> Result<(), SearchError>;

    /// Delete an IPTV channel from the index.
    async fn delete_iptv_channel(&self, id: &str) -> Result<(), SearchError>;

    /// Index IPTV EPG documents.
    async fn index_iptv_epg(&self, docs: &[IptvEpgDocument]) -> Result<(), SearchError>;

    // =========================================================================
    // Live TV Operations
    // =========================================================================

    /// Search Live TV channels with filters, returning IDs only.
    async fn search_livetv_channels(
        &self,
        query: &str,
        filters: &LivetvChannelSearchFilters,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<String>, SearchError>;

    /// Index Live TV channel documents.
    async fn index_livetv_channels(
        &self,
        docs: &[LivetvChannelDocument],
    ) -> Result<(), SearchError>;

    /// Delete a Live TV channel from the index.
    async fn delete_livetv_channel(&self, id: &str) -> Result<(), SearchError>;

    /// Index Live TV guide documents.
    async fn index_livetv_guide(&self, docs: &[LivetvGuideDocument]) -> Result<(), SearchError>;

    /// Search the Live TV guide with filters.
    async fn search_livetv_guide(
        &self,
        query: &str,
        filters: &LivetvGuideSearchFilters,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<LivetvGuideDocument>, SearchError>;

    /// Search IPTV EPG with filters.
    async fn search_iptv_epg(
        &self,
        query: &str,
        filters: &IptvEpgSearchFilters,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<IptvEpgDocument>, SearchError>;

    // =========================================================================
    // Transcript Segment Operations
    // =========================================================================

    /// Search transcript segments with optional book_id filter.
    async fn search_transcript_segments(
        &self,
        query: &str,
        book_id: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<TranscriptSegmentDocument>, SearchError>;

    /// Index transcript segment documents.
    async fn index_transcript_segments(
        &self,
        docs: Vec<TranscriptSegmentDocument>,
    ) -> Result<(), SearchError>;

    /// Delete all transcript segments for a book.
    async fn delete_transcript_segments_for_book(&self, book_id: &str) -> Result<(), SearchError>;

    // =========================================================================
    // Bulk Operations
    // =========================================================================

    /// Reindex all books (clears and rebuilds).
    async fn reindex_all_books(&self, docs: Vec<BookDocument>) -> Result<usize, SearchError>;

    /// Reindex all authors (clears and rebuilds).
    async fn reindex_all_authors(&self, docs: Vec<AuthorDocument>) -> Result<usize, SearchError>;

    /// Reindex all series (clears and rebuilds).
    async fn reindex_all_series(&self, docs: Vec<SeriesDocument>) -> Result<usize, SearchError>;

    /// Reindex all genres (clears and rebuilds).
    async fn reindex_all_genres(&self, docs: Vec<GenreDocument>) -> Result<usize, SearchError>;

    /// Reindex all movies (clears and rebuilds).
    async fn reindex_all_movies(&self, docs: Vec<MovieDocument>) -> Result<usize, SearchError>;

    /// Reindex all TV shows (clears and rebuilds).
    async fn reindex_all_tv_shows(&self, docs: Vec<TvShowDocument>) -> Result<usize, SearchError>;

    /// Reindex all TV seasons (clears and rebuilds).
    async fn reindex_all_tv_seasons(
        &self,
        docs: Vec<TvSeasonDocument>,
    ) -> Result<usize, SearchError>;

    /// Reindex all TV episodes (clears and rebuilds).
    async fn reindex_all_tv_episodes(
        &self,
        docs: Vec<TvEpisodeDocument>,
    ) -> Result<usize, SearchError>;

    /// Reindex all people (clears and rebuilds).
    async fn reindex_all_people(&self, docs: Vec<PersonDocument>) -> Result<usize, SearchError>;

    /// Reindex all media root files (clears and rebuilds).
    async fn reindex_all_media_root_files(
        &self,
        docs: Vec<MediaRootFileDocument>,
    ) -> Result<usize, SearchError>;

    /// Reindex all pages (clears and rebuilds).
    async fn reindex_all_pages(&self, docs: Vec<PageDocument>) -> Result<usize, SearchError>;

    /// Reindex all commands (clears and rebuilds).
    async fn reindex_all_commands(&self, docs: Vec<CommandDocument>) -> Result<usize, SearchError>;

    /// Reindex all settings (clears and rebuilds).
    async fn reindex_all_settings(&self, docs: Vec<SettingDocument>) -> Result<usize, SearchError>;

    /// Reindex all IPTV channels (clears and rebuilds).
    async fn reindex_all_iptv_channels(
        &self,
        docs: Vec<IptvChannelDocument>,
    ) -> Result<usize, SearchError>;

    /// Reindex all IPTV EPG entries (clears and rebuilds).
    async fn reindex_all_iptv_epg(&self, docs: Vec<IptvEpgDocument>) -> Result<usize, SearchError>;

    /// Reindex all Live TV channels (clears and rebuilds).
    async fn reindex_all_livetv_channels(
        &self,
        docs: Vec<LivetvChannelDocument>,
    ) -> Result<usize, SearchError>;

    /// Reindex all Live TV guide entries (clears and rebuilds).
    async fn reindex_all_livetv_guide(
        &self,
        docs: Vec<LivetvGuideDocument>,
    ) -> Result<usize, SearchError>;

    // =========================================================================
    // Initialization
    // =========================================================================

    /// Initialize indexes with proper settings.
    async fn init_indexes(&self) -> Result<(), SearchError>;

    // =========================================================================
    // Status
    // =========================================================================

    /// Get status information about the search backend.
    async fn status(&self) -> SearchBackendStatus;

    // =========================================================================
    // Configuration
    // =========================================================================

    /// Reconfigure the search backend with new connection settings.
    ///
    /// This allows hot-reloading connection settings without restarting the server.
    /// Returns Ok(()) if reconfiguration was successful, or an error if the new
    /// settings are invalid or connection failed.
    async fn reconfigure(&self, url: &str, master_key: &str) -> Result<(), SearchError>;

    // =========================================================================
    // Semantic search
    // =========================================================================

    fn supports_semantic(&self) -> bool {
        false
    }

    /// Whether title documents are written with `_vectors`.
    fn semantic_enabled(&self) -> bool {
        false
    }

    fn set_semantic_enabled(&self, _enabled: bool) {}

    /// Whether the title indexes carry exactly the current embedders.
    async fn embedders_configured(&self) -> Result<bool, SearchError> {
        Ok(false)
    }

    /// Whether any title index carries any embedder at all, current or not.
    async fn has_embedders(&self) -> Result<bool, SearchError> {
        Ok(false)
    }

    /// Declares or removes the summary and artwork embedders on the title indexes
    /// and waits for Meili to accept them. Declaring fails while any document
    /// lacks `_vectors`.
    async fn apply_embedders(&self, _enabled: bool) -> Result<(), SearchError> {
        Ok(())
    }
}

// =============================================================================
// SQL Fallback Backend
// =============================================================================

/// The search backend used when Meilisearch is not configured.
///
/// This is a production path, not a test double: kobbler runs on Postgres and
/// NATS alone, and every search route already handles a backend error by
/// running an ILIKE query instead. So the entity searches, letter lists and
/// facet lookups here deliberately return `SearchError`, which is the signal
/// those handlers use to take the SQL path.
///
/// Indexing is a no-op rather than an error: there is no index to maintain, and
/// write paths (including NATS jobs) must not fail just because search is
/// running without Meilisearch.
pub struct SqlFallbackBackend;

impl SqlFallbackBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SqlFallbackBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SearchBackend for SqlFallbackBackend {
    async fn unified_search(
        &self,
        _query: &str,
        _limit: usize,
        _semantic: Option<&SemanticQuery>,
    ) -> Result<UnifiedSearchResponse, SearchError> {
        // Return empty results for federated search
        Ok(UnifiedSearchResponse {
            top_hits: vec![],
            remaining: vec![],
        })
    }

    async fn search(
        &self,
        _query: &str,
        _limit: usize,
    ) -> Result<UnifiedSearchResult, SearchError> {
        // Return empty results for unified search (command palette tests may need this)
        Ok(UnifiedSearchResult {
            books: vec![],
            authors: vec![],
            series: vec![],
            genres: vec![],
        })
    }

    async fn search_books(
        &self,
        _query: &str,
        _filters: BookSearchFilters,
        _limit: usize,
        _offset: usize,
    ) -> Result<Vec<Uuid>, SearchError> {
        // Return error to trigger ILIKE fallback in route handlers
        Err(SearchError::Backend(
            "Meilisearch not configured: falling back to ILIKE".to_string(),
        ))
    }

    async fn search_authors(
        &self,
        _query: &str,
        _filters: AuthorSearchFilters,
        _limit: usize,
        _offset: usize,
    ) -> Result<Vec<Uuid>, SearchError> {
        Err(SearchError::Backend(
            "Meilisearch not configured: falling back to ILIKE".to_string(),
        ))
    }

    async fn search_series(
        &self,
        _query: &str,
        _filters: SeriesSearchFilters,
        _limit: usize,
        _offset: usize,
    ) -> Result<Vec<Uuid>, SearchError> {
        Err(SearchError::Backend(
            "Meilisearch not configured: falling back to ILIKE".to_string(),
        ))
    }

    async fn search_movies(
        &self,
        _query: &str,
        _filters: MovieSearchFilters,
        _limit: usize,
        _offset: usize,
    ) -> Result<Vec<Uuid>, SearchError> {
        Err(SearchError::Backend(
            "Meilisearch not configured: falling back to ILIKE".to_string(),
        ))
    }

    async fn search_tv_shows(
        &self,
        _query: &str,
        _filters: TvShowSearchFilters,
        _limit: usize,
        _offset: usize,
    ) -> Result<Vec<Uuid>, SearchError> {
        Err(SearchError::Backend(
            "Meilisearch not configured: falling back to ILIKE".to_string(),
        ))
    }

    async fn get_available_book_letters(
        &self,
        _filters: BookSearchFilters,
    ) -> Result<Vec<String>, SearchError> {
        // Return error to trigger SQL fallback
        Err(SearchError::Backend(
            "Meilisearch not configured: falling back to SQL".to_string(),
        ))
    }

    async fn get_available_movie_letters(
        &self,
        _filters: MovieSearchFilters,
    ) -> Result<Vec<String>, SearchError> {
        Err(SearchError::Backend(
            "Meilisearch not configured: falling back to SQL".to_string(),
        ))
    }

    async fn get_available_tv_show_letters(
        &self,
        _filters: TvShowSearchFilters,
    ) -> Result<Vec<String>, SearchError> {
        Err(SearchError::Backend(
            "Meilisearch not configured: falling back to SQL".to_string(),
        ))
    }

    async fn get_book_facets(
        &self,
        _filters: BookSearchFilters,
    ) -> Result<LibraryFacets, SearchError> {
        Err(SearchError::Backend(
            "Meilisearch not configured: falling back to SQL".to_string(),
        ))
    }

    async fn get_movie_facets(
        &self,
        _filters: MovieSearchFilters,
    ) -> Result<LibraryFacets, SearchError> {
        Err(SearchError::Backend(
            "Meilisearch not configured: falling back to SQL".to_string(),
        ))
    }

    async fn get_tv_show_facets(
        &self,
        _filters: TvShowSearchFilters,
    ) -> Result<LibraryFacets, SearchError> {
        Err(SearchError::Backend(
            "Meilisearch not configured: falling back to SQL".to_string(),
        ))
    }

    async fn index_book(&self, _doc: BookDocument) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn index_author(&self, _doc: AuthorDocument) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn index_series(&self, _doc: SeriesDocument) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn delete_book(&self, _id: Uuid) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn delete_author(&self, _id: Uuid) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn delete_series(&self, _id: Uuid) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn reindex_all_books(&self, docs: Vec<BookDocument>) -> Result<usize, SearchError> {
        Ok(docs.len()) // Pretend we indexed them
    }

    async fn reindex_all_authors(&self, docs: Vec<AuthorDocument>) -> Result<usize, SearchError> {
        Ok(docs.len())
    }

    async fn reindex_all_series(&self, docs: Vec<SeriesDocument>) -> Result<usize, SearchError> {
        Ok(docs.len())
    }

    async fn index_genre(&self, _doc: GenreDocument) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn delete_genre(&self, _id: Uuid) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn reindex_all_genres(&self, docs: Vec<GenreDocument>) -> Result<usize, SearchError> {
        Ok(docs.len())
    }

    async fn search_people(
        &self,
        _query: &str,
        _limit: usize,
        _offset: usize,
    ) -> Result<Vec<Uuid>, SearchError> {
        Err(SearchError::Backend(
            "Meilisearch not configured: falling back to ILIKE".to_string(),
        ))
    }

    async fn index_movie(&self, _doc: MovieDocument) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn index_tv_show(&self, _doc: TvShowDocument) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn delete_movie(&self, _id: Uuid) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn delete_tv_show(&self, _id: Uuid) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn reindex_all_movies(&self, docs: Vec<MovieDocument>) -> Result<usize, SearchError> {
        Ok(docs.len()) // Pretend we indexed them
    }

    async fn reindex_all_tv_shows(&self, docs: Vec<TvShowDocument>) -> Result<usize, SearchError> {
        Ok(docs.len()) // Pretend we indexed them
    }

    async fn index_tv_season(&self, _doc: TvSeasonDocument) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn index_tv_episode(&self, _doc: TvEpisodeDocument) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn index_tv_seasons_batch(
        &self,
        _docs: Vec<TvSeasonDocument>,
    ) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn index_tv_episodes_batch(
        &self,
        _docs: Vec<TvEpisodeDocument>,
    ) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn delete_tv_season(&self, _id: Uuid) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn delete_tv_episode(&self, _id: Uuid) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn reindex_all_tv_seasons(
        &self,
        docs: Vec<TvSeasonDocument>,
    ) -> Result<usize, SearchError> {
        Ok(docs.len())
    }

    async fn reindex_all_tv_episodes(
        &self,
        docs: Vec<TvEpisodeDocument>,
    ) -> Result<usize, SearchError> {
        Ok(docs.len())
    }

    async fn index_person(&self, _doc: PersonDocument) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn delete_person(&self, _id: Uuid) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn reindex_all_people(&self, docs: Vec<PersonDocument>) -> Result<usize, SearchError> {
        Ok(docs.len())
    }

    async fn index_media_root_file(&self, _doc: MediaRootFileDocument) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn index_media_root_files_batch(
        &self,
        _docs: Vec<MediaRootFileDocument>,
    ) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn delete_media_root_file(&self, _id: Uuid) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn search_media_root_files(
        &self,
        _query: &str,
        _media_root_id: Uuid,
        _limit: usize,
        _offset: usize,
    ) -> Result<Vec<MediaRootFileDocument>, SearchError> {
        Ok(vec![])
    }

    async fn reindex_all_media_root_files(
        &self,
        docs: Vec<MediaRootFileDocument>,
    ) -> Result<usize, SearchError> {
        Ok(docs.len())
    }

    async fn index_page(&self, _doc: PageDocument) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn index_command(&self, _doc: CommandDocument) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn reindex_all_pages(&self, docs: Vec<PageDocument>) -> Result<usize, SearchError> {
        Ok(docs.len())
    }

    async fn reindex_all_commands(&self, docs: Vec<CommandDocument>) -> Result<usize, SearchError> {
        Ok(docs.len())
    }

    async fn reindex_all_settings(&self, docs: Vec<SettingDocument>) -> Result<usize, SearchError> {
        Ok(docs.len())
    }

    async fn reindex_all_iptv_channels(
        &self,
        docs: Vec<IptvChannelDocument>,
    ) -> Result<usize, SearchError> {
        Ok(docs.len())
    }

    async fn reindex_all_iptv_epg(&self, docs: Vec<IptvEpgDocument>) -> Result<usize, SearchError> {
        Ok(docs.len())
    }

    async fn reindex_all_livetv_channels(
        &self,
        docs: Vec<LivetvChannelDocument>,
    ) -> Result<usize, SearchError> {
        Ok(docs.len())
    }

    async fn reindex_all_livetv_guide(
        &self,
        docs: Vec<LivetvGuideDocument>,
    ) -> Result<usize, SearchError> {
        Ok(docs.len())
    }

    async fn search_transcript_segments(
        &self,
        _query: &str,
        _book_id: Option<Uuid>,
        _limit: usize,
    ) -> Result<Vec<TranscriptSegmentDocument>, SearchError> {
        Ok(vec![])
    }

    async fn index_transcript_segments(
        &self,
        _docs: Vec<TranscriptSegmentDocument>,
    ) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn delete_transcript_segments_for_book(&self, _book_id: &str) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn search_iptv_channels(
        &self,
        _query: &str,
        _filters: &IptvChannelSearchFilters,
        _limit: usize,
        _offset: usize,
    ) -> Result<Vec<String>, SearchError> {
        Ok(vec![])
    }

    async fn index_iptv_channels(&self, _docs: &[IptvChannelDocument]) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn delete_iptv_channel(&self, _id: &str) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn index_iptv_epg(&self, _docs: &[IptvEpgDocument]) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn search_iptv_epg(
        &self,
        _query: &str,
        _filters: &IptvEpgSearchFilters,
        _limit: usize,
        _offset: usize,
    ) -> Result<Vec<IptvEpgDocument>, SearchError> {
        Ok(vec![])
    }

    async fn search_livetv_channels(
        &self,
        _query: &str,
        _filters: &LivetvChannelSearchFilters,
        _limit: usize,
        _offset: usize,
    ) -> Result<Vec<String>, SearchError> {
        Ok(vec![])
    }

    async fn index_livetv_channels(
        &self,
        _docs: &[LivetvChannelDocument],
    ) -> Result<(), SearchError> {
        Ok(())
    }

    async fn delete_livetv_channel(&self, _id: &str) -> Result<(), SearchError> {
        Ok(())
    }

    async fn index_livetv_guide(&self, _docs: &[LivetvGuideDocument]) -> Result<(), SearchError> {
        Ok(())
    }

    async fn search_livetv_guide(
        &self,
        _query: &str,
        _filters: &LivetvGuideSearchFilters,
        _limit: usize,
        _offset: usize,
    ) -> Result<Vec<LivetvGuideDocument>, SearchError> {
        Ok(vec![])
    }

    async fn init_indexes(&self) -> Result<(), SearchError> {
        Ok(()) // No-op
    }

    async fn status(&self) -> SearchBackendStatus {
        SearchBackendStatus {
            backend_type: "sql_fallback".to_string(),
            connected: true,
            url: "(Meilisearch not configured)".to_string(),
            book_count: None,
            author_count: None,
            series_count: None,
            genre_count: None,
            movie_count: None,
            tv_show_count: None,
            tv_season_count: None,
            tv_episode_count: None,
            person_count: None,
            embedded_document_count: None,
            error: None,
        }
    }

    async fn reconfigure(&self, _url: &str, _master_key: &str) -> Result<(), SearchError> {
        Ok(())
    }
}
