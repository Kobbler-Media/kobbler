//! Library stats endpoint for nav counts.

use axum::{
    Json as AxumJson,
    extract::{Query, State},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{AppState, routes::AuthUser};

/// Stats response for nav sidebar counts.
#[derive(Serialize)]
pub struct StatsResponse {
    pub books_count: i64,
    pub audiobooks_count: i64,
    pub series_count: i64,
    pub authors_count: i64,
    pub shelves_count: i64,
    pub downloads_active: i64,
    pub ingest_pending: i64,
    pub movies_count: i64,
    pub tv_shows_count: i64,
    pub sports_count: i64,
    pub people_count: i64,
    pub kobload_configured: bool,
}

/// DB-only subset of stats (excludes runtime-derived fields like kobload_configured).
struct DbStats {
    books_count: i64,
    audiobooks_count: i64,
    series_count: i64,
    authors_count: i64,
    shelves_count: i64,
    downloads_active: i64,
    ingest_pending: i64,
    movies_count: i64,
    tv_shows_count: i64,
    sports_count: i64,
    people_count: i64,
}

#[derive(Deserialize)]
pub struct StatsQuery {
    /// Filter book/series/author counts by format: "ebook" or "audiobook"
    pub format: Option<String>,
}

/// Get library stats for nav counts.
pub async fn get_stats(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<StatsQuery>,
) -> Response {
    let format = params.format.as_deref();
    match fetch_stats(&state.pool, format, auth.user_id).await {
        Ok(db) => {
            let runtime = state.runtime_config.get().await;
            let kobload_configured = runtime
                .kobload_base_url
                .as_ref()
                .is_some_and(|u| !u.is_empty())
                && runtime
                    .kobload_api_key
                    .as_ref()
                    .is_some_and(|k| !k.is_empty());
            let stats = StatsResponse {
                books_count: db.books_count,
                audiobooks_count: db.audiobooks_count,
                series_count: db.series_count,
                authors_count: db.authors_count,
                shelves_count: db.shelves_count,
                downloads_active: db.downloads_active,
                ingest_pending: db.ingest_pending,
                movies_count: db.movies_count,
                tv_shows_count: db.tv_shows_count,
                sports_count: db.sports_count,
                people_count: db.people_count,
                kobload_configured,
            };
            AxumJson(stats).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch stats");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to fetch stats",
            )
                .into_response()
        }
    }
}

/// `user_id` scopes the shelves count — shelves are per-user, every other
/// count in this query is library-wide.
async fn fetch_stats(
    pool: &PgPool,
    format: Option<&str>,
    user_id: Uuid,
) -> Result<DbStats, sqlx::Error> {
    // Normalize format param: None, "all", or unknown → no filter
    let fmt_param: Option<String> = match format {
        Some("ebook") => Some("ebook".to_string()),
        Some("audiobook") => Some("audiobook".to_string()),
        _ => None,
    };

    // Get counts in a single query for efficiency.
    // The format param conditionally filters books/series/authors counts
    // using parameterized SQL (NULL = no filter, "ebook"/"audiobook" = filtered).
    // Non-book counts (movies, tv, sports, etc.) are unaffected by format.
    sqlx::query_as!(
        DbStats,
        r#"
        SELECT
            (SELECT COUNT(*) FROM books
             WHERE (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = books.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = books.id))
             AND ($1::text IS NULL
               OR ($1 = 'ebook' AND EXISTS (SELECT 1 FROM book_files bf2 WHERE bf2.book_id = books.id))
               OR ($1 = 'audiobook' AND EXISTS (SELECT 1 FROM audiobook_files af2 WHERE af2.book_id = books.id))
             )
            ) AS "books_count!",
            (SELECT COUNT(DISTINCT b.id) FROM books b
             WHERE EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)
            ) AS "audiobooks_count!",
            (SELECT COUNT(DISTINCT s.id) FROM series s
             WHERE EXISTS (SELECT 1 FROM book_series bss JOIN books b ON b.id = bss.book_id WHERE bss.series_id = s.id AND (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
               AND ($1::text IS NULL
                 OR ($1 = 'ebook' AND EXISTS (SELECT 1 FROM book_files bf2 WHERE bf2.book_id = b.id))
                 OR ($1 = 'audiobook' AND EXISTS (SELECT 1 FROM audiobook_files af2 WHERE af2.book_id = b.id))
               )
             )
            ) AS "series_count!",
            (SELECT COUNT(DISTINCT a.id) FROM authors a
             WHERE EXISTS (SELECT 1 FROM book_authors ba JOIN books b ON b.id = ba.book_id
                          WHERE ba.author_id = a.id AND (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
               AND ($1::text IS NULL
                 OR ($1 = 'ebook' AND EXISTS (SELECT 1 FROM book_files bf3 WHERE bf3.book_id = b.id))
                 OR ($1 = 'audiobook' AND EXISTS (SELECT 1 FROM audiobook_files af3 WHERE af3.book_id = b.id))
               )
             )
            ) AS "authors_count!",
            (SELECT COUNT(*) FROM shelves WHERE user_id = $2) AS "shelves_count!",
            0::bigint AS "downloads_active!",
            (SELECT COUNT(*) FROM books WHERE ingest_status = 'pending') AS "ingest_pending!",
            (SELECT COUNT(*) FROM movies WHERE EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = movies.id)) AS "movies_count!",
            (SELECT COUNT(*) FROM tv_shows WHERE content_category = 'tv') AS "tv_shows_count!",
            (SELECT COUNT(*) FROM tv_shows WHERE content_category = 'sport') AS "sports_count!",
            (SELECT COUNT(*) FROM people p
             WHERE p.is_monitored = true
                OR EXISTS (SELECT 1 FROM movie_cast mc WHERE mc.person_id = p.id)
                OR EXISTS (SELECT 1 FROM movie_crew mcr WHERE mcr.person_id = p.id)
                OR EXISTS (SELECT 1 FROM tv_cast tc WHERE tc.person_id = p.id)
                OR EXISTS (SELECT 1 FROM tv_crew tcr WHERE tcr.person_id = p.id)
            ) AS "people_count!"
        "#,
        fmt_param,
        user_id,
    )
    .fetch_one(pool)
    .await
}
