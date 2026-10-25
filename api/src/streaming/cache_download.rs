use std::path::PathBuf;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::AppState;
use crate::streaming::download_meter::DownloadMeter;
use crate::streaming::progressive::{
    DownloadError, DownloadTarget, ProgressiveDownloadConfig, ProgressiveDownloadManager,
};
use crate::streaming::session::SessionError;

pub fn cache_dir(state: &AppState, runtime: &crate::runtime_config::RuntimeConfig) -> PathBuf {
    runtime
        .debrid_cache_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&state.config.storage_root).join("debrid_cache"))
}

/// Download `target` into the cache until it is complete or `cancel` fires. A partial
/// file already on disk is resumed.
pub async fn download(
    state: &AppState,
    target: DownloadTarget,
    cancel: CancellationToken,
    meter: Arc<DownloadMeter>,
) -> Result<(), String> {
    fetch(state, target, cancel, meter)
        .await
        .map_err(|e| e.to_string())
}

async fn fetch(
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
        .build()
        .map_err(|e| DownloadError::Failed(format!("download client: {e}")))?;
    let manager = Arc::new(ProgressiveDownloadManager::new(
        config,
        state.pool.clone(),
        client,
        state.nats.publisher().clone(),
    ));
    manager.start_download(target, cancel, meter).await
}

pub fn spawn(
    state: &AppState,
    target: DownloadTarget,
    cancel: CancellationToken,
    meter: Arc<DownloadMeter>,
) {
    let state = state.clone();
    tokio::spawn(async move {
        let session_id = target.session_id;
        match fetch(&state, target, cancel, meter.clone()).await {
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
            Err(e) => {
                warn!(%session_id, error = %e, "Download did not complete; leaving HybridIo on HTTP fallback");
            }
        }
    });
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
