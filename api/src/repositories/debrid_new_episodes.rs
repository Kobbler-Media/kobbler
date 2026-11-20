use sqlx::PgPool;
use uuid::Uuid;

use super::Result;

pub struct Due {
    pub user_id: Uuid,
    pub episode_id: Uuid,
}

/// Debrid episodes Up Next is holding back for want of a download: each viewer's next
/// episode of a show they had caught up on, out now and not yet on disk. A show leaves once
/// Up Next would drop it anyway (14 days without a watch, unless it is a new season) or
/// was dismissed, and an episode once it has been out `wait_days` with nothing usable.
/// An episode queued within `retry_hours` is left to that attempt.
pub async fn due(
    pool: &PgPool,
    job_type: &str,
    wait_days: i32,
    retry_hours: f64,
) -> Result<Vec<Due>> {
    Ok(sqlx::query_as!(
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
          AND n.air_date <= CURRENT_DATE
          AND n.air_date >= n.last_watched_at::date
          AND n.air_date >= CURRENT_DATE - $2::int
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
          AND NOT EXISTS (
              SELECT 1 FROM job_status
              WHERE job_type = $1 AND payload->>'episode_id' = n.episode_id::text
                AND created_at > NOW() - $3::float8 * INTERVAL '1 hour'
          )
        ORDER BY n.episode_id, n.last_watched_at DESC
        "#,
        job_type,
        wait_days,
        retry_hours,
    )
    .fetch_all(pool)
    .await?)
}
