use sqlx::{PgConnection, PgPool};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use super::Result;

#[derive(Debug, Clone)]
pub struct ContinueRow {
    pub stremio_id: String,
    pub media_type: String,
    pub kind: String,
    pub tmdb_id: String,
    pub title: String,
    pub year: Option<i32>,
    pub description: Option<String>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub genres: Vec<String>,
    pub rating: Option<f64>,
    pub content_rating: Option<String>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub episode_title: Option<String>,
    pub air_date: Option<Date>,
    pub season_premiere: bool,
    pub position_ms: i64,
    pub duration_ms: i64,
    pub last_watched: OffsetDateTime,
}

pub async fn replace_for_account(
    pool: &PgPool,
    account_id: Uuid,
    rows: &[ContinueRow],
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query!(
        "DELETE FROM stremio_continue_items WHERE account_id = $1",
        account_id
    )
    .execute(&mut *tx)
    .await?;
    for row in rows {
        insert(&mut tx, account_id, row).await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn insert(conn: &mut PgConnection, account_id: Uuid, r: &ContinueRow) -> Result<()> {
    sqlx::query!(
        r#"
        INSERT INTO stremio_continue_items (
            account_id, stremio_id, media_type, kind, tmdb_id, title, year, description,
            poster_url, backdrop_url, genres, rating, content_rating, season, episode,
            episode_title, air_date, season_premiere, position_ms, duration_ms, last_watched
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18,
                  $19, $20, $21)
        "#,
        account_id,
        r.stremio_id,
        r.media_type,
        r.kind,
        r.tmdb_id,
        r.title,
        r.year,
        r.description,
        r.poster_url,
        r.backdrop_url,
        &r.genres,
        r.rating,
        r.content_rating,
        r.season,
        r.episode,
        r.episode_title,
        r.air_date,
        r.season_premiere,
        r.position_ms,
        r.duration_ms,
        r.last_watched,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// The account a profile reads: its own if it has one, else the instance-wide account.
///
/// Visibility follows `dashboard::watch_next_episodes`: TV drops out 14 days after it was last
/// watched unless the episode waiting is a season premiere that aired in the last 30 days, and a
/// caught-up show only comes back for an episode that aired (plus a day for release lag) after
/// the last watch. In-progress movies never expire.
///
/// A title already in the library is judged by the library's Kids rule (with its manual
/// override); anything else by the TMDB rating stored at sync time.
pub async fn list_for_user(
    pool: &PgPool,
    user_id: Uuid,
    kids_only: bool,
    exclude_movie_ids: &[Uuid],
    exclude_episode_ids: &[Uuid],
) -> Result<Vec<ContinueRow>> {
    Ok(sqlx::query_as!(
        ContinueRow,
        r#"
        WITH acct AS (
            SELECT id FROM stremio_accounts
            WHERE user_id = $1 OR user_id IS NULL
            ORDER BY user_id NULLS LAST
            LIMIT 1
        )
        SELECT i.stremio_id, i.media_type, i.kind, i.tmdb_id, i.title, i.year, i.description,
               i.poster_url, i.backdrop_url, i.genres, i.rating, i.content_rating, i.season,
               i.episode, i.episode_title, i.air_date, i.season_premiere, i.position_ms,
               i.duration_ms, i.last_watched
        FROM stremio_continue_items i
        JOIN acct ON acct.id = i.account_id
        LEFT JOIN LATERAL (
            SELECT m.id, m.kids_override, m.content_rating FROM movies m
            WHERE i.media_type = 'movie' AND m.identifiers->>'tmdb_id' = i.tmdb_id
            LIMIT 1
        ) lm ON TRUE
        LEFT JOIN LATERAL (
            SELECT s.id, s.kids_override, s.content_rating FROM tv_shows s
            WHERE i.media_type = 'tv' AND s.identifiers->>'tmdb_id' = i.tmdb_id
            LIMIT 1
        ) ls ON TRUE
        WHERE (
            i.media_type = 'movie'
            OR (
                (i.kind <> 'new_episode'
                 OR (i.air_date > i.last_watched::date
                     AND i.air_date::timestamp + INTERVAL '1 day' <= NOW()))
                AND (i.last_watched >= NOW() - INTERVAL '14 days'
                     OR (i.season_premiere AND i.air_date >= CURRENT_DATE - 30))
            )
        )
          AND ($2 = FALSE OR CASE
                WHEN lm.id IS NOT NULL THEN kids_allowed(lm.kids_override, lm.content_rating)
                WHEN ls.id IS NOT NULL THEN kids_allowed(ls.kids_override, ls.content_rating)
                ELSE kids_allowed(NULL, i.content_rating)
              END)
          AND (lm.id IS NULL OR NOT lm.id = ANY($3))
          AND (ls.id IS NULL OR NOT EXISTS (
                SELECT 1 FROM tv_episodes e
                JOIN tv_seasons se ON se.id = e.season_id
                WHERE e.id = ANY($4) AND se.show_id = ls.id
              ))
        ORDER BY i.last_watched DESC
        LIMIT 20
        "#,
        user_id,
        kids_only,
        exclude_movie_ids,
        exclude_episode_ids,
    )
    .fetch_all(pool)
    .await?)
}
