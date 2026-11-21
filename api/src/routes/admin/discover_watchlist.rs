use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::discover::{DiscoverShowSpec, enqueue_show_enrich, ensure_discover_show};
use crate::AppState;
use crate::LibraryEvent;
use crate::jobs::{self, JobContext};
use crate::repositories::{media_roots, movies, watchlist};
use crate::routes::AuthUser;

#[derive(Debug, Deserialize)]
pub struct DiscoverWatchlistRequest {
    pub media_type: String,
    #[serde(flatten)]
    pub spec: DiscoverShowSpec,
}

/// POST /api/v1/admin/discover/watchlist
///
/// Puts a title that is not in the library yet on the caller's watchlist. The
/// watchlist is keyed on a library row, so one is created — but nothing is
/// wanted or monitored: the point is to stream it later, not to collect it.
pub async fn add_discover_to_watchlist(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<DiscoverWatchlistRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }

    let resolved = match body.media_type.as_str() {
        "tv" => ensure_discover_show(&state.pool, &body.spec, false)
            .await
            .map(|(id, created)| (watchlist::SHOW, id, created)),
        "movie" => ensure_discover_movie(&state.pool, &body.spec)
            .await
            .map(|(id, created)| (watchlist::MOVIE, id, created)),
        other => {
            return error(
                StatusCode::BAD_REQUEST,
                format!("Invalid media_type: {other}. Must be 'tv' or 'movie'."),
            );
        }
    };
    let (media_type, media_id, created) = match resolved {
        Ok(r) => r,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to add to library: {e}"),
            );
        }
    };

    if let Err(e) = watchlist::add(&state.pool, auth.user_id, media_type, media_id).await {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to add to watchlist: {e}"),
        );
    }

    if created {
        if media_type == watchlist::SHOW {
            enqueue_show_enrich(&state, media_id, &body.spec.title).await;
            let _ = state
                .library_events
                .send(LibraryEvent::tv_show_updated(media_id));
        } else {
            enqueue_movie_enrich(&state, media_id, &body.spec.title).await;
            let _ = state
                .library_events
                .send(LibraryEvent::movie_updated(media_id));
        }
    }

    (
        StatusCode::OK,
        Json(json!({ "media_type": media_type, "media_id": media_id })),
    )
        .into_response()
}

async fn ensure_discover_movie(
    pool: &sqlx::PgPool,
    spec: &DiscoverShowSpec,
) -> crate::repositories::Result<(Uuid, bool)> {
    let identifier_key = format!("{}_id", spec.provider);
    if let Some(m) = movies::get_by_identifier(pool, &identifier_key, &spec.provider_id).await? {
        return Ok((m.id, false));
    }
    let media_root_id = media_roots::get_default_for_type(pool, "movie")
        .await
        .ok()
        .flatten()
        .map(|r| r.id);
    let input = movies::MovieInput {
        title: spec.title.clone(),
        year: spec.year,
        runtime: spec.runtime,
        description: spec.description.clone(),
        images: spec.images(),
        identifiers: spec.identifiers.clone(),
        genres: spec.genres.clone(),
        rating: spec.rating,
        rating_count: spec.rating_count,
        media_root_id,
        ..Default::default()
    };
    let id = movies::upsert_by_provider(pool, &spec.provider, &spec.provider_id, input).await?;
    Ok((id, true))
}

async fn enqueue_movie_enrich(state: &AppState, movie_id: Uuid, title: &str) {
    let display = jobs::fetch_job_display(&state.pool, "movie", movie_id).await;
    let context = JobContext::for_entity("movie", movie_id, format!("Enriching '{title}'"))
        .with_display_opt(display);
    if let Err(e) = jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "movie_enrich",
        json!({ "movie_id": movie_id.to_string() }),
        context,
    )
    .await
    {
        tracing::warn!(movie_id = %movie_id, error = %e, "Failed to enqueue movie enrichment");
    }
}

fn error(status: StatusCode, message: String) -> axum::response::Response {
    (status, Json::<Value>(json!({ "error": message }))).into_response()
}
