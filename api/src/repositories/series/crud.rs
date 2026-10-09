//! Series CRUD operations.

use super::{RepositoryError, Result, SeriesInput};
use crate::images::ImageMap;
use serde_json::Value;
use sqlx::{Executor, PgPool, Postgres, Row};
use uuid::Uuid;

/// Get an existing series by sort_key, or create a new one with just the name.
///
/// This is the simplest form - use when you just need a series ID for linking.
/// Returns the oldest matching series if multiple exist, or creates a new one.
pub async fn get_or_create_by_name(pool: &PgPool, name: &str) -> Result<Uuid> {
    let name = name.trim();
    if name.is_empty() {
        return Err(RepositoryError::Validation(
            "series name cannot be empty".to_string(),
        ));
    }

    let sort_key = name.to_lowercase();

    // Find existing by sort_key (oldest first)
    if let Some(id) = sqlx::query_scalar!(
        "SELECT id FROM series WHERE sort_key = $1 ORDER BY created_at ASC LIMIT 1",
        sort_key,
    )
    .fetch_optional(pool)
    .await?
    {
        return Ok(id);
    }

    // Create new series
    let id = Uuid::now_v7();
    sqlx::query!(
        r#"
        INSERT INTO series (id, name, sort_key)
        VALUES ($1, $2, $3)
        "#,
        id,
        name,
        sort_key,
    )
    .execute(pool)
    .await?;

    Ok(id)
}

/// Get an existing series by name and author overlap, or create a new one.
///
/// Matching algorithm:
/// 1. If identifiers contain hardcover_id, try lookup by hardcover_id (authoritative)
/// 2. Find candidates by sort_key (name), pick best by author overlap
/// 3. If no suitable match, create new series
///
/// If identifiers are provided and a match is found, identifiers will be merged.
pub async fn get_or_create_with_author_overlap(
    pool: &PgPool,
    name: &str,
    author_ids: &[Uuid],
    identifiers: Option<Value>,
) -> Result<Option<Uuid>> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }

    let sort_key = name.to_lowercase();

    // Extract hardcover_id from identifiers if present
    let hc_id = identifiers
        .as_ref()
        .and_then(|ids| ids.get("hardcover"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());

    // Step 1: Try hardcover_id lookup (authoritative)
    if let Some(hc_id) = hc_id
        && let Some(series_id) = sqlx::query_scalar!(
            "SELECT id FROM series WHERE identifiers->>'hardcover' = $1",
            hc_id,
        )
        .fetch_optional(pool)
        .await?
    {
        merge_identifiers(pool, series_id, &identifiers).await;
        return Ok(Some(series_id));
    }

    // Step 2: Find candidates by sort_key
    let candidates = sqlx::query!(
        "SELECT id FROM series WHERE sort_key = $1 ORDER BY created_at ASC",
        sort_key,
    )
    .fetch_all(pool)
    .await?;

    // If candidates exist but no author_ids provided, return the oldest
    if !candidates.is_empty() && author_ids.is_empty() {
        let series_id = candidates[0].id;
        merge_identifiers(pool, series_id, &identifiers).await;
        return Ok(Some(series_id));
    }

    // If candidates exist and author_ids provided, try to find one with overlap
    if !candidates.is_empty()
        && !author_ids.is_empty()
        && let Ok(Some(row)) = sqlx::query(
            r#"
            SELECT s.id, COUNT(DISTINCT ba.author_id) AS overlap
            FROM series s
            JOIN book_series bks ON bks.series_id = s.id
            JOIN books b ON b.id = bks.book_id
            JOIN book_authors ba ON ba.book_id = b.id
            WHERE s.sort_key = $1 AND ba.author_id = ANY($2)
            GROUP BY s.id
            ORDER BY overlap DESC, s.created_at ASC
            LIMIT 1
            "#,
        )
        .bind(&sort_key)
        .bind(author_ids)
        .fetch_optional(pool)
        .await
    {
        let overlap: i64 = row.get("overlap");
        if overlap > 0 {
            let series_id = row.get::<Uuid, _>("id");
            merge_identifiers(pool, series_id, &identifiers).await;
            return Ok(Some(series_id));
        }
    }

    // Step 3: No suitable match - create new series
    let id = Uuid::now_v7();
    let identifiers_value = identifiers.unwrap_or_else(|| serde_json::json!({}));
    sqlx::query!(
        r#"
        INSERT INTO series (id, name, sort_key, identifiers)
        VALUES ($1, $2, $3, $4)
        "#,
        id,
        name,
        sort_key,
        identifiers_value,
    )
    .execute(pool)
    .await?;

    Ok(Some(id))
}

/// Upsert a series with full metadata.
///
/// Matching algorithm:
/// 1. If identifiers contain hardcover_id, try lookup by hardcover_id
/// 2. If found, update metadata on that series
/// 3. If not found, try name match (oldest by sort_key)
/// 4. If name match found, update metadata
/// 5. If no match at all, create new series
pub async fn upsert(pool: &PgPool, input: SeriesInput) -> Result<Uuid> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(RepositoryError::Validation(
            "series name cannot be empty".to_string(),
        ));
    }

    let sort_key = name.to_lowercase();
    let identifiers = input.identifiers.unwrap_or_else(|| serde_json::json!({}));

    // Extract hardcover_id from identifiers
    let hc_id = identifiers
        .get("hardcover")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    // Step 1: Try hardcover_id lookup
    let existing_id = if let Some(ref hc_id) = hc_id {
        sqlx::query_scalar!(
            "SELECT id FROM series WHERE identifiers->>'hardcover' = $1",
            hc_id
        )
        .fetch_optional(pool)
        .await?
    } else {
        None
    };

    // Step 2: If not found by hardcover_id, try name match (oldest)
    let existing_id = match existing_id {
        Some(id) => Some(id),
        None => {
            sqlx::query_scalar!(
                "SELECT id FROM series WHERE sort_key = $1 ORDER BY created_at ASC LIMIT 1",
                sort_key,
            )
            .fetch_optional(pool)
            .await?
        }
    };

    let images_json = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();

    if let Some(existing_id) = existing_id {
        // Update existing series
        sqlx::query!(
            r#"
            UPDATE series SET
                name = $2,
                sort_key = $3,
                identifiers = identifiers || $4,
                images = images || $5,
                description = COALESCE($6, description),
                total_book_count = COALESCE($7, total_book_count),
                is_monitored = COALESCE($8, is_monitored),
                auto_want_mode = CASE
                    WHEN $9::text IS NOT NULL AND $9::text != 'none' THEN $9::text
                    ELSE auto_want_mode
                END,
                updated_at = NOW()
            WHERE id = $1
            "#,
            existing_id,
            name,
            sort_key,
            identifiers,
            images_json,
            input.description.as_deref(),
            input.total_book_count,
            input.is_monitored,
            input.auto_want_mode.as_deref(),
        )
        .execute(pool)
        .await?;

        Ok(existing_id)
    } else {
        // Create new series
        let id = Uuid::now_v7();
        sqlx::query!(
            r#"
            INSERT INTO series (id, name, sort_key, identifiers, images, description,
                                total_book_count, is_monitored, auto_want_mode)
            VALUES ($1, $2, $3, $4, $5, $6, $7, COALESCE($8, FALSE), COALESCE($9, 'none'))
            "#,
            id,
            name,
            sort_key,
            identifiers,
            images_json,
            input.description.as_deref(),
            input.total_book_count,
            input.is_monitored,
            input.auto_want_mode.as_deref(),
        )
        .execute(pool)
        .await?;

        Ok(id)
    }
}

/// Merge identifiers into an existing series.
pub async fn merge_identifiers(pool: &PgPool, series_id: Uuid, identifiers: &Option<Value>) {
    if let Some(ids) = identifiers
        && !ids.is_null()
        && ids.as_object().is_some_and(|o| !o.is_empty())
    {
        let _ = sqlx::query!(
            "UPDATE series SET identifiers = identifiers || $2, updated_at = NOW() WHERE id = $1",
            series_id,
            ids,
        )
        .execute(pool)
        .await;
    }
}

/// Delete orphaned series (no books referencing them).
/// Returns IDs of deleted series for search index cleanup.
pub async fn delete_orphaned<'e, E>(exec: E) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!(
        r#"
        DELETE FROM series
        WHERE is_monitored = false
          AND NOT EXISTS (SELECT 1 FROM book_series WHERE series_id = series.id)
        RETURNING id
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(ids)
}

/// Update a book's primary series assignment via the book_series junction table.
///
/// Delegates to `book_series::add()` for set, `book_series::clear_for_book()` for clear.
pub async fn set_book_series(
    pool: &sqlx::PgPool,
    book_id: Uuid,
    series_id: Option<Uuid>,
    series_index: Option<f64>,
) -> Result<()> {
    if let Some(sid) = series_id {
        super::super::book_series::add(pool, book_id, sid, series_index, true).await?;
    } else {
        super::super::book_series::clear_for_book(pool, book_id).await?;
    }
    Ok(())
}

/// Rename a series.
///
/// Returns true if the series was found and renamed.
pub async fn rename<'e, E>(exec: E, id: Uuid, name: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let name = name.trim();
    if name.is_empty() {
        return Err(RepositoryError::Validation(
            "series name cannot be empty".to_string(),
        ));
    }

    let sort_key = name.to_lowercase();

    let result = sqlx::query!(
        "UPDATE series SET name = $2, sort_key = $3, updated_at = NOW() WHERE id = $1",
        id,
        name,
        sort_key,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete a series by ID.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!("DELETE FROM series WHERE id = $1", id)
        .execute(exec)
        .await?;

    Ok(())
}

/// Update monitoring settings for a series.
///
/// Returns true if the series was found and updated.
pub async fn update_monitoring<'e, E>(
    exec: E,
    id: Uuid,
    is_monitored: Option<bool>,
    auto_want_mode: Option<&str>,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    // Build dynamic query
    let mut updates = Vec::new();
    let mut param_idx = 2;

    if is_monitored.is_some() {
        updates.push(format!("is_monitored = ${}", param_idx));
        param_idx += 1;
    }
    if auto_want_mode.is_some() {
        updates.push(format!("auto_want_mode = ${}", param_idx));
    }

    if updates.is_empty() {
        return Ok(true); // Nothing to update
    }

    // If enabling monitoring, set monitoring_started_at
    let monitoring_started_update = if is_monitored == Some(true) {
        ", monitoring_started_at = COALESCE(monitoring_started_at, NOW())"
    } else {
        ""
    };

    let query_str = format!(
        "UPDATE series SET {}, updated_at = NOW(){} WHERE id = $1",
        updates.join(", "),
        monitoring_started_update
    );

    let mut query = sqlx::query(sqlx::AssertSqlSafe(query_str.as_str())).bind(id);

    if let Some(monitored) = is_monitored {
        query = query.bind(monitored);
    }
    if let Some(mode) = auto_want_mode {
        query = query.bind(mode);
    }

    let result = query.execute(exec).await?;
    Ok(result.rows_affected() > 0)
}

/// Apply metadata from a provider (merge identifiers, merge images, update description/total_book_count).
///
/// Returns true if the series was found and updated.
pub async fn apply_metadata<'e, E>(
    exec: E,
    id: Uuid,
    identifiers: Value,
    images: Option<ImageMap>,
    description: Option<&str>,
    total_book_count: Option<i32>,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let images_json = serde_json::to_value(images.unwrap_or_default()).unwrap_or_default();
    let result = sqlx::query!(
        r#"
        UPDATE series SET
            identifiers = identifiers || $2,
            images = images || $3,
            description = COALESCE($4, description),
            total_book_count = COALESCE($5, total_book_count),
            updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        identifiers,
        images_json,
        description,
        total_book_count,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Input for refreshing series metadata from a provider.
#[derive(Debug, Default)]
pub struct SeriesRefreshInput {
    pub description: Option<String>,
    pub total_book_count: Option<i32>,
    pub images: Option<ImageMap>,
}

/// Refresh metadata from a provider (updates description, total_book_count, images, refresh timestamps).
pub async fn refresh_metadata<'e, E>(exec: E, id: Uuid, input: SeriesRefreshInput) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let images_json = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();
    sqlx::query!(
        r#"
        UPDATE series SET
            description = COALESCE($2, description),
            total_book_count = $3,
            images = images || $4,
            last_metadata_refresh_at = NOW(),
            next_metadata_refresh_at = NOW() + (metadata_refresh_interval_hours || ' hours')::INTERVAL,
            updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        input.description.as_deref(),
        input.total_book_count,
        images_json,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Set next_metadata_refresh_at to prevent re-processing before the actual refresh completes.
pub async fn bump_next_refresh<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE series SET
            next_metadata_refresh_at = NOW() + (metadata_refresh_interval_hours || ' hours')::INTERVAL
        WHERE id = $1
        "#,
        id,
    )
    .execute(exec)
    .await?;

    Ok(())
}
