//! Dashboard endpoint — returns recently added content across all media types.

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Serialize;
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::AppState;
use crate::images::{ImageMap, endpoint_url};
use crate::routes::AuthUser;
use crate::routes::library::helpers::book_cover_url;

// =============================================================================
// Error Type
// =============================================================================

pub enum AppError {
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

// =============================================================================
// Response Types
// =============================================================================

#[derive(Serialize)]
pub struct DashboardResponse {
    read_next: Vec<ReadNextItem>,
    recent_books: Vec<RecentBook>,
    recent_audiobooks: Vec<RecentBook>,
    recent_movies: Vec<RecentMovie>,
    recent_tv: Vec<RecentTvEntry>,
    recent_sports: Vec<RecentTvEntry>,
}

#[derive(Serialize, Clone)]
pub struct WatchNextItem {
    media_type: String,
    media_id: String,
    title: String,
    subtitle: Option<String>,
    image_url: Option<String>,
    backdrop_url: Option<String>,
    link: String,
    progress_percent: Option<f64>,
    position_seconds: Option<i32>,
    duration_seconds: Option<f64>,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Serialize)]
pub struct ReadNextItem {
    media_type: String,
    media_id: String,
    title: String,
    subtitle: Option<String>,
    image_url: Option<String>,
    link: String,
    progress_percent: Option<f64>,
    position_seconds: Option<i32>,
    duration_seconds: Option<f64>,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Serialize, Clone)]
pub struct RecentItem {
    item_type: String,
    id: String,
    title: String,
    subtitle: String,
    image_url: Option<String>,
    backdrop_url: Option<String>,
    link: String,
    #[serde(with = "time::serde::rfc3339")]
    added_at: OffsetDateTime,
}

#[derive(Serialize)]
pub struct RecentBook {
    id: Uuid,
    title: String,
    authors: String,
    cover_url: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    added_at: OffsetDateTime,
}

#[derive(Serialize)]
pub struct RecentMovie {
    id: Uuid,
    title: String,
    year: Option<i32>,
    poster_url: Option<String>,
    backdrop_url: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    added_at: OffsetDateTime,
}

#[derive(Serialize)]
pub struct RecentTvEntry {
    show_id: Uuid,
    show_title: String,
    show_poster_url: Option<String>,
    show_backdrop_url: Option<String>,
    season_identifier: String,
    episode_id: Uuid,
    episode_identifier: String,
    episode_title: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    added_at: OffsetDateTime,
}

// =============================================================================
// Internal row types for sqlx
// =============================================================================

// =============================================================================
// Handler
// =============================================================================

pub async fn get_dashboard(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<DashboardResponse>, AppError> {
    let (read_next, book_rows, audiobook_rows, movie_rows, tv_rows, sport_rows) = tokio::try_join!(
        fetch_read_next(&state, auth.user_id),
        crate::repositories::dashboard::recent_books(&state.pool, false),
        crate::repositories::dashboard::recent_books(&state.pool, true),
        crate::repositories::dashboard::recent_movies(&state.pool, auth.is_kids),
        crate::repositories::dashboard::recent_episodes(&state.pool, "tv", auth.is_kids),
        crate::repositories::dashboard::recent_episodes(&state.pool, "sport", auth.is_kids),
    )
    .map_err(|e| AppError::Internal(format!("Dashboard query failed: {e}")))?;

    // Build typed sections
    let to_recent_book = |r: &crate::repositories::dashboard::BookRow| {
        let images: ImageMap = serde_json::from_value(r.images.clone()).unwrap_or_default();
        RecentBook {
            id: r.id,
            title: r.title.clone(),
            authors: parse_author_names(&r.authors),
            cover_url: book_cover_url(&images, r.id),
            added_at: r.added_at,
        }
    };
    let recent_books: Vec<RecentBook> = book_rows.iter().map(to_recent_book).collect();
    let recent_audiobooks: Vec<RecentBook> = audiobook_rows.iter().map(to_recent_book).collect();

    let recent_movies: Vec<RecentMovie> = movie_rows
        .iter()
        .map(|r| {
            let images: ImageMap = serde_json::from_value(r.images.clone()).unwrap_or_default();
            RecentMovie {
                id: r.id,
                title: r.title.clone(),
                year: r.year,
                poster_url: endpoint_url(
                    &images,
                    "poster",
                    "/api/v1/admin/movies/",
                    r.id,
                    "/poster",
                ),
                backdrop_url: endpoint_url(
                    &images,
                    "backdrop",
                    "/api/v1/admin/movies/",
                    r.id,
                    "/backdrop",
                ),
                added_at: r.added_at,
            }
        })
        .collect();

    let recent_tv: Vec<RecentTvEntry> = tv_rows
        .iter()
        .map(|r| {
            let images: ImageMap =
                serde_json::from_value(r.show_images.clone()).unwrap_or_default();
            RecentTvEntry {
                show_id: r.show_id,
                show_title: r.show_title.clone(),
                show_poster_url: endpoint_url(
                    &images,
                    "poster",
                    "/api/v1/admin/tv/shows/",
                    r.show_id,
                    "/poster",
                ),
                show_backdrop_url: endpoint_url(
                    &images,
                    "backdrop",
                    "/api/v1/admin/tv/shows/",
                    r.show_id,
                    "/backdrop",
                ),
                season_identifier: r.season_identifier.clone(),
                episode_id: r.episode_id,
                episode_identifier: r.episode_identifier.clone(),
                episode_title: r.episode_title.clone(),
                added_at: r.added_at,
            }
        })
        .collect();

    let recent_sports: Vec<RecentTvEntry> = sport_rows
        .iter()
        .map(|r| {
            let images: ImageMap =
                serde_json::from_value(r.show_images.clone()).unwrap_or_default();
            RecentTvEntry {
                show_id: r.show_id,
                show_title: r.show_title.clone(),
                show_poster_url: endpoint_url(
                    &images,
                    "poster",
                    "/api/v1/admin/sports/leagues/",
                    r.show_id,
                    "/poster",
                ),
                show_backdrop_url: endpoint_url(
                    &images,
                    "backdrop",
                    "/api/v1/admin/sports/leagues/",
                    r.show_id,
                    "/backdrop",
                ),
                season_identifier: r.season_identifier.clone(),
                episode_id: r.episode_id,
                episode_identifier: r.episode_identifier.clone(),
                episode_title: r.episode_title.clone(),
                added_at: r.added_at,
            }
        })
        .collect();

    Ok(Json(DashboardResponse {
        read_next,
        recent_books,
        recent_audiobooks,
        recent_movies,
        recent_tv,
        recent_sports,
    }))
}

// =============================================================================
// Watch Next query (in-progress video + up-next episodes)
// =============================================================================

async fn fetch_watch_next(
    state: &AppState,
    user_id: Uuid,
    kids_only: bool,
) -> Result<Vec<WatchNextItem>, sqlx::Error> {
    let (movies, episodes, watchlist_shows, watchlist_movies) = tokio::try_join!(
        crate::repositories::dashboard::in_progress_movies(&state.pool, user_id, kids_only),
        crate::repositories::dashboard::watch_next_episodes(&state.pool, user_id, kids_only),
        crate::repositories::dashboard::watchlist_shows(&state.pool, user_id, kids_only),
        crate::repositories::dashboard::watchlist_movies(&state.pool, user_id, kids_only),
    )?;

    // Merge in-progress + up-next + watchlist into WatchNextItem, sort by updated_at DESC.
    // Watchlist rows carry `added_at` as their timestamp, so something just added
    // surfaces at the top — which is the point of asking for it.
    let mut items: Vec<WatchNextItem> = Vec::with_capacity(
        movies.len() + episodes.len() + watchlist_shows.len() + watchlist_movies.len(),
    );

    for r in &movies {
        let images: ImageMap = serde_json::from_value(r.images.clone()).unwrap_or_default();
        let progress: Option<f64> = r
            .duration_seconds
            .filter(|&d| d > 0.0)
            .map(|d| (f64::from(r.position_seconds) / d * 100.0).min(100.0));
        items.push(WatchNextItem {
            media_type: "movie".into(),
            media_id: r.movie_id.to_string(),
            title: r.title.clone(),
            subtitle: r.year.map(|y: i32| y.to_string()),
            image_url: endpoint_url(
                &images,
                "poster",
                "/api/v1/admin/movies/",
                r.movie_id,
                "/poster",
            ),
            backdrop_url: endpoint_url(
                &images,
                "backdrop",
                "/api/v1/admin/movies/",
                r.movie_id,
                "/backdrop",
            ),
            link: format!("/movies/{}", r.movie_id),
            progress_percent: progress,
            position_seconds: Some(r.position_seconds),
            duration_seconds: r.duration_seconds,
            updated_at: r.updated_at,
        });
    }

    for r in &episodes {
        let images: ImageMap = serde_json::from_value(r.show_images.clone()).unwrap_or_default();
        let progress: Option<f64> = r.position_seconds.and_then(|pos| {
            r.duration_seconds
                .filter(|&d| d > 0.0)
                .map(|d| (f64::from(pos) / d * 100.0).min(100.0))
        });
        let is_sport = r.content_category == "sport";
        let (media_type, link, image_base, subtitle) = if is_sport {
            (
                "event",
                format!("/sports/events/{}", r.episode_id),
                "/api/v1/admin/sports/leagues/",
                format_sport_label(
                    &r.show_title,
                    &r.season_identifier,
                    r.episode_title.as_deref(),
                ),
            )
        } else {
            (
                "episode",
                format!("/tv/episodes/{}", r.episode_id),
                "/api/v1/admin/tv/shows/",
                format_episode_label(
                    &r.season_identifier,
                    &r.episode_identifier,
                    r.episode_title.as_deref(),
                ),
            )
        };
        items.push(WatchNextItem {
            media_type: media_type.into(),
            media_id: r.episode_id.to_string(),
            title: r.show_title.clone(),
            subtitle: Some(subtitle),
            image_url: endpoint_url(&images, "poster", image_base, r.show_id, "/poster"),
            backdrop_url: endpoint_url(&images, "backdrop", image_base, r.show_id, "/backdrop"),
            link,
            progress_percent: progress,
            position_seconds: r.position_seconds,
            duration_seconds: r.duration_seconds,
            updated_at: r.updated_at,
        });
    }

    // A watchlisted show the user has never started: the card points at the
    // episode they would begin on. `watchlist_shows` already excludes shows with
    // any watch history, so this cannot double up with the up-next branch above.
    for r in &watchlist_shows {
        let images: ImageMap = serde_json::from_value(r.show_images.clone()).unwrap_or_default();
        let is_sport = r.content_category == "sport";
        let (media_type, link, image_base, subtitle) = if is_sport {
            (
                "event",
                format!("/sports/events/{}", r.episode_id),
                "/api/v1/admin/sports/leagues/",
                format_sport_label(
                    &r.show_title,
                    &r.season_identifier,
                    r.episode_title.as_deref(),
                ),
            )
        } else {
            (
                "episode",
                format!("/tv/episodes/{}", r.episode_id),
                "/api/v1/admin/tv/shows/",
                format_episode_label(
                    &r.season_identifier,
                    &r.episode_identifier,
                    r.episode_title.as_deref(),
                ),
            )
        };
        items.push(WatchNextItem {
            media_type: media_type.into(),
            media_id: r.episode_id.to_string(),
            title: r.show_title.clone(),
            subtitle: Some(subtitle),
            image_url: endpoint_url(&images, "poster", image_base, r.show_id, "/poster"),
            backdrop_url: endpoint_url(&images, "backdrop", image_base, r.show_id, "/backdrop"),
            link,
            progress_percent: None,
            position_seconds: None,
            duration_seconds: None,
            updated_at: r.added_at,
        });
    }

    for r in &watchlist_movies {
        let images: ImageMap = serde_json::from_value(r.images.clone()).unwrap_or_default();
        items.push(WatchNextItem {
            media_type: "movie".into(),
            media_id: r.movie_id.to_string(),
            title: r.title.clone(),
            subtitle: r.year.map(|y: i32| y.to_string()),
            image_url: endpoint_url(
                &images,
                "poster",
                "/api/v1/admin/movies/",
                r.movie_id,
                "/poster",
            ),
            backdrop_url: endpoint_url(
                &images,
                "backdrop",
                "/api/v1/admin/movies/",
                r.movie_id,
                "/backdrop",
            ),
            link: format!("/movies/{}", r.movie_id),
            progress_percent: None,
            position_seconds: None,
            duration_seconds: None,
            updated_at: r.added_at,
        });
    }

    items.sort_by_key(|b| std::cmp::Reverse(b.updated_at));
    items.truncate(20);

    Ok(items)
}

// =============================================================================
// Read Next query (in-progress books/audiobooks)
// =============================================================================

async fn fetch_read_next(
    state: &AppState,
    user_id: Uuid,
) -> Result<Vec<ReadNextItem>, sqlx::Error> {
    let books = crate::repositories::dashboard::in_progress_books(&state.pool, user_id).await?;

    // Filter out books with less than 2% progress (but keep books with no progress data,
    // since those were explicitly set to "reading" without a bookmark)
    let items: Vec<ReadNextItem> = books
        .into_iter()
        .filter(|b| b.progress_percent.is_none_or(|p| p >= 2.0))
        .map(|r| {
            let images: ImageMap = serde_json::from_value(r.images.clone()).unwrap_or_default();
            let media_type = if r.has_audiobook { "audiobook" } else { "book" };
            ReadNextItem {
                media_type: media_type.into(),
                media_id: r.book_id.to_string(),
                title: r.title.clone(),
                subtitle: Some(parse_author_names(&r.authors)),
                image_url: book_cover_url(&images, r.book_id),
                link: format!("/library/books/{}", r.book_id),
                progress_percent: r.progress_percent,
                position_seconds: None,
                duration_seconds: r.duration_seconds.map(f64::from),
                updated_at: r.updated_at,
            }
        })
        .collect();

    Ok(items)
}

/// Format an episode label like "S01E03 - Episode Title".
fn format_episode_label(season: &str, episode: &str, title: Option<&str>) -> String {
    let s_num: u32 = season.parse().unwrap_or(0);
    let e_num: u32 = episode.parse().unwrap_or(0);
    let code = format!("S{s_num:02}E{e_num:02}");
    match title {
        Some(t) if !t.is_empty() => format!("{code} - {t}"),
        _ => code,
    }
}

/// Format a sport event label like "2026 Thailand Race" (year + event title minus league name).
pub(crate) fn format_sport_label(
    show_title: &str,
    season: &str,
    episode_title: Option<&str>,
) -> String {
    let event_title = episode_title
        .map(|t| t.strip_prefix(show_title).unwrap_or(t).trim())
        .unwrap_or("");
    if event_title.is_empty() {
        season.to_string()
    } else {
        format!("{season} {event_title}")
    }
}

/// Fetch watch-next and recently-added items for use on the home/discover page.
pub async fn fetch_home_sections(
    state: &AppState,
    user_id: Uuid,
    kids_only: bool,
) -> Result<(Vec<WatchNextItem>, Vec<RecentItem>), sqlx::Error> {
    let (watch_next, book_rows, movie_rows, tv_rows, sport_rows) = tokio::try_join!(
        fetch_watch_next(state, user_id, kids_only),
        crate::repositories::dashboard::recent_books(&state.pool, false),
        crate::repositories::dashboard::recent_movies(&state.pool, kids_only),
        crate::repositories::dashboard::recent_episodes(&state.pool, "tv", kids_only),
        crate::repositories::dashboard::recent_episodes(&state.pool, "sport", kids_only),
    )?;

    let mut unified: Vec<RecentItem> = Vec::new();
    for r in &book_rows {
        let images: ImageMap = serde_json::from_value(r.images.clone()).unwrap_or_default();
        let authors_str = parse_author_names(&r.authors);
        unified.push(RecentItem {
            item_type: "book".into(),
            id: r.id.to_string(),
            title: r.title.clone(),
            subtitle: authors_str,
            image_url: book_cover_url(&images, r.id),
            backdrop_url: None,
            link: format!("/library/books/{}", r.id),
            added_at: r.added_at,
        });
    }
    for r in &movie_rows {
        let images: ImageMap = serde_json::from_value(r.images.clone()).unwrap_or_default();
        unified.push(RecentItem {
            item_type: "movie".into(),
            id: r.id.to_string(),
            title: r.title.clone(),
            subtitle: r.year.map(|y| y.to_string()).unwrap_or_default(),
            image_url: endpoint_url(&images, "poster", "/api/v1/admin/movies/", r.id, "/poster"),
            backdrop_url: endpoint_url(
                &images,
                "backdrop",
                "/api/v1/admin/movies/",
                r.id,
                "/backdrop",
            ),
            link: format!("/movies/{}", r.id),
            added_at: r.added_at,
        });
    }
    for r in &tv_rows {
        let images: ImageMap = serde_json::from_value(r.show_images.clone()).unwrap_or_default();
        unified.push(RecentItem {
            item_type: "episode".into(),
            id: r.episode_id.to_string(),
            title: r.show_title.clone(),
            subtitle: format_episode_label(
                &r.season_identifier,
                &r.episode_identifier,
                r.episode_title.as_deref(),
            ),
            image_url: endpoint_url(
                &images,
                "poster",
                "/api/v1/admin/tv/shows/",
                r.show_id,
                "/poster",
            ),
            backdrop_url: endpoint_url(
                &images,
                "backdrop",
                "/api/v1/admin/tv/shows/",
                r.show_id,
                "/backdrop",
            ),
            link: format!("/tv/{}", r.show_id),
            added_at: r.added_at,
        });
    }
    for r in &sport_rows {
        let images: ImageMap = serde_json::from_value(r.show_images.clone()).unwrap_or_default();
        unified.push(RecentItem {
            item_type: "event".into(),
            id: r.episode_id.to_string(),
            title: r.show_title.clone(),
            subtitle: format_sport_label(
                &r.show_title,
                &r.season_identifier,
                r.episode_title.as_deref(),
            ),
            image_url: endpoint_url(
                &images,
                "poster",
                "/api/v1/admin/sports/leagues/",
                r.show_id,
                "/poster",
            ),
            backdrop_url: endpoint_url(
                &images,
                "backdrop",
                "/api/v1/admin/sports/leagues/",
                r.show_id,
                "/backdrop",
            ),
            link: format!("/sports/{}", r.show_id),
            added_at: r.added_at,
        });
    }
    unified.sort_by_key(|b| std::cmp::Reverse(b.added_at));
    unified.truncate(20);

    Ok((watch_next, unified))
}

/// Parse the JSONB author array into a comma-separated string.
fn parse_author_names(authors_json: &Option<serde_json::Value>) -> String {
    #[derive(serde::Deserialize)]
    struct AuthorName {
        name: String,
    }

    let Some(val) = authors_json else {
        return String::new();
    };
    let names: Vec<AuthorName> = serde_json::from_value(val.clone()).unwrap_or_default();
    names
        .into_iter()
        .map(|a| a.name)
        .collect::<Vec<_>>()
        .join(", ")
}
