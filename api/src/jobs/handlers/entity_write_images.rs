//! Per-entity image write job handler.
//!
//! Downloads and writes images for a single entity (movie, TV show, or book).
//! Decoupled from enrichment so metadata appears in the UI immediately while
//! images download asynchronously.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityWriteImagesPayload {
    pub entity_type: String,
    pub entity_id: Uuid,
}

pub struct EntityWriteImagesHandler {
    pool: PgPool,
    storage_root: String,
    fs_semaphore: Arc<Semaphore>,
    runtime_config: crate::RuntimeConfigManager,
}

impl EntityWriteImagesHandler {
    pub fn new(
        pool: PgPool,
        storage_root: String,
        fs_semaphore: Arc<Semaphore>,
        runtime_config: crate::RuntimeConfigManager,
    ) -> Self {
        Self {
            pool,
            storage_root,
            fs_semaphore,
            runtime_config,
        }
    }
}

#[async_trait]
impl JobHandler for EntityWriteImagesHandler {
    fn job_type(&self) -> &'static str {
        "entity_write_images"
    }

    fn max_concurrent(&self) -> usize {
        self.runtime_config
            .job_concurrency("entity_write_images", 4)
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        nats_jobs::check_cancelled(&cancel)?;
        let _fs_permit = self
            .fs_semaphore
            .acquire()
            .await
            .map_err(|_| JobError::ExecutionFailed("FS semaphore closed".into()))?;
        let payload: EntityWriteImagesPayload = serde_json::from_value(payload)?;

        match payload.entity_type.as_str() {
            "tv_show" => {
                crate::sidecar::write_back::write_tvshow_images(
                    &self.pool,
                    &self.storage_root,
                    payload.entity_id,
                )
                .await;
            }
            "movie" => {
                crate::sidecar::write_back::write_movie_images(
                    &self.pool,
                    &self.storage_root,
                    payload.entity_id,
                )
                .await;
            }
            "book" => {
                crate::sidecar::write_back::write_book_images(
                    &self.pool,
                    &self.storage_root,
                    payload.entity_id,
                )
                .await;
            }
            other => {
                info!(
                    entity_type = other,
                    "Unknown entity type for image write, skipping"
                );
            }
        }

        Ok(json!({
            "entity_type": payload.entity_type,
            "entity_id": payload.entity_id,
        }))
    }
}
