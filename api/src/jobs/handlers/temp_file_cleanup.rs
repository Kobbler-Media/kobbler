//! Cleanup handler for orphaned `.kobbler-dl-*` temp files.
//!
//! During ingest, files are downloaded to hidden temp files (`.kobbler-dl-{uuid}.{ext}`)
//! then renamed to their final names. If ingest fails or skips a file, the temp file
//! can be left behind. This job scans all media roots and deletes stale temp files.
//!
//! Safety: files are only deleted when BOTH conditions are met:
//! 1. The file is older than 1 hour (STALE_THRESHOLD)
//! 2. No `kobload_ingest` job is currently running (checked AFTER scanning,
//!    so a job that starts between scan and check will block deletion)

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::media_roots;
use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

const STALE_THRESHOLD: Duration = Duration::from_secs(3600); // 1 hour
const MAX_DIRS: u64 = 10_000;
const SCAN_TIMEOUT: Duration = Duration::from_secs(600);

struct StaleFile {
    path: PathBuf,
    size: u64,
    age_secs: u64,
}

pub struct TempFileCleanupHandler {
    pool: PgPool,
    fs_semaphore: Arc<Semaphore>,
}

impl TempFileCleanupHandler {
    pub fn new(pool: PgPool, fs_semaphore: Arc<Semaphore>) -> Self {
        Self { pool, fs_semaphore }
    }
}

#[async_trait]
impl JobHandler for TempFileCleanupHandler {
    fn job_type(&self) -> &'static str {
        "temp_file_cleanup"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        job_id: Uuid,
        _payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let _fs_permit = self
            .fs_semaphore
            .acquire()
            .await
            .map_err(|_| JobError::ExecutionFailed("FS semaphore closed".into()))?;
        nats_jobs::check_cancelled(&cancel)?;
        info!(job_id = %job_id, "Running temp file cleanup");

        let roots = media_roots::get_all_enabled(&self.pool)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to list media roots: {e}")))?;

        // Phase 1: Scan all media roots and collect stale candidates
        let now = SystemTime::now();

        // Collect paths to scan
        let root_paths: Vec<PathBuf> = roots.iter().map(|r| PathBuf::from(&r.path)).collect();

        // Scan all media roots on the blocking pool (single spawn_blocking for the batch)
        let (candidates, scanned_roots, scan_truncated) = tokio::task::spawn_blocking(move || {
            let mut found_candidates = Vec::new();
            let mut roots_scanned = 0u64;
            let mut truncated = false;
            let deadline = Instant::now() + SCAN_TIMEOUT;
            let mut dirs_visited = 0u64;
            for root_path in &root_paths {
                if !root_path.exists() {
                    continue;
                }
                roots_scanned += 1;
                let result = find_stale_files(
                    root_path,
                    now,
                    &mut found_candidates,
                    &mut dirs_visited,
                    deadline,
                );
                if result.truncated {
                    truncated = true;
                    break;
                }
            }
            (found_candidates, roots_scanned, truncated)
        })
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("Join error: {e}")))?;

        if candidates.is_empty() {
            return Ok(json!({
                "scanned_roots": scanned_roots,
                "deleted_files": 0,
                "deleted_bytes": 0,
                "scan_truncated": scan_truncated,
            }));
        }

        // Phase 2: Check for running kobload_ingest jobs AFTER scanning.
        // This ordering eliminates the race: if a job started during our scan,
        // it will show as running here and we'll skip deletion.
        let has_running_ingest = sqlx::query_scalar!(
            r#"SELECT EXISTS(
                SELECT 1 FROM job_status
                WHERE job_type = 'kobload_ingest' AND status = 'running'
            ) AS "exists!""#,
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false);

        if has_running_ingest {
            info!(
                candidates = candidates.len(),
                "Skipping temp file cleanup — kobload_ingest job is running"
            );
            return Ok(json!({
                "scanned_roots": scanned_roots,
                "deleted_files": 0,
                "deleted_bytes": 0,
                "skipped_reason": "kobload_ingest_running",
                "skipped_candidates": candidates.len(),
            }));
        }

        // Phase 3: Delete the candidates on the blocking pool
        let (deleted_files, deleted_bytes) = tokio::task::spawn_blocking(move || {
            let mut del_files: u64 = 0;
            let mut del_bytes: u64 = 0;
            for candidate in &candidates {
                match std::fs::remove_file(&candidate.path) {
                    Ok(()) => {
                        info!(
                            path = %candidate.path.display(),
                            age_minutes = candidate.age_secs / 60,
                            size_mb = candidate.size / (1024 * 1024),
                            "Deleted stale temp file"
                        );
                        del_files += 1;
                        del_bytes += candidate.size;
                    }
                    Err(e) => {
                        warn!(
                            path = %candidate.path.display(),
                            error = %e,
                            "Failed to delete stale temp file"
                        );
                    }
                }
            }
            (del_files, del_bytes)
        })
        .await
        .unwrap_or((0, 0));

        if deleted_files > 0 {
            info!(
                deleted_files,
                deleted_bytes, scanned_roots, "Temp file cleanup complete"
            );
        }

        Ok(json!({
            "scanned_roots": scanned_roots,
            "deleted_files": deleted_files,
            "deleted_bytes": deleted_bytes,
            "scan_truncated": scan_truncated,
        }))
    }
}

struct ScanResult {
    truncated: bool,
}

/// Recursively scan a directory for `.kobbler-dl-*` files older than STALE_THRESHOLD.
///
/// Runs synchronously (intended for use inside spawn_blocking). Bounded by both
/// a directory count limit and a wall-clock deadline to prevent hangs on slow
/// network filesystems.
fn find_stale_files(
    dir: &Path,
    now: SystemTime,
    candidates: &mut Vec<StaleFile>,
    dirs_visited: &mut u64,
    deadline: Instant,
) -> ScanResult {
    let mut stack = vec![dir.to_path_buf()];

    while let Some(current) = stack.pop() {
        *dirs_visited += 1;
        if *dirs_visited > MAX_DIRS {
            warn!(
                dirs_visited,
                root = %dir.display(),
                "temp_file_cleanup: hit directory limit, returning partial results"
            );
            return ScanResult { truncated: true };
        }
        if Instant::now() > deadline {
            warn!(
                dirs_visited,
                root = %dir.display(),
                "temp_file_cleanup: hit time limit, returning partial results"
            );
            return ScanResult { truncated: true };
        }

        let entries = match std::fs::read_dir(&current) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let file_type = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };

            if file_type.is_dir() {
                stack.push(entry.path());
                continue;
            }

            if !file_type.is_file() {
                continue;
            }

            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            if !name.starts_with(".kobbler-dl-") {
                continue;
            }

            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };

            let modified = match metadata.modified() {
                Ok(t) => t,
                Err(_) => continue,
            };

            let age = now.duration_since(modified).unwrap_or_default();
            if age < STALE_THRESHOLD {
                continue;
            }

            candidates.push(StaleFile {
                path: entry.path(),
                size: metadata.len(),
                age_secs: age.as_secs(),
            });
        }
    }

    ScanResult { truncated: false }
}
