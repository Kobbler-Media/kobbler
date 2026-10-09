//! Book CRUD operations.

use super::{BookInput, BookUpdate, DiscoveredBookInput, RepositoryError, Result};
use serde_json::Value;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Create Operations
// =============================================================================

/// Upsert a library book with full metadata.
pub async fn upsert_library_book<'e, E>(exec: E, input: BookInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let title = input.title.trim();
    if title.is_empty() {
        return Err(RepositoryError::Validation(
            "book title cannot be empty".to_string(),
        ));
    }

    let id = input.id.unwrap_or_else(Uuid::now_v7);
    let identifiers = input.identifiers.unwrap_or_else(|| serde_json::json!({}));
    let images_json = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();

    // Note: series_id/series_index are handled via book_series junction table by callers
    let row = sqlx::query!(
        r#"
        INSERT INTO books (id, title, description, language, publisher, pubdate,
                           identifiers, page_count, rating, rating_count, images,
                           want_status, narrator, want_format, is_abridged)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'none', $12,
                COALESCE($13, 'ebook'), COALESCE($14, FALSE))
        ON CONFLICT (id) DO UPDATE SET
            title = EXCLUDED.title,
            description = COALESCE(EXCLUDED.description, books.description),
            language = COALESCE(EXCLUDED.language, books.language),
            publisher = COALESCE(EXCLUDED.publisher, books.publisher),
            pubdate = COALESCE(EXCLUDED.pubdate, books.pubdate),
            identifiers = books.identifiers || EXCLUDED.identifiers,
            page_count = COALESCE(EXCLUDED.page_count, books.page_count),
            rating = COALESCE(EXCLUDED.rating, books.rating),
            rating_count = COALESCE(EXCLUDED.rating_count, books.rating_count),
            images = books.images || EXCLUDED.images,
            narrator = COALESCE(EXCLUDED.narrator, books.narrator),
            want_format = CASE
                WHEN books.want_format = EXCLUDED.want_format THEN books.want_format
                WHEN books.want_format = 'both' OR EXCLUDED.want_format = 'both' THEN 'both'
                ELSE 'both'
            END,
            is_abridged = COALESCE(EXCLUDED.is_abridged, books.is_abridged),
            want_status = 'none',
            updated_at = NOW()
        RETURNING id
        "#,
        id,
        title,
        input.description.as_deref(),
        input.language.as_deref(),
        input.publisher.as_deref(),
        input.pubdate,
        &identifiers,
        input.page_count,
        input.rating,
        input.rating_count,
        &images_json,
        input.narrator.as_deref(),
        input.want_format.as_deref(),
        input.is_abridged,
    )
    .fetch_one(exec)
    .await?;

    Ok(row.id)
}

/// Create a discovered (non-library) book.
pub async fn create_discovered_book<'e, E>(exec: E, input: DiscoveredBookInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let title = input.title.trim();
    if title.is_empty() {
        return Err(RepositoryError::Validation(
            "book title cannot be empty".to_string(),
        ));
    }

    let id = Uuid::now_v7();
    let identifiers = input.identifiers.unwrap_or_else(|| serde_json::json!({}));
    let images_json = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();
    let want_status = input.want_status.unwrap_or_else(|| "pending".to_string());
    let is_released = input.is_released.unwrap_or(true);

    let provider_metadata = input
        .provider_metadata
        .unwrap_or_else(|| serde_json::json!({}));

    // Note: series_id/series_index are handled via book_series junction table by callers
    let row = sqlx::query!(
        r#"
        INSERT INTO books (id, title, description, identifiers, images,
                           release_date, is_released, want_status,
                           discovered_from_author_id, page_count, rating, rating_count,
                           first_seen_at, provider_metadata,
                           narrator, want_format)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, NOW(), $13,
                $14, COALESCE($15, 'ebook'))
        RETURNING id
        "#,
        id,
        title,
        input.description.as_deref(),
        &identifiers,
        &images_json,
        input.release_date,
        is_released,
        &want_status,
        input.discovered_from_author_id,
        input.page_count,
        input.rating,
        input.rating_count,
        &provider_metadata,
        input.narrator.as_deref(),
        input.want_format.as_deref(),
    )
    .fetch_one(exec)
    .await?;

    Ok(row.id)
}

// =============================================================================
// Read Operations
// =============================================================================

/// Check if a book exists.
pub async fn exists<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let exists: bool = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM books WHERE id = $1) AS "exists!""#,
        id,
    )
    .fetch_one(exec)
    .await?;

    Ok(exists)
}

/// Result from find_by_identifier with all info needed for sync decisions.
#[derive(Debug)]
pub struct BookIdentifierMatch {
    pub id: Uuid,
    pub is_in_library: bool,
    pub user_edited_at: Option<OffsetDateTime>,
}

/// Find a book by an identifier (e.g., "hardcover", "isbn", "goodreads").
/// Returns book info if found, including user_edited_at for sync decisions.
pub async fn find_by_identifier<'e, E>(
    exec: E,
    identifier_type: &str,
    identifier_value: &str,
) -> Result<Option<BookIdentifierMatch>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"SELECT id,
            (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = books.id)
             OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = books.id)) AS "is_in_library!",
            user_edited_at
        FROM books WHERE identifiers->>$1 = $2"#,
        identifier_type,
        identifier_value,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| BookIdentifierMatch {
        id: r.id,
        is_in_library: r.is_in_library,
        user_edited_at: r.user_edited_at,
    }))
}

/// Get a book's identifiers.
pub async fn get_identifiers<'e, E>(exec: E, id: Uuid) -> Result<Option<Value>>
where
    E: Executor<'e, Database = Postgres>,
{
    let identifiers: Option<Value> =
        sqlx::query_scalar!("SELECT identifiers FROM books WHERE id = $1", id)
            .fetch_optional(exec)
            .await?;

    Ok(identifiers)
}

/// Get a book's identifiers and provider_metadata in a single query.
pub async fn get_identifiers_and_metadata<'e, E>(
    exec: E,
    id: Uuid,
) -> Result<Option<(Value, Value)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"SELECT identifiers, COALESCE(provider_metadata, '{}'::jsonb) AS "provider_metadata!" FROM books WHERE id = $1"#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| (r.identifiers.clone(), r.provider_metadata.clone())))
}

/// Get the title of a book.
pub async fn get_title<'e, E>(exec: E, id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let title: Option<String> = sqlx::query_scalar!("SELECT title FROM books WHERE id = $1", id)
        .fetch_optional(exec)
        .await?;

    Ok(title)
}

/// Get the cover image source URL of a book.
pub async fn get_cover_url<'e, E>(exec: E, id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let cover: Option<String> = sqlx::query_scalar!(
        "SELECT images->'cover'->>'source_url' FROM books WHERE id = $1",
        id,
    )
    .fetch_optional(exec)
    .await?
    .flatten();

    Ok(cover)
}

// =============================================================================
// Update Operations
// =============================================================================

/// Update a book with the provided fields. Only non-None fields are updated.
/// Identifiers are merged (not replaced) if provided.
/// updated_at is only bumped if at least one field actually changes.
pub async fn update<'e, E>(exec: E, id: Uuid, input: BookUpdate) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let identifiers = input.identifiers.unwrap_or_else(|| serde_json::json!({}));
    let images_json = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();

    let provider_metadata = input
        .provider_metadata
        .unwrap_or_else(|| serde_json::json!({}));

    // Note: series_id/series_index are handled via book_series junction table by callers,
    // not set directly on the books table.
    // Use IS DISTINCT FROM to only bump updated_at when values actually change.
    // This prevents series_enrich from causing pending_sync on books where
    // no actual changes occurred.
    // Note: identifiers, images, and provider_metadata use || merge, so we exclude them
    // from change detection - they're metadata-only fields that merge and always
    // appear to "change" even when adding empty {}.
    // Note: rating and rating_count are also excluded - they drift slightly on
    // each Hardcover API call (float precision) and shouldn't trigger device syncs.
    let result = sqlx::query!(
        r#"
        UPDATE books SET
            title = COALESCE($2, title),
            description = COALESCE($3, description),
            language = COALESCE($4, language),
            publisher = COALESCE($5, publisher),
            pubdate = COALESCE($6, pubdate),
            identifiers = identifiers || $7,
            page_count = COALESCE($8, page_count),
            rating = COALESCE($9, rating),
            rating_count = COALESCE($10, rating_count),
            images = images || $11,
            is_released = COALESCE($12, is_released),
            want_status = COALESCE($13, want_status),
            provider_metadata = provider_metadata || $14,
            narrator = COALESCE($15, narrator),
            duration_seconds = COALESCE($16, duration_seconds),
            is_abridged = COALESCE($17, is_abridged),
            want_format = COALESCE($18, want_format),
            updated_at = CASE WHEN
                title IS DISTINCT FROM COALESCE($2, title)
                OR description IS DISTINCT FROM COALESCE($3, description)
                OR language IS DISTINCT FROM COALESCE($4, language)
                OR publisher IS DISTINCT FROM COALESCE($5, publisher)
                OR pubdate IS DISTINCT FROM COALESCE($6, pubdate)
                OR page_count IS DISTINCT FROM COALESCE($8, page_count)
                OR is_released IS DISTINCT FROM COALESCE($12, is_released)
                OR want_status IS DISTINCT FROM COALESCE($13, want_status)
                OR narrator IS DISTINCT FROM COALESCE($15, narrator)
                OR duration_seconds IS DISTINCT FROM COALESCE($16, duration_seconds)
                OR is_abridged IS DISTINCT FROM COALESCE($17, is_abridged)
                OR want_format IS DISTINCT FROM COALESCE($18, want_format)
            THEN NOW() ELSE updated_at END
        WHERE id = $1
        "#,
        id,
        input.title.as_deref(),
        input.description.as_deref(),
        input.language.as_deref(),
        input.publisher.as_deref(),
        input.pubdate,
        &identifiers,
        input.page_count,
        input.rating,
        input.rating_count,
        &images_json,
        input.is_released,
        input.want_status.as_deref(),
        &provider_metadata,
        input.narrator.as_deref(),
        input.duration_seconds,
        input.is_abridged,
        input.want_format.as_deref(),
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Update a book's identifiers by merging with existing.
pub async fn update_identifiers<'e, E>(exec: E, id: Uuid, identifiers: Value) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE books SET identifiers = identifiers || $2, updated_at = NOW() WHERE id = $1",
        id,
        identifiers,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Replace a book's identifiers entirely.
pub async fn set_identifiers<'e, E>(exec: E, id: Uuid, identifiers: Value) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE books SET identifiers = $2 WHERE id = $1",
        id,
        identifiers
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Set a book's want status.
pub async fn set_want_status<'e, E>(exec: E, id: Uuid, status: &str) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE books SET want_status = $2, updated_at = NOW() WHERE id = $1",
        id,
        status,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Set want status only if current status matches expected.
pub async fn set_want_status_if<'e, E>(
    exec: E,
    id: Uuid,
    new_status: &str,
    expected_current: &str,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE books SET want_status = $2, updated_at = NOW() WHERE id = $1 AND want_status = $3",
        id,
        new_status,
        expected_current,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Set want status only for books not in library.
pub async fn set_want_status_if_not_in_library<'e, E>(
    exec: E,
    id: Uuid,
    status: &str,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"UPDATE books SET want_status = $2, updated_at = NOW()
        WHERE id = $1
        AND NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = books.id)
                 OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = books.id))"#,
        id,
        status,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Set the media_root_id for a book.
pub async fn set_media_root_id<'e, E>(
    exec: E,
    book_id: Uuid,
    media_root_id: Option<Uuid>,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE books SET media_root_id = $2, updated_at = NOW() WHERE id = $1",
        book_id,
        media_root_id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Touch a book's updated_at timestamp.
pub async fn touch<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!("UPDATE books SET updated_at = NOW() WHERE id = $1", id)
        .execute(exec)
        .await?;

    Ok(())
}

/// Touch multiple books' updated_at timestamps.
pub async fn touch_many<'e, E>(exec: E, ids: &[Uuid]) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE books SET updated_at = NOW() WHERE id = ANY($1)",
        ids
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Update series assignment for a single book via the junction table.
pub async fn set_series(
    pool: &sqlx::PgPool,
    id: Uuid,
    series_id: Option<Uuid>,
    series_index: Option<f64>,
) -> Result<()> {
    if let Some(sid) = series_id {
        super::super::book_series::add(pool, id, sid, series_index, true).await?;
    } else {
        super::super::book_series::clear_for_book(pool, id).await?;
    }
    Ok(())
}

/// Clear series from all books in a series.
pub async fn clear_series_from_books<'e, E>(exec: E, series_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    super::super::book_series::delete_for_series(exec, series_id).await?;
    Ok(())
}

/// Move all books from one series to another.
pub async fn move_books_to_series<'e, E>(
    exec: E,
    from_series_id: Uuid,
    to_series_id: Uuid,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    super::super::book_series::move_all_to_series(exec, from_series_id, to_series_id).await?;
    Ok(())
}

/// Update series index for a book (validates book is in the specified series).
pub async fn update_series_index<'e, E>(
    exec: E,
    book_id: Uuid,
    series_id: Uuid,
    series_index: Option<f64>,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query(
        "UPDATE book_series SET series_index = $1 WHERE book_id = $2 AND series_id = $3",
    )
    .bind(series_index)
    .bind(book_id)
    .bind(series_id)
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

// =============================================================================
// Bulk Want Operations
// =============================================================================

/// Mark all pending books for an author as wanted.
/// Returns the list of book IDs that were updated.
pub async fn want_all_pending_for_author<'e, E>(exec: E, author_id: Uuid) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        UPDATE books
        SET want_status = 'wanted', updated_at = NOW()
        WHERE id IN (
            SELECT ba.book_id
            FROM book_authors ba
            JOIN books b ON b.id = ba.book_id
            WHERE ba.author_id = $1
            AND NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)
                     OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
            AND (b.want_status IS NULL OR b.want_status = 'pending')
        )
        RETURNING id
        "#,
        author_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows.into_iter().map(|r| r.id).collect())
}

/// Mark all pending books for a series as wanted.
/// Returns the list of book IDs that were updated.
pub async fn want_all_pending_for_series<'e, E>(exec: E, series_id: Uuid) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        UPDATE books
        SET want_status = 'wanted', updated_at = NOW()
        WHERE id IN (SELECT book_id FROM book_series WHERE series_id = $1)
        AND NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = books.id)
                 OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = books.id))
        AND (want_status IS NULL OR want_status = 'pending')
        RETURNING id
        "#,
        series_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows.into_iter().map(|r| r.id).collect())
}

/// The want statuses that mean a book is outstanding: asked for, and still being
/// chased. 'pending' is not one of them — a discovered book nobody has acted on
/// was never wanted, so an unwant must not swallow it — and neither is 'skipped'.
const OUTSTANDING_WANT_STATUSES: &[&str] = &[
    "wanted",
    "searching",
    "downloading",
    "watching",
    "search_failed",
];

/// Drop every outstanding want on an author's books, leaving 'pending',
/// 'skipped' and anything already in the library alone.
/// Returns the book IDs cleared, so callers can tear down their watch patterns.
pub async fn unwant_all_for_author<'e, E>(exec: E, author_id: Uuid) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!(
        r#"
        UPDATE books
        SET want_status = 'none', updated_at = NOW()
        WHERE id IN (
            SELECT ba.book_id
            FROM book_authors ba
            JOIN books b ON b.id = ba.book_id
            WHERE ba.author_id = $1
            AND NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)
                     OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
            AND b.want_status = ANY($2)
        )
        RETURNING id
        "#,
        author_id,
        OUTSTANDING_WANT_STATUSES as &[&str],
    )
    .fetch_all(exec)
    .await?;

    Ok(ids)
}

/// Drop every outstanding want on a series' books.
/// See [`unwant_all_for_author`] for what is deliberately left untouched.
pub async fn unwant_all_for_series<'e, E>(exec: E, series_id: Uuid) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!(
        r#"
        UPDATE books
        SET want_status = 'none', updated_at = NOW()
        WHERE id IN (SELECT book_id FROM book_series WHERE series_id = $1)
        AND NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = books.id)
                 OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = books.id))
        AND want_status = ANY($2)
        RETURNING id
        "#,
        series_id,
        OUTSTANDING_WANT_STATUSES as &[&str],
    )
    .fetch_all(exec)
    .await?;

    Ok(ids)
}

// =============================================================================
// Ingest Status Operations
// =============================================================================

/// Input for creating an ingest book (a book in the ingest queue).
#[derive(Debug, Default)]
pub struct IngestBookInput {
    pub id: Option<Uuid>,
    pub title: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<time::OffsetDateTime>,
    pub ingest_status: String,
}

impl IngestBookInput {
    pub fn new(title: impl Into<String>, status: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ingest_status: status.into(),
            ..Default::default()
        }
    }

    pub fn with_id(mut self, id: Uuid) -> Self {
        self.id = Some(id);
        self
    }
}

/// Create a book for the ingest queue (not in library, with ingest_status set).
pub async fn create_ingest_book<'e, E>(exec: E, input: IngestBookInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let title = input.title.trim();
    if title.is_empty() {
        return Err(RepositoryError::Validation(
            "book title cannot be empty".to_string(),
        ));
    }

    let id = input.id.unwrap_or_else(Uuid::now_v7);

    let row = sqlx::query!(
        r#"
        INSERT INTO books (id, title, description, language, publisher, pubdate,
                           want_status, ingest_status)
        VALUES ($1, $2, $3, $4, $5, $6, 'none', $7)
        RETURNING id
        "#,
        id,
        title,
        input.description.as_deref(),
        input.language.as_deref(),
        input.publisher.as_deref(),
        input.pubdate,
        &input.ingest_status,
    )
    .fetch_one(exec)
    .await?;

    Ok(row.id)
}

/// Set the ingest status for a book.
/// Setting to None "approves" the book into the library.
pub async fn set_ingest_status<'e, E>(exec: E, id: Uuid, status: Option<&str>) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = if let Some(status) = status {
        sqlx::query!(
            "UPDATE books SET ingest_status = $2, updated_at = NOW() WHERE id = $1",
            id,
            status,
        )
        .execute(exec)
        .await?
    } else {
        // Clear ingest_status (approve into library)
        sqlx::query!(
            "UPDATE books SET ingest_status = NULL, want_status = 'none', updated_at = NOW() WHERE id = $1",
            id,
        )
        .execute(exec)
        .await?
    };

    Ok(result.rows_affected() > 0)
}

// =============================================================================
// Auto-Want Clearing Operations
// =============================================================================

/// Clear want_status on discovered books for an author.
/// Only clears books that are still "wanted" or "pending" (not skipped or in-library).
pub async fn clear_auto_wanted_for_author<'e, E>(exec: E, author_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE books
        SET want_status = 'none', updated_at = NOW()
        WHERE id IN (
            SELECT ba.book_id
            FROM book_authors ba
            JOIN books b ON b.id = ba.book_id
            WHERE ba.author_id = $1
            AND NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)
                     OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
            AND b.want_status IN ('wanted', 'pending')
        )
        "#,
        author_id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected())
}

/// Clear want_status on discovered books for a series.
/// Only clears books that are still "wanted" or "pending" (not skipped or in-library).
pub async fn clear_auto_wanted_for_series<'e, E>(exec: E, series_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE books
        SET want_status = 'none', updated_at = NOW()
        WHERE id IN (SELECT book_id FROM book_series WHERE series_id = $1)
        AND NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = books.id)
                 OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = books.id))
        AND want_status IN ('wanted', 'pending')
        "#,
        series_id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected())
}

/// Delete discovered books for an author that are NOT in the given set of Hardcover IDs,
/// AND are not in the library, AND have want_status = 'none' (not wanted/skipped).
pub async fn delete_stale_discovered<'e, E>(
    exec: E,
    author_id: Uuid,
    current_hardcover_ids: &[String],
) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        DELETE FROM books
        WHERE discovered_from_author_id = $1
          AND NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = books.id)
                   OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = books.id))
          AND want_status = 'none'
          AND NOT (identifiers->>'hardcover' = ANY($2))
        "#,
        author_id,
        current_hardcover_ids,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected())
}

/// Delete orphaned books: no files (ebook or audiobook), not wanted, no media
/// root file pointing to them, and not recently updated (grace period for
/// in-progress jobs).
/// Returns IDs of deleted books for search index cleanup.
pub async fn delete_orphaned_books<'e, E>(exec: E) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!(
        r#"
        DELETE FROM books
        WHERE want_status = 'none'
          AND updated_at < NOW() - INTERVAL '30 minutes'
          AND NOT EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = books.id)
          AND NOT EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = books.id)
          AND NOT EXISTS (
              SELECT 1 FROM media_root_files mrf
              WHERE mrf.entity_id = books.id AND mrf.entity_type = 'book'
          )
        RETURNING id
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(ids)
}

// =============================================================================
// Delete Operations
// =============================================================================

/// Mark a book as user-edited.
/// This prevents background jobs (series_enrich, author_sync) from overwriting
/// user customizations with metadata from external providers.
pub async fn mark_user_edited<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!("UPDATE books SET user_edited_at = NOW() WHERE id = $1", id)
        .execute(exec)
        .await?;
    Ok(())
}

/// Delete a book by ID.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM books WHERE id = $1", id)
        .execute(exec)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete all books belonging to a media root, returning deleted IDs.
pub async fn delete_by_media_root<'e, E>(exec: E, media_root_id: Uuid) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!(
        "DELETE FROM books WHERE media_root_id = $1 RETURNING id",
        media_root_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(ids)
}

// =============================================================================
// Rejected Releases
// =============================================================================

/// Append a rejected release entry to a book's rejected_releases array.
pub async fn add_rejected_release<'e, E>(
    exec: E,
    id: Uuid,
    release_info: serde_json::Value,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE books SET rejected_releases = rejected_releases || $2::jsonb, updated_at = NOW() WHERE id = $1",
        id,
        serde_json::json!([release_info]),
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Clear all rejected releases for a book.
pub async fn clear_rejected_releases<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE books SET rejected_releases = '[]'::jsonb, updated_at = NOW() WHERE id = $1",
        id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Get rejected releases for a book.
pub async fn get_rejected_releases<'e, E>(exec: E, id: Uuid) -> Result<serde_json::Value>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!("SELECT rejected_releases FROM books WHERE id = $1", id,)
        .fetch_optional(exec)
        .await?;

    Ok(row.unwrap_or(serde_json::json!([])))
}
