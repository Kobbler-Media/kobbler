//! TV season repository operations.

use super::Result;
use crate::images::ImageMap;
use serde_json::Value;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Input for creating or upserting a season.
#[derive(Debug)]
pub struct SeasonInput {
    pub show_id: Uuid,
    pub season_identifier: String,
    pub season_sort_order: i32,
    pub title: Option<String>,
    pub description: Option<String>,
    pub images: Option<ImageMap>,
    pub air_date: Option<time::Date>,
    pub episode_count: Option<i32>,
    pub is_monitored: bool,
    pub is_special: bool,
    pub auto_want_mode: String,
}

/// Row returned from tv_seasons queries.
#[derive(Debug)]
pub struct SeasonRow {
    pub id: Uuid,
    pub show_id: Uuid,
    pub season_identifier: String,
    pub season_sort_order: i32,
    pub title: Option<String>,
    pub description: Option<String>,
    pub images: ImageMap,
    pub air_date: Option<time::Date>,
    pub episode_count: Option<i32>,
    pub is_monitored: bool,
    pub is_special: bool,
    pub auto_want_mode: String,
    pub created_at: OffsetDateTime,
}

#[allow(clippy::too_many_arguments)]
fn row_to_season(
    id: Uuid,
    show_id: Uuid,
    season_identifier: String,
    season_sort_order: i32,
    title: Option<String>,
    description: Option<String>,
    images_json: Value,
    air_date: Option<time::Date>,
    episode_count: Option<i32>,
    is_monitored: bool,
    is_special: bool,
    auto_want_mode: String,
    created_at: OffsetDateTime,
) -> SeasonRow {
    let images: ImageMap = serde_json::from_value(images_json).unwrap_or_default();
    SeasonRow {
        id,
        show_id,
        season_identifier,
        season_sort_order,
        title,
        description,
        images,
        air_date,
        episode_count,
        is_monitored,
        is_special,
        auto_want_mode,
        created_at,
    }
}

// =============================================================================
// Create
// =============================================================================

/// Create a new season.
pub async fn create<'e, E>(exec: E, input: SeasonInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let images = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();

    let created_id: Uuid = sqlx::query_scalar!(
        r#"
        INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order,
                                title, description, images, air_date,
                                episode_count, is_monitored, is_special, auto_want_mode)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        RETURNING id
        "#,
        id,
        input.show_id,
        input.season_identifier,
        input.season_sort_order,
        input.title as Option<String>,
        input.description as Option<String>,
        images,
        input.air_date,
        input.episode_count,
        input.is_monitored,
        input.is_special,
        input.auto_want_mode,
    )
    .fetch_one(exec)
    .await?;

    Ok(created_id)
}

// =============================================================================
// Read
// =============================================================================

/// Get a season by ID.
pub async fn get<'e, E>(exec: E, id: Uuid) -> Result<Option<SeasonRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, show_id, season_identifier, season_sort_order,
               title, description, images, air_date,
               episode_count, is_monitored, is_special, auto_want_mode, created_at
        FROM tv_seasons
        WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| {
        row_to_season(
            r.id,
            r.show_id,
            r.season_identifier,
            r.season_sort_order,
            r.title,
            r.description,
            r.images,
            r.air_date,
            r.episode_count,
            r.is_monitored,
            r.is_special,
            r.auto_want_mode,
            r.created_at,
        )
    }))
}

/// List all seasons for a show, ordered by sort order.
pub async fn list_by_show<'e, E>(exec: E, show_id: Uuid) -> Result<Vec<SeasonRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, show_id, season_identifier, season_sort_order,
               title, description, images, air_date,
               episode_count, is_monitored, is_special, auto_want_mode, created_at
        FROM tv_seasons
        WHERE show_id = $1
        ORDER BY season_sort_order ASC
        "#,
        show_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            row_to_season(
                r.id,
                r.show_id,
                r.season_identifier,
                r.season_sort_order,
                r.title,
                r.description,
                r.images,
                r.air_date,
                r.episode_count,
                r.is_monitored,
                r.is_special,
                r.auto_want_mode,
                r.created_at,
            )
        })
        .collect())
}

/// Row returned from paginated season queries, includes actual episode count.
#[derive(Debug)]
pub struct PaginatedSeasonRow {
    pub season: SeasonRow,
    pub actual_episode_count: i64,
    /// Episodes still outstanding: wanted (or mid-acquisition) with no file yet.
    /// Lets a list view know whether a bulk unwant would do anything, without
    /// shipping every episode row.
    pub wanted_episode_count: i64,
    pub in_library_episode_count: i64,
    /// Watched by the requesting user.
    pub watched_episode_count: i64,
}

/// List seasons for a show with pagination, ordered by sort order.
/// Returns (seasons, total_count).
pub async fn list_by_show_paginated(
    pool: &sqlx::PgPool,
    show_id: Uuid,
    user_id: Uuid,
    limit: i64,
    offset: i64,
    sort_desc: bool,
) -> Result<(Vec<PaginatedSeasonRow>, i64)> {
    let order = if sort_desc { "DESC" } else { "ASC" };
    let query = format!(
        r#"
        SELECT s.id, s.show_id, s.season_identifier, s.season_sort_order,
               s.title, s.description, s.images, s.air_date,
               s.episode_count, s.is_monitored, s.is_special, s.auto_want_mode, s.created_at,
               COUNT(e.id) AS actual_episode_count,
               COUNT(e.id) FILTER (
                   WHERE e.want_status <> 'none'
                     AND NOT EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = e.id)
               ) AS wanted_episode_count,
               COUNT(e.id) FILTER (
                   WHERE EXISTS (SELECT 1 FROM tv_episode_file_episodes efe WHERE efe.episode_id = e.id)
               ) AS in_library_episode_count,
               COUNT(e.id) FILTER (
                   WHERE EXISTS (SELECT 1 FROM tv_watch_states ws WHERE ws.episode_id = e.id AND ws.user_id = $4 AND ws.watched)
               ) AS watched_episode_count
        FROM tv_seasons s
        LEFT JOIN tv_episodes e ON e.season_id = s.id
        WHERE s.show_id = $1
        GROUP BY s.id
        ORDER BY s.season_sort_order {order}
        LIMIT $2 OFFSET $3
        "#
    );

    let rows = sqlx::query(sqlx::AssertSqlSafe(query.as_str()))
        .bind(show_id)
        .bind(limit)
        .bind(offset)
        .bind(user_id)
        .fetch_all(pool)
        .await?;

    let seasons: Vec<PaginatedSeasonRow> = rows
        .into_iter()
        .map(|r| {
            use sqlx::Row;
            let images_json: Value = r.get("images");
            PaginatedSeasonRow {
                season: row_to_season(
                    r.get("id"),
                    r.get("show_id"),
                    r.get("season_identifier"),
                    r.get("season_sort_order"),
                    r.get("title"),
                    r.get("description"),
                    images_json,
                    r.get("air_date"),
                    r.get("episode_count"),
                    r.get("is_monitored"),
                    r.get("is_special"),
                    r.get("auto_want_mode"),
                    r.get("created_at"),
                ),
                actual_episode_count: r.get("actual_episode_count"),
                wanted_episode_count: r.get("wanted_episode_count"),
                in_library_episode_count: r.get("in_library_episode_count"),
                watched_episode_count: r.get("watched_episode_count"),
            }
        })
        .collect();

    let total: i64 = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM tv_seasons WHERE show_id = $1",
        show_id
    )
    .fetch_one(pool)
    .await?
    .unwrap_or(0);

    Ok((seasons, total))
}

// =============================================================================
// Upsert
// =============================================================================

/// Upsert a season by (show_id, season_identifier).
pub async fn upsert<'e, E>(exec: E, input: SeasonInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let images = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();

    let created_id: Uuid = sqlx::query_scalar!(
        r#"
        INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order,
                                title, description, images, air_date,
                                episode_count, is_monitored, is_special, auto_want_mode)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        ON CONFLICT (show_id, season_identifier)
        DO UPDATE SET
            season_sort_order = EXCLUDED.season_sort_order,
            title = COALESCE(EXCLUDED.title, tv_seasons.title),
            description = COALESCE(EXCLUDED.description, tv_seasons.description),
            images = tv_seasons.images || EXCLUDED.images,
            air_date = COALESCE(EXCLUDED.air_date, tv_seasons.air_date),
            episode_count = COALESCE(EXCLUDED.episode_count, tv_seasons.episode_count),
            is_special = EXCLUDED.is_special,
            is_monitored = EXCLUDED.is_monitored
        RETURNING id
        "#,
        id,
        input.show_id,
        input.season_identifier,
        input.season_sort_order,
        input.title as Option<String>,
        input.description as Option<String>,
        images,
        input.air_date,
        input.episode_count,
        input.is_monitored,
        input.is_special,
        input.auto_want_mode,
    )
    .fetch_one(exec)
    .await?;

    Ok(created_id)
}

// =============================================================================
// Update
// =============================================================================

/// Input for updating a season. All fields optional.
#[derive(Debug, Default)]
pub struct SeasonUpdate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub images: Option<ImageMap>,
    pub air_date: Option<time::Date>,
    pub episode_count: Option<i32>,
    pub is_special: Option<bool>,
    pub auto_want_mode: Option<String>,
}

/// Update a season by ID. Only non-None fields are updated.
pub async fn update<'e, E>(exec: E, id: Uuid, input: SeasonUpdate) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let images = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();
    let result = sqlx::query!(
        r#"
        UPDATE tv_seasons SET
            title = COALESCE($2, title),
            description = COALESCE($3, description),
            images = images || $4,
            air_date = COALESCE($5, air_date),
            episode_count = COALESCE($6, episode_count),
            is_special = COALESCE($7, is_special),
            auto_want_mode = COALESCE($8, auto_want_mode)
        WHERE id = $1
        "#,
        id,
        input.title as Option<String>,
        input.description as Option<String>,
        images,
        input.air_date,
        input.episode_count,
        input.is_special,
        input.auto_want_mode as Option<String>,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Set the is_monitored flag for a season.
pub async fn set_monitored<'e, E>(exec: E, id: Uuid, monitored: bool) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE tv_seasons SET is_monitored = $2 WHERE id = $1",
        id,
        monitored,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Set the auto_want_mode for a season.
pub async fn set_auto_want_mode<'e, E>(exec: E, id: Uuid, mode: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE tv_seasons SET auto_want_mode = $2 WHERE id = $1",
        id,
        mode,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Turn off the season-level auto-want modes that would re-want an unwanted
/// episode ('all' and 'fill_gaps'). 'new_only' is left alone: it only picks up
/// episodes that have not aired yet, so it cannot undo a bulk unwant.
/// Returns how many seasons were changed.
pub async fn disable_refilling_auto_want_for_show<'e, E>(exec: E, show_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE tv_seasons SET auto_want_mode = 'none' WHERE show_id = $1 AND auto_want_mode IN ('all', 'fill_gaps')",
        show_id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected())
}

// =============================================================================
// Delete
// =============================================================================

/// Delete a season by ID.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM tv_seasons WHERE id = $1", id)
        .execute(exec)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete orphaned seasons: not monitored or auto_want_mode resolves to 'none',
/// no episodes remain.
/// Returns IDs of deleted seasons for cascading cleanup.
pub async fn delete_orphaned<'e, E>(exec: E) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!(
        r#"
        DELETE FROM tv_seasons
        WHERE (
            is_monitored = false
            OR auto_want_mode = 'none'
            OR (auto_want_mode = 'inherit' AND EXISTS (
                SELECT 1 FROM tv_shows WHERE tv_shows.id = tv_seasons.show_id AND tv_shows.auto_want_mode = 'none'
            ))
        )
          AND NOT EXISTS (SELECT 1 FROM tv_episodes WHERE season_id = tv_seasons.id)
        RETURNING id
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(ids)
}
