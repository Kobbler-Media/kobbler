use std::collections::HashSet;

use sqlx::PgPool;
use uuid::Uuid;

use super::Result;

// Air dates are stored as the broadcast date and compared to the database's UTC date, where
// a US evening premiere is already tomorrow.
const RECENT_AIR_DAYS: i32 = 2;
const RECENT_RETRY_HOURS: f64 = 2.0;
const RETRY_HOURS: f64 = 12.0;
// Real early releases land at most this far ahead of the listed date; earlier ones were fakes.
const EARLY_RELEASE_DAYS: i32 = 3;

pub struct Due {
    pub user_id: Uuid,
    pub episode_id: Uuid,
}

/// Debrid episodes Up Next is holding back for want of a download: each viewer's next
/// episode of a show they had caught up on, out or about to be, and not yet on disk. A show
/// leaves once Up Next would drop it anyway (14 days without a watch, unless it is a new
/// season) or was dismissed, and an episode once it has been out `wait_days` with nothing
/// usable.
/// One whose last download ahead is still standing is left to it: see [`retry_due`].
pub async fn due(pool: &PgPool, job_type: &str, wait_days: i32) -> Result<Vec<Due>> {
    let candidates = sqlx::query_as!(
        Due,
        r#"
        WITH last_watched AS (
            SELECT DISTINCT ON (ws.user_id, ts.show_id)
                ws.user_id,
                ts.show_id,
                ts.season_sort_order AS last_season_sort,
                te.episode_sort_order AS last_episode_sort,
                MAX(COALESCE(ws.watched_at, ws.updated_at))
                    OVER (PARTITION BY ws.user_id, ts.show_id) AS last_watched_at
            FROM tv_watch_states ws
            JOIN tv_episodes te ON te.id = ws.episode_id
            JOIN tv_seasons ts ON ts.id = te.season_id AND NOT ts.is_special
            WHERE ws.watched
            ORDER BY ws.user_id, ts.show_id, ts.season_sort_order DESC, te.episode_sort_order DESC
        ),
        next AS (
            SELECT DISTINCT ON (lw.user_id, lw.show_id)
                lw.user_id,
                lw.show_id,
                lw.last_watched_at,
                lw.last_season_sort < ts.season_sort_order AS new_season,
                te.id AS episode_id,
                te.air_date,
                te.want_status
            FROM last_watched lw
            JOIN tv_shows sh ON sh.id = lw.show_id AND sh.content_category = 'tv'
            JOIN tv_seasons ts ON ts.show_id = lw.show_id AND NOT ts.is_special
            JOIN tv_episodes te ON te.season_id = ts.id
            WHERE (ts.season_sort_order, te.episode_sort_order)
                  > (lw.last_season_sort, lw.last_episode_sort)
            ORDER BY lw.user_id, lw.show_id, ts.season_sort_order, te.episode_sort_order
        )
        SELECT DISTINCT ON (n.episode_id) n.user_id AS "user_id!", n.episode_id AS "episode_id!"
        FROM next n
        WHERE n.want_status = 'none'
          AND n.air_date <= CURRENT_DATE + $2::int
          AND n.air_date >= n.last_watched_at::date
          AND n.air_date >= CURRENT_DATE - $1::int
          AND (n.last_watched_at >= NOW() - INTERVAL '14 days' OR n.new_season)
          AND NOT EXISTS (
              SELECT 1 FROM up_next_dismissals d
              WHERE d.user_id = n.user_id AND d.kind = 'show' AND d.target = n.show_id::text
                AND d.dismissed_at >= n.last_watched_at
          )
          AND NOT EXISTS (
              SELECT 1 FROM tv_episode_file_episodes fe
              JOIN tv_episode_files f ON f.id = fe.episode_file_id
              WHERE fe.episode_id = n.episode_id AND f.available
          )
          AND NOT EXISTS (
              SELECT 1 FROM tv_watch_states ws
              WHERE ws.user_id = n.user_id AND ws.episode_id = n.episode_id
                AND ws.position_seconds > 0
          )
          AND NOT debrid_episode_on_disk(n.episode_id)
        ORDER BY n.episode_id, n.last_watched_at DESC
        "#,
        wait_days,
        EARLY_RELEASE_DAYS,
    )
    .fetch_all(pool)
    .await?;
    let ids: Vec<Uuid> = candidates.iter().map(|c| c.episode_id).collect();
    let ready = retry_due(pool, job_type, &ids).await?;
    Ok(candidates
        .into_iter()
        .filter(|c| ready.contains(&c.episode_id))
        .collect())
}

/// Which of these episodes have a whole debrid download on disk.
pub async fn on_disk(pool: &PgPool, episode_ids: &[Uuid]) -> Result<HashSet<Uuid>> {
    let found = sqlx::query_scalar!(
        r#"SELECT id AS "id!" FROM UNNEST($1::uuid[]) AS id WHERE debrid_episode_on_disk(id)"#,
        episode_ids,
    )
    .fetch_all(pool)
    .await?;
    Ok(found.into_iter().collect())
}

/// Which of these episodes may be queued again. A job still alive holds its episode for as
/// long as it heartbeats; one that downloaded holds it for the long interval; one that
/// found nothing or failed holds it for an interval set by how recently the episode aired.
pub async fn retry_due(
    pool: &PgPool,
    job_type: &str,
    episode_ids: &[Uuid],
) -> Result<HashSet<Uuid>> {
    let due = sqlx::query_scalar!(
        r#"
        SELECT e.id
        FROM tv_episodes e
        WHERE e.id = ANY($1)
          AND NOT EXISTS (
              SELECT 1 FROM job_status j
              WHERE j.job_type = $2 AND j.payload->>'episode_id' = e.id::text
                AND CASE
                    WHEN j.status IN ('pending', 'running', 'retrying')
                        THEN COALESCE(j.updated_at, j.created_at)
                             > NOW() - $5::float8 * INTERVAL '1 hour'
                    WHEN j.status = 'completed' AND j.result->>'skipped' IS NULL
                        THEN COALESCE(j.completed_at, j.created_at)
                             > NOW() - $5::float8 * INTERVAL '1 hour'
                    ELSE COALESCE(j.completed_at, j.created_at) > NOW()
                         - CASE WHEN e.air_date BETWEEN CURRENT_DATE - $3::int AND CURRENT_DATE
                                THEN $4::float8 ELSE $5::float8 END * INTERVAL '1 hour'
                END
          )
        "#,
        episode_ids,
        job_type,
        RECENT_AIR_DAYS,
        RECENT_RETRY_HOURS,
        RETRY_HOURS,
    )
    .fetch_all(pool)
    .await?;
    Ok(due.into_iter().collect())
}
