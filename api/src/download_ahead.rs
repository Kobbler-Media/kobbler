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
use crate::streaming::download_meter::DownloadMeter;

pub const DOWNLOAD_AHEAD_JOB: &str = "debrid_download_ahead";

/// How long a queued download ahead of an episode stands before another may be queued.
const REQUEUE_AFTER_HOURS: f64 = 12.0;

#[derive(Clone)]
struct InFlight {
    cancel: CancellationToken,
    stopped: CancellationToken,
    meter: Arc<DownloadMeter>,
}

/// Background downloads by release key. Two writers on one cache file corrupt it, so a
/// player reads along with this one, and anything else that must write waits for it to
/// stop and resumes from what it left.
#[derive(Default)]
pub struct DownloadsAhead(DashMap<String, InFlight>);

pub struct Claim {
    owner: Arc<DownloadsAhead>,
    key: String,
    flight: InFlight,
}

impl Claim {
    pub fn cancel(&self) -> CancellationToken {
        self.flight.cancel.clone()
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.flight.stopped.cancel();
        self.owner
            .0
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
    ) -> Option<Claim> {
        let flight = InFlight {
            cancel: parent.child_token(),
            stopped: CancellationToken::new(),
            meter,
        };
        match self.0.entry(key.to_string()) {
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
        self.0.contains_key(key)
    }

    /// The meter of the download in flight, for a player to read the file along with it.
    pub fn join(&self, key: &str) -> Option<Arc<DownloadMeter>> {
        self.0.get(key).map(|f| f.meter.clone())
    }

    /// Stop the download ahead of this release, if any, and wait until it has stopped
    /// writing.
    pub async fn hand_over(&self, key: &str) {
        let Some(flight) = self.0.get(key).map(|f| f.clone()) else {
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
    if !wanted {
        return;
    }
    if let Err(e) = crate::jobs::enqueue(
        pool,
        state.nats.publisher(),
        DOWNLOAD_AHEAD_JOB,
        serde_json::json!({ "user_id": user_id, "episode_id": next }),
        crate::jobs::JobContext::for_entity(
            "tv_episode",
            next,
            "Downloading the next episode ahead",
        ),
    )
    .await
    {
        warn!(error = %e, %next, "failed to queue download ahead");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_player_takes_over_only_once_the_download_ahead_has_stopped() {
        let ahead = Arc::new(DownloadsAhead::default());
        let parent = CancellationToken::new();
        let meter = || DownloadMeter::new(10, 0);
        let claim = ahead.claim("rel", &parent, meter()).unwrap();
        assert!(
            ahead.claim("rel", &parent, meter()).is_none(),
            "two downloads of one release"
        );

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
