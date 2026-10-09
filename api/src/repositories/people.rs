//! People repository operations (actors, directors, crew).

use super::Result;
use crate::images::ImageMap;
use serde_json::Value;
use sqlx::{Executor, PgPool, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Input for creating or updating a person.
#[derive(Debug, Default)]
pub struct PersonInput {
    pub name: String,
    pub sort_name: Option<String>,
    pub identifiers: Option<Value>,
    pub images: Option<ImageMap>,
    pub bio: Option<String>,
    pub birth_date: Option<time::Date>,
    pub death_date: Option<time::Date>,
}

/// Row returned from people queries.
#[derive(Debug, Clone)]
pub struct PersonRow {
    pub id: Uuid,
    pub name: String,
    pub sort_name: Option<String>,
    pub identifiers: Value,
    pub images: ImageMap,
    pub bio: Option<String>,
    pub birth_date: Option<time::Date>,
    pub death_date: Option<time::Date>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub is_monitored: bool,
    pub auto_want_mode: String,
    pub monitoring_started_at: Option<OffsetDateTime>,
    pub last_metadata_refresh_at: Option<OffsetDateTime>,
    pub next_metadata_refresh_at: Option<OffsetDateTime>,
    pub metadata_refresh_interval_hours: i32,
}

/// A dismissed credit entry for a person.
#[derive(Debug, Clone)]
pub struct DismissedCredit {
    pub tmdb_id: i32,
    pub media_type: String,
}

#[allow(clippy::too_many_arguments)]
fn row_to_person(
    id: Uuid,
    name: String,
    sort_name: Option<String>,
    identifiers: Value,
    images_json: Value,
    bio: Option<String>,
    birth_date: Option<time::Date>,
    death_date: Option<time::Date>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    is_monitored: bool,
    auto_want_mode: String,
    monitoring_started_at: Option<OffsetDateTime>,
    last_metadata_refresh_at: Option<OffsetDateTime>,
    next_metadata_refresh_at: Option<OffsetDateTime>,
    metadata_refresh_interval_hours: i32,
) -> PersonRow {
    let images: ImageMap = serde_json::from_value(images_json).unwrap_or_default();
    PersonRow {
        id,
        name,
        sort_name,
        identifiers,
        images,
        bio,
        birth_date,
        death_date,
        created_at,
        updated_at,
        is_monitored,
        auto_want_mode,
        monitoring_started_at,
        last_metadata_refresh_at,
        next_metadata_refresh_at,
        metadata_refresh_interval_hours,
    }
}

// =============================================================================
// CRUD
// =============================================================================

/// Create a new person.
pub async fn create<'e, E>(exec: E, input: PersonInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let identifiers = input.identifiers.unwrap_or_else(|| serde_json::json!({}));
    let images = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();

    let created_id: Uuid = sqlx::query_scalar!(
        r#"
        INSERT INTO people (id, name, sort_name, identifiers, images, bio, birth_date, death_date)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING id
        "#,
        id,
        input.name,
        input.sort_name as Option<String>,
        identifiers,
        images,
        input.bio as Option<String>,
        input.birth_date,
        input.death_date,
    )
    .fetch_one(exec)
    .await?;

    Ok(created_id)
}

/// Get a person by ID.
pub async fn get<'e, E>(exec: E, id: Uuid) -> Result<Option<PersonRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, name, sort_name, identifiers, images,
               bio, birth_date, death_date, created_at, updated_at,
               is_monitored, auto_want_mode, monitoring_started_at,
               last_metadata_refresh_at, next_metadata_refresh_at,
               metadata_refresh_interval_hours
        FROM people
        WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| {
        row_to_person(
            r.id,
            r.name,
            r.sort_name,
            r.identifiers,
            r.images,
            r.bio,
            r.birth_date,
            r.death_date,
            r.created_at,
            r.updated_at,
            r.is_monitored,
            r.auto_want_mode,
            r.monitoring_started_at,
            r.last_metadata_refresh_at,
            r.next_metadata_refresh_at,
            r.metadata_refresh_interval_hours,
        )
    }))
}

/// Upsert a person by a provider-specific identifier.
///
/// Tries atomic `ON CONFLICT` first; falls back to advisory-locked
/// SELECT + INSERT/UPDATE for providers without a unique index.
pub async fn upsert_by_provider(
    pool: &PgPool,
    provider_type: &str,
    provider_id: &str,
    input: PersonInput,
) -> Result<Uuid> {
    let id_key = format!("{provider_type}_id");
    let mut identifiers = input.identifiers.unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = identifiers.as_object_mut() {
        obj.insert(id_key.clone(), Value::String(provider_id.to_string()));
    }
    let images = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();

    // Try atomic ON CONFLICT first
    let id = Uuid::now_v7();
    let upsert_sql = format!(
        r#"
        INSERT INTO people (id, name, sort_name, identifiers, images, bio, birth_date, death_date)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT ((identifiers->>'{id_key}'))
        DO UPDATE SET
            name = EXCLUDED.name,
            sort_name = COALESCE(EXCLUDED.sort_name, people.sort_name),
            identifiers = people.identifiers || EXCLUDED.identifiers,
            images = people.images || EXCLUDED.images,
            bio = COALESCE(EXCLUDED.bio, people.bio),
            birth_date = COALESCE(EXCLUDED.birth_date, people.birth_date),
            death_date = COALESCE(EXCLUDED.death_date, people.death_date),
            updated_at = NOW()
        RETURNING id
        "#
    );

    let result = sqlx::query_as::<_, (Uuid,)>(sqlx::AssertSqlSafe(upsert_sql.as_str()))
        .bind(id)
        .bind(&input.name)
        .bind(input.sort_name.as_deref())
        .bind(&identifiers)
        .bind(&images)
        .bind(input.bio.as_deref())
        .bind(input.birth_date)
        .bind(input.death_date)
        .fetch_one(pool)
        .await;

    match result {
        Ok((row_id,)) => return Ok(row_id),
        Err(ref e) if super::is_no_unique_constraint_error(e) => {}
        Err(e) => return Err(e.into()),
    }

    // Fallback: advisory-locked SELECT + INSERT/UPDATE
    let lock_key = super::advisory_lock_key("people", &id_key, provider_id);
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(lock_key)
        .execute(&mut *tx)
        .await?;

    let existing =
        super::resolve_provider_row(&mut tx, "people", &id_key, provider_id, &identifiers).await?;

    let result_id = if let Some(existing_id) = existing {
        sqlx::query(
            r#"
            UPDATE people SET
                name = $2,
                sort_name = COALESCE($3, sort_name),
                identifiers = identifiers || $4,
                images = images || $5,
                bio = COALESCE($6, bio),
                birth_date = COALESCE($7, birth_date),
                death_date = COALESCE($8, death_date),
                updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(existing_id)
        .bind(&input.name)
        .bind(input.sort_name.as_deref())
        .bind(&identifiers)
        .bind(&images)
        .bind(input.bio.as_deref())
        .bind(input.birth_date)
        .bind(input.death_date)
        .execute(&mut *tx)
        .await?;
        existing_id
    } else {
        let new_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO people (id, name, sort_name, identifiers, images, bio, birth_date, death_date)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(new_id)
        .bind(&input.name)
        .bind(input.sort_name.as_deref())
        .bind(&identifiers)
        .bind(&images)
        .bind(input.bio.as_deref())
        .bind(input.birth_date)
        .bind(input.death_date)
        .execute(&mut *tx)
        .await?;
        new_id
    };

    tx.commit().await?;
    Ok(result_id)
}

/// Input for updating a person. All fields optional.
#[derive(Debug, Default)]
pub struct PersonUpdate {
    pub name: Option<String>,
    pub sort_name: Option<String>,
    pub identifiers: Option<Value>,
    pub images: Option<ImageMap>,
    pub bio: Option<String>,
    pub birth_date: Option<time::Date>,
    pub death_date: Option<time::Date>,
}

/// Update a person by ID. Only non-None fields are updated.
/// Identifiers are merged (not replaced) if provided.
pub async fn update<'e, E>(exec: E, id: Uuid, input: PersonUpdate) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let identifiers = input.identifiers.unwrap_or_else(|| serde_json::json!({}));
    let images = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();

    let result = sqlx::query!(
        r#"
        UPDATE people SET
            name = COALESCE($2, name),
            sort_name = COALESCE($3, sort_name),
            identifiers = identifiers || $4,
            images = images || $5,
            bio = COALESCE($6, bio),
            birth_date = COALESCE($7, birth_date),
            death_date = COALESCE($8, death_date),
            updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        input.name as Option<String>,
        input.sort_name as Option<String>,
        identifiers,
        images,
        input.bio as Option<String>,
        input.birth_date,
        input.death_date,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Update monitoring settings for a person.
/// Pass None to leave a field unchanged.
pub async fn update_monitoring<'e, E>(
    exec: E,
    id: Uuid,
    is_monitored: Option<bool>,
    auto_want_mode: Option<&str>,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE people SET
            is_monitored = COALESCE($2, is_monitored),
            auto_want_mode = COALESCE($3, auto_want_mode),
            monitoring_started_at = CASE
                WHEN $2 = true THEN COALESCE(monitoring_started_at, NOW())
                WHEN $2 = false THEN NULL
                ELSE monitoring_started_at
            END,
            updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        is_monitored,
        auto_want_mode,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete a person by ID.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM people WHERE id = $1", id)
        .execute(exec)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete orphaned people (no credits in any cast/crew table, not monitored).
/// Returns IDs of deleted people for search index cleanup.
pub async fn delete_orphaned<'e, E>(exec: E) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!(
        r#"
        DELETE FROM people
        WHERE is_monitored = false
          AND NOT EXISTS (SELECT 1 FROM movie_cast WHERE person_id = people.id)
          AND NOT EXISTS (SELECT 1 FROM movie_crew WHERE person_id = people.id)
          AND NOT EXISTS (SELECT 1 FROM tv_cast WHERE person_id = people.id)
          AND NOT EXISTS (SELECT 1 FROM tv_crew WHERE person_id = people.id)
        RETURNING id
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(ids)
}

/// List people with pagination.
pub async fn list<'e, E>(exec: E, limit: i64, offset: i64) -> Result<Vec<PersonRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, name, sort_name, identifiers, images,
               bio, birth_date, death_date, created_at, updated_at,
               is_monitored, auto_want_mode, monitoring_started_at,
               last_metadata_refresh_at, next_metadata_refresh_at,
               metadata_refresh_interval_hours
        FROM people
        ORDER BY COALESCE(sort_name, name) ASC
        LIMIT $1 OFFSET $2
        "#,
        limit,
        offset,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            row_to_person(
                r.id,
                r.name,
                r.sort_name,
                r.identifiers,
                r.images,
                r.bio,
                r.birth_date,
                r.death_date,
                r.created_at,
                r.updated_at,
                r.is_monitored,
                r.auto_want_mode,
                r.monitoring_started_at,
                r.last_metadata_refresh_at,
                r.next_metadata_refresh_at,
                r.metadata_refresh_interval_hours,
            )
        })
        .collect())
}

/// Get people by a list of IDs (preserving order).
pub async fn get_by_ids<'e, E>(exec: E, ids: &[Uuid]) -> Result<Vec<PersonRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    if ids.is_empty() {
        return Ok(vec![]);
    }

    let rows = sqlx::query!(
        r#"
        SELECT id, name, sort_name, identifiers, images,
               bio, birth_date, death_date, created_at, updated_at,
               is_monitored, auto_want_mode, monitoring_started_at,
               last_metadata_refresh_at, next_metadata_refresh_at,
               metadata_refresh_interval_hours
        FROM people
        WHERE id = ANY($1)
        "#,
        ids,
    )
    .fetch_all(exec)
    .await?;

    // Preserve the order from the IDs list
    let map: std::collections::HashMap<Uuid, PersonRow> = rows
        .into_iter()
        .map(|r| {
            let p = row_to_person(
                r.id,
                r.name,
                r.sort_name,
                r.identifiers,
                r.images,
                r.bio,
                r.birth_date,
                r.death_date,
                r.created_at,
                r.updated_at,
                r.is_monitored,
                r.auto_want_mode,
                r.monitoring_started_at,
                r.last_metadata_refresh_at,
                r.next_metadata_refresh_at,
                r.metadata_refresh_interval_hours,
            );
            (p.id, p)
        })
        .collect();

    Ok(ids.iter().filter_map(|id| map.get(id).cloned()).collect())
}

/// Search people by name (ILIKE fallback).
pub async fn search<'e, E>(exec: E, query: &str, limit: i64) -> Result<Vec<PersonRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let pattern = format!("%{}%", query);
    let rows = sqlx::query!(
        r#"
        SELECT id, name, sort_name, identifiers, images,
               bio, birth_date, death_date, created_at, updated_at,
               is_monitored, auto_want_mode, monitoring_started_at,
               last_metadata_refresh_at, next_metadata_refresh_at,
               metadata_refresh_interval_hours
        FROM people
        WHERE name ILIKE $1 OR sort_name ILIKE $1
        ORDER BY COALESCE(sort_name, name) ASC
        LIMIT $2
        "#,
        pattern,
        limit,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            row_to_person(
                r.id,
                r.name,
                r.sort_name,
                r.identifiers,
                r.images,
                r.bio,
                r.birth_date,
                r.death_date,
                r.created_at,
                r.updated_at,
                r.is_monitored,
                r.auto_want_mode,
                r.monitoring_started_at,
                r.last_metadata_refresh_at,
                r.next_metadata_refresh_at,
                r.metadata_refresh_interval_hours,
            )
        })
        .collect())
}

/// Filter for cursor-based people listing.
pub struct PersonListFilter {
    pub cursor: Option<(String, Uuid)>, // (sort_key, id)
    pub direction: super::PaginationDirection,
    pub search: Option<String>,
    pub search_ids: Option<Vec<Uuid>>,
    pub limit: i64,
}

/// List people with cursor-based pagination.
pub async fn list_with_cursor(
    pool: &sqlx::PgPool,
    filter: &PersonListFilter,
) -> Result<Vec<PersonRow>> {
    use sqlx::Row;

    let sort_col = "COALESCE(sort_name, name)";
    let is_forward = matches!(filter.direction, super::PaginationDirection::Forward);

    let mut conditions = Vec::new();
    let mut binds_str: Vec<String> = Vec::new();
    let mut binds_uuid: Vec<Uuid> = Vec::new();

    // Cursor condition
    if let Some((ref cursor_name, ref cursor_id)) = filter.cursor {
        let op = if is_forward { ">" } else { "<" };
        conditions.push(format!(
            "({sort_col} {op} $STR{idx} OR ({sort_col} = $STR{idx} AND id {op} $UUID{idx2}))",
            idx = binds_str.len(),
            idx2 = binds_uuid.len(),
        ));
        binds_str.push(cursor_name.clone());
        binds_uuid.push(*cursor_id);
    }

    // Search (ILIKE fallback)
    if let Some(ref q) = filter.search {
        let pattern = format!("%{q}%");
        conditions.push(format!(
            "(name ILIKE $STR{idx} OR sort_name ILIKE $STR{idx})",
            idx = binds_str.len(),
        ));
        binds_str.push(pattern);
    }

    // Search IDs (from MeiliSearch)
    if let Some(ref ids) = filter.search_ids {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        // For search IDs, return in the order MeiliSearch gave us
        let placeholders: Vec<String> = ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("$UUIDS{i}"))
            .collect();
        conditions.push(format!("id IN ({})", placeholders.join(", ")));
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let order = if filter.search_ids.is_some() {
        // Preserve MeiliSearch order — no ORDER BY needed, but we still need deterministic results
        String::new()
    } else {
        let dir = if is_forward { "ASC" } else { "DESC" };
        format!("ORDER BY {sort_col} {dir}, id {dir}")
    };

    let sql = format!(
        "SELECT id, name, sort_name, identifiers, images,
                bio, birth_date, death_date, created_at, updated_at,
                is_monitored, auto_want_mode, monitoring_started_at,
                last_metadata_refresh_at, next_metadata_refresh_at,
                metadata_refresh_interval_hours
         FROM people
         {where_clause}
         {order}
         LIMIT {limit}",
        limit = filter.limit,
    );

    // Build the query with proper parameter binding
    // We need to number params sequentially ($1, $2, ...)
    // Rewrite placeholders
    let mut final_sql = sql.clone();
    let mut param_idx = 1u32;

    // Replace $STR{n} placeholders
    for i in 0..binds_str.len() {
        let placeholder = format!("$STR{i}");
        final_sql = final_sql.replace(&placeholder, &format!("${param_idx}"));
        param_idx += 1;
    }
    // Replace $UUID{n} placeholders
    for i in 0..binds_uuid.len() {
        let placeholder = format!("$UUID{i}");
        final_sql = final_sql.replace(&placeholder, &format!("${param_idx}"));
        param_idx += 1;
    }
    // Replace $UUIDS{n} placeholders for search IDs
    if let Some(ref ids) = filter.search_ids {
        for i in 0..ids.len() {
            let placeholder = format!("$UUIDS{i}");
            final_sql = final_sql.replace(&placeholder, &format!("${param_idx}"));
            param_idx += 1;
        }
    }

    let mut q = sqlx::query(sqlx::AssertSqlSafe(final_sql.as_str()));
    for s in &binds_str {
        q = q.bind(s);
    }
    for u in &binds_uuid {
        q = q.bind(u);
    }
    if let Some(ref ids) = filter.search_ids {
        for id in ids {
            q = q.bind(id);
        }
    }

    let rows = q.fetch_all(pool).await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let images_json: serde_json::Value = r.get("images");
            row_to_person(
                r.get("id"),
                r.get("name"),
                r.get("sort_name"),
                r.get("identifiers"),
                images_json,
                r.get("bio"),
                r.get("birth_date"),
                r.get("death_date"),
                r.get("created_at"),
                r.get("updated_at"),
                r.get("is_monitored"),
                r.get("auto_want_mode"),
                r.get("monitoring_started_at"),
                r.get("last_metadata_refresh_at"),
                r.get("next_metadata_refresh_at"),
                r.get("metadata_refresh_interval_hours"),
            )
        })
        .collect())
}

/// Count total people (optionally filtered by search).
pub async fn count(pool: &sqlx::PgPool, search: Option<&str>) -> Result<i64> {
    if let Some(q) = search {
        let pattern = format!("%{q}%");
        let row = sqlx::query_scalar!(
            r#"SELECT COUNT(*) as "count!" FROM people WHERE name ILIKE $1 OR sort_name ILIKE $1"#,
            pattern,
        )
        .fetch_one(pool)
        .await?;
        Ok(row)
    } else {
        let row = sqlx::query_scalar!(r#"SELECT COUNT(*) as "count!" FROM people"#)
            .fetch_one(pool)
            .await?;
        Ok(row)
    }
}

/// Get a person's sort key (for cursor-based pagination).
pub async fn get_sort_name(pool: &sqlx::PgPool, id: Uuid) -> Result<Option<String>> {
    let row = sqlx::query_scalar!(
        r#"SELECT COALESCE(sort_name, name) as "sort_key!" FROM people WHERE id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// List monitored people whose metadata refresh is due.
pub async fn list_due_for_sync<'e, E>(exec: E, limit: i64) -> Result<Vec<PersonRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, name, sort_name, identifiers, images,
               bio, birth_date, death_date, created_at, updated_at,
               is_monitored, auto_want_mode, monitoring_started_at,
               last_metadata_refresh_at, next_metadata_refresh_at,
               metadata_refresh_interval_hours
        FROM people
        WHERE is_monitored = true
          AND (next_metadata_refresh_at IS NULL OR next_metadata_refresh_at < NOW())
        ORDER BY next_metadata_refresh_at ASC NULLS FIRST
        LIMIT $1
        "#,
        limit,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            row_to_person(
                r.id,
                r.name,
                r.sort_name,
                r.identifiers,
                r.images,
                r.bio,
                r.birth_date,
                r.death_date,
                r.created_at,
                r.updated_at,
                r.is_monitored,
                r.auto_want_mode,
                r.monitoring_started_at,
                r.last_metadata_refresh_at,
                r.next_metadata_refresh_at,
                r.metadata_refresh_interval_hours,
            )
        })
        .collect())
}

/// Bump the next metadata refresh time based on the person's configured interval.
pub async fn bump_next_refresh<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE people SET
            last_metadata_refresh_at = NOW(),
            next_metadata_refresh_at = NOW() + (metadata_refresh_interval_hours * interval '1 hour')
        WHERE id = $1
        "#,
        id,
    )
    .execute(exec)
    .await?;

    Ok(())
}

// =============================================================================
// Dismissed Credits
// =============================================================================

/// Dismiss a credit (movie/show) for a person so it is excluded from auto-want.
pub async fn dismiss_credit<'e, E>(
    exec: E,
    person_id: Uuid,
    tmdb_id: i32,
    media_type: &str,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO person_dismissed_credits (person_id, tmdb_id, media_type)
        VALUES ($1, $2, $3)
        ON CONFLICT (person_id, tmdb_id, media_type) DO NOTHING
        "#,
        person_id,
        tmdb_id,
        media_type,
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// Undismiss a previously dismissed credit. Returns true if a row was deleted.
pub async fn undismiss_credit<'e, E>(
    exec: E,
    person_id: Uuid,
    tmdb_id: i32,
    media_type: &str,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        DELETE FROM person_dismissed_credits
        WHERE person_id = $1 AND tmdb_id = $2 AND media_type = $3
        "#,
        person_id,
        tmdb_id,
        media_type,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// List all dismissed credits for a person.
pub async fn list_dismissed_credits<'e, E>(exec: E, person_id: Uuid) -> Result<Vec<DismissedCredit>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT tmdb_id, media_type
        FROM person_dismissed_credits
        WHERE person_id = $1
        ORDER BY tmdb_id ASC
        "#,
        person_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| DismissedCredit {
            tmdb_id: r.tmdb_id,
            media_type: r.media_type,
        })
        .collect())
}
