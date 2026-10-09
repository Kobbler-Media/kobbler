//! TV watch states repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Input for creating or updating a TV watch state.
#[derive(Debug)]
pub struct TvWatchStateInput {
    pub episode_id: Uuid,
    pub user_id: Uuid,
    pub watched: bool,
    pub watched_at: Option<OffsetDateTime>,
    pub position_seconds: Option<i32>,
    pub rating: Option<f64>,
}

/// Row returned from tv_watch_states queries.
#[derive(Debug)]
pub struct TvWatchStateRow {
    pub id: Uuid,
    pub episode_id: Uuid,
    pub user_id: Uuid,
    pub watched: bool,
    pub watched_at: Option<OffsetDateTime>,
    pub position_seconds: Option<i32>,
    pub rating: Option<f64>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

// =============================================================================
// Operations
// =============================================================================

/// Get a watch state for a specific episode and user.
pub async fn get<'e, E>(exec: E, episode_id: Uuid, user_id: Uuid) -> Result<Option<TvWatchStateRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, episode_id, user_id, watched, watched_at,
               position_seconds, rating, created_at, updated_at
        FROM tv_watch_states
        WHERE episode_id = $1 AND user_id = $2
        "#,
        episode_id,
        user_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| TvWatchStateRow {
        id: r.id,
        episode_id: r.episode_id,
        user_id: r.user_id,
        watched: r.watched,
        watched_at: r.watched_at,
        position_seconds: r.position_seconds,
        rating: r.rating,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }))
}

/// Upsert a watch state (insert or update on conflict of episode_id + user_id).
pub async fn upsert<'e, E>(exec: E, input: TvWatchStateInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();

    let row = sqlx::query_scalar!(
        r#"
        INSERT INTO tv_watch_states (id, episode_id, user_id, watched, watched_at,
                                     position_seconds, rating)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (episode_id, user_id)
        DO UPDATE SET
            watched = EXCLUDED.watched,
            watched_at = COALESCE(EXCLUDED.watched_at, tv_watch_states.watched_at),
            position_seconds = COALESCE(EXCLUDED.position_seconds, tv_watch_states.position_seconds),
            rating = COALESCE(EXCLUDED.rating, tv_watch_states.rating),
            updated_at = NOW()
        RETURNING id
        "#,
        id,
        input.episode_id,
        input.user_id,
        input.watched,
        input.watched_at,
        input.position_seconds,
        input.rating,
    )
    .fetch_one(exec)
    .await?;

    Ok(row)
}

/// Fetch all watch states for a show's episodes (for a given user).
/// Joins through tv_episodes → tv_seasons to find episodes belonging to the show.
pub async fn list_for_show<'e, E>(
    exec: E,
    show_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<TvWatchStateRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT ws.id, ws.episode_id, ws.user_id, ws.watched, ws.watched_at,
               ws.position_seconds, ws.rating, ws.created_at, ws.updated_at
        FROM tv_watch_states ws
        JOIN tv_episodes e ON e.id = ws.episode_id
        JOIN tv_seasons s ON s.id = e.season_id
        WHERE s.show_id = $1 AND ws.user_id = $2
        "#,
        show_id,
        user_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| TvWatchStateRow {
            id: r.id,
            episode_id: r.episode_id,
            user_id: r.user_id,
            watched: r.watched,
            watched_at: r.watched_at,
            position_seconds: r.position_seconds,
            rating: r.rating,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect())
}

/// Fetch all watch states for a season's episodes (for a given user).
pub async fn list_for_season<'e, E>(
    exec: E,
    season_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<TvWatchStateRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT ws.id, ws.episode_id, ws.user_id, ws.watched, ws.watched_at,
               ws.position_seconds, ws.rating, ws.created_at, ws.updated_at
        FROM tv_watch_states ws
        JOIN tv_episodes e ON e.id = ws.episode_id
        WHERE e.season_id = $1 AND ws.user_id = $2
        "#,
        season_id,
        user_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| TvWatchStateRow {
            id: r.id,
            episode_id: r.episode_id,
            user_id: r.user_id,
            watched: r.watched,
            watched_at: r.watched_at,
            position_seconds: r.position_seconds,
            rating: r.rating,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect())
}

/// Bulk mark all episodes in a show as watched for a user.
/// Uses a single INSERT ... SELECT ON CONFLICT to upsert all episode watch states.
/// Returns the list of affected episode IDs (for Trakt sync).
pub async fn bulk_mark_show_watched<'e, E>(
    exec: E,
    show_id: Uuid,
    user_id: Uuid,
    watched_at: OffsetDateTime,
) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO tv_watch_states (id, episode_id, user_id, watched, watched_at, position_seconds)
        SELECT gen_random_uuid(), e.id, $1, TRUE, $2, NULL
        FROM tv_episodes e
        JOIN tv_seasons s ON s.id = e.season_id
        WHERE s.show_id = $3
        ON CONFLICT (episode_id, user_id)
        DO UPDATE SET
            watched = TRUE,
            watched_at = $2,
            position_seconds = NULL,
            updated_at = NOW()
        RETURNING episode_id
        "#,
    )
    .bind(user_id)
    .bind(watched_at)
    .bind(show_id)
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Bulk mark all episodes in a show as unwatched for a user.
/// Only updates rows that are currently watched. Clears watched_at and position.
/// Returns the list of affected episode IDs (for Trakt sync).
pub async fn bulk_mark_show_unwatched<'e, E>(
    exec: E,
    show_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_scalar::<_, Uuid>(
        r#"
        UPDATE tv_watch_states ws
        SET watched = FALSE, watched_at = NULL, position_seconds = NULL, updated_at = NOW()
        FROM tv_episodes e
        JOIN tv_seasons s ON s.id = e.season_id
        WHERE ws.episode_id = e.id AND s.show_id = $1 AND ws.user_id = $2 AND ws.watched = TRUE
        RETURNING ws.episode_id
        "#,
    )
    .bind(show_id)
    .bind(user_id)
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Bulk mark all episodes in a season as watched for a user.
/// Returns the list of affected episode IDs (for Trakt sync).
pub async fn bulk_mark_season_watched<'e, E>(
    exec: E,
    season_id: Uuid,
    user_id: Uuid,
    watched_at: OffsetDateTime,
) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO tv_watch_states (id, episode_id, user_id, watched, watched_at, position_seconds)
        SELECT gen_random_uuid(), e.id, $1, TRUE, $2, NULL
        FROM tv_episodes e
        WHERE e.season_id = $3
        ON CONFLICT (episode_id, user_id)
        DO UPDATE SET
            watched = TRUE,
            watched_at = $2,
            position_seconds = NULL,
            updated_at = NOW()
        RETURNING episode_id
        "#,
    )
    .bind(user_id)
    .bind(watched_at)
    .bind(season_id)
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Bulk mark all episodes in a season as unwatched for a user.
/// Only updates rows that are currently watched. Clears watched_at and position.
/// Returns the list of affected episode IDs (for Trakt sync).
pub async fn bulk_mark_season_unwatched<'e, E>(
    exec: E,
    season_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_scalar::<_, Uuid>(
        r#"
        UPDATE tv_watch_states ws
        SET watched = FALSE, watched_at = NULL, position_seconds = NULL, updated_at = NOW()
        FROM tv_episodes e
        WHERE ws.episode_id = e.id AND e.season_id = $1 AND ws.user_id = $2 AND ws.watched = TRUE
        RETURNING ws.episode_id
        "#,
    )
    .bind(season_id)
    .bind(user_id)
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Bulk mark an explicit set of episodes as watched for a user.
/// Returns the affected episode IDs (for Trakt sync).
pub async fn bulk_mark_episodes_watched<'e, E>(
    exec: E,
    episode_ids: &[Uuid],
    user_id: Uuid,
    watched_at: OffsetDateTime,
) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_scalar!(
        r#"
        INSERT INTO tv_watch_states (id, episode_id, user_id, watched, watched_at, position_seconds)
        SELECT gen_random_uuid(), e.id, $1, TRUE, $2, NULL
        FROM tv_episodes e
        WHERE e.id = ANY($3)
        ON CONFLICT (episode_id, user_id)
        DO UPDATE SET
            watched = TRUE,
            watched_at = $2,
            position_seconds = NULL,
            updated_at = NOW()
        RETURNING episode_id
        "#,
        user_id,
        watched_at,
        episode_ids,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Bulk mark an explicit set of episodes as unwatched for a user.
/// Only touches rows that are currently watched. Returns the affected episode IDs.
pub async fn bulk_mark_episodes_unwatched<'e, E>(
    exec: E,
    episode_ids: &[Uuid],
    user_id: Uuid,
) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_scalar!(
        r#"
        UPDATE tv_watch_states
        SET watched = FALSE, watched_at = NULL, position_seconds = NULL, updated_at = NOW()
        WHERE episode_id = ANY($1) AND user_id = $2 AND watched = TRUE
        RETURNING episode_id
        "#,
        episode_ids,
        user_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows)
}

/// Update only the position_seconds field, preserving all other fields.
/// Creates the row if it doesn't exist (with watched = false).
pub async fn update_position<'e, E>(
    exec: E,
    episode_id: Uuid,
    user_id: Uuid,
    position_seconds: i32,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();

    let row = sqlx::query_scalar!(
        r#"
        INSERT INTO tv_watch_states (id, episode_id, user_id, watched, position_seconds)
        VALUES ($1, $2, $3, FALSE, $4)
        ON CONFLICT (episode_id, user_id)
        DO UPDATE SET
            position_seconds = EXCLUDED.position_seconds,
            updated_at = NOW()
        RETURNING id
        "#,
        id,
        episode_id,
        user_id,
        position_seconds,
    )
    .fetch_one(exec)
    .await?;

    Ok(row)
}
