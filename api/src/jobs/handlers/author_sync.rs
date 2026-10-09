//! Author sync check job handler.
//!
//! Syncs metadata for all authors that have a tracking provider ID.
//! - Syncs ALL authors with provider IDs (not just tracked ones)
//! - Tracking (is_monitored=TRUE) only controls auto-want behavior for discovered books
//! - Uses consolidated schema from migration 0019:
//!   - Authors with identifiers->>'{provider}' for sync eligibility
//!   - Books with is_in_library=FALSE for discovered books not yet owned

use crate::images::ImageEntry;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{
    authors::{self, AuthorRefreshInput, AuthorSyncInfo},
    book_series,
    books::{self, DiscoveredBookInput},
    metadata_providers,
    series::{self, SeriesInput},
};
use crate::tracking::CachedAuthorProvider;
use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

/// The tracking provider used for author/series sync.
/// Uses the system-wide metadata provider API key (not per-user sync provider key).
const TRACKING_PROVIDER: &str = "hardcover";

/// Get the tracking provider API key from metadata providers config.
async fn get_tracking_api_key(pool: &PgPool) -> Result<String, JobError> {
    metadata_providers::get_api_key(pool, TRACKING_PROVIDER)
        .await?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            JobError::ExecutionFailed(format!("{} API key not configured", TRACKING_PROVIDER))
        })
}

/// Create a cached tracking provider instance.
fn create_provider(api_key: String, pool: PgPool) -> Result<CachedAuthorProvider, JobError> {
    crate::tracking::create_tracking_provider(TRACKING_PROVIDER, api_key, pool).ok_or_else(|| {
        JobError::ExecutionFailed(format!("Unknown tracking provider: {}", TRACKING_PROVIDER))
    })
}

/// Build identifier JSON for a book/series/author from this provider.
fn build_identifiers(provider_id: &str, external_id: &str, slug: Option<&str>) -> Value {
    let slug_key = format!("{}_slug", provider_id);
    let mut ids = json!({ provider_id: external_id });
    if let Some(slug) = slug {
        ids[slug_key] = json!(slug);
    }
    ids
}

/// Add all series memberships for a book from the tracking data.
/// Falls back to single series_name/series_id if series_memberships is empty.
/// Returns the first series ID found (for auto-want mode lookup).
async fn add_all_series_memberships(
    pool: &PgPool,
    pid: &str,
    book_id: Uuid,
    book: &crate::tracking::hardcover::AuthorBook,
) -> Option<Uuid> {
    use crate::tracking::hardcover::SeriesMembership;

    let memberships: Vec<SeriesMembership> = if book.series_memberships.is_empty() {
        // Backward compat: use single series fields
        let name = book.series_name.as_ref()?;
        vec![SeriesMembership {
            series_name: name.clone(),
            series_id: book.series_id.clone(),
            series_slug: book.series_slug.clone(),
            series_index: book.series_index,
            books_count: None,
        }]
    } else {
        book.series_memberships.clone()
    };

    let mut first_series_id = None;
    for membership in &memberships {
        let sid = find_or_create_series(
            pool,
            pid,
            &membership.series_name,
            membership.series_id.as_deref(),
            membership.series_slug.as_deref(),
        )
        .await;

        if let Some(sid) = sid {
            let _ = book_series::add(pool, book_id, sid, membership.series_index, false).await;

            // Update total_book_count on the series if known from provider
            if let Some(count) = membership.books_count {
                let _ = sqlx::query(
                    "UPDATE series SET total_book_count = $2 WHERE id = $1 AND (total_book_count IS NULL OR total_book_count != $2)",
                )
                .bind(sid)
                .bind(count)
                .execute(pool)
                .await;
            }

            if first_series_id.is_none() {
                first_series_id = Some(sid);
            }
        }
    }

    // Auto-select primary (picks most specific = lowest total_book_count)
    let _ = book_series::auto_select_primary(pool, book_id, false).await;

    first_series_id
}

/// Find or create a series by name/ID using the repository layer.
async fn find_or_create_series(
    pool: &PgPool,
    provider_id: &str,
    name: &str,
    external_id: Option<&str>,
    external_slug: Option<&str>,
) -> Option<Uuid> {
    let slug_key = format!("{}_slug", provider_id);

    // First try to find by provider ID
    if let Some(ext_id) = external_id
        && let Ok(Some(id)) = series::find_by_identifier(pool, provider_id, ext_id).await
    {
        // Merge slug into existing series identifiers
        if let Some(slug) = external_slug {
            let slug_ids = Some(json!({ slug_key: slug }));
            series::merge_identifiers(pool, id, &slug_ids).await;
        }
        return Some(id);
    }

    // Build identifiers
    let identifiers = external_id.map(|id| build_identifiers(provider_id, id, external_slug));

    let input = SeriesInput {
        name: name.to_string(),
        identifiers,
        ..Default::default()
    };

    match series::upsert(pool, input).await {
        Ok(id) => Some(id),
        Err(e) => {
            warn!(error = %e, name = %name, "Failed to create series");
            None
        }
    }
}

/// Handler for author sync check jobs.
pub struct AuthorSyncCheckHandler {
    pool: PgPool,
    nats_publisher: nats_jobs::Publisher,
}

impl AuthorSyncCheckHandler {
    pub fn new(pool: PgPool, nats_publisher: nats_jobs::Publisher) -> Self {
        Self {
            pool,
            nats_publisher,
        }
    }

    /// Check a single tracked author and update their books.
    /// Returns (new_books, wanted_created, api_called).
    async fn check_single_author(
        &self,
        provider: &CachedAuthorProvider,
        author: &AuthorSyncInfo,
    ) -> Result<(i32, i32, bool), JobError> {
        let pid = provider.provider_id();
        let slug_key = format!("{}_slug", pid);

        // Fetch author books from provider (with caching)
        let outcome = provider
            .get_author_books(&author.hardcover_id)
            .await
            .map_err(|e| JobError::retryable(e.to_string()))?;
        let api_called = !outcome.cached;
        let author_data = outcome.data;

        // Update author info
        let refresh_input = AuthorRefreshInput {
            name: author_data.name.clone(),
            images: author_data.image_url.as_ref().map(|url| {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    "profile".to_string(),
                    ImageEntry {
                        source_url: Some(url.clone()),
                        cached_at: None,
                    },
                );
                m
            }),
            bio: author_data.bio.clone(),
            known_book_count: Some(author_data.books.len() as i32),
        };
        if let Err(e) = authors::refresh_metadata(&self.pool, author.id, refresh_input).await {
            error!(error = %e, "Failed to update author");
        }

        // Store author slug and provider metadata in identifiers
        if let Some(ref slug) = author_data.slug {
            authors::merge_identifiers(&self.pool, author.id, &json!({ slug_key.clone(): slug }))
                .await;
        }
        // Store author-level provider metadata
        if let Some(ref meta) = author_data.provider_metadata {
            let _ = sqlx::query!(
                "UPDATE authors SET metadata = COALESCE(metadata, '{}'::jsonb) || $2 WHERE id = $1",
                author.id,
                meta,
            )
            .execute(&self.pool)
            .await;
        }

        let mut new_books = 0;
        let mut wanted_created = 0;

        for book in &author_data.books {
            // Check if book already exists by provider ID
            let existing = books::find_by_identifier(&self.pool, pid, &book.id)
                .await
                .map_err(|e| JobError::ExecutionFailed(e.to_string()))?;

            match existing {
                Some(match_info) => {
                    let book_id = match_info.id;

                    // Skip metadata updates for user-edited books to preserve customizations
                    if match_info.user_edited_at.is_none() {
                        let pubdate = book.release_date.map(|d| d.midnight().assume_utc());
                        let update = books::BookUpdate {
                            description: book.description.clone(),
                            images: book.cover_url.as_ref().map(|url| {
                                let mut m = std::collections::HashMap::new();
                                m.insert(
                                    "cover".to_string(),
                                    ImageEntry {
                                        source_url: Some(url.clone()),
                                        cached_at: None,
                                    },
                                );
                                m
                            }),
                            pubdate,
                            is_released: Some(book.is_released),
                            page_count: book.pages,
                            rating: book.rating,
                            rating_count: book.ratings_count,
                            provider_metadata: book.provider_metadata.clone(),
                            ..Default::default()
                        };
                        let _ = books::update(&self.pool, book_id, update).await;
                    }
                }
                None => {
                    // No match by provider ID — check if a library book with same
                    // title + author already exists (e.g. audiobook imported from
                    // media root before enrichment assigned a hardcover ID).
                    let library_match = books::find_library_book_by_title_and_author_id(
                        &self.pool,
                        &book.title,
                        author.id,
                    )
                    .await
                    .ok()
                    .flatten();

                    if let Some(lib_book) = library_match {
                        // Update existing library book with identifiers and series info
                        let identifiers = build_identifiers(pid, &book.id, book.slug.as_deref());

                        if lib_book.user_edited_at.is_none() {
                            let pubdate = book.release_date.map(|d| d.midnight().assume_utc());
                            let update = books::BookUpdate {
                                description: book.description.clone(),
                                images: book.cover_url.as_ref().map(|url| {
                                    let mut m = std::collections::HashMap::new();
                                    m.insert(
                                        "cover".to_string(),
                                        ImageEntry {
                                            source_url: Some(url.clone()),
                                            cached_at: None,
                                        },
                                    );
                                    m
                                }),
                                pubdate,
                                identifiers: Some(identifiers),
                                is_released: Some(book.is_released),
                                page_count: book.pages,
                                rating: book.rating,
                                rating_count: book.ratings_count,
                                provider_metadata: book.provider_metadata.clone(),
                                ..Default::default()
                            };
                            let _ = books::update(&self.pool, lib_book.id, update).await;
                        }

                        // Add ALL series memberships via junction table
                        add_all_series_memberships(&self.pool, pid, lib_book.id, book).await;

                        info!(
                            book_id = %lib_book.id,
                            title = %book.title,
                            "Linked discovered book to existing library copy"
                        );
                        continue;
                    }

                    // No library match — create discovered book
                    new_books += 1;

                    // Find or create primary series for auto-want mode lookup
                    let series_id = if let Some(ref series_name) = book.series_name {
                        find_or_create_series(
                            &self.pool,
                            pid,
                            series_name,
                            book.series_id.as_deref(),
                            book.series_slug.as_deref(),
                        )
                        .await
                    } else {
                        None
                    };

                    // Get effective auto_want_mode (series explicit override > author)
                    // Always use author.monitoring_started_at for date checks
                    let effective_mode = if let Some(sid) = series_id {
                        // Check if series has explicit override (not 'inherit')
                        let series_mode = series::get_auto_want_mode(&self.pool, sid)
                            .await
                            .ok()
                            .flatten();

                        match series_mode.as_deref() {
                            Some("inherit") | None => author.auto_want_mode.clone(),
                            Some(mode) => mode.to_string(),
                        }
                    } else {
                        author.auto_want_mode.clone()
                    };

                    // Determine want_status based on effective auto_want_mode
                    // Auto-want only applies when author is tracked
                    let want_status = if author.is_monitored
                        && book.is_released
                        && effective_mode != "none"
                    {
                        let should_want = match effective_mode.as_str() {
                            "all" => true,
                            "new_only" => match (book.release_date, author.monitoring_started_at) {
                                (Some(d), Some(started_at)) => {
                                    let release_dt = d.midnight().assume_utc();
                                    release_dt > started_at
                                }
                                _ => true,
                            },
                            "fill_gaps" => true,
                            _ => false,
                        };
                        if should_want { "wanted" } else { "pending" }
                    } else {
                        "pending"
                    };

                    // Create the book
                    let identifiers = build_identifiers(pid, &book.id, book.slug.as_deref());
                    let input = DiscoveredBookInput {
                        title: book.title.clone(),
                        description: book.description.clone(),
                        identifiers: Some(identifiers),
                        images: book.cover_url.as_ref().map(|url| {
                            let mut m = std::collections::HashMap::new();
                            m.insert(
                                "cover".to_string(),
                                ImageEntry {
                                    source_url: Some(url.clone()),
                                    cached_at: None,
                                },
                            );
                            m
                        }),
                        release_date: book.release_date,
                        is_released: Some(book.is_released),
                        want_status: Some(want_status.to_string()),
                        discovered_from_author_id: Some(author.id),
                        page_count: book.pages,
                        rating: book.rating,
                        rating_count: book.ratings_count,
                        provider_metadata: book.provider_metadata.clone(),
                        ..Default::default()
                    };

                    let book_id = match books::create_discovered_book(&self.pool, input).await {
                        Ok(id) => id,
                        Err(e) => {
                            error!(error = %e, book_title = %book.title, "Failed to create book");
                            continue;
                        }
                    };

                    // Link book to author
                    let _ = authors::link_book(&self.pool, book_id, author.id).await;

                    // Add ALL series memberships via junction table
                    add_all_series_memberships(&self.pool, pid, book_id, book).await;

                    if want_status == "wanted" {
                        wanted_created += 1;
                        if let Err(e) = self
                            .nats_publisher
                            .enqueue_job("kobbler", "book_want", json!({ "book_id": book_id }))
                            .await
                        {
                            warn!(book_id = %book_id, error = %e, "Failed to enqueue book_want job");
                        }
                    }
                }
            }
        }

        // Try to match non-library books to library books
        if let Err(e) = self.match_books_to_library(author.id).await {
            warn!(error = %e, "Failed to match books to library");
        }

        // Clean up stale discovered books no longer in provider results
        let current_ids: Vec<String> = author_data.books.iter().map(|b| b.id.clone()).collect();
        match books::delete_stale_discovered(&self.pool, author.id, &current_ids).await {
            Ok(deleted) if deleted > 0 => {
                info!(deleted = deleted, author_id = %author.id, "Cleaned up stale discovered books");
            }
            Err(e) => {
                warn!(error = %e, "Failed to clean up stale discovered books");
            }
            _ => {}
        }

        Ok((new_books, wanted_created, api_called))
    }

    /// Try to match non-library books to existing library books by title + author.
    async fn match_books_to_library(&self, author_id: Uuid) -> Result<(), JobError> {
        let matched = books::match_discovered_to_library(&self.pool, author_id).await?;

        if matched > 0 {
            info!(
                matched = matched,
                author_id = %author_id,
                "Matched discovered books to library"
            );
        }

        Ok(())
    }
}

#[async_trait]
impl JobHandler for AuthorSyncCheckHandler {
    fn job_type(&self) -> &'static str {
        "author_sync_check"
    }

    /// Only 1 at a time - external API calls may be rate limited.
    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        _payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        // Get up to 3 authors due for checking
        // Sync ALL authors with provider IDs (not just tracked ones)
        // Tracking only controls auto-want behavior, not metadata sync
        let author_list = authors::list_due_for_sync(&self.pool, 3).await?;

        if author_list.is_empty() {
            return Ok(json!({ "checked": 0, "message": "No authors due for check" }));
        }

        let api_key = get_tracking_api_key(&self.pool).await?;
        let provider = create_provider(api_key, self.pool.clone())?;
        let mut total_new_books = 0;
        let mut total_wanted = 0;
        let mut checked = 0;

        for author_data in author_list {
            nats_jobs::check_cancelled(&cancel)?;
            let _ = authors::bump_next_refresh(&self.pool, author_data.id).await;

            info!(author_id = %author_data.id, name = %author_data.name, tracked = %author_data.is_monitored, "Syncing author metadata");

            let api_called = match self.check_single_author(&provider, &author_data).await {
                Ok((new_books, wanted_created, api_called)) => {
                    total_new_books += new_books;
                    total_wanted += wanted_created;
                    checked += 1;
                    if new_books > 0 || wanted_created > 0 {
                        info!(
                            author_id = %author_data.id,
                            name = %author_data.name,
                            new_books = new_books,
                            wanted_created = wanted_created,
                            "Author check complete"
                        );
                    }
                    api_called
                }
                Err(e) => {
                    error!(
                        author_id = %author_data.id,
                        name = %author_data.name,
                        error = %e,
                        "Error checking author"
                    );
                    true
                }
            };

            if api_called {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            }
        }

        Ok(json!({
            "checked": checked,
            "new_books": total_new_books,
            "wanted_created": total_wanted
        }))
    }
}

// =============================================================================
// Single Author Enrich Handler
// =============================================================================

/// Handler for enriching a single author's metadata and books.
/// Used when manually refreshing an author or after tracking a new author.
pub struct AuthorEnrichHandler {
    pool: PgPool,
    nats_publisher: nats_jobs::Publisher,
}

impl AuthorEnrichHandler {
    pub fn new(pool: PgPool, nats_publisher: nats_jobs::Publisher) -> Self {
        Self {
            pool,
            nats_publisher,
        }
    }

    /// Sync a single author's books from the tracking provider.
    #[allow(clippy::too_many_arguments)]
    async fn sync_author(
        &self,
        provider: &CachedAuthorProvider,
        author_id: Uuid,
        external_id: &str,
        auto_want_mode: &str,
        monitoring_started_at: Option<OffsetDateTime>,
        is_monitored: bool,
    ) -> Result<(i32, i32), JobError> {
        let pid = provider.provider_id();
        let slug_key = format!("{}_slug", pid);

        let author_data = provider
            .get_author_books(external_id)
            .await
            .map_err(|e| JobError::retryable(e.to_string()))?
            .data;

        // Update author info
        let refresh_input = AuthorRefreshInput {
            name: author_data.name.clone(),
            images: author_data.image_url.as_ref().map(|url| {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    "profile".to_string(),
                    ImageEntry {
                        source_url: Some(url.clone()),
                        cached_at: None,
                    },
                );
                m
            }),
            bio: author_data.bio.clone(),
            known_book_count: Some(author_data.books.len() as i32),
        };
        if let Err(e) = authors::refresh_metadata(&self.pool, author_id, refresh_input).await {
            error!(author_id = %author_id, error = %e, "Failed to update author metadata");
        }

        // Store author slug and provider metadata
        if let Some(ref slug) = author_data.slug {
            authors::merge_identifiers(&self.pool, author_id, &json!({ slug_key.clone(): slug }))
                .await;
        }
        if let Some(ref meta) = author_data.provider_metadata {
            let _ = sqlx::query!(
                "UPDATE authors SET metadata = COALESCE(metadata, '{}'::jsonb) || $2 WHERE id = $1",
                author_id,
                meta,
            )
            .execute(&self.pool)
            .await;
        }

        // Process books
        let mut new_books = 0;
        let mut wanted_created = 0;

        for book in &author_data.books {
            let existing = books::find_by_identifier(&self.pool, pid, &book.id)
                .await
                .map_err(|e| JobError::ExecutionFailed(e.to_string()))?;

            match existing {
                Some(match_info) => {
                    let book_id = match_info.id;

                    if match_info.user_edited_at.is_none() {
                        let pubdate = book.release_date.map(|d| d.midnight().assume_utc());
                        let update = books::BookUpdate {
                            description: book.description.clone(),
                            images: book.cover_url.as_ref().map(|url| {
                                let mut m = std::collections::HashMap::new();
                                m.insert(
                                    "cover".to_string(),
                                    ImageEntry {
                                        source_url: Some(url.clone()),
                                        cached_at: None,
                                    },
                                );
                                m
                            }),
                            pubdate,
                            is_released: Some(book.is_released),
                            page_count: book.pages,
                            rating: book.rating,
                            rating_count: book.ratings_count,
                            provider_metadata: book.provider_metadata.clone(),
                            ..Default::default()
                        };
                        let _ = books::update(&self.pool, book_id, update).await;
                    }
                }
                None => {
                    // No match by provider ID — check if a library book with same
                    // title + author already exists.
                    let library_match = books::find_library_book_by_title_and_author_id(
                        &self.pool,
                        &book.title,
                        author_id,
                    )
                    .await
                    .ok()
                    .flatten();

                    if let Some(lib_book) = library_match {
                        let identifiers = build_identifiers(pid, &book.id, book.slug.as_deref());

                        if lib_book.user_edited_at.is_none() {
                            let pubdate = book.release_date.map(|d| d.midnight().assume_utc());
                            let update = books::BookUpdate {
                                description: book.description.clone(),
                                images: book.cover_url.as_ref().map(|url| {
                                    let mut m = std::collections::HashMap::new();
                                    m.insert(
                                        "cover".to_string(),
                                        ImageEntry {
                                            source_url: Some(url.clone()),
                                            cached_at: None,
                                        },
                                    );
                                    m
                                }),
                                pubdate,
                                identifiers: Some(identifiers),
                                is_released: Some(book.is_released),
                                page_count: book.pages,
                                rating: book.rating,
                                rating_count: book.ratings_count,
                                provider_metadata: book.provider_metadata.clone(),
                                ..Default::default()
                            };
                            let _ = books::update(&self.pool, lib_book.id, update).await;
                        }

                        // Add ALL series memberships via junction table
                        add_all_series_memberships(&self.pool, pid, lib_book.id, book).await;

                        info!(
                            book_id = %lib_book.id,
                            title = %book.title,
                            "Linked discovered book to existing library copy"
                        );
                        continue;
                    }

                    // No library match — create discovered book

                    // Find or create primary series for auto-want mode lookup
                    let series_id = if let Some(ref series_name) = book.series_name {
                        find_or_create_series(
                            &self.pool,
                            pid,
                            series_name,
                            book.series_id.as_deref(),
                            book.series_slug.as_deref(),
                        )
                        .await
                    } else {
                        None
                    };

                    let effective_mode = if let Some(sid) = series_id {
                        let series_mode = series::get_auto_want_mode(&self.pool, sid)
                            .await
                            .ok()
                            .flatten();

                        match series_mode.as_deref() {
                            Some("inherit") | None => auto_want_mode.to_string(),
                            Some(mode) => mode.to_string(),
                        }
                    } else {
                        auto_want_mode.to_string()
                    };

                    let want_status =
                        if is_monitored && book.is_released && effective_mode != "none" {
                            let should_want = match effective_mode.as_str() {
                                "all" => true,
                                "new_only" => match (book.release_date, monitoring_started_at) {
                                    (Some(d), Some(started_at)) => d >= started_at.date(),
                                    _ => true,
                                },
                                "fill_gaps" => true,
                                _ => false,
                            };
                            if should_want { "wanted" } else { "pending" }
                        } else {
                            "pending"
                        };

                    let identifiers = build_identifiers(pid, &book.id, book.slug.as_deref());
                    let input = DiscoveredBookInput {
                        title: book.title.clone(),
                        description: book.description.clone(),
                        identifiers: Some(identifiers),
                        images: book.cover_url.as_ref().map(|url| {
                            let mut m = std::collections::HashMap::new();
                            m.insert(
                                "cover".to_string(),
                                ImageEntry {
                                    source_url: Some(url.clone()),
                                    cached_at: None,
                                },
                            );
                            m
                        }),
                        release_date: book.release_date,
                        is_released: Some(book.is_released),
                        want_status: Some(want_status.to_string()),
                        discovered_from_author_id: Some(author_id),
                        page_count: book.pages,
                        rating: book.rating,
                        rating_count: book.ratings_count,
                        provider_metadata: book.provider_metadata.clone(),
                        ..Default::default()
                    };

                    let book_id = match books::create_discovered_book(&self.pool, input).await {
                        Ok(id) => id,
                        Err(e) => {
                            error!(book_title = %book.title, error = %e, "Failed to create book");
                            continue;
                        }
                    };

                    if let Err(e) = authors::link_book(&self.pool, book_id, author_id).await {
                        error!(book_id = %book_id, author_id = %author_id, error = %e, "Failed to link book to author");
                    }

                    // Add ALL series memberships via junction table
                    add_all_series_memberships(&self.pool, pid, book_id, book).await;

                    new_books += 1;

                    if want_status == "wanted" {
                        wanted_created += 1;
                        if let Err(e) = self
                            .nats_publisher
                            .enqueue_job("kobbler", "book_want", json!({ "book_id": book_id }))
                            .await
                        {
                            warn!(book_id = %book_id, error = %e, "Failed to enqueue book_want job");
                        }
                    }
                }
            }
        }

        // Try to match non-library books to library books
        if let Err(e) = books::match_discovered_to_library(&self.pool, author_id).await {
            warn!(error = %e, "Failed to match books to library");
        }

        // Clean up stale discovered books no longer in provider results
        let current_ids: Vec<String> = author_data.books.iter().map(|b| b.id.clone()).collect();
        match books::delete_stale_discovered(&self.pool, author_id, &current_ids).await {
            Ok(deleted) if deleted > 0 => {
                info!(deleted = deleted, author_id = %author_id, "Cleaned up stale discovered books");
            }
            Err(e) => {
                warn!(error = %e, "Failed to clean up stale discovered books");
            }
            _ => {}
        }

        Ok((new_books, wanted_created))
    }
}

#[async_trait]
impl JobHandler for AuthorEnrichHandler {
    fn job_type(&self) -> &'static str {
        "author_enrich"
    }

    fn max_concurrent(&self) -> usize {
        2
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        nats_jobs::check_cancelled(&cancel)?;
        let author_id = payload["author_id"]
            .as_str()
            .ok_or_else(|| JobError::ExecutionFailed("Missing author_id in payload".to_string()))?;

        let author_id = Uuid::parse_str(author_id)
            .map_err(|_| JobError::ExecutionFailed("Invalid author_id".to_string()))?;

        let _ = authors::bump_next_refresh(&self.pool, author_id).await;

        let author_info = authors::get_for_enrich(&self.pool, author_id)
            .await?
            .ok_or_else(|| JobError::ExecutionFailed(format!("Author not found: {}", author_id)))?;

        let external_id = author_info
            .hardcover_id
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                JobError::ExecutionFailed(format!(
                    "Author has no {} ID configured",
                    TRACKING_PROVIDER
                ))
            })?;

        let api_key = get_tracking_api_key(&self.pool).await?;
        let provider = create_provider(api_key, self.pool.clone())?;

        info!(author_id = %author_id, name = %author_info.name, "Enriching author metadata");

        let (new_books, wanted_created) = self
            .sync_author(
                &provider,
                author_id,
                &external_id,
                &author_info.auto_want_mode,
                author_info.monitoring_started_at,
                author_info.is_monitored,
            )
            .await?;

        Ok(json!({
            "author_id": author_id,
            "author_name": author_info.name,
            "new_books": new_books,
            "wanted_created": wanted_created
        }))
    }
}
