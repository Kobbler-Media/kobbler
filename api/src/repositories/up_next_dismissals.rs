use super::Result;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

pub const MOVIE: &str = "movie";
pub const SHOW: &str = "show";
pub const STREMIO: &str = "stremio";

pub async fn dismiss<'e, E>(exec: E, user_id: Uuid, kind: &str, target: &str) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO up_next_dismissals (user_id, kind, target)
        VALUES ($1, $2, $3)
        ON CONFLICT (user_id, kind, target) DO UPDATE SET dismissed_at = NOW()
        "#,
        user_id,
        kind,
        target,
    )
    .execute(exec)
    .await?;
    Ok(())
}
