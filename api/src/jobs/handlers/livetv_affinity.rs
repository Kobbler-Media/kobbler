use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::livetv::affinity::{self, WatchFact};
use crate::repositories::{livetv_affinity, livetv_watch_events};
use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// The window the affinity rows are computed over. Longer than a season so a
/// weekly series still reads as a habit; shorter than the retention window so
/// last year's viewing cannot keep a dead habit alive.
const ROLLUP_WINDOW_DAYS: i64 = 180;

/// Raw events are kept well past the rollup window — everything else derives
/// from them, so a scoring change can be replayed rather than lost.
const EVENT_RETENTION_DAYS: i64 = 365;

pub struct LivetvAffinityHandler {
    pool: PgPool,
}

impl LivetvAffinityHandler {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn recompute_user(&self, user_id: Uuid, since: OffsetDateTime) -> JobResult<usize> {
        let facts: Vec<WatchFact> = livetv_watch_events::facts_for_user(&self.pool, user_id, since)
            .await?
            .into_iter()
            .map(|r| WatchFact {
                user_id: r.user_id,
                channel_id: r.channel_id,
                series_id: r.series_id,
                genres: r.genres,
                started_at: r.started_at,
                watched_secs: r.watched_secs,
                programme_secs: r.programme_secs,
            })
            .collect();

        let zone = crate::notifications::prefs::zone_for_user(&self.pool, user_id).await;
        let rows = affinity::roll_up(&facts, |t| zone.offset_at(t));

        let mut tx = self.pool.begin().await?;
        livetv_affinity::delete_for_user(&mut *tx, user_id).await?;
        for (row_user, key, stats) in &rows {
            livetv_affinity::upsert(
                &mut *tx,
                *row_user,
                livetv_affinity::UpsertAffinity {
                    series_id: key.series_id.clone(),
                    channel_id: key.channel_id,
                    genre: key.genre.clone(),
                    weekday: key.weekday,
                    hour: key.hour,
                    watch_count: stats.watch_count,
                    total_secs: stats.total_secs,
                    mean_completion: stats.mean_completion,
                    last_watched_at: Some(stats.last_watched_at),
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(rows.len())
    }
}

#[async_trait]
impl JobHandler for LivetvAffinityHandler {
    fn job_type(&self) -> &'static str {
        "livetv_affinity"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        job_id: Uuid,
        _payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        nats_jobs::check_cancelled(&cancel)?;
        debug!(job_id = %job_id, "Rolling up Live TV watch history");

        let pruned = livetv_watch_events::prune_older_than(&self.pool, EVENT_RETENTION_DAYS)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Pruning watch events failed: {e}")))?;

        let orphaned = livetv_affinity::delete_orphaned(&self.pool).await?;
        let users = livetv_affinity::distinct_users(&self.pool).await?;
        let since = OffsetDateTime::now_utc() - time::Duration::days(ROLLUP_WINDOW_DAYS);

        let mut rows = 0usize;
        let mut recomputed = 0usize;
        for user_id in &users {
            nats_jobs::check_cancelled(&cancel)?;
            match self.recompute_user(*user_id, since).await {
                Ok(n) => {
                    rows += n;
                    recomputed += 1;
                }
                Err(e) => {
                    warn!(user_id = %user_id, error = %e, "Live TV affinity rollup failed for a user")
                }
            }
        }

        info!(
            users = recomputed,
            rows, pruned, orphaned, "Live TV affinity rollup complete"
        );
        Ok(json!({ "users": recomputed, "rows": rows, "pruned": pruned, "orphaned": orphaned }))
    }
}
