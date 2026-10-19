use serde::Serialize;
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::discover_art::art_url;
use crate::jobs::handlers::discover_sync::DiscoverItem;
use crate::repositories::stremio_continue::{self, ContinueRow};
use crate::routes::admin::dashboard::format_episode_label;
use crate::routes::admin::discover::DiscoverItemWithStatus;
use crate::routes::admin::library_status::{batch_movie_status, batch_tv_status};

#[derive(Debug, Serialize)]
pub struct StremioContinueItem {
    #[serde(flatten)]
    pub item: DiscoverItemWithStatus,
    pub subtitle: Option<String>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub progress_percent: Option<f64>,
    #[serde(with = "time::serde::rfc3339")]
    pub last_watched: OffsetDateTime,
}

/// `exclude_*` are the library movies and episodes already on the user's What to Watch row; a
/// title shown there is not repeated here.
pub async fn continue_watching(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    kids_only: bool,
    exclude_movie_ids: &[Uuid],
    exclude_episode_ids: &[Uuid],
) -> Vec<StremioContinueItem> {
    let rows = match stremio_continue::list_for_user(
        pool,
        user_id,
        kids_only,
        exclude_movie_ids,
        exclude_episode_ids,
    )
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("Failed to load Stremio Continue Watching: {e}");
            return Vec::new();
        }
    };

    let ids_of = |media_type: &str| -> Vec<String> {
        rows.iter()
            .filter(|r| r.media_type == media_type)
            .map(|r| r.tmdb_id.clone())
            .collect()
    };
    let movie_status = batch_movie_status(pool, &ids_of("movie")).await;
    let tv_status = batch_tv_status(pool, &ids_of("tv")).await;

    rows.into_iter()
        .map(|r| {
            let status = match r.media_type.as_str() {
                "movie" => movie_status.get(&r.tmdb_id),
                _ => tv_status.get(&r.tmdb_id),
            };
            StremioContinueItem {
                subtitle: subtitle(&r),
                season: r.season,
                episode: r.episode,
                progress_percent: (r.kind == "resume" && r.duration_ms > 0)
                    .then(|| (r.position_ms as f64 / r.duration_ms as f64 * 100.0).min(100.0)),
                last_watched: r.last_watched,
                item: DiscoverItemWithStatus {
                    library_id: status.map(|s| s.id),
                    want_status: status.and_then(|s| s.want_status.clone()),
                    is_in_library: status.and_then(|s| s.is_in_library),
                    item: DiscoverItem {
                        provider: "tmdb".into(),
                        provider_id: r.tmdb_id.clone(),
                        media_type: r.media_type,
                        title: r.title,
                        year: r.year,
                        description: r.description,
                        poster_url: r.poster_url.as_deref().map(art_url),
                        backdrop_url: r.backdrop_url.as_deref().map(art_url),
                        genres: r.genres,
                        rating: r.rating,
                        rating_count: None,
                        authors: Vec::new(),
                        identifiers: json!({ "tmdb_id": r.tmdb_id, "imdb_id": r.stremio_id }),
                    },
                },
            }
        })
        .collect()
}

fn subtitle(r: &ContinueRow) -> Option<String> {
    let (season, episode) = (r.season?, r.episode?);
    let label = format_episode_label(
        &season.to_string(),
        &episode.to_string(),
        r.episode_title.as_deref(),
    );
    Some(match r.kind.as_str() {
        "new_episode" => format!("New · {label}"),
        _ => label,
    })
}
