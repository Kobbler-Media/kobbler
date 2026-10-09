//! TV show ratings repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Row returned from tv_show_ratings queries.
#[derive(Debug)]
pub struct TvShowRatingRow {
    pub id: Uuid,
    pub show_id: Uuid,
    pub user_id: Uuid,
    pub rating: f64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

// =============================================================================
// Operations
// =============================================================================

/// Get a show rating for a specific show and user.
pub async fn get<'e, E>(exec: E, show_id: Uuid, user_id: Uuid) -> Result<Option<TvShowRatingRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, show_id, user_id, rating, created_at, updated_at
        FROM tv_show_ratings
        WHERE show_id = $1 AND user_id = $2
        "#,
        show_id,
        user_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| TvShowRatingRow {
        id: r.id,
        show_id: r.show_id,
        user_id: r.user_id,
        rating: r.rating,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }))
}

/// Upsert a show rating (insert or update on conflict of show_id + user_id).
pub async fn upsert<'e, E>(exec: E, show_id: Uuid, user_id: Uuid, rating: f64) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();

    let row = sqlx::query_scalar!(
        r#"
        INSERT INTO tv_show_ratings (id, show_id, user_id, rating)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (show_id, user_id)
        DO UPDATE SET
            rating = EXCLUDED.rating,
            updated_at = NOW()
        RETURNING id
        "#,
        id,
        show_id,
        user_id,
        rating,
    )
    .fetch_one(exec)
    .await?;

    Ok(row)
}

/// Delete a show rating for a specific show and user.
pub async fn delete<'e, E>(exec: E, show_id: Uuid, user_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        DELETE FROM tv_show_ratings
        WHERE show_id = $1 AND user_id = $2
        "#,
        show_id,
        user_id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}
