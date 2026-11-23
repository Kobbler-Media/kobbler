//! Download Ahead: halfway through an episode of a show being watched, the next episode's
//! release is downloaded whole and analysed, so it starts from disk with its intro and
//! credits known. A player opening the same release joins the download in flight.

use std::sync::Arc;

use dashmap::DashMap;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use uuid::Uuid;

use crate::AppState;
use crate::routes::helpers::build_kobload_client;
use crate::streaming::download_lane::DownloadLane;
use crate::streaming::download_meter::DownloadMeter;

pub const DOWNLOAD_AHEAD_JOB: &str = "debrid_download_ahead";

/// How long a queued download ahead of an episode stands before another may be queued.
const REQUEUE_AFTER_HOURS: f64 = 12.0;

/// How long after a debrid episode airs Up Next holds it back for a download, and the
/// new-episode watch keeps looking for one: releases trickle in over the first day or
/// two, and past a week nothing usable is coming.
pub const NEW_EPISODE_WAIT_DAYS: i32 = 7;

#[derive(Clone)]
struct InFlight {
    cancel: CancellationToken,
    stopped: CancellationToken,
    row_written: CancellationToken,
    meter: Arc<DownloadMeter>,
    session_id: Uuid,
    lane: Arc<DownloadLane>,
}

/// The download in flight, as a viewer reading along with it sees it.
pub struct Joined {
    pub meter: Arc<DownloadMeter>,
    pub session_id: Uuid,
    pub lane: Arc<DownloadLane>,
    pub stopped: CancellationToken,
    pub row_written: CancellationToken,
}

/// Background downloads by release key. Two writers on one cache file corrupt it, so a
/// player reads along with this one, and anything else that must write waits for it to
/// stop and resumes from what it left.
#[derive(Default)]
pub struct DownloadsAhead {
    flights: DashMap<String, InFlight>,
    damaged: dashmap::DashSet<String>,
}

pub struct Claim {
    owner: Arc<DownloadsAhead>,
    key: String,
    flight: InFlight,
}

impl Claim {
    pub fn cancel(&self) -> CancellationToken {
        self.flight.cancel.clone()
    }

    /// An upgrade stores its URLs on the row, so it waits until there is one.
    pub fn row_written(&self) {
        self.flight.row_written.cancel();
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.flight.stopped.cancel();
        self.owner
            .flights
            .remove_if(&self.key, |_, f| f.stopped.is_cancelled());
    }
}

impl DownloadsAhead {
    /// `None` when the release is already being downloaded ahead.
    pub fn claim(
        self: &Arc<Self>,
        key: &str,
        parent: &CancellationToken,
        meter: Arc<DownloadMeter>,
        session_id: Uuid,
        lane: Arc<DownloadLane>,
    ) -> Option<Claim> {
        let flight = InFlight {
            cancel: parent.child_token(),
            stopped: CancellationToken::new(),
            row_written: CancellationToken::new(),
            meter,
            session_id,
            lane,
        };
        match self.flights.entry(key.to_string()) {
            dashmap::Entry::Occupied(_) => None,
            dashmap::Entry::Vacant(v) => {
                v.insert(flight.clone());
                Some(Claim {
                    owner: self.clone(),
                    key: key.to_string(),
                    flight,
                })
            }
        }
    }

    pub fn contains(&self, key: &str) -> bool {
        self.flights.contains_key(key)
    }

    pub fn join(&self, key: &str) -> Option<Joined> {
        self.flights.get(key).map(|f| Joined {
            meter: f.meter.clone(),
            session_id: f.session_id,
            lane: f.lane.clone(),
            stopped: f.stopped.clone(),
            row_written: f.row_written.clone(),
        })
    }

    /// Damage is reported the first time a download ahead meets it; a later attempt at the
    /// same release would only meet and report it again.
    pub fn mark_damaged(&self, key: &str) {
        self.damaged.insert(key.to_string());
    }

    pub fn is_damaged(&self, key: &str) -> bool {
        self.damaged.contains(key)
    }

    /// Stop the download ahead of this release, if any, and wait until it has stopped
    /// writing.
    pub async fn hand_over(&self, key: &str) {
        let Some(flight) = self.flights.get(key).map(|f| f.clone()) else {
            return;
        };
        flight.cancel.cancel();
        flight.stopped.cancelled().await;
    }
}

/// Called on every episode position save past the halfway mark.
pub async fn consider(state: &AppState, user_id: Uuid, episode_id: Uuid) {
    let runtime = state.runtime_config.get().await;
    if build_kobload_client(&runtime).is_err() {
        return;
    }
    let pool = &state.pool;
    let next =
        match crate::repositories::tv_episodes::next_streamable_episode(pool, episode_id).await {
            Ok(Some(next)) => next,
            Ok(None) => return,
            Err(e) => {
                warn!(error = %e, %episode_id, "next episode lookup failed");
                return;
            }
        };
    let wanted = sqlx::query_scalar!(
        r#"
        SELECT NOT EXISTS(
                   SELECT 1 FROM tv_episode_file_episodes fe
                   JOIN tv_episode_files f ON f.id = fe.episode_file_id
                   WHERE fe.episode_id = $1 AND f.available)
               AND NOT EXISTS(
                   SELECT 1 FROM job_status
                   WHERE job_type = $2 AND payload->>'episode_id' = $1::text
                     AND created_at > NOW() - $3::float8 * INTERVAL '1 hour')
               AS "wanted!"
        "#,
        next,
        DOWNLOAD_AHEAD_JOB,
        REQUEUE_AFTER_HOURS,
    )
    .fetch_one(pool)
    .await
    .unwrap_or(false);
    if wanted {
        queue(state, user_id, next, None).await;
    }
}

/// `hold_days` keeps the download from expiring out of the cache before then.
pub async fn queue(state: &AppState, user_id: Uuid, episode_id: Uuid, hold_days: Option<i64>) {
    if let Err(e) = crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        DOWNLOAD_AHEAD_JOB,
        serde_json::json!({ "user_id": user_id, "episode_id": episode_id, "hold_days": hold_days }),
        crate::jobs::JobContext::for_entity(
            "tv_episode",
            episode_id,
            "Downloading the next episode ahead",
        ),
    )
    .await
    {
        warn!(error = %e, %episode_id, "failed to queue download ahead");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_player_takes_over_only_once_the_download_ahead_has_stopped() {
        let ahead = Arc::new(DownloadsAhead::default());
        let parent = CancellationToken::new();
        let claim_rel = || {
            ahead.claim(
                "rel",
                &parent,
                DownloadMeter::new(10, 0),
                Uuid::nil(),
                DownloadLane::warm(true),
            )
        };
        let claim = claim_rel().unwrap();
        assert!(claim_rel().is_none(), "two downloads of one release");

        let cancel = claim.cancel();
        let handed = tokio::spawn(async move {
            cancel.cancelled().await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            drop(claim);
        });
        let started = std::time::Instant::now();
        ahead.hand_over("rel").await;
        assert!(started.elapsed() >= std::time::Duration::from_millis(50));
        handed.await.unwrap();
        assert!(!ahead.contains("rel"));
        ahead.hand_over("rel").await;
    }
}
