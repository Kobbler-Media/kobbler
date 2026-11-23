use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::AppState;
use crate::debrid_outcomes::{self, StreamRelease};
use crate::routes::admin::release_key_param::is_plain;
use crate::streaming::download_lane::DownloadLane;
use crate::streaming::download_meter::DownloadMeter;
use crate::streaming::progressive::{
    DownloadError, DownloadTarget, ProgressiveDownloadConfig, ProgressiveDownloadManager,
    UrlRefresh,
};
use crate::streaming::session::SessionError;

pub fn cache_dir(state: &AppState, runtime: &crate::runtime_config::RuntimeConfig) -> PathBuf {
    runtime
        .debrid_cache_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&state.config.storage_root).join("debrid_cache"))
}

/// Where a release's file lives in the cache. Keys and filenames come from kobload and the
/// provider, and `Path::join` lets an absolute or `..` part leave `dir`, so `None` refuses them.
pub fn cache_file(dir: &Path, key: &str, filename: &str) -> Option<PathBuf> {
    if !is_plain(key) {
        return None;
    }
    let name = filename.rsplit(['/', '\\']).next()?;
    if matches!(name, "" | "." | "..") || name.contains('\0') {
        return None;
    }
    Some(dir.join(key).join(name))
}

/// Download `target` into the cache until it is complete or `cancel` fires. A partial
/// file already on disk is resumed.
pub async fn download(
    state: &AppState,
    target: DownloadTarget,
    cancel: CancellationToken,
    meter: Arc<DownloadMeter>,
) -> Result<(), DownloadError> {
    let runtime = state.runtime_config.get().await;
    let config = ProgressiveDownloadConfig {
        cache_dir: cache_dir(state, &runtime),
        default_ttl_secs: runtime.debrid_cache_ttl_secs,
        max_cache_bytes: runtime.debrid_cache_max_bytes,
    };
    let client = crate::http::builder()
        .timeout(std::time::Duration::from_secs(3600))
        .read_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| DownloadError::Failed(format!("download client: {e}")))?;
    let manager = Arc::new(
        ProgressiveDownloadManager::new(
            config,
            state.pool.clone(),
            client,
            state.nats.publisher().clone(),
        )
        .with_refresh(fresh_url(state, target.session_id, target.lane.clone())),
    );
    let release = StreamRelease {
        debrid_provider_id: target.release.debrid_provider_id,
        release_key: target.release.release_key.clone(),
        file_id: target.release.file_id,
        release_title: String::new(),
        filename: target.release.filename.clone(),
    };
    let session_id = target.session_id;
    let outcome = manager.start_download(target, cancel, meter.clone()).await;
    if let Err(DownloadError::Dead(reason)) = &outcome
        && meter.first_failure_report()
    {
        warn!(%session_id, %reason, "the release cannot be downloaded past where it stopped");
        debrid_outcomes::report_failure(state, &release, reason).await;
    }
    outcome
}

fn fresh_url(state: &AppState, session_id: uuid::Uuid, lane: Arc<DownloadLane>) -> UrlRefresh {
    let state = state.clone();
    Arc::new(move |force| {
        let state = state.clone();
        let priority = lane.priority();
        Box::pin(async move {
            crate::routes::admin::debrid_play::fresh_cache_url(&state, session_id, force, priority)
                .await
        })
    })
}

pub fn spawn(
    state: &AppState,
    target: DownloadTarget,
    cancel: CancellationToken,
    meter: Arc<DownloadMeter>,
) {
    let state = state.clone();
    tokio::spawn(async move { for_session(&state, target, cancel, meter).await });
}

/// A session's own cache-ahead. One that gives up is left stopped, so the session's next
/// read starts it again.
pub async fn for_session(
    state: &AppState,
    target: DownloadTarget,
    cancel: CancellationToken,
    meter: Arc<DownloadMeter>,
) {
    let session_id = target.session_id;
    match download(state, target, cancel.clone(), meter.clone()).await {
        Ok(()) => {}
        // Nothing landed and the host will not serve it: the probe and the player would
        // each only find that out by retrying the same link until they gave up.
        Err(DownloadError::Refused(reason)) if meter.downloaded() == 0 => {
            warn!(%session_id, %reason, "Debrid host refused the file before its first byte; failing the session");
            if let Some(mut session) = state.playback_sessions.get_mut(&session_id) {
                session.error = Some(SessionError::new(
                    "source_refused",
                    format!("The debrid host refused the file ({reason})"),
                ));
            }
        }
        Err(DownloadError::Failed(reason)) => {
            warn!(%session_id, %reason, "Download gave up; the next read restarts it");
            cancel.cancel();
        }
        Err(e) => {
            warn!(%session_id, error = %e, "Download did not complete; leaving HybridIo on HTTP fallback");
        }
    }
}

/// Stop a download once `head` bytes of it are on disk, leaving the rest for later.
pub fn stop_after_head(meter: Arc<DownloadMeter>, head: u64, cancel: CancellationToken) {
    tokio::spawn(async move {
        loop {
            if cancel.is_cancelled() || meter.is_complete() {
                return;
            }
            if meter.downloaded() >= head {
                cancel.cancel();
                return;
            }
            meter
                .wait_for_growth(std::time::Duration::from_millis(250))
                .await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cache_file_stays_in_its_release_directory() {
        let dir = Path::new("/cache");
        let inside = Some(PathBuf::from("/cache/key_f1/x.mkv"));
        for name in [
            "x.mkv",
            "../../x.mkv",
            "/etc/x.mkv",
            "..\\..\\x.mkv",
            "a/b/x.mkv",
        ] {
            assert_eq!(cache_file(dir, "key_f1", name), inside, "{name}");
        }
        for name in ["", ".", "..", "a/..", "a/.", "a/", "a\\..", "x\0.mkv"] {
            assert_eq!(cache_file(dir, "key", name), None, "{name:?}");
        }
        let long = "k".repeat(129);
        for key in [
            "",
            ".",
            "..",
            "../x",
            "/abs",
            "a/b",
            "a\\b",
            "a b",
            long.as_str(),
        ] {
            assert_eq!(cache_file(dir, key, "x.mkv"), None, "{key:?}");
        }
    }
}
