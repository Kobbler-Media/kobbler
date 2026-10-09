//! Notification send job handler.
//!
//! Dispatches a notification payload through a configured notification agent.
//! For web push agents, injects all active subscriptions and VAPID keys into
//! the agent config before sending.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::notifications::relevance::{self, ChatDelivery};
use crate::notifications::{
    NotificationAgentRegistry, NotificationPayload, NotificationTrigger, google_chat,
};
use crate::repositories::{notification_agents, webpush_subscriptions};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// Payload for notification_send jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct NotificationSendPayload {
    pub agent_id: Uuid,
    pub payload: NotificationPayload,
}

pub struct NotificationSendHandler {
    pool: PgPool,
}

impl NotificationSendHandler {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl JobHandler for NotificationSendHandler {
    fn job_type(&self) -> &'static str {
        "notification_send"
    }

    fn max_concurrent(&self) -> usize {
        2
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: NotificationSendPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {e}")))?;
        nats_jobs::check_cancelled(&cancel)?;

        info!(
            job_id = %job_id,
            agent_id = %payload.agent_id,
            trigger = ?payload.payload.trigger,
            "Sending notification"
        );

        // Load the agent from the database
        let agent_row = notification_agents::get_by_id(&self.pool, payload.agent_id)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;

        let agent_row = match agent_row {
            Some(row) => row,
            None => {
                info!(agent_id = %payload.agent_id, "Agent not found, skipping");
                return Ok(json!({ "skipped": true, "reason": "agent not found" }));
            }
        };

        if !agent_row.enabled {
            info!(agent_id = %payload.agent_id, name = %agent_row.name, "Agent disabled, skipping");
            return Ok(json!({ "skipped": true, "reason": "agent disabled" }));
        }

        // Build the effective config (may need enrichment for webpush)
        let effective_config = if agent_row.agent_type == "webpush" {
            enrich_webpush_config(&self.pool, &agent_row.config, payload.payload.trigger).await?
        } else {
            agent_row.config.clone()
        };

        // The chat agent is the only one that is relevance-scoped. Every other
        // agent — Pushover, Plex, web push — falls straight through to the
        // global fan-out below, unchanged.
        if agent_row.agent_type == google_chat::INFO.id {
            let delivery = relevance::chat_delivery(
                &self.pool,
                google_chat::INFO.id,
                payload.payload.entity_id,
                payload.payload.trigger,
                time::OffsetDateTime::now_utc(),
            )
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;

            if let ChatDelivery::Scoped(recipients) = delivery {
                let interested = recipients.len();
                let outcome =
                    google_chat::send_scoped(&effective_config, &recipients, &payload.payload)
                        .await
                        .map_err(|e| {
                            JobError::retryable(format!(
                                "Failed to send notification via {}: {e}",
                                agent_row.name
                            ))
                        })?;

                info!(
                    job_id = %job_id,
                    agent_id = %payload.agent_id,
                    interested,
                    delivered = outcome.delivered,
                    "Chat notification scoped to interested users"
                );
                return Ok(json!({
                    "agent_id": payload.agent_id,
                    "agent_name": agent_row.name,
                    "agent_type": agent_row.agent_type,
                    "trigger": payload.payload.trigger,
                    "scoped": true,
                    "interested": interested,
                    "delivered": outcome.delivered,
                    "unreachable": outcome.unreachable,
                    "failed": outcome.failed,
                }));
            }
        }

        // Create the agent via the registry
        let registry = NotificationAgentRegistry::new();
        let agent = registry
            .create_agent(
                &agent_row.agent_type,
                &effective_config,
                &self.pool,
                payload.agent_id,
            )
            .ok_or_else(|| {
                JobError::ExecutionFailed(format!("Unknown agent type: {}", agent_row.agent_type))
            })?;

        // Send the notification
        match agent.send(&payload.payload).await {
            Ok(()) => {
                info!(
                    job_id = %job_id,
                    agent_id = %payload.agent_id,
                    name = %agent_row.name,
                    "Notification sent successfully"
                );
                Ok(json!({
                    "agent_id": payload.agent_id,
                    "agent_name": agent_row.name,
                    "agent_type": agent_row.agent_type,
                    "trigger": payload.payload.trigger,
                }))
            }
            Err(e) => {
                warn!(
                    job_id = %job_id,
                    agent_id = %payload.agent_id,
                    error = %e,
                    "Notification send failed"
                );
                Err(JobError::retryable(format!(
                    "Failed to send notification via {}: {e}",
                    agent_row.name
                )))
            }
        }
    }
}

/// Enrich a webpush agent config with subscription list and VAPID keys.
///
/// An import means "something is available to watch", so it reaches every
/// subscriber. Grabs and failures are operational and stay with the people who
/// can act on them.
async fn enrich_webpush_config(
    pool: &PgPool,
    base_config: &Value,
    trigger: NotificationTrigger,
) -> JobResult<Value> {
    let subscriptions = match trigger {
        NotificationTrigger::OnImport => webpush_subscriptions::list_all(pool).await,
        _ => webpush_subscriptions::list_for_roles(pool, &["admin", "contributor"]).await,
    }
    .map_err(|e| JobError::ExecutionFailed(format!("Failed to load subscriptions: {e}")))?;

    let subs_json: Vec<Value> = subscriptions
        .into_iter()
        .map(|s| {
            json!({
                "endpoint": s.endpoint,
                "p256dh": s.p256dh,
                "auth": s.auth,
            })
        })
        .collect();

    // Fetch VAPID keys from system_config
    let vapid = sqlx::query!(
        r#"
        SELECT vapid_private_key, vapid_public_key
        FROM system_config
        WHERE id = 1
        "#,
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| JobError::ExecutionFailed(format!("Failed to load VAPID keys: {e}")))?;

    let mut config = base_config.as_object().cloned().unwrap_or_default();

    config.insert("subscriptions".to_string(), json!(subs_json));

    if let Some(row) = vapid {
        if let Some(private_key) = row.vapid_private_key {
            config.insert("vapid_private_key".to_string(), json!(private_key));
        }
        if let Some(public_key) = row.vapid_public_key {
            config.insert("vapid_public_key".to_string(), json!(public_key));
        }
    }

    Ok(Value::Object(config))
}
