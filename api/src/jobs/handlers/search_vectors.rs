//! Keeps the search indexes' artwork vectors in step with `semantic_search_enabled`.
//!
//! Turning semantic search on embeds every title's artwork through koblem, rewrites
//! the title indexes with `_vectors`, then declares the embedder — in that order,
//! because Meili refuses an embedder while any document lacks its vector. Turning it
//! off removes the embedder, then rewrites the indexes without vectors. Between those
//! transitions a run only embeds artwork that is new or changed.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use crate::images::{self, ImageEntry, ImageType};
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::search_embeddings::{self, EmbeddedKind, StaleEmbedding};
use crate::runtime_config::RuntimeConfigManager;
use crate::search::{self, SearchBackend, semantic};
use crate::skip_detect::vision::VisionClient;

pub const JOB_TYPE: &str = "search_vectors";

const EMBED_BATCH: usize = 16;

pub struct SearchVectorsHandler {
    pool: PgPool,
    search: Arc<dyn SearchBackend>,
    runtime_config: RuntimeConfigManager,
    storage_root: String,
}

impl SearchVectorsHandler {
    pub fn new(
        pool: PgPool,
        search: Arc<dyn SearchBackend>,
        runtime_config: RuntimeConfigManager,
        storage_root: String,
    ) -> Self {
        Self {
            pool,
            search,
            runtime_config,
            storage_root,
        }
    }

    async fn reindex_titles(&self) -> JobResult<()> {
        let search = self.search.as_ref();
        search::semantic::reindex_movies(search, &self.pool)
            .await
            .map_err(|e| JobError::retryable(format!("Failed to reindex movies: {e}")))?;
        search::semantic::reindex_tv_shows(search, &self.pool)
            .await
            .map_err(|e| JobError::retryable(format!("Failed to reindex TV shows: {e}")))?;
        search::semantic::reindex_books(search, &self.pool)
            .await
            .map_err(|e| JobError::retryable(format!("Failed to reindex books: {e}")))?;
        Ok(())
    }

    async fn reindex_one(&self, kind: EmbeddedKind, id: Uuid) -> JobResult<()> {
        let search = self.search.as_ref();
        let result = match kind {
            EmbeddedKind::Movie => search::index_movie(search, &self.pool, id).await,
            EmbeddedKind::TvShow => search::index_tv_show(search, &self.pool, id).await,
            EmbeddedKind::Book => search::index_book(search, &self.pool, id).await,
        };
        result.map_err(|e| JobError::retryable(format!("Failed to index {kind:?} {id}: {e}")))
    }

    async fn load_artwork(&self, title: &StaleEmbedding) -> Option<Vec<u8>> {
        let (table, key, image_type) = match title.kind {
            EmbeddedKind::Movie => ("movies", "poster", ImageType::MoviePoster),
            EmbeddedKind::TvShow => ("tv_shows", "poster", ImageType::TvShowPoster),
            EmbeddedKind::Book => {
                return images::load_book_cover(&self.pool, &self.storage_root, title.entity_id)
                    .await;
            }
        };
        let entry: ImageEntry = serde_json::from_str(&title.source).ok()?;
        let image_map = HashMap::from([(key.to_string(), entry)]);
        images::load_entity_image(
            &self.pool,
            &self.storage_root,
            table,
            key,
            image_type,
            title.entity_id,
            &image_map,
        )
        .await
        .map_err(|e| warn!(entity_id = %title.entity_id, error = %e, "Artwork unavailable for embedding"))
        .ok()
    }

    /// Embeds every stale title's artwork and returns the titles that got a new vector.
    async fn embed_stale(
        &self,
        koblem_url: &str,
        cancel: &CancellationToken,
    ) -> JobResult<Vec<(EmbeddedKind, Uuid)>> {
        let stale = search_embeddings::stale(&self.pool)
            .await
            .map_err(|e| JobError::retryable(format!("Failed to list stale artwork: {e}")))?;
        let vision = VisionClient::new(koblem_url);
        let mut embedded = Vec::new();

        for batch in stale.chunks(EMBED_BATCH) {
            nats_jobs::check_cancelled(cancel)?;
            let mut titles = Vec::with_capacity(batch.len());
            let mut artwork = Vec::with_capacity(batch.len());
            for title in batch {
                if let Some(bytes) = self.load_artwork(title).await {
                    titles.push(title);
                    artwork.push(bytes);
                }
            }
            if artwork.is_empty() {
                continue;
            }
            let vectors = vision
                .embed_images(&artwork)
                .await
                .map_err(|e| JobError::retryable(format!("koblem image embedding failed: {e}")))?;
            if vectors.len() != titles.len() {
                return Err(JobError::retryable(format!(
                    "koblem returned {} embeddings for {} images",
                    vectors.len(),
                    titles.len()
                )));
            }
            for (title, vector) in titles.into_iter().zip(vectors) {
                if vector.len() != semantic::DIMENSIONS {
                    return Err(JobError::ExecutionFailed(format!(
                        "koblem returned a {}-dimensional embedding, expected {}",
                        vector.len(),
                        semantic::DIMENSIONS
                    )));
                }
                let stored =
                    search_embeddings::upsert(&self.pool, title.kind, title.entity_id, &vector)
                        .await
                        .map_err(|e| JobError::retryable(format!("Failed to store vector: {e}")))?;
                if stored {
                    embedded.push((title.kind, title.entity_id));
                }
            }
        }
        Ok(embedded)
    }
}

#[async_trait]
impl JobHandler for SearchVectorsHandler {
    fn job_type(&self) -> &'static str {
        JOB_TYPE
    }

    async fn execute(
        &self,
        job_id: Uuid,
        _payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        if !self.search.supports_semantic() {
            return Ok(json!({"skipped": "the search backend has no vector store"}));
        }
        let config = self.runtime_config.get().await;
        let enabled = config.semantic_search_enabled;
        self.search.set_semantic_enabled(enabled);
        let configured = self
            .search
            .embedders_configured()
            .await
            .map_err(|e| JobError::retryable(format!("Failed to read embedders: {e}")))?;

        if !enabled {
            if !configured {
                return Ok(json!({"enabled": false, "changed": false}));
            }
            self.search
                .apply_embedders(false)
                .await
                .map_err(|e| JobError::retryable(e.to_string()))?;
            self.reindex_titles().await?;
            info!(job_id = %job_id, "Semantic search turned off; vectors removed");
            return Ok(json!({"enabled": false, "changed": true}));
        }

        let koblem_url = semantic::koblem_url(&config).ok_or_else(|| {
            JobError::ExecutionFailed(
                "semantic search is on but koblem_service_url is not set".to_string(),
            )
        })?;

        let pruned = search_embeddings::prune(&self.pool)
            .await
            .map_err(|e| JobError::retryable(format!("Failed to prune vectors: {e}")))?;
        let embedded = self.embed_stale(&koblem_url, &cancel).await?;

        if configured {
            for (kind, id) in pruned
                .iter()
                .map(|t| (t.kind, t.entity_id))
                .chain(embedded.iter().copied())
            {
                self.reindex_one(kind, id).await?;
            }
        } else {
            self.reindex_titles().await?;
            self.search
                .apply_embedders(true)
                .await
                .map_err(|e| JobError::retryable(e.to_string()))?;
        }

        info!(
            job_id = %job_id,
            embedded = embedded.len(),
            pruned = pruned.len(),
            reconfigured = !configured,
            "Search vectors up to date"
        );
        Ok(json!({
            "enabled": true,
            "changed": !configured,
            "embedded": embedded.len(),
            "pruned": pruned.len(),
        }))
    }
}
