//! Read-only queries behind the chat bot's tools.
//!
//! Everything here is deliberately library-wide. Watch state, reading progress
//! and personal ratings are per-user and are not counted from this module —
//! the search documents were fixed in schema v32/v33 for exactly that reason,
//! and a "how many unread" number computed without a `user_id` would be the
//! same bug in a different place.

use sqlx::{Executor, PgPool, Postgres};
use time::Date;
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, sqlx::Error>;

#[derive(Debug, Clone)]
pub struct LibraryStats {
    pub books: i64,
    pub audiobooks: i64,
    pub authors: i64,
    pub series: i64,
    pub movies: i64,
    pub tv_shows: i64,
    pub sports: i64,
    pub episodes_available: i64,
    pub books_wanted: i64,
    pub movies_wanted: i64,
    pub episodes_wanted: i64,
}

pub async fn library_stats<'e, E>(exec: E) -> Result<LibraryStats>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT
            (SELECT COUNT(*) FROM books b
             WHERE EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)
                OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)
            ) AS "books!",
            (SELECT COUNT(DISTINCT af.book_id) FROM audiobook_files af) AS "audiobooks!",
            (SELECT COUNT(DISTINCT ba.author_id) FROM book_authors ba) AS "authors!",
            (SELECT COUNT(DISTINCT bs.series_id) FROM book_series bs) AS "series!",
            (SELECT COUNT(*) FROM movies m
             WHERE EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = m.id AND mf.available)
            ) AS "movies!",
            (SELECT COUNT(*) FROM tv_shows WHERE content_category = 'tv') AS "tv_shows!",
            (SELECT COUNT(*) FROM tv_shows WHERE content_category = 'sport') AS "sports!",
            (SELECT COUNT(DISTINCT tefe.episode_id)
             FROM tv_episode_file_episodes tefe
             JOIN tv_episode_files tef ON tef.id = tefe.episode_file_id AND tef.available
            ) AS "episodes_available!",
            (SELECT COUNT(*) FROM books WHERE want_status NOT IN ('none', 'skipped')) AS "books_wanted!",
            (SELECT COUNT(*) FROM movies WHERE want_status NOT IN ('none', 'skipped')) AS "movies_wanted!",
            (SELECT COUNT(*) FROM tv_episodes WHERE want_status NOT IN ('none', 'skipped')) AS "episodes_wanted!"
        "#,
    )
    .fetch_one(exec)
    .await?;

    Ok(LibraryStats {
        books: row.books,
        audiobooks: row.audiobooks,
        authors: row.authors,
        series: row.series,
        movies: row.movies,
        tv_shows: row.tv_shows,
        sports: row.sports,
        episodes_available: row.episodes_available,
        books_wanted: row.books_wanted,
        movies_wanted: row.movies_wanted,
        episodes_wanted: row.episodes_wanted,
    })
}

#[derive(Debug, Clone)]
pub struct ShowMatch {
    pub id: Uuid,
    pub title: String,
    pub year: Option<i32>,
    pub content_category: String,
    pub status: Option<String>,
}

/// Candidate shows for a title the user said out loud.
///
/// Returns several on purpose: an ambiguous title is a question for the model
/// to ask, never a row for this function to pick. The exact-match-first
/// ordering is what makes a full title resolve in one step anyway.
pub async fn find_shows(pool: &PgPool, title: &str, limit: i64) -> Result<Vec<ShowMatch>> {
    let pattern = format!("%{}%", title.trim());
    let rows = sqlx::query!(
        r#"
        SELECT id, title, year, content_category, status
        FROM tv_shows
        WHERE title ILIKE $1 OR original_title ILIKE $1
        ORDER BY (LOWER(title) = LOWER($2)) DESC, title ASC, id ASC
        LIMIT $3
        "#,
        pattern,
        title.trim(),
        limit,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| ShowMatch {
            id: r.id,
            title: r.title,
            year: r.year,
            content_category: r.content_category,
            status: r.status,
        })
        .collect())
}

#[derive(Debug, Clone)]
pub struct UpcomingEpisode {
    pub episode_id: Uuid,
    pub show_id: Uuid,
    pub show_title: String,
    pub content_category: String,
    pub season_identifier: String,
    pub episode_identifier: String,
    pub title: Option<String>,
    pub air_date: Date,
}

/// The next episode of a show with an air date in the future.
///
/// Not filtered on `is_monitored`: the calendar endpoint filters that way
/// because it is drawing a schedule of things being acquired, but "when's the
/// next one" is a question about a show, monitored or not.
pub async fn next_airing(pool: &PgPool, show_id: Uuid) -> Result<Option<UpcomingEpisode>> {
    let row = sqlx::query!(
        r#"
        SELECT
            te.id AS episode_id,
            tshow.id AS show_id,
            tshow.title AS show_title,
            tshow.content_category,
            ts.season_identifier,
            te.episode_identifier,
            te.title,
            te.air_date AS "air_date!"
        FROM tv_episodes te
        JOIN tv_seasons ts ON ts.id = te.season_id
        JOIN tv_shows tshow ON tshow.id = ts.show_id
        WHERE tshow.id = $1
          AND te.air_date > CURRENT_DATE
          AND ts.is_special = FALSE
        ORDER BY te.air_date ASC, ts.season_sort_order ASC, te.episode_sort_order ASC, te.id ASC
        LIMIT 1
        "#,
        show_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| UpcomingEpisode {
        episode_id: r.episode_id,
        show_id: r.show_id,
        show_title: r.show_title,
        content_category: r.content_category,
        season_identifier: r.season_identifier,
        episode_identifier: r.episode_identifier,
        title: r.title,
        air_date: r.air_date,
    }))
}

/// The most recent episode of a show that has already aired — the honest
/// answer when nothing is scheduled, so the bot can say "nothing listed, the
/// last one was …" instead of going quiet.
pub async fn last_aired(pool: &PgPool, show_id: Uuid) -> Result<Option<UpcomingEpisode>> {
    let row = sqlx::query!(
        r#"
        SELECT
            te.id AS episode_id,
            tshow.id AS show_id,
            tshow.title AS show_title,
            tshow.content_category,
            ts.season_identifier,
            te.episode_identifier,
            te.title,
            te.air_date AS "air_date!"
        FROM tv_episodes te
        JOIN tv_seasons ts ON ts.id = te.season_id
        JOIN tv_shows tshow ON tshow.id = ts.show_id
        WHERE tshow.id = $1
          AND te.air_date <= CURRENT_DATE
          AND ts.is_special = FALSE
        ORDER BY te.air_date DESC, ts.season_sort_order DESC, te.episode_sort_order DESC, te.id DESC
        LIMIT 1
        "#,
        show_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| UpcomingEpisode {
        episode_id: r.episode_id,
        show_id: r.show_id,
        show_title: r.show_title,
        content_category: r.content_category,
        season_identifier: r.season_identifier,
        episode_identifier: r.episode_identifier,
        title: r.title,
        air_date: r.air_date,
    }))
}
