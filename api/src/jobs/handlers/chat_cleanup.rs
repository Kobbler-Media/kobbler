//! Tier 2 session close and the three-tier retention sweep.
//!
//! Both halves live in one job type because they are the same housekeeping pass
//! at two cadences: `sessions` runs every few minutes to close what has gone
//! quiet, `retention` runs nightly to drop what is past its window. The payload
//! chooses; `nats.rs` registers one schedule for each.
//!
//! The summarising half issues LLM completions. Those go through koblem's
//! `acquire_for_llm` and a flux or songgen render can evict the model outright,
//! so this deliberately runs in a job with no clock on it. Nothing here is ever
//! on a live turn's path.

use crate::chatbot::llm::{self, LlmConfig};
use crate::chatbot::memory::{self, IDLE_MINUTES};
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::chat_memory as memory_repo;
use crate::repositories::chat_memory::{MESSAGE_RETENTION_DAYS, SUMMARY_RETENTION_DAYS};
use crate::runtime_config::RuntimeConfigManager;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChatCleanupMode {
    /// Close idle sessions and summarise the ones that earned it.
    Sessions,
    /// Drop raw turns past 30 days and whole sessions past a year.
    Retention,
    #[default]
    All,
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct ChatCleanupPayload {
    #[serde(default)]
    mode: ChatCleanupMode,
    #[serde(default = "default_message_days")]
    message_retention_days: i32,
    #[serde(default = "default_summary_days")]
    summary_retention_days: i32,
    #[serde(default = "default_idle_minutes")]
    idle_minutes: i32,
}

fn default_message_days() -> i32 {
    MESSAGE_RETENTION_DAYS
}

fn default_summary_days() -> i32 {
    SUMMARY_RETENTION_DAYS
}

fn default_idle_minutes() -> i32 {
    IDLE_MINUTES
}

pub struct ChatCleanupHandler {
    pool: PgPool,
    runtime_config: RuntimeConfigManager,
}

impl ChatCleanupHandler {
    pub fn new(pool: PgPool, runtime_config: RuntimeConfigManager) -> Self {
        Self {
            pool,
            runtime_config,
        }
    }

    /// The configured provider, or `None` when the box has no LLM wired up. A
    /// missing provider closes sessions silently rather than failing the job —
    /// leaving them open forever would be the worse outcome.
    async fn llm(&self) -> Option<LlmConfig> {
        match llm::resolve(&self.runtime_config.get().await) {
            Ok(config) => Some(config),
            Err(e) => {
                warn!("chat session summaries skipped: {e}");
                None
            }
        }
    }
}

#[async_trait]
impl JobHandler for ChatCleanupHandler {
    fn job_type(&self) -> &'static str {
        "chat_cleanup"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        nats_jobs::check_cancelled(&cancel)?;
        let payload: ChatCleanupPayload = serde_json::from_value(payload).unwrap_or_default();

        let mut result = json!({"mode": payload.mode});

        if matches!(
            payload.mode,
            ChatCleanupMode::Sessions | ChatCleanupMode::All
        ) {
            let config = self.llm().await;
            let report =
                memory::sweep_idle_sessions(&self.pool, config.as_ref(), payload.idle_minutes)
                    .await
                    .map_err(|e| {
                        JobError::ExecutionFailed(format!("chat session sweep failed: {e}"))
                    })?;
            if report.closed > 0 {
                info!(
                    closed = report.closed,
                    summarised = report.summarised,
                    facts_added = report.facts_added,
                    "Closed idle chat sessions"
                );
            }
            result["closed"] = json!(report.closed);
            result["summarised"] = json!(report.summarised);
            result["facts_added"] = json!(report.facts_added);
        }

        nats_jobs::check_cancelled(&cancel)?;

        if matches!(
            payload.mode,
            ChatCleanupMode::Retention | ChatCleanupMode::All
        ) {
            let messages = memory_repo::delete_messages_older_than_days(
                &self.pool,
                payload.message_retention_days.max(1),
            )
            .await
            .map_err(JobError::Repository)?;
            let sessions = memory_repo::delete_sessions_older_than_days(
                &self.pool,
                payload.summary_retention_days.max(1),
            )
            .await
            .map_err(JobError::Repository)?;

            if messages > 0 || sessions > 0 {
                info!(
                    messages = messages,
                    sessions = sessions,
                    "Swept old chat conversations"
                );
            }
            result["messages_deleted"] = json!(messages);
            result["sessions_deleted"] = json!(sessions);
            // Profile facts are kept until their owner removes them, and saying
            // so in the result is cheaper than someone re-deriving it later.
            result["facts_deleted"] = json!(0);
        }

        Ok(result)
    }
}
