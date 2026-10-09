//! Calendar endpoint — returns upcoming releases across all media types.

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::AppState;
use crate::images::{ImageMap, endpoint_url};
use crate::routes::AuthUser;
use crate::routes::library::helpers::book_cover_url;

// =============================================================================
// Error Type
// =============================================================================

pub enum AppError {
    BadRequest(String),
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

// =============================================================================
// Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct CalendarQuery {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize)]
pub struct CalendarResponse {
    pub items: Vec<CalendarItem>,
}

#[derive(Debug, Serialize)]
pub struct CalendarItem {
    pub id: Uuid,
    pub media_type: String,
    #[serde(with = "crate::serde_date::date")]
    pub date: time::Date,
    pub title: String,
    pub subtitle: Option<String>,
    pub image_url: Option<String>,
    pub link: String,
}

// =============================================================================
// Handler
// =============================================================================

/// GET /api/v1/admin/calendar?from=YYYY-MM-DD&to=YYYY-MM-DD
pub async fn get_calendar(
    State(state): State<AppState>,
    _auth: AuthUser,
    axum::extract::Query(params): axum::extract::Query<CalendarQuery>,
) -> Result<impl IntoResponse, AppError> {
    let from = time::Date::parse(
        &params.from,
        &time::format_description::well_known::Iso8601::DEFAULT,
    )
    .or_else(|_| {
        time::Date::parse(
            &params.from,
            time::macros::format_description!("[year]-[month]-[day]"),
        )
    })
    .map_err(|_| AppError::BadRequest(format!("Invalid 'from' date: {}", params.from)))?;

    let to = time::Date::parse(
        &params.to,
        &time::format_description::well_known::Iso8601::DEFAULT,
    )
    .or_else(|_| {
        time::Date::parse(
            &params.to,
            time::macros::format_description!("[year]-[month]-[day]"),
        )
    })
    .map_err(|_| AppError::BadRequest(format!("Invalid 'to' date: {}", params.to)))?;

    let items = items_between(&state, from, to)
        .await
        .map_err(|e| AppError::Internal(format!("Calendar query failed: {e}")))?;

    Ok(Json(CalendarResponse { items }))
}

/// The calendar's answer, without the HTTP around it.
///
/// `whats_new` is the same question in both directions over a seven-day window,
/// so it calls this rather than growing a second set of queries that would drift
/// from what the calendar page shows.
pub async fn items_between(
    state: &AppState,
    from: time::Date,
    to: time::Date,
) -> Result<Vec<CalendarItem>, sqlx::Error> {
    let (tv_items, sport_items, movie_items, book_items) = tokio::try_join!(
        fetch_tv_episodes(state, from, to, "tv"),
        fetch_tv_episodes(state, from, to, "sport"),
        fetch_movies(state, from, to),
        fetch_books(state, from, to),
    )?;

    let mut items: Vec<CalendarItem> = Vec::new();
    items.extend(tv_items);
    items.extend(sport_items);
    items.extend(movie_items);
    items.extend(book_items);
    items.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.title.cmp(&b.title)));
    Ok(items)
}

// =============================================================================
// Fetch functions
// =============================================================================

async fn fetch_tv_episodes(
    state: &AppState,
    from: time::Date,
    to: time::Date,
    content_category: &str,
) -> Result<Vec<CalendarItem>, sqlx::Error> {
    let media_type = if content_category == "sport" {
        "sport"
    } else {
        "tv"
    };

    let rows = sqlx::query(
        r#"
        SELECT
            te.id,
            tshow.title AS show_title,
            tshow.images AS show_images,
            tshow.id AS show_id,
            ts.season_identifier,
            te.episode_identifier,
            te.title AS episode_title,
            te.air_date
        FROM tv_episodes te
        JOIN tv_seasons ts ON ts.id = te.season_id
        JOIN tv_shows tshow ON tshow.id = ts.show_id
        WHERE te.air_date >= $1 AND te.air_date < $2
          AND tshow.is_monitored = TRUE
          AND tshow.content_category = $3
          AND ts.is_special = FALSE
        ORDER BY te.air_date ASC, tshow.title ASC
        "#,
    )
    .bind(from)
    .bind(to)
    .bind(content_category)
    .fetch_all(&state.pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let id: Uuid = r.get("id");
            let show_id: Uuid = r.get("show_id");
            let show_title: String = r.get("show_title");
            let images_json: serde_json::Value = r.get("show_images");
            let images: ImageMap = serde_json::from_value(images_json).unwrap_or_default();
            let season_id: String = r.get("season_identifier");
            let episode_id: String = r.get("episode_identifier");
            let episode_title: Option<String> = r.get("episode_title");
            let air_date: time::Date = r.get("air_date");

            let subtitle = if media_type == "sport" {
                crate::routes::admin::dashboard::format_sport_label(
                    &show_title,
                    &season_id,
                    episode_title.as_deref(),
                )
            } else if let Some(ref ep_title) = episode_title {
                format!("S{season_id}E{episode_id} - {ep_title}")
            } else {
                format!("S{season_id}E{episode_id}")
            };

            let image_url = endpoint_url(
                &images,
                "poster",
                "/api/v1/admin/tv/shows/",
                show_id,
                "/poster",
            );

            let link = if media_type == "sport" {
                format!("/sports/events/{id}")
            } else {
                format!("/tv/episodes/{id}")
            };

            CalendarItem {
                id,
                media_type: media_type.to_string(),
                date: air_date,
                title: show_title,
                subtitle: Some(subtitle),
                image_url,
                link,
            }
        })
        .collect())
}

async fn fetch_movies(
    state: &AppState,
    from: time::Date,
    to: time::Date,
) -> Result<Vec<CalendarItem>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT
            m.id,
            m.title,
            m.year,
            m.images,
            m.release_date
        FROM movies m
        WHERE m.release_date >= $1 AND m.release_date < $2
          AND (m.want_status != 'none'
               OR EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = m.id))
        ORDER BY m.release_date ASC, m.title ASC
        "#,
    )
    .bind(from)
    .bind(to)
    .fetch_all(&state.pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let id: Uuid = r.get("id");
            let title: String = r.get("title");
            let year: Option<i32> = r.get("year");
            let images_json: serde_json::Value = r.get("images");
            let images: ImageMap = serde_json::from_value(images_json).unwrap_or_default();
            let release_date: time::Date = r.get("release_date");

            let image_url = endpoint_url(&images, "poster", "/api/v1/admin/movies/", id, "/poster");
            let subtitle = year.map(|y| y.to_string());

            CalendarItem {
                id,
                media_type: "movie".to_string(),
                date: release_date,
                title,
                subtitle,
                image_url,
                link: format!("/movies/{id}"),
            }
        })
        .collect())
}

async fn fetch_books(
    state: &AppState,
    from: time::Date,
    to: time::Date,
) -> Result<Vec<CalendarItem>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT
            b.id,
            b.title,
            b.images,
            COALESCE(b.release_date, b.pubdate::date) AS book_date,
            COALESCE(
                (SELECT string_agg(a.name, ', ' ORDER BY a.name)
                 FROM book_authors ba
                 JOIN authors a ON a.id = ba.author_id
                 WHERE ba.book_id = b.id),
                ''
            ) AS author_names
        FROM books b
        WHERE COALESCE(b.release_date, b.pubdate::date) >= $1
          AND COALESCE(b.release_date, b.pubdate::date) < $2
          AND (b.want_status NOT IN ('none', 'skipped') OR (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)))
        ORDER BY COALESCE(b.release_date, b.pubdate::date) ASC, b.title ASC
        "#,
    )
    .bind(from)
    .bind(to)
    .fetch_all(&state.pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let id: Uuid = r.get("id");
            let title: String = r.get("title");
            let images_json: serde_json::Value = r.get("images");
            let images: ImageMap = serde_json::from_value(images_json).unwrap_or_default();
            let book_date: time::Date = r.get("book_date");
            let author_names: String = r.get("author_names");

            let image_url = book_cover_url(&images, id);
            let subtitle = if author_names.is_empty() {
                None
            } else {
                Some(author_names)
            };

            CalendarItem {
                id,
                media_type: "book".to_string(),
                date: book_date,
                title,
                subtitle,
                image_url,
                link: format!("/library/books/{id}"),
            }
        })
        .collect())
}
