//! Hardcover author provider for tracking functionality.

use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use time::Date;

use crate::serde_date;

/// A provider that can search for authors and retrieve their books.
#[async_trait]
pub trait TrackedAuthorProvider: Send + Sync {
    /// Identifier key for this provider (e.g. "hardcover").
    /// Used as the key in identifiers JSONB and for cache namespacing.
    fn provider_id(&self) -> &str;

    /// Search for authors by name.
    async fn search_authors(&self, query: &str) -> Result<Vec<AuthorSearchResult>, String>;

    /// Get an author's books by their provider ID.
    async fn get_author_books(&self, author_id: &str) -> Result<AuthorWithBooks, String>;

    /// Get a series' books by the provider's series ID.
    async fn get_series_books(&self, _series_id: &str) -> Result<SeriesWithBooks, String> {
        Err("get_series_books not supported by this provider".to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorSearchResult {
    pub id: String,
    pub slug: Option<String>,
    pub name: String,
    pub image_url: Option<String>,
    pub book_count: Option<i32>,
    /// Series names this author has written (for identification in search results)
    pub series_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorWithBooks {
    pub id: String,
    pub slug: Option<String>,
    pub name: String,
    pub image_url: Option<String>,
    pub bio: Option<String>,
    pub books: Vec<AuthorBook>,
    pub provider_metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesWithBooks {
    pub id: String,
    pub slug: Option<String>,
    pub name: String,
    pub image_url: Option<String>,
    pub description: Option<String>,
    pub book_count: i32,
    pub books: Vec<SeriesBook>,
    pub provider_metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesBook {
    pub id: String,
    pub slug: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub position: Option<f64>,
    #[serde(with = "serde_date::option_date")]
    pub release_date: Option<Date>,
    pub is_released: bool,
    pub pages: Option<i32>,
    pub rating: Option<f64>,
    pub ratings_count: Option<i32>,
    /// Authors for this book
    pub authors: Vec<String>,
    pub provider_metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct SeriesMembership {
    pub series_name: String,
    pub series_id: Option<String>,
    pub series_slug: Option<String>,
    pub series_index: Option<f64>,
    pub books_count: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorBook {
    pub id: String,
    pub slug: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub series_name: Option<String>,
    pub series_id: Option<String>,
    pub series_slug: Option<String>,
    pub series_index: Option<f64>,
    /// All series this book belongs to (superset of the single series_name/series_id fields)
    #[serde(skip)]
    pub series_memberships: Vec<SeriesMembership>,
    #[serde(with = "serde_date::option_date")]
    pub release_date: Option<Date>,
    pub is_released: bool,
    pub pages: Option<i32>,
    pub rating: Option<f64>,
    pub ratings_count: Option<i32>,
    pub provider_metadata: Option<serde_json::Value>,
}

/// Raw Hardcover fields to store for debugging.
struct HardcoverBookMeta {
    id: i64,
    canonical_id: Option<i64>,
    compilation: Option<bool>,
    featured_book_series_id: Option<i64>,
    book_status_id: Option<i32>,
    book_category_id: Option<i32>,
    subtitle: Option<String>,
    users_read_count: Option<i32>,
    default_physical_edition_id: Option<i64>,
    default_ebook_edition_id: Option<i64>,
    default_audio_edition_id: Option<i64>,
    default_cover_edition_id: Option<i64>,
    cached_image: Option<serde_json::Value>,
    users_count: Option<i32>,
    release_year: Option<i32>,
    editions_count: Option<i32>,
    featured: Option<bool>,
    series_state: Option<String>,
}

/// Build a `{"hardcover": {...}}` metadata blob from raw Hardcover fields.
fn build_book_provider_metadata(m: &HardcoverBookMeta) -> Option<serde_json::Value> {
    let mut hc = serde_json::Map::new();
    hc.insert("id".into(), m.id.into());
    if let Some(v) = m.canonical_id {
        hc.insert("canonical_id".into(), v.into());
    }
    if let Some(v) = m.compilation {
        hc.insert("compilation".into(), v.into());
    }
    if let Some(v) = m.featured_book_series_id {
        hc.insert("featured_book_series_id".into(), v.into());
    }
    if let Some(v) = m.book_status_id {
        hc.insert("book_status_id".into(), v.into());
    }
    if let Some(v) = m.book_category_id {
        hc.insert("book_category_id".into(), v.into());
    }
    if let Some(ref v) = m.subtitle {
        hc.insert("subtitle".into(), v.clone().into());
    }
    if let Some(v) = m.users_read_count {
        hc.insert("users_read_count".into(), v.into());
    }
    if let Some(v) = m.default_physical_edition_id {
        hc.insert("default_physical_edition_id".into(), v.into());
    }
    if let Some(v) = m.default_ebook_edition_id {
        hc.insert("default_ebook_edition_id".into(), v.into());
    }
    if let Some(v) = m.default_audio_edition_id {
        hc.insert("default_audio_edition_id".into(), v.into());
    }
    if let Some(v) = m.default_cover_edition_id {
        hc.insert("default_cover_edition_id".into(), v.into());
    }
    if let Some(ref v) = m.cached_image {
        hc.insert("cached_image".into(), v.clone());
    }
    if let Some(v) = m.users_count {
        hc.insert("users_count".into(), v.into());
    }
    if let Some(v) = m.release_year {
        hc.insert("release_year".into(), v.into());
    }
    if let Some(v) = m.editions_count {
        hc.insert("editions_count".into(), v.into());
    }
    if let Some(v) = m.featured {
        hc.insert("featured".into(), v.into());
    }
    if let Some(ref v) = m.series_state {
        hc.insert("series_state".into(), v.clone().into());
    }
    // Always has at least id, so never empty
    Some(serde_json::json!({ "hardcover": hc }))
}

/// Hardcover GraphQL implementation of the author provider.
pub struct HardcoverAuthorProvider {
    client: crate::http::ProviderClient,
    base_url: String,
    api_key: String,
}

impl HardcoverAuthorProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: crate::http::ProviderClient::new(
                crate::http::new_client(),
                crate::http::provider_ids::HARDCOVER,
            ),
            base_url: "https://api.hardcover.app".to_string(),
            api_key,
        }
    }

    /// Create a provider with a custom base URL (for testing).
    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            client: crate::http::ProviderClient::new(
                crate::http::new_client(),
                crate::http::provider_ids::HARDCOVER,
            ),
            base_url,
            api_key,
        }
    }
}

#[async_trait]
impl TrackedAuthorProvider for HardcoverAuthorProvider {
    fn provider_id(&self) -> &str {
        crate::http::provider_ids::HARDCOVER
    }

    async fn search_authors(&self, query: &str) -> Result<Vec<AuthorSearchResult>, String> {
        let graphql_query = GraphQLQuery {
            query: SEARCH_AUTHORS_QUERY.to_string(),
            variables: serde_json::json!({
                "query": query,
                "per_page": 10
            }),
        };

        let response = self
            .client
            .execute(
                self.client
                    .post(&format!("{}/v1/graphql", self.base_url))
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .json(&graphql_query),
            )
            .await
            .map_err(|e| e.to_string())?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("API error: {} - {}", status, text));
        }

        let result: SearchResponse = response.json().await.map_err(|e| e.to_string())?;

        if let Some(ref errors) = result.errors
            && !errors.is_empty()
        {
            tracing::warn!("Hardcover GraphQL errors: {:?}", errors);
        }

        let search_data = match result.data {
            Some(data) => data.search,
            None => return Err("Hardcover API returned no data".to_string()),
        };

        let results_json = match search_data.results {
            Some(r) => r,
            None => return Err("Hardcover API returned no results field".to_string()),
        };

        let hits = match results_json.get("hits").and_then(|h| h.as_array()) {
            Some(h) => h,
            None => {
                return Err(format!(
                    "Hardcover API response missing hits array. Got: {}",
                    serde_json::to_string(&results_json).unwrap_or_default()
                ));
            }
        };

        let mut results = Vec::new();
        for hit in hits {
            let Some(doc) = hit.get("document") else {
                tracing::warn!("Hardcover search: skipping hit with no document: {:?}", hit);
                continue;
            };

            let Some(name) = doc.get("name").and_then(|n| n.as_str()) else {
                tracing::warn!(
                    "Hardcover search: skipping author with missing name: {:?}",
                    doc
                );
                continue;
            };

            // Handle ID as either string or integer
            let id = doc.get("id").and_then(|id| {
                id.as_str()
                    .map(String::from)
                    .or_else(|| id.as_i64().map(|n| n.to_string()))
            });

            let Some(id) = id else {
                tracing::warn!(
                    "Hardcover search: skipping author with missing/invalid ID: {:?}",
                    doc
                );
                continue;
            };

            // Extract series names for identification
            let series_names = doc
                .get("series_names")
                .and_then(|s| s.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let slug = doc.get("slug").and_then(|s| s.as_str()).map(String::from);

            results.push(AuthorSearchResult {
                id,
                slug,
                name: name.to_string(),
                image_url: doc
                    .get("image")
                    .and_then(|i| i.get("url"))
                    .and_then(|u| u.as_str())
                    .map(String::from),
                book_count: doc
                    .get("books_count")
                    .and_then(|c| c.as_i64())
                    .map(|c| c as i32),
                series_names,
            });
        }

        Ok(results)
    }

    async fn get_author_books(&self, author_id: &str) -> Result<AuthorWithBooks, String> {
        let author_id_int: i64 = author_id.parse().map_err(|_| "Invalid author ID")?;

        let mut all_series_slots: Vec<SeriesSlotEntry> = Vec::new();
        let mut all_standalones: Vec<StandaloneBookData> = Vec::new();
        let mut author_meta: Option<AuthorMetaData> = None;

        // Dual pagination: series_slots and standalones paginate independently
        let mut series_offset: i64 = 0;
        let mut standalone_offset: i64 = 0;
        let mut series_done = false;
        let mut standalones_done = false;

        while !series_done || !standalones_done {
            let graphql_query = GraphQLQuery {
                query: GET_AUTHOR_BOOKS_QUERY.to_string(),
                variables: serde_json::json!({
                    "authorId": author_id_int,
                    "pageSize": 100,
                    "seriesOffset": if series_done { 0 } else { series_offset },
                    "standaloneOffset": if standalones_done { 0 } else { standalone_offset },
                }),
            };

            let response = self
                .client
                .execute(
                    self.client
                        .post(&format!("{}/v1/graphql", self.base_url))
                        .header("Authorization", format!("Bearer {}", self.api_key))
                        .json(&graphql_query),
                )
                .await
                .map_err(|e| e.to_string())?;

            if !response.status().is_success() {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                return Err(format!("API error: {} - {}", status, text));
            }

            let bytes = response.bytes().await.map_err(|e| e.to_string())?;
            let result: AuthorBooksResponse = serde_json::from_slice(&bytes).map_err(|e| {
                let preview = String::from_utf8_lossy(&bytes[..bytes.len().min(2000)]);
                tracing::error!(
                    "Failed to parse Hardcover author books response: {}. Body: {}",
                    e,
                    preview
                );
                format!("Failed to parse response: {}", e)
            })?;

            if let Some(ref errors) = result.errors
                && !errors.is_empty()
            {
                tracing::warn!("Hardcover GraphQL errors: {:?}", errors);
            }

            let data = result
                .data
                .ok_or_else(|| "No data in response".to_string())?;

            if author_meta.is_none() {
                author_meta = data.author;
            }

            if !series_done {
                let page_size = data.series_slots.len();
                all_series_slots.extend(data.series_slots);
                if page_size < 100 {
                    series_done = true;
                } else {
                    series_offset += 100;
                }
            }

            if !standalones_done {
                let page_size = data.standalones.len();
                all_standalones.extend(data.standalones);
                if page_size < 100 {
                    standalones_done = true;
                } else {
                    standalone_offset += 100;
                }
            }
        }

        let author_meta = author_meta.ok_or_else(|| "Author not found".to_string())?;
        let today = time::OffsetDateTime::now_utc().date();

        let parse_date = |d: &str| {
            time::Date::parse(d, time::macros::format_description!("[year]-[month]-[day]")).ok()
        };

        // Deduplicate series with the same name. Hardcover sometimes has duplicate
        // series entries (e.g. two "Cradle" series with different IDs). When this happens,
        // keep only slots from the series with the most books to avoid phantom entries.
        let all_series_slots = {
            // For each series name, track the series_id with the highest books_count.
            let mut best_series: HashMap<String, (i64, i32)> = HashMap::new();
            for slot in &all_series_slots {
                if let Some(ref s) = slot.series {
                    let count = s.books_count.unwrap_or(0);
                    let entry = best_series.entry(s.name.clone()).or_insert((s.id, count));
                    if count > entry.1 {
                        *entry = (s.id, count);
                    }
                }
            }
            all_series_slots
                .into_iter()
                .filter(|slot| {
                    let Some(ref s) = slot.series else {
                        return true;
                    };
                    let dominated =
                        best_series
                            .get(&s.name)
                            .is_some_and(|(best_id, best_count)| {
                                *best_id != s.id && *best_count > s.books_count.unwrap_or(0)
                            });
                    if dominated {
                        tracing::debug!(
                            series_name = %s.name,
                            dropped_id = s.id,
                            "Dropping book from duplicate series"
                        );
                    }
                    !dominated
                })
                .collect::<Vec<_>>()
        };

        // Group series_slots by book ID, collecting all series memberships per book
        let mut books: Vec<AuthorBook> = {
            let mut book_map: HashMap<i64, (AuthorBook, Vec<SeriesMembership>)> = HashMap::new();

            for slot in all_series_slots {
                let Some(book) = slot.book else {
                    continue;
                };
                let book_id = book.id;

                let membership = slot.series.as_ref().map(|s| SeriesMembership {
                    series_name: s.name.clone(),
                    series_id: Some(s.id.to_string()),
                    series_slug: s.slug.clone(),
                    series_index: slot.position,
                    books_count: s.books_count,
                });

                if let Some((_, memberships)) = book_map.get_mut(&book_id) {
                    if let Some(m) = membership {
                        memberships.push(m);
                    }
                } else {
                    let series_state = slot.series.as_ref().and_then(|s| s.state.clone());
                    let (series_name, series_id, series_slug) = match &slot.series {
                        Some(s) => (Some(s.name.clone()), Some(s.id.to_string()), s.slug.clone()),
                        None => (None, None, None),
                    };

                    let release_date = book.release_date.as_deref().and_then(parse_date);
                    let is_released = release_date.map(|d| d <= today).unwrap_or(true);

                    let provider_metadata = build_book_provider_metadata(&HardcoverBookMeta {
                        id: book.id,
                        canonical_id: book.canonical_id,
                        compilation: book.compilation,
                        featured_book_series_id: book.featured_book_series_id,
                        book_status_id: book.book_status_id,
                        book_category_id: book.book_category_id,
                        subtitle: book.subtitle,
                        users_read_count: book.users_read_count,
                        default_physical_edition_id: book.default_physical_edition_id,
                        default_ebook_edition_id: book.default_ebook_edition_id,
                        default_audio_edition_id: book.default_audio_edition_id,
                        default_cover_edition_id: book.default_cover_edition_id,
                        cached_image: book.cached_image,
                        users_count: book.users_count,
                        release_year: book.release_year,
                        editions_count: book.editions_count,
                        featured: slot.featured,
                        series_state,
                    });

                    let memberships = membership.into_iter().collect();

                    let author_book = AuthorBook {
                        id: book.id.to_string(),
                        slug: book.slug,
                        title: book.title,
                        description: book.description,
                        cover_url: book.image.map(|i| i.url),
                        series_name,
                        series_id,
                        series_slug,
                        series_index: slot.position,
                        series_memberships: Vec::new(),
                        release_date,
                        is_released,
                        pages: book.pages,
                        rating: book.rating,
                        ratings_count: book.ratings_count,
                        provider_metadata,
                    };

                    book_map.insert(book_id, (author_book, memberships));
                }
            }

            book_map
                .into_values()
                .map(|(mut book, memberships)| {
                    book.series_memberships = memberships;
                    // Populate single series fields from the first membership
                    if let Some(first) = book.series_memberships.first() {
                        book.series_name = Some(first.series_name.clone());
                        book.series_id = first.series_id.clone();
                        book.series_slug = first.series_slug.clone();
                        book.series_index = first.series_index;
                    }
                    book
                })
                .collect()
        };

        // Convert standalones to AuthorBook
        books.extend(all_standalones.into_iter().map(|book| {
            let release_date = book.release_date.as_deref().and_then(parse_date);
            let is_released = release_date.map(|d| d <= today).unwrap_or(true);
            let provider_metadata = build_book_provider_metadata(&HardcoverBookMeta {
                id: book.id,
                canonical_id: book.canonical_id,
                compilation: book.compilation,
                featured_book_series_id: book.featured_book_series_id,
                book_status_id: book.book_status_id,
                book_category_id: book.book_category_id,
                subtitle: book.subtitle,
                users_read_count: book.users_read_count,
                default_physical_edition_id: book.default_physical_edition_id,
                default_ebook_edition_id: book.default_ebook_edition_id,
                default_audio_edition_id: book.default_audio_edition_id,
                default_cover_edition_id: book.default_cover_edition_id,
                cached_image: book.cached_image,
                users_count: book.users_count,
                release_year: book.release_year,
                editions_count: book.editions_count,
                featured: None,
                series_state: None,
            });
            AuthorBook {
                id: book.id.to_string(),
                slug: book.slug,
                title: book.title,
                description: book.description,
                cover_url: book.image.map(|i| i.url),
                series_name: None,
                series_id: None,
                series_slug: None,
                series_index: None,
                series_memberships: Vec::new(),
                release_date,
                is_released,
                pages: book.pages,
                rating: book.rating,
                ratings_count: book.ratings_count,
                provider_metadata,
            }
        }));

        // Sort by series name → series_index → release_date
        books.sort_by(|a, b| {
            match (&a.series_name, &b.series_name) {
                (Some(sa), Some(sb)) if sa != sb => return sa.cmp(sb),
                (None, Some(_)) => return std::cmp::Ordering::Greater,
                (Some(_), None) => return std::cmp::Ordering::Less,
                _ => {}
            }
            match (a.series_index, b.series_index) {
                (Some(ia), Some(ib)) => {
                    let cmp = ia.partial_cmp(&ib).unwrap_or(std::cmp::Ordering::Equal);
                    if cmp != std::cmp::Ordering::Equal {
                        return cmp;
                    }
                }
                (None, Some(_)) => return std::cmp::Ordering::Greater,
                (Some(_), None) => return std::cmp::Ordering::Less,
                _ => {}
            }
            match (&a.release_date, &b.release_date) {
                (Some(da), Some(db)) => da.cmp(db),
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });

        tracing::info!(
            author_id = %author_id_int,
            author_name = %author_meta.name,
            series_books = %books.iter().filter(|b| b.series_name.is_some()).count(),
            standalones = %books.iter().filter(|b| b.series_name.is_none()).count(),
            "Hardcover author books fetched"
        );

        // Build author-level provider metadata
        let author_provider_metadata = {
            let mut hc = serde_json::Map::new();
            if let Some(c) = author_meta.books_count {
                hc.insert("books_count".into(), c.into());
            }
            if hc.is_empty() {
                None
            } else {
                Some(serde_json::json!({ "hardcover": hc }))
            }
        };

        Ok(AuthorWithBooks {
            id: author_meta.id.to_string(),
            slug: author_meta.slug,
            name: author_meta.name,
            image_url: author_meta.image.map(|i| i.url),
            bio: author_meta.bio,
            books,
            provider_metadata: author_provider_metadata,
        })
    }

    async fn get_series_books(&self, series_id: &str) -> Result<SeriesWithBooks, String> {
        self.get_series_books_impl(series_id).await
    }
}

/// A Hardcover table whose ids we store, for [`HardcoverAuthorProvider::lookup_canonical`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HardcoverEntity {
    Author,
    Series,
    Book,
}

impl HardcoverEntity {
    fn table(self) -> &'static str {
        match self {
            HardcoverEntity::Author => "authors",
            HardcoverEntity::Series => "series",
            HardcoverEntity::Book => "books",
        }
    }
}

/// Ids per lookup. Hardcover silently caps rows per query -- 100 for authors and
/// series, more for books -- so this stays well under the smallest cap seen.
const LOOKUP_CHUNK: usize = 50;

impl HardcoverAuthorProvider {
    /// Ask Hardcover what became of `ids`. Each id it still has maps to its
    /// `canonical_id` (`None` when the id is itself canonical, `Some` when Hardcover
    /// merged it into another). An id Hardcover no longer has is absent from the map,
    /// which is the only way a deletion shows -- so any error fails the whole call, and
    /// an id missing from a batch is asked for again on its own before it counts as
    /// gone, since a capped batch drops rows without saying so.
    pub async fn lookup_canonical(
        &self,
        entity: HardcoverEntity,
        ids: &[i64],
    ) -> Result<HashMap<i64, Option<i64>>, String> {
        let mut out = HashMap::with_capacity(ids.len());
        for chunk in ids.chunks(LOOKUP_CHUNK) {
            let gql = GraphQLQuery {
                query: format!(
                    "query($ids: [Int!]!) {{ items: {}(where: {{ id: {{ _in: $ids }} }}) {{ id canonical_id }} }}",
                    entity.table()
                ),
                variables: serde_json::json!({ "ids": chunk }),
            };

            let response = self
                .client
                .execute(
                    self.client
                        .post(&format!("{}/v1/graphql", self.base_url))
                        .header("Authorization", format!("Bearer {}", self.api_key))
                        .json(&gql),
                )
                .await
                .map_err(|e| format!("HTTP error: {}", e))?;

            if !response.status().is_success() {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                return Err(format!("API error: {} - {}", status, text));
            }

            let bytes = response
                .bytes()
                .await
                .map_err(|e| format!("HTTP body error: {}", e))?;
            let resp: LookupResponse =
                serde_json::from_slice(&bytes).map_err(|e| format!("JSON parse error: {}", e))?;

            if let Some(errors) = resp.errors
                && !errors.is_empty()
            {
                return Err(format!("GraphQL error: {:?}", errors));
            }

            let data = resp.data.ok_or("No data in response")?;
            for item in data.items {
                out.insert(item.id, item.canonical_id);
            }

            let missing: Vec<i64> = chunk
                .iter()
                .copied()
                .filter(|id| !out.contains_key(id))
                .collect();
            if !missing.is_empty() {
                let confirmed = self.lookup_by_pk(entity, &missing).await?;
                if !confirmed.is_empty() {
                    tracing::warn!(
                        table = entity.table(),
                        dropped = confirmed.len(),
                        "Hardcover batch lookup dropped rows that exist; LOOKUP_CHUNK may exceed its cap"
                    );
                }
                out.extend(confirmed);
            }
        }
        Ok(out)
    }

    /// Look ids up one by one (`<table>_by_pk`, aliased into one request), which answers
    /// definitively: an object, or null for an id Hardcover does not have.
    async fn lookup_by_pk(
        &self,
        entity: HardcoverEntity,
        ids: &[i64],
    ) -> Result<HashMap<i64, Option<i64>>, String> {
        let fields: String = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                format!(
                    "m{i}: {}_by_pk(id: {id}) {{ id canonical_id }} ",
                    entity.table()
                )
            })
            .collect();
        let gql = GraphQLQuery {
            query: format!("query {{ {fields}}}"),
            variables: serde_json::json!({}),
        };

        let response = self
            .client
            .execute(
                self.client
                    .post(&format!("{}/v1/graphql", self.base_url))
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .json(&gql),
            )
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("API error: {} - {}", status, text));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("HTTP body error: {}", e))?;
        let resp: ByPkResponse =
            serde_json::from_slice(&bytes).map_err(|e| format!("JSON parse error: {}", e))?;

        if let Some(errors) = resp.errors
            && !errors.is_empty()
        {
            return Err(format!("GraphQL error: {:?}", errors));
        }

        let data = resp.data.ok_or("No data in response")?;
        if data.len() != ids.len() {
            return Err(format!(
                "by_pk lookup answered {} of {} ids",
                data.len(),
                ids.len()
            ));
        }
        Ok(data
            .into_values()
            .flatten()
            .map(|item| (item.id, item.canonical_id))
            .collect())
    }
}

impl HardcoverAuthorProvider {
    /// Fetch all books in a series by its Hardcover ID.
    async fn get_series_books_impl(&self, series_id: &str) -> Result<SeriesWithBooks, String> {
        let series_id_int: i64 = series_id
            .parse()
            .map_err(|_| format!("invalid series id: {}", series_id))?;

        let mut all_entries: Vec<SeriesBookEntry> = Vec::new();
        let mut saved_id: i64 = 0;
        let mut saved_slug: Option<String> = None;
        let mut saved_name = String::new();
        let mut saved_description: Option<String> = None;
        let mut saved_primary_books_count: Option<i32> = None;
        let mut saved_books_count: Option<i32> = None;
        let mut offset: i64 = 0;

        loop {
            let gql = GraphQLQuery {
                query: GET_SERIES_BOOKS_QUERY.to_string(),
                variables: serde_json::json!({
                    "id": series_id_int,
                    "limit": 100,
                    "offset": offset
                }),
            };

            let response = self
                .client
                .execute(
                    self.client
                        .post(&format!("{}/v1/graphql", self.base_url))
                        .header("Authorization", format!("Bearer {}", self.api_key))
                        .json(&gql),
                )
                .await
                .map_err(|e| format!("HTTP error: {}", e))?;

            let bytes = response
                .bytes()
                .await
                .map_err(|e| format!("HTTP body error: {}", e))?;
            let resp: SeriesBooksResponse =
                serde_json::from_slice(&bytes).map_err(|e| format!("JSON parse error: {}", e))?;

            if let Some(errors) = resp.errors
                && !errors.is_empty()
            {
                return Err(format!("GraphQL error: {:?}", errors));
            }

            let data = resp.data.ok_or("No data in response")?;
            let series = data.series.ok_or("Series not found")?;
            let page_size = series.book_series.len();

            if offset == 0 {
                saved_id = series.id;
                saved_slug = series.slug;
                saved_name = series.name;
                saved_description = series.description;
                saved_primary_books_count = series.primary_books_count;
                saved_books_count = series.books_count;
            }

            all_entries.extend(series.book_series);

            if page_size < 100 {
                break;
            }
            offset += 100;
        }

        let today = time::OffsetDateTime::now_utc().date();

        let books: Vec<SeriesBook> = all_entries
            .into_iter()
            .filter_map(|entry| {
                let book = entry.book?;

                let release_date = book.release_date.as_ref().and_then(|d| {
                    time::Date::parse(d, time::macros::format_description!("[year]-[month]-[day]"))
                        .ok()
                });
                let is_released = release_date.map(|d| d <= today).unwrap_or(true);

                let authors: Vec<String> = book
                    .contributions
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|c| c.author.map(|a| a.name))
                    .collect();

                let provider_metadata = build_book_provider_metadata(&HardcoverBookMeta {
                    id: book.id,
                    canonical_id: book.canonical_id,
                    compilation: book.compilation,
                    featured_book_series_id: book.featured_book_series_id,
                    book_status_id: book.book_status_id,
                    book_category_id: book.book_category_id,
                    subtitle: book.subtitle,
                    users_read_count: book.users_read_count,
                    default_physical_edition_id: book.default_physical_edition_id,
                    default_ebook_edition_id: book.default_ebook_edition_id,
                    default_audio_edition_id: book.default_audio_edition_id,
                    default_cover_edition_id: book.default_cover_edition_id,
                    cached_image: book.cached_image,
                    users_count: book.users_count,
                    release_year: book.release_year,
                    editions_count: book.editions_count,
                    featured: entry.featured,
                    series_state: None,
                });

                Some(SeriesBook {
                    id: book.id.to_string(),
                    slug: book.slug,
                    title: book.title,
                    description: book.description,
                    cover_url: book.image.map(|i| i.url),
                    position: entry.position,
                    release_date,
                    is_released,
                    pages: book.pages,
                    rating: book.rating,
                    ratings_count: book.ratings_count,
                    authors,
                    provider_metadata,
                })
            })
            .collect();

        tracing::info!(
            series_id = %series_id,
            series_name = %saved_name,
            book_count = %books.len(),
            "Hardcover series books fetched"
        );

        // Build series-level provider metadata
        let series_provider_metadata = {
            let mut hc = serde_json::Map::new();
            if let Some(c) = saved_primary_books_count {
                hc.insert("primary_books_count".into(), c.into());
            }
            if let Some(c) = saved_books_count {
                hc.insert("books_count".into(), c.into());
            }
            if hc.is_empty() {
                None
            } else {
                Some(serde_json::json!({ "hardcover": hc }))
            }
        };

        Ok(SeriesWithBooks {
            id: saved_id.to_string(),
            slug: saved_slug,
            name: saved_name,
            image_url: None,
            description: saved_description,
            book_count: saved_primary_books_count.unwrap_or(saved_books_count.unwrap_or(0)),
            books,
            provider_metadata: series_provider_metadata,
        })
    }
}

// ============================================================================
// GraphQL Types
// ============================================================================

const SEARCH_AUTHORS_QUERY: &str = r#"
query SearchAuthors($query: String!, $per_page: Int!) {
  search(query: $query, query_type: "Author", per_page: $per_page) {
    ids
    results
  }
}
"#;

const GET_AUTHOR_BOOKS_QUERY: &str = r#"
query AuthorBooksPaged($authorId: Int!, $pageSize: Int! = 100, $seriesOffset: Int! = 0, $standaloneOffset: Int! = 0) {
  series_slots: book_series(
    where: {
      featured: { _eq: true }
      series: { state: { _neq: "duplicate" }, canonical_id: { _is_null: true } }
      book: {
        canonical_id: { _is_null: true }
        compilation: { _eq: false }
        contributions: { author_id: { _eq: $authorId }, contributable_type: { _eq: "Book" } }
        _or: [
          { default_physical_edition_id: { _is_null: false } }
          { default_ebook_edition_id: { _is_null: false } }
        ]
      }
    }
    distinct_on: [series_id, position]
    order_by: [
      { series_id: asc }
      { position: asc_nulls_last }
      { book: { users_count: desc_nulls_last } }
      { book: { id: asc } }
    ]
    limit: $pageSize
    offset: $seriesOffset
  ) {
    series { id name slug books_count state }
    position
    featured
    book {
      id slug title description pages release_date
      image { url }
      rating ratings_count
      canonical_id compilation featured_book_series_id
      book_status_id book_category_id subtitle users_read_count
      default_physical_edition_id default_ebook_edition_id
      default_audio_edition_id default_cover_edition_id
      cached_image users_count release_year editions_count
    }
  }
  standalones: books(
    where: {
      canonical_id: { _is_null: true }
      compilation: { _eq: false }
      featured_book_series_id: { _is_null: true }
      contributions: { author_id: { _eq: $authorId }, contributable_type: { _eq: "Book" } }
      _or: [
        { default_physical_edition_id: { _is_null: false } }
        { default_ebook_edition_id: { _is_null: false } }
      ]
    }
    order_by: [{ release_date: desc_nulls_last }, { id: desc }]
    limit: $pageSize
    offset: $standaloneOffset
  ) {
    id slug title description pages release_date
    image { url }
    rating ratings_count
    canonical_id compilation featured_book_series_id
    book_status_id book_category_id subtitle users_read_count
    default_physical_edition_id default_ebook_edition_id
    default_audio_edition_id default_cover_edition_id
    cached_image users_count release_year editions_count
  }
  author: authors_by_pk(id: $authorId) {
    id slug name bio
    image { url }
    books_count
  }
}
"#;

const GET_SERIES_BOOKS_QUERY: &str = r#"
query GetSeriesBooks($id: Int!, $limit: Int! = 100, $offset: Int! = 0) {
  series: series_by_pk(id: $id) {
    id
    slug
    name
    description
    primary_books_count
    books_count
    book_series(
      where: {
        book: {
          canonical_id: { _is_null: true }
          compilation: { _eq: false }
          _or: [
            { default_physical_edition_id: { _is_null: false } }
            { default_ebook_edition_id: { _is_null: false } }
          ]
        }
      }
      distinct_on: [position]
      order_by: [
        { position: asc_nulls_last }
        { book: { users_count: desc_nulls_last } }
        { book: { id: asc } }
      ]
      limit: $limit
      offset: $offset
    ) {
      position
      featured
      book {
        id
        slug
        title
        description
        pages
        release_date
        image { url }
        rating
        ratings_count
        canonical_id
        compilation
        featured_book_series_id
        book_status_id
        book_category_id
        subtitle
        users_read_count
        users_count
        default_physical_edition_id
        default_ebook_edition_id
        default_audio_edition_id
        default_cover_edition_id
        cached_image
        release_year
        editions_count
        contributions(
          where: { contribution: { _is_null: true } }
          limit: 5
        ) {
          author { id name }
        }
      }
    }
  }
}
"#;

#[derive(Debug, Serialize)]
struct GraphQLQuery {
    query: String,
    variables: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    data: Option<SearchResponseData>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQLError {
    #[allow(dead_code)]
    message: String,
}

#[derive(Debug, Deserialize)]
struct SearchResponseData {
    search: SearchOutput,
}

#[derive(Debug, Deserialize)]
struct SearchOutput {
    #[allow(dead_code)]
    ids: Option<Vec<String>>,
    results: Option<serde_json::Value>,
}

// Response types for AuthorBooksPaged query
#[derive(Debug, Deserialize)]
struct AuthorBooksResponse {
    data: Option<AuthorBooksData>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Debug, Deserialize)]
struct AuthorBooksData {
    series_slots: Vec<SeriesSlotEntry>,
    standalones: Vec<StandaloneBookData>,
    author: Option<AuthorMetaData>,
}

#[derive(Debug, Deserialize)]
struct SeriesSlotEntry {
    series: Option<SeriesData>,
    position: Option<f64>,
    featured: Option<bool>,
    book: Option<SlotBookData>,
}

#[derive(Debug, Deserialize)]
struct SlotBookData {
    id: i64,
    slug: Option<String>,
    title: String,
    description: Option<String>,
    pages: Option<i32>,
    release_date: Option<String>,
    image: Option<ImageData>,
    rating: Option<f64>,
    ratings_count: Option<i32>,
    canonical_id: Option<i64>,
    compilation: Option<bool>,
    featured_book_series_id: Option<i64>,
    book_status_id: Option<i32>,
    book_category_id: Option<i32>,
    subtitle: Option<String>,
    users_read_count: Option<i32>,
    default_physical_edition_id: Option<i64>,
    default_ebook_edition_id: Option<i64>,
    default_audio_edition_id: Option<i64>,
    default_cover_edition_id: Option<i64>,
    cached_image: Option<serde_json::Value>,
    users_count: Option<i32>,
    release_year: Option<i32>,
    editions_count: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct StandaloneBookData {
    id: i64,
    slug: Option<String>,
    title: String,
    description: Option<String>,
    pages: Option<i32>,
    release_date: Option<String>,
    image: Option<ImageData>,
    rating: Option<f64>,
    ratings_count: Option<i32>,
    canonical_id: Option<i64>,
    compilation: Option<bool>,
    featured_book_series_id: Option<i64>,
    book_status_id: Option<i32>,
    book_category_id: Option<i32>,
    subtitle: Option<String>,
    users_read_count: Option<i32>,
    default_physical_edition_id: Option<i64>,
    default_ebook_edition_id: Option<i64>,
    default_audio_edition_id: Option<i64>,
    default_cover_edition_id: Option<i64>,
    cached_image: Option<serde_json::Value>,
    users_count: Option<i32>,
    release_year: Option<i32>,
    editions_count: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct AuthorMetaData {
    id: i64,
    slug: Option<String>,
    name: String,
    bio: Option<String>,
    image: Option<ImageData>,
    books_count: Option<i32>,
}

#[derive(Debug, Deserialize, Clone)]
struct SeriesData {
    id: i64,
    slug: Option<String>,
    name: String,
    books_count: Option<i32>,
    state: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ImageData {
    url: String,
}

// Response types for lookup_canonical
#[derive(Debug, Deserialize)]
struct LookupResponse {
    data: Option<LookupData>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Debug, Deserialize)]
struct LookupData {
    items: Vec<LookupItem>,
}

#[derive(Debug, Deserialize)]
struct LookupItem {
    id: i64,
    canonical_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ByPkResponse {
    data: Option<HashMap<String, Option<LookupItem>>>,
    errors: Option<Vec<GraphQLError>>,
}

// Response types for GetSeriesBooks query
#[derive(Debug, Deserialize)]
struct SeriesBooksResponse {
    data: Option<SeriesBooksData>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Debug, Deserialize)]
struct SeriesBooksData {
    series: Option<SeriesFullData>,
}

#[derive(Debug, Deserialize)]
struct SeriesFullData {
    id: i64,
    slug: Option<String>,
    name: String,
    description: Option<String>,
    primary_books_count: Option<i32>,
    books_count: Option<i32>,
    book_series: Vec<SeriesBookEntry>,
}

#[derive(Debug, Deserialize)]
struct SeriesBookEntry {
    position: Option<f64>,
    featured: Option<bool>,
    book: Option<SeriesBookData>,
}

#[derive(Debug, Deserialize)]
struct SeriesBookData {
    id: i64,
    slug: Option<String>,
    title: String,
    description: Option<String>,
    pages: Option<i32>,
    release_date: Option<String>,
    image: Option<ImageData>,
    rating: Option<f64>,
    ratings_count: Option<i32>,
    canonical_id: Option<i64>,
    compilation: Option<bool>,
    featured_book_series_id: Option<i64>,
    book_status_id: Option<i32>,
    book_category_id: Option<i32>,
    subtitle: Option<String>,
    users_read_count: Option<i32>,
    users_count: Option<i32>,
    default_physical_edition_id: Option<i64>,
    default_ebook_edition_id: Option<i64>,
    default_audio_edition_id: Option<i64>,
    default_cover_edition_id: Option<i64>,
    cached_image: Option<serde_json::Value>,
    release_year: Option<i32>,
    editions_count: Option<i32>,
    contributions: Option<Vec<SeriesBookContribution>>,
}

#[derive(Debug, Deserialize)]
struct SeriesBookContribution {
    author: Option<SeriesBookAuthor>,
}

#[derive(Debug, Deserialize)]
struct SeriesBookAuthor {
    #[allow(dead_code)]
    id: i64,
    name: String,
}
