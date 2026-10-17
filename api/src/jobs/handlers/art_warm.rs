//! Daily sweep that puts library art on disk for titles enriched before warming happened
//! at enrichment time, or whose download failed then. Selects only what is not cached
//! yet, so a steady-state run touches nothing; a large backlog drains over several days,
//! newest titles first, at the pace of [`images::ART_DOWNLOAD_PACE`].

use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

use crate::images::{self, ImageType};
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};

pub struct ArtWarmHandler {
    pool: PgPool,
    storage_root: String,
}

impl ArtWarmHandler {
    pub fn new(pool: PgPool, storage_root: String) -> Self {
        Self { pool, storage_root }
    }

    async fn uncached(&self, sql: &'static str) -> JobResult<Vec<Uuid>> {
        sqlx::query_scalar::<_, Uuid>(sql)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to list uncached art: {e}")))
    }
}

// Per kind, per run: two images a title at the download pace is a few minutes of work.
const UNCACHED_MOVIES: &str = "SELECT id FROM movies m
    WHERE EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = m.id)
      AND ((m.images->'backdrop'->>'source_url' IS NOT NULL AND m.images->'backdrop'->>'cached_at' IS NULL)
        OR (m.images->'poster'->>'source_url' IS NOT NULL AND m.images->'poster'->>'cached_at' IS NULL))
    ORDER BY m.created_at DESC
    LIMIT 150";

const UNCACHED_SHOWS: &str = "SELECT id FROM tv_shows s
    WHERE (s.images->'backdrop'->>'source_url' IS NOT NULL AND s.images->'backdrop'->>'cached_at' IS NULL)
       OR (s.images->'poster'->>'source_url' IS NOT NULL AND s.images->'poster'->>'cached_at' IS NULL)
    ORDER BY s.created_at DESC
    LIMIT 150";

#[async_trait]
impl JobHandler for ArtWarmHandler {
    fn job_type(&self) -> &'static str {
        "art_warm"
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
        let batches: [(&str, Vec<Uuid>, &[(&str, ImageType)]); 2] = [
            (
                "movies",
                self.uncached(UNCACHED_MOVIES).await?,
                images::MOVIE_ART,
            ),
            (
                "tv_shows",
                self.uncached(UNCACHED_SHOWS).await?,
                images::SHOW_ART,
            ),
        ];
        let mut warmed = 0;
        for (table, ids, keys) in batches {
            for id in ids {
                nats_jobs::check_cancelled(&cancel)?;
                images::warm_entity_images(&self.pool, &self.storage_root, table, id, keys).await;
                warmed += 1;
            }
        }
        info!(titles = warmed, "Art warm sweep complete");
        Ok(json!({ "titles": warmed }))
    }
}
