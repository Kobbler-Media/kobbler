//! Dashboard / "What to Watch" queries.

use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

pub struct InProgressBookRow {
    pub book_id: Uuid,
    pub title: String,
    pub images: serde_json::Value,
    pub authors: Option<serde_json::Value>,
    pub progress_percent: Option<f64>,
    pub updated_at: OffsetDateTime,
    pub has_audiobook: bool,
    pub duration_seconds: Option<i32>,
}

pub struct BookRow {
    pub id: Uuid,
    pub title: String,
    pub images: serde_json::Value,
    pub authors: Option<serde_json::Value>,
    pub added_at: OffsetDateTime,
}

pub struct MovieRow {
    pub id: Uuid,
    pub title: String,
    pub year: Option<i32>,
    pub images: serde_json::Value,
    pub added_at: OffsetDateTime,
}

pub struct EpisodeRow {
    pub show_id: Uuid,
    pub show_title: String,
    pub show_images: serde_json::Value,
    pub season_identifier: String,
    pub episode_id: Uuid,
    pub episode_identifier: String,
    pub episode_title: Option<String>,
    pub added_at: OffsetDateTime,
}

pub struct InProgressMovieRow {
    pub movie_id: Uuid,
    pub title: String,
    pub year: Option<i32>,
    pub images: serde_json::Value,
    pub position_seconds: i32,
    pub duration_seconds: Option<f64>,
    pub updated_at: OffsetDateTime,
}

pub struct WatchNextEpisodeRow {
    pub show_id: Uuid,
    pub show_title: String,
    pub show_images: serde_json::Value,
    pub content_category: String,
    pub episode_id: Uuid,
    pub season_identifier: String,
    pub episode_identifier: String,
    pub episode_title: Option<String>,
    pub position_seconds: Option<i32>,
    pub duration_seconds: Option<f64>,
    pub updated_at: OffsetDateTime,
}

pub struct WatchlistShowRow {
    pub show_id: Uuid,
    pub show_title: String,
    pub show_images: serde_json::Value,
    pub content_category: String,
    pub episode_id: Uuid,
    pub season_identifier: String,
    pub episode_identifier: String,
    pub episode_title: Option<String>,
    pub added_at: OffsetDateTime,
}

pub struct WatchlistMovieRow {
    pub movie_id: Uuid,
    pub title: String,
    pub year: Option<i32>,
    pub images: serde_json::Value,
    pub added_at: OffsetDateTime,
}

pub async fn in_progress_movies<'e, E>(
    exec: E,
    user_id: Uuid,
    kids_only: bool,
) -> Result<Vec<InProgressMovieRow>, sqlx::Error>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query_as!(
        InProgressMovieRow,
        r#"
        SELECT
            m.id AS movie_id,
            m.title,
            m.year,
            m.images AS "images: serde_json::Value",
            ws.position_seconds AS "position_seconds!",
            COALESCE(
                (SELECT MAX(mf.duration_seconds) FROM movie_files mf
                 WHERE mf.movie_id = m.id AND mf.available = true),
                (SELECT (ds.probe_data->>'duration')::float8 FROM debrid_stream_sessions ds
                 WHERE ds.media_type = 'movie' AND ds.media_id = m.id AND ds.probe_data IS NOT NULL
                 ORDER BY ds.updated_at DESC LIMIT 1)
            ) AS "duration_seconds?: f64",
            ws.updated_at AS "updated_at!"
        FROM movie_watch_states ws
        JOIN movies m ON m.id = ws.movie_id
        WHERE ws.user_id = $1
          AND ws.position_seconds IS NOT NULL
          AND ws.position_seconds >= 60
          AND ws.watched = FALSE
          AND ($2 = FALSE OR kids_allowed(m.kids_override, m.content_rating))
        ORDER BY ws.updated_at DESC
        LIMIT 20
        "#,
        user_id,
        kids_only,
    )
    .fetch_all(exec)
    .await
}

/// Up-next TV episodes: one entry per show, deduped, with 14-day expiry and
/// new-season exception.
///
/// Availability rules for the `up_next` CTE branch:
///   - Wanted episodes require a real file (kobload will set one once grabbed).
///   - Debrid-only episodes (`want_status = 'none'`) appear 24h after their
///     stored DATE to absorb release/indexing lag. We don't have per-show
///     `airs_time` metadata yet, so the fuzz is a flat 1 day.
///
/// A sport league is followed live, so `sport_next` replaces `up_next` for it: the
/// newest available event, and only while every watched event is older.
pub async fn watch_next_episodes<'e, E>(
    exec: E,
    user_id: Uuid,
    kids_only: bool,
) -> Result<Vec<WatchNextEpisodeRow>, sqlx::Error>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query_as!(
        WatchNextEpisodeRow,
        r#"
        WITH
        last_watched AS (
            SELECT DISTINCT ON (ts.show_id)
                ts.show_id,
                ts.season_sort_order AS last_season_sort,
                te.episode_sort_order AS last_episode_sort,
                ts.id AS last_season_id
            FROM tv_watch_states ws
            JOIN tv_episodes te ON te.id = ws.episode_id
            JOIN tv_seasons ts ON ts.id = te.season_id AND NOT ts.is_special
            WHERE ws.user_id = $1 AND ws.watched = TRUE
            ORDER BY ts.show_id, ts.season_sort_order DESC, te.episode_sort_order DESC
        ),
        in_progress AS (
            SELECT DISTINCT ON (ts.show_id)
                ts.show_id,
                te.id AS episode_id,
                ts.season_sort_order,
                ts.season_identifier,
                te.episode_identifier,
                te.title AS episode_title,
                ws.position_seconds,
                ws.updated_at
            FROM tv_watch_states ws
            JOIN tv_episodes te ON te.id = ws.episode_id
            JOIN tv_seasons ts ON ts.id = te.season_id AND NOT ts.is_special
            WHERE ws.user_id = $1
              AND ws.position_seconds >= 60
              AND ws.watched = FALSE
            ORDER BY ts.show_id, ws.updated_at DESC
        ),
        sport_events AS (
            SELECT
                ts.show_id,
                te.id AS episode_id,
                ts.season_sort_order,
                ts.season_identifier,
                te.episode_identifier,
                te.title AS episode_title,
                ROW_NUMBER() OVER (
                    PARTITION BY ts.show_id
                    ORDER BY te.air_date ASC NULLS FIRST, te.air_time ASC NULLS FIRST,
                             ts.season_sort_order, te.episode_sort_order
                ) AS recency,
                (
                    EXISTS (
                        SELECT 1 FROM tv_episode_file_episodes efe
                        JOIN tv_episode_files ef ON ef.id = efe.episode_file_id
                        WHERE efe.episode_id = te.id
                    )
                    OR (te.want_status = 'none'
                        AND te.air_date IS NOT NULL
                        AND (te.air_date::timestamp + INTERVAL '1 day') <= NOW())
                ) AS available,
                EXISTS (
                    SELECT 1 FROM tv_watch_states ws
                    WHERE ws.episode_id = te.id AND ws.user_id = $1 AND ws.watched = TRUE
                ) AS watched
            FROM tv_shows sh
            JOIN tv_seasons ts ON ts.show_id = sh.id AND NOT ts.is_special
            JOIN tv_episodes te ON te.season_id = ts.id
            WHERE sh.content_category = 'sport'
              AND sh.id IN (SELECT show_id FROM last_watched)
        ),
        sport_next AS (
            SELECT DISTINCT ON (se.show_id)
                se.show_id, se.episode_id, se.season_sort_order, se.season_identifier,
                se.episode_identifier, se.episode_title
            FROM sport_events se
            WHERE se.available
              AND NOT se.watched
              AND NOT EXISTS (SELECT 1 FROM in_progress ip WHERE ip.show_id = se.show_id)
              AND se.recency > (
                  SELECT MAX(w.recency) FROM sport_events w
                  WHERE w.show_id = se.show_id AND w.watched
              )
            ORDER BY se.show_id, se.recency DESC
        ),
        up_next AS (
            SELECT DISTINCT ON (lw.show_id)
                lw.show_id,
                te.id AS episode_id,
                ts.season_sort_order,
                ts.season_identifier,
                te.episode_identifier,
                te.title AS episode_title
            FROM last_watched lw
            JOIN tv_seasons ts ON ts.show_id = lw.show_id AND NOT ts.is_special
            JOIN tv_episodes te ON te.season_id = ts.id
            WHERE NOT EXISTS (SELECT 1 FROM in_progress ip WHERE ip.show_id = lw.show_id)
              AND NOT EXISTS (
                  SELECT 1 FROM tv_shows sh
                  WHERE sh.id = lw.show_id AND sh.content_category = 'sport'
              )
              AND (ts.season_sort_order, te.episode_sort_order) >
                  (lw.last_season_sort, lw.last_episode_sort)
              AND (
                  EXISTS (
                      SELECT 1 FROM tv_episode_file_episodes efe
                      JOIN tv_episode_files ef ON ef.id = efe.episode_file_id
                      WHERE efe.episode_id = te.id
                  )
                  OR (te.want_status = 'none'
                      AND te.air_date IS NOT NULL
                      AND (te.air_date::timestamp + INTERVAL '1 day') <= NOW())
              )
              AND NOT EXISTS (
                  SELECT 1 FROM tv_watch_states ws2
                  WHERE ws2.episode_id = te.id AND ws2.user_id = $1 AND ws2.watched = TRUE
              )
            ORDER BY lw.show_id, ts.season_sort_order, te.episode_sort_order
        ),
        combined AS (
            SELECT show_id, episode_id, season_sort_order, season_identifier,
                   episode_identifier, episode_title, position_seconds
            FROM in_progress
            UNION ALL
            SELECT show_id, episode_id, season_sort_order, season_identifier,
                   episode_identifier, episode_title, NULL::int4
            FROM up_next
            UNION ALL
            SELECT show_id, episode_id, season_sort_order, season_identifier,
                   episode_identifier, episode_title, NULL::int4
            FROM sport_next
        ),
        show_activity AS (
            SELECT ts.show_id, MAX(COALESCE(ws.watched_at, ws.updated_at)) AS last_activity
            FROM tv_watch_states ws
            JOIN tv_episodes te ON te.id = ws.episode_id
            JOIN tv_seasons ts ON ts.id = te.season_id
            WHERE ws.user_id = $1
            GROUP BY ts.show_id
        )
        SELECT
            c.show_id AS "show_id!",
            tshow.title AS "show_title!",
            tshow.images AS "show_images!: serde_json::Value",
            tshow.content_category AS "content_category!",
            c.episode_id AS "episode_id!",
            c.season_identifier AS "season_identifier!",
            c.episode_identifier AS "episode_identifier!",
            c.episode_title,
            c.position_seconds,
            COALESCE(
                (SELECT MAX(tef.duration_seconds) FROM tv_episode_files tef
                 JOIN tv_episode_file_episodes tefe ON tefe.episode_file_id = tef.id
                 WHERE tefe.episode_id = c.episode_id AND tef.available = true),
                (SELECT (ds.probe_data->>'duration')::float8 FROM debrid_stream_sessions ds
                 WHERE ds.media_type = 'episode' AND ds.media_id = c.episode_id
                   AND ds.probe_data IS NOT NULL
                 ORDER BY ds.updated_at DESC LIMIT 1)
            ) AS "duration_seconds?: f64",
            sa.last_activity AS "updated_at!"
        FROM combined c
        JOIN tv_shows tshow ON tshow.id = c.show_id
            AND tshow.content_category IN ('tv', 'sport')
        JOIN show_activity sa ON sa.show_id = c.show_id
        WHERE ($2 = FALSE OR kids_allowed(tshow.kids_override, tshow.content_rating))
          AND (
            sa.last_activity >= NOW() - INTERVAL '14 days'
            OR (tshow.content_category = 'sport' AND EXISTS (
                SELECT 1 FROM tv_episodes te5
                WHERE te5.id = c.episode_id AND te5.air_date >= CURRENT_DATE - 14
            ))
            OR EXISTS (
                SELECT 1 FROM last_watched lw
                WHERE lw.show_id = c.show_id
                  AND c.season_sort_order > lw.last_season_sort
                  AND lw.last_episode_sort = (
                      SELECT MAX(te2.episode_sort_order)
                      FROM tv_episodes te2
                      WHERE te2.season_id = lw.last_season_id
                  )
                  AND (
                      EXISTS (
                          SELECT 1 FROM tv_episodes te3
                          WHERE te3.id = c.episode_id
                            AND te3.air_date >= CURRENT_DATE - 30
                      )
                      OR EXISTS (
                          SELECT 1 FROM tv_episode_file_episodes efe3
                          JOIN tv_episode_files ef3 ON ef3.id = efe3.episode_file_id
                          WHERE efe3.episode_id = c.episode_id
                            AND ef3.created_at >= NOW() - INTERVAL '30 days'
                      )
                      OR EXISTS (
                          SELECT 1 FROM debrid_stream_sessions ds
                          JOIN tv_episodes te4 ON te4.id = ds.media_id
                          JOIN tv_seasons ts4 ON ts4.id = te4.season_id
                          WHERE ds.media_type = 'episode'
                            AND ts4.show_id = lw.show_id
                            AND ds.created_at >= NOW() - INTERVAL '30 days'
                      )
                  )
            )
          )
        ORDER BY sa.last_activity DESC
        LIMIT 20
        "#,
        user_id,
        kids_only,
    )
    .fetch_all(exec)
    .await
}

pub async fn in_progress_books<'e, E>(
    exec: E,
    user_id: Uuid,
) -> Result<Vec<InProgressBookRow>, sqlx::Error>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query_as!(
        InProgressBookRow,
        r#"
        SELECT
            b.id AS book_id,
            b.title,
            b.images AS "images: serde_json::Value",
            COALESCE(
                (SELECT jsonb_agg(DISTINCT jsonb_build_object('name', a2.name))
                 FROM book_authors ba2
                 JOIN authors a2 ON a2.id = ba2.author_id
                 WHERE ba2.book_id = b.id),
                '[]'::jsonb
            ) AS "authors: serde_json::Value",
            (SELECT bm.progress_percent
             FROM reading_states rs
             JOIN reading_bookmark bm ON bm.reading_state_id = rs.id
             WHERE rs.user_id = $1 AND rs.book_id = b.id
             ORDER BY rs.updated_at DESC
             LIMIT 1) AS "progress_percent?",
            ubd.reading_status_updated_at AS "updated_at!",
            EXISTS (
                SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id
            ) AS "has_audiobook!",
            b.duration_seconds
        FROM user_book_data ubd
        JOIN books b ON b.id = ubd.book_id
        WHERE ubd.user_id = $1
          AND ubd.reading_status = 'reading'
        ORDER BY ubd.reading_status_updated_at DESC
        LIMIT 20
        "#,
        user_id,
    )
    .fetch_all(exec)
    .await
}

pub async fn recent_books<'e, E>(
    exec: E,
    audiobooks_only: bool,
) -> Result<Vec<BookRow>, sqlx::Error>
where
    E: Executor<'e, Database = Postgres>,
{
    // An audiobook bought years after the ebook is new to the audiobook shelf but not
    // to the library, so the audio rail dates from the file, not from books.created_at.
    sqlx::query_as!(
        BookRow,
        r#"
        SELECT
            b.id,
            b.title,
            b.images as "images: serde_json::Value",
            COALESCE(
                (SELECT jsonb_agg(DISTINCT jsonb_build_object('name', a2.name))
                 FROM book_authors ba2
                 JOIN authors a2 ON a2.id = ba2.author_id
                 WHERE ba2.book_id = b.id),
                '[]'::jsonb
            ) as "authors: serde_json::Value",
            (CASE WHEN $1 THEN audio.added_at ELSE b.created_at END) as "added_at!"
        FROM books b
        LEFT JOIN LATERAL (
            SELECT MAX(af.added_at) AS added_at FROM audiobook_files af WHERE af.book_id = b.id
        ) audio ON TRUE
        WHERE audio.added_at IS NOT NULL
           OR (NOT $1 AND EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id))
        ORDER BY (CASE WHEN $1 THEN audio.added_at ELSE b.created_at END) DESC
        LIMIT 12
        "#,
        audiobooks_only,
    )
    .fetch_all(exec)
    .await
}

pub async fn recent_movies<'e, E>(exec: E, kids_only: bool) -> Result<Vec<MovieRow>, sqlx::Error>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query_as!(
        MovieRow,
        r#"
        SELECT
            m.id,
            m.title,
            m.year,
            m.images as "images: serde_json::Value",
            MAX(mf.created_at) as "added_at!: OffsetDateTime"
        FROM movies m
        JOIN movie_files mf ON mf.movie_id = m.id AND mf.available = true
        WHERE $1 = FALSE OR kids_allowed(m.kids_override, m.content_rating)
        GROUP BY m.id, m.title, m.year, m.images
        ORDER BY MAX(mf.created_at) DESC
        LIMIT 12
        "#,
        kids_only,
    )
    .fetch_all(exec)
    .await
}

pub async fn recent_episodes<'e, E>(
    exec: E,
    content_category: &str,
    kids_only: bool,
) -> Result<Vec<EpisodeRow>, sqlx::Error>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query_as!(
        EpisodeRow,
        r#"
        SELECT
            sub.show_id as "show_id!",
            sub.show_title as "show_title!",
            sub.show_images as "show_images!: serde_json::Value",
            sub.season_identifier as "season_identifier!",
            sub.episode_id as "episode_id!",
            sub.episode_identifier as "episode_identifier!",
            sub.episode_title,
            sub.added_at as "added_at!"
        FROM (
            SELECT DISTINCT ON (tshow.id)
                tshow.id as show_id,
                tshow.title as show_title,
                tshow.images as show_images,
                ts.season_identifier,
                te.id as episode_id,
                te.episode_identifier,
                te.title as episode_title,
                tef.created_at as added_at
            FROM tv_episode_files tef
            JOIN tv_episode_file_episodes tefe ON tefe.episode_file_id = tef.id
            JOIN tv_episodes te ON te.id = tefe.episode_id
            JOIN tv_seasons ts ON te.season_id = ts.id
            JOIN tv_shows tshow ON ts.show_id = tshow.id
            WHERE tef.available = true AND tshow.content_category = $1
              AND ($2 = FALSE OR kids_allowed(tshow.kids_override, tshow.content_rating))
            ORDER BY tshow.id, tef.created_at DESC
        ) sub
        ORDER BY sub.added_at DESC
        LIMIT 12
        "#,
        content_category,
        kids_only,
    )
    .fetch_all(exec)
    .await
}

/// Watchlisted shows the user has not started, resolved to the episode they
/// would start on: the first non-special episode with no watched state, or for a sport
/// league its newest available event.
///
/// Shows with any watch history are excluded — `dashboard::watch_next_episodes`
/// already covers those through its `last_watched` branch, and surfacing both
/// would double up the card.
pub async fn watchlist_shows<'e, E>(
    exec: E,
    user_id: Uuid,
    kids_only: bool,
) -> Result<Vec<WatchlistShowRow>, sqlx::Error>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        WatchlistShowRow,
        r#"
        SELECT DISTINCT ON (w.media_id)
            tshow.id AS "show_id!",
            tshow.title AS "show_title!",
            tshow.images AS "show_images!: serde_json::Value",
            tshow.content_category AS "content_category!",
            te.id AS "episode_id!",
            ts.season_identifier AS "season_identifier!",
            te.episode_identifier AS "episode_identifier!",
            te.title AS episode_title,
            w.added_at AS "added_at!"
        FROM watchlist_items w
        JOIN tv_shows tshow ON tshow.id = w.media_id
        JOIN tv_seasons ts ON ts.show_id = tshow.id AND NOT ts.is_special
        JOIN tv_episodes te ON te.season_id = ts.id
        WHERE w.user_id = $1
          AND w.media_type = 'show'
          AND (
              tshow.content_category = 'tv'
              OR (tshow.content_category = 'sport' AND (
                  EXISTS (
                      SELECT 1 FROM tv_episode_file_episodes efe
                      JOIN tv_episode_files ef ON ef.id = efe.episode_file_id
                      WHERE efe.episode_id = te.id
                  )
                  OR (te.want_status = 'none'
                      AND te.air_date IS NOT NULL
                      AND (te.air_date::timestamp + INTERVAL '1 day') <= NOW())
              ))
          )
          AND ($2 = FALSE OR kids_allowed(tshow.kids_override, tshow.content_rating))
          AND NOT EXISTS (
              SELECT 1
              FROM tv_watch_states ws
              JOIN tv_episodes te2 ON te2.id = ws.episode_id
              JOIN tv_seasons ts2 ON ts2.id = te2.season_id
              WHERE ws.user_id = $1 AND ts2.show_id = tshow.id
          )
        ORDER BY w.media_id,
                 (CASE WHEN tshow.content_category = 'sport' THEN te.air_date END) DESC NULLS LAST,
                 (CASE WHEN tshow.content_category = 'sport' THEN te.air_time END) DESC NULLS LAST,
                 (CASE WHEN tshow.content_category = 'sport' THEN ts.season_sort_order END) DESC,
                 (CASE WHEN tshow.content_category = 'sport' THEN te.episode_sort_order END) DESC,
                 ts.season_sort_order, te.episode_sort_order
        "#,
        user_id,
        kids_only,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// Watchlisted movies the user has neither watched nor started. In-progress
/// ones are already returned by `dashboard::in_progress_movies`.
pub async fn watchlist_movies<'e, E>(
    exec: E,
    user_id: Uuid,
    kids_only: bool,
) -> Result<Vec<WatchlistMovieRow>, sqlx::Error>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        WatchlistMovieRow,
        r#"
        SELECT
            m.id AS "movie_id!",
            m.title AS "title!",
            m.year,
            m.images AS "images!: serde_json::Value",
            w.added_at AS "added_at!"
        FROM watchlist_items w
        JOIN movies m ON m.id = w.media_id
        WHERE w.user_id = $1
          AND w.media_type = 'movie'
          AND ($2 = FALSE OR kids_allowed(m.kids_override, m.content_rating))
          AND NOT EXISTS (
              SELECT 1 FROM movie_watch_states ws
              WHERE ws.movie_id = m.id
                AND ws.user_id = $1
                AND (ws.watched = TRUE OR COALESCE(ws.position_seconds, 0) >= 60)
          )
        ORDER BY w.added_at DESC
        "#,
        user_id,
        kids_only,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}
