//! Series query operations for sync and enrichment.

use super::Result;
use crate::images::ImageMap;
use serde_json::Value;
use sqlx::{Executor, Postgres, Row};
use uuid::Uuid;

/// Series info for enrichment jobs.
#[derive(Debug)]
pub struct SeriesEnrichmentInfo {
    pub id: Uuid,
    pub name: String,
    pub identifiers: Option<Value>,
    pub auto_want_mode: String,
}

/// Get series info needed for enrichment.
pub async fn get_for_enrichment<'e, E>(exec: E, id: Uuid) -> Result<Option<SeriesEnrichmentInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"SELECT id, name, identifiers, COALESCE(auto_want_mode, 'inherit') as "auto_want_mode!" FROM series WHERE id = $1"#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| SeriesEnrichmentInfo {
        id: r.id,
        name: r.name,
        identifiers: Some(r.identifiers),
        auto_want_mode: r.auto_want_mode,
    }))
}

/// Series info for sync check job.
#[derive(Debug)]
pub struct SeriesSyncInfo {
    pub id: Uuid,
    pub name: String,
    pub hardcover_id: Option<String>,
}

/// List series due for metadata refresh (have hardcover ID and next_refresh is due).
pub async fn list_due_for_refresh<'e, E>(exec: E, limit: i32) -> Result<Vec<SeriesSyncInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, name, identifiers->>'hardcover' as hardcover_id
        FROM series
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
        .map(|r| SeriesSyncInfo {
            id: r.id,
            name: r.name,
            hardcover_id: r.hardcover_id,
        })
        .collect())
}

/// Get auto_want_mode for a series.
pub async fn get_auto_want_mode<'e, E>(exec: E, id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let mode: Option<String> =
        sqlx::query_scalar!("SELECT auto_want_mode FROM series WHERE id = $1", id)
            .fetch_optional(exec)
            .await?;
    Ok(mode)
}

/// Find a series by a specific identifier key/value.
pub async fn find_by_identifier<'e, E>(exec: E, key: &str, value: &str) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    // Dynamic query - key is interpolated into SQL
    let query = format!(
        "SELECT id FROM series WHERE identifiers->>'{}' = $1",
        key.replace('\'', "''")
    );
    let id: Option<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(query.as_str()))
        .bind(value)
        .fetch_optional(exec)
        .await?;
    Ok(id)
}

/// Get the name of a series by ID.
pub async fn get_name<'e, E>(exec: E, id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let name: Option<String> = sqlx::query_scalar!("SELECT name FROM series WHERE id = $1", id)
        .fetch_optional(exec)
        .await?;
    Ok(name)
}

/// Get series cover image source URL by ID.
pub async fn get_image_url<'e, E>(exec: E, id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let url: Option<String> = sqlx::query_scalar!(
        "SELECT images->'cover'->>'source_url' FROM series WHERE id = $1",
        id
    )
    .fetch_optional(exec)
    .await?
    .flatten();
    Ok(url)
}

/// Check if a series exists.
pub async fn exists<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let exists: bool = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM series WHERE id = $1) as "exists!""#,
        id,
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

/// Get a single series by ID with full summary data.
pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<SeriesSummaryRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT s.id, s.name, s.updated_at, s.identifiers, s.images, s.description,
               s.auto_want_mode, s.total_book_count, s.is_monitored, s.monitoring_started_at, s.metadata,
               COUNT(DISTINCT CASE WHEN (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)) THEN b.id END) AS "book_count!",
               COUNT(DISTINCT CASE WHEN b.want_status = 'wanted' THEN b.id END) AS "wanted_count!",
               (
                 SELECT COALESCE(array_agg(id), ARRAY[]::uuid[])
                 FROM (
                   SELECT b2.id FROM books b2
                   JOIN book_series bs2 ON bs2.book_id = b2.id AND bs2.series_id = s.id
                   WHERE (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b2.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b2.id))
                   ORDER BY
                     COALESCE(bs2.series_index, 999999) ASC,
                     b2.created_at ASC
                   LIMIT 5
                 ) sub
               ) AS "sample_book_ids!: Vec<Uuid>",
               COALESCE(
                 (SELECT jsonb_agg(DISTINCT jsonb_build_object('id', a2.id, 'name', a2.name))
                  FROM book_authors ba2
                  JOIN authors a2 ON a2.id = ba2.author_id
                  JOIN books b2 ON b2.id = ba2.book_id
                  JOIN book_series bs2 ON bs2.book_id = b2.id AND bs2.series_id = s.id
                  WHERE (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b2.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b2.id))),
                 '[]'::jsonb
               ) AS "authors!"
        FROM series s
        LEFT JOIN book_series bks ON bks.series_id = s.id
        LEFT JOIN books b ON b.id = bks.book_id
        WHERE s.id = $1
        GROUP BY s.id
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|row| {
        let authors: Vec<AuthorRef> =
            serde_json::from_value(row.authors.clone()).unwrap_or_default();
        let images: ImageMap = serde_json::from_value(row.images.clone()).unwrap_or_default();
        SeriesSummaryRow {
            id: row.id,
            name: row.name,
            updated_at: row.updated_at,
            identifiers: Some(row.identifiers),
            images,
            description: row.description,
            auto_want_mode: Some(row.auto_want_mode),
            total_book_count: row.total_book_count,
            is_monitored: row.is_monitored,
            monitoring_started_at: row.monitoring_started_at,
            book_count: row.book_count,
            wanted_count: row.wanted_count,
            sample_book_ids: row.sample_book_ids,
            authors,
            metadata: row.metadata,
        }
    }))
}

// =============================================================================
// Series Listing with Filters
// =============================================================================

use super::super::PaginationDirection;

/// Filter parameters for series listing.
#[derive(Debug, Default)]
pub struct SeriesListFilter {
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
    /// Include series with no library books (default false).
    pub include_no_books: bool,
    /// Filter by media format: "ebook" or "audiobook" (series with at least one matching book).
    pub format: Option<String>,
    /// Limit results (fetches limit+1 for has_more).
    pub limit: i64,
}

use crate::routes::library::types::AuthorRef;

/// Series summary row for listing.
#[derive(Debug)]
pub struct SeriesSummaryRow {
    pub id: Uuid,
    pub name: String,
    pub updated_at: time::OffsetDateTime,
    pub identifiers: Option<Value>,
    pub images: ImageMap,
    pub description: Option<String>,
    pub auto_want_mode: Option<String>,
    pub total_book_count: Option<i32>,
    pub is_monitored: bool,
    pub monitoring_started_at: Option<time::OffsetDateTime>,
    pub book_count: i64,
    pub wanted_count: i64,
    pub sample_book_ids: Vec<Uuid>,
    pub authors: Vec<AuthorRef>,
    pub metadata: serde_json::Value,
}

/// Count series matching filters (for pagination total).
pub async fn count_with_filters<'e, E>(exec: E, filter: &SeriesListFilter) -> Result<i64>
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
        conditions.push(format!("s.id = ANY(${}::uuid[])", param_idx));
        param_idx += 1;
    }

    // ILIKE search filter (fallback when search_ids not provided)
    let search_pattern = if !use_search_ids {
        filter.search.as_ref().map(|s| format!("%{}%", s))
    } else {
        None
    };
    if search_pattern.is_some() {
        conditions.push(format!("s.name ILIKE ${}", param_idx));
        param_idx += 1;
    }

    // include_no_books filter (default: only show series with library books)
    if !filter.include_no_books {
        conditions.push(
            "EXISTS (SELECT 1 FROM book_series bs2 JOIN books b2 ON b2.id = bs2.book_id WHERE bs2.series_id = s.id AND (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b2.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b2.id)))"
                .to_string(),
        );
    }

    // is_monitored filter
    if filter.is_monitored.is_some() {
        conditions.push(format!("s.is_monitored = ${}", param_idx));
    }

    // format filter — series with at least one book matching the format
    if let Some(ref fmt) = filter.format {
        match fmt.as_str() {
            "audiobook" => {
                conditions.push(
                    "EXISTS (SELECT 1 FROM book_series bs3 JOIN books b3 ON b3.id = bs3.book_id WHERE bs3.series_id = s.id AND \
                     (((EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b3.id) OR EXISTS (SELECT 1 FROM audiobook_files af2 WHERE af2.book_id = b3.id)) AND EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b3.id)) \
                      OR (NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b3.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b3.id)) AND b3.want_format IN ('audiobook', 'both'))))"
                        .to_string(),
                );
            }
            "ebook" => {
                conditions.push(
                    "EXISTS (SELECT 1 FROM book_series bs3 JOIN books b3 ON b3.id = bs3.book_id WHERE bs3.series_id = s.id AND \
                     (((EXISTS (SELECT 1 FROM book_files bf2 WHERE bf2.book_id = b3.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b3.id)) AND EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b3.id)) \
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

    let query_str = format!("SELECT COUNT(*) FROM series s {}", where_clause);

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

/// List series with filters (for admin listing).
/// Supports both forward and backward pagination via the direction field.
pub async fn list_with_filters<'e, E>(
    exec: E,
    filter: &SeriesListFilter,
) -> Result<Vec<SeriesSummaryRow>>
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
            "(s.name, s.id) {} (${}, ${})",
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
        conditions.push(format!("s.id = ANY(${}::uuid[])", param_idx));
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
        conditions.push(format!("s.name ILIKE ${}", param_idx));
        param_idx += 1;
    }

    // include_no_books filter (default: only show series with library books)
    if !filter.include_no_books {
        conditions.push(
            "EXISTS (SELECT 1 FROM book_series bs2 JOIN books b2 ON b2.id = bs2.book_id WHERE bs2.series_id = s.id AND (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b2.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b2.id)))"
                .to_string(),
        );
    }

    // is_monitored filter
    if filter.is_monitored.is_some() {
        conditions.push(format!("s.is_monitored = ${}", param_idx));
        param_idx += 1;
    }

    // format filter — series with at least one book matching the format
    if let Some(ref fmt) = filter.format {
        match fmt.as_str() {
            "audiobook" => {
                conditions.push(
                    "EXISTS (SELECT 1 FROM book_series bs3 JOIN books b3 ON b3.id = bs3.book_id WHERE bs3.series_id = s.id AND \
                     (((EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b3.id) OR EXISTS (SELECT 1 FROM audiobook_files af2 WHERE af2.book_id = b3.id)) AND EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b3.id)) \
                      OR (NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b3.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b3.id)) AND b3.want_format IN ('audiobook', 'both'))))"
                        .to_string(),
                );
            }
            "ebook" => {
                conditions.push(
                    "EXISTS (SELECT 1 FROM book_series bs3 JOIN books b3 ON b3.id = bs3.book_id WHERE bs3.series_id = s.id AND \
                     (((EXISTS (SELECT 1 FROM book_files bf2 WHERE bf2.book_id = b3.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b3.id)) AND EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b3.id)) \
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
        format!("array_position(${}::uuid[], s.id), s.id", idx)
    } else {
        format!("s.name {}, s.id {}", order_dir, order_dir)
    };

    let query_str = format!(
        r#"
        SELECT s.id, s.name, s.updated_at, s.identifiers, s.images, s.description,
               s.auto_want_mode, s.total_book_count, s.is_monitored, s.monitoring_started_at, s.metadata,
               COUNT(DISTINCT CASE WHEN (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)) THEN b.id END) AS book_count,
               COUNT(DISTINCT CASE WHEN b.want_status = 'wanted' THEN b.id END) AS wanted_count,
               (
                 SELECT COALESCE(array_agg(id), ARRAY[]::uuid[])
                 FROM (
                   SELECT b2.id FROM books b2
                   JOIN book_series bs2 ON bs2.book_id = b2.id AND bs2.series_id = s.id
                   WHERE (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b2.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b2.id))
                   ORDER BY
                     COALESCE(bs2.series_index, 999999) ASC,
                     b2.created_at ASC
                   LIMIT 5
                 ) sub
               ) AS sample_book_ids,
               COALESCE(
                 (SELECT jsonb_agg(DISTINCT jsonb_build_object('id', a2.id, 'name', a2.name))
                  FROM book_authors ba2
                  JOIN authors a2 ON a2.id = ba2.author_id
                  JOIN books b2 ON b2.id = ba2.book_id
                  JOIN book_series bs2 ON bs2.book_id = b2.id AND bs2.series_id = s.id
                  WHERE (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b2.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b2.id))),
                 '[]'::jsonb
               ) AS authors
        FROM series s
        LEFT JOIN book_series bks ON bks.series_id = s.id
        LEFT JOIN books b ON b.id = bks.book_id
        {}
        GROUP BY s.id
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
            let authors_json: Value = row.get("authors");
            let authors: Vec<AuthorRef> = serde_json::from_value(authors_json).unwrap_or_default();
            let images_json: serde_json::Value = row.get("images");
            let images: ImageMap = serde_json::from_value(images_json).unwrap_or_default();
            SeriesSummaryRow {
                id: row.get("id"),
                name: row.get("name"),
                updated_at: row.get("updated_at"),
                identifiers: row.get("identifiers"),
                images,
                description: row.get("description"),
                auto_want_mode: row.get("auto_want_mode"),
                total_book_count: row.get("total_book_count"),
                is_monitored: row.get("is_monitored"),
                monitoring_started_at: row.get("monitoring_started_at"),
                book_count: row.get("book_count"),
                wanted_count: row.get("wanted_count"),
                sample_book_ids: row.get("sample_book_ids"),
                authors,
                metadata: row.get("metadata"),
            }
        })
        .collect())
}

// =============================================================================
// Series Books (for Kobo)
// =============================================================================

/// Book in a series with metadata for Kobo display.
#[derive(Debug)]
pub struct SeriesBookRow {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<time::OffsetDateTime>,
    pub series_id: Option<Uuid>,
    pub series_name: Option<String>,
    pub series_index: Option<f64>,
    pub identifiers: Option<Value>,
}

/// List books in a series ordered by index.
pub async fn list_books<'e, E>(exec: E, series_id: Uuid) -> Result<Vec<SeriesBookRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT b.id, b.title, b.description, b.language, b.publisher, b.pubdate,
               bks.series_id, s.name AS series_name, bks.series_index,
               b.identifiers
        FROM books b
        JOIN book_series bks ON bks.book_id = b.id AND bks.series_id = $1
        JOIN series s ON s.id = bks.series_id
        WHERE (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
        ORDER BY bks.series_index NULLS LAST, b.title
        "#,
        series_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| SeriesBookRow {
            id: r.id,
            title: r.title,
            description: r.description,
            language: r.language,
            publisher: r.publisher,
            pubdate: r.pubdate,
            series_id: Some(r.series_id),
            series_name: Some(r.series_name),
            series_index: r.series_index,
            identifiers: Some(r.identifiers),
        })
        .collect())
}
