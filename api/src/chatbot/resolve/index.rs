//! One HTTP round trip across every library index.
//!
//! The resolver fans out on nearly every turn, so this is Meili's multi-search
//! rather than five sequential queries, with `attributesToRetrieve` set
//! explicitly — poster URLs and first-letter facets are pure prompt tax.

use async_trait::async_trait;
use serde::de::DeserializeOwned;

use crate::search::{
    AuthorDocument, BookDocument, MovieDocument, SeriesDocument, TvEpisodeDocument,
    TvSeasonDocument, TvShowDocument, indexes,
};

use super::types::ResolveError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IndexKind {
    Books,
    Movies,
    TvShows,
    TvSeasons,
    TvEpisodes,
    Authors,
    Series,
}

impl IndexKind {
    pub fn uid(self) -> &'static str {
        match self {
            IndexKind::Books => indexes::BOOKS,
            IndexKind::Movies => indexes::MOVIES,
            IndexKind::TvShows => indexes::TV_SHOWS,
            IndexKind::TvSeasons => indexes::TV_SEASONS,
            IndexKind::TvEpisodes => indexes::TV_EPISODES,
            IndexKind::Authors => indexes::AUTHORS,
            IndexKind::Series => indexes::SERIES,
        }
    }

    /// Only what the ranker and the prompt actually read.
    pub fn attributes(self) -> &'static [&'static str] {
        match self {
            IndexKind::Books => &[
                "id",
                "title",
                "authors",
                "author_ids",
                "series_name",
                "series_id",
                "series_index",
                "genres",
                "is_in_library",
                "want_status",
                "has_audiobook",
                "has_ebook",
                "narrator",
                "description",
            ],
            IndexKind::Movies => &[
                "id",
                "title",
                "year",
                "directors",
                "genres",
                "is_in_library",
                "want_status",
                "rating",
                "runtime",
                "content_rating",
                "description",
            ],
            IndexKind::TvShows => &[
                "id",
                "title",
                "year",
                "genres",
                "is_monitored",
                "status",
                "network",
                "total_episodes",
                "available_episode_count",
                "rating",
                "content_rating",
                "runtime",
                "description",
            ],
            IndexKind::TvSeasons => &[
                "id",
                "show_id",
                "show_title",
                "display_title",
                "season_title",
                "season_identifier",
                "season_code",
                "episode_count",
                "is_monitored",
                "is_special",
            ],
            IndexKind::TvEpisodes => &[
                "id",
                "show_id",
                "season_id",
                "show_title",
                "display_title",
                "episode_title",
                "episode_code",
                "is_in_library",
                "want_status",
                "air_date",
                "air_date_ts",
                "season_sort_order",
                "episode_sort_order",
                "runtime",
                "description",
            ],
            IndexKind::Authors => &["id", "name", "book_count", "is_monitored"],
            IndexKind::Series => &["id", "name", "book_count", "is_monitored"],
        }
    }

    /// Mirrors each index's `searchableAttributes`.
    pub fn searchable(self) -> &'static [&'static str] {
        match self {
            IndexKind::Books => &[
                "title",
                "authors",
                "series_name",
                "all_series_names",
                "genres",
                "narrator",
            ],
            IndexKind::Movies => &["title", "directors", "genres"],
            IndexKind::TvShows => &["title", "genres", "network"],
            IndexKind::TvSeasons => &["display_title", "show_title", "season_title", "season_code"],
            IndexKind::TvEpisodes => &[
                "display_title",
                "show_title",
                "episode_title",
                "episode_code",
            ],
            IndexKind::Authors => &["name"],
            IndexKind::Series => &["name"],
        }
    }
}

#[derive(Debug, Clone)]
pub struct IndexQuery {
    pub index: IndexKind,
    /// Empty means "everything matching the filter", which is how the relative
    /// time queries work.
    pub query: String,
    pub filter: Option<String>,
    /// Meili sort expressions, e.g. `air_date_ts:desc`.
    pub sort: Vec<String>,
    pub limit: usize,
}

impl IndexQuery {
    pub fn new(index: IndexKind, query: impl Into<String>, limit: usize) -> Self {
        Self {
            index,
            query: query.into(),
            filter: None,
            sort: Vec::new(),
            limit,
        }
    }

    pub fn filtered(mut self, filter: impl Into<String>) -> Self {
        self.filter = Some(filter.into());
        self
    }

    pub fn sorted(mut self, sort: &[&str]) -> Self {
        self.sort = sort.iter().map(|s| (*s).to_string()).collect();
        self
    }
}

/// The union of everything the fan-out returned, deduplicated by id.
#[derive(Debug, Clone, Default)]
pub struct IndexHits {
    pub books: Vec<BookDocument>,
    pub movies: Vec<MovieDocument>,
    pub shows: Vec<TvShowDocument>,
    pub seasons: Vec<TvSeasonDocument>,
    pub episodes: Vec<TvEpisodeDocument>,
    pub authors: Vec<AuthorDocument>,
    pub series: Vec<SeriesDocument>,
}

impl IndexHits {
    pub fn is_empty(&self) -> bool {
        self.books.is_empty()
            && self.movies.is_empty()
            && self.shows.is_empty()
            && self.seasons.is_empty()
            && self.episodes.is_empty()
            && self.authors.is_empty()
            && self.series.is_empty()
    }

    pub(super) fn push_values(&mut self, index: IndexKind, values: Vec<serde_json::Value>) {
        fn decode<T: DeserializeOwned>(values: Vec<serde_json::Value>) -> Vec<T> {
            values
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect()
        }
        match index {
            IndexKind::Books => self.books.extend(decode(values)),
            IndexKind::Movies => self.movies.extend(decode(values)),
            IndexKind::TvShows => self.shows.extend(decode(values)),
            IndexKind::TvSeasons => self.seasons.extend(decode(values)),
            IndexKind::TvEpisodes => self.episodes.extend(decode(values)),
            IndexKind::Authors => self.authors.extend(decode(values)),
            IndexKind::Series => self.series.extend(decode(values)),
        }
    }

    /// Keeps the first occurrence, so the order the queries were issued in is
    /// the order the ranker sees.
    pub fn dedup(&mut self) {
        fn keep_first<T>(v: &mut Vec<T>, id: impl Fn(&T) -> String) {
            let mut seen = std::collections::HashSet::new();
            v.retain(|item| seen.insert(id(item)));
        }
        keep_first(&mut self.books, |b| b.id.clone());
        keep_first(&mut self.movies, |m| m.id.clone());
        keep_first(&mut self.shows, |s| s.id.clone());
        keep_first(&mut self.seasons, |s| s.id.clone());
        keep_first(&mut self.episodes, |e| e.id.clone());
        keep_first(&mut self.authors, |a| a.id.clone());
        keep_first(&mut self.series, |s| s.id.clone());
    }

    pub fn merge(&mut self, other: IndexHits) {
        self.books.extend(other.books);
        self.movies.extend(other.movies);
        self.shows.extend(other.shows);
        self.seasons.extend(other.seasons);
        self.episodes.extend(other.episodes);
        self.authors.extend(other.authors);
        self.series.extend(other.series);
        self.dedup();
    }
}

#[async_trait]
pub trait LibraryIndex: Send + Sync {
    /// One round trip for every query in `queries`.
    async fn multi_search(&self, queries: &[IndexQuery]) -> Result<IndexHits, ResolveError>;
}

/// The real thing. Holds its own Meili client because `AppState` exposes the
/// search backend as `dyn SearchBackend`, which has no multi-search method and
/// is shared with the rest of the app.
#[derive(Clone)]
pub struct MeiliLibraryIndex {
    client: meilisearch_sdk::client::Client,
}

impl MeiliLibraryIndex {
    pub fn new(url: &str, master_key: &str) -> Result<Self, ResolveError> {
        let client = meilisearch_sdk::client::Client::new(url, Some(master_key))
            .map_err(|e| ResolveError::Index(e.to_string()))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl LibraryIndex for MeiliLibraryIndex {
    async fn multi_search(&self, queries: &[IndexQuery]) -> Result<IndexHits, ResolveError> {
        if queries.is_empty() {
            return Ok(IndexHits::default());
        }
        // Declared before `indexes` so the borrows in the built queries outlive
        // the index handles they are attached to.
        let sorts: Vec<Vec<&str>> = queries
            .iter()
            .map(|q| q.sort.iter().map(String::as_str).collect())
            .collect();
        let indexes: Vec<_> = queries
            .iter()
            .map(|q| self.client.index(q.index.uid()))
            .collect();

        let mut multi = self.client.multi_search();
        for (i, q) in queries.iter().enumerate() {
            let mut search = indexes[i].search();
            search
                .with_query(&q.query)
                .with_limit(q.limit)
                // Every returned document contains every query term, so the
                // ranker never has to argue with Meili's default term-dropping.
                .with_matching_strategy(meilisearch_sdk::search::MatchingStrategies::ALL)
                .with_attributes_to_retrieve(meilisearch_sdk::search::Selectors::Some(
                    q.index.attributes(),
                ));
            if let Some(filter) = &q.filter {
                search.with_filter(filter);
            }
            if !sorts[i].is_empty() {
                search.with_sort(&sorts[i]);
            }
            multi.with_search_query(search.build());
        }

        let response = multi
            .execute::<serde_json::Value>()
            .await
            .map_err(|e| ResolveError::Index(e.to_string()))?;

        let mut hits = IndexHits::default();
        for (i, result) in response.results.into_iter().enumerate() {
            let Some(q) = queries.get(i) else { continue };
            hits.push_values(q.index, result.hits.into_iter().map(|h| h.result).collect());
        }
        hits.dedup();
        Ok(hits)
    }
}
