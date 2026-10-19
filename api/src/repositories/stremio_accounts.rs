use serde::Serialize;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

use super::Result;

pub struct SyncTarget {
    pub id: Uuid,
    pub auth_key: String,
}

#[derive(Serialize)]
pub struct AccountStatus {
    pub email: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_synced_at: Option<OffsetDateTime>,
    pub last_error: Option<String>,
    pub item_count: i64,
}

pub async fn instance_status<'e, E>(exec: E) -> Result<Option<AccountStatus>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_as!(
        AccountStatus,
        r#"
        SELECT a.email, a.last_synced_at, a.last_error,
               (SELECT COUNT(*) FROM stremio_continue_items i WHERE i.account_id = a.id) AS "item_count!"
        FROM stremio_accounts a
        WHERE a.user_id IS NULL
        "#,
    )
    .fetch_optional(exec)
    .await?)
}

pub async fn upsert_instance<'e, E>(exec: E, email: &str, auth_key: &str) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_scalar!(
        r#"
        INSERT INTO stremio_accounts (id, user_id, email, auth_key)
        VALUES ($1, NULL, $2, $3)
        ON CONFLICT ((user_id IS NULL)) WHERE user_id IS NULL
        DO UPDATE SET email = EXCLUDED.email, auth_key = EXCLUDED.auth_key,
                      last_error = NULL, updated_at = NOW()
        RETURNING id
        "#,
        Uuid::now_v7(),
        email,
        auth_key,
    )
    .fetch_one(exec)
    .await?)
}

pub async fn delete_instance<'e, E>(exec: E) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM stremio_accounts WHERE user_id IS NULL")
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn list_sync_targets<'e, E>(exec: E) -> Result<Vec<SyncTarget>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_as!(
        SyncTarget,
        "SELECT id, auth_key FROM stremio_accounts ORDER BY id"
    )
    .fetch_all(exec)
    .await?)
}

pub async fn record_sync<'e, E>(exec: E, id: Uuid, error: Option<&str>) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE stremio_accounts
        SET last_error = $2,
            last_synced_at = CASE WHEN $2::text IS NULL THEN NOW() ELSE last_synced_at END,
            updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        error,
    )
    .execute(exec)
    .await?;
    Ok(())
}
