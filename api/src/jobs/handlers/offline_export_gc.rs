use std::collections::HashSet;
use std::path::Path;

use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::repositories::offline_exports;
use crate::streaming::offline_export::exports_root;

const IDLE_DAYS: i32 = 14;
const FAILED_DAYS: i32 = 1;

pub struct OfflineExportGcHandler {
    pool: PgPool,
    storage_root: String,
}

impl OfflineExportGcHandler {
    pub fn new(pool: PgPool, storage_root: String) -> Self {
        Self { pool, storage_root }
    }
}

#[async_trait]
impl JobHandler for OfflineExportGcHandler {
    fn job_type(&self) -> &'static str {
        "offline_export_gc"
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
        nats_jobs::check_cancelled(&cancel)?;
        let expired = offline_exports::delete_expired(&self.pool, IDLE_DAYS, FAILED_DAYS).await?;
        let live: HashSet<Uuid> = offline_exports::all_ids(&self.pool)
            .await?
            .into_iter()
            .collect();

        // Recursive deletes of multi-gigabyte files would stall a runtime worker.
        let root = exports_root(&self.storage_root);
        let removed_dirs = tokio::task::spawn_blocking(move || sweep(&root, &live, &cancel))
            .await
            .expect("offline_export_gc sweep panicked");

        info!(
            expired = expired.len(),
            removed_dirs, "offline_export_gc complete"
        );
        Ok(json!({ "expired": expired.len(), "removed_dirs": removed_dirs }))
    }
}

/// Remove every entry under `root` that is not the directory of a live export, which
/// covers both the exports just expired and directories whose row is long gone.
fn sweep(root: &Path, live: &HashSet<Uuid>, cancel: &CancellationToken) -> usize {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return 0,
        Err(e) => {
            warn!(root = %root.display(), error = %e, "failed to list offline exports");
            return 0;
        }
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        if cancel.is_cancelled() {
            break;
        }
        let name = entry.file_name();
        let is_live = name
            .to_str()
            .and_then(|n| Uuid::parse_str(n).ok())
            .is_some_and(|id| live.contains(&id));
        if is_live {
            continue;
        }
        let path = entry.path();
        let result = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match result {
            Ok(()) => removed += 1,
            Err(e) => warn!(path = %path.display(), error = %e, "failed to remove offline export"),
        }
    }
    removed
}
