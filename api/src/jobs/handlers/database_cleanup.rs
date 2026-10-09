//! Database cleanup handler — comprehensive orphan sweeper.
//!
//! Deletes all database entities not linked to user-interacted content:
//! - Leaf entities with no files and not wanted (books, movies, episodes)
//! - Empty parent containers not monitored (seasons, shows)
//! - Orphaned supporting entities (authors, series, genres, people)
//! - Stale wanted_attempts, orphaned episode files, old notifications
//!
//! Optionally chains to orphan_cleanup for filesystem cleanup.

use crate::jobs::worker::JobHandler;
use crate::jobs::{self, JobContext, JobError, JobResult};
use crate::repositories::{
    authors, books, genres, movies, people, playback_tokens, series, tv_episodes, tv_seasons,
    tv_shows, wanted_attempts,
};
use crate::search::SearchBackend;
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct DatabaseCleanupPayload {
    /// When true, also enqueue filesystem orphan cleanup after DB cleanup.
    #[serde(default)]
    include_file_cleanup: bool,
}

pub struct DatabaseCleanupHandler {
    pool: PgPool,
    search: Arc<dyn SearchBackend>,
    nats: Publisher,
}

impl DatabaseCleanupHandler {
    pub fn new(pool: PgPool, search: Arc<dyn SearchBackend>, nats: Publisher) -> Self {
        Self { pool, search, nats }
    }
}

#[async_trait]
impl JobHandler for DatabaseCleanupHandler {
    fn job_type(&self) -> &'static str {
        "database_cleanup"
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
        let payload: DatabaseCleanupPayload = serde_json::from_value(payload).unwrap_or_default();
        nats_jobs::check_cancelled(&cancel)?;

        info!(job_id = %job_id, include_file_cleanup = payload.include_file_cleanup, "Running database cleanup");

        // Phase 1: Delete leaf entities (no files, not wanted)
        let deleted_books = books::delete_orphaned_books(&self.pool)
            .await
            .map_err(|e| {
                JobError::ExecutionFailed(format!("Failed to clean orphaned books: {e}"))
            })?;
        if !deleted_books.is_empty() {
            info!(count = deleted_books.len(), "Deleted orphaned books");
        }

        let deleted_movies = movies::delete_orphaned_movies(&self.pool)
            .await
            .map_err(|e| {
                JobError::ExecutionFailed(format!("Failed to clean orphaned movies: {e}"))
            })?;
        if !deleted_movies.is_empty() {
            info!(count = deleted_movies.len(), "Deleted orphaned movies");
        }

        let deleted_episodes = tv_episodes::delete_orphaned_episodes(&self.pool)
            .await
            .map_err(|e| {
                JobError::ExecutionFailed(format!("Failed to clean orphaned episodes: {e}"))
            })?;
        if !deleted_episodes.is_empty() {
            info!(count = deleted_episodes.len(), "Deleted orphaned episodes");
        }

        // Orphaned episode files (existing logic — CASCADE on tv_episodes can leave these)
        let orphaned_episode_files = sqlx::query(
            r#"
            DELETE FROM tv_episode_files
            WHERE NOT EXISTS (
                SELECT 1 FROM tv_episode_file_episodes
                WHERE episode_file_id = tv_episode_files.id
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| {
            JobError::ExecutionFailed(format!("Failed to clean orphaned episode files: {e}"))
        })?
        .rows_affected();

        if orphaned_episode_files > 0 {
            info!(
                count = orphaned_episode_files,
                "Deleted orphaned episode files"
            );
        }

        // Phase 2: Delete empty parent containers (not monitored, no children)
        let deleted_seasons = tv_seasons::delete_orphaned(&self.pool).await.map_err(|e| {
            JobError::ExecutionFailed(format!("Failed to clean orphaned seasons: {e}"))
        })?;
        if !deleted_seasons.is_empty() {
            info!(count = deleted_seasons.len(), "Deleted orphaned seasons");
        }

        let deleted_shows = tv_shows::delete_orphaned(&self.pool).await.map_err(|e| {
            JobError::ExecutionFailed(format!("Failed to clean orphaned shows: {e}"))
        })?;
        if !deleted_shows.is_empty() {
            info!(count = deleted_shows.len(), "Deleted orphaned shows");
        }

        // Phase 3: Delete orphaned supporting entities
        let orphaned_authors = authors::delete_orphaned(&self.pool).await.map_err(|e| {
            JobError::ExecutionFailed(format!("Failed to clean orphaned authors: {e}"))
        })?;
        if !orphaned_authors.is_empty() {
            info!(count = orphaned_authors.len(), "Deleted orphaned authors");
        }

        let orphaned_series = series::delete_orphaned(&self.pool).await.map_err(|e| {
            JobError::ExecutionFailed(format!("Failed to clean orphaned series: {e}"))
        })?;
        if !orphaned_series.is_empty() {
            info!(count = orphaned_series.len(), "Deleted orphaned series");
        }

        let orphaned_genres = genres::delete_orphaned(&self.pool).await.map_err(|e| {
            JobError::ExecutionFailed(format!("Failed to clean orphaned genres: {e}"))
        })?;
        if !orphaned_genres.is_empty() {
            info!(count = orphaned_genres.len(), "Deleted orphaned genres");
        }

        let orphaned_people = people::delete_orphaned(&self.pool).await.map_err(|e| {
            JobError::ExecutionFailed(format!("Failed to clean orphaned people: {e}"))
        })?;
        if !orphaned_people.is_empty() {
            info!(count = orphaned_people.len(), "Deleted orphaned people");
        }

        let orphaned_attempts =
            wanted_attempts::delete_orphaned(&self.pool)
                .await
                .map_err(|e| {
                    JobError::ExecutionFailed(format!("Failed to clean orphaned attempts: {e}"))
                })?;
        if orphaned_attempts > 0 {
            info!(
                count = orphaned_attempts,
                "Deleted orphaned wanted attempts"
            );
        }

        // Phase 4: Clean search index for all deleted entities
        for id in &deleted_books {
            if let Err(e) = self.search.delete_book(*id).await {
                warn!(id = %id, error = %e, "Failed to remove book from search");
            }
        }
        for id in &deleted_movies {
            if let Err(e) = self.search.delete_movie(*id).await {
                warn!(id = %id, error = %e, "Failed to remove movie from search");
            }
        }
        for id in &deleted_shows {
            if let Err(e) = self.search.delete_tv_show(*id).await {
                warn!(id = %id, error = %e, "Failed to remove show from search");
            }
        }
        for id in &orphaned_authors {
            if let Err(e) = self.search.delete_author(*id).await {
                warn!(id = %id, error = %e, "Failed to remove author from search");
            }
        }
        for id in &orphaned_series {
            if let Err(e) = self.search.delete_series(*id).await {
                warn!(id = %id, error = %e, "Failed to remove series from search");
            }
        }
        for id in &orphaned_genres {
            if let Err(e) = self.search.delete_genre(*id).await {
                warn!(id = %id, error = %e, "Failed to remove genre from search");
            }
        }
        for id in &orphaned_people {
            if let Err(e) = self.search.delete_person(*id).await {
                warn!(id = %id, error = %e, "Failed to remove person from search");
            }
        }

        // Phase 5: Clean expired playback tokens
        let expired_tokens = playback_tokens::delete_expired(&self.pool)
            .await
            .map_err(|e| {
                JobError::ExecutionFailed(format!("Failed to clean expired playback tokens: {e}"))
            })?;
        if expired_tokens > 0 {
            info!(count = expired_tokens, "Deleted expired playback tokens");
        }

        // Phase 6: Clean old notifications
        let old_notifications = sqlx::query(
            r#"
            DELETE FROM user_notifications
            WHERE (read = true AND created_at < NOW() - INTERVAL '30 days')
               OR created_at < NOW() - INTERVAL '90 days'
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("Failed to clean old notifications: {e}")))?
        .rows_affected();

        if old_notifications > 0 {
            info!(count = old_notifications, "Deleted old notifications");
        }

        // Phase 7: Chain filesystem orphan cleanup if requested
        if payload.include_file_cleanup {
            match jobs::enqueue(
                &self.pool,
                &self.nats,
                "orphan_cleanup",
                json!({}),
                JobContext::new("Cleaning up orphaned files (chained from library cleanup)"),
            )
            .await
            {
                Ok(cleanup_job_id) => {
                    info!(job_id = %cleanup_job_id, "Chained orphan_cleanup job");
                }
                Err(e) => {
                    warn!(error = %e, "Failed to chain orphan_cleanup job");
                }
            }
        }

        Ok(json!({
            "deleted_books": deleted_books.len(),
            "deleted_movies": deleted_movies.len(),
            "deleted_episodes": deleted_episodes.len(),
            "orphaned_episode_files": orphaned_episode_files,
            "deleted_seasons": deleted_seasons.len(),
            "deleted_shows": deleted_shows.len(),
            "orphaned_authors": orphaned_authors.len(),
            "orphaned_series": orphaned_series.len(),
            "orphaned_genres": orphaned_genres.len(),
            "orphaned_people": orphaned_people.len(),
            "orphaned_attempts": orphaned_attempts,
            "old_notifications": old_notifications,
            "expired_playback_tokens": expired_tokens
        }))
    }
}
