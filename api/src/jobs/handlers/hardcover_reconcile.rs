//! Follow Hardcover's merges and deletions (see `tracking::reconcile`).
//!
//! With no payload it walks everything linked to Hardcover; scheduled daily. With
//! `{"entity_type", "entity_id"}` it settles one entity, which the author and series
//! syncs enqueue when Hardcover stops returning something they asked for.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::metadata_providers;
use crate::repositories::provider_reconcile::{self as repo, EntityKind};
use crate::tracking::HardcoverAuthorProvider;
use crate::tracking::reconcile::{self, PROVIDER};

#[derive(Debug, Default, Deserialize)]
struct HardcoverReconcilePayload {
    entity_type: Option<String>,
    entity_id: Option<Uuid>,
}

pub struct HardcoverReconcileHandler {
    pool: PgPool,
    nats_publisher: nats_jobs::Publisher,
}

impl HardcoverReconcileHandler {
    pub fn new(pool: PgPool, nats_publisher: nats_jobs::Publisher) -> Self {
        Self {
            pool,
            nats_publisher,
        }
    }
}

fn entity_kind(s: &str) -> Option<EntityKind> {
    match s {
        "author" => Some(EntityKind::Author),
        "series" => Some(EntityKind::Series),
        "book" => Some(EntityKind::Book),
        _ => None,
    }
}

#[async_trait]
impl JobHandler for HardcoverReconcileHandler {
    fn job_type(&self) -> &'static str {
        "hardcover_reconcile"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        nats_jobs::check_cancelled(&cancel)?;
        let payload: HardcoverReconcilePayload =
            serde_json::from_value(payload).unwrap_or_default();

        let Some(api_key) = metadata_providers::get_api_key(&self.pool, PROVIDER).await? else {
            return Ok(json!({ "status": "skipped", "reason": "no hardcover api key" }));
        };
        let hc = HardcoverAuthorProvider::new(api_key);

        if let (Some(entity_type), Some(entity_id)) = (payload.entity_type, payload.entity_id) {
            let kind = entity_kind(&entity_type).ok_or_else(|| {
                JobError::ExecutionFailed(format!("Unknown entity_type: {entity_type}"))
            })?;
            let linked = repo::list_linked_one(&self.pool, kind, entity_id, PROVIDER).await?;
            let Some(external_id) = linked else {
                return Ok(json!({ "status": "skipped", "reason": "not linked to hardcover" }));
            };
            let remote = reconcile::reconcile_entity(
                &self.pool,
                &self.nats_publisher,
                &hc,
                kind,
                entity_id,
                external_id,
            )
            .await
            .map_err(JobError::retryable)?;
            return Ok(json!({
                "entity_type": entity_type,
                "entity_id": entity_id,
                "remote": format!("{remote:?}"),
            }));
        }

        let report = reconcile::run(&self.pool, &self.nats_publisher, &hc)
            .await
            .map_err(JobError::retryable)?;
        info!(job_id = %job_id, report = ?report, "Hardcover reconcile complete");
        Ok(serde_json::to_value(&report).unwrap_or_default())
    }
}
