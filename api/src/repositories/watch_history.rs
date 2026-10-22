//! A user's watch history, one row per title, from every source the For You seeds read:
//! library watch state (written here, or imported from Trakt or Stremio) and the
//! Stremio account's own history.

use serde::Deserialize;
use sqlx::{Executor, PgPool, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

use super::Result;

pub const SOURCES: &[&str] = &["local", "trakt", "stremio"];

#[derive(Debug, Deserialize)]
pub struct HistoryRow {
    pub media_type: String,
    pub tmdb_id: Option<String>,
    pub library_id: Option<Uuid>,
    pub title: String,
    pub year: Option<i32>,
    pub images: Option<serde_json::Value>,
    pub poster_url: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_watched_at: Option<OffsetDateTime>,
    pub sources: Vec<String>,
    pub excluded: bool,
}

pub async fn list(
    pool: &PgPool,
    user_id: Uuid,
    search: Option<&str>,
    source: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<(Vec<HistoryRow>, i64)> {
    let row = sqlx::query!(
        r#"
        WITH acct AS (
            SELECT id FROM stremio_accounts
            WHERE user_id = $1 OR user_id IS NULL
            ORDER BY user_id NULLS LAST
            LIMIT 1
        ),
        started AS (
            SELECT 'movie' AS media_type, NULLIF(m.identifiers->>'tmdb_id', '') AS tmdb_id,
                   m.id AS library_id, m.title, m.year, m.images, NULL::text AS poster_url,
                   ws.source,
                   CASE WHEN ws.source = 'local' THEN COALESCE(ws.watched_at, ws.updated_at)
                        ELSE ws.watched_at END AS at
            FROM movie_watch_states ws
            JOIN movies m ON m.id = ws.movie_id
            WHERE ws.user_id = $1 AND (ws.watched OR ws.position_seconds > 0)
            UNION ALL
            SELECT 'tv', NULLIF(s.identifiers->>'tmdb_id', ''), s.id, s.title, s.year, s.images,
                   NULL::text, ws.source,
                   CASE WHEN ws.source = 'local' THEN COALESCE(ws.watched_at, ws.updated_at)
                        ELSE ws.watched_at END
            FROM tv_watch_states ws
            JOIN tv_episodes e ON e.id = ws.episode_id
            JOIN tv_seasons se ON se.id = e.season_id
            JOIN tv_shows s ON s.id = se.show_id
            WHERE ws.user_id = $1 AND (ws.watched OR ws.position_seconds > 0)
            UNION ALL
            SELECT h.media_type, NULLIF(h.tmdb_id, ''), NULL::uuid, COALESCE(h.title, h.name),
                   h.year, NULL::jsonb, h.poster_url, 'stremio', h.last_watched
            FROM stremio_history_items h
            JOIN acct ON acct.id = h.account_id
        ),
        titles AS (
            SELECT media_type,
                   MAX(tmdb_id) AS tmdb_id,
                   (array_agg(library_id ORDER BY library_id IS NULL, at DESC NULLS LAST))[1]
                       AS library_id,
                   (array_agg(title ORDER BY library_id IS NULL, at DESC NULLS LAST))[1] AS title,
                   (array_agg(year ORDER BY library_id IS NULL, at DESC NULLS LAST))[1] AS year,
                   (array_agg(images ORDER BY library_id IS NULL, at DESC NULLS LAST))[1] AS images,
                   (array_agg(poster_url) FILTER (WHERE poster_url IS NOT NULL))[1] AS poster_url,
                   MAX(at) AS last_watched_at,
                   array_agg(DISTINCT source ORDER BY source) AS sources
            FROM started
            GROUP BY media_type, COALESCE(tmdb_id, 'library:' || library_id::text)
        ),
        filtered AS (
            SELECT t.*,
                   EXISTS (
                       SELECT 1 FROM watch_history_exclusions x
                       WHERE x.user_id = $1 AND x.media_type = t.media_type
                         AND x.tmdb_id = t.tmdb_id
                   ) AS excluded
            FROM titles t
            WHERE ($2::text IS NULL OR t.title ILIKE '%' || $2 || '%')
              AND ($3::text IS NULL OR $3 = ANY(t.sources))
        )
        SELECT
            (SELECT COUNT(*) FROM filtered) AS "total!",
            COALESCE(
                (SELECT jsonb_agg(to_jsonb(p) ORDER BY p.last_watched_at DESC NULLS LAST,
                                                       p.title, p.media_type)
                 FROM (
                     SELECT * FROM filtered
                     ORDER BY last_watched_at DESC NULLS LAST, title, media_type
                     LIMIT $4 OFFSET $5
                 ) p),
                '[]'::jsonb
            ) AS "items!"
        "#,
        user_id,
        search,
        source,
        limit,
        offset,
    )
    .fetch_one(pool)
    .await?;
    let items: Vec<HistoryRow> =
        serde_json::from_value(row.items).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
    Ok((items, row.total))
}

pub async fn set_excluded<'e, E>(
    exec: E,
    user_id: Uuid,
    media_types: &[String],
    tmdb_ids: &[String],
    excluded: bool,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    if excluded {
        sqlx::query!(
            r#"
            INSERT INTO watch_history_exclusions (user_id, media_type, tmdb_id)
            SELECT $1, k.media_type, k.tmdb_id
            FROM UNNEST($2::text[], $3::text[]) AS k(media_type, tmdb_id)
            ON CONFLICT DO NOTHING
            "#,
            user_id,
            media_types,
            tmdb_ids,
        )
        .execute(exec)
        .await?;
    } else {
        sqlx::query!(
            r#"
            DELETE FROM watch_history_exclusions x
            USING UNNEST($2::text[], $3::text[]) AS k(media_type, tmdb_id)
            WHERE x.user_id = $1 AND x.media_type = k.media_type AND x.tmdb_id = k.tmdb_id
            "#,
            user_id,
            media_types,
            tmdb_ids,
        )
        .execute(exec)
        .await?;
    }
    Ok(())
}

pub async fn show_ids_for_tmdb<'e, E>(exec: E, tmdb_ids: &[String]) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_scalar!(
        "SELECT id FROM tv_shows WHERE identifiers->>'tmdb_id' = ANY($1)",
        tmdb_ids,
    )
    .fetch_all(exec)
    .await?)
}

/// The given titles that the user's Stremio history holds, as (media_type, tmdb_id).
pub async fn stremio_titles<'e, E>(
    exec: E,
    user_id: Uuid,
    media_types: &[String],
    tmdb_ids: &[String],
) -> Result<Vec<(String, String)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        WITH acct AS (
            SELECT id FROM stremio_accounts
            WHERE user_id = $1 OR user_id IS NULL
            ORDER BY user_id NULLS LAST
            LIMIT 1
        )
        SELECT DISTINCT h.media_type, h.tmdb_id AS "tmdb_id!"
        FROM stremio_history_items h
        JOIN acct ON acct.id = h.account_id
        JOIN UNNEST($2::text[], $3::text[]) AS k(media_type, tmdb_id)
          ON k.media_type = h.media_type AND k.tmdb_id = h.tmdb_id
        "#,
        user_id,
        media_types,
        tmdb_ids,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.media_type, r.tmdb_id))
        .collect())
}
