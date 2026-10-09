//! Media root delete job handler.
//!
//! Deletes a media root and all associated library items (TV shows, movies, books).
//! FK cascades handle child entities (seasons, episodes, files, entitlements, etc.).
//! After deletion, cleans up orphaned entities and the search index.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{
    authors, books, genres, media_roots, movies, people, series, tv_shows, wanted_attempts,
};
use crate::search::SearchBackend;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRootDeletePayload {
    pub media_root_id: Uuid,
}

pub struct MediaRootDeleteHandler {
    pool: PgPool,
    search: Arc<dyn SearchBackend>,
}

impl MediaRootDeleteHandler {
    pub fn new(pool: PgPool, search: Arc<dyn SearchBackend>) -> Self {
        Self { pool, search }
    }
}

#[async_trait]
impl JobHandler for MediaRootDeleteHandler {
    fn job_type(&self) -> &'static str {
        "media_root_delete"
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: MediaRootDeletePayload = serde_json::from_value(payload)?;

        let root = media_roots::get_by_id(&self.pool, payload.media_root_id)
            .await?
            .ok_or_else(|| {
                JobError::ExecutionFailed(format!("Media root {} not found", payload.media_root_id))
            })?;

        // Delete associated library items and collect IDs for search cleanup
        let deleted_ids: Vec<Uuid> = match root.media_type.as_str() {
            "tv" => tv_shows::delete_by_media_root(&self.pool, root.id)
                .await
                .unwrap_or_default(),
            "movie" => movies::delete_by_media_root(&self.pool, root.id)
                .await
                .unwrap_or_default(),
            "book" => books::delete_by_media_root(&self.pool, root.id)
                .await
                .unwrap_or_default(),
            _ => vec![],
        };

        info!(
            root = %root.name,
            media_type = %root.media_type,
            deleted = deleted_ids.len(),
            "Deleted library items for media root"
        );

        // Remove deleted items from search index
        for id in &deleted_ids {
            nats_jobs::check_cancelled(&cancel)?;
            let result = match root.media_type.as_str() {
                "tv" => self.search.delete_tv_show(*id).await,
                "movie" => self.search.delete_movie(*id).await,
                "book" => self.search.delete_book(*id).await,
                _ => Ok(()),
            };
            if let Err(e) = result {
                tracing::warn!(id = %id, error = %e, "Failed to remove item from search index");
            }
        }

        // Delete the root itself (media_root_files cascade via FK)
        media_roots::delete(&self.pool, root.id).await?;

        info!(root = %root.name, "Media root deleted");

        // Clean up orphaned entities
        let orphaned_authors = authors::delete_orphaned(&self.pool)
            .await
            .unwrap_or_default();
        let orphaned_series = series::delete_orphaned(&self.pool)
            .await
            .unwrap_or_default();
        let orphaned_genres = genres::delete_orphaned(&self.pool)
            .await
            .unwrap_or_default();
        let orphaned_people = people::delete_orphaned(&self.pool)
            .await
            .unwrap_or_default();
        let orphaned_attempts = wanted_attempts::delete_orphaned(&self.pool)
            .await
            .unwrap_or(0);

        // Remove orphaned entities from search index
        for id in &orphaned_authors {
            if let Err(e) = self.search.delete_author(*id).await {
                tracing::warn!(id = %id, error = %e, "Failed to remove orphaned author from search");
            }
        }
        for id in &orphaned_series {
            if let Err(e) = self.search.delete_series(*id).await {
                tracing::warn!(id = %id, error = %e, "Failed to remove orphaned series from search");
            }
        }
        for id in &orphaned_genres {
            if let Err(e) = self.search.delete_genre(*id).await {
                tracing::warn!(id = %id, error = %e, "Failed to remove orphaned genre from search");
            }
        }
        for id in &orphaned_people {
            if let Err(e) = self.search.delete_person(*id).await {
                tracing::warn!(id = %id, error = %e, "Failed to remove orphaned person from search");
            }
        }

        info!(
            authors = orphaned_authors.len(),
            series = orphaned_series.len(),
            genres = orphaned_genres.len(),
            people = orphaned_people.len(),
            wanted_attempts = orphaned_attempts,
            "Cleaned up orphaned entities"
        );

        Ok(json!({
            "deleted_items": deleted_ids.len(),
            "orphaned_authors": orphaned_authors.len(),
            "orphaned_series": orphaned_series.len(),
            "orphaned_genres": orphaned_genres.len(),
            "orphaned_people": orphaned_people.len(),
            "orphaned_attempts": orphaned_attempts,
        }))
    }
}
