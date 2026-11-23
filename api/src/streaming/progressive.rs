//! Progressive Download Manager for debrid streams.
//!
//! When a user streams via debrid, this module simultaneously downloads the full file
//! to a local cache. Once downloaded, it triggers the processing pipeline (probe →
//! keyframes → thumbnails → intro detection) and emits SSE events as features complete.
//!
//! The player starts immediately via debrid proxy, then progressively gains features
//! (timeline thumbnails, skip-intro buttons, subtitles) as processing completes.

use crate::repositories::debrid_cache_files::{self as cache_files, CacheRelease};
use crate::streaming::byte_range::content_range_start;
use crate::streaming::cache_writers;
use crate::streaming::download_lane::DownloadLane;
use crate::streaming::download_meter::DownloadMeter;
use crate::streaming::resume::{self, Interruption, Resume};
use nats_jobs::Publisher;
use sqlx::PgPool;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

pub type UrlRefresh =
    Arc<dyn Fn(bool) -> futures::future::BoxFuture<'static, Option<String>> + Send + Sync>;

struct Sink<'a> {
    dest: &'a Path,
    session_id: Uuid,
    expected_size: i64,
    cancel: &'a CancellationToken,
    meter: &'a DownloadMeter,
}

/// A host still refusing after this many retries has not handed over a single byte: the
/// client is watching a spinner over it, so it is a dead link, not a hiccup.
const MAX_ATTEMPTS_BEFORE_FIRST_BYTE: u32 = 1;

#[derive(Debug)]
pub enum DownloadError {
    /// The host answered, and not with the file.
    Refused(String),
    /// The release itself cannot be served past some point; the reason is kobload's.
    Dead(String),
    Failed(String),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(m) | Self::Dead(m) | Self::Failed(m) => f.write_str(m),
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
    pub lane: Arc<DownloadLane>,
}

/// Manages progressive background downloads of debrid stream files.
pub struct ProgressiveDownloadManager {
    config: ProgressiveDownloadConfig,
    pool: PgPool,
    http_client: reqwest::Client,
    nats_publisher: Publisher,
    refresh: Option<UrlRefresh>,
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
            refresh: None,
        }
    }

    /// Where to get a fresh URL between attempts; `true` asks for one even if the current
    /// URL has not expired.
    pub fn with_refresh(mut self, refresh: UrlRefresh) -> Self {
        self.refresh = Some(refresh);
        self
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
            lane,
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
                (&debrid_url, lane.upgrades()),
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
                if let Err(e) = cache_files::mark_complete(&self.pool, &cache_path_str).await {
                    warn!(session_id = %session_id, error = %e, "Failed to mark cache file complete");
                }
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

    /// Download file from URL to local path with progress tracking, resuming over Range
    /// whenever the connection drops, and taking a fresh URL when the host refuses a lapsed
    /// one.
    async fn download_file(
        &self,
        (url, mut upgrades): (&str, watch::Receiver<Option<String>>),
        dest: &Path,
        session_id: Uuid,
        expected_size: i64,
        cancel: &CancellationToken,
        meter: &DownloadMeter,
    ) -> Result<(), DownloadError> {
        let _writer = until_cancelled(cancel, cache_writers::lock(dest)).await?;
        let mut downloaded: i64 = 0;
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

        let mut url = url.to_string();
        // A viewer can join between the target being built and this download subscribing.
        adopt(&upgrades, &mut url);
        let mut resume = Resume::new();
        let mut want_fresh = false;
        let mut refreshed_after_refusal = false;
        loop {
            let req = if downloaded > 0 {
                self.http_client
                    .get(&url)
                    .header("Range", format!("bytes={downloaded}-"))
            } else {
                self.http_client.get(&url)
            };
            let Some(sent) = until_cancelled(cancel, or_upgrade(&mut upgrades, req.send())).await?
            else {
                adopt(&upgrades, &mut url);
                refreshed_after_refusal = false;
                continue;
            };
            let interruption = match sent {
                Err(e) if e.is_connect() => {
                    warn!(session_id = %session_id, error = %e.without_url(), "Download host unreachable, will retry");
                    Interruption::Unreachable
                }
                Err(e) => {
                    warn!(session_id = %session_id, error = %e.without_url(), "Download host cut the response, will retry");
                    Interruption::Aborted
                }
                Ok(resp) if !resp.status().is_success() => {
                    let status = resp.status();
                    let code = resume::error_code(resp).await;
                    if let Some(code) = code.clone().filter(|c| resume::dead_code(c)) {
                        return Err(DownloadError::Dead(code));
                    }
                    let forgotten = code.as_deref() == Some(resume::FORGOTTEN_CODE);
                    if (resume::is_lapsed(status) || forgotten)
                        && self.refresh.is_some()
                        && !refreshed_after_refusal
                    {
                        refreshed_after_refusal = true;
                        want_fresh = true;
                        Interruption::Busy
                    } else if code.as_deref().is_some_and(resume::busy_code) {
                        Interruption::Unavailable
                    } else if !resume::is_transient_status(status) {
                        return Err(DownloadError::Refused(format!("HTTP {status}")));
                    } else if downloaded == 0 && resume.attempts() >= MAX_ATTEMPTS_BEFORE_FIRST_BYTE
                    {
                        return Err(DownloadError::Refused(format!(
                            "HTTP {status} after {MAX_ATTEMPTS_BEFORE_FIRST_BYTE} retries"
                        )));
                    } else {
                        Interruption::Busy
                    }
                }
                Ok(resp) if !starts_at(&resp, downloaded) => {
                    warn!(
                        session_id = %session_id,
                        asked = downloaded,
                        answered = ?resp.headers().get(reqwest::header::CONTENT_RANGE),
                        "Download host answered another range, asking again"
                    );
                    Interruption::Aborted
                }
                Ok(resp) => {
                    // A server that ignores our Range header answers 200 with the whole
                    // file from byte 0. Appending that to a partial download silently
                    // corrupts the cache, so start the file over instead.
                    if downloaded > 0 && resp.status() == reqwest::StatusCode::OK {
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
                    let sink = Sink {
                        dest,
                        session_id,
                        expected_size,
                        cancel,
                        meter,
                    };
                    let pumped = self
                        .pump(
                            resp,
                            &mut file,
                            &mut downloaded,
                            &mut resume,
                            &sink,
                            &mut upgrades,
                        )
                        .await?;
                    if pumped == Pumped::Upgraded {
                        adopt(&upgrades, &mut url);
                        refreshed_after_refusal = false;
                        if expected_size > 0 && downloaded >= expected_size {
                            break;
                        }
                        continue;
                    }
                    let ended = pumped == Pumped::Ended;
                    if ended && (expected_size <= 0 || downloaded >= expected_size) {
                        break;
                    }
                    if ended {
                        warn!(
                            session_id = %session_id,
                            downloaded_mb = downloaded / (1024 * 1024),
                            total_mb = expected_size / (1024 * 1024),
                            "Download host ended the response before the file, resuming"
                        );
                    }
                    refreshed_after_refusal = false;
                    Interruption::Aborted
                }
            };

            let Some(delay) = resume.interrupted(interruption) else {
                return Err(DownloadError::Failed(format!(
                    "no progress at {}MB after {} attempts ({interruption:?})",
                    downloaded / (1024 * 1024),
                    resume.attempts()
                )));
            };
            let _ = file.flush().await;
            meter.record(downloaded as u64);
            info!(
                session_id = %session_id,
                attempt = resume.attempts(),
                downloaded_mb = downloaded / (1024 * 1024),
                delay_ms = delay.as_millis() as u64,
                ?interruption,
                "Resuming download"
            );
            if until_cancelled(cancel, or_upgrade(&mut upgrades, tokio::time::sleep(delay)))
                .await?
                .is_none()
            {
                adopt(&upgrades, &mut url);
                refreshed_after_refusal = false;
                want_fresh = false;
                continue;
            }
            if let Some(refresh) = &self.refresh
                && let Some(fresh) = until_cancelled(cancel, refresh(want_fresh)).await?
            {
                url = fresh;
            }
            want_fresh = false;
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

    /// Append one response's body to the file until it ends, is cut, or the lane moves.
    async fn pump(
        &self,
        resp: reqwest::Response,
        file: &mut tokio::fs::File,
        downloaded: &mut i64,
        resume: &mut Resume,
        sink: &Sink<'_>,
        upgrades: &mut watch::Receiver<Option<String>>,
    ) -> Result<Pumped, DownloadError> {
        use futures_util::StreamExt;
        let Sink {
            dest,
            session_id,
            expected_size,
            cancel,
            meter,
        } = *sink;
        let mut last_progress_update = tokio::time::Instant::now();
        let mut stream = resp.bytes_stream();
        loop {
            let Some(next) = until_cancelled(cancel, or_upgrade(upgrades, stream.next())).await?
            else {
                return Ok(Pumped::Upgraded);
            };
            let Some(chunk) = next else {
                return Ok(Pumped::Ended);
            };
            let data = match chunk {
                Ok(data) => data,
                Err(e) => {
                    warn!(
                        session_id = %session_id,
                        downloaded_mb = *downloaded / (1024 * 1024),
                        error = %e.without_url(),
                        "Download stream interrupted, will resume"
                    );
                    return Ok(Pumped::Cut);
                }
            };
            if cancel.is_cancelled() {
                return Err(cancelled());
            }
            if let Err(e) = file.write_all(&data).await {
                return Err(format!("Write error: {e}").into());
            }
            // tokio's File returns from a write before it lands. A local read
            // follows this file, so the meter must count exactly what a reader
            // can see: never less, or a client is served bytes reported missing.
            file.flush()
                .await
                .map_err(|e| format!("Flush error: {e}"))?;
            *downloaded += data.len() as i64;
            meter.record(*downloaded as u64);
            if !data.is_empty() {
                resume.progressed();
            }

            if last_progress_update.elapsed() > std::time::Duration::from_secs(5) {
                if expected_size > 0 {
                    let pct = (*downloaded as f64) / (expected_size as f64) * 100.0;
                    info!(
                        session_id = %session_id,
                        progress = format!("{pct:.0}%"),
                        downloaded_mb = *downloaded / (1024 * 1024),
                        total_mb = expected_size / (1024 * 1024),
                        "Download progress"
                    );
                }
                let pool = self.pool.clone();
                let path = dest.to_string_lossy().to_string();
                tokio::spawn(async move {
                    let _ = sqlx::query!(
                        "UPDATE debrid_stream_sessions SET updated_at = NOW() WHERE id = $1",
                        session_id,
                    )
                    .execute(&pool)
                    .await;
                    let _ = cache_files::touch(&pool, &path).await;
                });
                last_progress_update = tokio::time::Instant::now();
            }
        }
    }

    /// Trigger the processing pipeline on a fully-cached file.
    ///
    /// Enqueues a `debrid_process` job that chains: probe → thumbnails → skip_detect.
    /// Each stage updates the session row and emits SSE events.
    async fn trigger_processing(&self, session_id: Uuid, cache_path: &Path) {
        crate::jobs::handlers::debrid_process::enqueue(
            &self.pool,
            &self.nats_publisher,
            session_id,
            cache_path,
        )
        .await;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pumped {
    Ended,
    Cut,
    Upgraded,
}

/// `None` when the lane moved first; the abandoned work has written nothing.
async fn or_upgrade<T>(
    upgrades: &mut watch::Receiver<Option<String>>,
    work: impl std::future::Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        biased;
        Ok(()) = upgrades.changed() => None,
        out = work => Some(out),
    }
}

fn adopt(upgrades: &watch::Receiver<Option<String>>, url: &mut String) {
    if let Some(upgraded) = upgrades.borrow().clone() {
        *url = upgraded;
    }
}

fn cancelled() -> DownloadError {
    DownloadError::Failed("Cancelled".to_string())
}

/// A cancelled download must not go on to write: whoever resumes it appends alongside.
async fn until_cancelled<T>(
    cancel: &CancellationToken,
    work: impl std::future::Future<Output = T>,
) -> Result<T, DownloadError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(cancelled()),
        out = work => Ok(out),
    }
}

/// A 206 must start where the download asked it to.
fn starts_at(resp: &reqwest::Response, downloaded: i64) -> bool {
    resp.status() != reqwest::StatusCode::PARTIAL_CONTENT
        || resp
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(content_range_start)
            == Some(downloaded as u64)
}
