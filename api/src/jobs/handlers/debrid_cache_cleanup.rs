//! Debrid cache cleanup handler.
//!
//! Runs on a schedule to:
//! 1. Delete expired cache files (past TTL, not pinned)
//! 2. Evict LRU files if total cache exceeds size limit
//! 3. Forget evicted files on every session that pointed at them

use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::repositories::debrid_cache_files as cache_files;
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
pub(crate) async fn remove_cache_entry(file_path: &str) -> bool {
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
        let retryable = |e: crate::repositories::RepositoryError| {
            crate::jobs::JobError::Retryable(e.to_string())
        };
        let expired = cache_files::expired(&self.pool).await.map_err(retryable)?;

        let mut cleaned_bytes: i64 = 0;
        let mut cleaned_count = 0;
        for file in &expired {
            if cancel.is_cancelled() {
                break;
            }
            if self.evict(file).await {
                cleaned_bytes += file.file_size;
                cleaned_count += 1;
            }
        }
        if cleaned_count > 0 {
            info!(
                count = cleaned_count,
                bytes = cleaned_bytes,
                "Cleaned expired debrid cache files"
            );
        }

        let max_cache_bytes = self.runtime_config.get().await.debrid_cache_max_bytes;
        let total_size = cache_files::unpinned_bytes(&self.pool)
            .await
            .map_err(retryable)?;
        if total_size > max_cache_bytes {
            let excess = total_size - max_cache_bytes;
            info!(
                total_mb = total_size / (1024 * 1024),
                limit_mb = max_cache_bytes / (1024 * 1024),
                excess_mb = excess / (1024 * 1024),
                "Cache over limit, evicting LRU files"
            );
            let lru_files = cache_files::least_recently_used(&self.pool)
                .await
                .map_err(retryable)?;
            let mut freed: i64 = 0;
            for file in lru_files {
                if cancel.is_cancelled() || freed >= excess {
                    break;
                }
                if self.evict(&file).await {
                    freed += file.file_size;
                }
            }
            info!(freed_mb = freed / (1024 * 1024), "LRU eviction complete");
        }

        Ok(serde_json::json!({
            "expired_cleaned": cleaned_count,
            "expired_bytes": cleaned_bytes,
        }))
    }
}

impl DebridCacheCleanupHandler {
    async fn evict(&self, file: &cache_files::CachedFile) -> bool {
        if !remove_cache_entry(&file.file_path).await {
            return false;
        }
        if let Err(e) = cache_files::forget(&self.pool, &file.file_path).await {
            warn!(path = %file.file_path, error = %e, "Failed to forget evicted cache file");
        }
        true
    }
}
