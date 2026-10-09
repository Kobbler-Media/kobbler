//! Ingest queue query operations.
//!
//! Now queries books + book_ingest_source instead of the old ingest_queue table.

use super::Result;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

/// Minimal info for extraction jobs.
#[derive(Debug)]
pub struct ExtractionItem {
    pub id: Uuid,
    pub file_path: String,
    pub file_name: String,
}

/// SELECT columns for ingest queue queries - joins books with book_ingest_source.
/// Field aliases match IngestQueueItem struct (Book-shaped names).
const SELECT_COLS: &str = r#"
    b.id, bis.file_path, bis.file_name, bis.file_size,
    bis.file_hash, bis.source_mtime, bis.source_format,
    b.title,
    COALESCE(
        (SELECT jsonb_agg(DISTINCT jsonb_build_object('id', a.id, 'name', a.name))
         FROM book_authors ba
         JOIN authors a ON a.id = ba.author_id
         WHERE ba.book_id = b.id),
        '[]'::jsonb
    ) AS linked_authors,
    (SELECT array_agg(DISTINCT author_name) FROM (
        SELECT a.name as author_name FROM book_authors ba JOIN authors a ON a.id = ba.author_id WHERE ba.book_id = b.id
        UNION ALL
        SELECT elem->>'name' as author_name FROM jsonb_array_elements(COALESCE(bis.pending_authors, '[]'::jsonb)) AS elem
    ) combined WHERE author_name IS NOT NULL) AS merged_author_names,
    b.description,
    bs.series_id,
    COALESCE(s.name, bis.pending_series->>'name') AS series_name,
    COALESCE(bs.series_index, bis.pending_series_index) AS series_index,
    b.language,
    b.publisher,
    b.pubdate,
    COALESCE(b.identifiers, '{}'::jsonb) AS identifiers,
    b.rating,
    b.page_count,
    COALESCE(
        (SELECT array_agg(g.name ORDER BY g.name) FROM book_genres bg
         JOIN genres g ON g.id = bg.genre_id WHERE bg.book_id = b.id),
        ARRAY[]::text[]
    ) AS genres,
    (bis.extracted_cover IS NOT NULL) AS has_cover,
    bis.pending_authors,
    bis.pending_series,
    bis.pending_series_index,
    bis.source_metadata,
    b.ingest_status AS status,
    bis.conversion_error,
    bis.update_target_book_id,
    b.created_at,
    b.updated_at
"#;

/// Status counts for ingest queue.
#[derive(Debug, Default, serde::Serialize)]
pub struct StatusCounts {
    pub extracting: i64,
    pub pending: i64,
    pub converting: i64,
    pub approved: i64,
    pub rejected: i64,
    pub error: i64,
}

/// Get status counts for all items in the queue.
pub async fn get_status_counts(pool: &sqlx::PgPool) -> Result<StatusCounts> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        r#"
        SELECT ingest_status AS status, COUNT(*) AS count
        FROM books
        WHERE ingest_status IS NOT NULL
        GROUP BY ingest_status
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut counts = StatusCounts::default();
    for (status, count) in rows {
        match status.as_str() {
            "extracting" => counts.extracting = count,
            "pending" => counts.pending = count,
            "converting" => counts.converting = count,
            "approved" => counts.approved = count,
            "rejected" => counts.rejected = count,
            "error" => counts.error = count,
            _ => {}
        }
    }

    // Count approved as books that came from ingest but are now in library
    // (have book_ingest_source record but no ingest_status)
    let approved_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM books b
        JOIN book_ingest_source bis ON bis.book_id = b.id
        WHERE b.ingest_status IS NULL AND (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
        "#,
    )
    .fetch_one(pool)
    .await?;
    counts.approved = approved_count;

    Ok(counts)
}

/// Filters for listing ingest queue items.
#[derive(Debug, Default)]
pub struct ListFilters {
    pub status: Option<String>,
    pub search: Option<String>,
}

/// List ingest queue items with optional filters.
pub async fn list_with_filters<'e, E>(
    exec: E,
    filters: ListFilters,
) -> Result<Vec<crate::watcher::IngestQueueItem>>
where
    E: Executor<'e, Database = Postgres>,
{
    let has_status = filters.status.is_some();
    let has_search = filters.search.is_some();

    match (has_status, has_search) {
        (true, true) => {
            let status = filters.status.unwrap();
            let search_pattern = format!("%{}%", filters.search.unwrap());
            let query = format!(
                r#"SELECT {SELECT_COLS}
                   FROM books b
                   JOIN book_ingest_source bis ON bis.book_id = b.id
                   LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
                   LEFT JOIN series s ON s.id = bs.series_id
                   WHERE b.ingest_status = $1
                   AND (bis.file_name ILIKE $2 OR bis.file_path ILIKE $2 OR b.title ILIKE $2)
                   ORDER BY b.created_at DESC"#
            );
            Ok(
                sqlx::query_as::<_, crate::watcher::IngestQueueItem>(sqlx::AssertSqlSafe(
                    query.as_str(),
                ))
                .bind(&status)
                .bind(&search_pattern)
                .fetch_all(exec)
                .await?,
            )
        }
        (true, false) => {
            let status = filters.status.unwrap();
            let query = format!(
                r#"SELECT {SELECT_COLS}
                   FROM books b
                   JOIN book_ingest_source bis ON bis.book_id = b.id
                   LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
                   LEFT JOIN series s ON s.id = bs.series_id
                   WHERE b.ingest_status = $1
                   ORDER BY b.created_at DESC"#
            );
            Ok(
                sqlx::query_as::<_, crate::watcher::IngestQueueItem>(sqlx::AssertSqlSafe(
                    query.as_str(),
                ))
                .bind(&status)
                .fetch_all(exec)
                .await?,
            )
        }
        (false, true) => {
            let search_pattern = format!("%{}%", filters.search.unwrap());
            let query = format!(
                r#"SELECT {SELECT_COLS}
                   FROM books b
                   JOIN book_ingest_source bis ON bis.book_id = b.id
                   LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
                   LEFT JOIN series s ON s.id = bs.series_id
                   WHERE b.ingest_status IS NOT NULL
                   AND (bis.file_name ILIKE $1 OR bis.file_path ILIKE $1 OR b.title ILIKE $1)
                   ORDER BY b.created_at DESC"#
            );
            Ok(
                sqlx::query_as::<_, crate::watcher::IngestQueueItem>(sqlx::AssertSqlSafe(
                    query.as_str(),
                ))
                .bind(&search_pattern)
                .fetch_all(exec)
                .await?,
            )
        }
        (false, false) => {
            let query = format!(
                r#"SELECT {SELECT_COLS}
                   FROM books b
                   JOIN book_ingest_source bis ON bis.book_id = b.id
                   LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
                   LEFT JOIN series s ON s.id = bs.series_id
                   WHERE b.ingest_status IS NOT NULL
                   ORDER BY b.created_at DESC"#
            );
            Ok(
                sqlx::query_as::<_, crate::watcher::IngestQueueItem>(sqlx::AssertSqlSafe(
                    query.as_str(),
                ))
                .fetch_all(exec)
                .await?,
            )
        }
    }
}

/// Get a single ingest queue item with has_cover computed.
/// Only returns items with ingest_status IS NOT NULL (not approved items).
pub async fn get_by_id_full<'e, E>(
    exec: E,
    id: Uuid,
) -> Result<Option<crate::watcher::IngestQueueItem>>
where
    E: Executor<'e, Database = Postgres>,
{
    let query = format!(
        r#"SELECT {SELECT_COLS}
           FROM books b
           JOIN book_ingest_source bis ON bis.book_id = b.id
           LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
           LEFT JOIN series s ON s.id = bs.series_id
           WHERE b.id = $1 AND b.ingest_status IS NOT NULL"#
    );
    Ok(
        sqlx::query_as::<_, crate::watcher::IngestQueueItem>(sqlx::AssertSqlSafe(query.as_str()))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

/// List items with status 'extracting'.
pub async fn list_extracting<'e, E>(exec: E) -> Result<Vec<ExtractionItem>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
        r#"
        SELECT b.id, bis.file_path, bis.file_name
        FROM books b
        JOIN book_ingest_source bis ON bis.book_id = b.id
        WHERE b.ingest_status = 'extracting'
        ORDER BY b.created_at
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, file_path, file_name)| ExtractionItem {
            id,
            file_path,
            file_name,
        })
        .collect())
}
