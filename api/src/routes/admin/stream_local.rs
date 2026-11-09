//! GET /api/v1/admin/stream/local — a title's release that is already on disk, offered as
//! a stream option so playing it needs no search and no debrid host.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::AppState;
use crate::routes::PlaybackAuth;
use crate::routes::admin::debrid_open::{complete_cache_entry, release_key};

#[derive(Deserialize)]
pub struct LocalQuery {
    pub media_type: String,
    pub media_id: Uuid,
}

pub async fn local_release(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Query(q): Query<LocalQuery>,
) -> Response {
    // A Kids profile goes through the search, which decides what it may play.
    if auth.is_kids {
        return StatusCode::NOT_FOUND.into_response();
    }
    match local_option(&state, &q.media_type, q.media_id).await {
        Ok(Some(option)) => Json(option).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// The most recently played release of the title whose file is complete on disk, or is
/// being downloaded ahead, in the shape of a search result.
pub(crate) async fn local_option(
    state: &AppState,
    media_type: &str,
    media_id: Uuid,
) -> Result<Option<Value>, sqlx::Error> {
    let mut releases = sqlx::query!(
        r#"
        SELECT DISTINCT ON (info_hash, file_id)
               info_hash AS "info_hash!", file_id, release_title, filename, filesize, quality,
               debrid_provider, debrid_provider_id AS "debrid_provider_id!", updated_at
        FROM debrid_stream_sessions
        WHERE media_type = $1 AND media_id = $2
          AND info_hash IS NOT NULL AND debrid_provider_id IS NOT NULL
        ORDER BY info_hash, file_id, updated_at DESC
        "#,
        media_type,
        media_id,
    )
    .fetch_all(&state.pool)
    .await?;
    releases.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
    for r in releases {
        let downloading = release_key(Some(&r.info_hash), r.file_id)
            .is_some_and(|key| state.downloads_ahead.contains(&key));
        if !downloading
            && complete_cache_entry(state, r.debrid_provider_id, Some(&r.info_hash), r.file_id)
                .await
                .is_none()
        {
            continue;
        }
        let files = r
            .file_id
            .map(|id| vec![json!({ "id": id, "path": r.filename, "size": r.filesize })])
            .unwrap_or_default();
        return Ok(Some(json!({
            "id": format!("local-{}-{}", r.info_hash, r.file_id.unwrap_or(-1)),
            "release_title": r.release_title,
            "size": r.filesize,
            "indexer": "cache",
            "info_hash": r.info_hash,
            "quality": r.quality,
            "total_score": 1.0,
            "match_score": 1.0,
            "quality_score": 1.0,
            "debrid_provider": r.debrid_provider,
            "debrid_provider_id": r.debrid_provider_id,
            "cached": true,
            "source_type": "torrent",
            "availability": "ready",
            "files": files,
        })));
    }
    Ok(None)
}
