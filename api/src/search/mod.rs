//! Search integration with pluggable backends (currently MeiliSearch).

mod backend;
pub mod chat;
mod indexing;
mod meili;
mod selection;

// Re-export the trait and filter types
pub use backend::{
    AuthorSearchFilters, BookSearchFilters, FacetBucket, IptvChannelSearchFilters,
    IptvEpgSearchFilters, LibraryFacets, LivetvChannelSearchFilters, LivetvGuideSearchFilters,
    MovieSearchFilters, PersonSearchFilters, SearchBackend, SearchBackendStatus, SearchError,
    SeriesSearchFilters, SqlFallbackBackend, TvShowSearchFilters,
};

pub use selection::SearchBackendChoice;

// Re-export the MeiliSearch implementation
pub use meili::MeiliState;

// Re-export indexing functions (they work with any SearchBackend)
pub use indexing::{
    batch_index_media_root_files, fetch_all_author_docs, fetch_all_book_docs, fetch_all_genre_docs,
    fetch_all_iptv_channel_docs, fetch_all_iptv_epg_docs, fetch_all_livetv_channel_docs,
    fetch_all_livetv_guide_docs, fetch_all_media_root_file_docs, fetch_all_movie_docs,
    fetch_all_people_docs, fetch_all_series_docs, fetch_all_tv_episode_docs,
    fetch_all_tv_season_docs, fetch_all_tv_show_docs, index_author, index_book, index_iptv_channel,
    index_livetv_channel, index_media_root_file, index_movie, index_person, index_series,
    index_transcript_segments_for_book, index_tv_episode, index_tv_season, index_tv_show,
    index_tv_show_children,
};

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

/// Current search schema version.
///
/// Bump this when making changes to index settings (searchable fields, filterable
/// attributes, etc.) that require a full reindex. The startup check will detect
/// version mismatches and trigger an automatic reindex.
///
/// Version history:
/// - 1: Initial federated search with pages/commands indexes (MeiliSearch 1.33)
/// - 2: Added first_letter field to books for alphabet bar faceting
/// - 3: Added sortable attributes (title, series_name, series_index_float) for series ordering
/// - 4: Custom ranking rules with sort last (tie-breaker after relevance)
/// - 5: Added ingest_status field for unified books/ingest queue model
/// - 6: Added ingest_status as filterable attribute for library page filtering
/// - 7: Renamed sortable attribute series_index_float to series_index
/// - 8: Added movies and tv_shows indexes for movie/TV search integration
/// - 9: Added people index for person search
/// - 10: Added media_root_files index for in-page file search
/// - 11: Meilisearch 1.36 split ranking rules (attributeRank + wordPosition), federated search
/// - 12: Added Discover page to static pages
/// - 13: Added Sports page to static pages
/// - 14: has_audiobook subquery switched to audiobook_files table
/// - 15: Added transcript_segments index with HuggingFace embedder for hybrid semantic search
/// - 17: Added Voice Presets page to static pages
/// - 18: Added Calendar page to static pages
/// - 19: media_root_files: stricter matching (matching_strategy=ALL, tighter typo tolerance)
/// - 20: Added watch/reading state fields (watched, rating, episodes_watched, total_episodes, reading_progress)
/// - 21: Removed Audiobooks static page (replaced by global format filter)
/// - 22: Added has_ebook filterable field to book documents
/// - 23: Removed transcript_segments embeddings and sliding window documents
/// - 24: Split Jobs page: /jobs (history) + /settings/jobs (concurrency)
/// - 25: book_series junction table: series lookups via book_series, all_series_names searchable field
/// - 26: IPTV nav restructure: Live TV Home, Channels, Recordings pages
/// - 27: Add "&" / "and" synonyms to all indexes
/// - 28: Remove transcript_segments from unified search
/// - 29: Add genres as filterable on books, movies, tv_shows
/// - 30: Add settings index for field-level settings search
/// - 31: Declare "&" as non-separation token so "and"/"&" synonyms match
/// - 32: description/runtime/content_rating on movies, shows and books; air_date +
///   sort orders on episodes; year/rating/runtime/content_rating filterable. Dropped
///   `watched` and `episodes_watched`, which were read from an arbitrary user's row.
/// - 33: Dropped `reading_progress` (same per-user bug as v32's `watched`); made the
///   IPTV group pick deterministic and enabled-only; ordered the directors aggregate.
/// - 34: `id` sortable on books, movies and tv_shows, so the chatbot's `recommend`
///   shortlist can end every sort on `id:asc`. Tied ratings previously fell back to
///   Meili's internal document order, which does not survive a reindex.
/// - 36: sort_title backfilled for movies and TV shows (migration 0171). The
///   indexed `sort_title`/`first_letter` come from `COALESCE(sort_title, title)`,
///   so every video document had the leading article baked into its sort key.
/// - 37: Added livetv_channels and livetv_guide indexes plus the Live TV static
///   pages. Landed alongside 36 rather than after it, so a server that already
///   reindexed for sort_title still needs one for the tuner documents.
/// - 39: Added the Stremio History static page.
/// - 40: Added the Book Title Suggestions setting.
/// - 41: Movie and TV show `rating` on its providers' 0-10 scale instead of halved to 0-5.
pub const SEARCH_SCHEMA_VERSION: i32 = 41;

/// Result of checking if search reindex is needed on startup.
#[derive(Debug)]
pub struct ReindexCheck {
    /// Whether a reindex is needed.
    pub needs_reindex: bool,
    /// Reason for needing reindex (if any).
    pub reason: Option<String>,
}

/// Check if search indexes need to be rebuilt on startup.
///
/// Returns true if:
/// - Schema version in DB doesn't match current SEARCH_SCHEMA_VERSION
/// - MeiliSearch indexes are empty but DB has entities
pub async fn check_reindex_needed(pool: &PgPool, search: &dyn SearchBackend) -> ReindexCheck {
    use crate::repositories::search_meta;

    // Check schema version
    let stored_version = search_meta::get_schema_version(pool).await.unwrap_or(0);
    if stored_version != SEARCH_SCHEMA_VERSION {
        return ReindexCheck {
            needs_reindex: true,
            reason: Some(format!(
                "Schema version mismatch: stored={}, current={}",
                stored_version, SEARCH_SCHEMA_VERSION
            )),
        };
    }

    // Check if MeiliSearch is empty but DB has data
    let status = search.status().await;

    // If not connected, can't check
    if !status.connected {
        return ReindexCheck {
            needs_reindex: false,
            reason: None,
        };
    }

    let meili_book_count = status.book_count.unwrap_or(0);
    let meili_author_count = status.author_count.unwrap_or(0);

    // Query DB for counts
    let db_book_count: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) as "count!" FROM books"#)
        .fetch_one(pool)
        .await
        .unwrap_or(0);

    let db_author_count: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) as "count!" FROM authors"#)
        .fetch_one(pool)
        .await
        .unwrap_or(0);

    // If DB has entities but MeiliSearch is empty, need reindex
    if db_book_count > 0 && meili_book_count == 0 {
        return ReindexCheck {
            needs_reindex: true,
            reason: Some(format!(
                "MeiliSearch books index empty but DB has {} books",
                db_book_count
            )),
        };
    }

    if db_author_count > 0 && meili_author_count == 0 {
        return ReindexCheck {
            needs_reindex: true,
            reason: Some(format!(
                "MeiliSearch authors index empty but DB has {} authors",
                db_author_count
            )),
        };
    }

    let meili_movie_count = status.movie_count.unwrap_or(0);
    let meili_tv_show_count = status.tv_show_count.unwrap_or(0);

    let db_movie_count: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) as "count!" FROM movies"#)
        .fetch_one(pool)
        .await
        .unwrap_or(0);

    let db_tv_show_count: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) as "count!" FROM tv_shows"#)
        .fetch_one(pool)
        .await
        .unwrap_or(0);

    if db_movie_count > 0 && meili_movie_count == 0 {
        return ReindexCheck {
            needs_reindex: true,
            reason: Some(format!(
                "MeiliSearch movies index empty but DB has {} movies",
                db_movie_count
            )),
        };
    }

    if db_tv_show_count > 0 && meili_tv_show_count == 0 {
        return ReindexCheck {
            needs_reindex: true,
            reason: Some(format!(
                "MeiliSearch tv_shows index empty but DB has {} TV shows",
                db_tv_show_count
            )),
        };
    }

    ReindexCheck {
        needs_reindex: false,
        reason: None,
    }
}

/// Index names used by MeiliSearch (and potentially other backends).
pub mod indexes {
    pub const BOOKS: &str = "books";
    pub const AUTHORS: &str = "authors";
    pub const SERIES: &str = "series";
    pub const GENRES: &str = "genres";
    pub const PAGES: &str = "pages";
    pub const COMMANDS: &str = "commands";
    pub const MOVIES: &str = "movies";
    pub const TV_SHOWS: &str = "tv_shows";
    pub const PEOPLE: &str = "people";
    pub const MEDIA_ROOT_FILES: &str = "media_root_files";
    pub const TV_SEASONS: &str = "tv_seasons";
    pub const TV_EPISODES: &str = "tv_episodes";
    pub const TRANSCRIPT_SEGMENTS: &str = "transcript_segments";
    pub const IPTV_CHANNELS: &str = "iptv_channels";
    pub const IPTV_EPG: &str = "iptv_epg";
    pub const LIVETV_CHANNELS: &str = "livetv_channels";
    pub const LIVETV_GUIDE: &str = "livetv_guide";
    pub const SETTINGS: &str = "settings";
}

/// Compute the first letter for alphabet bar from a title.
///
/// Strips common articles (The, A, An) and returns uppercase A-Z or "#" for non-alphabetic.
/// Synopses are for the model to reason over a shortlist, not to read aloud. Long
/// enough to tell two films apart, short enough that thirty of them fit in a prompt.
pub const MAX_DESCRIPTION_CHARS: usize = 400;

pub fn truncate_description(text: Option<String>) -> Option<String> {
    let text = text?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().count() <= MAX_DESCRIPTION_CHARS {
        return Some(trimmed.to_string());
    }
    let cut: String = trimmed.chars().take(MAX_DESCRIPTION_CHARS).collect();
    let cut = match cut.rfind(' ') {
        Some(i) if i > MAX_DESCRIPTION_CHARS / 2 => cut[..i].to_string(),
        _ => cut,
    };
    Some(format!("{}…", cut.trim_end()))
}

pub fn compute_first_letter(title: &str) -> String {
    let trimmed = title.trim();

    // Strip common articles (case-insensitive)
    let sort_title = if trimmed.len() > 4 && trimmed.as_bytes()[..4].eq_ignore_ascii_case(b"the ") {
        &trimmed[4..]
    } else if trimmed.len() > 2 && trimmed.as_bytes()[..2].eq_ignore_ascii_case(b"a ") {
        &trimmed[2..]
    } else if trimmed.len() > 3 && trimmed.as_bytes()[..3].eq_ignore_ascii_case(b"an ") {
        &trimmed[3..]
    } else {
        trimmed
    };

    // Get first character
    if let Some(ch) = sort_title.chars().next() {
        if ch.is_ascii_alphabetic() {
            ch.to_ascii_uppercase().to_string()
        } else {
            "#".to_string()
        }
    } else {
        "#".to_string()
    }
}

/// Book document for search indexing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookDocument {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    /// Author UUIDs for searching books by author ID.
    pub author_ids: Vec<String>,
    pub series_name: Option<String>,
    /// Series UUID for searching books by series ID.
    pub series_id: Option<String>,
    /// Series index for sorting within series.
    pub series_index: Option<f64>,
    /// Genre names for searchability.
    pub genres: Vec<String>,
    pub cover_url: Option<String>,
    pub is_in_library: bool,
    pub want_status: String,
    /// First letter of title (A-Z or # for non-alphabetic) for alphabet bar faceting.
    pub first_letter: String,
    /// Ingest status: None for library books, Some("pending"|"extracting"|...) for queue items.
    pub ingest_status: Option<String>,
    /// Whether this book has audiobook files attached.
    pub has_audiobook: bool,
    /// Whether this book has ebook files attached.
    pub has_ebook: bool,
    /// Primary narrator name for audiobooks.
    pub narrator: Option<String>,
    /// All series names (pipe-separated) for search discoverability.
    pub all_series_names: Option<String>,
    /// Stored, not searchable. See `MovieDocument::description`.
    pub description: Option<String>,
}

impl BookDocument {
    /// Compute the first letter for alphabet bar from a title.
    ///
    /// Delegates to the free function [`compute_first_letter`].
    pub fn compute_first_letter(title: &str) -> String {
        compute_first_letter(title)
    }
}

/// Author document for search indexing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorDocument {
    pub id: String,
    pub name: String,
    pub image_url: Option<String>,
    pub book_count: i64,
    pub is_monitored: bool,
}

/// Series document for search indexing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesDocument {
    pub id: String,
    pub name: String,
    pub book_count: i64,
    pub is_monitored: bool,
}

/// Genre document for search indexing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenreDocument {
    pub id: String,
    pub name: String,
    pub book_count: i64,
}

/// Page document for search indexing (static navigation pages).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageDocument {
    pub id: String,
    pub label: String,
    pub path: String,
    pub aliases: Vec<String>,
    pub icon: String,
}

/// Command document for search indexing (static commands).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandDocument {
    pub id: String,
    pub label: String,
    pub action: String,
    pub aliases: Vec<String>,
    pub icon: String,
}

/// Setting document for search indexing (individual config fields).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingDocument {
    pub id: String,
    pub field_name: String,
    pub label: String,
    pub description: String,
    pub page_path: String,
    pub page_label: String,
    pub aliases: Vec<String>,
}

/// Movie document for search indexing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovieDocument {
    pub id: String,
    pub title: String,
    pub year: Option<i32>,
    pub directors: Vec<String>,
    pub genres: Vec<String>,
    pub poster_url: Option<String>,
    pub is_in_library: bool,
    pub want_status: String,
    pub first_letter: String,
    /// The film's own rating from the metadata provider — NOT anyone's personal rating.
    pub rating: Option<f64>,
    pub runtime: Option<i32>,
    pub content_rating: Option<String>,
    /// Stored, not searchable: keyword matching over synopses is noise, but a chat
    /// shortlist is unusable without the text to reason over.
    pub description: Option<String>,
}

/// TV show document for search indexing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TvShowDocument {
    pub id: String,
    pub title: String,
    pub year: Option<i32>,
    pub genres: Vec<String>,
    pub poster_url: Option<String>,
    pub is_monitored: bool,
    pub status: Option<String>,
    pub network: Option<String>,
    pub first_letter: String,
    pub total_episodes: i32,
    /// Episodes with a file, so "can we start this tonight" is answerable without a
    /// second query. `is_in_library` on a show says nothing about how much of it exists.
    pub available_episode_count: i32,
    pub rating: Option<f64>,
    pub content_rating: Option<String>,
    pub runtime: Option<i32>,
    pub description: Option<String>,
}

/// TV season document for search indexing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TvSeasonDocument {
    pub id: String,
    pub show_id: String,
    pub show_title: String,
    /// Composite title including show name, e.g. "DTF St Louis Season 2".
    pub display_title: String,
    /// The season's own title if any.
    pub season_title: Option<String>,
    pub season_identifier: String,
    /// Padded season code for search, e.g. "S02".
    pub season_code: String,
    pub poster_url: Option<String>,
    pub episode_count: i32,
    pub is_monitored: bool,
    pub is_special: bool,
}

/// TV episode document for search indexing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TvEpisodeDocument {
    pub id: String,
    pub show_id: String,
    pub season_id: String,
    pub show_title: String,
    /// Composite title including show name and episode code, e.g. "DTF St Louis S02E05 The Draft".
    pub display_title: String,
    /// The episode's own title if any.
    pub episode_title: Option<String>,
    /// Padded episode code for search, e.g. "S02E05".
    pub episode_code: String,
    pub still_url: Option<String>,
    pub is_in_library: bool,
    pub want_status: String,
    /// ISO date, for display.
    pub air_date: Option<String>,
    /// Unix seconds. Meili range filters and sorts want a number, and "last night's
    /// episode" / "the latest one" are the whole reason this document is searched.
    pub air_date_ts: Option<i64>,
    pub season_sort_order: i32,
    pub episode_sort_order: i32,
    pub runtime: Option<i32>,
    pub description: Option<String>,
}

/// Person document for search indexing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonDocument {
    pub id: String,
    pub name: String,
    pub sort_name: Option<String>,
    pub image_url: Option<String>,
    pub bio: Option<String>,
    pub movie_count: i64,
    pub tv_show_count: i64,
}

/// Media root file document for search indexing (scoped file search, not unified search).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRootFileDocument {
    pub id: String,
    pub file_name: String,
    pub file_path: String,
    pub media_root_id: String,
    pub media_root_name: String,
    pub match_status: String,
    pub entity_type: Option<String>,
    pub show_name: Option<String>,
    pub movie_title: Option<String>,
    pub book_title: Option<String>,
    pub series_name: Option<String>,
}

/// Transcript segment document for search indexing (audiobook transcripts and ebook text).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegmentDocument {
    pub id: String,
    pub book_id: String,
    /// "audio" or "ebook"
    pub source_type: String,
    pub file_id: String,
    pub segment_index: i32,
    pub start_seconds: Option<f64>,
    pub cfi: Option<String>,
    pub text: String,
    pub book_title: String,
    pub chapter_title: Option<String>,
}

/// IPTV channel document for search indexing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IptvChannelDocument {
    pub id: String,
    pub name: String,
    pub category: Option<String>,
    pub group: Option<String>,
    pub language: Option<String>,
    pub country: Option<String>,
    pub icon_url: Option<String>,
    pub source_count: i32,
    pub has_epg: bool,
    pub current_program: Option<String>,
    pub enabled: bool,
}

/// IPTV EPG (Electronic Program Guide) document for search indexing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IptvEpgDocument {
    pub id: String,
    pub channel_id: String,
    pub channel_name: String,
    pub title: String,
    pub description: Option<String>,
    pub categories: Vec<String>,
    pub start_time: i64,
    pub stop_time: i64,
    pub start_time_iso: String,
    pub stop_time_iso: String,
    pub channel_icon_url: Option<String>,
    pub is_live: bool,
}

/// Live TV (HDHomeRun tuner) channel document for search indexing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LivetvChannelDocument {
    pub id: String,
    pub name: String,
    pub scan_name: Option<String>,
    pub callsign: Option<String>,
    pub network: Option<String>,
    pub region: Option<String>,
    pub kind: String,
    pub lcn: Option<i32>,
    pub mux_label: String,
    pub type_label: String,
    pub hd: bool,
    pub enabled: bool,
    pub favourite: bool,
    pub has_guide: bool,
    pub logo_url: Option<String>,
    pub current_program: Option<String>,
}

/// Live TV guide (programme) document for search indexing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LivetvGuideDocument {
    pub id: String,
    pub channel_id: String,
    pub channel_name: String,
    pub title: String,
    pub episode_title: Option<String>,
    pub synopsis: Option<String>,
    pub genres: Vec<String>,
    pub start_time: i64,
    pub end_time: i64,
    pub start_time_iso: String,
    pub end_time_iso: String,
    pub series_id: Option<String>,
    pub image_url: Option<String>,
    pub is_live: bool,
}

/// A single search hit with type discrimination and ranking score.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SearchHit {
    #[serde(rename = "book")]
    Book(BookDocument),
    #[serde(rename = "author")]
    Author(AuthorDocument),
    #[serde(rename = "series")]
    Series(SeriesDocument),
    #[serde(rename = "genre")]
    Genre(GenreDocument),
    #[serde(rename = "page")]
    Page(PageDocument),
    #[serde(rename = "command")]
    Command(CommandDocument),
    #[serde(rename = "movie")]
    Movie(MovieDocument),
    #[serde(rename = "tv_show")]
    TvShow(TvShowDocument),
    #[serde(rename = "person")]
    Person(PersonDocument),
    #[serde(rename = "tv_season")]
    TvSeason(TvSeasonDocument),
    #[serde(rename = "tv_episode")]
    TvEpisode(TvEpisodeDocument),
    #[serde(rename = "transcript")]
    Transcript(TranscriptSegmentDocument),
    #[serde(rename = "iptv_channel")]
    IptvChannel(IptvChannelDocument),
    #[serde(rename = "iptv_epg")]
    IptvEpg(IptvEpgDocument),
    #[serde(rename = "livetv_channel")]
    LivetvChannel(LivetvChannelDocument),
    #[serde(rename = "livetv_guide")]
    LivetvGuide(LivetvGuideDocument),
    #[serde(rename = "setting")]
    Setting(SettingDocument),
}

impl SearchHit {
    /// Get the type name for grouping.
    ///
    /// Books with `ingest_status` set return "ingest" so they get their own
    /// slot in `top_hits` (separate from library books).
    pub fn type_name(&self) -> &'static str {
        match self {
            SearchHit::Book(doc) => {
                if doc.ingest_status.is_some() {
                    "ingest"
                } else {
                    "book"
                }
            }
            SearchHit::Author(_) => "author",
            SearchHit::Series(_) => "series",
            SearchHit::Genre(_) => "genre",
            SearchHit::Page(_) => "page",
            SearchHit::Command(_) => "command",
            SearchHit::Movie(_) => "movie",
            SearchHit::TvShow(_) => "tv_show",
            SearchHit::Person(_) => "person",
            SearchHit::TvSeason(_) => "tv_season",
            SearchHit::TvEpisode(_) => "tv_episode",
            SearchHit::Transcript(_) => "transcript",
            SearchHit::IptvChannel(_) => "iptv_channel",
            SearchHit::IptvEpg(_) => "iptv_epg",
            SearchHit::LivetvChannel(_) => "livetv_channel",
            SearchHit::LivetvGuide(_) => "livetv_guide",
            SearchHit::Setting(_) => "setting",
        }
    }
}

/// Unified search response with hybrid format.
///
/// Results are ranked by relevance score from MeiliSearch's federated search.
/// `top_hits` contains the best hit of each type (ordered by relevance, not fixed type order).
/// `remaining` contains all other results in relevance order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedSearchResponse {
    /// Best hit per type, ordered by relevance score.
    /// e.g., if searching "Brandon Sanderson", this might be [Author, Series, Book]
    /// but if searching "The Way of Kings", this might be [Book, Series, Author]
    pub top_hits: Vec<SearchHit>,
    /// Remaining results in relevance order (excludes top hits).
    pub remaining: Vec<SearchHit>,
}

/// Legacy unified search result (kept for backward compatibility during migration).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedSearchResult {
    pub books: Vec<BookDocument>,
    pub authors: Vec<AuthorDocument>,
    pub series: Vec<SeriesDocument>,
    pub genres: Vec<GenreDocument>,
}

#[cfg(test)]
mod tests {
    use super::BookDocument;

    #[test]
    fn compute_first_letter_strips_articles() {
        let cases = [
            ("The Hobbit", "H"),
            ("A Game", "G"),
            ("An Example", "E"),
            ("the lower", "L"),
            ("a lower", "L"),
            ("an lower", "L"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                BookDocument::compute_first_letter(input),
                expected,
                "input: {input}"
            );
        }
    }

    #[test]
    fn compute_first_letter_non_ascii() {
        let cases = [
            ("Злочестие", "#"),
            ("the Злочестие", "#"),
            ("日本語タイトル", "#"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                BookDocument::compute_first_letter(input),
                expected,
                "input: {input}"
            );
        }
    }

    #[test]
    fn compute_first_letter_edge_cases() {
        let cases = [
            ("", "#"),
            ("   ", "#"),
            ("123 Numbers", "#"),
            ("A", "A"),   // "A" alone, no space after — kept as-is
            ("An", "A"),  // "An" alone — not stripped, starts with 'A'
            ("The", "T"), // "The" alone — not stripped, starts with 'T'
        ];
        for (input, expected) in cases {
            assert_eq!(
                BookDocument::compute_first_letter(input),
                expected,
                "input: {input}"
            );
        }
    }
}
