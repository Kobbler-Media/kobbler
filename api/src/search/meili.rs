//! MeiliSearch implementation of the SearchBackend trait.

use async_trait::async_trait;
use meilisearch_sdk::client::Client;
use meilisearch_sdk::search::{FederatedMultiSearchResponse, FederationOptions};
use reqwest::header::HeaderValue;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use url::Url;
use uuid::Uuid;

use super::backend::{
    AuthorSearchFilters, BookSearchFilters, FacetBucket, IptvChannelSearchFilters,
    IptvEpgSearchFilters, LibraryFacets, LivetvChannelSearchFilters, LivetvGuideSearchFilters,
    MovieSearchFilters, SearchBackend, SearchBackendStatus, SearchError, SeriesSearchFilters,
    TvShowSearchFilters,
};
use super::{
    AuthorDocument, BookDocument, CommandDocument, GenreDocument, IptvChannelDocument,
    IptvEpgDocument, LivetvChannelDocument, LivetvGuideDocument, MediaRootFileDocument,
    MovieDocument, PageDocument, PersonDocument, SearchHit, SeriesDocument, SettingDocument,
    TranscriptSegmentDocument, TvEpisodeDocument, TvSeasonDocument, TvShowDocument,
    UnifiedSearchResponse, UnifiedSearchResult, indexes, semantic,
};
use semantic::SemanticQuery;

/// Convert a Meilisearch facet distribution for `field` into buckets sorted by
/// descending count (then value, for stable ordering).
fn facet_buckets(
    distribution: Option<&HashMap<String, HashMap<String, usize>>>,
    field: &str,
) -> Vec<FacetBucket> {
    let mut buckets: Vec<FacetBucket> = distribution
        .and_then(|fd| fd.get(field))
        .map(|counts| {
            counts
                .iter()
                .map(|(value, &count)| FacetBucket {
                    value: value.clone(),
                    count,
                })
                .collect()
        })
        .unwrap_or_default();
    buckets.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    buckets
}

/// A hit only the vector half found, scoring under the floor. Keyword hits are
/// never dropped: their details carry the keyword ranking rules, not `vectorSort`.
fn is_weak_semantic_hit(details: Option<&serde_json::Map<String, serde_json::Value>>) -> bool {
    details
        .and_then(|d| d.get("vectorSort"))
        .and_then(|v| v.get("similarity"))
        .and_then(serde_json::Value::as_f64)
        .is_some_and(|similarity| similarity < semantic::semantic_hit_floor())
}

/// A title index's query: keyword, or a hybrid against the summary embedder.
fn titled_query<'a, Http: meilisearch_sdk::request::HttpClient>(
    index: &'a meilisearch_sdk::indexes::Index<Http>,
    query: &'a str,
    semantic: Option<&'a SemanticQuery>,
) -> meilisearch_sdk::search::SearchQuery<'a, Http> {
    let mut search = index.search();
    search.with_query(query);
    if let Some(semantic) = semantic {
        search
            .with_hybrid(semantic::DESCRIPTION_EMBEDDER, semantic.ratio)
            .with_vector(&semantic.vector)
            .with_show_ranking_score_details(true);
    }
    search.build()
}

/// The secondary, artwork-only query: pure vector, so it adds titles whose poster
/// matches without repeating the keyword half the summary query already ran.
fn poster_query<'a, Http: meilisearch_sdk::request::HttpClient>(
    index: &'a meilisearch_sdk::indexes::Index<Http>,
    query: &'a str,
    semantic: &'a SemanticQuery,
) -> meilisearch_sdk::search::SearchQuery<'a, Http> {
    let mut search = index.search();
    search
        .with_query(query)
        .with_hybrid(semantic::POSTER_EMBEDDER, 1.0)
        .with_vector(&semantic.vector)
        .with_show_ranking_score_details(true);
    search.build()
}

/// Internal state that can be reconfigured.
struct MeiliInner {
    client: Client,
    url: String,
}

/// MeiliSearch state wrapper for the application.
///
/// Uses an RwLock to allow hot-reloading of connection settings.
#[derive(Clone)]
pub struct MeiliState {
    inner: Arc<RwLock<MeiliInner>>,
    semantic: Arc<AtomicBool>,
}

/// Build a Meilisearch client from user-supplied settings.
///
/// The SDK unwraps the `Authorization` header it builds from the master key and
/// never validates the host, so both are checked here before handing them over:
/// a typo in the config file must surface as an error, not a panic.
pub(crate) fn build_client(url: &str, master_key: &str) -> Result<Client, SearchError> {
    Url::parse(url)
        .map_err(|e| SearchError::Connection(format!("Invalid Meilisearch URL {url:?}: {e}")))?;

    HeaderValue::from_str(&format!("Bearer {master_key}")).map_err(|_| {
        SearchError::Connection(
            "Meilisearch master key contains characters that are not valid in an HTTP header"
                .to_string(),
        )
    })?;

    Client::new(url, Some(master_key))
        .map_err(|e| SearchError::Connection(format!("Failed to create Meilisearch client: {e}")))
}

impl MeiliState {
    /// Create a new MeiliSearch state from URL and master key.
    pub fn new(url: &str, master_key: &str) -> Result<Self, SearchError> {
        Ok(Self {
            inner: Arc::new(RwLock::new(MeiliInner {
                client: build_client(url, master_key)?,
                url: url.to_string(),
            })),
            semantic: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Get a clone of the client and URL (cheap operation).
    ///
    /// This acquires a read lock briefly to clone the client,
    /// allowing the caller to use the client without holding the lock.
    pub async fn get_client(&self) -> (Client, String) {
        let inner = self.inner.read().await;
        (inner.client.clone(), inner.url.clone())
    }

    async fn federated_search(
        &self,
        query: &str,
        limit: usize,
        semantic: Option<&SemanticQuery>,
    ) -> Result<UnifiedSearchResponse, SearchError> {
        let (client, _url) = self.get_client().await;

        // Build federated multi-search query across all indexes
        let mut multi_search = client.multi_search();

        // Create search queries for each index
        let books_index = client.index(indexes::BOOKS);
        let authors_index = client.index(indexes::AUTHORS);
        let series_index = client.index(indexes::SERIES);
        let genres_index = client.index(indexes::GENRES);
        let pages_index = client.index(indexes::PAGES);
        let commands_index = client.index(indexes::COMMANDS);
        let movies_index = client.index(indexes::MOVIES);
        let tv_shows_index = client.index(indexes::TV_SHOWS);
        let tv_seasons_index = client.index(indexes::TV_SEASONS);
        let tv_episodes_index = client.index(indexes::TV_EPISODES);
        let people_index = client.index(indexes::PEOPLE);

        // Add per-index search queries
        // Note: transcript_segments excluded — too noisy for unified search (no per-query limit — federation.limit controls pagination)
        // TV shows get a weight boost so parent entities rank above their seasons/episodes
        // for the same query terms (seasons/episodes match show name in multiple fields)
        let iptv_channels_index = client.index(indexes::IPTV_CHANNELS);
        let livetv_channels_index = client.index(indexes::LIVETV_CHANNELS);
        // IPTV_EPG and LIVETV_GUIDE are intentionally excluded from unified search —
        // scheduled programmes create too much noise. Still searchable in the guides.
        let settings_index = client.index(indexes::SETTINGS);

        for index in [
            &authors_index,
            &series_index,
            &genres_index,
            &pages_index,
            &commands_index,
            &settings_index,
            &people_index,
            &iptv_channels_index,
            &livetv_channels_index,
        ] {
            multi_search.with_search_query(index.search().with_query(query).build());
        }
        multi_search.with_search_query(titled_query(&books_index, query, semantic));
        multi_search.with_search_query(titled_query(&movies_index, query, semantic));
        multi_search
            .with_search_query_and_weight(titled_query(&tv_shows_index, query, semantic), 1.3);
        if let Some(semantic) = semantic {
            let w = semantic::POSTER_WEIGHT;
            multi_search
                .with_search_query_and_weight(poster_query(&books_index, query, semantic), w);
            multi_search
                .with_search_query_and_weight(poster_query(&movies_index, query, semantic), w);
            multi_search.with_search_query_and_weight(
                poster_query(&tv_shows_index, query, semantic),
                w * 1.3,
            );
        }
        multi_search
            .with_search_query_and_weight(tv_seasons_index.search().with_query(query).build(), 0.8);
        multi_search.with_search_query_and_weight(
            tv_episodes_index.search().with_query(query).build(),
            0.7,
        );

        let federation = FederationOptions {
            limit: Some(limit * 10), // enough room for all indexes
            ..Default::default()
        };

        // Execute federated multi-search — returns a single merged+ranked hit list
        let result: FederatedMultiSearchResponse<serde_json::Value> =
            multi_search.with_federation(federation).execute().await?;

        // Convert federated hits to SearchHit using index_uid from federation metadata
        let mut top_hits = Vec::new();
        let mut remaining = Vec::new();
        let mut seen_types: HashSet<&'static str> = HashSet::new();
        // The summary and poster queries can both find a title; hits arrive best
        // first, so the first copy is the one to keep.
        let mut seen_docs: HashSet<(String, String)> = HashSet::new();

        for hit in &result.hits {
            if is_weak_semantic_hit(hit.ranking_score_details.as_ref()) {
                continue;
            }
            let index_uid = hit
                .federation
                .as_ref()
                .map(|f| f.index_uid.as_str())
                .unwrap_or("");
            if let Some(id) = hit.result.get("id").and_then(serde_json::Value::as_str)
                && !seen_docs.insert((index_uid.to_string(), id.to_string()))
            {
                continue;
            }

            let search_hit = match index_uid {
                indexes::BOOKS => serde_json::from_value::<BookDocument>(hit.result.clone())
                    .ok()
                    .map(SearchHit::Book),
                indexes::AUTHORS => serde_json::from_value::<AuthorDocument>(hit.result.clone())
                    .ok()
                    .map(SearchHit::Author),
                indexes::SERIES => serde_json::from_value::<SeriesDocument>(hit.result.clone())
                    .ok()
                    .map(SearchHit::Series),
                indexes::GENRES => serde_json::from_value::<GenreDocument>(hit.result.clone())
                    .ok()
                    .map(SearchHit::Genre),
                indexes::PAGES => serde_json::from_value::<PageDocument>(hit.result.clone())
                    .ok()
                    .map(SearchHit::Page),
                indexes::COMMANDS => serde_json::from_value::<CommandDocument>(hit.result.clone())
                    .ok()
                    .map(SearchHit::Command),
                indexes::MOVIES => serde_json::from_value::<MovieDocument>(hit.result.clone())
                    .ok()
                    .map(SearchHit::Movie),
                indexes::TV_SHOWS => serde_json::from_value::<TvShowDocument>(hit.result.clone())
                    .ok()
                    .map(SearchHit::TvShow),
                indexes::TV_SEASONS => {
                    serde_json::from_value::<TvSeasonDocument>(hit.result.clone())
                        .ok()
                        .map(SearchHit::TvSeason)
                }
                indexes::TV_EPISODES => {
                    serde_json::from_value::<TvEpisodeDocument>(hit.result.clone())
                        .ok()
                        .map(SearchHit::TvEpisode)
                }
                indexes::PEOPLE => serde_json::from_value::<PersonDocument>(hit.result.clone())
                    .ok()
                    .map(SearchHit::Person),
                indexes::TRANSCRIPT_SEGMENTS => {
                    serde_json::from_value::<TranscriptSegmentDocument>(hit.result.clone())
                        .ok()
                        .map(SearchHit::Transcript)
                }
                indexes::IPTV_CHANNELS => {
                    serde_json::from_value::<IptvChannelDocument>(hit.result.clone())
                        .ok()
                        .map(SearchHit::IptvChannel)
                }
                indexes::IPTV_EPG => serde_json::from_value::<IptvEpgDocument>(hit.result.clone())
                    .ok()
                    .map(SearchHit::IptvEpg),
                indexes::LIVETV_CHANNELS => {
                    serde_json::from_value::<LivetvChannelDocument>(hit.result.clone())
                        .ok()
                        .map(SearchHit::LivetvChannel)
                }
                indexes::LIVETV_GUIDE => {
                    serde_json::from_value::<LivetvGuideDocument>(hit.result.clone())
                        .ok()
                        .map(SearchHit::LivetvGuide)
                }
                indexes::SETTINGS => serde_json::from_value::<SettingDocument>(hit.result.clone())
                    .ok()
                    .map(SearchHit::Setting),
                _ => None,
            };

            if let Some(search_hit) = search_hit {
                let type_name = search_hit.type_name();
                if !seen_types.contains(type_name) {
                    seen_types.insert(type_name);
                    top_hits.push(search_hit);
                } else {
                    remaining.push(search_hit);
                }
            }
        }

        Ok(UnifiedSearchResponse {
            top_hits,
            remaining,
        })
    }

    /// Check if a query string looks like a UUID.
    fn is_uuid_query(query: &str) -> Option<String> {
        let trimmed = query.trim();
        Uuid::parse_str(trimmed).ok().map(|u| u.to_string())
    }

    /// Build filter string from book search filters.
    fn build_book_filter(filters: &BookSearchFilters, uuid_filter: Option<&str>) -> Option<String> {
        let mut parts = vec![];

        if let Some(val) = filters.is_in_library {
            parts.push(format!("is_in_library = {}", val));
        }

        if let Some(ref status) = filters.want_status {
            parts.push(format!("want_status = \"{}\"", status));
        }

        // Exclude ingest queue items (only show library books with ingest_status = NULL)
        if filters.exclude_ingest_queue {
            parts.push("ingest_status IS NULL".to_string());
        }

        // Format filter
        if let Some(ref fmt) = filters.format {
            match fmt.as_str() {
                "audiobook" => parts.push("has_audiobook = true".to_string()),
                "ebook" => parts.push("has_ebook = true".to_string()),
                _ => {}
            }
        }

        // UUID filter: match book ID, any author ID, or series ID
        if let Some(uuid) = uuid_filter {
            parts.push(format!(
                "(id = \"{}\" OR author_ids = \"{}\" OR series_id = \"{}\")",
                uuid, uuid, uuid
            ));
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" AND "))
        }
    }

    /// Build filter string from author search filters.
    fn build_author_filter(
        filters: &AuthorSearchFilters,
        uuid_filter: Option<&str>,
    ) -> Option<String> {
        let mut parts = vec![];

        if let Some(val) = filters.is_monitored {
            parts.push(format!("is_monitored = {}", val));
        }

        if let Some(uuid) = uuid_filter {
            parts.push(format!("id = \"{}\"", uuid));
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" AND "))
        }
    }

    /// Build filter string from series search filters.
    fn build_series_filter(
        filters: &SeriesSearchFilters,
        uuid_filter: Option<&str>,
    ) -> Option<String> {
        let mut parts = vec![];

        if let Some(val) = filters.is_monitored {
            parts.push(format!("is_monitored = {}", val));
        }

        if let Some(uuid) = uuid_filter {
            parts.push(format!("id = \"{}\"", uuid));
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" AND "))
        }
    }

    /// Build filter string from movie search filters.
    fn build_movie_filter(
        filters: &MovieSearchFilters,
        uuid_filter: Option<&str>,
    ) -> Option<String> {
        let mut parts = vec![];

        if let Some(val) = filters.is_in_library {
            parts.push(format!("is_in_library = {}", val));
        }

        if let Some(ref status) = filters.want_status {
            parts.push(format!("want_status = \"{}\"", status));
        }

        if let Some(uuid) = uuid_filter {
            parts.push(format!("id = \"{}\"", uuid));
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" AND "))
        }
    }

    /// Build filter string from TV show search filters.
    fn build_tv_show_filter(
        filters: &TvShowSearchFilters,
        uuid_filter: Option<&str>,
    ) -> Option<String> {
        let mut parts = vec![];

        if let Some(val) = filters.is_monitored {
            parts.push(format!("is_monitored = {}", val));
        }

        if let Some(ref status) = filters.status {
            parts.push(format!("status = \"{}\"", status));
        }

        if let Some(uuid) = uuid_filter {
            parts.push(format!("id = \"{}\"", uuid));
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" AND "))
        }
    }

    /// UUID lookup for exact matches (when query is a UUID).
    async fn uuid_lookup(
        &self,
        uuid: &str,
        limit: usize,
    ) -> Result<UnifiedSearchResponse, SearchError> {
        let (client, _url) = self.get_client().await;

        // Search each index with UUID filter
        let books_index = client.index(indexes::BOOKS);
        let authors_index = client.index(indexes::AUTHORS);
        let series_index = client.index(indexes::SERIES);
        let movies_index = client.index(indexes::MOVIES);
        let tv_shows_index = client.index(indexes::TV_SHOWS);
        let tv_seasons_index = client.index(indexes::TV_SEASONS);
        let tv_episodes_index = client.index(indexes::TV_EPISODES);
        let people_index = client.index(indexes::PEOPLE);

        let books_filter = format!(
            "(id = \"{}\" OR author_ids = \"{}\" OR series_id = \"{}\")",
            uuid, uuid, uuid
        );
        let id_filter = format!("id = \"{}\"", uuid);
        let show_or_id_filter = format!("(id = \"{}\" OR show_id = \"{}\")", uuid, uuid);

        let mut books_search = books_index.search();
        books_search
            .with_query("")
            .with_limit(limit)
            .with_filter(&books_filter);

        let mut authors_search = authors_index.search();
        authors_search
            .with_query("")
            .with_limit(limit)
            .with_filter(&id_filter);

        let mut series_search = series_index.search();
        series_search
            .with_query("")
            .with_limit(limit)
            .with_filter(&id_filter);

        let mut movies_search = movies_index.search();
        movies_search
            .with_query("")
            .with_limit(limit)
            .with_filter(&id_filter);

        let mut tv_shows_search = tv_shows_index.search();
        tv_shows_search
            .with_query("")
            .with_limit(limit)
            .with_filter(&id_filter);

        let mut tv_seasons_search = tv_seasons_index.search();
        tv_seasons_search
            .with_query("")
            .with_limit(limit)
            .with_filter(&show_or_id_filter);

        let mut tv_episodes_search = tv_episodes_index.search();
        tv_episodes_search
            .with_query("")
            .with_limit(limit)
            .with_filter(&show_or_id_filter);

        let mut people_search = people_index.search();
        people_search
            .with_query("")
            .with_limit(limit)
            .with_filter(&id_filter);

        let (
            books_result,
            authors_result,
            series_result,
            movies_result,
            tv_shows_result,
            tv_seasons_result,
            tv_episodes_result,
            people_result,
        ) = tokio::join!(
            books_search.execute::<BookDocument>(),
            authors_search.execute::<AuthorDocument>(),
            series_search.execute::<SeriesDocument>(),
            movies_search.execute::<MovieDocument>(),
            tv_shows_search.execute::<TvShowDocument>(),
            tv_seasons_search.execute::<TvSeasonDocument>(),
            tv_episodes_search.execute::<TvEpisodeDocument>(),
            people_search.execute::<PersonDocument>(),
        );

        let mut top_hits = Vec::new();
        let mut remaining = Vec::new();

        // Add author first if found (most likely what user wants)
        if let Ok(result) = authors_result {
            for hit in result.hits {
                if top_hits
                    .iter()
                    .all(|h: &SearchHit| h.type_name() != "author")
                {
                    top_hits.push(SearchHit::Author(hit.result));
                } else {
                    remaining.push(SearchHit::Author(hit.result));
                }
            }
        }

        // Add series
        if let Ok(result) = series_result {
            for hit in result.hits {
                if top_hits
                    .iter()
                    .all(|h: &SearchHit| h.type_name() != "series")
                {
                    top_hits.push(SearchHit::Series(hit.result));
                } else {
                    remaining.push(SearchHit::Series(hit.result));
                }
            }
        }

        // Add books
        if let Ok(result) = books_result {
            for hit in result.hits {
                if top_hits.iter().all(|h: &SearchHit| h.type_name() != "book") {
                    top_hits.push(SearchHit::Book(hit.result));
                } else {
                    remaining.push(SearchHit::Book(hit.result));
                }
            }
        }

        // Add movies
        if let Ok(result) = movies_result {
            for hit in result.hits {
                if top_hits
                    .iter()
                    .all(|h: &SearchHit| h.type_name() != "movie")
                {
                    top_hits.push(SearchHit::Movie(hit.result));
                } else {
                    remaining.push(SearchHit::Movie(hit.result));
                }
            }
        }

        // Add TV shows
        if let Ok(result) = tv_shows_result {
            for hit in result.hits {
                if top_hits
                    .iter()
                    .all(|h: &SearchHit| h.type_name() != "tv_show")
                {
                    top_hits.push(SearchHit::TvShow(hit.result));
                } else {
                    remaining.push(SearchHit::TvShow(hit.result));
                }
            }
        }

        // Add TV seasons
        if let Ok(result) = tv_seasons_result {
            for hit in result.hits {
                if top_hits
                    .iter()
                    .all(|h: &SearchHit| h.type_name() != "tv_season")
                {
                    top_hits.push(SearchHit::TvSeason(hit.result));
                } else {
                    remaining.push(SearchHit::TvSeason(hit.result));
                }
            }
        }

        // Add TV episodes
        if let Ok(result) = tv_episodes_result {
            for hit in result.hits {
                if top_hits
                    .iter()
                    .all(|h: &SearchHit| h.type_name() != "tv_episode")
                {
                    top_hits.push(SearchHit::TvEpisode(hit.result));
                } else {
                    remaining.push(SearchHit::TvEpisode(hit.result));
                }
            }
        }

        // Add people
        if let Ok(result) = people_result {
            for hit in result.hits {
                if top_hits
                    .iter()
                    .all(|h: &SearchHit| h.type_name() != "person")
                {
                    top_hits.push(SearchHit::Person(hit.result));
                } else {
                    remaining.push(SearchHit::Person(hit.result));
                }
            }
        }

        Ok(UnifiedSearchResponse {
            top_hits,
            remaining,
        })
    }
}

#[async_trait]
impl SearchBackend for MeiliState {
    async fn unified_search(
        &self,
        query: &str,
        limit: usize,
        semantic: Option<&SemanticQuery>,
    ) -> Result<UnifiedSearchResponse, SearchError> {
        // Check if query is a UUID - skip federated search and do targeted lookup
        if let Some(uuid) = Self::is_uuid_query(query) {
            return self.uuid_lookup(&uuid, limit).await;
        }

        match semantic {
            Some(semantic) => match self.federated_search(query, limit, Some(semantic)).await {
                Ok(response) => Ok(response),
                Err(e) => {
                    warn!(error = %e, "Hybrid search rejected; searching by keyword");
                    self.federated_search(query, limit, None).await
                }
            },
            None => self.federated_search(query, limit, None).await,
        }
    }

    async fn search(&self, query: &str, limit: usize) -> Result<UnifiedSearchResult, SearchError> {
        let (client, _url) = self.get_client().await;
        let books_index = client.index(indexes::BOOKS);
        let authors_index = client.index(indexes::AUTHORS);
        let series_index = client.index(indexes::SERIES);
        let genres_index = client.index(indexes::GENRES);

        // Check if query is a UUID - use exact filter instead of fuzzy search
        let uuid_filter = Self::is_uuid_query(query);
        let search_query = if uuid_filter.is_some() { "" } else { query };

        // Build filters
        let books_filter =
            Self::build_book_filter(&BookSearchFilters::default(), uuid_filter.as_deref());
        let authors_filter =
            Self::build_author_filter(&AuthorSearchFilters::default(), uuid_filter.as_deref());
        let series_filter =
            Self::build_series_filter(&SeriesSearchFilters::default(), uuid_filter.as_deref());

        // Build search queries
        let mut books_search = books_index.search();
        books_search.with_query(search_query).with_limit(limit);
        if let Some(ref filter) = books_filter {
            books_search.with_filter(filter);
        }

        let mut authors_search = authors_index.search();
        authors_search.with_query(search_query).with_limit(limit);
        if let Some(ref filter) = authors_filter {
            authors_search.with_filter(filter);
        }

        let mut series_search = series_index.search();
        series_search.with_query(search_query).with_limit(limit);
        if let Some(ref filter) = series_filter {
            series_search.with_filter(filter);
        }

        let mut genres_search = genres_index.search();
        genres_search.with_query(search_query).with_limit(limit);
        // Genres don't need UUID filter since they don't have meaningful UUIDs to search for

        // Run searches in parallel
        let (books_result, authors_result, series_result, genres_result) = tokio::join!(
            books_search.execute::<BookDocument>(),
            authors_search.execute::<AuthorDocument>(),
            series_search.execute::<SeriesDocument>(),
            genres_search.execute::<GenreDocument>(),
        );

        let books = books_result
            .map(|r| r.hits.into_iter().map(|h| h.result).collect())
            .unwrap_or_else(|e| {
                error!(error = %e, "Failed to search books");
                vec![]
            });

        let authors = authors_result
            .map(|r| r.hits.into_iter().map(|h| h.result).collect())
            .unwrap_or_else(|e| {
                error!(error = %e, "Failed to search authors");
                vec![]
            });

        let series = series_result
            .map(|r| r.hits.into_iter().map(|h| h.result).collect())
            .unwrap_or_else(|e| {
                error!(error = %e, "Failed to search series");
                vec![]
            });

        let genres = genres_result
            .map(|r| r.hits.into_iter().map(|h| h.result).collect())
            .unwrap_or_else(|e| {
                error!(error = %e, "Failed to search genres");
                vec![]
            });

        Ok(UnifiedSearchResult {
            books,
            authors,
            series,
            genres,
        })
    }

    async fn search_books(
        &self,
        query: &str,
        filters: BookSearchFilters,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Uuid>, SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::BOOKS);

        // Check if query is a UUID - use exact filter instead of fuzzy search
        let uuid_filter = Self::is_uuid_query(query);
        let search_query = if uuid_filter.is_some() { "" } else { query };
        let filter_str = Self::build_book_filter(&filters, uuid_filter.as_deref());

        let mut search = index.search();
        search
            .with_query(search_query)
            .with_limit(limit)
            .with_offset(offset)
            // Sort by series name then index as a tiebreaker within similar relevance
            .with_sort(&["series_name:asc", "series_index:asc"]);

        if let Some(ref filter) = filter_str {
            search.with_filter(filter);
        }

        let result = search.execute::<BookDocument>().await?;

        let ids = result
            .hits
            .into_iter()
            .filter_map(|h| Uuid::parse_str(&h.result.id).ok())
            .collect();

        Ok(ids)
    }

    async fn search_authors(
        &self,
        query: &str,
        filters: AuthorSearchFilters,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Uuid>, SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::AUTHORS);

        // Check if query is a UUID - use exact filter instead of fuzzy search
        let uuid_filter = Self::is_uuid_query(query);
        let search_query = if uuid_filter.is_some() { "" } else { query };
        let filter_str = Self::build_author_filter(&filters, uuid_filter.as_deref());

        let mut search = index.search();
        search
            .with_query(search_query)
            .with_limit(limit)
            .with_offset(offset);

        if let Some(ref filter) = filter_str {
            search.with_filter(filter);
        }

        let result = search.execute::<AuthorDocument>().await?;

        let ids = result
            .hits
            .into_iter()
            .filter_map(|h| Uuid::parse_str(&h.result.id).ok())
            .collect();

        Ok(ids)
    }

    async fn search_series(
        &self,
        query: &str,
        filters: SeriesSearchFilters,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Uuid>, SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::SERIES);

        // Check if query is a UUID - use exact filter instead of fuzzy search
        let uuid_filter = Self::is_uuid_query(query);
        let search_query = if uuid_filter.is_some() { "" } else { query };
        let filter_str = Self::build_series_filter(&filters, uuid_filter.as_deref());

        let mut search = index.search();
        search
            .with_query(search_query)
            .with_limit(limit)
            .with_offset(offset);

        if let Some(ref filter) = filter_str {
            search.with_filter(filter);
        }

        let result = search.execute::<SeriesDocument>().await?;

        let ids = result
            .hits
            .into_iter()
            .filter_map(|h| Uuid::parse_str(&h.result.id).ok())
            .collect();

        Ok(ids)
    }

    async fn search_movies(
        &self,
        query: &str,
        filters: MovieSearchFilters,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Uuid>, SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::MOVIES);

        // Check if query is a UUID - use exact filter instead of fuzzy search
        let uuid_filter = Self::is_uuid_query(query);
        let search_query = if uuid_filter.is_some() { "" } else { query };
        let filter_str = Self::build_movie_filter(&filters, uuid_filter.as_deref());

        let mut search = index.search();
        search
            .with_query(search_query)
            .with_limit(limit)
            .with_offset(offset)
            .with_sort(&["title:asc"]);

        if let Some(ref filter) = filter_str {
            search.with_filter(filter);
        }

        let result = search.execute::<MovieDocument>().await?;

        let ids = result
            .hits
            .into_iter()
            .filter_map(|h| Uuid::parse_str(&h.result.id).ok())
            .collect();

        Ok(ids)
    }

    async fn search_tv_shows(
        &self,
        query: &str,
        filters: TvShowSearchFilters,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Uuid>, SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::TV_SHOWS);

        // Check if query is a UUID - use exact filter instead of fuzzy search
        let uuid_filter = Self::is_uuid_query(query);
        let search_query = if uuid_filter.is_some() { "" } else { query };
        let filter_str = Self::build_tv_show_filter(&filters, uuid_filter.as_deref());

        let mut search = index.search();
        search
            .with_query(search_query)
            .with_limit(limit)
            .with_offset(offset)
            .with_sort(&["title:asc"]);

        if let Some(ref filter) = filter_str {
            search.with_filter(filter);
        }

        let result = search.execute::<TvShowDocument>().await?;

        let ids = result
            .hits
            .into_iter()
            .filter_map(|h| Uuid::parse_str(&h.result.id).ok())
            .collect();

        Ok(ids)
    }

    async fn search_people(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Uuid>, SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::PEOPLE);

        let uuid_filter = Self::is_uuid_query(query);
        let search_query = if uuid_filter.is_some() { "" } else { query };

        let mut search = index.search();
        search
            .with_query(search_query)
            .with_limit(limit)
            .with_offset(offset)
            .with_sort(&["name:asc"]);

        let filter_string = uuid_filter.map(|uuid| format!("id = \"{}\"", uuid));
        if let Some(ref filter) = filter_string {
            search.with_filter(filter);
        }

        let result = search.execute::<PersonDocument>().await?;

        let ids = result
            .hits
            .into_iter()
            .filter_map(|h| Uuid::parse_str(&h.result.id).ok())
            .collect();

        Ok(ids)
    }

    async fn get_available_book_letters(
        &self,
        filters: BookSearchFilters,
    ) -> Result<Vec<String>, SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::BOOKS);

        // Build filter from BookSearchFilters
        let filter_str = Self::build_book_filter(&filters, None);

        // Use an empty search with facets to get distinct first_letter values
        let mut search = index.search();
        search.with_query("").with_limit(0);
        search.with_facets(meilisearch_sdk::search::Selectors::Some(&["first_letter"]));

        if let Some(ref filter) = filter_str {
            search.with_filter(filter);
        }

        let result = search.execute::<BookDocument>().await?;

        // Extract letters from facet distribution
        let mut letters: Vec<String> = result
            .facet_distribution
            .and_then(|fd| fd.get("first_letter").cloned())
            .map(|letter_counts| letter_counts.keys().cloned().collect())
            .unwrap_or_default();

        // Sort: A-Z first, then #
        letters.sort_by(|a, b| match (a.as_str(), b.as_str()) {
            ("#", _) => std::cmp::Ordering::Greater,
            (_, "#") => std::cmp::Ordering::Less,
            _ => a.cmp(b),
        });

        Ok(letters)
    }

    async fn get_available_movie_letters(
        &self,
        filters: MovieSearchFilters,
    ) -> Result<Vec<String>, SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::MOVIES);

        let filter_str = Self::build_movie_filter(&filters, None);

        let mut search = index.search();
        search.with_query("").with_limit(0);
        search.with_facets(meilisearch_sdk::search::Selectors::Some(&["first_letter"]));

        if let Some(ref filter) = filter_str {
            search.with_filter(filter);
        }

        let result = search.execute::<MovieDocument>().await?;

        let mut letters: Vec<String> = result
            .facet_distribution
            .and_then(|fd| fd.get("first_letter").cloned())
            .map(|letter_counts| letter_counts.keys().cloned().collect())
            .unwrap_or_default();

        letters.sort_by(|a, b| match (a.as_str(), b.as_str()) {
            ("#", _) => std::cmp::Ordering::Greater,
            (_, "#") => std::cmp::Ordering::Less,
            _ => a.cmp(b),
        });

        Ok(letters)
    }

    async fn get_available_tv_show_letters(
        &self,
        filters: TvShowSearchFilters,
    ) -> Result<Vec<String>, SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::TV_SHOWS);

        let filter_str = Self::build_tv_show_filter(&filters, None);

        let mut search = index.search();
        search.with_query("").with_limit(0);
        search.with_facets(meilisearch_sdk::search::Selectors::Some(&["first_letter"]));

        if let Some(ref filter) = filter_str {
            search.with_filter(filter);
        }

        let result = search.execute::<TvShowDocument>().await?;

        let mut letters: Vec<String> = result
            .facet_distribution
            .and_then(|fd| fd.get("first_letter").cloned())
            .map(|letter_counts| letter_counts.keys().cloned().collect())
            .unwrap_or_default();

        letters.sort_by(|a, b| match (a.as_str(), b.as_str()) {
            ("#", _) => std::cmp::Ordering::Greater,
            (_, "#") => std::cmp::Ordering::Less,
            _ => a.cmp(b),
        });

        Ok(letters)
    }

    async fn get_book_facets(
        &self,
        filters: BookSearchFilters,
    ) -> Result<LibraryFacets, SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::BOOKS);
        let filter_str = Self::build_book_filter(&filters, None);

        let mut search = index.search();
        search.with_query("").with_limit(0);
        search.with_facets(meilisearch_sdk::search::Selectors::Some(&[
            "genres",
            "want_status",
        ]));
        if let Some(ref filter) = filter_str {
            search.with_filter(filter);
        }

        let result = search.execute::<BookDocument>().await?;
        Ok(LibraryFacets {
            genres: facet_buckets(result.facet_distribution.as_ref(), "genres"),
            status: facet_buckets(result.facet_distribution.as_ref(), "want_status"),
        })
    }

    async fn get_movie_facets(
        &self,
        filters: MovieSearchFilters,
    ) -> Result<LibraryFacets, SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::MOVIES);
        let filter_str = Self::build_movie_filter(&filters, None);

        let mut search = index.search();
        search.with_query("").with_limit(0);
        search.with_facets(meilisearch_sdk::search::Selectors::Some(&[
            "genres",
            "want_status",
        ]));
        if let Some(ref filter) = filter_str {
            search.with_filter(filter);
        }

        let result = search.execute::<MovieDocument>().await?;
        Ok(LibraryFacets {
            genres: facet_buckets(result.facet_distribution.as_ref(), "genres"),
            status: facet_buckets(result.facet_distribution.as_ref(), "want_status"),
        })
    }

    async fn get_tv_show_facets(
        &self,
        filters: TvShowSearchFilters,
    ) -> Result<LibraryFacets, SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::TV_SHOWS);
        let filter_str = Self::build_tv_show_filter(&filters, None);

        let mut search = index.search();
        search.with_query("").with_limit(0);
        search.with_facets(meilisearch_sdk::search::Selectors::Some(&[
            "genres", "status",
        ]));
        if let Some(ref filter) = filter_str {
            search.with_filter(filter);
        }

        let result = search.execute::<TvShowDocument>().await?;
        Ok(LibraryFacets {
            genres: facet_buckets(result.facet_distribution.as_ref(), "genres"),
            status: facet_buckets(result.facet_distribution.as_ref(), "status"),
        })
    }

    async fn index_book(&self, doc: BookDocument) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::BOOKS);
        index.add_or_update(&[doc], Some("id")).await?;
        Ok(())
    }

    async fn index_author(&self, doc: AuthorDocument) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::AUTHORS);
        index.add_or_update(&[doc], Some("id")).await?;
        Ok(())
    }

    async fn index_series(&self, doc: SeriesDocument) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::SERIES);
        index.add_or_update(&[doc], Some("id")).await?;
        Ok(())
    }

    async fn delete_book(&self, id: Uuid) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::BOOKS);
        index.delete_document(&id.to_string()).await?;
        Ok(())
    }

    async fn delete_author(&self, id: Uuid) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::AUTHORS);
        index.delete_document(&id.to_string()).await?;
        Ok(())
    }

    async fn delete_series(&self, id: Uuid) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::SERIES);
        index.delete_document(&id.to_string()).await?;
        Ok(())
    }

    async fn index_genre(&self, doc: GenreDocument) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::GENRES);
        index.add_or_update(&[doc], Some("id")).await?;
        Ok(())
    }

    async fn delete_genre(&self, id: Uuid) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::GENRES);
        index.delete_document(&id.to_string()).await?;
        Ok(())
    }

    async fn index_movie(&self, doc: MovieDocument) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::MOVIES);
        index.add_or_update(&[doc], Some("id")).await?;
        Ok(())
    }

    async fn index_tv_show(&self, doc: TvShowDocument) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::TV_SHOWS);
        index.add_or_update(&[doc], Some("id")).await?;
        Ok(())
    }

    async fn delete_movie(&self, id: Uuid) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::MOVIES);
        index.delete_document(&id.to_string()).await?;
        Ok(())
    }

    async fn delete_tv_show(&self, id: Uuid) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::TV_SHOWS);
        index.delete_document(&id.to_string()).await?;
        Ok(())
    }

    async fn index_tv_season(&self, doc: TvSeasonDocument) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::TV_SEASONS);
        index.add_or_update(&[doc], Some("id")).await?;
        Ok(())
    }

    async fn index_tv_episode(&self, doc: TvEpisodeDocument) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::TV_EPISODES);
        index.add_or_update(&[doc], Some("id")).await?;
        Ok(())
    }

    async fn index_tv_seasons_batch(&self, docs: Vec<TvSeasonDocument>) -> Result<(), SearchError> {
        if docs.is_empty() {
            return Ok(());
        }
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::TV_SEASONS);
        index.add_or_update(&docs, Some("id")).await?;
        Ok(())
    }

    async fn index_tv_episodes_batch(
        &self,
        docs: Vec<TvEpisodeDocument>,
    ) -> Result<(), SearchError> {
        if docs.is_empty() {
            return Ok(());
        }
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::TV_EPISODES);
        index.add_or_update(&docs, Some("id")).await?;
        Ok(())
    }

    async fn delete_tv_season(&self, id: Uuid) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::TV_SEASONS);
        index.delete_document(&id.to_string()).await?;
        Ok(())
    }

    async fn delete_tv_episode(&self, id: Uuid) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::TV_EPISODES);
        index.delete_document(&id.to_string()).await?;
        Ok(())
    }

    async fn index_person(&self, doc: PersonDocument) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::PEOPLE);
        index.add_or_update(&[doc], Some("id")).await?;
        Ok(())
    }

    async fn delete_person(&self, id: Uuid) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::PEOPLE);
        index.delete_document(&id.to_string()).await?;
        Ok(())
    }

    async fn index_media_root_file(&self, doc: MediaRootFileDocument) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::MEDIA_ROOT_FILES);
        index.add_or_update(&[doc], Some("id")).await?;
        Ok(())
    }

    async fn index_media_root_files_batch(
        &self,
        docs: Vec<MediaRootFileDocument>,
    ) -> Result<(), SearchError> {
        if docs.is_empty() {
            return Ok(());
        }
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::MEDIA_ROOT_FILES);
        index.add_or_update(&docs, Some("id")).await?;
        Ok(())
    }

    async fn delete_media_root_file(&self, id: Uuid) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::MEDIA_ROOT_FILES);
        index.delete_document(&id.to_string()).await?;
        Ok(())
    }

    async fn search_media_root_files(
        &self,
        query: &str,
        media_root_id: Uuid,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<MediaRootFileDocument>, SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::MEDIA_ROOT_FILES);

        let filter = format!("media_root_id = \"{}\"", media_root_id);

        let mut search = index.search();
        search
            .with_query(query)
            .with_limit(limit)
            .with_offset(offset)
            .with_filter(&filter)
            .with_matching_strategy(meilisearch_sdk::search::MatchingStrategies::ALL);

        let result = search.execute::<MediaRootFileDocument>().await?;

        Ok(result.hits.into_iter().map(|h| h.result).collect())
    }

    async fn reindex_all_media_root_files(
        &self,
        docs: Vec<MediaRootFileDocument>,
    ) -> Result<usize, SearchError> {
        let count = docs.len();
        if count > 0 {
            let (client, _url) = self.get_client().await;
            let index = client.index(indexes::MEDIA_ROOT_FILES);
            index.delete_all_documents().await?;
            index.add_or_update(&docs, Some("id")).await?;
            info!(count, "Reindexed all media root files");
        }
        Ok(count)
    }

    async fn index_page(&self, doc: PageDocument) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::PAGES);
        index.add_or_update(&[doc], Some("id")).await?;
        Ok(())
    }

    async fn index_command(&self, doc: CommandDocument) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::COMMANDS);
        index.add_or_update(&[doc], Some("id")).await?;
        Ok(())
    }

    async fn reindex_all_books(&self, docs: Vec<BookDocument>) -> Result<usize, SearchError> {
        let count = docs.len();
        if count > 0 {
            let (client, _url) = self.get_client().await;
            let index = client.index(indexes::BOOKS);
            index.delete_all_documents().await?;
            index.add_or_update(&docs, Some("id")).await?;
            info!(count, "Reindexed all books");
        }
        Ok(count)
    }

    async fn reindex_all_authors(&self, docs: Vec<AuthorDocument>) -> Result<usize, SearchError> {
        let count = docs.len();
        if count > 0 {
            let (client, _url) = self.get_client().await;
            let index = client.index(indexes::AUTHORS);
            index.delete_all_documents().await?;
            index.add_or_update(&docs, Some("id")).await?;
            info!(count, "Reindexed all authors");
        }
        Ok(count)
    }

    async fn reindex_all_series(&self, docs: Vec<SeriesDocument>) -> Result<usize, SearchError> {
        let count = docs.len();
        if count > 0 {
            let (client, _url) = self.get_client().await;
            let index = client.index(indexes::SERIES);
            index.delete_all_documents().await?;
            index.add_or_update(&docs, Some("id")).await?;
            info!(count, "Reindexed all series");
        }
        Ok(count)
    }

    async fn reindex_all_genres(&self, docs: Vec<GenreDocument>) -> Result<usize, SearchError> {
        let count = docs.len();
        if count > 0 {
            let (client, _url) = self.get_client().await;
            let index = client.index(indexes::GENRES);
            index.delete_all_documents().await?;
            index.add_or_update(&docs, Some("id")).await?;
            info!(count, "Reindexed all genres");
        }
        Ok(count)
    }

    async fn reindex_all_movies(&self, docs: Vec<MovieDocument>) -> Result<usize, SearchError> {
        let count = docs.len();
        if count > 0 {
            let (client, _url) = self.get_client().await;
            let index = client.index(indexes::MOVIES);
            index.delete_all_documents().await?;
            index.add_or_update(&docs, Some("id")).await?;
            info!(count, "Reindexed all movies");
        }
        Ok(count)
    }

    async fn reindex_all_tv_shows(&self, docs: Vec<TvShowDocument>) -> Result<usize, SearchError> {
        let count = docs.len();
        if count > 0 {
            let (client, _url) = self.get_client().await;
            let index = client.index(indexes::TV_SHOWS);
            index.delete_all_documents().await?;
            index.add_or_update(&docs, Some("id")).await?;
            info!(count, "Reindexed all TV shows");
        }
        Ok(count)
    }

    async fn reindex_all_tv_seasons(
        &self,
        docs: Vec<TvSeasonDocument>,
    ) -> Result<usize, SearchError> {
        let count = docs.len();
        if count > 0 {
            let (client, _url) = self.get_client().await;
            let index = client.index(indexes::TV_SEASONS);
            index.delete_all_documents().await?;
            index.add_or_update(&docs, Some("id")).await?;
            info!(count, "Reindexed all TV seasons");
        }
        Ok(count)
    }

    async fn reindex_all_tv_episodes(
        &self,
        docs: Vec<TvEpisodeDocument>,
    ) -> Result<usize, SearchError> {
        let count = docs.len();
        if count > 0 {
            let (client, _url) = self.get_client().await;
            let index = client.index(indexes::TV_EPISODES);
            index.delete_all_documents().await?;
            index.add_or_update(&docs, Some("id")).await?;
            info!(count, "Reindexed all TV episodes");
        }
        Ok(count)
    }

    async fn reindex_all_people(&self, docs: Vec<PersonDocument>) -> Result<usize, SearchError> {
        let count = docs.len();
        if count > 0 {
            let (client, _url) = self.get_client().await;
            let index = client.index(indexes::PEOPLE);
            index.delete_all_documents().await?;
            index.add_or_update(&docs, Some("id")).await?;
            info!(count, "Reindexed all people");
        }
        Ok(count)
    }

    async fn reindex_all_pages(&self, docs: Vec<PageDocument>) -> Result<usize, SearchError> {
        let count = docs.len();
        if count > 0 {
            let (client, _url) = self.get_client().await;
            let index = client.index(indexes::PAGES);
            index.delete_all_documents().await?;
            index.add_or_update(&docs, Some("id")).await?;
            info!(count, "Reindexed all pages");
        }
        Ok(count)
    }

    async fn reindex_all_commands(&self, docs: Vec<CommandDocument>) -> Result<usize, SearchError> {
        let count = docs.len();
        if count > 0 {
            let (client, _url) = self.get_client().await;
            let index = client.index(indexes::COMMANDS);
            index.delete_all_documents().await?;
            index.add_or_update(&docs, Some("id")).await?;
            info!(count, "Reindexed all commands");
        }
        Ok(count)
    }

    async fn reindex_all_settings(&self, docs: Vec<SettingDocument>) -> Result<usize, SearchError> {
        let count = docs.len();
        if count > 0 {
            let (client, _url) = self.get_client().await;
            let index = client.index(indexes::SETTINGS);
            index.delete_all_documents().await?;
            index.add_or_update(&docs, Some("id")).await?;
            info!(count, "Reindexed all settings");
        }
        Ok(count)
    }

    async fn reindex_all_iptv_channels(
        &self,
        docs: Vec<IptvChannelDocument>,
    ) -> Result<usize, SearchError> {
        let count = docs.len();
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::IPTV_CHANNELS);
        index.delete_all_documents().await?;
        if count > 0 {
            index.add_or_update(&docs, Some("id")).await?;
        }
        info!(count, "Reindexed all IPTV channels");
        Ok(count)
    }

    async fn reindex_all_iptv_epg(&self, docs: Vec<IptvEpgDocument>) -> Result<usize, SearchError> {
        let count = docs.len();
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::IPTV_EPG);
        index.delete_all_documents().await?;
        if count > 0 {
            index.add_or_update(&docs, Some("id")).await?;
        }
        info!(count, "Reindexed all IPTV EPG entries");
        Ok(count)
    }

    async fn reindex_all_livetv_channels(
        &self,
        docs: Vec<LivetvChannelDocument>,
    ) -> Result<usize, SearchError> {
        let count = docs.len();
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::LIVETV_CHANNELS);
        index.delete_all_documents().await?;
        if count > 0 {
            index.add_or_update(&docs, Some("id")).await?;
        }
        info!(count, "Reindexed all Live TV channels");
        Ok(count)
    }

    async fn reindex_all_livetv_guide(
        &self,
        docs: Vec<LivetvGuideDocument>,
    ) -> Result<usize, SearchError> {
        let count = docs.len();
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::LIVETV_GUIDE);
        index.delete_all_documents().await?;
        if count > 0 {
            index.add_or_update(&docs, Some("id")).await?;
        }
        info!(count, "Reindexed all Live TV guide entries");
        Ok(count)
    }

    async fn search_transcript_segments(
        &self,
        query: &str,
        book_id: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<TranscriptSegmentDocument>, SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::TRANSCRIPT_SEGMENTS);

        let filter = book_id.map(|bid| format!("book_id = \"{}\"", bid));

        let mut search = index.search();
        search.with_query(query).with_limit(limit);

        if let Some(ref f) = filter {
            search.with_filter(f);
        }

        let result = search.execute::<TranscriptSegmentDocument>().await?;
        Ok(result.hits.into_iter().map(|h| h.result).collect())
    }

    async fn index_transcript_segments(
        &self,
        docs: Vec<TranscriptSegmentDocument>,
    ) -> Result<(), SearchError> {
        if !docs.is_empty() {
            let (client, _url) = self.get_client().await;
            let index = client.index(indexes::TRANSCRIPT_SEGMENTS);
            index.add_or_update(&docs, Some("id")).await?;
        }
        Ok(())
    }

    async fn delete_transcript_segments_for_book(&self, book_id: &str) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::TRANSCRIPT_SEGMENTS);
        let filter = format!("book_id = \"{}\"", book_id);
        let mut query = meilisearch_sdk::documents::DocumentDeletionQuery::new(&index);
        query.with_filter(&filter);
        query.execute::<serde_json::Value>().await?;
        Ok(())
    }

    async fn search_iptv_channels(
        &self,
        query: &str,
        filters: &IptvChannelSearchFilters,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<String>, SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::IPTV_CHANNELS);

        let mut parts = vec![];
        if let Some(ref cat) = filters.category {
            parts.push(format!("category = \"{}\"", cat));
        }
        if let Some(ref group) = filters.group {
            parts.push(format!("group = \"{}\"", group));
        }
        if let Some(ref langs) = filters.languages {
            if langs.len() == 1 {
                parts.push(format!("language = \"{}\"", langs[0]));
            } else if langs.len() > 1 {
                let or_parts: Vec<String> = langs
                    .iter()
                    .map(|l| format!("language = \"{}\"", l))
                    .collect();
                parts.push(format!("({})", or_parts.join(" OR ")));
            }
        }
        if let Some(enabled) = filters.enabled {
            parts.push(format!("enabled = {}", enabled));
        }
        let filter = if parts.is_empty() {
            None
        } else {
            Some(parts.join(" AND "))
        };

        let mut search = index.search();
        search
            .with_query(query)
            .with_limit(limit)
            .with_offset(offset);
        if let Some(ref f) = filter {
            search.with_filter(f);
        }
        let results = search.execute::<IptvChannelDocument>().await?;
        Ok(results.hits.into_iter().map(|h| h.result.id).collect())
    }

    async fn index_iptv_channels(&self, docs: &[IptvChannelDocument]) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::IPTV_CHANNELS);
        index.add_or_update(docs, Some("id")).await?;
        Ok(())
    }

    async fn delete_iptv_channel(&self, id: &str) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::IPTV_CHANNELS);
        index.delete_document(id).await?;
        Ok(())
    }

    async fn index_iptv_epg(&self, docs: &[IptvEpgDocument]) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::IPTV_EPG);
        index.add_or_update(docs, Some("id")).await?;
        Ok(())
    }

    async fn search_iptv_epg(
        &self,
        query: &str,
        filters: &IptvEpgSearchFilters,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<IptvEpgDocument>, SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::IPTV_EPG);

        let mut parts = vec![];
        if let Some(ref channel_id) = filters.channel_id {
            parts.push(format!("channel_id = \"{}\"", channel_id));
        }
        if let Some(ref categories) = filters.categories {
            let cat_filters: Vec<String> = categories
                .iter()
                .map(|c| format!("categories = \"{}\"", c))
                .collect();
            if !cat_filters.is_empty() {
                parts.push(format!("({})", cat_filters.join(" OR ")));
            }
        }
        if let Some(is_live) = filters.is_live {
            parts.push(format!("is_live = {}", is_live));
        }
        if let Some(start_after) = filters.start_after {
            parts.push(format!("start_time >= {}", start_after));
        }
        if let Some(start_before) = filters.start_before {
            parts.push(format!("start_time <= {}", start_before));
        }
        let filter = if parts.is_empty() {
            None
        } else {
            Some(parts.join(" AND "))
        };

        let mut search = index.search();
        search
            .with_query(query)
            .with_limit(limit)
            .with_offset(offset);
        if let Some(ref f) = filter {
            search.with_filter(f);
        }
        let results = search.execute::<IptvEpgDocument>().await?;
        Ok(results.hits.into_iter().map(|h| h.result).collect())
    }

    async fn search_livetv_channels(
        &self,
        query: &str,
        filters: &LivetvChannelSearchFilters,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<String>, SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::LIVETV_CHANNELS);

        let mut parts = vec![];
        if let Some(ref v) = filters.network {
            parts.push(format!("network = \"{}\"", v));
        }
        if let Some(ref v) = filters.region {
            parts.push(format!("region = \"{}\"", v));
        }
        if let Some(ref v) = filters.kind {
            parts.push(format!("kind = \"{}\"", v));
        }
        if let Some(v) = filters.enabled {
            parts.push(format!("enabled = {}", v));
        }
        if let Some(v) = filters.favourite {
            parts.push(format!("favourite = {}", v));
        }
        if let Some(v) = filters.hd {
            parts.push(format!("hd = {}", v));
        }

        let mut search = index.search();
        search
            .with_query(query)
            .with_limit(limit)
            .with_offset(offset);
        let filter = parts.join(" AND ");
        if !filter.is_empty() {
            search.with_filter(&filter);
        }
        let results = search.execute::<LivetvChannelDocument>().await?;
        Ok(results.hits.into_iter().map(|h| h.result.id).collect())
    }

    async fn index_livetv_channels(
        &self,
        docs: &[LivetvChannelDocument],
    ) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::LIVETV_CHANNELS);
        index.add_or_update(docs, Some("id")).await?;
        Ok(())
    }

    async fn delete_livetv_channel(&self, id: &str) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::LIVETV_CHANNELS);
        index.delete_document(id).await?;
        Ok(())
    }

    async fn index_livetv_guide(&self, docs: &[LivetvGuideDocument]) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::LIVETV_GUIDE);
        index.add_or_update(docs, Some("id")).await?;
        Ok(())
    }

    async fn search_livetv_guide(
        &self,
        query: &str,
        filters: &LivetvGuideSearchFilters,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<LivetvGuideDocument>, SearchError> {
        let (client, _url) = self.get_client().await;
        let index = client.index(indexes::LIVETV_GUIDE);

        let mut parts = vec![];
        if let Some(ref v) = filters.channel_id {
            parts.push(format!("channel_id = \"{}\"", v));
        }
        if let Some(ref genres) = filters.genres {
            let ors: Vec<String> = genres
                .iter()
                .map(|g| format!("genres = \"{}\"", g))
                .collect();
            if !ors.is_empty() {
                parts.push(format!("({})", ors.join(" OR ")));
            }
        }
        if let Some(v) = filters.start_after {
            parts.push(format!("start_time >= {}", v));
        }
        if let Some(v) = filters.start_before {
            parts.push(format!("start_time <= {}", v));
        }

        let mut search = index.search();
        // Episode titles and synopses are prose: a word in one ("Breaking the Ice")
        // surfaces a programme nobody searched for.
        search
            .with_query(query)
            .with_attributes_to_search_on(&["title"])
            .with_limit(limit)
            .with_offset(offset);
        let filter = parts.join(" AND ");
        if !filter.is_empty() {
            search.with_filter(&filter);
        }
        let results = search.execute::<LivetvGuideDocument>().await?;
        Ok(results.hits.into_iter().map(|h| h.result).collect())
    }

    async fn init_indexes(&self) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        // Create indexes if they don't exist
        let books_index = client.index(indexes::BOOKS);
        let authors_index = client.index(indexes::AUTHORS);
        let series_index = client.index(indexes::SERIES);
        let genres_index = client.index(indexes::GENRES);

        // Synonyms so "&" and "and" are interchangeable in searches
        // (e.g. "Law & Order" ↔ "Law and Order", "Simon & Schuster")
        let synonyms: HashMap<&str, Vec<&str>> =
            HashMap::from([("&", vec!["and"]), ("and", vec!["&"])]);

        // Configure books index
        // IDs are filterable (not searchable) for exact UUID lookups
        // Text fields are searchable for fuzzy matching (including genres)
        // first_letter is filterable for alphabet bar faceting
        // Ranking rules: sort is last so it only affects tie-breaking, not primary relevance
        let books_task = books_index
            .set_settings(
                &meilisearch_sdk::settings::Settings::new()
                    .with_searchable_attributes([
                        "title",
                        "authors",
                        "series_name",
                        "all_series_names",
                        "genres",
                        "narrator",
                    ])
                    .with_filterable_attributes([
                        "is_in_library",
                        "want_status",
                        "ingest_status",
                        "id",
                        "author_ids",
                        "series_id",
                        "first_letter",
                        "has_audiobook",
                        "has_ebook",
                        "genres",
                    ])
                    .with_sortable_attributes(["title", "series_name", "series_index", "id"])
                    .with_ranking_rules([
                        "words",
                        "typo",
                        "proximity",
                        "attributeRank",
                        "wordPosition",
                        "exactness",
                        "sort",
                    ])
                    .with_synonyms(synonyms.clone())
                    .with_non_separation_tokens(["&"]),
            )
            .await?;
        info!(
            task_uid = books_task.task_uid,
            "Books index settings updated"
        );

        // Configure authors index
        // ID is filterable for exact UUID lookups
        let authors_task = authors_index
            .set_settings(
                &meilisearch_sdk::settings::Settings::new()
                    .with_searchable_attributes(["name"])
                    .with_filterable_attributes(["is_monitored", "id"])
                    .with_sortable_attributes(["name", "book_count"])
                    .with_synonyms(synonyms.clone())
                    .with_non_separation_tokens(["&"]),
            )
            .await?;
        info!(
            task_uid = authors_task.task_uid,
            "Authors index settings updated"
        );

        // Configure series index
        // ID is filterable for exact UUID lookups
        let series_task = series_index
            .set_settings(
                &meilisearch_sdk::settings::Settings::new()
                    .with_searchable_attributes(["name"])
                    .with_filterable_attributes(["is_monitored", "id"])
                    .with_sortable_attributes(["name", "book_count"])
                    .with_synonyms(synonyms.clone())
                    .with_non_separation_tokens(["&"]),
            )
            .await?;
        info!(
            task_uid = series_task.task_uid,
            "Series index settings updated"
        );

        // Configure genres index
        let genres_task = genres_index
            .set_settings(
                &meilisearch_sdk::settings::Settings::new()
                    .with_searchable_attributes(["name"])
                    .with_sortable_attributes(["name", "book_count"])
                    .with_synonyms(synonyms.clone())
                    .with_non_separation_tokens(["&"]),
            )
            .await?;
        info!(
            task_uid = genres_task.task_uid,
            "Genres index settings updated"
        );

        // Configure pages index
        let pages_index = client.index(indexes::PAGES);
        let pages_task = pages_index
            .set_settings(
                &meilisearch_sdk::settings::Settings::new()
                    .with_searchable_attributes(["label", "aliases", "path"])
                    .with_sortable_attributes(["label"]),
            )
            .await?;
        info!(
            task_uid = pages_task.task_uid,
            "Pages index settings updated"
        );

        // Configure commands index
        let commands_index = client.index(indexes::COMMANDS);
        let commands_task = commands_index
            .set_settings(
                &meilisearch_sdk::settings::Settings::new()
                    .with_searchable_attributes(["label", "aliases"])
                    .with_sortable_attributes(["label"]),
            )
            .await?;
        info!(
            task_uid = commands_task.task_uid,
            "Commands index settings updated"
        );

        // Configure settings index
        let settings_index = client.index(indexes::SETTINGS);
        let settings_task = settings_index
            .set_settings(
                &meilisearch_sdk::settings::Settings::new()
                    .with_searchable_attributes([
                        "label",
                        "description",
                        "aliases",
                        "page_label",
                        "field_name",
                    ])
                    .with_sortable_attributes(["label"]),
            )
            .await?;
        info!(
            task_uid = settings_task.task_uid,
            "Settings index settings updated"
        );

        // Configure movies index
        let movies_index = client.index(indexes::MOVIES);
        let movies_task = movies_index
            .set_settings(
                &meilisearch_sdk::settings::Settings::new()
                    .with_searchable_attributes(["title", "directors", "genres"])
                    .with_filterable_attributes([
                        "is_in_library",
                        "want_status",
                        "first_letter",
                        "id",
                        "genres",
                        "year",
                        "rating",
                        "runtime",
                        "content_rating",
                    ])
                    .with_sortable_attributes(["title", "year", "rating", "runtime", "id"])
                    .with_synonyms(synonyms.clone())
                    .with_non_separation_tokens(["&"]),
            )
            .await?;
        info!(
            task_uid = movies_task.task_uid,
            "Movies index settings updated"
        );

        // Configure TV shows index
        let tv_shows_index = client.index(indexes::TV_SHOWS);
        let tv_shows_task = tv_shows_index
            .set_settings(
                &meilisearch_sdk::settings::Settings::new()
                    .with_searchable_attributes(["title", "genres", "network"])
                    .with_filterable_attributes([
                        "is_monitored",
                        "status",
                        "first_letter",
                        "id",
                        "genres",
                        "year",
                        "rating",
                        "content_rating",
                        "available_episode_count",
                        "kind",
                    ])
                    .with_sortable_attributes(["title", "year", "rating", "id"])
                    .with_synonyms(synonyms.clone())
                    .with_non_separation_tokens(["&"]),
            )
            .await?;
        info!(
            task_uid = tv_shows_task.task_uid,
            "TV shows index settings updated"
        );

        // Configure TV seasons index
        let tv_seasons_index = client.index(indexes::TV_SEASONS);
        let tv_seasons_task = tv_seasons_index
            .set_settings(
                &meilisearch_sdk::settings::Settings::new()
                    .with_searchable_attributes([
                        "display_title",
                        "show_title",
                        "season_title",
                        "season_code",
                    ])
                    .with_filterable_attributes(["show_id", "is_monitored", "is_special", "id"])
                    .with_sortable_attributes(["display_title"])
                    .with_synonyms(synonyms.clone())
                    .with_non_separation_tokens(["&"]),
            )
            .await?;
        info!(
            task_uid = tv_seasons_task.task_uid,
            "TV seasons index settings updated"
        );

        // Configure TV episodes index
        let tv_episodes_index = client.index(indexes::TV_EPISODES);
        let tv_episodes_task = tv_episodes_index
            .set_settings(
                &meilisearch_sdk::settings::Settings::new()
                    .with_searchable_attributes([
                        "display_title",
                        "show_title",
                        "episode_title",
                        "episode_code",
                    ])
                    .with_filterable_attributes([
                        "show_id",
                        "season_id",
                        "is_in_library",
                        "want_status",
                        "id",
                        "air_date_ts",
                        "season_sort_order",
                        "episode_sort_order",
                    ])
                    .with_sortable_attributes([
                        "display_title",
                        "air_date_ts",
                        "season_sort_order",
                        "episode_sort_order",
                    ])
                    .with_synonyms(synonyms.clone())
                    .with_non_separation_tokens(["&"]),
            )
            .await?;
        info!(
            task_uid = tv_episodes_task.task_uid,
            "TV episodes index settings updated"
        );

        // Configure media root files index
        let media_root_files_index = client.index(indexes::MEDIA_ROOT_FILES);
        let media_root_files_task = media_root_files_index
            .set_settings(
                &meilisearch_sdk::settings::Settings::new()
                    .with_searchable_attributes([
                        "file_name",
                        "file_path",
                        "show_name",
                        "movie_title",
                        "book_title",
                        "series_name",
                    ])
                    .with_filterable_attributes(["media_root_id", "match_status", "entity_type"])
                    .with_sortable_attributes(["file_name"])
                    .with_typo_tolerance(meilisearch_sdk::settings::TypoToleranceSettings {
                        enabled: Some(true),
                        min_word_size_for_typos: Some(
                            meilisearch_sdk::settings::MinWordSizeForTypos {
                                one_typo: Some(6),
                                two_typos: Some(12),
                            },
                        ),
                        disable_on_numbers: Some(true),
                        ..Default::default()
                    })
                    .with_synonyms(synonyms.clone())
                    .with_non_separation_tokens(["&"]),
            )
            .await?;
        info!(
            task_uid = media_root_files_task.task_uid,
            "Media root files index settings updated"
        );

        // Configure people index
        let people_index = client.index(indexes::PEOPLE);
        let people_task = people_index
            .set_settings(
                &meilisearch_sdk::settings::Settings::new()
                    .with_searchable_attributes(["name", "sort_name"])
                    .with_filterable_attributes(["id"])
                    .with_sortable_attributes(["name", "movie_count", "tv_show_count"])
                    .with_synonyms(synonyms.clone())
                    .with_non_separation_tokens(["&"]),
            )
            .await?;
        info!(
            task_uid = people_task.task_uid,
            "People index settings updated"
        );

        let transcript_segments_index = client.index(indexes::TRANSCRIPT_SEGMENTS);
        let transcript_segments_task = transcript_segments_index
            .set_settings(
                &meilisearch_sdk::settings::Settings::new()
                    .with_searchable_attributes(["text"])
                    .with_filterable_attributes(["book_id", "source_type"])
                    .with_sortable_attributes(["start_seconds", "segment_index"]),
            )
            .await?;
        info!(
            task_uid = transcript_segments_task.task_uid,
            "Transcript segments index settings updated"
        );

        // Configure IPTV channels index
        let iptv_channels_index = client.index(indexes::IPTV_CHANNELS);
        let iptv_channels_task = iptv_channels_index
            .set_settings(
                &meilisearch_sdk::settings::Settings::new()
                    .with_searchable_attributes(["name", "category", "current_program"])
                    .with_filterable_attributes([
                        "category", "group", "language", "country", "enabled", "has_epg",
                    ])
                    .with_sortable_attributes(["name", "source_count"])
                    .with_ranking_rules([
                        "exactness",
                        "words",
                        "typo",
                        "proximity",
                        "attribute",
                        "sort",
                    ])
                    .with_typo_tolerance(meilisearch_sdk::settings::TypoToleranceSettings {
                        enabled: Some(true),
                        min_word_size_for_typos: Some(
                            meilisearch_sdk::settings::MinWordSizeForTypos {
                                one_typo: Some(4),
                                two_typos: Some(8),
                            },
                        ),
                        ..Default::default()
                    })
                    .with_synonyms(synonyms.clone())
                    .with_non_separation_tokens(["&"]),
            )
            .await?;
        info!(
            task_uid = iptv_channels_task.task_uid,
            "IPTV channels index settings updated"
        );

        // Configure IPTV EPG index
        let iptv_epg_index = client.index(indexes::IPTV_EPG);
        let iptv_epg_task = iptv_epg_index
            .set_settings(
                &meilisearch_sdk::settings::Settings::new()
                    .with_searchable_attributes([
                        "title",
                        "description",
                        "channel_name",
                        "categories",
                    ])
                    .with_filterable_attributes([
                        "channel_id",
                        "categories",
                        "start_time",
                        "stop_time",
                        "is_live",
                    ])
                    .with_sortable_attributes(["start_time", "stop_time"])
                    .with_ranking_rules([
                        "exactness",
                        "words",
                        "typo",
                        "proximity",
                        "attribute",
                        "sort",
                    ])
                    .with_synonyms(synonyms.clone())
                    .with_non_separation_tokens(["&"]),
            )
            .await?;
        info!(
            task_uid = iptv_epg_task.task_uid,
            "IPTV EPG index settings updated"
        );

        // Configure Live TV channels index
        let livetv_channels_index = client.index(indexes::LIVETV_CHANNELS);
        let livetv_channels_task = livetv_channels_index
            .set_settings(
                &meilisearch_sdk::settings::Settings::new()
                    .with_searchable_attributes([
                        "name",
                        "scan_name",
                        "callsign",
                        "network",
                        "current_program",
                    ])
                    .with_filterable_attributes([
                        "network",
                        "region",
                        "kind",
                        "enabled",
                        "favourite",
                        "hd",
                        "has_guide",
                        "mux_label",
                    ])
                    .with_sortable_attributes(["name", "lcn"])
                    .with_ranking_rules([
                        "exactness",
                        "words",
                        "typo",
                        "proximity",
                        "attribute",
                        "sort",
                    ])
                    .with_synonyms(synonyms.clone())
                    .with_non_separation_tokens(["&"]),
            )
            .await?;
        info!(
            task_uid = livetv_channels_task.task_uid,
            "Live TV channels index settings updated"
        );

        // Configure Live TV guide index
        let livetv_guide_index = client.index(indexes::LIVETV_GUIDE);
        let livetv_guide_task = livetv_guide_index
            .set_settings(
                &meilisearch_sdk::settings::Settings::new()
                    .with_searchable_attributes([
                        "title",
                        "episode_title",
                        "synopsis",
                        "channel_name",
                        "genres",
                    ])
                    .with_filterable_attributes(["channel_id", "start_time", "genres"])
                    .with_sortable_attributes(["start_time", "end_time"])
                    .with_ranking_rules([
                        "exactness",
                        "words",
                        "typo",
                        "proximity",
                        "attribute",
                        "sort",
                    ])
                    .with_synonyms(synonyms.clone())
                    .with_non_separation_tokens(["&"]),
            )
            .await?;
        info!(
            task_uid = livetv_guide_task.task_uid,
            "Live TV guide index settings updated"
        );

        Ok(())
    }

    async fn status(&self) -> SearchBackendStatus {
        let (client, url) = self.get_client().await;
        // Try to get index stats to verify connection
        let books_index = client.index(indexes::BOOKS);
        let authors_index = client.index(indexes::AUTHORS);
        let series_index = client.index(indexes::SERIES);
        let genres_index = client.index(indexes::GENRES);
        let movies_index = client.index(indexes::MOVIES);
        let tv_shows_index = client.index(indexes::TV_SHOWS);
        let tv_seasons_index = client.index(indexes::TV_SEASONS);
        let tv_episodes_index = client.index(indexes::TV_EPISODES);
        let people_index = client.index(indexes::PEOPLE);

        // Get stats for each index
        let books_stats = books_index.get_stats().await;
        let authors_stats = authors_index.get_stats().await;
        let series_stats = series_index.get_stats().await;
        let genres_stats = genres_index.get_stats().await;
        let movies_stats = movies_index.get_stats().await;
        let tv_shows_stats = tv_shows_index.get_stats().await;
        let tv_seasons_stats = tv_seasons_index.get_stats().await;
        let tv_episodes_stats = tv_episodes_index.get_stats().await;
        let people_stats = people_index.get_stats().await;

        // Check if we got valid stats from at least one index
        let connected = books_stats.is_ok()
            || authors_stats.is_ok()
            || series_stats.is_ok()
            || genres_stats.is_ok()
            || movies_stats.is_ok()
            || tv_shows_stats.is_ok()
            || people_stats.is_ok();

        let error = if connected {
            None
        } else {
            Some("Failed to connect to MeiliSearch".to_string())
        };

        let embedded_document_count = connected.then(|| {
            [&books_stats, &movies_stats, &tv_shows_stats]
                .into_iter()
                .filter_map(|stats| stats.as_ref().ok())
                .map(|stats| stats.number_of_embedded_documents as i64)
                .sum()
        });

        SearchBackendStatus {
            backend_type: "meilisearch".to_string(),
            connected,
            url,
            book_count: books_stats.ok().map(|s| s.number_of_documents as i64),
            author_count: authors_stats.ok().map(|s| s.number_of_documents as i64),
            series_count: series_stats.ok().map(|s| s.number_of_documents as i64),
            genre_count: genres_stats.ok().map(|s| s.number_of_documents as i64),
            movie_count: movies_stats.ok().map(|s| s.number_of_documents as i64),
            tv_show_count: tv_shows_stats.ok().map(|s| s.number_of_documents as i64),
            tv_season_count: tv_seasons_stats.ok().map(|s| s.number_of_documents as i64),
            tv_episode_count: tv_episodes_stats.ok().map(|s| s.number_of_documents as i64),
            person_count: people_stats.ok().map(|s| s.number_of_documents as i64),
            embedded_document_count,
            error,
        }
    }

    async fn reconfigure(&self, url: &str, master_key: &str) -> Result<(), SearchError> {
        // Create a new client with the new settings
        let new_client = build_client(url, master_key)?;

        // Test the connection by trying to get health
        // MeiliSearch client doesn't have a direct health check, so we try to list indexes
        new_client
            .list_all_indexes()
            .await
            .map_err(|e| SearchError::Connection(format!("Failed to connect: {}", e)))?;

        // Connection successful, update the inner state
        let mut inner = self.inner.write().await;
        inner.client = new_client;
        inner.url = url.to_string();

        info!(url = %url, "MeiliSearch reconfigured");
        Ok(())
    }

    fn supports_semantic(&self) -> bool {
        true
    }

    fn semantic_enabled(&self) -> bool {
        self.semantic.load(Ordering::Relaxed)
    }

    fn set_semantic_enabled(&self, enabled: bool) {
        self.semantic.store(enabled, Ordering::Relaxed);
    }

    /// Every title index has exactly the current embedders at the current width.
    /// A name check alone would keep a previous model's embedder (e.g. a
    /// 1152-d `poster`) that rejects every 768-d document.
    async fn embedders_configured(&self) -> Result<bool, SearchError> {
        let (client, _url) = self.get_client().await;
        for uid in semantic::INDEXES {
            let embedders = client.index(uid).get_embedders().await?;
            let current = embedders.len() == semantic::EMBEDDERS.len()
                && semantic::EMBEDDERS.iter().all(|name| {
                    embedders
                        .get(*name)
                        .is_some_and(|e| e.dimensions == Some(semantic::DIMENSIONS))
                });
            if !current {
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn has_embedders(&self) -> Result<bool, SearchError> {
        let (client, _url) = self.get_client().await;
        for uid in semantic::INDEXES {
            if !client.index(uid).get_embedders().await?.is_empty() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn apply_embedders(&self, enabled: bool) -> Result<(), SearchError> {
        let (client, _url) = self.get_client().await;
        let embedder = |distribution: crate::embedding::profile::Distribution| {
            meilisearch_sdk::settings::Embedder {
                source: meilisearch_sdk::settings::EmbedderSource::UserProvided,
                dimensions: Some(semantic::DIMENSIONS),
                distribution: Some(meilisearch_sdk::settings::EmbedderDistribution {
                    mean: distribution.mean,
                    sigma: distribution.sigma,
                }),
                ..Default::default()
            }
        };
        let embedders = HashMap::from([
            (
                semantic::DESCRIPTION_EMBEDDER.to_string(),
                embedder(semantic::description_distribution()),
            ),
            (
                semantic::POSTER_EMBEDDER.to_string(),
                embedder(semantic::poster_distribution()),
            ),
        ]);
        for uid in semantic::INDEXES {
            let index = client.index(uid);
            let task = match enabled {
                true => index.set_embedders(&embedders).await?,
                false => index.reset_embedders().await?,
            };
            // Generous: Meili reindexes every vector of the index before the task settles.
            let task = task
                .wait_for_completion(&client, None, Some(Duration::from_secs(600)))
                .await?;
            if task.is_failure() {
                return Err(SearchError::Backend(format!(
                    "Meilisearch rejected the {uid} embedder: {}",
                    task.unwrap_failure()
                )));
            }
        }
        match enabled {
            true => info!("Search embedders declared"),
            // Also the first step of a model change, before the new ones are declared.
            false => info!("Search embedders removed"),
        }
        Ok(())
    }
}
