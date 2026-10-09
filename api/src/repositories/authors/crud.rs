//! Author CRUD operations and book linking.

use super::{AuthorInput, RepositoryError, Result};
use crate::images::ImageMap;
use serde_json::Value;
use sqlx::{Executor, PgPool, Postgres};
use uuid::Uuid;

/// Get an existing author by normalized name, or create a new one with just the name.
///
/// This is the simplest form - use when you just need an author ID for linking.
/// Uses `normalized_name` (lowercase) for case-insensitive matching.
pub async fn get_or_create_by_name(pool: &PgPool, name: &str) -> Result<Uuid> {
    get_or_create(pool, name, None).await
}

/// Get an existing author by normalized name or hardcover_id, or create a new one.
///
/// Matching algorithm:
/// 1. If hardcover_id provided, try to find by hardcover_id (authoritative match)
/// 2. Find all authors with matching normalized_name
/// 3. If exactly one: backfill hardcover_id if needed, return it
/// 4. If multiple: prefer one without hardcover_id (for backfill), else oldest
/// 5. If none: create new author
pub async fn get_or_create(pool: &PgPool, name: &str, hardcover_id: Option<&str>) -> Result<Uuid> {
    let name = name.trim();
    if name.is_empty() {
        return Err(RepositoryError::Validation(
            "author name cannot be empty".to_string(),
        ));
    }

    let normalized = name.to_lowercase();
    let hc_id = hardcover_id.filter(|s| !s.is_empty());

    // Step 1: Try hardcover_id lookup (authoritative)
    if let Some(hc_id) = hc_id
        && let Some(row) = sqlx::query!(
            "SELECT id FROM authors WHERE identifiers->>'hardcover' = $1",
            hc_id
        )
        .fetch_optional(pool)
        .await?
    {
        return Ok(row.id);
    }

    // Step 2: Find candidates by normalized name
    let candidates: Vec<_> = sqlx::query!(
        r#"
        SELECT id, identifiers->>'hardcover' as hardcover_id
        FROM authors
        WHERE normalized_name = $1
        ORDER BY created_at ASC
        "#,
        normalized,
    )
    .fetch_all(pool)
    .await?;

    let candidate_pairs: Vec<(Uuid, Option<String>)> = candidates
        .iter()
        .map(|r| (r.id, r.hardcover_id.clone()))
        .collect();

    if let Some(chosen) = pick_best_candidate(&candidate_pairs, hc_id) {
        // Backfill hardcover_id if the chosen author doesn't have one
        if let Some(hc_id) = hc_id
            && candidate_pairs
                .iter()
                .find(|(id, _)| *id == chosen)
                .is_some_and(|(_, existing_hc)| existing_hc.is_none())
        {
            let backfill_ids = serde_json::json!({"hardcover": hc_id});
            let _ = sqlx::query!(
                r#"
                UPDATE authors SET
                    identifiers = identifiers || $2,
                    updated_at = NOW()
                WHERE id = $1
                "#,
                chosen,
                backfill_ids,
            )
            .execute(pool)
            .await;
        }
        return Ok(chosen);
    }

    // Step 5: No match - create new author
    let id = Uuid::now_v7();
    let identifiers = match hc_id {
        Some(hc_id) => serde_json::json!({"hardcover": hc_id}),
        None => serde_json::json!({}),
    };

    sqlx::query!(
        r#"
        INSERT INTO authors (id, name, normalized_name, identifiers)
        VALUES ($1, $2, $3, $4)
        "#,
        id,
        name,
        normalized,
        identifiers,
    )
    .execute(pool)
    .await?;

    Ok(id)
}

/// Pick the best candidate from a list of (id, hardcover_id) pairs.
///
/// When `hc_id` is provided, candidates with a *different* hardcover_id are excluded
/// (they belong to a different real-world entity). Among remaining candidates:
/// - Prefer one without a hardcover_id (can be backfilled)
/// - Otherwise return oldest (first in list, sorted by created_at ASC)
///
/// When `hc_id` is None, all candidates are eligible and the oldest is returned.
fn pick_best_candidate(candidates: &[(Uuid, Option<String>)], hc_id: Option<&str>) -> Option<Uuid> {
    if candidates.is_empty() {
        return None;
    }

    if let Some(hc_id) = hc_id {
        // Filter out candidates that have a *different* hardcover_id
        let eligible: Vec<_> = candidates
            .iter()
            .filter(|(_, existing_hc)| {
                match existing_hc {
                    Some(existing) => existing == hc_id, // Same ID is fine (shouldn't happen, caught in step 1)
                    None => true,                        // No ID = eligible for backfill
                }
            })
            .collect();

        // Prefer unlinked (no hardcover_id) for backfill, else oldest eligible
        eligible
            .iter()
            .find(|(_, existing_hc)| existing_hc.is_none())
            .or(eligible.first())
            .map(|(id, _)| *id)
    } else {
        // No hardcover_id constraint - return oldest
        Some(candidates[0].0)
    }
}

/// Upsert an author with full metadata.
///
/// Matching algorithm:
/// 1. If identifiers contain hardcover_id, try lookup by hardcover_id
/// 2. If found, update metadata on that author
/// 3. If not found, try name match (same as get_or_create)
/// 4. If name match found, update metadata
/// 5. If no match at all, create new author
pub async fn upsert(pool: &PgPool, input: AuthorInput) -> Result<Uuid> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(RepositoryError::Validation(
            "author name cannot be empty".to_string(),
        ));
    }

    let normalized = name.to_lowercase();
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
            "SELECT id FROM authors WHERE identifiers->>'hardcover' = $1",
            hc_id
        )
        .fetch_optional(pool)
        .await?
    } else {
        None
    };

    // Step 2: If not found by hardcover_id, try name match
    let existing_id = match existing_id {
        Some(id) => Some(id),
        None => {
            let candidates: Vec<_> = sqlx::query!(
                r#"
                SELECT id, identifiers->>'hardcover' as hardcover_id
                FROM authors
                WHERE normalized_name = $1
                ORDER BY created_at ASC
                "#,
                normalized,
            )
            .fetch_all(pool)
            .await?;
            let candidate_pairs: Vec<(Uuid, Option<String>)> = candidates
                .iter()
                .map(|r| (r.id, r.hardcover_id.clone()))
                .collect();
            pick_best_candidate(&candidate_pairs, hc_id.as_deref())
        }
    };

    let images_json = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();

    if let Some(existing_id) = existing_id {
        // Update existing author
        sqlx::query!(
            r#"
            UPDATE authors SET
                name = $2,
                normalized_name = $3,
                identifiers = identifiers || $4,
                images = images || $5,
                bio = COALESCE($6, bio),
                is_monitored = COALESCE($7, is_monitored),
                auto_want_mode = CASE
                    WHEN $8::text IS NOT NULL AND $8::text != 'none' THEN $8::text
                    ELSE auto_want_mode
                END,
                known_book_count = COALESCE($9, known_book_count),
                updated_at = NOW()
            WHERE id = $1
            "#,
            existing_id,
            name,
            normalized,
            identifiers,
            images_json,
            input.bio.as_deref(),
            input.is_monitored,
            input.auto_want_mode.as_deref(),
            input.known_book_count,
        )
        .execute(pool)
        .await?;

        Ok(existing_id)
    } else {
        // Create new author
        let id = Uuid::now_v7();
        sqlx::query!(
            r#"
            INSERT INTO authors (id, name, normalized_name, identifiers, images, bio,
                                 is_monitored, auto_want_mode, known_book_count)
            VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, FALSE), COALESCE($8, 'none'), $9)
            "#,
            id,
            name,
            normalized,
            identifiers,
            images_json,
            input.bio.as_deref(),
            input.is_monitored,
            input.auto_want_mode.as_deref(),
            input.known_book_count,
        )
        .execute(pool)
        .await?;

        Ok(id)
    }
}

/// Update metadata for an existing author by ID.
///
/// Only updates fields that are provided (non-None).
/// Identifiers are merged, not replaced.
pub async fn update_metadata<'e, E>(exec: E, id: Uuid, input: AuthorInput) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let name = input.name.trim();
    if name.is_empty() {
        return Err(RepositoryError::Validation(
            "author name cannot be empty".to_string(),
        ));
    }

    let normalized = name.to_lowercase();
    let identifiers = input.identifiers.unwrap_or_else(|| serde_json::json!({}));
    let images_json = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();

    sqlx::query!(
        r#"
        UPDATE authors SET
            name = $2,
            normalized_name = $3,
            identifiers = authors.identifiers || $4,
            images = authors.images || $5,
            bio = COALESCE($6, authors.bio),
            is_monitored = COALESCE($7, authors.is_monitored),
            auto_want_mode = CASE
                WHEN $8::text IS NOT NULL AND $8::text != 'none' THEN $8::text
                ELSE authors.auto_want_mode
            END,
            known_book_count = COALESCE($9, authors.known_book_count),
            updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        name,
        normalized,
        identifiers,
        images_json,
        input.bio.as_deref(),
        input.is_monitored,
        input.auto_want_mode.as_deref(),
        input.known_book_count,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Delete orphaned authors (no book links, not monitored).
/// Returns IDs of deleted authors for search index cleanup.
pub async fn delete_orphaned<'e, E>(exec: E) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!(
        r#"
        DELETE FROM authors
        WHERE is_monitored = false
          AND NOT EXISTS (SELECT 1 FROM book_authors WHERE author_id = authors.id)
        RETURNING id
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(ids)
}

/// Link a book to an author.
///
/// Does nothing if the link already exists.
pub async fn link_book<'e, E>(exec: E, book_id: Uuid, author_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "INSERT INTO book_authors (book_id, author_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        book_id,
        author_id,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Clear all authors for a book.
pub async fn clear_book_authors<'e, E>(exec: E, book_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!("DELETE FROM book_authors WHERE book_id = $1", book_id)
        .execute(exec)
        .await?;

    Ok(())
}

/// Rename an author.
///
/// Returns true if the author was found and renamed.
pub async fn rename<'e, E>(exec: E, id: Uuid, name: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let name = name.trim();
    if name.is_empty() {
        return Err(RepositoryError::Validation(
            "author name cannot be empty".to_string(),
        ));
    }

    let normalized = name.to_lowercase();

    let result = sqlx::query!(
        "UPDATE authors SET name = $2, normalized_name = $3, updated_at = NOW() WHERE id = $1",
        id,
        name,
        normalized,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete an author by ID.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!("DELETE FROM authors WHERE id = $1", id)
        .execute(exec)
        .await?;

    Ok(())
}

/// Remove all book links for an author (delete book_authors entries).
pub async fn unlink_all_books<'e, E>(exec: E, author_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!("DELETE FROM book_authors WHERE author_id = $1", author_id)
        .execute(exec)
        .await?;

    Ok(())
}

/// Copy book links from one author to another (for merging).
pub async fn copy_book_links<'e, E>(exec: E, source_id: Uuid, target_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "INSERT INTO book_authors (book_id, author_id)
         SELECT book_id, $1 FROM book_authors WHERE author_id = $2
         ON CONFLICT DO NOTHING",
        target_id,
        source_id,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Update monitoring settings for an author.
///
/// Returns true if the author was found and updated.
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
        "UPDATE authors SET {}, updated_at = NOW(){} WHERE id = $1",
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

/// Apply metadata from a provider (merge identifiers, merge images, update bio).
///
/// Returns true if the author was found and updated.
pub async fn apply_metadata<'e, E>(
    exec: E,
    id: Uuid,
    identifiers: Value,
    images: Option<ImageMap>,
    bio: Option<&str>,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let images_json = serde_json::to_value(images.unwrap_or_default()).unwrap_or_default();
    let result = sqlx::query!(
        r#"
        UPDATE authors SET
            identifiers = identifiers || $2,
            images = images || $3,
            bio = COALESCE($4, bio),
            updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        identifiers,
        images_json,
        bio,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Merge identifiers into an existing author (e.g. adding hardcover_slug).
pub async fn merge_identifiers(pool: &PgPool, author_id: Uuid, identifiers: &Value) {
    if identifiers.is_null() || identifiers.as_object().is_some_and(|o| o.is_empty()) {
        return;
    }
    let _ = sqlx::query!(
        "UPDATE authors SET identifiers = identifiers || $2, updated_at = NOW() WHERE id = $1",
        author_id,
        identifiers,
    )
    .execute(pool)
    .await;
}

/// Input for refreshing author metadata from a provider.
#[derive(Debug, Default)]
pub struct AuthorRefreshInput {
    pub name: String,
    pub images: Option<ImageMap>,
    pub bio: Option<String>,
    pub known_book_count: Option<i32>,
}

/// Refresh metadata from a provider (updates name, images, bio, known_book_count, refresh timestamps).
pub async fn refresh_metadata<'e, E>(exec: E, id: Uuid, input: AuthorRefreshInput) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let images_json = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();
    sqlx::query!(
        r#"
        UPDATE authors SET
            name = $2,
            images = images || $3,
            bio = COALESCE($4, bio),
            known_book_count = $5,
            last_metadata_refresh_at = NOW(),
            next_metadata_refresh_at = NOW() + (metadata_refresh_interval_hours || ' hours')::INTERVAL,
            updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        input.name,
        images_json,
        input.bio.as_deref(),
        input.known_book_count,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Set next_metadata_refresh_at to prevent re-processing before the actual refresh completes.
/// Called at the start of sync to avoid duplicate work if the job is slow or fails.
pub async fn bump_next_refresh<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE authors SET
            next_metadata_refresh_at = NOW() + (metadata_refresh_interval_hours || ' hours')::INTERVAL
        WHERE id = $1
        "#,
        id,
    )
    .execute(exec)
    .await?;

    Ok(())
}
