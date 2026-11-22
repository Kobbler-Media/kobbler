//! A debrid release's black bars, measured as soon as its length is known rather than
//! after the download and its analysis: by then the viewer has watched through them.

use std::time::Duration;

use tracing::{info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::debrid_outcomes::StreamRelease;
use crate::media_cache::picture_area::{self, PictureArea};
use crate::repositories::debrid_stream_sessions;

const GIVE_UP_AFTER: Duration = Duration::from_secs(30 * 60);

/// What a session response carries: nothing when the picture fills the frame.
pub(crate) fn shown(stored: serde_json::Value) -> Option<PictureArea> {
    serde_json::from_value::<PictureArea>(stored)
        .ok()
        .filter(|area| *area != PictureArea::FULL)
}

/// The area an earlier session of the release measured, else a fresh measurement of `source`.
pub(crate) async fn measured(
    pool: &sqlx::PgPool,
    release: &StreamRelease,
    source: &str,
    duration_secs: f64,
) -> Option<serde_json::Value> {
    if let Some(hash) = release.release_key.as_deref()
        && let Ok(Some(known)) =
            debrid_stream_sessions::release_picture(pool, hash, release.file_id).await
    {
        return Some(known);
    }
    let area = picture_area::check_source(source, duration_secs).await?;
    serde_json::to_value(area).ok()
}

pub(super) async fn measure_once_probed(
    state: AppState,
    session_id: Uuid,
    release: StreamRelease,
    source: String,
    duration_secs: f64,
) {
    let Some(picture) = measured(&state.pool, &release, &source, duration_secs).await else {
        return;
    };
    // A warm session has no row until it is claimed, which can be the length of an episode away.
    let deadline = tokio::time::Instant::now() + GIVE_UP_AFTER;
    while state.playback_sessions.contains_key(&session_id)
        && tokio::time::Instant::now() < deadline
    {
        match debrid_stream_sessions::set_picture(&state.pool, session_id, &picture).await {
            Ok(true) => {
                info!(session_id = %session_id, %picture, "Measured debrid picture area");
                return;
            }
            Ok(false) => tokio::time::sleep(Duration::from_secs(3)).await,
            Err(e) => {
                warn!(session_id = %session_id, error = %e, "Failed to store debrid picture area");
                return;
            }
        }
    }
}
