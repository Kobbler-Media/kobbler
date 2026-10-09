//! TV episode repository operations.

use super::Result;
use crate::images::ImageMap;
use serde_json::Value;
use sqlx::{Executor, PgPool, Postgres, Row};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Input for creating or upserting an episode.
#[derive(Debug)]
pub struct EpisodeInput {
    pub season_id: Uuid,
    pub episode_identifier: String,
    pub episode_sort_order: i32,
    pub title: Option<String>,
    pub description: Option<String>,
    pub images: Option<ImageMap>,
    pub air_date: Option<time::Date>,
    pub air_time: Option<OffsetDateTime>,
    pub runtime: Option<i32>,
    pub identifiers: Value,
    pub country: Option<String>,
}

/// Row returned from tv_episodes queries.
#[derive(Debug, Clone)]
pub struct EpisodeRow {
    pub id: Uuid,
    pub season_id: Uuid,
    pub episode_identifier: String,
    pub episode_sort_order: i32,
    pub title: Option<String>,
    pub description: Option<String>,
    pub images: ImageMap,
    pub air_date: Option<time::Date>,
    /// Full broadcast instant when a provider publishes one (TVMaze airstamp).
    /// `None` means the day is known but the time is not.
    pub air_time: Option<OffsetDateTime>,
    pub runtime: Option<i32>,
    pub is_in_library: bool,
    pub want_status: String,
    pub identifiers: Value,
    pub country: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[allow(clippy::too_many_arguments)]
fn row_to_episode(
    id: Uuid,
    season_id: Uuid,
    episode_identifier: String,
    episode_sort_order: i32,
    title: Option<String>,
    description: Option<String>,
    images_json: Value,
    air_date: Option<time::Date>,
    air_time: Option<OffsetDateTime>,
    runtime: Option<i32>,
    is_in_library: bool,
    want_status: String,
    identifiers: Value,
    country: Option<String>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
) -> EpisodeRow {
    let images: ImageMap = serde_json::from_value(images_json).unwrap_or_default();
    EpisodeRow {
        id,
        season_id,
        episode_identifier,
        episode_sort_order,
        title,
        description,
        images,
        air_date,
        air_time,
        runtime,
        is_in_library,
        want_status,
        identifiers,
        country,
        created_at,
        updated_at,
    }
}

// =============================================================================
// Create
// =============================================================================

/// Create a new episode.
pub async fn create<'e, E>(exec: E, input: EpisodeInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let images = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();

    let created_id: Uuid = sqlx::query_scalar!(
        r#"
        INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order,
                                 title, description, images, air_date, air_time, runtime, identifiers, country)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        RETURNING id
        "#,
        id,
        input.season_id,
        input.episode_identifier,
        input.episode_sort_order,
        input.title as Option<String>,
        input.description as Option<String>,
        images,
        input.air_date,
        input.air_time,
        input.runtime,
        input.identifiers,
        input.country as Option<String>,
    )
    .fetch_one(exec)
    .await?;

    Ok(created_id)
}

// =============================================================================
// Read
// =============================================================================

/// Get an episode by ID.
pub async fn get<'e, E>(exec: E, id: Uuid) -> Result<Option<EpisodeRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, season_id, episode_identifier, episode_sort_order,
               title, description, images, air_date, air_time, runtime,
               EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = tv_episodes.id) AS "is_in_library!",
               want_status, identifiers, country, created_at, updated_at
        FROM tv_episodes
        WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| {
        row_to_episode(
            r.id,
            r.season_id,
            r.episode_identifier,
            r.episode_sort_order,
            r.title,
            r.description,
            r.images,
            r.air_date,
            r.air_time,
            r.runtime,
            r.is_in_library,
            r.want_status,
            r.identifiers,
            r.country,
            r.created_at,
            r.updated_at,
        )
    }))
}

/// List all episodes for a season, ordered by sort order.
pub async fn list_by_season<'e, E>(exec: E, season_id: Uuid) -> Result<Vec<EpisodeRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, season_id, episode_identifier, episode_sort_order,
               title, description, images, air_date, air_time, runtime,
               EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = tv_episodes.id) AS "is_in_library!",
               want_status, identifiers, country, created_at, updated_at
        FROM tv_episodes
        WHERE season_id = $1
        ORDER BY episode_sort_order ASC
        "#,
        season_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            row_to_episode(
                r.id,
                r.season_id,
                r.episode_identifier,
                r.episode_sort_order,
                r.title,
                r.description,
                r.images,
                r.air_date,
                r.air_time,
                r.runtime,
                r.is_in_library,
                r.want_status,
                r.identifiers,
                r.country,
                r.created_at,
                r.updated_at,
            )
        })
        .collect())
}

/// Get an episode by season ID and identifier.
pub async fn get_by_season_and_identifier<'e, E>(
    exec: E,
    season_id: Uuid,
    identifier: &str,
) -> Result<Option<EpisodeRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, season_id, episode_identifier, episode_sort_order,
               title, description, images, air_date, air_time, runtime,
               EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = tv_episodes.id) AS "is_in_library!",
               want_status, identifiers, country, created_at, updated_at
        FROM tv_episodes
        WHERE season_id = $1 AND episode_identifier = $2
        "#,
        season_id,
        identifier,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| {
        row_to_episode(
            r.id,
            r.season_id,
            r.episode_identifier,
            r.episode_sort_order,
            r.title,
            r.description,
            r.images,
            r.air_date,
            r.air_time,
            r.runtime,
            r.is_in_library,
            r.want_status,
            r.identifiers,
            r.country,
            r.created_at,
            r.updated_at,
        )
    }))
}

/// List episodes that are missing (want_status='none' AND no files).
pub async fn list_missing<'e, E>(exec: E, season_id: Uuid) -> Result<Vec<EpisodeRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, season_id, episode_identifier, episode_sort_order,
               title, description, images, air_date, air_time, runtime,
               EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = tv_episodes.id) AS "is_in_library!",
               want_status, identifiers, country, created_at, updated_at
        FROM tv_episodes
        WHERE season_id = $1 AND want_status = 'none'
          AND NOT EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = tv_episodes.id)
        ORDER BY episode_sort_order ASC
        "#,
        season_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            row_to_episode(
                r.id,
                r.season_id,
                r.episode_identifier,
                r.episode_sort_order,
                r.title,
                r.description,
                r.images,
                r.air_date,
                r.air_time,
                r.runtime,
                r.is_in_library,
                r.want_status,
                r.identifiers,
                r.country,
                r.created_at,
                r.updated_at,
            )
        })
        .collect())
}

/// List ALL episodes for a season regardless of want_status or library state.
/// Used by the whole-season want flow where every episode should be considered.
pub async fn list_for_season<'e, E>(exec: E, season_id: Uuid) -> Result<Vec<EpisodeRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, season_id, episode_identifier, episode_sort_order,
               title, description, images, air_date, air_time, runtime,
               EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = tv_episodes.id) AS "is_in_library!",
               want_status, identifiers, country, created_at, updated_at
        FROM tv_episodes
        WHERE season_id = $1
        ORDER BY episode_sort_order ASC
        "#,
        season_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            row_to_episode(
                r.id,
                r.season_id,
                r.episode_identifier,
                r.episode_sort_order,
                r.title,
                r.description,
                r.images,
                r.air_date,
                r.air_time,
                r.runtime,
                r.is_in_library,
                r.want_status,
                r.identifiers,
                r.country,
                r.created_at,
                r.updated_at,
            )
        })
        .collect())
}

/// List all episodes that should be included in a kobload watch pattern for a
/// season: episodes not in library with an active want status (stuck or in-progress),
/// plus episodes in library (upgrade candidates). Excludes truly unwanted episodes
/// (want_status='none' AND not in library).
pub async fn list_for_season_pattern<'e, E>(exec: E, season_id: Uuid) -> Result<Vec<EpisodeRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, season_id, episode_identifier, episode_sort_order,
               title, description, images, air_date, air_time, runtime,
               EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = tv_episodes.id) AS "is_in_library!",
               want_status, identifiers, country, created_at, updated_at
        FROM tv_episodes
        WHERE season_id = $1 AND (want_status != 'none' OR EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = tv_episodes.id))
        ORDER BY episode_sort_order ASC
        "#,
        season_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            row_to_episode(
                r.id,
                r.season_id,
                r.episode_identifier,
                r.episode_sort_order,
                r.title,
                r.description,
                r.images,
                r.air_date,
                r.air_time,
                r.runtime,
                r.is_in_library,
                r.want_status,
                r.identifiers,
                r.country,
                r.created_at,
                r.updated_at,
            )
        })
        .collect())
}

/// A lightweight row for stale-watch-pattern reconciliation: episode id + season_id.
pub struct StaleWatchEpisode {
    pub id: Uuid,
    pub season_id: Uuid,
}

/// List all episodes whose want_status is 'watching' or 'searching' (i.e. they
/// believe a kobload watch pattern is active). Used by tv_monitor to reconcile
/// against the actual watch patterns on kobload and reset stale ones.
///
/// Excludes episodes that already have library files: those are satisfied (kobload may
/// still hold an upgrade pattern) and must never be reset back to 'wanted'. Guards
/// against historical rows left at a non-terminal want_status before imports started
/// setting it to 'imported'.
pub async fn list_active_watch_statuses(pool: &sqlx::PgPool) -> Result<Vec<StaleWatchEpisode>> {
    let rows = sqlx::query!(
        r#"
        SELECT id, season_id
        FROM tv_episodes e
        WHERE want_status IN ('watching', 'searching', 'downloading', 'ingesting')
          AND NOT EXISTS (
              SELECT 1 FROM tv_episode_file_episodes efe WHERE efe.episode_id = e.id
          )
        ORDER BY season_id
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| StaleWatchEpisode {
            id: r.id,
            season_id: r.season_id,
        })
        .collect())
}

// =============================================================================
// Upsert / Bulk
// =============================================================================

/// Bulk upsert episodes for a season. Uses a transaction and ON CONFLICT.
pub async fn bulk_upsert(
    pool: &sqlx::PgPool,
    season_id: Uuid,
    episodes: Vec<EpisodeInput>,
) -> Result<Vec<Uuid>> {
    let mut tx = pool.begin().await?;
    let mut ids = Vec::with_capacity(episodes.len());

    for ep in episodes {
        let id = Uuid::now_v7();
        let images = serde_json::to_value(ep.images.unwrap_or_default()).unwrap_or_default();
        let created_id: Uuid = sqlx::query_scalar!(
            r#"
            INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order,
                                     title, description, images, air_date, air_time, runtime, identifiers, country)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT (season_id, episode_identifier)
            DO UPDATE SET
                title = COALESCE(EXCLUDED.title, tv_episodes.title),
                description = COALESCE(EXCLUDED.description, tv_episodes.description),
                images = tv_episodes.images || EXCLUDED.images,
                air_date = COALESCE(EXCLUDED.air_date, tv_episodes.air_date),
                air_time = COALESCE(EXCLUDED.air_time, tv_episodes.air_time),
                runtime = COALESCE(EXCLUDED.runtime, tv_episodes.runtime),
                episode_sort_order = EXCLUDED.episode_sort_order,
                identifiers = tv_episodes.identifiers || EXCLUDED.identifiers,
                country = COALESCE(EXCLUDED.country, tv_episodes.country),
                updated_at = NOW()
            RETURNING id
            "#,
            id,
            season_id,
            ep.episode_identifier,
            ep.episode_sort_order,
            ep.title as Option<String>,
            ep.description as Option<String>,
            images,
            ep.air_date,
            ep.air_time,
            ep.runtime,
            ep.identifiers,
            ep.country as Option<String>,
        )
        .fetch_one(&mut *tx)
        .await?;

        ids.push(created_id);
    }

    tx.commit().await?;
    Ok(ids)
}

// =============================================================================
// Update
// =============================================================================

/// Set the want_status of a single episode.
pub async fn set_want_status<'e, E>(exec: E, id: Uuid, status: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE tv_episodes SET want_status = $2, updated_at = NOW() WHERE id = $1",
        id,
        status,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Set the want_status for a batch of episodes.
/// Season an episode belongs to.
pub async fn season_id_for_episode<'e, E>(exec: E, episode_id: Uuid) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = sqlx::query_scalar!(
        "SELECT season_id FROM tv_episodes WHERE id = $1",
        episode_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(id)
}

/// An episode's place in the library hierarchy, for bulk operations that have to
/// group by season and emit show-level library events.
pub struct EpisodeScope {
    pub id: Uuid,
    pub season_id: Uuid,
    pub show_id: Uuid,
    pub is_in_library: bool,
    pub want_status: String,
}

/// Resolve an arbitrary set of episode IDs to their season and show. IDs that do
/// not exist are simply absent from the result, so callers can detect them.
pub async fn list_scopes<'e, E>(exec: E, ids: &[Uuid]) -> Result<Vec<EpisodeScope>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT e.id, e.season_id, s.show_id, e.want_status,
               EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = e.id) AS "is_in_library!"
        FROM tv_episodes e
        JOIN tv_seasons s ON s.id = e.season_id
        WHERE e.id = ANY($1)
        "#,
        ids,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| EpisodeScope {
            id: r.id,
            season_id: r.season_id,
            show_id: r.show_id,
            is_in_library: r.is_in_library,
            want_status: r.want_status,
        })
        .collect())
}

pub async fn set_want_status_batch(pool: &sqlx::PgPool, ids: &[Uuid], status: &str) -> Result<()> {
    sqlx::query!(
        "UPDATE tv_episodes SET want_status = $2, updated_at = NOW() WHERE id = ANY($1)",
        ids,
        status,
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Clear the want status of every not-yet-acquired episode in a season.
///
/// Episodes that already have a file are left alone — their want_status records
/// how they were acquired, and resetting it would make them look never-wanted.
/// Returns the IDs actually flipped so callers can report a count.
pub async fn clear_wanted_in_season<'e, E>(exec: E, season_id: Uuid) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!(
        r#"
        UPDATE tv_episodes SET want_status = 'none', updated_at = NOW()
        WHERE season_id = $1 AND want_status != 'none'
          AND NOT EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = tv_episodes.id)
        RETURNING id
        "#,
        season_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(ids)
}

/// Reset episodes that lost their last file back to a wantable state.
///
/// A successful import parks an episode at 'imported'. Every "want missing" path
/// — [`list_missing`], want_season, auto_want — matches `want_status = 'none'`,
/// so an episode left at 'imported' with nothing on disk renders as missing and
/// is never picked up again. The `NOT EXISTS` guard means an episode that still
/// has another copy keeps its status, and an in-flight acquisition
/// ('wanted'/'searching'/'downloading'/…) is left alone so deleting the old file
/// does not cancel an upgrade.
pub async fn clear_imported_without_files<'e, E>(exec: E, ids: &[Uuid]) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let reset = sqlx::query_scalar!(
        r#"
        UPDATE tv_episodes SET want_status = 'none', updated_at = NOW()
        WHERE id = ANY($1) AND want_status = 'imported'
          AND NOT EXISTS (SELECT 1 FROM tv_episode_file_episodes efe WHERE efe.episode_id = tv_episodes.id)
        RETURNING id
        "#,
        ids,
    )
    .fetch_all(exec)
    .await?;

    Ok(reset)
}

/// Clear the want status of an explicit set of episodes.
/// See [`clear_wanted_in_season`] for what is deliberately left untouched.
pub async fn clear_wanted_in_episodes<'e, E>(exec: E, ids: &[Uuid]) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let cleared = sqlx::query_scalar!(
        r#"
        UPDATE tv_episodes SET want_status = 'none', updated_at = NOW()
        WHERE id = ANY($1) AND want_status != 'none'
          AND NOT EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = tv_episodes.id)
        RETURNING id
        "#,
        ids,
    )
    .fetch_all(exec)
    .await?;

    Ok(cleared)
}

/// Clear the want status of every not-yet-acquired episode across a whole show.
/// See [`clear_wanted_in_season`] for what is deliberately left untouched.
pub async fn clear_wanted_in_show<'e, E>(exec: E, show_id: Uuid) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!(
        r#"
        UPDATE tv_episodes SET want_status = 'none', updated_at = NOW()
        WHERE season_id IN (SELECT id FROM tv_seasons WHERE show_id = $1)
          AND want_status != 'none'
          AND NOT EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = tv_episodes.id)
        RETURNING id
        "#,
        show_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(ids)
}

/// Find or create an episode, returning its ID.
///
/// Uses INSERT ON CONFLICT to avoid the SELECT+INSERT round-trip pattern.
/// Always returns the ID whether the episode already existed or was just created.
pub async fn find_or_create(
    pool: &sqlx::PgPool,
    season_id: Uuid,
    episode_identifier: &str,
    episode_sort_order: i32,
    title: Option<&str>,
) -> Result<Uuid> {
    let id = Uuid::now_v7();
    let result: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (season_id, episode_identifier)
        DO UPDATE SET episode_sort_order = EXCLUDED.episode_sort_order
        RETURNING id
        "#,
    )
    .bind(id)
    .bind(season_id)
    .bind(episode_identifier)
    .bind(episode_sort_order)
    .bind(title)
    .fetch_one(pool)
    .await?;

    Ok(result)
}

// =============================================================================
// Update
// =============================================================================

/// Input for updating an episode.
#[derive(Debug, Default)]
pub struct EpisodeUpdate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub images: Option<ImageMap>,
    pub air_date: Option<time::Date>,
    pub runtime: Option<i32>,
}

/// Update an episode by ID.
pub async fn update<'e, E>(exec: E, id: Uuid, input: EpisodeUpdate) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let images = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();
    let result = sqlx::query!(
        r#"
        UPDATE tv_episodes SET
            title = COALESCE($2, title),
            description = COALESCE($3, description),
            images = images || $4,
            air_date = COALESCE($5, air_date),
            runtime = COALESCE($6, runtime),
            updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        input.title as Option<String>,
        input.description as Option<String>,
        images,
        input.air_date,
        input.runtime,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Update the country field for an episode.
pub async fn update_country<'e, E>(exec: E, id: Uuid, country: Option<&str>) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE tv_episodes SET country = $2, updated_at = NOW() WHERE id = $1",
        id,
        country,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

// =============================================================================
// Delete
// =============================================================================

/// Delete an episode by ID.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM tv_episodes WHERE id = $1", id)
        .execute(exec)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete an explicit set of episodes. Returns the number of rows removed.
pub async fn delete_many<'e, E>(exec: E, ids: &[Uuid]) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM tv_episodes WHERE id = ANY($1)", ids)
        .execute(exec)
        .await?;

    Ok(result.rows_affected())
}

/// Delete orphaned episodes: no *available* files, not wanted, no *available*
/// media root file pointing to them, not debrid-streamed, and not recently
/// updated (grace period for in-progress jobs).
/// Returns IDs of deleted episodes for cascading cleanup.
pub async fn delete_orphaned_episodes<'e, E>(exec: E) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!(
        r#"
        DELETE FROM tv_episodes
        WHERE want_status = 'none'
          AND updated_at < NOW() - INTERVAL '30 minutes'
          AND NOT EXISTS (
              SELECT 1 FROM tv_episode_file_episodes tefe
              JOIN tv_episode_files tef ON tef.id = tefe.episode_file_id AND tef.available = true
              WHERE tefe.episode_id = tv_episodes.id
          )
          AND NOT EXISTS (
              SELECT 1 FROM media_root_files mrf
              WHERE mrf.entity_id = tv_episodes.id AND mrf.entity_type = 'tv_episode' AND mrf.available = true
          )
          AND NOT EXISTS (
              SELECT 1 FROM debrid_stream_sessions dss
              WHERE dss.media_id = tv_episodes.id AND dss.media_type = 'episode'
          )
        RETURNING id
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(ids)
}

// =============================================================================
// Rejected Releases
// =============================================================================

/// Append a rejected release entry to an episode's rejected_releases array.
pub async fn add_rejected_release<'e, E>(
    exec: E,
    id: Uuid,
    release_info: serde_json::Value,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE tv_episodes SET rejected_releases = rejected_releases || $2::jsonb, updated_at = NOW() WHERE id = $1",
        id,
        serde_json::json!([release_info]),
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Clear all rejected releases for an episode.
pub async fn clear_rejected_releases<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE tv_episodes SET rejected_releases = '[]'::jsonb, updated_at = NOW() WHERE id = $1",
        id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Get rejected releases for an episode.
pub async fn get_rejected_releases<'e, E>(exec: E, id: Uuid) -> Result<serde_json::Value>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!(
        "SELECT rejected_releases FROM tv_episodes WHERE id = $1",
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.unwrap_or(serde_json::json!([])))
}

// =============================================================================
// Lookup by show TMDB ID + season/episode numbers
// =============================================================================

/// Find an episode by the show's TMDB identifier and season/episode numbers.
pub async fn get_by_show_tmdb_and_numbers<'e, E>(
    exec: E,
    show_tmdb_id: &str,
    season_number: i32,
    episode_number: i32,
) -> Result<Option<EpisodeRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let season_str = season_number.to_string();
    let episode_str = episode_number.to_string();
    let row = sqlx::query!(
        r#"
        SELECT e.id, e.season_id, e.episode_identifier, e.episode_sort_order,
               e.title, e.description, e.images, e.air_date, e.air_time, e.runtime,
               EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = e.id) AS "is_in_library!",
               e.want_status, e.identifiers, e.country, e.created_at, e.updated_at
        FROM tv_episodes e
        JOIN tv_seasons s ON s.id = e.season_id
        JOIN tv_shows sh ON sh.id = s.show_id
        WHERE sh.identifiers->>'tmdb_id' = $1
          AND s.season_identifier = $2
          AND e.episode_identifier = $3
        "#,
        show_tmdb_id,
        season_str,
        episode_str,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| {
        row_to_episode(
            r.id,
            r.season_id,
            r.episode_identifier,
            r.episode_sort_order,
            r.title,
            r.description,
            r.images,
            r.air_date,
            r.air_time,
            r.runtime,
            r.is_in_library,
            r.want_status,
            r.identifiers,
            r.country,
            r.created_at,
            r.updated_at,
        )
    }))
}

// =============================================================================
// Next Episode
// =============================================================================

#[derive(Debug, serde::Serialize)]
pub struct NextEpisodeInfo {
    pub episode_id: Uuid,
    pub title: Option<String>,
    pub episode_identifier: String,
    pub season_identifier: String,
    pub show_title: String,
    pub still_url: Option<String>,
    pub file_id: Option<Uuid>,
}

fn extract_still_from_images(images: Option<serde_json::Value>) -> Option<String> {
    images.and_then(|v| {
        v.get("still")?
            .get("source_url")?
            .as_str()
            .map(String::from)
    })
}

/// Find the next available episode after the given one.
/// An episode is available if it has a local file OR its air_date has passed.
/// Checks the same season first, then the next season.
pub async fn get_next_episode(pool: &PgPool, episode_id: Uuid) -> Result<Option<NextEpisodeInfo>> {
    // Get current episode's position
    let current =
        sqlx::query("SELECT season_id, episode_sort_order FROM tv_episodes WHERE id = $1")
            .bind(episode_id)
            .fetch_optional(pool)
            .await?;

    let current = match current {
        Some(row) => row,
        None => return Ok(None),
    };

    let season_id: Uuid = current.get("season_id");
    let sort_order: i32 = current.get("episode_sort_order");

    // Try next episode in same season. A local file is the only thing that counts as
    // playable here: an episode that has merely aired is still a download the user is
    // waiting on. `compute_continue_episode` can also offer a file-less episode on a
    // show being streamed, but that turns on the caller's watch history and this query
    // takes no user, so it stays with what is on disk.
    let next_in_season = sqlx::query(
        r#"
        SELECT e.id, e.title, e.episode_identifier, e.images,
               s.season_identifier, sh.title AS show_title,
               (SELECT f.id FROM tv_episode_file_episodes efe
                JOIN tv_episode_files f ON f.id = efe.episode_file_id
                WHERE efe.episode_id = e.id AND f.available = true
                ORDER BY f.created_at DESC, f.id DESC
                LIMIT 1) AS file_id
        FROM tv_episodes e
        JOIN tv_seasons s ON s.id = e.season_id
        JOIN tv_shows sh ON sh.id = s.show_id
        WHERE e.season_id = $1
          AND e.episode_sort_order > $2
          AND EXISTS (SELECT 1 FROM tv_episode_file_episodes efe2
                      JOIN tv_episode_files ef2 ON ef2.id = efe2.episode_file_id
                      WHERE efe2.episode_id = e.id)
        ORDER BY e.episode_sort_order
        LIMIT 1
        "#,
    )
    .bind(season_id)
    .bind(sort_order)
    .fetch_optional(pool)
    .await?;

    if let Some(row) = next_in_season {
        return Ok(Some(NextEpisodeInfo {
            episode_id: row.get("id"),
            title: row.get("title"),
            episode_identifier: row.get("episode_identifier"),
            season_identifier: row.get("season_identifier"),
            show_title: row.get("show_title"),
            still_url: extract_still_from_images(row.get::<Option<serde_json::Value>, _>("images")),
            file_id: row.get("file_id"),
        }));
    }

    // Try first episode of next season
    let next_season = sqlx::query(
        r#"
        SELECT e.id, e.title, e.episode_identifier, e.images,
               s2.season_identifier, sh.title AS show_title,
               (SELECT f.id FROM tv_episode_file_episodes efe
                JOIN tv_episode_files f ON f.id = efe.episode_file_id
                WHERE efe.episode_id = e.id AND f.available = true
                ORDER BY f.created_at DESC, f.id DESC
                LIMIT 1) AS file_id
        FROM tv_seasons s
        JOIN tv_shows sh ON sh.id = s.show_id
        JOIN tv_seasons s2 ON s2.show_id = s.show_id AND s2.season_sort_order > s.season_sort_order
        JOIN tv_episodes e ON e.season_id = s2.id
        WHERE s.id = $1
          AND EXISTS (SELECT 1 FROM tv_episode_file_episodes efe2
                      JOIN tv_episode_files ef2 ON ef2.id = efe2.episode_file_id
                      WHERE efe2.episode_id = e.id)
        ORDER BY s2.season_sort_order, e.episode_sort_order
        LIMIT 1
        "#,
    )
    .bind(season_id)
    .fetch_optional(pool)
    .await?;

    if let Some(row) = next_season {
        return Ok(Some(NextEpisodeInfo {
            episode_id: row.get("id"),
            title: row.get("title"),
            episode_identifier: row.get("episode_identifier"),
            season_identifier: row.get("season_identifier"),
            show_title: row.get("show_title"),
            still_url: extract_still_from_images(row.get::<Option<serde_json::Value>, _>("images")),
            file_id: row.get("file_id"),
        }));
    }

    Ok(None)
}
