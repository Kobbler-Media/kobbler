use nats_jobs::{StreamOption, StreamOutcomeReport, StreamPlayback, StreamPreference};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::AppState;
use crate::jobs::JobContext;
use crate::repositories::debrid_new_episodes;
use crate::repositories::debrid_stream_sessions::{self as sessions, PlayedRelease};
use crate::routes::helpers::build_kobload_client;

pub const PREWARM_JOB: &str = "debrid_prewarm";

#[derive(Clone)]
pub struct StreamRelease {
    pub debrid_provider_id: Option<Uuid>,
    pub release_key: Option<String>,
    pub file_id: Option<i32>,
    pub release_title: String,
    pub filename: String,
}

impl From<sessions::SessionRelease> for StreamRelease {
    fn from(r: sessions::SessionRelease) -> Self {
        Self {
            debrid_provider_id: Uuid::parse_str(&r.debrid_provider).ok(),
            release_key: r.release_key,
            file_id: r.file_id,
            release_title: r.release_title,
            filename: r.filename,
        }
    }
}

/// An NZB kobload never resolved has no key yet, and kobload keys every outcome by one.
async fn forward(state: &AppState, release: &StreamRelease, ok: bool, reason: Option<&str>) {
    let Some(release_key) = release.release_key.as_deref() else {
        return;
    };
    let runtime = state.runtime_config.get().await;
    let client = match build_kobload_client(&runtime) {
        Ok(c) => c,
        Err(e) => {
            debug!(error = ?e, "kobload not configured; stream outcome dropped");
            return;
        }
    };
    let report = StreamOutcomeReport {
        debrid_provider_id: release.debrid_provider_id,
        release_key: release_key.to_string(),
        file_id: release.file_id,
        ok,
        reason: reason.map(String::from),
        release_title: Some(release.release_title.clone()).filter(|t| !t.is_empty()),
        filename: Some(release.filename.clone()),
    };
    if let Err(e) = client.stream_outcome(&report).await {
        warn!(error = %e, release_key, ok, "failed to forward stream outcome to kobload");
    }
}

pub async fn report_failure(state: &AppState, release: &StreamRelease, reason: &str) {
    forward(state, release, false, Some(reason)).await;
}

/// Counts once per session: the first report that it played forwards the success and,
/// for an episode, queues a search for the next one so kobload warms it in advance.
pub async fn record_played(state: &AppState, session_id: Uuid) {
    let played = match sessions::mark_played(&state.pool, session_id).await {
        Ok(Some(p)) if p.first => p,
        Ok(_) => return,
        Err(e) => {
            warn!(error = %e, %session_id, "failed to mark stream session played");
            return;
        }
    };
    forward(
        state,
        &StreamRelease {
            debrid_provider_id: Uuid::parse_str(&played.debrid_provider).ok(),
            release_key: played.release_key.clone(),
            file_id: played.file_id,
            release_title: played.release_title.clone(),
            filename: played.filename.clone(),
        },
        true,
        None,
    )
    .await;

    if played.media_type == "episode" {
        queue_prewarm(state, played.user_id, played.media_id).await;
    }
}

async fn queue_prewarm(state: &AppState, user_id: Uuid, after: Uuid) {
    let next = match debrid_new_episodes::preload_due(&state.pool, PREWARM_JOB, after).await {
        Ok(Some(next)) => next,
        Ok(None) => return,
        Err(e) => {
            warn!(error = %e, %after, "next-episode prewarm check failed");
            return;
        }
    };
    if let Err(e) = crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        PREWARM_JOB,
        serde_json::json!({ "user_id": user_id, "episode_id": next }),
        JobContext::for_entity("episode", next, "Warming streams for the next episode"),
    )
    .await
    {
        warn!(error = %e, episode_id = %next, "failed to queue next-episode prewarm");
    }
}

fn preference(played: PlayedRelease) -> StreamPreference {
    StreamPreference {
        release_key: played.release_key,
        file_id: played.file_id,
        release_title: Some(played.release_title),
    }
}

/// The release that last played for this title, skipping ones whose local cache is gone.
pub async fn preferred_stream(
    pool: &sqlx::PgPool,
    media_type: &str,
    media_id: Uuid,
) -> Option<StreamPreference> {
    let candidates = sessions::played_for_title(pool, media_type, media_id)
        .await
        .unwrap_or_else(|e| {
            warn!(error = %e, "preferred stream lookup failed");
            Vec::new()
        });
    for c in candidates {
        if let Some(ref path) = c.cache_path
            && tokio::fs::metadata(path).await.is_err()
        {
            continue;
        }
        return Some(preference(c));
    }
    None
}

pub struct SearchPreference {
    /// The stream that already played for this title, offered back to the client.
    pub replay: Option<StreamPreference>,
    /// What kobload should favour: the replay, else (for an episode) what this user last
    /// played from the same show.
    pub prefer: Option<StreamPreference>,
}

pub async fn search_preference(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    movie_id: Option<Uuid>,
    episode_id: Option<Uuid>,
) -> SearchPreference {
    let replay = match (movie_id, episode_id) {
        (Some(id), _) => preferred_stream(pool, "movie", id).await,
        (None, Some(id)) => preferred_stream(pool, "episode", id).await,
        (None, None) => None,
    };
    let prefer = match (&replay, episode_id) {
        (Some(r), _) => Some(r.clone()),
        (None, Some(id)) if movie_id.is_none() => sessions::last_played_in_show(pool, user_id, id)
            .await
            .unwrap_or_else(|e| {
                warn!(error = %e, "continuity lookup failed");
                None
            })
            .map(preference),
        _ => None,
    };
    SearchPreference { replay, prefer }
}

/// A replay preference is only worth surfacing while kobload still believes it plays.
pub fn playable_preference(
    preferred: Option<StreamPreference>,
    options: &[StreamOption],
) -> Option<StreamPreference> {
    preferred.filter(|p| {
        !options.iter().any(|o| {
            o.playback == Some(StreamPlayback::Failed)
                && o.release_key
                    .as_deref()
                    .is_some_and(|h| h.eq_ignore_ascii_case(&p.release_key))
                && (p.file_id.is_none() || o.files.first().map(|f| f.id) == p.file_id)
        })
    })
}
