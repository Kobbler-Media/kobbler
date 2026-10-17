//! Only admins and contributors change the library. Enforced here by path rather than in
//! each handler, so a route added later is covered without anyone remembering to.

use axum::{
    body::Body,
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::AppState;
use crate::routes::{extract_bearer, extract_session_cookie, require_auth_token};

const LIBRARY_PREFIXES: &[&str] = &[
    "/api/v1/admin/movies",
    "/api/v1/admin/tv",
    "/api/v1/admin/sports",
];

/// Writes that belong to the viewer, not the library: what they watched, where they
/// stopped, how they rated it, and the subtitle a player fetches mid-playback.
const OWN_SUFFIXES: &[&str] = &[
    "/watch-state",
    "/watch-state/position",
    "/rating",
    "/subtitles/search",
];

/// Mixes the viewer's own watch marks with library edits; the handler checks per action.
const PER_ACTION_PATHS: &[&str] = &["/api/v1/admin/tv/episodes/bulk"];

fn needs_editor(method: &Method, path: &str) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return false;
    }
    let in_library = LIBRARY_PREFIXES.iter().any(|prefix| {
        path.strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    });
    in_library
        && !OWN_SUFFIXES.iter().any(|suffix| path.ends_with(suffix))
        && !PER_ACTION_PATHS.contains(&path)
}

pub async fn require_editor(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if needs_editor(request.method(), request.uri().path()) {
        let token = extract_bearer(request.headers())
            .or_else(|| extract_session_cookie(request.headers()))
            .unwrap_or_default();
        match require_auth_token(&state, &token).await {
            Some((_, role, _)) if role == "admin" || role == "contributor" => {}
            Some(_) => {
                return (
                    StatusCode::FORBIDDEN,
                    "only admins and contributors edit the library",
                )
                    .into_response();
            }
            None => return StatusCode::UNAUTHORIZED.into_response(),
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_writes_need_an_editor_and_a_viewers_own_do_not() {
        for (method, path, expected) in [
            (Method::PATCH, "/api/v1/admin/movies/1", true),
            (Method::DELETE, "/api/v1/admin/tv/shows/1", true),
            (Method::POST, "/api/v1/admin/sports/leagues", true),
            (Method::POST, "/api/v1/admin/movies/1/want", true),
            (Method::GET, "/api/v1/admin/movies/1", false),
            (Method::PUT, "/api/v1/admin/movies/1/watch-state", false),
            (
                Method::PATCH,
                "/api/v1/admin/tv/episodes/1/watch-state/position",
                false,
            ),
            (Method::PUT, "/api/v1/admin/tv/shows/1/rating", false),
            (
                Method::POST,
                "/api/v1/admin/movies/1/subtitles/search",
                false,
            ),
            (Method::POST, "/api/v1/admin/tv/episodes/bulk", false),
            (Method::POST, "/api/v1/admin/tvx", false),
        ] {
            assert_eq!(needs_editor(&method, path), expected, "{method} {path}");
        }
    }
}
