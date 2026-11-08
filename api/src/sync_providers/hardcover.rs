//! Hardcover sync provider implementation
//!
//! Syncs reading activity to Hardcover via their GraphQL API.

// Allow dead_code for GraphQL response structs that are only used for deserialization
#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::types::{
    ConfigField, ConfigFieldType, PullSyncBook, PullSyncStatus, SyncBookResult, SyncCapabilities,
    SyncError, SyncProviderInfo, SyncResult,
};
use super::{SyncProvider, SyncProviderFactory};

// ============================================================================
// Provider Info
// ============================================================================

/// Configuration fields for Hardcover sync
pub static CONFIG_FIELDS: &[ConfigField] = &[ConfigField {
    key: "api_key",
    label: "API Key",
    field_type: ConfigFieldType::Secret,
    required: true,
    description: Some("Get your API key from Hardcover account settings"),
    help_url: Some("https://hardcover.app/account/api"),
}];

/// Static provider information
pub static INFO: SyncProviderInfo = SyncProviderInfo {
    id: crate::http::provider_ids::HARDCOVER,
    display_name: "Hardcover",
    description: "Sync reading activity to Hardcover, a modern book tracking platform.",
    website: "https://hardcover.app",
    capabilities: SyncCapabilities {
        mark_reading: true,
        mark_finished: true,
        progress: true,
        rating: true,
        pull_sync: true,
        edition_resolution: true,
    },
    config_fields: CONFIG_FIELDS,
};

// ============================================================================
// Factory
// ============================================================================

/// Factory for creating Hardcover sync provider instances
pub struct HardcoverSyncFactory {
    base_url_override: Option<String>,
}

impl HardcoverSyncFactory {
    pub fn new(base_url_override: Option<String>) -> Self {
        Self { base_url_override }
    }
}

impl SyncProviderFactory for HardcoverSyncFactory {
    fn info(&self) -> &'static SyncProviderInfo {
        &INFO
    }

    fn create(&self, client: reqwest::Client, api_key: &str) -> Box<dyn SyncProvider> {
        let base_url = self
            .base_url_override
            .clone()
            .unwrap_or_else(|| "https://api.hardcover.app".to_string());

        Box::new(HardcoverSyncProvider {
            client: crate::http::ProviderClient::new(client, crate::http::provider_ids::HARDCOVER),
            base_url,
            api_key: api_key.to_string(),
        })
    }
}

// ============================================================================
// Provider Implementation
// ============================================================================

/// Hardcover GraphQL API sync provider
pub struct HardcoverSyncProvider {
    client: crate::http::ProviderClient,
    base_url: String,
    api_key: String,
}

/// Resolve the best Hardcover edition_id from provider_metadata.
/// Prefers physical > ebook > cover, because Hardcover's book-level page count
/// corresponds to the physical edition. Using the ebook edition (often Kindle)
/// causes page count mismatches.
fn resolve_hardcover_edition_id(provider_metadata: &serde_json::Value) -> Option<i64> {
    let hc = provider_metadata.get("hardcover")?;
    hc.get("default_physical_edition_id")
        .and_then(|v| v.as_i64())
        .or_else(|| hc.get("default_ebook_edition_id").and_then(|v| v.as_i64()))
        .or_else(|| hc.get("default_cover_edition_id").and_then(|v| v.as_i64()))
}

impl HardcoverSyncProvider {
    /// Create a new Hardcover sync provider.
    pub fn new(
        client: reqwest::Client,
        api_key: String,
        base_url_override: Option<String>,
    ) -> Self {
        let base_url = base_url_override.unwrap_or_else(|| "https://api.hardcover.app".to_string());
        Self {
            client: crate::http::ProviderClient::new(client, crate::http::provider_ids::HARDCOVER),
            base_url,
            api_key,
        }
    }

    async fn graphql<T: for<'de> Deserialize<'de>>(
        &self,
        query: &str,
        variables: impl Serialize,
    ) -> Result<T, SyncError> {
        if self.api_key.is_empty() {
            return Err(SyncError::ApiKeyRequired);
        }

        let request = GraphQLRequest {
            query: query.to_string(),
            variables,
        };

        let response = self
            .client
            .execute(
                self.client
                    .post(&format!("{}/v1/graphql", self.base_url))
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .json(&request),
            )
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            tracing::warn!("Hardcover API error: {} - {}", status, text);
            return Err(SyncError::from_http_status(status, "Hardcover API"));
        }

        let result: GraphQLResponse<T> = response.json().await?;

        if let Some(errors) = result.errors
            && !errors.is_empty()
        {
            let msg = errors
                .iter()
                .map(|e| e.message.clone())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(SyncError::Other(format!("GraphQL errors: {}", msg)));
        }

        result
            .data
            .ok_or_else(|| SyncError::Parse("No data in response".to_string()))
    }

    /// Get or create a user_book entry, returning the user_book_id.
    /// Uses the new Hardcover `insert_user_book` mutation which upserts.
    async fn get_or_create_user_book(
        &self,
        book_id: &str,
        status_id: i32,
        edition_id: Option<i64>,
    ) -> Result<i64, SyncError> {
        let book_id_int: i64 = book_id
            .parse()
            .map_err(|_| SyncError::Parse("Invalid book ID".to_string()))?;

        #[derive(Serialize)]
        struct Variables {
            object: UserBookObject,
        }

        #[derive(Serialize)]
        struct UserBookObject {
            book_id: i64,
            status_id: i32,
            #[serde(skip_serializing_if = "Option::is_none")]
            edition_id: Option<i64>,
        }

        #[derive(Deserialize)]
        struct InsertResult {
            insert_user_book: InsertUserBookData,
        }

        #[derive(Deserialize)]
        struct InsertUserBookData {
            user_book: Option<UserBookResponse>,
            error: Option<String>,
        }

        #[derive(Deserialize)]
        struct UserBookResponse {
            id: i64,
        }

        let result: InsertResult = self
            .graphql(
                r#"
                mutation InsertUserBook($object: UserBookCreateInput!) {
                    insert_user_book(object: $object) {
                        error
                        user_book {
                            id
                        }
                    }
                }
                "#,
                Variables {
                    object: UserBookObject {
                        book_id: book_id_int,
                        status_id,
                        edition_id,
                    },
                },
            )
            .await?;

        if let Some(err) = result.insert_user_book.error {
            return Err(SyncError::Other(format!("insert_user_book error: {}", err)));
        }

        result
            .insert_user_book
            .user_book
            .map(|ub| ub.id)
            .ok_or_else(|| SyncError::Other("No user_book returned".to_string()))
    }
}

#[async_trait]
impl SyncProvider for HardcoverSyncProvider {
    fn info(&self) -> &'static SyncProviderInfo {
        &INFO
    }

    fn identifier_key(&self) -> &'static str {
        crate::http::provider_ids::HARDCOVER
    }

    async fn mark_reading(
        &self,
        book_id: &str,
        provider_metadata: &serde_json::Value,
    ) -> Result<SyncResult, SyncError> {
        let edition_id = resolve_hardcover_edition_id(provider_metadata);
        // Status 2 = Currently Reading
        let user_book_id = self.get_or_create_user_book(book_id, 2, edition_id).await?;

        tracing::info!(
            book_id = book_id,
            user_book_id = user_book_id,
            "Marked book as reading on Hardcover"
        );

        Ok(SyncResult::success()
            .with_user_book_id(user_book_id.to_string())
            .with_message("Marked as currently reading"))
    }

    async fn mark_finished(
        &self,
        book_id: &str,
        existing_read_id: Option<i64>,
        provider_metadata: &serde_json::Value,
    ) -> Result<SyncResult, SyncError> {
        let edition_id = resolve_hardcover_edition_id(provider_metadata);
        // Status 3 = Read
        let user_book_id = self.get_or_create_user_book(book_id, 3, edition_id).await?;

        // If we have a stored read ID, close it with finished_at
        if let Some(read_id) = existing_read_id {
            let today = Utc::now().format("%Y-%m-%d").to_string();

            #[derive(Serialize)]
            struct Variables {
                id: i64,
                object: ReadUpdateObject,
            }

            #[derive(Serialize)]
            struct ReadUpdateObject {
                finished_at: String,
                #[serde(skip_serializing_if = "Option::is_none")]
                edition_id: Option<i64>,
            }

            #[derive(Deserialize)]
            struct UpdateResult {
                update_user_book_read: UpdateReadData,
            }

            #[derive(Deserialize)]
            struct UpdateReadData {
                error: Option<String>,
            }

            let result: UpdateResult = self
                .graphql(
                    r#"
                    mutation UpdateUserBookRead($id: Int!, $object: DatesReadInput!) {
                        update_user_book_read(id: $id, object: $object) {
                            error
                        }
                    }
                    "#,
                    Variables {
                        id: read_id,
                        object: ReadUpdateObject {
                            finished_at: today,
                            edition_id,
                        },
                    },
                )
                .await?;

            if let Some(err) = result.update_user_book_read.error {
                tracing::warn!(
                    read_id = read_id,
                    error = %err,
                    "Failed to close read on Hardcover, continuing"
                );
            } else {
                tracing::info!(
                    book_id = book_id,
                    read_id = read_id,
                    "Closed read on Hardcover with finished_at"
                );
            }
        }

        tracing::info!(
            book_id = book_id,
            user_book_id = user_book_id,
            "Marked book as finished on Hardcover"
        );

        Ok(SyncResult::success()
            .with_user_book_id(user_book_id.to_string())
            .with_message("Marked as read"))
    }

    async fn update_progress(
        &self,
        book_id: &str,
        existing_read_id: Option<i64>,
        percent: f64,
        page_count: Option<i32>,
        started_at: Option<&str>,
        provider_metadata: &serde_json::Value,
    ) -> Result<SyncResult, SyncError> {
        let edition_id = resolve_hardcover_edition_id(provider_metadata);
        // Ensure user_book exists with status 2 = Currently Reading
        let user_book_id = self.get_or_create_user_book(book_id, 2, edition_id).await?;

        let progress_pages = page_count
            .filter(|&p| p > 0)
            .map(|pages| ((percent / 100.0) * pages as f64).round() as i32);

        if let Some(read_id) = existing_read_id {
            // UPDATE existing read
            #[derive(Serialize)]
            struct Variables {
                id: i64,
                object: ReadUpdateObject,
            }

            #[derive(Serialize)]
            struct ReadUpdateObject {
                #[serde(skip_serializing_if = "Option::is_none")]
                progress_pages: Option<i32>,
                #[serde(skip_serializing_if = "Option::is_none")]
                started_at: Option<String>,
                #[serde(skip_serializing_if = "Option::is_none")]
                edition_id: Option<i64>,
            }

            #[derive(Deserialize)]
            struct UpdateResult {
                update_user_book_read: UpdateReadData,
            }

            #[derive(Deserialize)]
            struct UpdateReadData {
                error: Option<String>,
                user_book_read: Option<UserBookReadResponse>,
            }

            #[derive(Deserialize)]
            struct UserBookReadResponse {
                id: i64,
            }

            let result: UpdateResult = self
                .graphql(
                    r#"
                    mutation UpdateUserBookRead($id: Int!, $object: DatesReadInput!) {
                        update_user_book_read(id: $id, object: $object) {
                            error
                            user_book_read {
                                id
                            }
                        }
                    }
                    "#,
                    Variables {
                        id: read_id,
                        object: ReadUpdateObject {
                            progress_pages,
                            started_at: started_at.map(String::from),
                            edition_id,
                        },
                    },
                )
                .await?;

            if let Some(err) = result.update_user_book_read.error {
                return Err(SyncError::Other(format!(
                    "update_user_book_read error: {}",
                    err
                )));
            }

            tracing::debug!(
                book_id = book_id,
                read_id = read_id,
                progress_pages = ?progress_pages,
                "Updated existing read on Hardcover ({:.0}%)",
                percent,
            );

            return Ok(SyncResult::success()
                .with_user_book_id(user_book_id.to_string())
                .with_read_id(read_id)
                .with_message(format!("Updated read {} ({:.0}%)", read_id, percent)));
        }

        // INSERT new read
        #[derive(Serialize)]
        struct InsertVariables {
            id: i64,
            user_book_read: InsertReadObject,
        }

        #[derive(Serialize)]
        struct InsertReadObject {
            #[serde(skip_serializing_if = "Option::is_none")]
            progress_pages: Option<i32>,
            #[serde(skip_serializing_if = "Option::is_none")]
            started_at: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            edition_id: Option<i64>,
        }

        #[derive(Deserialize)]
        struct InsertResult {
            insert_user_book_read: InsertReadData,
        }

        #[derive(Deserialize)]
        struct InsertReadData {
            error: Option<String>,
            user_book_read: Option<UserBookReadInsertResponse>,
        }

        #[derive(Deserialize)]
        struct UserBookReadInsertResponse {
            id: i64,
        }

        let result: InsertResult = self
            .graphql(
                r#"
                mutation InsertUserBookRead($id: Int!, $user_book_read: DatesReadInput!) {
                    insert_user_book_read(user_book_id: $id, user_book_read: $user_book_read) {
                        error
                        user_book_read {
                            id
                        }
                    }
                }
                "#,
                InsertVariables {
                    id: user_book_id,
                    user_book_read: InsertReadObject {
                        progress_pages,
                        started_at: started_at.map(String::from),
                        edition_id,
                    },
                },
            )
            .await?;

        if let Some(err) = result.insert_user_book_read.error {
            return Err(SyncError::Other(format!(
                "insert_user_book_read error: {}",
                err
            )));
        }

        let new_read_id = result.insert_user_book_read.user_book_read.map(|r| r.id);

        tracing::debug!(
            book_id = book_id,
            user_book_id = user_book_id,
            new_read_id = ?new_read_id,
            progress_pages = ?progress_pages,
            "Inserted new read on Hardcover ({:.0}%)",
            percent,
        );

        let mut sync_result = SyncResult::success()
            .with_user_book_id(user_book_id.to_string())
            .with_message(format!("New read ({:.0}%)", percent));

        if let Some(id) = new_read_id {
            sync_result = sync_result.with_read_id(id);
        }

        Ok(sync_result)
    }

    async fn set_rating(
        &self,
        book_id: &str,
        rating: i32,
        provider_metadata: &serde_json::Value,
    ) -> Result<SyncResult, SyncError> {
        let edition_id = resolve_hardcover_edition_id(provider_metadata);
        // Clamp rating to 1-5
        let rating = rating.clamp(1, 5);

        // Ensure user_book exists (if new, create with status 3 = Read)
        let user_book_id = self.get_or_create_user_book(book_id, 3, edition_id).await?;

        // Update rating using the new update_user_book mutation
        #[derive(Serialize)]
        struct Variables {
            id: i64,
            object: RatingObject,
        }

        #[derive(Serialize)]
        struct RatingObject {
            rating: i32,
        }

        #[derive(Deserialize)]
        struct UpdateResult {
            update_user_book: UpdateUserBookData,
        }

        #[derive(Deserialize)]
        struct UpdateUserBookData {
            error: Option<String>,
        }

        let result: UpdateResult = self
            .graphql(
                r#"
                mutation SetRating($id: Int!, $object: UserBookUpdateInput!) {
                    update_user_book(id: $id, object: $object) {
                        error
                    }
                }
                "#,
                Variables {
                    id: user_book_id,
                    object: RatingObject { rating },
                },
            )
            .await?;

        if let Some(err) = result.update_user_book.error {
            return Err(SyncError::Other(format!("update_user_book error: {}", err)));
        }

        tracing::info!(
            book_id = book_id,
            user_book_id = user_book_id,
            rating = rating,
            "Set rating on Hardcover"
        );

        Ok(SyncResult::success()
            .with_user_book_id(user_book_id.to_string())
            .with_message(format!("Rating set to {} stars", rating)))
    }

    fn resolve_edition_id(&self, provider_metadata: &serde_json::Value) -> Option<i64> {
        resolve_hardcover_edition_id(provider_metadata)
    }

    async fn pull_user_books(&self) -> Result<Vec<PullSyncBook>, SyncError> {
        let hc_books = self.get_user_books().await?;
        Ok(hc_books
            .into_iter()
            .map(|ub| {
                let status = ub.status_id.and_then(|s| match s {
                    1 => Some(PullSyncStatus::WantToRead),
                    2 => Some(PullSyncStatus::Reading),
                    3 => Some(PullSyncStatus::Finished),
                    4 => Some(PullSyncStatus::DidNotFinish),
                    _ => None,
                });
                PullSyncBook {
                    external_id: ub.book_id,
                    status,
                    progress: ub.progress,
                    rating: ub.rating,
                    updated_at: ub.updated_at,
                    active_read_id: ub.active_read_id,
                }
            })
            .collect())
    }

    async fn search_book(&self, query: &str) -> Result<Vec<SyncBookResult>, SyncError> {
        #[derive(Serialize)]
        struct Variables {
            query: String,
            per_page: i32,
        }

        #[derive(Deserialize)]
        struct SearchResult {
            search: SearchData,
        }

        #[derive(Deserialize)]
        struct SearchData {
            results: Option<serde_json::Value>,
        }

        let result: SearchResult = self
            .graphql(
                r#"
                query SearchBooks($query: String!, $per_page: Int!) {
                    search(query: $query, query_type: "Book", per_page: $per_page) {
                        results
                    }
                }
                "#,
                Variables {
                    query: query.to_string(),
                    per_page: 10,
                },
            )
            .await?;

        let hits = result
            .search
            .results
            .and_then(|r| r.get("hits").and_then(|h| h.as_array()).cloned())
            .unwrap_or_default();

        let results = hits
            .iter()
            .filter_map(|hit| {
                let doc = hit.get("document")?;

                let external_id = doc.get("id")?.as_str()?.to_string();
                let title = doc.get("title")?.as_str()?.to_string();

                let authors = doc
                    .get("author_names")
                    .and_then(|a| a.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|a| a.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                let year = doc
                    .get("release_year")
                    .and_then(|y| y.as_i64())
                    .map(|y| y.to_string());

                let cover_url = doc
                    .get("image")
                    .and_then(|img| img.get("url"))
                    .and_then(|u| u.as_str())
                    .map(String::from);

                Some(SyncBookResult {
                    external_id,
                    title,
                    authors,
                    year,
                    cover_url,
                    confidence: 0.8,
                })
            })
            .collect();

        Ok(results)
    }
}

// ============================================================================
// Pull Functionality (for background sync from Hardcover)
// ============================================================================

/// A user's book data from Hardcover (for pull sync).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardcoverUserBook {
    /// Hardcover book ID
    pub book_id: String,
    /// Reading status: 1=Want to Read, 2=Reading, 3=Read, 4=Did Not Finish
    pub status_id: Option<i32>,
    /// Progress 0.0-1.0 (from latest user_book_read)
    pub progress: Option<f64>,
    /// Rating 1-5
    pub rating: Option<i32>,
    /// When this user_book was last updated
    pub updated_at: Option<String>,
    /// The in-progress user_book_read ID (None if no active read)
    pub active_read_id: Option<i64>,
}

impl HardcoverSyncProvider {
    /// Fetch all of the user's books from Hardcover.
    /// This is used for background pull sync.
    /// Paginates in pages of 100 (Hasura hard limit).
    pub async fn get_user_books(&self) -> Result<Vec<HardcoverUserBook>, SyncError> {
        #[derive(Serialize)]
        struct Variables {
            offset: i32,
        }

        #[derive(Deserialize)]
        struct QueryResult {
            me: Vec<MeData>,
        }

        #[derive(Deserialize)]
        struct MeData {
            user_books: Vec<UserBookData>,
        }

        #[derive(Deserialize)]
        struct UserBookData {
            book_id: i64,
            status_id: Option<i32>,
            // Hardcover's rating is `numeric` with half stars (5.0, 3.5); ours is whole 1-5.
            rating: Option<f64>,
            updated_at: Option<String>,
            user_book_reads: Vec<UserBookReadData>,
        }

        #[derive(Deserialize)]
        struct UserBookReadData {
            id: Option<i64>,
            progress: Option<f64>,
            finished_at: Option<String>,
        }

        let mut all_books = Vec::new();
        let mut offset = 0;

        loop {
            let result: QueryResult = self
                .graphql(
                    r#"
                    query GetMyBooks($offset: Int!) {
                        me {
                            user_books(limit: 100, offset: $offset, order_by: { updated_at: desc }) {
                                book_id
                                status_id
                                rating
                                updated_at
                                user_book_reads(limit: 1, order_by: { id: desc }) {
                                    id
                                    progress
                                    finished_at
                                }
                            }
                        }
                    }
                    "#,
                    Variables { offset },
                )
                .await?;

            let page_books: Vec<UserBookData> =
                result.me.into_iter().flat_map(|me| me.user_books).collect();
            let page_len = page_books.len();

            for ub in page_books {
                let latest_read = ub.user_book_reads.into_iter().next();
                let progress = latest_read.as_ref().and_then(|r| r.progress);

                // Only store the read ID if the read is in-progress (no finished_at)
                let active_read_id = latest_read
                    .as_ref()
                    .filter(|r| r.finished_at.is_none())
                    .and_then(|r| r.id);

                all_books.push(HardcoverUserBook {
                    book_id: ub.book_id.to_string(),
                    status_id: ub.status_id,
                    progress,
                    rating: ub.rating.map(|r| (r.round() as i32).clamp(1, 5)),
                    updated_at: ub.updated_at,
                    active_read_id,
                });
            }

            if page_len < 100 {
                break;
            }
            offset += 100;
        }

        Ok(all_books)
    }
}

// ============================================================================
// GraphQL Types
// ============================================================================

#[derive(Serialize)]
struct GraphQLRequest<V> {
    query: String,
    variables: V,
}

#[derive(Deserialize)]
struct GraphQLResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Deserialize)]
struct GraphQLError {
    message: String,
}
