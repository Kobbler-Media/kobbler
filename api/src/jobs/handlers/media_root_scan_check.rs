//! Media root scan check job handler.
//!
//! Periodic check that enqueues `media_root_scan` jobs for all enabled
//! media roots. Runs every 5 minutes via the NATS scheduler.

use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::repositories::{deferred_ingests, media_roots};
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

pub struct MediaRootScanCheckHandler {
    pool: PgPool,
    nats_publisher: Publisher,
    runtime_config: crate::RuntimeConfigManager,
}

impl MediaRootScanCheckHandler {
    pub fn new(
        pool: PgPool,
        nats_publisher: Publisher,
        runtime_config: crate::RuntimeConfigManager,
    ) -> Self {
        Self {
            pool,
            nats_publisher,
            runtime_config,
        }
    }
}

#[async_trait]
impl JobHandler for MediaRootScanCheckHandler {
    fn job_type(&self) -> &'static str {
        "media_root_scan_check"
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
        let roots = media_roots::get_all_enabled(&self.pool).await?;

        if roots.is_empty() {
            return Ok(json!({"checked": 0, "enqueued": 0, "message": "No enabled media roots"}));
        }

        let default_interval = self
            .runtime_config
            .get()
            .await
            .media_root_scan_interval_seconds;

        let mut checked = 0u64;
        let mut enqueued = 0u64;
        let mut skipped_backoff = 0u64;
        let mut skipped_active = 0u64;
        let mut skipped_interval = 0u64;

        for mut root in roots {
            checked += 1;

            // For roots stuck in backoff due to an unavailable path, probe the
            // filesystem directly.  If the path is back, clear the error so the
            // scan can proceed this cycle instead of waiting for backoff expiry.
            if root.next_retry_at.is_some()
                && let Ok(Some(full)) = media_roots::get_by_id(&self.pool, root.id).await
                && full
                    .scan_error
                    .as_deref()
                    .is_some_and(|e| e.starts_with("Directory not found:"))
            {
                let root_path = std::path::Path::new(&root.path).to_owned();
                let exists = tokio::task::spawn_blocking(move || root_path.exists())
                    .await
                    .unwrap_or(false);
                if exists {
                    info!(
                        media_root_id = %root.id,
                        name = %root.name,
                        "Media root path is accessible again, clearing error state"
                    );
                    let _ = media_roots::clear_error(&self.pool, root.id).await;
                    let db = nats_jobs::JobStatusDb::new(self.pool.clone());
                    let root_id_str = root.id.to_string();
                    for job_type in media_roots::FS_JOB_TYPES {
                        let _ = db
                            .cancel_pending_jobs_with_field(
                                job_type,
                                "media_root_id",
                                &root_id_str,
                                "media_root_id",
                            )
                            .await;
                    }
                    root.next_retry_at = None;
                    root.error_count = 0;
                } else {
                    skipped_backoff += 1;
                    continue;
                }
            }

            // Check backoff
            if let Some(retry_at) = root.next_retry_at
                && retry_at > OffsetDateTime::now_utc()
            {
                debug!(
                    root_id = %root.id,
                    name = %root.name,
                    retry_at = %retry_at,
                    "Skipping media root - in backoff"
                );
                skipped_backoff += 1;
                continue;
            }

            // Check for active scan or interpretation
            let root_id_str = root.id.to_string();
            match crate::jobs::has_active_job(
                &self.pool,
                "media_root_scan",
                "media_root_id",
                &root_id_str,
            )
            .await
            {
                Ok(true) => {
                    debug!(
                        root_id = %root.id,
                        name = %root.name,
                        "Skipping media root - scan already in progress"
                    );
                    skipped_active += 1;
                    continue;
                }
                Ok(false) => {}
                Err(e) => {
                    warn!(
                        root_id = %root.id,
                        error = %e,
                        "Failed to check for active scan job"
                    );
                }
            }

            // Also skip if interpretation is still running — no point re-scanning
            // while the pipeline is processing the previous scan's results
            match crate::jobs::has_active_job(
                &self.pool,
                "media_root_interpret",
                "media_root_id",
                &root_id_str,
            )
            .await
            {
                Ok(true) => {
                    debug!(
                        root_id = %root.id,
                        name = %root.name,
                        "Skipping media root - interpretation still in progress"
                    );
                    skipped_active += 1;
                    continue;
                }
                Ok(false) => {}
                Err(e) => {
                    warn!(
                        root_id = %root.id,
                        error = %e,
                        "Failed to check for active interpret job"
                    );
                }
            }

            // Per-root interval gate: skip roots that were scanned recently enough.
            // NULL on the root = use global default. Continuation re-enqueues and
            // single-file watcher scans bypass this by enqueuing directly without
            // going through this check handler.
            let interval = root.scan_interval_seconds.unwrap_or(default_interval);
            if let Some(last) = root.last_scan_completed_at
                && (OffsetDateTime::now_utc() - last).whole_seconds() < interval as i64
            {
                debug!(
                    root_id = %root.id,
                    name = %root.name,
                    interval,
                    "Skipping media root — within scan interval"
                );
                skipped_interval += 1;
                continue;
            }

            // Cancel any pending single-file scans — the full scan supersedes them
            if let Err(e) = crate::jobs::cancel_pending_jobs_with_field(
                &self.pool,
                "media_root_scan",
                "media_root_id",
                &root_id_str,
                "single_file",
            )
            .await
            {
                warn!(
                    root_id = %root.id,
                    error = %e,
                    "Failed to cancel pending single-file scans"
                );
            }

            let display = crate::jobs::fetch_job_display(&self.pool, "media_root", root.id).await;
            let context = nats_jobs::JobContext::for_entity(
                "media_root",
                root.id,
                format!("Scanning media root '{}'", root.name),
            )
            .with_display_opt(display);

            match crate::jobs::enqueue_routine(
                &self.pool,
                &self.nats_publisher,
                "media_root_scan",
                json!({"media_root_id": root_id_str}),
                context,
            )
            .await
            {
                Ok(job_id) => {
                    debug!(
                        job_id = %job_id,
                        root_id = %root.id,
                        name = %root.name,
                        "Enqueued media root scan"
                    );
                    enqueued += 1;
                }
                Err(e) => {
                    warn!(
                        root_id = %root.id,
                        name = %root.name,
                        error = %e,
                        "Failed to enqueue media root scan"
                    );
                }
            }
        }

        // Re-enqueue deferred ingests for roots that are now available
        let mut deferred_processed = 0u64;
        let all_roots = media_roots::list_all(&self.pool).await.unwrap_or_default();
        for root in &all_roots {
            if !root.enabled {
                continue;
            }
            let deferred = match deferred_ingests::list_for_media_root(&self.pool, root.id).await {
                Ok(d) if !d.is_empty() => d,
                _ => continue,
            };
            // Quick check if the root's path is accessible
            let root_path = std::path::Path::new(&root.path).to_owned();
            let exists = tokio::task::spawn_blocking(move || root_path.exists())
                .await
                .unwrap_or(false);
            if !exists {
                continue;
            }
            info!(
                media_root_id = %root.id,
                name = %root.name,
                count = deferred.len(),
                "Re-enqueueing deferred ingests — media root is available again"
            );
            for d in deferred {
                let display =
                    crate::jobs::fetch_job_display(&self.pool, "media_root", root.id).await;
                let context = nats_jobs::JobContext::for_entity(
                    "media_root",
                    root.id,
                    format!("Retrying deferred ingest for download {}", d.download_id),
                )
                .with_display_opt(display);
                match crate::jobs::enqueue(
                    &self.pool,
                    &self.nats_publisher,
                    "kobload_ingest",
                    d.payload,
                    context,
                )
                .await
                {
                    Ok(_) => {
                        let _ = deferred_ingests::delete(&self.pool, d.id).await;
                        deferred_processed += 1;
                    }
                    Err(e) => {
                        warn!(
                            download_id = %d.download_id,
                            error = %e,
                            "Failed to re-enqueue deferred ingest"
                        );
                    }
                }
            }
        }

        if enqueued > 0 || deferred_processed > 0 {
            info!(
                checked = checked,
                enqueued = enqueued,
                skipped_backoff = skipped_backoff,
                skipped_active = skipped_active,
                skipped_interval = skipped_interval,
                deferred_processed = deferred_processed,
                "Media root scan check complete"
            );
        }

        Ok(json!({
            "checked": checked,
            "enqueued": enqueued,
            "skipped_backoff": skipped_backoff,
            "skipped_active": skipped_active,
            "skipped_interval": skipped_interval,
            "deferred_processed": deferred_processed,
        }))
    }
}
