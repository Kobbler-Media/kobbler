use std::collections::HashSet;

use super::Result;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

pub async fn dismiss<'e, E>(exec: E, user_id: Uuid, media_type: &str, tmdb_id: &str) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO for_you_dismissals (user_id, media_type, tmdb_id)
        VALUES ($1, $2, $3)
        ON CONFLICT DO NOTHING
        "#,
        user_id,
        media_type,
        tmdb_id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn undismiss<'e, E>(exec: E, user_id: Uuid, media_type: &str, tmdb_id: &str) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "DELETE FROM for_you_dismissals WHERE user_id = $1 AND media_type = $2 AND tmdb_id = $3",
        user_id,
        media_type,
        tmdb_id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn list<'e, E>(exec: E, user_id: Uuid) -> Result<HashSet<(String, String)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        "SELECT media_type, tmdb_id FROM for_you_dismissals WHERE user_id = $1",
        user_id,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.media_type, r.tmdb_id))
        .collect())
}
