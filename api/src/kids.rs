//! Kids profiles: what a profile flagged `users.is_kids` may see and play.
//!
//! The rule itself lives in SQL (`kids_allowed()`, migration 0183) so a list query and a
//! single-title check can never disagree.

use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use sqlx::PgPool;
use std::collections::HashSet;
use uuid::Uuid;

use crate::AppState;
use crate::routes::{extract_bearer, extract_session_cookie, require_auth_token};
use crate::search::{SearchHit, UnifiedSearchResponse};

/// [`allowed_sql`] for a row aliased `b`, as `letters::available_letters` takes conditions.
pub const KIDS_ALLOWED_B: &str = "kids_allowed(b.kids_override, b.content_rating)";

/// `alias` is the movies or tv_shows row in scope of the query the predicate goes into.
pub fn allowed_sql(alias: &str) -> String {
    format!("kids_allowed({alias}.kids_override, {alias}.content_rating)")
}

pub async fn movie_allowed(pool: &PgPool, movie_id: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar::<_, bool>(
        "SELECT kids_allowed(kids_override, content_rating) FROM movies WHERE id = $1",
    )
    .bind(movie_id)
    .fetch_optional(pool)
    .await
    .map(|allowed| allowed.unwrap_or(false))
}

pub async fn show_allowed(pool: &PgPool, show_id: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar::<_, bool>(
        "SELECT kids_allowed(kids_override, content_rating) FROM tv_shows WHERE id = $1",
    )
    .bind(show_id)
    .fetch_optional(pool)
    .await
    .map(|allowed| allowed.unwrap_or(false))
}

pub async fn episode_allowed(pool: &PgPool, episode_id: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar::<_, bool>(
        "SELECT kids_allowed(sh.kids_override, sh.content_rating)
         FROM tv_episodes e
         JOIN tv_seasons s ON s.id = e.season_id
         JOIN tv_shows sh ON sh.id = s.show_id
         WHERE e.id = $1",
    )
    .bind(episode_id)
    .fetch_optional(pool)
    .await
    .map(|allowed| allowed.unwrap_or(false))
}

pub async fn movie_file_allowed(pool: &PgPool, file_id: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar::<_, bool>(
        "SELECT kids_allowed(m.kids_override, m.content_rating)
         FROM movie_files f JOIN movies m ON m.id = f.movie_id
         WHERE f.id = $1",
    )
    .bind(file_id)
    .fetch_optional(pool)
    .await
    .map(|allowed| allowed.unwrap_or(false))
}

/// An episode file can hold several episodes; it plays only if every show it belongs to is allowed.
pub async fn episode_file_allowed(pool: &PgPool, file_id: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar::<_, bool>(
        "SELECT COALESCE(bool_and(kids_allowed(sh.kids_override, sh.content_rating)), FALSE)
         FROM tv_episode_file_episodes efe
         JOIN tv_episodes e ON e.id = efe.episode_id
         JOIN tv_seasons s ON s.id = e.season_id
         JOIN tv_shows sh ON sh.id = s.show_id
         WHERE efe.episode_file_id = $1",
    )
    .bind(file_id)
    .fetch_one(pool)
    .await
}

pub async fn allowed_movie_ids(pool: &PgPool, ids: &[Uuid]) -> sqlx::Result<HashSet<Uuid>> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM movies WHERE id = ANY($1) AND kids_allowed(kids_override, content_rating)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await
    .map(HashSet::from_iter)
}

pub async fn allowed_show_ids(pool: &PgPool, ids: &[Uuid]) -> sqlx::Result<HashSet<Uuid>> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM tv_shows WHERE id = ANY($1) AND kids_allowed(kids_override, content_rating)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await
    .map(HashSet::from_iter)
}

pub async fn allowed_episode_ids(pool: &PgPool, ids: &[Uuid]) -> sqlx::Result<HashSet<Uuid>> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT e.id FROM tv_episodes e
         JOIN tv_seasons s ON s.id = e.season_id
         JOIN tv_shows sh ON sh.id = s.show_id
         WHERE e.id = ANY($1) AND kids_allowed(sh.kids_override, sh.content_rating)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await
    .map(HashSet::from_iter)
}

/// A title outside the library has no rating we hold. It is fine for a Kids profile only
/// when the kids-filtered TMDB queries put it on one of their Discover shelves, or when it
/// is in the library under a TMDB id the rule allows.
pub async fn discover_title_allowed(
    pool: &PgPool,
    media_type: &str,
    tmdb_id: &str,
) -> sqlx::Result<bool> {
    let media_type = if media_type == "movie" { "movie" } else { "tv" };
    sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS (
               SELECT 1 FROM metadata_cache mc,
                    jsonb_array_elements(mc.response->'items') item
               WHERE mc.provider_type = 'discover' AND mc.expires_at > NOW()
                 AND COALESCE((mc.response->>'kids')::boolean, FALSE)
                 AND item->>'media_type' = $1
                 AND item->>'provider' = 'tmdb' AND item->>'provider_id' = $2
           ) OR EXISTS (
               SELECT 1 FROM movies
               WHERE $1 = 'movie' AND identifiers->>'tmdb_id' = $2
                 AND kids_allowed(kids_override, content_rating)
           ) OR EXISTS (
               SELECT 1 FROM tv_shows
               WHERE $1 = 'tv' AND identifiers->>'tmdb_id' = $2
                 AND kids_allowed(kids_override, content_rating)
           )"#,
    )
    .bind(media_type)
    .bind(tmdb_id)
    .fetch_one(pool)
    .await
}

/// Drops every hit a Kids profile may not open: movies and shows (and their seasons and
/// episodes) by the rule, and live channels and guide entries outright.
pub async fn filter_search(
    pool: &PgPool,
    response: &mut UnifiedSearchResponse,
) -> sqlx::Result<()> {
    let hits = response.top_hits.iter().chain(response.remaining.iter());
    let parse = |id: &str| Uuid::parse_str(id).ok();
    let mut movie_ids = Vec::new();
    let mut show_ids = Vec::new();
    for hit in hits {
        match hit {
            SearchHit::Movie(m) => movie_ids.extend(parse(&m.id)),
            SearchHit::TvShow(s) => show_ids.extend(parse(&s.id)),
            SearchHit::TvSeason(s) => show_ids.extend(parse(&s.show_id)),
            SearchHit::TvEpisode(e) => show_ids.extend(parse(&e.show_id)),
            _ => {}
        }
    }
    let movies = allowed_movie_ids(pool, &movie_ids).await?;
    let shows = allowed_show_ids(pool, &show_ids).await?;
    let keep = |hit: &SearchHit| match hit {
        SearchHit::Movie(m) => parse(&m.id).is_some_and(|id| movies.contains(&id)),
        SearchHit::TvShow(s) => parse(&s.id).is_some_and(|id| shows.contains(&id)),
        SearchHit::TvSeason(s) => parse(&s.show_id).is_some_and(|id| shows.contains(&id)),
        SearchHit::TvEpisode(e) => parse(&e.show_id).is_some_and(|id| shows.contains(&id)),
        SearchHit::IptvChannel(_)
        | SearchHit::IptvEpg(_)
        | SearchHit::LivetvChannel(_)
        | SearchHit::LivetvGuide(_) => false,
        _ => true,
    };
    response.top_hits.retain(keep);
    response.remaining.retain(keep);
    Ok(())
}

/// Books carry no rating and are open to Kids profiles; a notification about nothing in
/// particular is too. Anything else unknown is not.
pub async fn entity_allowed(
    pool: &PgPool,
    entity_type: Option<&str>,
    entity_id: Option<Uuid>,
) -> sqlx::Result<bool> {
    match (entity_type, entity_id) {
        (Some("movie"), Some(id)) => movie_allowed(pool, id).await,
        (Some("show"), Some(id)) => show_allowed(pool, id).await,
        (Some("episode"), Some(id)) => episode_allowed(pool, id).await,
        (Some("book"), _) | (None, None) => Ok(true),
        _ => Ok(false),
    }
}

/// Surfaces with no per-title rating to judge by: live channels, sports, chat, raw
/// provider metadata search (any title TMDB knows), and the library activity log. Prefixes, so every sub-route is covered.
const KIDS_BLOCKED_PREFIXES: &[&str] = &[
    "/api/v1/admin/livetv",
    "/api/v1/admin/iptv",
    "/api/v1/admin/chat",
    "/api/v1/chat",
    "/api/v1/admin/video-metadata",
    "/api/v1/library/activity",
    "/api/v1/admin/sports",
];

fn kids_blocked(path: &str) -> bool {
    KIDS_BLOCKED_PREFIXES.iter().any(|prefix| {
        path.strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

pub async fn deny_kids_surfaces(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if kids_blocked(request.uri().path()) {
        let token = extract_bearer(request.headers())
            .or_else(|| extract_session_cookie(request.headers()))
            .unwrap_or_default();
        if let Some((_, _, true)) = require_auth_token(&state, &token).await {
            return (StatusCode::FORBIDDEN, "not available on a Kids profile").into_response();
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_every_live_and_chat_route_and_nothing_else() {
        for path in [
            "/api/v1/admin/livetv/channels",
            "/api/v1/admin/iptv/channels/42/play",
            "/api/v1/admin/chat/completions",
            "/api/v1/chat/inbound",
            "/api/v1/admin/video-metadata/search/movies",
            "/api/v1/library/activity",
        ] {
            assert!(kids_blocked(path), "{path}");
        }
        for path in [
            "/api/v1/admin/movies",
            "/api/v1/admin/tv/shows",
            "/api/v1/admin/chatter",
            "/api/v1/admin/me",
        ] {
            assert!(!kids_blocked(path), "{path}");
        }
    }
}
