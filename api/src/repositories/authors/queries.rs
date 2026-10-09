//! Author query operations for sync and enrichment.

use super::Result;
use crate::images::ImageMap;
use sqlx::{Executor, Postgres, Row};
use uuid::Uuid;

/// Get the author IDs for a book.
pub async fn get_book_author_ids<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids: Vec<Uuid> = sqlx::query_scalar!(
        "SELECT author_id FROM book_authors WHERE book_id = $1",
        book_id
    )
    .fetch_all(exec)
    .await?;
    Ok(ids)
}

/// Get all book IDs for an author.
pub async fn get_book_ids<'e, E>(exec: E, author_id: Uuid) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids: Vec<Uuid> = sqlx::query_scalar!(
        "SELECT book_id FROM book_authors WHERE author_id = $1",
        author_id
    )
    .fetch_all(exec)
    .await?;
    Ok(ids)
}

/// Get author names for a book (in link order).
pub async fn get_book_author_names<'e, E>(exec: E, book_id: Uuid) -> Result<Vec<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT a.name
        FROM authors a
        JOIN book_authors ba ON ba.author_id = a.id
        WHERE ba.book_id = $1
        ORDER BY a.name
        "#,
        book_id,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| r.name).collect())
}

/// Get the first/primary author name for a book.
pub async fn get_first_for_book<'e, E>(exec: E, book_id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let name: Option<String> = sqlx::query_scalar!(
        r#"
        SELECT a.name
        FROM authors a
        JOIN book_authors ba ON ba.author_id = a.id
        WHERE ba.book_id = $1
        ORDER BY a.name
        LIMIT 1
        "#,
        book_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(name)
}

/// The monitoring settings a series inherits from its authors.
#[derive(Debug)]
pub struct SeriesMonitoring {
    pub auto_want_mode: String,
    pub monitoring_started_at: Option<time::OffsetDateTime>,
}

/// Resolve the monitoring settings a series inherits from its monitored authors.
///
/// A series has no primary author, so a co-authored series can offer several
/// conflicting settings. The most permissive one wins: `none` is the column
/// default, so an author sitting at it has usually never been configured rather
/// than opted out, and an unwanted acquisition is reversible where a silently
/// skipped book is invisible. Both values come from the same author, so a
/// `new_only` mode can never be paired with a different author's start date.
pub async fn get_series_monitoring<'e, E>(
    exec: E,
    series_id: Uuid,
) -> Result<Option<SeriesMonitoring>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT a.auto_want_mode, a.monitoring_started_at
        FROM authors a
        JOIN book_authors ba ON ba.author_id = a.id
        JOIN book_series bks ON bks.book_id = ba.book_id AND bks.series_id = $1
        WHERE a.is_monitored = TRUE
        -- An earlier start date wants more books, so it is the permissive
        -- tie-break; NULL is the most permissive of all ('new_only' falls
        -- through to wanting everything). Trailing id makes the order total.
        ORDER BY CASE a.auto_want_mode
                     WHEN 'all' THEN 0
                     WHEN 'fill_gaps' THEN 1
                     WHEN 'new_only' THEN 2
                     ELSE 3
                 END,
                 a.monitoring_started_at ASC NULLS FIRST,
                 a.id ASC
        LIMIT 1
        "#,
        series_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| SeriesMonitoring {
        auto_want_mode: r.auto_want_mode,
        monitoring_started_at: r.monitoring_started_at,
    }))
}

/// Author data for sync jobs.
#[derive(Debug)]
pub struct AuthorSyncInfo {
    pub id: Uuid,
    pub name: String,
    pub hardcover_id: String,
    pub auto_want_mode: String,
    pub monitoring_started_at: Option<time::OffsetDateTime>,
    pub is_monitored: bool,
}

/// List authors due for sync (have hardcover ID and next_refresh is due).
pub async fn list_due_for_sync<'e, E>(exec: E, limit: i32) -> Result<Vec<AuthorSyncInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, identifiers->>'hardcover' as hardcover_id, name, auto_want_mode, monitoring_started_at, is_monitored
        FROM authors
        WHERE identifiers->>'hardcover' IS NOT NULL
          AND (next_metadata_refresh_at IS NULL OR next_metadata_refresh_at <= NOW())
        ORDER BY last_metadata_refresh_at ASC NULLS FIRST
        LIMIT $1
        "#,
        limit as i64,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|r| {
            r.hardcover_id
                .filter(|s| !s.is_empty())
                .map(|hc_id| AuthorSyncInfo {
                    id: r.id,
                    name: r.name,
                    hardcover_id: hc_id,
                    auto_want_mode: r.auto_want_mode,
                    monitoring_started_at: r.monitoring_started_at,
                    is_monitored: r.is_monitored,
                })
        })
        .collect())
}

/// Author data for enrichment.
#[derive(Debug)]
pub struct AuthorEnrichInfo {
    pub id: Uuid,
    pub name: String,
    pub hardcover_id: Option<String>,
    pub auto_want_mode: String,
    pub monitoring_started_at: Option<time::OffsetDateTime>,
    pub is_monitored: bool,
}

/// Get author info needed for enrichment.
pub async fn get_for_enrich<'e, E>(exec: E, id: Uuid) -> Result<Option<AuthorEnrichInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, identifiers->>'hardcover' as hardcover_id, name, auto_want_mode, monitoring_started_at, is_monitored
        FROM authors
        WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| AuthorEnrichInfo {
        id: r.id,
        name: r.name,
        hardcover_id: r.hardcover_id,
        auto_want_mode: r.auto_want_mode,
        monitoring_started_at: r.monitoring_started_at,
        is_monitored: r.is_monitored,
    }))
}

/// Find an author by name (case-insensitive).
/// Returns (id, identifiers) if found.
///
/// Migration `0031` made author names non-unique on purpose, so this resolves
/// to the oldest match — the same rule `get_or_create` applies when its
/// normalized-name lookup returns several rows.
pub async fn find_by_name<'e, E>(exec: E, name: &str) -> Result<Option<(Uuid, serde_json::Value)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        // Trailing id makes the order total; created_at is not unique.
        "SELECT id, identifiers FROM authors WHERE LOWER(name) = LOWER($1)
         ORDER BY created_at ASC, id ASC LIMIT 1",
        name,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| (r.id, r.identifiers)))
}

/// Author data for tracked author sync.
#[derive(Debug)]
pub struct TrackedAuthorSyncInfo {
    pub id: Uuid,
    pub name: String,
    pub identifiers: serde_json::Value,
    pub auto_want_mode: String,
    pub monitoring_started_at: Option<time::OffsetDateTime>,
}

/// List monitored authors due for refresh (have hardcover ID and are monitored).
pub async fn list_monitored_due_for_refresh<'e, E>(
    exec: E,
    limit: i32,
) -> Result<Vec<TrackedAuthorSyncInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, name, identifiers, auto_want_mode, monitoring_started_at
        FROM authors
        WHERE is_monitored = TRUE
          AND identifiers->>'hardcover' IS NOT NULL
          AND (next_metadata_refresh_at IS NULL OR next_metadata_refresh_at <= NOW())
        ORDER BY last_metadata_refresh_at ASC NULLS FIRST
        LIMIT $1
        "#,
        limit as i64,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| TrackedAuthorSyncInfo {
            id: r.id,
            name: r.name,
            identifiers: r.identifiers,
            auto_want_mode: r.auto_want_mode,
            monitoring_started_at: r.monitoring_started_at,
        })
        .collect())
}

/// Get author name by ID.
pub async fn get_name<'e, E>(exec: E, id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let name: Option<String> = sqlx::query_scalar!("SELECT name FROM authors WHERE id = $1", id)
        .fetch_optional(exec)
        .await?;
    Ok(name)
}

/// Get a single author by ID with full summary data.
pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<AuthorSummaryRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT a.id, a.name, a.identifiers, a.images, a.bio,
               a.is_monitored, a.auto_want_mode, a.known_book_count, a.metadata,
               COUNT(DISTINCT CASE WHEN (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)) THEN ba.book_id END) AS "book_count!",
               COUNT(DISTINCT CASE WHEN (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)) THEN bks.series_id END) AS "series_count!",
               COUNT(DISTINCT ba.book_id) AS "total_book_count!",
               COUNT(DISTINCT bks.series_id) AS "total_series_count!",
               COUNT(DISTINCT CASE WHEN b.want_status = 'wanted' THEN ba.book_id END) AS "wanted_count!",
               (
                 SELECT COALESCE(array_agg(id), ARRAY[]::uuid[])
                 FROM (
                   SELECT b2.id FROM books b2
                   JOIN book_authors ba3 ON ba3.book_id = b2.id
                   WHERE ba3.author_id = a.id AND (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b2.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b2.id))
                   ORDER BY b2.created_at DESC
                   LIMIT 3
                 ) sub
               ) AS "sample_book_ids!: Vec<Uuid>",
               (
                 SELECT string_agg(s_name, ', ')
                 FROM (
                   SELECT DISTINCT s.name AS s_name FROM series s
                   JOIN book_series bs2 ON bs2.series_id = s.id
                   JOIN book_authors ba2 ON ba2.book_id = bs2.book_id
                   WHERE ba2.author_id = a.id
                   ORDER BY s_name
                   LIMIT 3
                 ) sub2
               ) AS top_series_names
        FROM authors a
        LEFT JOIN book_authors ba ON ba.author_id = a.id
        LEFT JOIN books b ON b.id = ba.book_id
        LEFT JOIN book_series bks ON bks.book_id = b.id AND bks.is_primary = TRUE
        WHERE a.id = $1
        GROUP BY a.id
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|row| {
        let images: ImageMap = serde_json::from_value(row.images.clone()).unwrap_or_default();
        AuthorSummaryRow {
            id: row.id,
            name: row.name,
            identifiers: row.identifiers,
            images,
            bio: row.bio,
            is_monitored: row.is_monitored,
            auto_want_mode: Some(row.auto_want_mode),
            known_book_count: row.known_book_count,
            book_count: row.book_count,
            series_count: row.series_count,
            total_book_count: row.total_book_count,
            total_series_count: row.total_series_count,
            wanted_count: row.wanted_count,
            sample_book_ids: row.sample_book_ids,
            top_series_names: row.top_series_names,
            metadata: row.metadata,
        }
    }))
}

/// Get author profile image source URL by ID.
pub async fn get_image_url<'e, E>(exec: E, id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let url: Option<String> = sqlx::query_scalar!(
        "SELECT images->'profile'->>'source_url' FROM authors WHERE id = $1",
        id
    )
    .fetch_optional(exec)
    .await?
    .flatten();
    Ok(url)
}

// =============================================================================
// Author Listing with Filters
// =============================================================================

use super::super::PaginationDirection;

/// Filter parameters for author listing.
#[derive(Debug, Default)]
pub struct AuthorListFilter {
    /// Cursor for pagination (name, id).
    pub cursor: Option<(String, Uuid)>,
    /// Pagination direction (forward or backward from cursor).
    pub direction: PaginationDirection,
    /// Search pattern (will be wrapped with %). Used for ILIKE fallback.
    pub search: Option<String>,
    /// Pre-filtered IDs from search backend (takes precedence over search).
    /// When provided, the query filters by these IDs instead of using ILIKE.
    pub search_ids: Option<Vec<Uuid>>,
    /// Filter by is_monitored.
    pub is_monitored: Option<bool>,
    /// Include authors with no library books (default false).
    pub include_no_books: bool,
    /// Filter by media format: "ebook" or "audiobook" (authors with at least one matching book).
    pub format: Option<String>,
    /// Limit results (fetches limit+1 for has_more).
    pub limit: i64,
}

/// Author summary row for listing.
#[derive(Debug)]
pub struct AuthorSummaryRow {
    pub id: Uuid,
    pub name: String,
    pub identifiers: serde_json::Value,
    pub images: ImageMap,
    pub bio: Option<String>,
    pub is_monitored: bool,
    pub auto_want_mode: Option<String>,
    pub known_book_count: Option<i32>,
    pub book_count: i64,
    pub series_count: i64,
    /// Total books in DB for this author (regardless of library status)
    pub total_book_count: i64,
    /// Total series in DB for this author (regardless of library status)
    pub total_series_count: i64,
    pub wanted_count: i64,
    pub sample_book_ids: Vec<Uuid>,
    /// Top series names for disambiguation (e.g., "Stormlight Archive, Mistborn")
    pub top_series_names: Option<String>,
    /// Raw provider metadata (stored in authors.metadata column)
    pub metadata: serde_json::Value,
}

/// Count authors matching filters (for pagination total).
pub async fn count_with_filters<'e, E>(exec: E, filter: &AuthorListFilter) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    // If search_ids is provided but empty, return 0
    if let Some(ref ids) = filter.search_ids
        && ids.is_empty()
    {
        return Ok(0);
    }

    // Build dynamic query with filters (same as list_with_filters but no cursor)
    let mut conditions = Vec::new();
    let mut param_idx = 1;

    // search_ids filter (from search backend, takes precedence over ILIKE)
    let use_search_ids = filter.search_ids.is_some();
    if use_search_ids {
        conditions.push(format!("a.id = ANY(${}::uuid[])", param_idx));
        param_idx += 1;
    }

    // ILIKE search filter (fallback when search_ids not provided)
    let search_pattern = if !use_search_ids {
        filter.search.as_ref().map(|s| format!("%{}%", s))
    } else {
        None
    };
    if search_pattern.is_some() {
        conditions.push(format!("a.name ILIKE ${}", param_idx));
        param_idx += 1;
    }

    // is_monitored filter
    if filter.is_monitored.is_some() {
        conditions.push(format!("a.is_monitored = ${}", param_idx));
    }

    // include_no_books filter: show authors with library books, monitored authors,
    // or authors with no book links at all (manually added, not from ingest)
    if !filter.include_no_books {
        conditions.push("(EXISTS (SELECT 1 FROM book_authors ba2 JOIN books b ON b.id = ba2.book_id WHERE ba2.author_id = a.id AND (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))) OR a.is_monitored = TRUE OR NOT EXISTS (SELECT 1 FROM book_authors ba3 WHERE ba3.author_id = a.id))".to_string());
    }

    // format filter — authors with at least one book matching the format
    if let Some(ref fmt) = filter.format {
        match fmt.as_str() {
            "audiobook" => {
                conditions.push(
                    "EXISTS (SELECT 1 FROM book_authors ba4 JOIN books b3 ON b3.id = ba4.book_id WHERE ba4.author_id = a.id AND \
                     ((EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b3.id)) \
                      OR (NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b3.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b3.id)) AND b3.want_format IN ('audiobook', 'both'))))"
                        .to_string(),
                );
            }
            "ebook" => {
                conditions.push(
                    "EXISTS (SELECT 1 FROM book_authors ba4 JOIN books b3 ON b3.id = ba4.book_id WHERE ba4.author_id = a.id AND \
                     ((EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b3.id)) \
                      OR (NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b3.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b3.id)) AND b3.want_format IN ('ebook', 'both'))))"
                        .to_string(),
                );
            }
            _ => {}
        }
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let query_str = format!("SELECT COUNT(*) FROM authors a {}", where_clause);

    let mut query = sqlx::query_scalar(sqlx::AssertSqlSafe(query_str.as_str()));

    if let Some(ref ids) = filter.search_ids {
        query = query.bind(ids);
    }
    if let Some(ref pattern) = search_pattern {
        query = query.bind(pattern);
    }
    if let Some(monitored) = filter.is_monitored {
        query = query.bind(monitored);
    }

    let count: i64 = query.fetch_one(exec).await?;
    Ok(count)
}

/// List authors with filters (for admin listing).
/// Supports both forward and backward pagination via the direction field.
pub async fn list_with_filters<'e, E>(
    exec: E,
    filter: &AuthorListFilter,
) -> Result<Vec<AuthorSummaryRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    // If search_ids is provided but empty, return early - no matches
    if let Some(ref ids) = filter.search_ids
        && ids.is_empty()
    {
        return Ok(vec![]);
    }

    // Determine cursor comparison and order based on direction
    let (cursor_op, order_dir) = match filter.direction {
        PaginationDirection::Forward => (">", "ASC"),
        PaginationDirection::Backward => ("<", "DESC"),
    };

    // Build dynamic query with filters
    let mut conditions = Vec::new();
    let mut param_idx = 1;

    // Cursor condition (direction-aware)
    let cursor_params = if let Some((ref name, id)) = filter.cursor {
        conditions.push(format!(
            "(a.name, a.id) {} (${}, ${})",
            cursor_op,
            param_idx,
            param_idx + 1
        ));
        param_idx += 2;
        Some((name.clone(), id))
    } else {
        None
    };

    // search_ids filter (from search backend, takes precedence over ILIKE)
    let use_search_ids = filter.search_ids.is_some();
    let search_ids_param_idx = if use_search_ids {
        let idx = param_idx;
        conditions.push(format!("a.id = ANY(${}::uuid[])", param_idx));
        param_idx += 1;
        Some(idx)
    } else {
        None
    };

    // ILIKE search filter (fallback when search_ids not provided)
    let search_pattern = if !use_search_ids {
        filter.search.as_ref().map(|s| format!("%{}%", s))
    } else {
        None
    };
    if search_pattern.is_some() {
        conditions.push(format!("a.name ILIKE ${}", param_idx));
        param_idx += 1;
    }

    // is_monitored filter
    if filter.is_monitored.is_some() {
        conditions.push(format!("a.is_monitored = ${}", param_idx));
        param_idx += 1;
    }

    // include_no_books filter: show authors with library books, monitored authors,
    // or authors with no book links at all (manually added, not from ingest)
    if !filter.include_no_books {
        conditions.push("(EXISTS (SELECT 1 FROM book_authors ba2 JOIN books b ON b.id = ba2.book_id WHERE ba2.author_id = a.id AND (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))) OR a.is_monitored = TRUE OR NOT EXISTS (SELECT 1 FROM book_authors ba3 WHERE ba3.author_id = a.id))".to_string());
    }

    // format filter — authors with at least one book matching the format
    if let Some(ref fmt) = filter.format {
        match fmt.as_str() {
            "audiobook" => {
                conditions.push(
                    "EXISTS (SELECT 1 FROM book_authors ba4 JOIN books b3 ON b3.id = ba4.book_id WHERE ba4.author_id = a.id AND \
                     ((EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b3.id)) \
                      OR (NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b3.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b3.id)) AND b3.want_format IN ('audiobook', 'both'))))"
                        .to_string(),
                );
            }
            "ebook" => {
                conditions.push(
                    "EXISTS (SELECT 1 FROM book_authors ba4 JOIN books b3 ON b3.id = ba4.book_id WHERE ba4.author_id = a.id AND \
                     ((EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b3.id)) \
                      OR (NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b3.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b3.id)) AND b3.want_format IN ('ebook', 'both'))))"
                        .to_string(),
                );
            }
            _ => {}
        }
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    // Build ORDER BY clause - use relevance ordering when search_ids provided
    let order_clause = if let Some(idx) = search_ids_param_idx {
        // Use array_position to preserve MeiliSearch's relevance ordering
        format!("array_position(${}::uuid[], a.id), a.id", idx)
    } else {
        format!("a.name {}, a.id {}", order_dir, order_dir)
    };

    let query_str = format!(
        r#"
        SELECT a.id, a.name, a.identifiers, a.images, a.bio,
               a.is_monitored, a.auto_want_mode, a.known_book_count, a.metadata,
               COUNT(DISTINCT CASE WHEN (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)) THEN ba.book_id END) AS book_count,
               COUNT(DISTINCT CASE WHEN (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)) THEN bks.series_id END) AS series_count,
               COUNT(DISTINCT ba.book_id) AS total_book_count,
               COUNT(DISTINCT bks.series_id) AS total_series_count,
               COUNT(DISTINCT CASE WHEN b.want_status = 'wanted' THEN ba.book_id END) AS wanted_count,
               (
                 SELECT COALESCE(array_agg(id), ARRAY[]::uuid[])
                 FROM (
                   SELECT b2.id FROM books b2
                   JOIN book_authors ba3 ON ba3.book_id = b2.id
                   WHERE ba3.author_id = a.id AND (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b2.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b2.id))
                   ORDER BY b2.created_at DESC
                   LIMIT 3
                 ) sub
               ) AS sample_book_ids,
               (
                 SELECT string_agg(s_name, ', ')
                 FROM (
                   SELECT DISTINCT s.name AS s_name FROM series s
                   JOIN book_series bs3 ON bs3.series_id = s.id
                   JOIN book_authors ba4 ON ba4.book_id = bs3.book_id
                   WHERE ba4.author_id = a.id
                   ORDER BY s_name
                   LIMIT 3
                 ) sub2
               ) AS top_series_names
        FROM authors a
        LEFT JOIN book_authors ba ON ba.author_id = a.id
        LEFT JOIN books b ON b.id = ba.book_id
        LEFT JOIN book_series bks ON bks.book_id = b.id AND bks.is_primary = TRUE
        {}
        GROUP BY a.id
        ORDER BY {}
        LIMIT ${}
        "#,
        where_clause, order_clause, param_idx
    );

    // Build and execute query with dynamic bindings
    let mut query = sqlx::query(sqlx::AssertSqlSafe(query_str.as_str()));

    if let Some((name, id)) = cursor_params {
        query = query.bind(name).bind(id);
    }
    if let Some(ref ids) = filter.search_ids {
        query = query.bind(ids);
    }
    if let Some(ref pattern) = search_pattern {
        query = query.bind(pattern);
    }
    if let Some(monitored) = filter.is_monitored {
        query = query.bind(monitored);
    }
    query = query.bind(filter.limit + 1);

    let rows = query.fetch_all(exec).await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let images_json: serde_json::Value = row.get("images");
            let images: ImageMap = serde_json::from_value(images_json).unwrap_or_default();
            AuthorSummaryRow {
                id: row.get("id"),
                name: row.get("name"),
                identifiers: row.get("identifiers"),
                images,
                bio: row.get("bio"),
                is_monitored: row.get("is_monitored"),
                auto_want_mode: row.get("auto_want_mode"),
                known_book_count: row.get("known_book_count"),
                book_count: row.get("book_count"),
                series_count: row.get("series_count"),
                total_book_count: row.get("total_book_count"),
                total_series_count: row.get("total_series_count"),
                wanted_count: row.get("wanted_count"),
                sample_book_ids: row.get("sample_book_ids"),
                top_series_names: row.get("top_series_names"),
                metadata: row.get("metadata"),
            }
        })
        .collect())
}
