//! Per-user "want to watch" list.

use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

pub const SHOW: &str = "show";
pub const MOVIE: &str = "movie";

pub fn is_valid_media_type(media_type: &str) -> bool {
    matches!(media_type, SHOW | MOVIE)
}

pub async fn add<'e, E>(exec: E, user_id: Uuid, media_type: &str, media_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO watchlist_items (user_id, media_type, media_id)
        VALUES ($1, $2, $3)
        ON CONFLICT (user_id, media_type, media_id) DO NOTHING
        "#,
        user_id,
        media_type,
        media_id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn remove<'e, E>(exec: E, user_id: Uuid, media_type: &str, media_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        DELETE FROM watchlist_items
        WHERE user_id = $1 AND media_type = $2 AND media_id = $3
        "#,
        user_id,
        media_type,
        media_id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn contains<'e, E>(
    exec: E,
    user_id: Uuid,
    media_type: &str,
    media_id: Uuid,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM watchlist_items
            WHERE user_id = $1 AND media_type = $2 AND media_id = $3
        ) AS "exists!"
        "#,
        user_id,
        media_type,
        media_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(row.exists)
}

/// Drop every user's watchlist entry for one item. Called from the show/movie
/// delete paths — there is no FK to cascade through, the row is polymorphic.
pub async fn remove_for_media<'e, E>(exec: E, media_type: &str, media_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "DELETE FROM watchlist_items WHERE media_type = $1 AND media_id = $2",
        media_type,
        media_id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub struct WatchlistMembershipRow {
    pub media_type: String,
    pub media_id: Uuid,
    pub title: String,
    pub subtitle: Option<String>,
    pub images: serde_json::Value,
    pub added_at: OffsetDateTime,
}

/// Literal membership, newest first.
///
/// Deliberately NOT the filtered Up Next queries in `dashboard`: those answer
/// "what can she start right now", and hid an item the moment a stray 30-second
/// watch state existed. This answers "what is on her list", which is what the
/// list screen and the detail-page button both mean.
pub async fn list_for_user<'e, E>(exec: E, user_id: Uuid) -> Result<Vec<WatchlistMembershipRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        WatchlistMembershipRow,
        r#"
        SELECT
            w.media_type AS "media_type!",
            w.media_id AS "media_id!",
            tshow.title AS "title!",
            NULL::text AS subtitle,
            tshow.images AS "images!: serde_json::Value",
            w.added_at AS "added_at!"
        FROM watchlist_items w
        JOIN tv_shows tshow ON tshow.id = w.media_id
        WHERE w.user_id = $1 AND w.media_type = 'show'

        UNION ALL

        SELECT
            w.media_type AS "media_type!",
            w.media_id AS "media_id!",
            m.title AS "title!",
            m.year::text AS subtitle,
            m.images AS "images!: serde_json::Value",
            w.added_at AS "added_at!"
        FROM watchlist_items w
        JOIN movies m ON m.id = w.media_id
        WHERE w.user_id = $1 AND w.media_type = 'movie'

        ORDER BY 6 DESC
        "#,
        user_id,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}
