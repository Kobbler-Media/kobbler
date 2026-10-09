//! Sync log repository operations.

use super::Result;
use serde_json::Value;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

/// Input for creating a sync log entry.
#[derive(Debug)]
pub struct SyncLogInput {
    pub device_id: Uuid,
    pub book_id: Uuid,
    pub provider: String,
    pub action: String,
    pub payload: Option<Value>,
    pub response: Option<Value>,
    pub error: Option<String>,
}

/// Create a sync log entry.
pub async fn create<'e, E>(exec: E, input: SyncLogInput) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO sync_log (device_id, book_id, provider, action, payload, response, error)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
        input.device_id,
        input.book_id,
        input.provider,
        input.action,
        input.payload as Option<Value>,
        input.response as Option<Value>,
        input.error as Option<String>,
    )
    .execute(exec)
    .await?;

    Ok(())
}
