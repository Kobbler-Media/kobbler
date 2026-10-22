use serde::Serialize;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use super::Result;
use crate::stremio_library::history::HistoryEntry;
use crate::stremio_library::resolve::Identity;

#[derive(Debug, Serialize)]
pub struct HistoryRow {
    pub stremio_id: String,
    pub media_type: String,
    pub tmdb_id: String,
    pub title: String,
    pub year: Option<i32>,
    pub poster_url: Option<String>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub marker_watched: bool,
    pub watched: bool,
    pub position_ms: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub last_watched: OffsetDateTime,
    pub imported: bool,
    pub import_error: Option<String>,
}

pub struct Unresolved {
    pub stremio_id: String,
    pub media_type: String,
}

pub struct PendingImport {
    pub user_id: Uuid,
    pub stremio_id: String,
    pub media_type: String,
    pub tmdb_id: String,
    pub title: String,
    pub year: Option<i32>,
    pub poster_url: Option<String>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub marker_watched: bool,
    pub watched: bool,
    pub position_ms: i64,
    pub last_watched: OffsetDateTime,
}

impl PendingImport {
    fn position_seconds(&self) -> Option<i32> {
        (self.position_ms > 0).then_some((self.position_ms / 1000) as i32)
    }
}

/// Upserts rather than replaces: a delete would cascade into the profiles' import selections.
pub async fn store_entries(
    pool: &PgPool,
    account_id: Uuid,
    entries: &[HistoryEntry],
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let ids: Vec<String> = entries.iter().map(|e| e.stremio_id.clone()).collect();
    sqlx::query!(
        "DELETE FROM stremio_history_items WHERE account_id = $1 AND NOT stremio_id = ANY($2)",
        account_id,
        &ids,
    )
    .execute(&mut *tx)
    .await?;
    for e in entries {
        sqlx::query!(
            r#"
            INSERT INTO stremio_history_items (account_id, stremio_id, media_type, name, season,
                                               episode, marker_watched, watched, position_ms,
                                               last_watched)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (account_id, stremio_id) DO UPDATE SET
                media_type = EXCLUDED.media_type,
                name = EXCLUDED.name,
                season = EXCLUDED.season,
                episode = EXCLUDED.episode,
                marker_watched = EXCLUDED.marker_watched,
                watched = EXCLUDED.watched,
                position_ms = EXCLUDED.position_ms,
                last_watched = EXCLUDED.last_watched
            "#,
            account_id,
            e.stremio_id,
            e.media_type,
            e.name,
            e.season,
            e.episode,
            e.marker_watched,
            e.watched,
            e.position_ms,
            e.last_watched,
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Titles TMDB had no match for are retried weekly, in case TMDB has since added them.
pub async fn list_unresolved(pool: &PgPool, account_id: Uuid) -> Result<Vec<Unresolved>> {
    Ok(sqlx::query_as!(
        Unresolved,
        r#"
        SELECT stremio_id, media_type FROM stremio_history_items
        WHERE account_id = $1
          AND (resolved_at IS NULL
               OR (tmdb_id IS NULL AND resolved_at < NOW() - INTERVAL '7 days'))
        "#,
        account_id,
    )
    .fetch_all(pool)
    .await?)
}

pub async fn set_identity(
    pool: &PgPool,
    account_id: Uuid,
    stremio_id: &str,
    identity: Option<&Identity>,
) -> Result<()> {
    sqlx::query!(
        r#"
        UPDATE stremio_history_items
        SET tmdb_id = $3, title = $4, year = $5, poster_url = $6, content_rating = $7,
            resolved_at = NOW()
        WHERE account_id = $1 AND stremio_id = $2
        "#,
        account_id,
        stremio_id,
        identity.map(|i| i.tmdb_id.as_str()),
        identity.map(|i| i.title.as_str()),
        identity.and_then(|i| i.year),
        identity.and_then(|i| i.poster_url.as_deref()),
        identity.and_then(|i| i.content_rating.as_deref()),
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// `None` when no account applies to the profile.
pub async fn list_for_user(
    pool: &PgPool,
    user_id: Uuid,
    kids_only: bool,
) -> Result<Option<Vec<HistoryRow>>> {
    let Some(account_id) = account_for(pool, user_id).await? else {
        return Ok(None);
    };
    Ok(Some(
        sqlx::query_as!(
            HistoryRow,
            r#"
            SELECT h.stremio_id, h.media_type, h.tmdb_id AS "tmdb_id!", h.title AS "title!",
                   h.year, h.poster_url, h.season, h.episode, h.marker_watched, h.watched,
                   h.position_ms, h.last_watched,
                   (i.user_id IS NOT NULL) AS "imported!", i.last_error AS import_error
            FROM stremio_history_items h
            LEFT JOIN stremio_history_imports i
              ON i.account_id = h.account_id AND i.stremio_id = h.stremio_id AND i.user_id = $2
            WHERE h.account_id = $1
              AND h.tmdb_id IS NOT NULL AND h.title IS NOT NULL
              AND ($3 = FALSE
                   OR stremio_title_kids_allowed(h.media_type, h.tmdb_id, h.content_rating))
            ORDER BY h.last_watched DESC
            "#,
            account_id,
            user_id,
            kids_only,
        )
        .fetch_all(pool)
        .await?,
    ))
}

/// Ids the profile can't see — unresolved, or barred by the Kids rule — are skipped. Returns how
/// many of the requested titles are now selected.
pub async fn select(
    pool: &PgPool,
    user_id: Uuid,
    kids_only: bool,
    stremio_ids: &[String],
) -> Result<usize> {
    let Some(account_id) = account_for(pool, user_id).await? else {
        return Ok(0);
    };
    Ok(sqlx::query_scalar!(
        r#"
        INSERT INTO stremio_history_imports (user_id, account_id, stremio_id)
        SELECT $2, h.account_id, h.stremio_id
        FROM stremio_history_items h
        WHERE h.account_id = $1 AND h.stremio_id = ANY($3) AND h.tmdb_id IS NOT NULL
          AND ($4 = FALSE
               OR stremio_title_kids_allowed(h.media_type, h.tmdb_id, h.content_rating))
        ON CONFLICT (user_id, account_id, stremio_id)
        DO UPDATE SET last_error = stremio_history_imports.last_error
        RETURNING stremio_id
        "#,
        account_id,
        user_id,
        stremio_ids,
        kids_only,
    )
    .fetch_all(pool)
    .await?
    .len())
}

pub async fn deselect(pool: &PgPool, user_id: Uuid, stremio_id: &str) -> Result<bool> {
    let result = sqlx::query!(
        "DELETE FROM stremio_history_imports WHERE user_id = $1 AND stremio_id = $2",
        user_id,
        stremio_id,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn list_pending(pool: &PgPool, account_id: Uuid) -> Result<Vec<PendingImport>> {
    Ok(sqlx::query_as!(
        PendingImport,
        r#"
        SELECT i.user_id, h.stremio_id, h.media_type, h.tmdb_id AS "tmdb_id!",
               h.title AS "title!", h.year, h.poster_url, h.season, h.episode,
               h.marker_watched, h.watched, h.position_ms, h.last_watched
        FROM stremio_history_imports i
        JOIN stremio_history_items h
          ON h.account_id = i.account_id AND h.stremio_id = i.stremio_id
        WHERE i.account_id = $1
          AND h.tmdb_id IS NOT NULL AND h.title IS NOT NULL
          AND (i.applied_watched_at IS NULL OR h.last_watched > i.applied_watched_at)
        "#,
        account_id,
    )
    .fetch_all(pool)
    .await?)
}

pub async fn record_import(
    pool: &PgPool,
    user_id: Uuid,
    account_id: Uuid,
    stremio_id: &str,
    applied_watched_at: Option<OffsetDateTime>,
    error: Option<&str>,
) -> Result<()> {
    sqlx::query!(
        r#"
        UPDATE stremio_history_imports
        SET applied_watched_at = COALESCE($4, applied_watched_at), last_error = $5
        WHERE user_id = $1 AND account_id = $2 AND stremio_id = $3
        "#,
        user_id,
        account_id,
        stremio_id,
        applied_watched_at,
        error,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Never un-watches: a re-run only adds what Stremio has seen since, and a resume point never
/// lands on something already finished.
pub async fn mark_movie(pool: &PgPool, movie_id: Uuid, p: &PendingImport) -> Result<()> {
    sqlx::query!(
        r#"
        INSERT INTO movie_watch_states (id, movie_id, user_id, watched, watched_at,
                                        position_seconds, updated_at, source)
        VALUES ($1, $2, $3, $4, CASE WHEN $4 THEN $6::timestamptz END, $5, $6, 'stremio')
        ON CONFLICT (movie_id, user_id) DO UPDATE SET
            watched = TRUE,
            watched_at = COALESCE(movie_watch_states.watched_at, EXCLUDED.watched_at),
            updated_at = NOW()
        WHERE NOT movie_watch_states.watched AND EXCLUDED.watched
        "#,
        Uuid::now_v7(),
        movie_id,
        p.user_id,
        p.watched,
        p.position_seconds(),
        p.last_watched,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Marks every regular episode before (season, episode) watched — and that one too when
/// `marker_watched` — and leaves a resume point on it otherwise.
pub async fn mark_episodes(
    pool: &PgPool,
    show_id: Uuid,
    season: i32,
    episode: i32,
    p: &PendingImport,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query!(
        r#"
        INSERT INTO tv_watch_states (id, episode_id, user_id, watched, watched_at, updated_at,
                                     source)
        SELECT gen_random_uuid(), te.id, $2, TRUE, $6, $6, 'stremio'
        FROM tv_episodes te
        JOIN tv_seasons ts ON ts.id = te.season_id AND NOT ts.is_special
        WHERE ts.show_id = $1
          AND ((ts.season_sort_order, te.episode_sort_order) < ($3, $4)
               OR ($5 AND ts.season_sort_order = $3 AND te.episode_sort_order = $4))
        ON CONFLICT (episode_id, user_id) DO UPDATE SET
            watched = TRUE,
            watched_at = COALESCE(tv_watch_states.watched_at, EXCLUDED.watched_at),
            updated_at = NOW()
        WHERE NOT tv_watch_states.watched
        "#,
        show_id,
        p.user_id,
        season,
        episode,
        p.marker_watched,
        p.last_watched,
    )
    .execute(&mut *tx)
    .await?;
    if let Some(position) = p.position_seconds().filter(|_| !p.marker_watched) {
        sqlx::query!(
            r#"
            INSERT INTO tv_watch_states (id, episode_id, user_id, watched, position_seconds,
                                         updated_at, source)
            SELECT gen_random_uuid(), te.id, $2, FALSE, $5, $6, 'stremio'
            FROM tv_episodes te
            JOIN tv_seasons ts ON ts.id = te.season_id AND NOT ts.is_special
            WHERE ts.show_id = $1 AND ts.season_sort_order = $3 AND te.episode_sort_order = $4
            ON CONFLICT (episode_id, user_id) DO NOTHING
            "#,
            show_id,
            p.user_id,
            season,
            episode,
            position,
            p.last_watched,
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn episode_exists(
    pool: &PgPool,
    show_id: Uuid,
    season: i32,
    episode: i32,
) -> Result<bool> {
    Ok(sqlx::query_scalar!(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM tv_episodes te
            JOIN tv_seasons ts ON ts.id = te.season_id
            WHERE ts.show_id = $1 AND ts.season_sort_order = $2 AND te.episode_sort_order = $3
        ) AS "exists!"
        "#,
        show_id,
        season,
        episode,
    )
    .fetch_one(pool)
    .await?)
}

async fn account_for(pool: &PgPool, user_id: Uuid) -> Result<Option<Uuid>> {
    Ok(sqlx::query_scalar!(
        r#"
        SELECT id FROM stremio_accounts
        WHERE user_id = $1 OR user_id IS NULL
        ORDER BY user_id NULLS LAST
        LIMIT 1
        "#,
        user_id,
    )
    .fetch_optional(pool)
    .await?)
}
