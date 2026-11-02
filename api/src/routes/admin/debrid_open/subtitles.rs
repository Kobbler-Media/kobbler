//! Names a debrid stream's subtitle tracks from the first stretch of its download, so a
//! release that ships them untagged is labelled minutes into playback rather than once the
//! whole file has arrived. See `streaming::subtitle_insight`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::repositories::debrid_stream_sessions;
use crate::streaming::download_meter::DownloadMeter;
use crate::streaming::subtitle_insight::{WINDOW_SECS, describe_tracks};

/// A release's bitrate is not even through the file, and Matroska interleaves a cluster
/// or two ahead: a little more than the window's share of the bytes covers it.
const HEAD_MARGIN: f64 = 1.15;
const HEAD_SLACK_BYTES: u64 = 8 * 1024 * 1024;
const GIVE_UP_AFTER: Duration = Duration::from_secs(30 * 60);

pub(super) async fn name_once_head_arrives(
    state: AppState,
    session_id: Uuid,
    path: PathBuf,
    meter: Arc<DownloadMeter>,
    duration_secs: f64,
) {
    let total = meter.total();
    let head = if duration_secs > 0.0 && total > 0 {
        ((total as f64 * (WINDOW_SECS / duration_secs).min(1.0) * HEAD_MARGIN) as u64
            + HEAD_SLACK_BYTES)
            .min(total)
    } else {
        total
    };
    let deadline = tokio::time::Instant::now() + GIVE_UP_AFTER;
    while !meter.is_complete() && meter.downloaded() < head {
        if tokio::time::Instant::now() > deadline {
            return;
        }
        meter.wait_for_growth(Duration::from_secs(5)).await;
    }

    let duration = (duration_secs > 0.0).then_some(duration_secs);
    let tracks = match tokio::task::spawn_blocking(move || describe_tracks(&path, duration)).await {
        Ok(Ok(tracks)) if !tracks.is_empty() => serde_json::Value::Array(tracks),
        Ok(Ok(_)) => return,
        Ok(Err(e)) => {
            warn!(session_id = %session_id, error = %e, "Could not read debrid subtitle text");
            return;
        }
        Err(e) => {
            warn!(session_id = %session_id, error = %e, "Debrid subtitle read panicked");
            return;
        }
    };
    for _ in 0..10 {
        match debrid_stream_sessions::set_subtitle_tracks(&state.pool, session_id, &tracks).await {
            Ok(true) => {
                info!(session_id = %session_id, "Named debrid subtitle tracks");
                return;
            }
            Ok(false) => tokio::time::sleep(Duration::from_secs(3)).await,
            Err(e) => {
                warn!(session_id = %session_id, error = %e, "Failed to store debrid subtitle tracks");
                return;
            }
        }
    }
}
