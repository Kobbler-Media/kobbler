//! Debrid cache cleanup handler.
//!
//! Runs on a schedule to:
//! 1. Delete expired cache files (past TTL, not pinned)
//! 2. Evict LRU files if total cache exceeds size limit
//! 3. Update DB records for evicted files (set cache_path = NULL)

use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use async_trait::async_trait;
use serde_json::Value;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

pub struct DebridCacheCleanupHandler {
    pool: PgPool,
    runtime_config: std::sync::Arc<crate::RuntimeConfigManager>,
}

impl DebridCacheCleanupHandler {
    pub fn new(pool: PgPool, runtime_config: std::sync::Arc<crate::RuntimeConfigManager>) -> Self {
        Self {
            pool,
            runtime_config,
        }
    }
}

/// Remove the cached video file, its probe sidecar, and its sibling
/// `media_cache/` dir produced by `debrid_process`. Returns `true` iff the
/// video file was removed (or was already absent); logs and continues past any
/// sidecar or sibling-dir failures because those are best-effort cleanup.
async fn remove_cache_entry(file_path: &str) -> bool {
    let path = std::path::Path::new(file_path);
    if path.exists()
        && let Err(e) = tokio::fs::remove_file(path).await
    {
        warn!(path = %file_path, error = %e, "Failed to delete debrid cache file");
        return false;
    }
    // The sidecar carries probe results for *this* file. Left behind, it is
    // trusted verbatim if the same info_hash is downloaded again later.
    let sidecar = crate::routes::admin::debrid_stream::probe_sidecar_path(path);
    if sidecar.exists()
        && let Err(e) = tokio::fs::remove_file(&sidecar).await
    {
        warn!(path = %sidecar.display(), error = %e, "Failed to delete probe sidecar");
    }
    if let Some(parent) = path.parent() {
        let sibling = parent.join("media_cache");
        if sibling.is_dir()
            && let Err(e) = tokio::fs::remove_dir_all(&sibling).await
        {
            warn!(path = %sibling.display(), error = %e, "Failed to prune sibling media_cache dir");
        }
    }
    true
}

#[async_trait]
impl JobHandler for DebridCacheCleanupHandler {
    fn job_type(&self) -> &'static str {
        "debrid_cache_cleanup"
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
        // Step 1: Delete expired, unpinned cache files
        // Skip files accessed in the last hour — they may be actively streaming.
        // The session heartbeat updates last_accessed_at every 30s.
        let expired = sqlx::query!(
            r#"
            SELECT id, file_path, file_size
            FROM debrid_cache_files
            WHERE NOT pinned
              AND expires_at < NOW()
              AND last_accessed_at < NOW() - INTERVAL '1 hour'
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| crate::jobs::JobError::Retryable(e.to_string()))?;

        let mut cleaned_bytes: i64 = 0;
        let mut cleaned_count = 0;

        for file in &expired {
            if cancel.is_cancelled() {
                break;
            }

            // Delete the physical file + its sibling media_cache dir
            if !remove_cache_entry(&file.file_path).await {
                continue;
            }

            // Null out the cache_path on the parent session
            sqlx::query!(
                r#"
                UPDATE debrid_stream_sessions
                SET cache_path = NULL, updated_at = NOW()
                WHERE id = (SELECT session_id FROM debrid_cache_files WHERE id = $1)
                "#,
                file.id,
            )
            .execute(&self.pool)
            .await
            .ok();

            // Delete the cache file record
            sqlx::query!("DELETE FROM debrid_cache_files WHERE id = $1", file.id)
                .execute(&self.pool)
                .await
                .ok();

            cleaned_bytes += file.file_size;
            cleaned_count += 1;
        }

        if cleaned_count > 0 {
            info!(
                count = cleaned_count,
                bytes = cleaned_bytes,
                "Cleaned expired debrid cache files"
            );
        }

        // Step 2: Check total cache size and evict LRU if over limit
        let max_cache_bytes = self.runtime_config.get().await.debrid_cache_max_bytes;

        let total_size: i64 = sqlx::query_scalar!(
            r#"SELECT COALESCE(SUM(file_size)::bigint, 0) as "total!: i64" FROM debrid_cache_files WHERE NOT pinned"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| crate::jobs::JobError::Retryable(e.to_string()))?;

        if total_size > max_cache_bytes {
            let excess = total_size - max_cache_bytes;
            info!(
                total_mb = total_size / (1024 * 1024),
                limit_mb = max_cache_bytes / (1024 * 1024),
                excess_mb = excess / (1024 * 1024),
                "Cache over limit, evicting LRU files"
            );

            // Fetch LRU files until we've freed enough space
            let lru_files = sqlx::query!(
                r#"
                SELECT id, file_path, file_size
                FROM debrid_cache_files
                WHERE NOT pinned
                  AND last_accessed_at < NOW() - INTERVAL '1 hour'
                ORDER BY last_accessed_at ASC
                "#,
            )
            .fetch_all(&self.pool)
            .await
            .map_err(|e| crate::jobs::JobError::Retryable(e.to_string()))?;

            let mut freed: i64 = 0;
            for file in lru_files {
                if cancel.is_cancelled() || freed >= excess {
                    break;
                }

                if !remove_cache_entry(&file.file_path).await {
                    continue;
                }

                sqlx::query!(
                    r#"
                    UPDATE debrid_stream_sessions
                    SET cache_path = NULL, updated_at = NOW()
                    WHERE id = (SELECT session_id FROM debrid_cache_files WHERE id = $1)
                    "#,
                    file.id,
                )
                .execute(&self.pool)
                .await
                .ok();

                sqlx::query!("DELETE FROM debrid_cache_files WHERE id = $1", file.id)
                    .execute(&self.pool)
                    .await
                    .ok();

                freed += file.file_size;
            }

            info!(freed_mb = freed / (1024 * 1024), "LRU eviction complete");
        }

        // Step 3: Clean up orphan files on disk that aren't tracked in DB
        // (Skip for now — can be added later if needed)

        Ok(serde_json::json!({
            "expired_cleaned": cleaned_count,
            "expired_bytes": cleaned_bytes,
        }))
    }
}
