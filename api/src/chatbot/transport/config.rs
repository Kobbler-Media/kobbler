//! Where a transport's per-instance config comes from.
//!
//! There is one Google Chat app, and it needs both directions configured — the inbound
//! audience and the outbound service-account credential. Rather than hold a second copy
//! of the same key, the transport reads the `notification_agents` row for its own id,
//! which the settings page already renders from `ChatConfigField`-shaped schemas.
//! When a `chat_transports` table lands, this is the only function that changes.

use serde_json::Value;
use sqlx::PgPool;

use super::ChatError;
use crate::repositories::notification_agents;

pub async fn load(pool: &PgPool, transport: &str) -> Result<Value, ChatError> {
    let rows = notification_agents::list_all(pool)
        .await
        .map_err(|e| ChatError::Transport(format!("failed to read transport config: {e}")))?;

    rows.into_iter()
        .find(|r| r.agent_type == transport)
        .map(|r| r.config)
        .ok_or_else(|| ChatError::NotConfigured(transport.to_string()))
}
