//! In-memory stand-ins for Meili, Postgres and the metadata providers, so the
//! phrasing table runs with no services and no network.
//!
//! The same shape as `search::SqlFallbackBackend`: shipped in the library rather
//! than behind `cfg(test)`, because the integration test that consumes it does
//! not see `cfg(test)` code.

pub mod data;

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::index::{IndexHits, IndexKind, IndexQuery, LibraryIndex};
use super::providers::{MetadataLookup, ProviderHit};
use super::store::{BookBrief, ResolveStore};
use super::text;
use super::types::{FormatSkew, ProviderPayload, ResolveError, ShowSeason};

// =============================================================================
// Index
// =============================================================================

/// A Meili stand-in.
///
/// Matching mirrors the real settings: only `searchableAttributes` are
/// searched, `matchingStrategy: all` so every query term must be present,
/// prefix matching on the final term, and Meili's default typo budget. It is
/// deliberately a shade stricter than Meili, so the synopsis pass gets
/// exercised rather than papered over by a lucky fuzzy hit.
pub struct FixtureIndex {
    corpus: IndexHits,
}

impl Default for FixtureIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl FixtureIndex {
    pub fn new() -> Self {
        Self {
            corpus: data::corpus(),
        }
    }

    pub fn with_corpus(corpus: IndexHits) -> Self {
        Self { corpus }
    }

    fn values(&self, kind: IndexKind) -> Vec<Value> {
        fn encode<T: Serialize>(docs: &[T]) -> Vec<Value> {
            docs.iter()
                .filter_map(|d| serde_json::to_value(d).ok())
                .collect()
        }
        match kind {
            IndexKind::Books => encode(&self.corpus.books),
            IndexKind::Movies => encode(&self.corpus.movies),
            IndexKind::TvShows => encode(&self.corpus.shows),
            IndexKind::TvSeasons => encode(&self.corpus.seasons),
            IndexKind::TvEpisodes => encode(&self.corpus.episodes),
            IndexKind::Authors => encode(&self.corpus.authors),
            IndexKind::Series => encode(&self.corpus.series),
        }
    }
}

#[async_trait]
impl LibraryIndex for FixtureIndex {
    async fn multi_search(&self, queries: &[IndexQuery]) -> Result<IndexHits, ResolveError> {
        let mut hits = IndexHits::default();
        for query in queries {
            let mut kept: Vec<Value> = self
                .values(query.index)
                .into_iter()
                .filter(|v| filter_matches(v, query.filter.as_deref()))
                .filter(|v| text_matches(v, &query.query, query.index.searchable()))
                .collect();
            sort_values(&mut kept, &query.sort);
            kept.truncate(query.limit);
            hits.push_values(query.index, kept);
        }
        hits.dedup();
        Ok(hits)
    }
}

/// Meili's default typo budget, by query-term length.
fn typo_budget(term: &str) -> usize {
    match term.chars().count() {
        0..=4 => 0,
        5..=8 => 1,
        _ => 2,
    }
}

fn term_matches(term: &str, token: &str, is_last: bool) -> bool {
    if term == token {
        return true;
    }
    if is_last && token.starts_with(term) {
        return true;
    }
    strsim::levenshtein(term, token) <= typo_budget(term)
}

fn searchable_tokens(doc: &Value, fields: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for field in fields {
        match doc.get(*field) {
            Some(Value::String(s)) => out.extend(text::tokens(s)),
            Some(Value::Array(items)) => {
                for item in items {
                    if let Value::String(s) = item {
                        out.extend(text::tokens(s));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn text_matches(doc: &Value, query: &str, fields: &[&str]) -> bool {
    let terms = text::tokens(query);
    if terms.is_empty() {
        return true;
    }
    let tokens = searchable_tokens(doc, fields);
    let last = terms.len() - 1;
    terms.iter().enumerate().all(|(i, term)| {
        tokens
            .iter()
            .any(|token| term_matches(term, token, i == last))
    })
}

// -----------------------------------------------------------------------------
// Filters
// -----------------------------------------------------------------------------

fn parse_operand(raw: &str) -> Value {
    let raw = raw.trim();
    if let Some(stripped) = raw.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        return Value::String(stripped.to_string());
    }
    if let Ok(n) = raw.parse::<i64>() {
        return Value::from(n);
    }
    match raw {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => Value::String(raw.to_string()),
    }
}

fn compare(field: &Value, op: &str, operand: &Value) -> bool {
    if let Value::Array(items) = field {
        return items.iter().any(|item| compare(item, op, operand));
    }
    match op {
        "=" => field == operand,
        "!=" => field != operand,
        _ => {
            let (Some(a), Some(b)) = (field.as_f64(), operand.as_f64()) else {
                return false;
            };
            match op {
                "<=" => a <= b,
                ">=" => a >= b,
                "<" => a < b,
                ">" => a > b,
                _ => false,
            }
        }
    }
}

fn clause_matches(doc: &Value, clause: &str) -> bool {
    for op in ["<=", ">=", "!=", "=", "<", ">"] {
        if let Some((lhs, rhs)) = clause.split_once(op) {
            let field = lhs.trim();
            let Some(value) = doc.get(field) else {
                return false;
            };
            if value.is_null() {
                return false;
            }
            return compare(value, op, &parse_operand(rhs));
        }
    }
    false
}

fn filter_matches(doc: &Value, filter: Option<&str>) -> bool {
    let Some(filter) = filter else { return true };
    filter
        .split(" AND ")
        .all(|clause| clause_matches(doc, clause.trim()))
}

// -----------------------------------------------------------------------------
// Sorting
// -----------------------------------------------------------------------------

fn compare_values(a: Option<&Value>, b: Option<&Value>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Some(Value::Number(x)), Some(Value::Number(y))) => x
            .as_f64()
            .partial_cmp(&y.as_f64())
            .unwrap_or(Ordering::Equal),
        (Some(Value::String(x)), Some(Value::String(y))) => x.cmp(y),
        (None, None) => Ordering::Equal,
        (Some(Value::Null), Some(Value::Null)) => Ordering::Equal,
        // Meili puts documents without the attribute last.
        (None | Some(Value::Null), _) => Ordering::Greater,
        (_, None | Some(Value::Null)) => Ordering::Less,
        _ => Ordering::Equal,
    }
}

fn sort_values(values: &mut [Value], sort: &[String]) {
    if sort.is_empty() {
        return;
    }
    values.sort_by(|a, b| {
        for expression in sort {
            let (field, direction) = match expression.split_once(':') {
                Some(parts) => parts,
                None => (expression.as_str(), "asc"),
            };
            let ordering = compare_values(a.get(field), b.get(field));
            let ordering = if direction == "desc" {
                ordering.reverse()
            } else {
                ordering
            };
            if ordering != std::cmp::Ordering::Equal {
                return ordering;
            }
        }
        std::cmp::Ordering::Equal
    });
}

// =============================================================================
// Store
// =============================================================================

#[derive(Default)]
pub struct FixtureStore {
    pub watched_episodes: HashSet<Uuid>,
    pub watched_movies: HashSet<Uuid>,
    pub skew: FormatSkew,
}

impl FixtureStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn watched(mut self, episodes: &[Uuid]) -> Self {
        self.watched_episodes = episodes.iter().copied().collect();
        self
    }

    pub fn with_skew(mut self, ebook: i64, audiobook: i64) -> Self {
        self.skew = FormatSkew { ebook, audiobook };
        self
    }
}

fn brief(
    id: Uuid,
    title: &str,
    year: i32,
    has_ebook: bool,
    has_audiobook: bool,
    author_id: Uuid,
) -> BookBrief {
    BookBrief {
        id,
        title: title.to_string(),
        authors: vec!["Brandon Sanderson".to_string()],
        author_ids: vec![author_id],
        year: Some(year),
        description: None,
        is_in_library: true,
        has_audiobook,
        has_ebook,
    }
}

#[async_trait]
impl ResolveStore for FixtureStore {
    async fn content_categories(
        &self,
        show_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, String>, ResolveError> {
        let all = data::content_categories();
        Ok(show_ids
            .iter()
            .filter_map(|id| all.get(id).map(|c| (*id, c.clone())))
            .collect())
    }

    async fn watched_episode_ids(
        &self,
        _user_id: Uuid,
        episode_ids: &[Uuid],
    ) -> Result<HashSet<Uuid>, ResolveError> {
        Ok(episode_ids
            .iter()
            .filter(|id| self.watched_episodes.contains(id))
            .copied()
            .collect())
    }

    async fn watched_movie_ids(
        &self,
        _user_id: Uuid,
        movie_ids: &[Uuid],
    ) -> Result<HashSet<Uuid>, ResolveError> {
        Ok(movie_ids
            .iter()
            .filter(|id| self.watched_movies.contains(id))
            .copied()
            .collect())
    }

    async fn format_skew(&self, _user_id: Uuid) -> Result<FormatSkew, ResolveError> {
        Ok(self.skew)
    }

    async fn newest_books(
        &self,
        author_id: Option<Uuid>,
        series_id: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<BookBrief>, ResolveError> {
        let sanderson = author_id == Some(data::AUTHOR_SANDERSON);
        let stormlight = series_id == Some(data::SERIES_STORMLIGHT);
        if !sanderson && !stormlight {
            return Ok(Vec::new());
        }
        let mut books = vec![
            brief(
                data::BOOK_RHYTHM_OF_WAR,
                "Rhythm of War",
                2020,
                true,
                false,
                data::AUTHOR_SANDERSON,
            ),
            brief(
                data::BOOK_WAY_OF_KINGS,
                "The Way of Kings",
                2010,
                true,
                true,
                data::AUTHOR_SANDERSON,
            ),
        ];
        books.truncate(limit.max(0) as usize);
        Ok(books)
    }
}

// =============================================================================
// Providers
// =============================================================================

/// Three things nobody owns, one per provider path.
pub struct FixtureProviders;

fn hit(provider: &str, id: &str, title: &str, year: i32, authors: &[&str]) -> ProviderHit {
    let authors: Vec<String> = authors.iter().map(|a| (*a).to_string()).collect();
    ProviderHit {
        provider: provider.to_string(),
        id: id.to_string(),
        title: title.to_string(),
        year: Some(year),
        authors: authors.clone(),
        description: None,
        rating: Some(7.5),
        payload: ProviderPayload {
            provider: provider.to_string(),
            provider_id: id.to_string(),
            title: title.to_string(),
            year: Some(year),
            authors,
            ..ProviderPayload::default()
        },
    }
}

fn names(query: &str, title: &str) -> bool {
    let q = text::content_tokens(query);
    let t = text::content_tokens(title);
    !q.is_empty() && text::dice(&q, &t) >= 0.8
}

#[async_trait]
impl MetadataLookup for FixtureProviders {
    async fn books(&self, title: &str, _author: Option<&str>) -> Vec<ProviderHit> {
        if names(title, "Project Hail Mary") {
            return vec![hit(
                "hardcover",
                "99001",
                "Project Hail Mary",
                2021,
                &["Andy Weir"],
            )];
        }
        Vec::new()
    }

    async fn movies(&self, title: &str, _year: Option<i32>) -> Vec<ProviderHit> {
        if names(title, "The Brutalist") {
            return vec![hit("tmdb", "549509", "The Brutalist", 2024, &[])];
        }
        Vec::new()
    }

    async fn shows(&self, title: &str, _year: Option<i32>) -> Vec<ProviderHit> {
        if names(title, "Severance") {
            return vec![hit("tmdb", "95396", "Severance", 2022, &[])];
        }
        // In the library *and* at the provider: the only way a season the
        // library does not hold can be reached for a show it does.
        if names(title, "someShow") {
            return vec![hit("tmdb", "70001", "someShow", 2001, &[])];
        }
        // Two shows with the same name, neither in the library: the title
        // cannot break the tie, and nobody said a year. Only a person named in
        // the request can.
        if names(title, "Ludwig") {
            return vec![
                hit("tmdb", "243360", "Ludwig", 2024, &[]),
                hit("tmdb", "85899", "Ludwig", 1981, &[]),
            ];
        }
        // A title made of an intent word. Matched on the words as spoken rather
        // than through `names`, which strips "last" from both sides and would
        // answer to a bare "seen" — the very thing that sent the real resolver
        // to TMDB asking for `seen` and brought back strangers.
        if text::normalize(title) == "last seen" {
            return vec![hit("tmdb", "70002", "Last Seen", 2026, &[])];
        }
        Vec::new()
    }

    async fn tv_credits(&self, _provider: &str, id: &str) -> Vec<String> {
        match id {
            "243360" => vec!["David Mitchell".to_string()],
            "85899" => vec!["Götz Otto".to_string()],
            _ => Vec::new(),
        }
    }
    async fn tv_seasons(&self, _provider: &str, id: &str) -> Vec<ShowSeason> {
        match id {
            // The show the person-match resolves to. Two seasons: one aired,
            // one announced. A request for season 3 must fail as not found.
            "243360" => vec![
                ShowSeason {
                    season: 1,
                    episodes: Some(6),
                    air_date: Some("2024-01-15".to_string()),
                },
                ShowSeason {
                    season: 2,
                    episodes: Some(6),
                    air_date: Some("2026-03-01".to_string()),
                },
            ],
            "85899" => vec![ShowSeason {
                season: 1,
                episodes: Some(4),
                air_date: Some("1981-05-02".to_string()),
            }],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn matching_strategy_is_all() {
        let index = FixtureIndex::new();
        let hits = index
            .multi_search(&[IndexQuery::new(IndexKind::TvShows, "great escape", 10)])
            .await
            .unwrap();
        assert!(hits.shows.is_empty(), "no show has both terms");

        let hits = index
            .multi_search(&[IndexQuery::new(IndexKind::Movies, "great escape", 10)])
            .await
            .unwrap();
        assert_eq!(hits.movies.len(), 1);
    }

    #[tokio::test]
    async fn descriptions_are_not_searchable() {
        let index = FixtureIndex::new();
        let hits = index
            .multi_search(&[IndexQuery::new(IndexKind::TvShows, "tents", 10)])
            .await
            .unwrap();
        assert!(hits.shows.is_empty());
    }

    #[tokio::test]
    async fn typo_budget_matches_meilis() {
        let index = FixtureIndex::new();
        let hits = index
            .multi_search(&[IndexQuery::new(IndexKind::Movies, "arival", 10)])
            .await
            .unwrap();
        assert_eq!(hits.movies.len(), 1, "one edit is inside the budget");

        let hits = index
            .multi_search(&[IndexQuery::new(IndexKind::TvShows, "baking", 10)])
            .await
            .unwrap();
        assert!(hits.shows.is_empty(), "three edits from 'bake' is not");
    }

    #[tokio::test]
    async fn filters_and_sorts() {
        let index = FixtureIndex::new();
        let now = data::now().unix_timestamp();
        let hits = index
            .multi_search(&[IndexQuery::new(IndexKind::TvEpisodes, "", 5)
                .filtered(format!(
                    "show_id = \"{}\" AND air_date_ts <= {now}",
                    data::SHOW_SOMESHOW
                ))
                .sorted(&["air_date_ts:desc"])])
            .await
            .unwrap();
        assert_eq!(hits.episodes[0].id, data::EP_SOMESHOW_S24E08.to_string());
    }

    #[test]
    fn placeholders_expand_to_fixture_ids() {
        assert_eq!(
            data::expand("show:@SHOW_SOMESHOW"),
            format!("show:{}", data::SHOW_SOMESHOW)
        );
        assert_eq!(
            data::expand("provider:tmdb:95396:s03"),
            "provider:tmdb:95396:s03"
        );
    }
}
