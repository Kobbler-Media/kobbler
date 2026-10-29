//! Complex book queries, recommendations, and Kobo sync data.

use super::Result;
use crate::images::ImageMap;
use serde_json::Value;
use sqlx::{Executor, Postgres, Row};
use time::OffsetDateTime;
use uuid::Uuid;

/// Get all book IDs for a series.
pub async fn get_ids_by_series<'e, E>(exec: E, series_id: Uuid) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids: Vec<Uuid> = sqlx::query_scalar!(
        "SELECT book_id FROM book_series WHERE series_id = $1",
        series_id
    )
    .fetch_all(exec)
    .await?;
    Ok(ids)
}

/// Existing book info for series enrichment.
#[derive(Debug)]
pub struct ExistingBookInfo {
    pub id: Uuid,
    pub is_in_library: bool,
    pub user_edited_at: Option<time::OffsetDateTime>,
    pub series_id: Option<Uuid>,
}

/// Find an existing book by hardcover ID or title match in a series.
pub async fn find_by_hardcover_or_series_title<'e, E>(
    exec: E,
    hardcover_id: &str,
    series_id: Uuid,
    title: &str,
) -> Result<Option<ExistingBookInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT b.id, (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)) AS "is_in_library!", b.user_edited_at,
               bs.series_id AS "series_id?"
        FROM books b
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        WHERE (b.identifiers->>'hardcover' = $1)
           OR (EXISTS (SELECT 1 FROM book_series WHERE book_id = b.id AND series_id = $2) AND LOWER(b.title) = LOWER($3))
           OR ((EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)) AND LOWER(b.title) = LOWER($3)
               AND EXISTS (
                   SELECT 1 FROM book_authors ba
                   JOIN book_authors series_ba ON series_ba.author_id = ba.author_id
                   JOIN book_series series_bs ON series_bs.book_id = series_ba.book_id
                                             AND series_bs.series_id = $2
                   WHERE ba.book_id = b.id
               ))
        LIMIT 1
        "#,
        hardcover_id,
        series_id,
        title,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| ExistingBookInfo {
        id: r.id,
        is_in_library: r.is_in_library,
        user_edited_at: r.user_edited_at,
        series_id: r.series_id,
    }))
}

/// Find an existing in-library book by title and a known author ID.
///
/// Used by author_sync where the author UUID is already known. Matches on
/// case-insensitive title and at least one shared author to avoid false merges
/// of different books with identical titles by different authors.
pub async fn find_library_book_by_title_and_author_id<'e, E>(
    exec: E,
    title: &str,
    author_id: Uuid,
) -> Result<Option<ExistingBookInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT b.id, (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)) AS "is_in_library!", b.user_edited_at,
               bs.series_id AS "series_id?"
        FROM books b
        JOIN book_authors ba ON ba.book_id = b.id
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        WHERE (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
          AND LOWER(b.title) = LOWER($1)
          AND ba.author_id = $2
        LIMIT 1
        "#,
        title,
        author_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| ExistingBookInfo {
        id: r.id,
        is_in_library: r.is_in_library,
        user_edited_at: r.user_edited_at,
        series_id: r.series_id,
    }))
}

/// Find an existing in-library book by title and author names.
///
/// Used by series_enrich and media_root_resolve where we have author name
/// strings from the provider rather than author UUIDs. Matches on
/// case-insensitive title and at least one shared author name.
pub async fn find_library_book_by_title_and_author_names<'e, E>(
    exec: E,
    title: &str,
    author_names: &[String],
) -> Result<Option<ExistingBookInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    if author_names.is_empty() {
        return Ok(None);
    }

    let lower_names: Vec<String> = author_names.iter().map(|n| n.to_lowercase()).collect();

    let row = sqlx::query!(
        r#"
        SELECT b.id, (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)) AS "is_in_library!", b.user_edited_at,
               bs.series_id AS "series_id?"
        FROM books b
        JOIN book_authors ba ON ba.book_id = b.id
        JOIN authors a ON a.id = ba.author_id
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        WHERE (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
          AND LOWER(b.title) = LOWER($1)
          AND LOWER(a.name) = ANY($2)
        LIMIT 1
        "#,
        title,
        &lower_names,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| ExistingBookInfo {
        id: r.id,
        is_in_library: r.is_in_library,
        user_edited_at: r.user_edited_at,
        series_id: r.series_id,
    }))
}

/// Find an in-library book at a given media root by exact (case-insensitive)
/// title. Last-resort fallback for media_root_resolve when neither the
/// identifier lookup nor the title+author check finds an existing book —
/// covers cases where the existing book has no authors linked yet, or where
/// two providers tag the same work under unrelated identifier keys.
///
/// Constrained to the same media_root_id so we don't cross-link an audiobook
/// to an ebook of the same name in a different root.
pub async fn find_library_book_by_title_in_media_root<'e, E>(
    exec: E,
    title: &str,
    media_root_id: Uuid,
) -> Result<Option<ExistingBookInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT b.id,
               (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)
                OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)) AS "is_in_library!",
               b.user_edited_at,
               bs.series_id AS "series_id?"
        FROM books b
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        WHERE b.media_root_id = $2
          AND LOWER(b.title) = LOWER($1)
          AND (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)
               OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
        ORDER BY b.created_at ASC
        LIMIT 1
        "#,
        title,
        media_root_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| ExistingBookInfo {
        id: r.id,
        is_in_library: r.is_in_library,
        user_edited_at: r.user_edited_at,
        series_id: r.series_id,
    }))
}

/// Book data needed for metadata writing.
#[derive(Debug)]
pub struct MetadataWriteBookInfo {
    pub title: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<OffsetDateTime>,
    pub series_id: Option<Uuid>,
    pub series_index: Option<f64>,
}

/// Get book data needed for metadata writing.
pub async fn get_for_metadata_write<'e, E>(
    exec: E,
    id: Uuid,
) -> Result<Option<MetadataWriteBookInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT b.title, b.description, b.language, b.publisher, b.pubdate,
               bs.series_id AS "series_id?", bs.series_index AS "series_index?"
        FROM books b
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        WHERE b.id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| MetadataWriteBookInfo {
        title: r.title,
        description: r.description,
        language: r.language,
        publisher: r.publisher,
        pubdate: r.pubdate,
        series_id: r.series_id,
        series_index: r.series_index,
    }))
}

/// Match discovered books to existing library books.
///
/// Finds non-library books discovered from the specified author that already exist
/// in the library (matching by title and author). Copies hardcover identifiers from
/// the discovered book to the library book, then deletes the discovered book to avoid
/// logical duplicates.
///
/// Returns the number of books matched.
pub async fn match_discovered_to_library<'e, E>(exec: E, author_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    // Copy identifiers from discovered books to their library matches,
    // then delete the discovered duplicates.
    // Series membership is handled separately via book_series operations.
    let result = sqlx::query!(
        r#"
        WITH matches AS (
            SELECT non_lib.id AS discovered_id,
                   lib.id AS library_id,
                   non_lib.identifiers AS discovered_identifiers
            FROM books non_lib
            JOIN book_authors non_lib_ba ON non_lib_ba.book_id = non_lib.id
            JOIN books lib ON (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = lib.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = lib.id))
                          AND LOWER(lib.title) = LOWER(non_lib.title)
            JOIN book_authors lib_ba ON lib_ba.book_id = lib.id
                                    AND lib_ba.author_id = non_lib_ba.author_id
            WHERE non_lib.discovered_from_author_id = $1
              AND NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = non_lib.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = non_lib.id))
        ),
        update_lib AS (
            UPDATE books lib
            SET identifiers = lib.identifiers || m.discovered_identifiers,
                updated_at = NOW()
            FROM matches m
            WHERE lib.id = m.library_id
        )
        DELETE FROM books
        WHERE id IN (SELECT discovered_id FROM matches)
        "#,
        author_id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected())
}

/// Match series-discovered books to existing library books.
///
/// Finds non-library books in the specified series that already exist in the
/// library (matching by title and shared author). Copies identifiers from the
/// discovered book to the library book, then deletes the discovered book.
///
/// When several library books match, the one sitting at the same position in
/// this series wins, then any member of this series, then the oldest row.
///
/// This is a post-hoc cleanup for the series_enrich / media_root_match race
/// condition: even if both paths created entries, this merges them after the
/// fact once authors are linked.
pub async fn match_series_discovered_to_library<'e, E>(exec: E, series_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    // Series membership is handled separately via book_series operations.
    let result = sqlx::query!(
        r#"
        WITH matches AS (
            SELECT DISTINCT ON (non_lib.id)
                   non_lib.id AS discovered_id,
                   lib.id AS library_id,
                   non_lib.identifiers AS discovered_identifiers
            FROM books non_lib
            JOIN book_series non_lib_bs ON non_lib_bs.book_id = non_lib.id AND non_lib_bs.series_id = $1
            JOIN book_authors non_lib_ba ON non_lib_ba.book_id = non_lib.id
            JOIN books lib ON (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = lib.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = lib.id))
                          AND LOWER(lib.title) = LOWER(non_lib.title)
            JOIN book_authors lib_ba ON lib_ba.book_id = lib.id
                                    AND lib_ba.author_id = non_lib_ba.author_id
            WHERE NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = non_lib.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = non_lib.id))
            -- The loser's row is DELETED, so an arbitrary winner is unrecoverable:
            -- title + shared author does not separate an omnibus from a single
            -- volume. Trailing id makes the order total; created_at is not unique.
            ORDER BY non_lib.id,
                     (EXISTS (SELECT 1 FROM book_series lib_bs
                              WHERE lib_bs.book_id = lib.id
                                AND lib_bs.series_id = $1
                                AND lib_bs.series_index IS NOT NULL
                                AND lib_bs.series_index = non_lib_bs.series_index)) DESC,
                     (EXISTS (SELECT 1 FROM book_series lib_bs
                              WHERE lib_bs.book_id = lib.id
                                AND lib_bs.series_id = $1)) DESC,
                     lib.created_at ASC,
                     lib.id ASC
        ),
        -- Several discovered stubs can resolve to one library book; UPDATE ... FROM
        -- would apply an arbitrary one of them and the rest would be deleted unmerged.
        merged AS (
            SELECT library_id, jsonb_object_agg(key, value) AS identifiers
            FROM (
                SELECT DISTINCT ON (m.library_id, e.key) m.library_id, e.key, e.value
                FROM matches m
                CROSS JOIN LATERAL jsonb_each(m.discovered_identifiers) e
                ORDER BY m.library_id, e.key, m.discovered_id
            ) kv
            GROUP BY library_id
        ),
        update_lib AS (
            UPDATE books lib
            SET identifiers = lib.identifiers || mg.identifiers,
                updated_at = NOW()
            FROM merged mg
            WHERE lib.id = mg.library_id
        )
        DELETE FROM books
        WHERE id IN (SELECT discovered_id FROM matches)
        "#,
        series_id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected())
}

/// Book data for Kobo sync metadata.
#[derive(Debug)]
pub struct KoboMetadataInfo {
    pub book_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<OffsetDateTime>,
    pub series_id: Option<Uuid>,
    pub series_name: Option<String>,
    pub series_index: Option<f64>,
    pub identifiers: Option<Value>,
    pub page_count: Option<i32>,
    pub rating: Option<f64>,
}

/// Get book data for Kobo sync metadata (includes series name join).
pub async fn get_for_kobo_sync<'e, E>(exec: E, id: Uuid) -> Result<Option<KoboMetadataInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT b.id, b.title, b.description, b.language, b.publisher, b.pubdate,
               bs.series_id AS "series_id?", s.name AS "series_name?",
               bs.series_index AS "series_index?",
               b.identifiers, b.page_count, b.rating
        FROM books b
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        LEFT JOIN series s ON s.id = bs.series_id
        WHERE b.id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| KoboMetadataInfo {
        book_id: r.id,
        title: r.title,
        description: r.description,
        language: r.language,
        publisher: r.publisher,
        pubdate: r.pubdate,
        series_id: r.series_id,
        series_name: r.series_name,
        series_index: r.series_index,
        identifiers: Some(r.identifiers),
        page_count: r.page_count,
        rating: r.rating,
    }))
}

/// Recommendation row from the recommendations query.
#[derive(Debug)]
pub struct RecommendationRow {
    pub book_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<OffsetDateTime>,
    pub series_id: Option<Uuid>,
    pub series_name: Option<String>,
    pub series_index: Option<f64>,
    pub identifiers: Option<Value>,
    pub authors: Vec<String>,
    pub reason: String,
    pub progress_percent: Option<f64>,
}

/// Discovered book info for library matching.
#[derive(Debug)]
pub struct DiscoveredBookMatchInfo {
    pub id: Uuid,
    pub title: String,
    pub hardcover_id: String,
    pub hardcover_slug: Option<String>,
}

/// List discovered books from an author that have hardcover IDs (for library matching).
pub async fn list_discovered_for_matching<'e, E>(
    exec: E,
    author_id: Uuid,
) -> Result<Vec<DiscoveredBookMatchInfo>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT b.id, b.title,
               b.identifiers->>'hardcover' AS "hardcover_id!",
               b.identifiers->>'hardcover_slug' AS hardcover_slug
        FROM books b
        WHERE b.discovered_from_author_id = $1
          AND NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
          AND b.identifiers->>'hardcover' IS NOT NULL
        "#,
        author_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| DiscoveredBookMatchInfo {
            id: r.id,
            title: r.title,
            hardcover_id: r.hardcover_id,
            hardcover_slug: r.hardcover_slug,
        })
        .collect())
}

/// Find a library book by title and author for matching discovered books.
pub async fn find_library_match_by_title<'e, E>(
    exec: E,
    author_id: Uuid,
    title: &str,
    hardcover_id: &str,
) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let id: Option<Uuid> = sqlx::query_scalar!(
        r#"
        SELECT b.id FROM books b
        JOIN book_authors ba ON ba.book_id = b.id
        WHERE ba.author_id = $1
          AND (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
          AND LOWER(b.title) = LOWER($2)
          AND (b.identifiers->>'hardcover' IS NULL OR b.identifiers->>'hardcover' = $3)
        LIMIT 1
        "#,
        author_id,
        title,
        hardcover_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(id)
}

/// Get book recommendations based on series and author.
///
/// Returns recommendations prioritized by:
/// 1. Next book(s) in the same series
/// 2. Other books by the same author(s)
///
/// Within each category, prioritizes unread books over read ones.
/// Only returns books that are entitled to the user/device.
pub async fn get_recommendations<'e, E>(
    exec: E,
    book_id: Uuid,
    user_id: Uuid,
    device_id: Uuid,
    limit: i32,
) -> Result<Vec<RecommendationRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        WITH source_book AS (
            SELECT b.id, bs.series_id,
                   COALESCE(bs.series_index, 0) AS series_pos
            FROM books b
            LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
            WHERE b.id = $1
        ),
        source_authors AS (
            SELECT author_id FROM book_authors WHERE book_id = $1
        ),
        entitled_books AS (
            SELECT DISTINCT e.book_id
            FROM entitlements e
            WHERE (
                e.scope_type = 'ALL_USERS'
                OR (e.scope_type = 'USER' AND e.scope_id = $2)
                OR (e.scope_type = 'DEVICE' AND e.scope_id = $3)
            )
            AND e.is_removed = FALSE
            AND e.book_id != $1
            AND EXISTS (
                SELECT 1 FROM book_files bf
                WHERE bf.book_id = e.book_id AND bf.available = TRUE
            )
        )
        SELECT
            b.id AS book_id,
            b.title,
            b.description,
            b.language,
            b.publisher,
            b.pubdate,
            bsp.series_id AS "series_id?",
            s.name AS "series_name?",
            bsp.series_index,
            b.identifiers,
            COALESCE(
                array_agg(DISTINCT a.name) FILTER (WHERE a.name IS NOT NULL),
                ARRAY[]::text[]
            ) AS "authors!: Vec<String>",
            rb.progress_percent,
            CASE
                WHEN bsp.series_id IS NOT NULL
                     AND bsp.series_id = sb.series_id
                     AND COALESCE(bsp.series_index, 0) > sb.series_pos
                THEN 'next_in_series'
                ELSE 'same_author'
            END AS "reason!",
            -- Priority: unread first (0), then started (1), then read (2)
            CASE
                WHEN rb.progress_percent IS NULL OR rb.progress_percent < 1 THEN 0
                WHEN rb.progress_percent >= 100 THEN 2
                ELSE 1
            END AS "read_priority!",
            CASE
                WHEN bsp.series_id IS NOT NULL
                     AND bsp.series_id = sb.series_id
                     AND COALESCE(bsp.series_index, 0) > sb.series_pos
                THEN 0
                ELSE 1
            END AS "sort_priority!",
            COALESCE(bsp.series_index, 999999) AS "sort_index!"
        FROM books b
        JOIN entitled_books eb ON eb.book_id = b.id
        LEFT JOIN book_series bsp ON bsp.book_id = b.id AND bsp.is_primary = TRUE
        LEFT JOIN series s ON s.id = bsp.series_id
        LEFT JOIN book_authors ba ON ba.book_id = b.id
        LEFT JOIN authors a ON a.id = ba.author_id
        CROSS JOIN source_book sb
        -- Join reading state to get progress
        LEFT JOIN reading_states rs ON rs.book_id = b.id AND rs.user_id = $2 AND rs.device_id = $3
        LEFT JOIN reading_bookmark rb ON rb.reading_state_id = rs.id
        WHERE (
            -- Same series, later position (check ANY shared series, not just primary)
            EXISTS (
                SELECT 1 FROM book_series bs_cand
                WHERE bs_cand.book_id = b.id
                  AND bs_cand.series_id = sb.series_id
                  AND COALESCE(bs_cand.series_index, 0) > sb.series_pos
            )
            OR
            -- Same author
            EXISTS (
                SELECT 1 FROM source_authors sa
                JOIN book_authors ba2 ON ba2.author_id = sa.author_id
                WHERE ba2.book_id = b.id
            )
        )
        GROUP BY b.id, bsp.series_id, bsp.series_index, s.name, sb.series_id, sb.series_pos, rs.id, rb.progress_percent
        ORDER BY
            CASE
                WHEN bsp.series_id IS NOT NULL
                     AND bsp.series_id = sb.series_id
                     AND COALESCE(bsp.series_index, 0) > sb.series_pos
                THEN 0
                ELSE 1
            END ASC,
            CASE
                WHEN rb.progress_percent IS NULL OR rb.progress_percent < 1 THEN 0
                WHEN rb.progress_percent >= 100 THEN 2
                ELSE 1
            END ASC,
            COALESCE(bsp.series_index, 999999) ASC,
            b.title ASC
        LIMIT $4
        "#,
        book_id,
        user_id,
        device_id,
        limit as i64,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| RecommendationRow {
            book_id: r.book_id,
            title: r.title,
            description: r.description,
            language: r.language,
            publisher: r.publisher,
            pubdate: r.pubdate,
            series_id: r.series_id,
            series_name: r.series_name,
            series_index: r.series_index,
            identifiers: Some(r.identifiers),
            authors: r.authors,
            reason: r.reason,
            progress_percent: r.progress_percent,
        })
        .collect())
}

/// Book summary (title and authors) for display.
pub struct BookSummary {
    pub title: String,
    pub authors: String,
}

/// Get a book's summary (title and authors) for display.
pub async fn get_summary<'e, E>(exec: E, book_id: Uuid) -> Result<Option<BookSummary>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT b.title,
               COALESCE(string_agg(a.name, ', ' ORDER BY a.name), '') AS "authors!"
        FROM books b
        LEFT JOIN book_authors ba ON ba.book_id = b.id
        LEFT JOIN authors a ON a.id = ba.author_id
        WHERE b.id = $1
        GROUP BY b.id, b.title
        "#,
        book_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| BookSummary {
        title: r.title,
        authors: r.authors,
    }))
}

/// Check if a book is one someone wanted and is not yet in the library.
///
/// A discovered (`pending`), skipped or unwanted book is not, even while Kobload
/// still holds a want for it: Kobload's events must not promote it into the
/// want flow.
pub async fn is_wanted_book<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let exists: bool = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM books WHERE id = $1 AND want_status IN ('wanted', 'searching', 'downloading', 'watching', 'search_failed') AND NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = books.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = books.id))) AS "exists!""#,
        id,
    )
    .fetch_one(exec)
    .await?;
    Ok(exists)
}

/// Book data for edit form (existing values to preserve).
#[derive(Debug)]
pub struct BookEditData {
    pub title: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<OffsetDateTime>,
    pub series_id: Option<Uuid>,
    pub series_index: Option<f64>,
    pub identifiers: Value,
    pub page_count: Option<i32>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub ingest_status: Option<String>,
}

/// Get book data for editing.
pub async fn get_for_edit<'e, E>(exec: E, id: Uuid) -> Result<Option<BookEditData>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT b.title, b.description, b.language, b.publisher, b.pubdate,
               bs.series_id AS "series_id?", bs.series_index AS "series_index?",
               b.identifiers,
               b.page_count, b.rating, b.rating_count, b.ingest_status
        FROM books b
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        WHERE b.id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| BookEditData {
        title: r.title,
        description: r.description,
        language: r.language,
        publisher: r.publisher,
        pubdate: r.pubdate,
        series_id: r.series_id,
        series_index: r.series_index,
        identifiers: r.identifiers.clone(),
        page_count: r.page_count,
        rating: r.rating,
        rating_count: r.rating_count,
        ingest_status: r.ingest_status,
    }))
}

/// Wanted book metadata for search/queue operations.
#[derive(Debug)]
pub struct WantedBookMetadata {
    pub title: String,
    pub author: Option<String>,
    pub identifiers: Value,
    pub series_name: Option<String>,
    pub series_index: Option<f64>,
    pub images: ImageMap,
    pub want_format: String,
    pub narrator: Option<String>,
    pub pubdate: Option<time::OffsetDateTime>,
}

/// Get wanted book metadata for search/queue operations.
pub async fn get_wanted_metadata<'e, E>(exec: E, id: Uuid) -> Result<Option<WantedBookMetadata>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT b.title, b.identifiers, b.images, b.want_format, b.narrator, b.pubdate,
               s.name as "series_name?", bs.series_index AS "series_index?",
               (SELECT STRING_AGG(a.name, ', ') FROM book_authors ba JOIN authors a ON a.id = ba.author_id WHERE ba.book_id = b.id) as author
        FROM books b
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        LEFT JOIN series s ON s.id = bs.series_id
        WHERE b.id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| {
        let images: ImageMap = serde_json::from_value(r.images.clone()).unwrap_or_default();
        WantedBookMetadata {
            title: r.title,
            author: r.author,
            identifiers: r.identifiers.clone(),
            series_name: r.series_name,
            series_index: r.series_index,
            images,
            want_format: r.want_format,
            narrator: r.narrator,
            pubdate: r.pubdate,
        }
    }))
}

/// Wanted item for list view.
#[derive(Debug)]
pub struct WantedItemRow {
    pub id: Uuid,
    pub title: String,
    pub author: Option<String>,
    pub series_name: Option<String>,
    pub series_index: Option<f64>,
    pub identifiers: Value,
    pub images: ImageMap,
    pub want_status: String,
    pub attempt_status: Option<String>,
    pub last_attempt_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// List wanted books with optional status and search filters.
pub async fn list_wanted<'e, E>(
    exec: E,
    search: Option<&str>,
    status: Option<&str>,
) -> Result<Vec<WantedItemRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    // Build dynamic query with filters
    let mut sql = String::from(
        r#"
        SELECT
            b.id, b.title, b.identifiers, b.images, b.want_status,
            bs.series_id, bs.series_index, b.created_at, b.updated_at,
            s.name as series_name,
            (SELECT STRING_AGG(a.name, ', ') FROM book_authors ba JOIN authors a ON a.id = ba.author_id WHERE ba.book_id = b.id) as author,
            la.status as attempt_status,
            la.created_at as last_attempt_at
        FROM books b
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        LEFT JOIN series s ON s.id = bs.series_id
        LEFT JOIN LATERAL (
            SELECT wa.status, wa.created_at
            FROM wanted_attempts wa
            WHERE wa.book_id = b.id
            ORDER BY wa.created_at DESC
            LIMIT 1
        ) la ON TRUE
        WHERE NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)) AND b.want_status IN ('pending', 'wanted', 'searching', 'downloading', 'watching', 'search_failed')
        "#,
    );
    let mut params: Vec<String> = Vec::new();

    if let Some(s) = search {
        let search_trimmed = s.trim();
        if !search_trimmed.is_empty() {
            params.push(format!("%{}%", search_trimmed));
            let param_idx = params.len();
            sql.push_str(&format!(
                " AND (b.title ILIKE ${p} OR s.name ILIKE ${p} OR EXISTS (
                    SELECT 1 FROM book_authors ba2 JOIN authors a2 ON a2.id = ba2.author_id
                    WHERE ba2.book_id = b.id AND a2.name ILIKE ${p}
                ))",
                p = param_idx
            ));
        }
    }

    if let Some(st) = status {
        let status_trimmed = st.trim();
        if !status_trimmed.is_empty() {
            // Map status filter to either want_status or attempt_status
            if matches!(
                status_trimmed,
                "pending" | "wanted" | "searching" | "downloading" | "watching" | "search_failed"
            ) {
                params.push(status_trimmed.to_string());
                let param_idx = params.len();
                sql.push_str(&format!(" AND b.want_status = ${}", param_idx));
            } else {
                // Filter by latest attempt status
                params.push(status_trimmed.to_string());
                let param_idx = params.len();
                sql.push_str(&format!(" AND la.status = ${}", param_idx));
            }
        }
    }

    sql.push_str(" ORDER BY b.created_at DESC");

    let mut q = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()));
    for param in &params {
        q = q.bind(param);
    }
    let rows = q.fetch_all(exec).await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let images_json: serde_json::Value = r.get("images");
            let images: ImageMap = serde_json::from_value(images_json).unwrap_or_default();
            WantedItemRow {
                id: r.get("id"),
                title: r.get("title"),
                author: r.get("author"),
                series_name: r.get("series_name"),
                series_index: r.get("series_index"),
                identifiers: r.get("identifiers"),
                images,
                want_status: r.get("want_status"),
                attempt_status: r.get("attempt_status"),
                last_attempt_at: r.get("last_attempt_at"),
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            }
        })
        .collect())
}

// =============================================================================
// Unavailable Books
// =============================================================================

/// Book with unavailable files info.
#[derive(Debug)]
pub struct UnavailableBookRow {
    pub id: Uuid,
    pub title: String,
    pub authors: Vec<String>,
    pub unavailable_files: i64,
    pub total_files: i64,
    pub unavailable_since: Option<OffsetDateTime>,
}

/// List books that have at least one unavailable file.
pub async fn list_unavailable<'e, E>(exec: E) -> Result<Vec<UnavailableBookRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT
            b.id,
            b.title,
            COALESCE(array_agg(DISTINCT a.name) FILTER (WHERE a.name IS NOT NULL), ARRAY[]::text[]) AS "authors!: Vec<String>",
            COUNT(bf.id) FILTER (WHERE bf.available = FALSE) AS "unavailable_files!",
            COUNT(bf.id) AS "total_files!",
            MIN(bf.unavailable_since) FILTER (WHERE bf.available = FALSE) AS unavailable_since
        FROM books b
        JOIN book_files bf ON bf.book_id = b.id
        LEFT JOIN book_authors ba ON ba.book_id = b.id
        LEFT JOIN authors a ON a.id = ba.author_id
        WHERE EXISTS (
            SELECT 1 FROM book_files bf2
            WHERE bf2.book_id = b.id AND bf2.available = FALSE
        )
        GROUP BY b.id
        ORDER BY MIN(bf.unavailable_since) DESC NULLS LAST
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| UnavailableBookRow {
            id: r.id,
            title: r.title,
            authors: r.authors,
            unavailable_files: r.unavailable_files,
            total_files: r.total_files,
            unavailable_since: r.unavailable_since,
        })
        .collect())
}

/// Get IDs of books where ALL files are unavailable.
pub async fn list_fully_unavailable_ids<'e, E>(exec: E) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids: Vec<Uuid> = sqlx::query_scalar!(
        r#"
        SELECT b.id
        FROM books b
        WHERE EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)
        AND NOT EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id AND bf.available = TRUE)
        "#,
    )
    .fetch_all(exec)
    .await?;
    Ok(ids)
}

/// Get a single wanted item with full details.
pub async fn get_wanted_item<'e, E>(exec: E, id: Uuid) -> Result<Option<WantedItemRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT
            b.id, b.title, b.identifiers, b.images, b.want_status,
            bs.series_id AS "series_id?", bs.series_index AS "series_index?",
            b.created_at, b.updated_at,
            s.name as "series_name?",
            (SELECT STRING_AGG(a.name, ', ') FROM book_authors ba JOIN authors a ON a.id = ba.author_id WHERE ba.book_id = b.id) as author,
            la.status as "attempt_status?",
            la.created_at as "last_attempt_at?"
        FROM books b
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        LEFT JOIN series s ON s.id = bs.series_id
        LEFT JOIN LATERAL (
            SELECT wa.status, wa.created_at
            FROM wanted_attempts wa
            WHERE wa.book_id = b.id
            ORDER BY wa.created_at DESC
            LIMIT 1
        ) la ON TRUE
        WHERE b.id = $1
          AND NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
          AND b.want_status IN ('pending', 'wanted', 'searching', 'downloading', 'watching', 'search_failed')
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| {
        let images: ImageMap = serde_json::from_value(r.images.clone()).unwrap_or_default();
        WantedItemRow {
            id: r.id,
            title: r.title,
            author: r.author,
            series_name: r.series_name,
            series_index: r.series_index,
            identifiers: r.identifiers.clone(),
            images,
            want_status: r.want_status,
            attempt_status: r.attempt_status,
            last_attempt_at: r.last_attempt_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }))
}

// =============================================================================
// Watch Sync Support
// =============================================================================

/// Row for watch sync operations (simplified, no attempt info needed).
#[derive(Debug)]
pub struct WantedSyncRow {
    pub id: Uuid,
    pub title: String,
    pub author: Option<String>,
    pub series_name: Option<String>,
    pub series_index: Option<f64>,
    pub identifiers: Value,
    pub want_status: String,
    pub want_format: String,
    pub narrator: Option<String>,
    pub pubdate: Option<time::OffsetDateTime>,
}

/// List all wanted books that should be synced to watch patterns.
/// Returns books with no files (not in library) and want_status is an active wanted state.
pub async fn list_syncable_wanted<'e, E>(exec: E) -> Result<Vec<WantedSyncRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT b.id, b.title, b.identifiers, b.want_status, b.want_format, b.narrator,
               b.pubdate,
               s.name as "series_name?", bs.series_index AS "series_index?",
               (SELECT STRING_AGG(a.name, ', ')
                FROM book_authors ba
                JOIN authors a ON a.id = ba.author_id
                WHERE ba.book_id = b.id) as author
        FROM books b
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        LEFT JOIN series s ON s.id = bs.series_id
        WHERE NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
          AND b.want_status IN ('wanted', 'searching', 'downloading', 'watching', 'search_failed')
        ORDER BY b.created_at DESC
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| WantedSyncRow {
            id: r.id,
            title: r.title,
            author: r.author,
            series_name: r.series_name,
            series_index: r.series_index,
            identifiers: r.identifiers,
            want_status: r.want_status,
            want_format: r.want_format,
            narrator: r.narrator,
            pubdate: r.pubdate,
        })
        .collect())
}

/// Get a single wanted book for sync operations.
/// Returns book with no files (not in library), no want_status filter.
pub async fn get_wanted_for_sync<'e, E>(exec: E, id: Uuid) -> Result<Option<WantedSyncRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT b.id, b.title, b.identifiers, b.want_status, b.want_format, b.narrator,
               b.pubdate,
               s.name as "series_name?", bs.series_index AS "series_index?",
               (SELECT STRING_AGG(a.name, ', ')
                FROM book_authors ba
                JOIN authors a ON a.id = ba.author_id
                WHERE ba.book_id = b.id) as author
        FROM books b
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        LEFT JOIN series s ON s.id = bs.series_id
        WHERE b.id = $1
          AND NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
          AND b.want_status IN ('wanted', 'searching', 'downloading', 'watching', 'search_failed')
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| WantedSyncRow {
        id: r.id,
        title: r.title,
        author: r.author,
        series_name: r.series_name,
        series_index: r.series_index,
        identifiers: r.identifiers,
        want_status: r.want_status,
        want_format: r.want_format,
        narrator: r.narrator,
        pubdate: r.pubdate,
    }))
}

// =============================================================================
// Library Book Listings
// =============================================================================

/// A library book row with all fields for listing views.
#[derive(Debug)]
pub struct LibraryBookRow {
    pub id: Uuid,
    pub title: String,
    /// Title with a leading article stripped (DB trigger keeps it in step with `title`).
    /// This — not `title` — is what the title-sorted queries order by.
    pub sort_title: Option<String>,
    pub description: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<OffsetDateTime>,
    pub series_id: Option<Uuid>,
    pub series_name: Option<String>,
    pub series_index: Option<f64>,
    pub language: Option<String>,
    pub identifiers: Value,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub is_in_library: bool,
    pub want_status: String,
    pub page_count: Option<i32>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub images: ImageMap,
    pub authors: serde_json::Value,
    pub formats: Vec<String>,
    pub synced: bool,
    pub pending_sync: bool,
    pub genres: Vec<String>,
    pub provider_metadata: serde_json::Value,
    pub narrator: Option<String>,
    pub duration_seconds: Option<i32>,
    pub is_abridged: bool,
    pub want_format: String,
    pub has_audiobook: bool,
    pub media_root_id: Option<Uuid>,
}

/// Stands in for a user id where no user is in scope: matches no USER or DEVICE
/// entitlement and no device row, so only ALL_USERS counts.
const NO_USER: &str = "NULL::uuid";

/// Which entitlements answer for `user_id`.
///
/// All three scopes count — a book can be entitled to everyone, to one user, or
/// to a single device — but only this user's, because Synced/Pending is per-user
/// state and another household member's Kobo must not answer for it.
fn entitlement_visibility(user_param: &str) -> String {
    format!(
        "(e.scope_type = 'ALL_USERS'
              OR (e.scope_type = 'USER' AND e.scope_id = {u})
              OR (e.scope_type = 'DEVICE' AND e.scope_id IN (
                     SELECT d_scope.id FROM devices d_scope WHERE d_scope.user_id = {u})))",
        u = user_param
    )
}

fn entitlement_join(user_param: &str) -> String {
    format!(
        "LEFT JOIN entitlements e ON e.book_id = b.id AND {}",
        entitlement_visibility(user_param)
    )
}

/// The oldest sync across this user's own Kobos. `infinity` when they have none,
/// so a user without a reader reads as up to date rather than permanently stale.
fn last_kobo_sync(user_param: &str) -> String {
    format!(
        "COALESCE(
                 (SELECT MIN(dss.last_sync_at) FROM device_sync_state dss
                  JOIN devices d ON d.id = dss.device_id
                  WHERE d.user_id = {u} AND d.device_type = 'kobo' AND dss.last_sync_at IS NOT NULL),
                 'infinity'::timestamptz
             )",
        u = user_param
    )
}

const SYNCED_EXPR: &str =
    "BOOL_OR(CASE WHEN e.id IS NOT NULL AND e.is_removed = FALSE THEN TRUE ELSE FALSE END)";

fn pending_sync_expr(user_param: &str) -> String {
    format!(
        "BOOL_OR(
                CASE WHEN e.id IS NOT NULL AND e.is_removed = FALSE
                     AND GREATEST(e.updated_at, b.updated_at) > {last}
                THEN TRUE ELSE FALSE END
            )",
        last = last_kobo_sync(user_param)
    )
}

/// `filter=all|synced|pending` as a HAVING predicate, with the filter type bound
/// at `filter_param`.
fn sync_filter_having(filter_param: usize, user_param: &str) -> String {
    let pending = pending_sync_expr(user_param);
    format!(
        "(
            ${f} = 'all'
            OR (${f} = 'synced' AND {synced} = TRUE AND {pending} = FALSE)
            OR (${f} = 'pending' AND {pending} = TRUE)
        )",
        f = filter_param,
        synced = SYNCED_EXPR,
        pending = pending,
    )
}

/// Common SELECT columns for library book listings.
fn library_book_select(user_param: &str) -> String {
    format!(
        r#"
    b.id,
    b.title,
    b.sort_title,
    b.description,
    b.publisher,
    b.pubdate,
    bs.series_id,
    s.name AS series_name,
    bs.series_index,
    b.language,
    b.identifiers,
    b.created_at,
    b.updated_at,
    (EXISTS (SELECT 1 FROM book_files bf_lib WHERE bf_lib.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af_lib WHERE af_lib.book_id = b.id)) AS is_in_library,
    b.want_status,
    b.page_count,
    b.rating,
    b.rating_count,
    b.images,
    COALESCE(
        (SELECT jsonb_agg(DISTINCT jsonb_build_object('id', a2.id, 'name', a2.name))
         FROM book_authors ba2
         JOIN authors a2 ON a2.id = ba2.author_id
         WHERE ba2.book_id = b.id),
        '[]'::jsonb
    ) AS authors,
    COALESCE(array_agg(DISTINCT bf.format) FILTER (WHERE bf.format IS NOT NULL), ARRAY[]::text[]) AS formats,
    {synced} AS synced,
    {pending} AS pending_sync,
    COALESCE(
        (SELECT array_agg(g.name ORDER BY g.name) FROM book_genres bg2
         JOIN genres g ON g.id = bg2.genre_id WHERE bg2.book_id = b.id),
        ARRAY[]::text[]
    ) AS genres,
    b.provider_metadata,
    b.narrator,
    b.duration_seconds,
    b.is_abridged,
    b.want_format,
    EXISTS (
        SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id
    ) AS has_audiobook,
    b.media_root_id
"#,
        synced = SYNCED_EXPR,
        pending = pending_sync_expr(user_param),
    )
}

/// Common FROM/JOIN clause for library book listings.
fn library_book_joins(user_param: &str) -> String {
    format!(
        r#"
    FROM books b
    LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
    LEFT JOIN series s ON s.id = bs.series_id
    LEFT JOIN book_authors ba ON ba.book_id = b.id
    LEFT JOIN authors a ON a.id = ba.author_id
    LEFT JOIN book_files bf ON bf.book_id = b.id
    {join}
"#,
        join = entitlement_join(user_param)
    )
}

fn row_to_library_book(r: sqlx::postgres::PgRow) -> LibraryBookRow {
    let images_json: serde_json::Value = r.get("images");
    let images: ImageMap = serde_json::from_value(images_json).unwrap_or_default();
    LibraryBookRow {
        id: r.get("id"),
        title: r.get("title"),
        sort_title: r.get("sort_title"),
        description: r.get("description"),
        publisher: r.get("publisher"),
        pubdate: r.get("pubdate"),
        series_id: r.get("series_id"),
        series_name: r.get("series_name"),
        series_index: r.get("series_index"),
        language: r.get("language"),
        identifiers: r.get("identifiers"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
        is_in_library: r.get("is_in_library"),
        want_status: r.get("want_status"),
        page_count: r.get("page_count"),
        rating: r.get("rating"),
        rating_count: r.get("rating_count"),
        images,
        authors: r.get("authors"),
        formats: r.get("formats"),
        synced: r.get("synced"),
        pending_sync: r.get("pending_sync"),
        genres: r.get("genres"),
        provider_metadata: r.get("provider_metadata"),
        narrator: r.get("narrator"),
        duration_seconds: r.get("duration_seconds"),
        is_abridged: r.get("is_abridged"),
        want_format: r.get("want_format"),
        has_audiobook: r.get("has_audiobook"),
        media_root_id: r.get("media_root_id"),
    }
}

/// Get a single book by ID with full library listing data.
///
/// No user, so `synced`/`pending_sync` see only ALL_USERS entitlements. Callers
/// rendering the Synced badge want [`get_by_id_for_user`]; this one is for the
/// metadata paths (sidecar, reading state) that ignore both fields.
pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<LibraryBookRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let query = library_book_by_id_sql(NO_USER);

    let row = sqlx::query(sqlx::AssertSqlSafe(query.as_str()))
        .bind(id)
        .fetch_optional(exec)
        .await?;

    Ok(row.map(row_to_library_book))
}

/// Get a single book by ID with `synced`/`pending_sync` computed for `user_id`.
pub async fn get_by_id_for_user<'e, E>(
    exec: E,
    id: Uuid,
    user_id: Uuid,
) -> Result<Option<LibraryBookRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let query = library_book_by_id_sql("$2");

    let row = sqlx::query(sqlx::AssertSqlSafe(query.as_str()))
        .bind(id)
        .bind(user_id)
        .fetch_optional(exec)
        .await?;

    Ok(row.map(row_to_library_book))
}

fn library_book_by_id_sql(user_param: &str) -> String {
    format!(
        r#"
        SELECT {}
        {}
        WHERE b.id = $1
        GROUP BY b.id, bs.series_id, bs.series_index, s.name
        "#,
        library_book_select(user_param),
        library_book_joins(user_param)
    )
}

/// List books by genre ID (library books only).
pub async fn list_by_genre<'e, E>(
    exec: E,
    genre_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<LibraryBookRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let query = format!(
        r#"
        SELECT {}
        FROM book_genres bg
        JOIN books b ON b.id = bg.book_id
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        LEFT JOIN series s ON s.id = bs.series_id
        LEFT JOIN book_authors ba ON ba.book_id = b.id
        LEFT JOIN authors a ON a.id = ba.author_id
        LEFT JOIN book_files bf ON bf.book_id = b.id
        {}
        WHERE bg.genre_id = $1 AND (EXISTS (SELECT 1 FROM book_files bf_lib WHERE bf_lib.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af_lib WHERE af_lib.book_id = b.id)) AND b.ingest_status IS NULL
        GROUP BY b.id, bs.series_id, bs.series_index, s.name
        ORDER BY b.title ASC
        "#,
        library_book_select("$2"),
        entitlement_join("$2")
    );

    let rows = sqlx::query(sqlx::AssertSqlSafe(query.as_str()))
        .bind(genre_id)
        .bind(user_id)
        .fetch_all(exec)
        .await?;

    Ok(rows.into_iter().map(row_to_library_book).collect())
}

/// List books by author ID with library filter option.
pub async fn list_by_author<'e, E>(
    exec: E,
    author_id: Uuid,
    user_id: Uuid,
    include_not_owned: bool,
) -> Result<Vec<LibraryBookRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    // Always exclude ingest queue items from library views
    let library_filter = if include_not_owned {
        "AND b.ingest_status IS NULL"
    } else {
        "AND (EXISTS (SELECT 1 FROM book_files bf_lib WHERE bf_lib.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af_lib WHERE af_lib.book_id = b.id)) AND b.ingest_status IS NULL"
    };

    let query = format!(
        r#"
        SELECT {}
        {}
        WHERE b.id IN (
            SELECT book_id FROM book_authors WHERE author_id = $1
        )
        {}
        GROUP BY b.id, bs.series_id, bs.series_index, s.name
        ORDER BY
            COALESCE(s.name, 'ZZZZZ') ASC,
            COALESCE(bs.series_index, 999999) ASC,
            b.title ASC
        "#,
        library_book_select("$2"),
        library_book_joins("$2"),
        library_filter
    );

    let rows = sqlx::query(sqlx::AssertSqlSafe(query.as_str()))
        .bind(author_id)
        .bind(user_id)
        .fetch_all(exec)
        .await?;

    Ok(rows.into_iter().map(row_to_library_book).collect())
}

/// List books by series ID with library filter option.
pub async fn list_by_series<'e, E>(
    exec: E,
    series_id: Uuid,
    user_id: Uuid,
    include_not_owned: bool,
) -> Result<Vec<LibraryBookRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    // Always exclude ingest queue items from library views
    let library_filter = if include_not_owned {
        "AND b.ingest_status IS NULL"
    } else {
        "AND (EXISTS (SELECT 1 FROM book_files bf_lib WHERE bf_lib.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af_lib WHERE af_lib.book_id = b.id)) AND b.ingest_status IS NULL"
    };

    let query = format!(
        r#"
        SELECT {}
        {}
        JOIN book_series bs_filter ON bs_filter.book_id = b.id AND bs_filter.series_id = $1
        WHERE TRUE {}
        GROUP BY b.id, bs.series_id, bs.series_index, s.name, bs_filter.series_index
        ORDER BY
            COALESCE(bs_filter.series_index, 999999) ASC,
            b.title ASC
        "#,
        library_book_select("$2"),
        library_book_joins("$2"),
        library_filter
    );

    let rows = sqlx::query(sqlx::AssertSqlSafe(query.as_str()))
        .bind(series_id)
        .bind(user_id)
        .fetch_all(exec)
        .await?;

    Ok(rows.into_iter().map(row_to_library_book).collect())
}

/// List books by shelf ID (library books only).
pub async fn list_by_shelf<'e, E>(
    exec: E,
    shelf_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<LibraryBookRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let query = format!(
        r#"
        SELECT {}
        FROM shelf_items si
        JOIN books b ON b.id = si.book_id
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        LEFT JOIN series s ON s.id = bs.series_id
        LEFT JOIN book_authors ba ON ba.book_id = b.id
        LEFT JOIN authors a ON a.id = ba.author_id
        LEFT JOIN book_files bf ON bf.book_id = b.id
        {}
        WHERE si.shelf_id = $1
        GROUP BY b.id, bs.series_id, bs.series_index, s.name
        ORDER BY b.created_at DESC
        "#,
        library_book_select("$2"),
        entitlement_join("$2")
    );

    let rows = sqlx::query(sqlx::AssertSqlSafe(query.as_str()))
        .bind(shelf_id)
        .bind(user_id)
        .fetch_all(exec)
        .await?;

    Ok(rows.into_iter().map(row_to_library_book).collect())
}

// =============================================================================
// Book Listing with Filters
// =============================================================================

use super::super::PaginationDirection;

/// Search term for book listing (supports negation and phrases).
#[derive(Debug, Clone)]
pub struct SearchTerm {
    pub pattern: String,
    pub negated: bool,
}

/// The expression every title-ordered book query sorts and paginates on. Cursors must
/// carry this value (see `LibraryBookRow::sort_title`), not the raw title.
const SORT_TITLE_EXPR: &str = "COALESCE(b.sort_title, b.title)";

/// Sort field for book listing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BookSortField {
    /// Sort by title alphabetically (A-Z).
    #[default]
    Title,
    /// Sort by created_at (most recent first).
    CreatedAt,
}

impl BookSortField {
    /// Parse from string, defaulting to Title for unknown values.
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "created_at" | "recent" => BookSortField::CreatedAt,
            _ => BookSortField::Title,
        }
    }
}

/// Cursor value that can be either a title string or a timestamp.
#[derive(Debug, Clone)]
pub enum BookCursor {
    /// Cursor for title-based pagination (title, id).
    Title(String, Uuid),
    /// Cursor for created_at-based pagination (timestamp, id).
    CreatedAt(OffsetDateTime, Uuid),
}

/// Filter parameters for book listing.
#[derive(Debug, Default)]
pub struct BookListFilter {
    /// Whose Synced/Pending state to compute. `Uuid::nil()` matches no user and
    /// no device, leaving only ALL_USERS entitlements visible.
    pub user_id: Uuid,
    /// Cursor for pagination - type depends on sort_field.
    pub cursor: Option<BookCursor>,
    /// Sort field for pagination.
    pub sort_field: BookSortField,
    /// Pagination direction (forward or backward from cursor).
    pub direction: PaginationDirection,
    /// Search terms (will be wrapped with %). Used for ILIKE fallback.
    pub search_terms: Vec<SearchTerm>,
    /// Pre-filtered IDs from search backend (takes precedence over search_terms).
    /// When provided, the query filters by these IDs instead of using ILIKE.
    pub search_ids: Option<Vec<Uuid>>,
    /// Filter by in_library status.
    pub in_library: Option<bool>,
    /// Filter by want_status.
    pub want_status: Option<String>,
    /// Filter type: "all", "synced", or "pending".
    pub filter_type: String,
    /// Filter to books that have audiobook files.
    pub has_audiobook: Option<bool>,
    /// Filter by media format: "ebook" or "audiobook".
    /// For library books, checks file existence; for wanted books, checks want_format.
    pub format: Option<String>,
    /// Filter to books with no media root (orphaned).
    pub orphaned: Option<bool>,
    /// Limit results (fetches limit+1 for has_more).
    pub limit: i64,
}

/// List books with filters (for admin listing).
/// Supports both forward and backward pagination via the direction field.
pub async fn list_with_filters<'e, E>(
    exec: E,
    filter: &BookListFilter,
) -> Result<Vec<LibraryBookRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    // If search_ids is provided but empty, return early - no matches
    if let Some(ref ids) = filter.search_ids
        && ids.is_empty()
    {
        return Ok(vec![]);
    }

    // Check if we should use search relevance ordering (when search_ids provided)
    let use_relevance_order = filter.search_ids.is_some();

    // Determine sort column and order direction based on sort_field and pagination direction
    // For title: ASC (A-Z)
    // For created_at: DESC (most recent first)
    // When search_ids is provided, we'll override with relevance ordering
    // Title order follows sort_title, matching the alphabet bar and first-by-letter
    // (both COALESCE the same way). Ordering by the raw title instead filed every
    // "The ..." under T while the bar advertised it under its real letter.
    let (sort_col, base_order, cursor_op, order_dir) = match filter.sort_field {
        BookSortField::Title => match filter.direction {
            PaginationDirection::Forward => (SORT_TITLE_EXPR, "ASC", ">", "ASC"),
            PaginationDirection::Backward => (SORT_TITLE_EXPR, "ASC", "<", "DESC"),
        },
        BookSortField::CreatedAt => match filter.direction {
            // For created_at, "forward" means getting older items (smaller timestamps)
            // "backward" means getting newer items (larger timestamps)
            PaginationDirection::Forward => ("b.created_at", "DESC", "<", "DESC"),
            PaginationDirection::Backward => ("b.created_at", "DESC", ">", "ASC"),
        },
    };
    let _ = base_order; // Used for reference, order_dir is what we use in query

    // Build the query dynamically based on cursor, search_ids, and search terms
    // Parameter indices: cursor uses $1, $2 if present, then search_ids, then search bindings, then other filters, then limit
    let (cursor_where, mut param_offset) = if filter.cursor.is_some() {
        (format!("({}, b.id) {} ($1, $2)", sort_col, cursor_op), 2)
    } else {
        (String::new(), 0)
    };

    // Handle search_ids (from search backend) or fall back to ILIKE search
    let (search_ids_condition, search_ids_param_idx) = if filter.search_ids.is_some() {
        param_offset += 1;
        (
            format!("b.id = ANY(${}::uuid[])", param_offset),
            Some(param_offset),
        )
    } else {
        (String::new(), None)
    };

    // Build ILIKE search conditions (only used when search_ids is not provided)
    let (search_conditions, search_bindings) = if filter.search_ids.is_none() {
        build_search_conditions(&filter.search_terms)
    } else {
        (String::new(), vec![])
    };

    // Remap search condition parameters to account for cursor and search_ids offset
    let remapped_search = if !search_conditions.is_empty() {
        let mut remapped = search_conditions.clone();
        for i in (1..=search_bindings.len()).rev() {
            remapped = remapped.replace(&format!("${}", i), &format!("${}", i + param_offset));
        }
        param_offset += search_bindings.len();
        remapped
    } else {
        String::new()
    };

    // Build additional filter conditions
    let mut extra_conditions = Vec::new();
    let mut extra_bindings: Vec<String> = Vec::new();

    // in_library filter
    let in_library_condition = match filter.in_library {
        Some(true) => "(EXISTS (SELECT 1 FROM book_files bf_lib WHERE bf_lib.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af_lib WHERE af_lib.book_id = b.id))".to_string(),
        Some(false) => "NOT (EXISTS (SELECT 1 FROM book_files bf_lib WHERE bf_lib.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af_lib WHERE af_lib.book_id = b.id))".to_string(),
        None => String::new(),
    };

    // want_status filter
    if let Some(ref status) = filter.want_status {
        if status == "library_visible" {
            // Show owned books + explicitly wanted books
            extra_conditions
                .push("((EXISTS (SELECT 1 FROM book_files bf_lib WHERE bf_lib.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af_lib WHERE af_lib.book_id = b.id)) OR b.want_status = 'wanted')".to_string());
        } else {
            param_offset += 1;
            extra_conditions.push(format!("b.want_status = ${}", param_offset));
            extra_bindings.push(status.clone());
        }
    }

    // has_audiobook filter
    if let Some(has_audio) = filter.has_audiobook {
        if has_audio {
            extra_conditions.push(
                "EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)".to_string(),
            );
        } else {
            extra_conditions.push(
                "NOT EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)".to_string(),
            );
        }
    }

    if let Some(condition) = filter.format.as_deref().and_then(format_condition) {
        extra_conditions.push(condition.to_string());
    }

    // orphaned filter (no media root)
    if let Some(orphaned) = filter.orphaned {
        if orphaned {
            extra_conditions.push("b.media_root_id IS NULL".to_string());
        } else {
            extra_conditions.push("b.media_root_id IS NOT NULL".to_string());
        }
    }

    let filter_param = param_offset + 1;
    let limit_param = param_offset + 2;
    let user_param = format!("${}", param_offset + 3);

    // Build WHERE clause
    let mut where_parts = Vec::new();
    // Always exclude ingest queue items from library views
    where_parts.push("b.ingest_status IS NULL".to_string());
    if !in_library_condition.is_empty() {
        where_parts.push(in_library_condition);
    }
    if filter.cursor.is_some() {
        where_parts.push(cursor_where);
    }
    if !search_ids_condition.is_empty() {
        where_parts.push(search_ids_condition);
    }
    if !remapped_search.is_empty() {
        where_parts.push(remapped_search);
    }
    for cond in &extra_conditions {
        where_parts.push(cond.clone());
    }

    let where_clause = if where_parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_parts.join(" AND "))
    };

    // Build ORDER BY clause - use relevance ordering when search_ids provided
    let order_clause = if use_relevance_order {
        // Use array_position to preserve MeiliSearch's relevance ordering
        // The search_ids parameter index was captured in search_ids_param_idx
        format!(
            "array_position(${}::uuid[], b.id), b.id",
            search_ids_param_idx.unwrap()
        )
    } else {
        format!("{} {}, b.id {}", sort_col, order_dir, order_dir)
    };

    let sql = format!(
        r#"
        SELECT {select}
        {joins}
        {where_clause}
        GROUP BY b.id, bs.series_id, bs.series_index, s.name
        HAVING {having}
        ORDER BY {order_clause}
        LIMIT ${limit_param}
        "#,
        select = library_book_select(&user_param),
        joins = library_book_joins(&user_param),
        where_clause = where_clause,
        having = sync_filter_having(filter_param, &user_param),
        order_clause = order_clause,
        limit_param = limit_param,
    );

    // Build and execute the query with dynamic bindings
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()));

    // Bind cursor parameters if present
    match &filter.cursor {
        Some(BookCursor::Title(title, id)) => {
            query = query.bind(title).bind(id);
        }
        Some(BookCursor::CreatedAt(ts, id)) => {
            query = query.bind(ts).bind(id);
        }
        None => {}
    }

    // Bind search_ids if present (before ILIKE search params)
    if let Some(ref ids) = filter.search_ids
        && search_ids_param_idx.is_some()
    {
        query = query.bind(ids);
    }

    // Bind ILIKE search parameters
    for pattern in &search_bindings {
        query = query.bind(pattern);
    }

    // Bind extra filter parameters (e.g., want_status)
    for binding in &extra_bindings {
        query = query.bind(binding);
    }

    // Bind filter, limit and the user whose sync state is being computed
    query = query
        .bind(&filter.filter_type)
        .bind(filter.limit + 1)
        .bind(filter.user_id);

    let rows = query.fetch_all(exec).await?;

    Ok(rows.into_iter().map(row_to_library_book).collect())
}

/// Count books matching filters (for pagination total).
/// This uses the same filtering logic as list_with_filters but returns only the count.
pub async fn count_with_filters<'e, E>(exec: E, filter: &BookListFilter) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    // If search_ids is provided but empty, return 0
    if let Some(ref ids) = filter.search_ids
        && ids.is_empty()
    {
        return Ok(0);
    }

    let mut param_offset = 0;

    // Handle search_ids (from search backend) or fall back to ILIKE search
    let (search_ids_condition, search_ids_param_idx) = if filter.search_ids.is_some() {
        param_offset += 1;
        (
            format!("b.id = ANY(${}::uuid[])", param_offset),
            Some(param_offset),
        )
    } else {
        (String::new(), None)
    };

    // Build ILIKE search conditions (only used when search_ids is not provided)
    let (search_conditions, search_bindings) = if filter.search_ids.is_none() {
        build_search_conditions(&filter.search_terms)
    } else {
        (String::new(), vec![])
    };

    // Remap search condition parameters
    let remapped_search = if !search_conditions.is_empty() {
        let mut remapped = search_conditions.clone();
        for i in (1..=search_bindings.len()).rev() {
            remapped = remapped.replace(&format!("${}", i), &format!("${}", i + param_offset));
        }
        param_offset += search_bindings.len();
        remapped
    } else {
        String::new()
    };

    // Build additional filter conditions
    let mut extra_conditions = Vec::new();
    let mut extra_bindings: Vec<String> = Vec::new();

    // in_library filter
    let in_library_condition = match filter.in_library {
        Some(true) => "(EXISTS (SELECT 1 FROM book_files bf_lib WHERE bf_lib.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af_lib WHERE af_lib.book_id = b.id))".to_string(),
        Some(false) => "NOT (EXISTS (SELECT 1 FROM book_files bf_lib WHERE bf_lib.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af_lib WHERE af_lib.book_id = b.id))".to_string(),
        None => String::new(),
    };

    // want_status filter
    if let Some(ref status) = filter.want_status {
        if status == "library_visible" {
            // Show owned books + explicitly wanted books
            extra_conditions
                .push("((EXISTS (SELECT 1 FROM book_files bf_lib WHERE bf_lib.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af_lib WHERE af_lib.book_id = b.id)) OR b.want_status = 'wanted')".to_string());
        } else {
            param_offset += 1;
            extra_conditions.push(format!("b.want_status = ${}", param_offset));
            extra_bindings.push(status.clone());
        }
    }

    // has_audiobook filter
    if let Some(has_audio) = filter.has_audiobook {
        if has_audio {
            extra_conditions.push(
                "EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)".to_string(),
            );
        } else {
            extra_conditions.push(
                "NOT EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)".to_string(),
            );
        }
    }

    if let Some(condition) = filter.format.as_deref().and_then(format_condition) {
        extra_conditions.push(condition.to_string());
    }

    // orphaned filter (no media root)
    if let Some(orphaned) = filter.orphaned {
        if orphaned {
            extra_conditions.push("b.media_root_id IS NULL".to_string());
        } else {
            extra_conditions.push("b.media_root_id IS NOT NULL".to_string());
        }
    }

    let filter_param = param_offset + 1;
    let user_param = format!("${}", param_offset + 2);

    // Build WHERE clause
    let mut where_parts = Vec::new();
    // Always exclude ingest queue items from count
    where_parts.push("b.ingest_status IS NULL".to_string());
    if !in_library_condition.is_empty() {
        where_parts.push(in_library_condition);
    }
    if !search_ids_condition.is_empty() {
        where_parts.push(search_ids_condition);
    }
    if !remapped_search.is_empty() {
        where_parts.push(remapped_search);
    }
    for cond in &extra_conditions {
        where_parts.push(cond.clone());
    }

    let where_clause = if where_parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_parts.join(" AND "))
    };

    let sql = format!(
        r#"
        SELECT COUNT(*) FROM (
            SELECT b.id
            FROM books b
            LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
            LEFT JOIN series s ON s.id = bs.series_id
            {joins}
            {where_clause}
            GROUP BY b.id
            HAVING {having}
        ) subq
        "#,
        joins = entitlement_join(&user_param),
        where_clause = where_clause,
        having = sync_filter_having(filter_param, &user_param),
    );

    let mut query = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()));

    // Bind search_ids if present
    if let Some(ref ids) = filter.search_ids
        && search_ids_param_idx.is_some()
    {
        query = query.bind(ids);
    }

    // Bind ILIKE search parameters
    for pattern in &search_bindings {
        query = query.bind(pattern);
    }

    // Bind extra filter parameters (e.g., want_status)
    for binding in &extra_bindings {
        query = query.bind(binding);
    }

    // Bind filter type and the user whose sync state is being computed
    query = query.bind(&filter.filter_type).bind(filter.user_id);

    let count: i64 = query.fetch_one(exec).await?;
    Ok(count)
}

/// Build search conditions and bindings from search terms.
/// Supports both UUID searches (book, series, or author ID) and text ILIKE searches.
fn build_search_conditions(terms: &[SearchTerm]) -> (String, Vec<String>) {
    if terms.is_empty() {
        return (String::new(), Vec::new());
    }

    let mut conditions = Vec::new();
    let mut bindings = Vec::new();

    for term in terms {
        let raw_term = term.pattern.trim();
        if raw_term.is_empty() {
            continue;
        }

        // Check if term looks like a UUID - match book ID, series ID, or author ID
        if let Ok(uuid) = Uuid::parse_str(raw_term) {
            let param_idx = bindings.len() + 1;
            bindings.push(uuid.to_string());

            // Cast the text parameter to UUID for comparison with UUID columns
            let uuid_match = format!(
                "(b.id = ${p}::uuid OR EXISTS (
                    SELECT 1 FROM book_series bs_uuid
                    WHERE bs_uuid.book_id = b.id AND bs_uuid.series_id = ${p}::uuid
                ) OR EXISTS (
                    SELECT 1 FROM book_authors ba_uuid
                    WHERE ba_uuid.book_id = b.id AND ba_uuid.author_id = ${p}::uuid
                ))",
                p = param_idx
            );

            if term.negated {
                conditions.push(format!("NOT {}", uuid_match));
            } else {
                conditions.push(uuid_match);
            }
            continue;
        }

        // Regular text search pattern
        let pattern = format!("%{}%", raw_term);
        if pattern.len() <= 2 {
            // Skip empty patterns (just %%)
            continue;
        }

        let param_idx = bindings.len() + 1;
        bindings.push(pattern);

        // Build condition that matches title, series name, OR author
        // Use COALESCE for series name to handle NULLs (books without series)
        let base_cond = format!(
            "(b.title ILIKE ${p} OR COALESCE(s.name ILIKE ${p}, FALSE) OR EXISTS (
                SELECT 1 FROM book_authors ba2 JOIN authors a2 ON a2.id = ba2.author_id
                WHERE ba2.book_id = b.id AND a2.name ILIKE ${p}
            ))",
            p = param_idx
        );

        if term.negated {
            conditions.push(format!("NOT {}", base_cond));
        } else {
            conditions.push(base_cond);
        }
    }

    if conditions.is_empty() {
        return (String::new(), Vec::new());
    }

    (conditions.join(" AND "), bindings)
}

// =============================================================================
// First By Letter (Alphabet Bar Jump Navigation)
// =============================================================================

/// Result from first_by_letter query - includes title for cursor creation.
#[derive(Debug)]
pub struct FirstByLetterResult {
    pub id: Uuid,
    pub title: String,
}

/// A book is in the library once it has a file of either kind — a want with no file yet
/// is a record of an intention, not a book on a shelf.
///
/// Written against the alias `b`, and shared with the alphabet bar so the bar and the
/// list it belongs to cannot disagree about what the shelf holds.
pub const IN_LIBRARY: &str = "(EXISTS (SELECT 1 FROM book_files bf_lib WHERE bf_lib.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af_lib WHERE af_lib.book_id = b.id))";

/// The complement of [`IN_LIBRARY`].
pub const NOT_IN_LIBRARY: &str = "NOT (EXISTS (SELECT 1 FROM book_files bf_lib WHERE bf_lib.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af_lib WHERE af_lib.book_id = b.id))";

/// The SQL that decides whether a book counts as an ebook or an audiobook.
///
/// A book in the library is judged on the files it has; one that is only wanted has none
/// yet, so it is judged on the format it was wanted in. Shared by the list queries and
/// the alphabet bar, which have to agree on what the shelf contains or the bar jumps to
/// a book the list does not show.
pub fn format_condition(format: &str) -> Option<&'static str> {
    match format {
        "audiobook" => Some(
            "(((EXISTS (SELECT 1 FROM book_files bf_lib WHERE bf_lib.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af_lib WHERE af_lib.book_id = b.id)) AND EXISTS (SELECT 1 FROM audiobook_files af2 WHERE af2.book_id = b.id)) \
             OR (NOT (EXISTS (SELECT 1 FROM book_files bf_lib2 WHERE bf_lib2.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af_lib2 WHERE af_lib2.book_id = b.id)) AND b.want_format IN ('audiobook', 'both')))",
        ),
        "ebook" => Some(
            "(((EXISTS (SELECT 1 FROM book_files bf_lib WHERE bf_lib.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af_lib WHERE af_lib.book_id = b.id)) AND EXISTS (SELECT 1 FROM book_files bf2 WHERE bf2.book_id = b.id)) \
             OR (NOT (EXISTS (SELECT 1 FROM book_files bf_lib2 WHERE bf_lib2.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af_lib2 WHERE af_lib2.book_id = b.id)) AND b.want_format IN ('ebook', 'both')))",
        ),
        _ => None,
    }
}

/// Find the first book starting with a given letter.
/// Used for alphabet bar jump navigation in the library UI.
///
/// # Arguments
/// * `letter` - The letter to search for (A-Z, or "#" for non-alphabetic)
/// * `in_library` - Filter by library status (true = library books only)
/// * `filter_type` - Filter type: "all", "synced", or "pending"
/// * `format` - Media format: "ebook" or "audiobook" (None for either)
pub async fn first_by_letter<'e, E>(
    exec: E,
    letter: &str,
    in_library: Option<bool>,
    filter_type: &str,
    format: Option<&str>,
    user_id: Uuid,
) -> Result<Option<FirstByLetterResult>>
where
    E: Executor<'e, Database = Postgres>,
{
    // Build the letter matching condition using sort_title (falls back to title if null)
    let letter_condition = if letter == "#" {
        // Non-alphabetic: sort_title doesn't start with a letter
        "COALESCE(b.sort_title, b.title) !~ '^[A-Za-z]'".to_string()
    } else {
        // Specific letter: sort_title starts with this letter (case-insensitive)
        "UPPER(LEFT(COALESCE(b.sort_title, b.title), 1)) = $2".to_string()
    };

    // Build in_library filter
    let library_condition = match in_library {
        Some(true) => IN_LIBRARY,
        Some(false) => NOT_IN_LIBRARY,
        None => "TRUE",
    };

    // $2 is the letter, and it is only bound when there is one to bind.
    let user_param = if letter == "#" { "$2" } else { "$3" };

    let sql = format!(
        r#"
        SELECT b.id, b.title
        FROM books b
        {entitlement_join}
        WHERE {letter_condition}
          AND {library_condition}
          AND {format_condition}
          AND b.ingest_status IS NULL
        GROUP BY b.id, b.title
        HAVING {having}
        ORDER BY COALESCE(b.sort_title, b.title) ASC
        LIMIT 1
        "#,
        entitlement_join = entitlement_join(user_param),
        letter_condition = letter_condition,
        library_condition = library_condition,
        format_condition = format.and_then(format_condition).unwrap_or("TRUE"),
        having = sync_filter_having(1, user_param),
    );

    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(filter_type);
    if letter != "#" {
        query = query.bind(letter.to_uppercase());
    }
    let row = query.bind(user_id).fetch_optional(exec).await?;

    Ok(row.map(|r| FirstByLetterResult {
        id: r.get("id"),
        title: r.get("title"),
    }))
}

/// Get just the page_count for a book (for sync progress calculations).
pub async fn get_page_count<'e, E>(exec: E, book_id: Uuid) -> Result<Option<i32>>
where
    E: Executor<'e, Database = Postgres>,
{
    let count: Option<i32> =
        sqlx::query_scalar!("SELECT page_count FROM books WHERE id = $1", book_id,)
            .fetch_optional(exec)
            .await?
            .flatten();
    Ok(count)
}
