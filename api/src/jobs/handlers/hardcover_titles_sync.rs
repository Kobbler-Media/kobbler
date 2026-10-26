use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

use crate::hardcover_titles::{self, SyncPlan};
use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::metadata::hardcover::HardcoverProvider;
use crate::repositories::{hardcover_titles as repo, metadata_providers};
use crate::runtime_config::RuntimeConfigManager;

const SYNC_EVERY: Duration = Duration::days(7);
const RETRY_AFTER: Duration = Duration::days(1);

/// Ticks hourly because the scheduler forgets its timers on every restart; the stored
/// attempt and sync times are what actually pace the work.
pub struct HardcoverTitlesSyncHandler {
    pool: PgPool,
    runtime_config: RuntimeConfigManager,
    plan: SyncPlan,
}

impl HardcoverTitlesSyncHandler {
    pub fn new(pool: PgPool, runtime_config: RuntimeConfigManager, plan: SyncPlan) -> Self {
        Self {
            pool,
            runtime_config,
            plan,
        }
    }
}

#[async_trait]
impl JobHandler for HardcoverTitlesSyncHandler {
    fn job_type(&self) -> &'static str {
        "hardcover_titles_sync"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        _payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        if !self
            .runtime_config
            .get()
            .await
            .book_title_suggestions_enabled
        {
            return Ok(json!({ "status": "skipped", "reason": "disabled" }));
        }
        let hardcover = metadata_providers::get_config(&self.pool, "hardcover")
            .await?
            .map(|config| HardcoverProvider::from_config(crate::http::new_client(), &config))
            .filter(HardcoverProvider::has_api_key);
        let Some(hardcover) = hardcover else {
            return Ok(json!({ "status": "skipped", "reason": "no_hardcover_key" }));
        };
        let now = OffsetDateTime::now_utc();
        let state = repo::sync_state(&self.pool).await?;
        if state.synced_at.is_some_and(|at| at > now - SYNC_EVERY) {
            return Ok(json!({ "status": "current" }));
        }
        if state.attempted_at.is_some_and(|at| at > now - RETRY_AFTER) {
            return Ok(json!({ "status": "backoff" }));
        }
        if !self.plan.off_peak.contains(now) {
            return Ok(json!({ "status": "off_peak_wait" }));
        }
        repo::mark_attempted(&self.pool).await?;
        let titles =
            hardcover_titles::most_read(&self.pool, &hardcover, self.plan, &cancel).await?;
        repo::replace(&self.pool, &titles).await?;
        info!(titles = titles.len(), "Imported Hardcover most-read titles");
        Ok(json!({ "status": "imported", "titles": titles.len() }))
    }
}
