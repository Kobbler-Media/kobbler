//! Movie watch states repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Input for creating or updating a watch state.
#[derive(Debug)]
pub struct WatchStateInput {
    pub movie_id: Uuid,
    pub user_id: Uuid,
    pub watched: bool,
    pub watched_at: Option<OffsetDateTime>,
    pub position_seconds: Option<i32>,
    pub rating: Option<f64>,
}

/// Row returned from movie_watch_states queries.
#[derive(Debug)]
pub struct WatchStateRow {
    pub id: Uuid,
    pub movie_id: Uuid,
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

/// Get a watch state for a specific movie and user.
pub async fn get<'e, E>(exec: E, movie_id: Uuid, user_id: Uuid) -> Result<Option<WatchStateRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, movie_id, user_id, watched, watched_at,
               position_seconds, rating, created_at, updated_at
        FROM movie_watch_states
        WHERE movie_id = $1 AND user_id = $2
        "#,
        movie_id,
        user_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| WatchStateRow {
        id: r.id,
        movie_id: r.movie_id,
        user_id: r.user_id,
        watched: r.watched,
        watched_at: r.watched_at,
        position_seconds: r.position_seconds,
        rating: r.rating,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }))
}

/// Upsert a watch state (insert or update on conflict of movie_id + user_id).
pub async fn upsert<'e, E>(exec: E, input: WatchStateInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();

    let row = sqlx::query_scalar!(
        r#"
        INSERT INTO movie_watch_states (id, movie_id, user_id, watched, watched_at,
                                        position_seconds, rating)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (movie_id, user_id)
        DO UPDATE SET
            source = 'local',
            watched = EXCLUDED.watched,
            watched_at = COALESCE(EXCLUDED.watched_at, movie_watch_states.watched_at),
            position_seconds = COALESCE(EXCLUDED.position_seconds, movie_watch_states.position_seconds),
            rating = COALESCE(EXCLUDED.rating, movie_watch_states.rating),
            updated_at = NOW()
        RETURNING id
        "#,
        id,
        input.movie_id,
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

/// A watch recorded somewhere else. It never clears or backdates what the library
/// already knows, and a row the library wrote keeps its source.
pub async fn upsert_imported<'e, E>(
    exec: E,
    movie_id: Uuid,
    user_id: Uuid,
    watched_at: Option<OffsetDateTime>,
    source: &str,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!(
        r#"
        INSERT INTO movie_watch_states (id, movie_id, user_id, watched, watched_at, source)
        VALUES ($1, $2, $3, TRUE, $4, $5)
        ON CONFLICT (movie_id, user_id)
        DO UPDATE SET
            watched = TRUE,
            watched_at = GREATEST(movie_watch_states.watched_at, EXCLUDED.watched_at),
            updated_at = NOW()
        RETURNING id
        "#,
        Uuid::now_v7(),
        movie_id,
        user_id,
        watched_at,
        source,
    )
    .fetch_one(exec)
    .await?;
    Ok(row)
}

/// Unwatches every library copy of the given TMDB titles for one user.
pub async fn clear_for_tmdb<'e, E>(exec: E, user_id: Uuid, tmdb_ids: &[String]) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let done = sqlx::query!(
        r#"
        UPDATE movie_watch_states ws
        SET watched = FALSE, watched_at = NULL, position_seconds = NULL, updated_at = NOW()
        FROM movies m
        WHERE m.id = ws.movie_id
          AND ws.user_id = $1
          AND m.identifiers->>'tmdb_id' = ANY($2)
          AND (ws.watched OR ws.position_seconds IS NOT NULL)
        "#,
        user_id,
        tmdb_ids,
    )
    .execute(exec)
    .await?;
    Ok(done.rows_affected())
}

/// Fetch watch states for multiple movies (for a given user).
pub async fn list_for_movie_ids<'e, E>(
    exec: E,
    movie_ids: &[Uuid],
    user_id: Uuid,
) -> Result<Vec<WatchStateRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, movie_id, user_id, watched, watched_at,
               position_seconds, rating, created_at, updated_at
        FROM movie_watch_states
        WHERE movie_id = ANY($1) AND user_id = $2
        "#,
        movie_ids,
        user_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| WatchStateRow {
            id: r.id,
            movie_id: r.movie_id,
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

/// Update only the position_seconds field, preserving all other fields.
/// Creates the row if it doesn't exist (with watched = false).
pub async fn update_position<'e, E>(
    exec: E,
    movie_id: Uuid,
    user_id: Uuid,
    position_seconds: i32,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();

    let row = sqlx::query_scalar!(
        r#"
        INSERT INTO movie_watch_states (id, movie_id, user_id, watched, position_seconds)
        VALUES ($1, $2, $3, FALSE, $4)
        ON CONFLICT (movie_id, user_id)
        DO UPDATE SET
            source = 'local',
            position_seconds = EXCLUDED.position_seconds,
            updated_at = NOW()
        RETURNING id
        "#,
        id,
        movie_id,
        user_id,
        position_seconds,
    )
    .fetch_one(exec)
    .await?;

    Ok(row)
}
