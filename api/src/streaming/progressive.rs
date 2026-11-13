//! Progressive Download Manager for debrid streams.
//!
//! When a user streams via debrid, this module simultaneously downloads the full file
//! to a local cache. Once downloaded, it triggers the processing pipeline (probe →
//! keyframes → thumbnails → intro detection) and emits SSE events as features complete.
//!
//! The player starts immediately via debrid proxy, then progressively gains features
//! (timeline thumbnails, skip-intro buttons, subtitles) as processing completes.

use crate::repositories::debrid_cache_files::{self as cache_files, CacheRelease};
use crate::streaming::download_meter::DownloadMeter;
use nats_jobs::Publisher;
use sqlx::PgPool;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

/// A host still refusing after this many retries has not handed over a single byte: the
/// client is watching a spinner over it, so it is a dead link, not a hiccup.
const MAX_ATTEMPTS_BEFORE_FIRST_BYTE: u32 = 1;

#[derive(Debug)]
pub enum DownloadError {
    /// The host answered, and not with the file.
    Refused(String),
    Failed(String),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(m) | Self::Failed(m) => f.write_str(m),
        }
    }
}

impl From<String> for DownloadError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

/// Configuration for the progressive download manager.
#[derive(Debug, Clone)]
pub struct ProgressiveDownloadConfig {
    /// Directory for cached debrid files.
    pub cache_dir: PathBuf,
    /// Default TTL for cached files in seconds.
    pub default_ttl_secs: i64,
    /// Maximum total cache size in bytes.
    pub max_cache_bytes: i64,
}

impl Default for ProgressiveDownloadConfig {
    fn default() -> Self {
        Self {
            cache_dir: PathBuf::from("/tmp/kobbler-debrid-cache"),
            default_ttl_secs: 24 * 3600,               // 24 hours
            max_cache_bytes: 100 * 1024 * 1024 * 1024, // 100 GB
        }
    }
}

/// One release's file, downloaded into the cache on behalf of a session.
#[derive(Debug, Clone)]
pub struct DownloadTarget {
    pub session_id: Uuid,
    pub url: String,
    pub cache_path: PathBuf,
    pub filesize: i64,
    pub release: CacheRelease,
}

/// Manages progressive background downloads of debrid stream files.
pub struct ProgressiveDownloadManager {
    config: ProgressiveDownloadConfig,
    pool: PgPool,
    http_client: reqwest::Client,
    nats_publisher: Publisher,
}

impl ProgressiveDownloadManager {
    pub fn new(
        config: ProgressiveDownloadConfig,
        pool: PgPool,
        http_client: reqwest::Client,
        nats_publisher: Publisher,
    ) -> Self {
        Self {
            config,
            pool,
            http_client,
            nats_publisher,
        }
    }

    /// Start a progressive download for a debrid stream session.
    ///
    /// Downloads the file from `debrid_url` to a local cache file.
    /// Updates the session's cache_path and progress in the DB.
    /// When complete, triggers the processing pipeline.
    ///
    /// `meter` is marked complete only when the whole file landed: a complete
    /// mark on a truncated file makes `HybridIo` trust it, and every read past the
    /// on-disk end becomes a hard EOF.
    pub async fn start_download(
        self: Arc<Self>,
        target: DownloadTarget,
        cancel: CancellationToken,
        meter: Arc<DownloadMeter>,
    ) -> Result<(), DownloadError> {
        meter.started();
        let outcome = self.download(target, &cancel, &meter).await;
        if outcome.is_err() {
            meter.finished(false);
        }
        outcome
    }

    async fn download(
        &self,
        target: DownloadTarget,
        cancel: &CancellationToken,
        meter: &DownloadMeter,
    ) -> Result<(), DownloadError> {
        let DownloadTarget {
            session_id,
            url: debrid_url,
            cache_path,
            filesize,
            release,
        } = target;
        if let Some(parent) = cache_path.parent()
            && let Err(e) = tokio::fs::create_dir_all(parent).await
        {
            error!(session_id = %session_id, error = %e, "Failed to create cache directory");
            return Err(format!("Failed to create cache directory: {e}").into());
        }

        let cache_path_str = cache_path.to_string_lossy().to_string();

        // Update session with cache path
        if let Err(e) = sqlx::query!(
            r#"
            UPDATE debrid_stream_sessions
            SET cache_path = $1, updated_at = NOW()
            WHERE id = $2
            "#,
            cache_path_str,
            session_id,
        )
        .execute(&self.pool)
        .await
        {
            error!(session_id = %session_id, error = %e, "Failed to update cache path");
            return Err(format!("Failed to update cache path: {e}").into());
        }

        let expires_at =
            time::OffsetDateTime::now_utc() + time::Duration::seconds(self.config.default_ttl_secs);
        if let Err(e) =
            cache_files::upsert(&self.pool, &release, &cache_path_str, filesize, expires_at).await
        {
            warn!(session_id = %session_id, error = %e, "Failed to record cache file");
        }

        // Start the download
        info!(
            session_id = %session_id,
            path = %cache_path.display(),
            size_mb = filesize / (1024 * 1024),
            "Starting progressive download"
        );

        match self
            .download_file(
                &debrid_url,
                &cache_path,
                session_id,
                filesize,
                cancel,
                meter,
            )
            .await
        {
            Ok(()) => {
                info!(session_id = %session_id, "Progressive download complete");
                meter.finished(true);
                // Trigger processing pipeline
                self.trigger_processing(session_id, &cache_path).await;
                Ok(())
            }
            Err(e) => {
                if cancel.is_cancelled() {
                    info!(session_id = %session_id, "Progressive download cancelled");
                } else {
                    error!(session_id = %session_id, error = %e, "Progressive download failed");
                }
                Err(e)
            }
        }
    }

    /// Download file from URL to local path with progress tracking.
    /// Automatically resumes with Range requests if the connection drops.
    async fn download_file(
        &self,
        url: &str,
        dest: &Path,
        session_id: Uuid,
        expected_size: i64,
        cancel: &CancellationToken,
        meter: &DownloadMeter,
    ) -> Result<(), DownloadError> {
        use futures_util::StreamExt;

        let mut downloaded: i64 = 0;
        let mut last_progress_update = tokio::time::Instant::now();
        let mut resume_attempts = 0;
        const MAX_RESUME_ATTEMPTS: u32 = 10;

        // Resume from existing partial download if present
        let mut file = if tokio::fs::try_exists(dest).await.unwrap_or(false) {
            let existing_size = tokio::fs::metadata(dest)
                .await
                .map(|m| m.len() as i64)
                .unwrap_or(0);
            meter.record(existing_size as u64);
            if expected_size > 0 && existing_size >= expected_size {
                info!(
                    session_id = %session_id,
                    size_mb = existing_size / (1024 * 1024),
                    "Download already complete on disk, skipping"
                );
                return Ok(());
            }
            if existing_size > 0 {
                downloaded = existing_size;
                info!(
                    session_id = %session_id,
                    resumed_mb = downloaded / (1024 * 1024),
                    total_mb = expected_size / (1024 * 1024),
                    "Resuming partial download"
                );
            }
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(dest)
                .await
                .map_err(|e| format!("Failed to open file for resume: {e}"))?
        } else {
            tokio::fs::create_dir_all(dest.parent().unwrap_or(dest))
                .await
                .map_err(|e| format!("Failed to create directory: {e}"))?;
            tokio::fs::File::create(dest)
                .await
                .map_err(|e| format!("Failed to create file: {e}"))?
        };

        loop {
            // Open connection — use Range header if resuming
            let req = if downloaded > 0 {
                self.http_client
                    .get(url)
                    .header("Range", format!("bytes={downloaded}-"))
            } else {
                self.http_client.get(url)
            };

            let resp = req
                .send()
                .await
                .map_err(|e| format!("HTTP request failed: {e}"))?;

            let status = resp.status();
            if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
                // Debrid CDN nodes 503/429 sporadically and serve the very next
                // request fine, so a transient status is a retry, not a failure.
                // A permanent one (404/410/dead link) still fails immediately.
                if !is_transient_status(status) {
                    return Err(DownloadError::Refused(format!("HTTP {status}")));
                }
                resume_attempts += 1;
                let max_attempts = if downloaded == 0 {
                    MAX_ATTEMPTS_BEFORE_FIRST_BYTE
                } else {
                    MAX_RESUME_ATTEMPTS
                };
                if resume_attempts > max_attempts {
                    return Err(DownloadError::Refused(format!(
                        "HTTP {status} after {max_attempts} retries at {}MB",
                        downloaded / (1024 * 1024)
                    )));
                }
                let delay = std::time::Duration::from_secs(1 << resume_attempts.min(4));
                warn!(
                    session_id = %session_id,
                    %status,
                    attempt = resume_attempts,
                    downloaded_mb = downloaded / (1024 * 1024),
                    delay_secs = delay.as_secs(),
                    "Transient HTTP status opening download, retrying"
                );
                tokio::time::sleep(delay).await;
                if cancel.is_cancelled() {
                    return Err(DownloadError::Failed("Cancelled".to_string()));
                }
                continue;
            }

            // A server that ignores our Range header answers 200 with the whole
            // file from byte 0. Appending that to a partial download silently
            // corrupts the cache, so start the file over instead.
            if downloaded > 0 && status == reqwest::StatusCode::OK {
                warn!(
                    session_id = %session_id,
                    discarded_mb = downloaded / (1024 * 1024),
                    "Server ignored Range header, restarting download from zero"
                );
                file.flush()
                    .await
                    .map_err(|e| format!("Flush error: {e}"))?;
                file = tokio::fs::File::create(dest)
                    .await
                    .map_err(|e| format!("Failed to truncate file: {e}"))?;
                downloaded = 0;
                meter.record(0);
            }

            let mut stream = resp.bytes_stream();
            let mut stream_error = false;

            loop {
                // A stalled host must not keep a cancelled download holding the file:
                // whoever resumes it would append alongside.
                let chunk = tokio::select! {
                    chunk = stream.next() => chunk,
                    _ = cancel.cancelled() => return Err(DownloadError::Failed("Cancelled".to_string())),
                };
                let Some(chunk) = chunk else {
                    break;
                };
                if cancel.is_cancelled() {
                    // Keep partial file on disk for resume on next playback.
                    // Cache cleanup job handles expiration.
                    return Err(DownloadError::Failed("Cancelled".to_string()));
                }

                match chunk {
                    Ok(data) => {
                        if let Err(e) = file.write_all(&data).await {
                            return Err(format!("Write error: {e}").into());
                        }
                        // tokio's File returns from a write before it lands. A local read
                        // follows this file, so the meter must count exactly what a reader
                        // can see: never less, or a client is served bytes reported missing.
                        file.flush()
                            .await
                            .map_err(|e| format!("Flush error: {e}"))?;
                        downloaded += data.len() as i64;
                        meter.record(downloaded as u64);
                        // Reset resume counter on successful data
                        resume_attempts = 0;
                    }
                    Err(e) => {
                        warn!(
                            session_id = %session_id,
                            downloaded_mb = downloaded / (1024 * 1024),
                            error = %e,
                            "Download stream interrupted, will resume"
                        );
                        stream_error = true;
                        break;
                    }
                }

                if last_progress_update.elapsed() > std::time::Duration::from_secs(5) {
                    if expected_size > 0 {
                        let pct = (downloaded as f64) / (expected_size as f64) * 100.0;
                        let dl_mb = downloaded / (1024 * 1024);
                        let total_mb = expected_size / (1024 * 1024);
                        info!(
                            session_id = %session_id,
                            progress = format!("{pct:.0}%"),
                            downloaded_mb = dl_mb,
                            total_mb = total_mb,
                            "Download progress"
                        );
                    }

                    let pool = self.pool.clone();
                    let sid = session_id;
                    let path = dest.to_string_lossy().to_string();
                    tokio::spawn(async move {
                        let _ = sqlx::query!(
                            "UPDATE debrid_stream_sessions SET updated_at = NOW() WHERE id = $1",
                            sid,
                        )
                        .execute(&pool)
                        .await;
                        let _ = cache_files::touch(&pool, &path).await;
                    });

                    last_progress_update = tokio::time::Instant::now();
                }
            }

            if !stream_error {
                // Stream ended normally — download complete
                break;
            }

            // Stream broke — try to resume
            resume_attempts += 1;
            if resume_attempts > MAX_RESUME_ATTEMPTS {
                return Err(format!(
                    "Download failed after {MAX_RESUME_ATTEMPTS} resume attempts at {}MB",
                    downloaded / (1024 * 1024)
                )
                .into());
            }

            // Flush what we have before resuming
            let _ = file.flush().await;
            meter.record(downloaded as u64);

            let delay = std::time::Duration::from_secs(1 << resume_attempts.min(4));
            info!(
                session_id = %session_id,
                attempt = resume_attempts,
                downloaded_mb = downloaded / (1024 * 1024),
                delay_secs = delay.as_secs(),
                "Resuming download"
            );
            tokio::time::sleep(delay).await;
        }

        file.flush()
            .await
            .map_err(|e| format!("Flush error: {e}"))?;
        meter.record(downloaded as u64);

        info!(
            session_id = %session_id,
            size_mb = downloaded / (1024 * 1024),
            "File download complete"
        );

        Ok(())
    }

    /// Trigger the processing pipeline on a fully-cached file.
    ///
    /// Enqueues a `debrid_process` job that chains: probe → thumbnails → skip_detect.
    /// Each stage updates the session row and emits SSE events.
    async fn trigger_processing(&self, session_id: Uuid, cache_path: &Path) {
        let cache_path_str = cache_path.to_string_lossy().to_string();

        // Skip if a debrid_process job for this file is already pending/running
        let already_queued: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM job_status
                WHERE job_type = 'debrid_process'
                  AND status IN ('pending', 'running')
                  AND payload->>'cache_path' = $1
            )
            "#,
        )
        .bind(&cache_path_str)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false);

        if already_queued {
            info!(
                session_id = %session_id,
                path = %cache_path_str,
                "Skipping debrid_process — job already queued for this file"
            );
            return;
        }

        info!(
            session_id = %session_id,
            path = %cache_path_str,
            "Triggering processing pipeline for cached debrid file"
        );

        let payload = serde_json::json!({
            "session_id": session_id,
            "cache_path": cache_path_str,
        });
        let context = crate::jobs::JobContext::for_entity(
            "debrid_session",
            session_id,
            "Processing cached debrid stream",
        );
        if let Err(e) = crate::jobs::enqueue(
            &self.pool,
            &self.nats_publisher,
            "debrid_process",
            payload,
            context,
        )
        .await
        {
            error!(
                session_id = %session_id,
                error = %e,
                "Failed to enqueue debrid_process job"
            );
        }
    }
}

/// Whether an HTTP status opening the download is worth retrying.
///
/// Debrid CDN nodes return 429/5xx under load and recover within seconds;
/// 4xx (other than 429) means the link itself is dead and retrying just
/// burns the session's startup budget.
fn is_transient_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status.is_server_error()
}
