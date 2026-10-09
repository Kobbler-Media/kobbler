//! Audiobook listing endpoints for koblem's LibraryProvider.
//!
//! Shapes match `koblem_client::types::AudiobookSummary` / `AudiobookDetail`.
//! Koblem reaches these via its `HttpLibraryProvider` to populate the
//! "clone voice from audiobook" and "STT a slice of audiobook" pickers.

use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{error, warn};
use uuid::Uuid;

use crate::AppState;
use crate::audio::clip;
use crate::repositories::audiobook_files;
use crate::routes::ProviderOrUserAuth;

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct AudiobookSummary {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub narrator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AudiobookChapter {
    pub index: u32,
    pub title: String,
    pub file_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct AudiobookDetail {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub narrator: Option<String>,
    pub chapters: Vec<AudiobookChapter>,
}

/// GET /api/v1/library/audiobooks?q=... — lean list for koblem pickers.
pub async fn search_audiobooks(
    State(state): State<AppState>,
    _auth: ProviderOrUserAuth,
    Query(params): Query<SearchQuery>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(50).min(200) as i64;
    let query = params.q.as_deref().unwrap_or("").trim().to_string();

    let pattern = if query.is_empty() {
        "%".to_string()
    } else {
        format!("%{}%", query)
    };

    let rows = sqlx::query!(
        r#"
        SELECT
            b.id,
            b.title,
            b.narrator,
            (
                SELECT a.name
                FROM book_authors ba
                JOIN authors a ON a.id = ba.author_id
                WHERE ba.book_id = b.id
                ORDER BY a.name ASC
                LIMIT 1
            ) AS author
        FROM books b
        WHERE EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)
          AND (b.title ILIKE $1 OR COALESCE(b.narrator, '') ILIKE $1)
        ORDER BY b.title
        LIMIT $2
        "#,
        pattern,
        limit,
    )
    .fetch_all(&state.pool)
    .await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "audiobooks search failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response();
        }
    };

    let result: Vec<AudiobookSummary> = rows
        .into_iter()
        .map(|row| AudiobookSummary {
            id: row.id.to_string(),
            title: row.title,
            author: row.author,
            narrator: row.narrator,
            cover_url: None,
        })
        .collect();

    Json(result).into_response()
}

/// GET /api/v1/library/audiobooks/{id} — chapters with file paths.
pub async fn get_audiobook(
    State(state): State<AppState>,
    _auth: ProviderOrUserAuth,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    let book_row = sqlx::query!(
        r#"
        SELECT
            b.title,
            b.narrator,
            (
                SELECT a.name
                FROM book_authors ba
                JOIN authors a ON a.id = ba.author_id
                WHERE ba.book_id = b.id
                ORDER BY a.name ASC
                LIMIT 1
            ) AS author
        FROM books b
        WHERE b.id = $1
        "#,
        book_id,
    )
    .fetch_optional(&state.pool)
    .await;

    let book_row = match book_row {
        Ok(Some(b)) => b,
        Ok(None) => return (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => {
            warn!(error = %e, "audiobook get failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response();
        }
    };

    let files = match audiobook_files::list_by_book(&state.pool, book_id).await {
        Ok(f) => f,
        Err(e) => {
            warn!(error = %e, "audiobook_files list failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response();
        }
    };

    let chapters = files
        .into_iter()
        .enumerate()
        .map(|(i, f)| AudiobookChapter {
            index: u32::try_from(i).unwrap_or(u32::MAX),
            title: format!("Chapter {}", f.file_index + 1),
            file_path: f.file_path,
            duration_seconds: f.duration_seconds,
        })
        .collect();

    Json(AudiobookDetail {
        id: book_id.to_string(),
        title: book_row.title,
        author: book_row.author,
        narrator: book_row.narrator,
        chapters,
    })
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct ClipQuery {
    pub chapter: u32,
    pub offset: f64,
    pub duration: f64,
}

/// GET /api/v1/library/audiobooks/{id}/clip?chapter=&offset=&duration=
///
/// Cut a window from the requested chapter and return it as a 16-bit mono WAV.
/// Used by koblem (via `LibraryProvider::fetch_audiobook_clip`) to obtain a
/// reference clip for TTS voice cloning. Mirrors `get_audiobook_clip` in the
/// admin TTS routes but uses the provider/library auth surface so koblem can
/// reach it with its bearer token.
pub async fn get_audiobook_clip(
    State(state): State<AppState>,
    _auth: ProviderOrUserAuth,
    Path(book_id): Path<String>,
    Query(q): Query<ClipQuery>,
) -> Response {
    if !q.offset.is_finite() || q.offset < 0.0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "offset must be a finite non-negative number"})),
        )
            .into_response();
    }
    if !q.duration.is_finite() || q.duration <= 0.0 || q.duration > clip::MAX_CLIP_SECS {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("duration must be in (0, {}]", clip::MAX_CLIP_SECS)})),
        )
            .into_response();
    }

    let book_uuid = match Uuid::parse_str(&book_id) {
        Ok(u) => u,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid book id"})),
            )
                .into_response();
        }
    };

    let files = match audiobook_files::list_by_book(&state.pool, book_uuid).await {
        Ok(f) => f,
        Err(e) => {
            warn!(error = %e, "audiobook_files list failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "db error"})),
            )
                .into_response();
        }
    };

    let file = match files.into_iter().nth(q.chapter as usize) {
        Some(f) => f,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "chapter index out of range"})),
            )
                .into_response();
        }
    };

    let abs_path =
        crate::audio::path::resolve_file_path(&state.config.storage_root, &file.file_path);
    if !abs_path.is_file() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "audiobook file missing on disk"})),
        )
            .into_response();
    }

    let dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => {
            error!(error = %e, "tempdir create failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "tempdir create failed"})),
            )
                .into_response();
        }
    };
    let tmp_path = dir.path().join("clip.wav");

    if let Err(e) =
        clip::cut_audiobook_segment(abs_path.clone(), tmp_path.clone(), q.offset, q.duration).await
    {
        error!(error = %e, "audiobook segment cut failed");
        let status = match e {
            clip::ClipError::Ffmpeg(_) => StatusCode::BAD_GATEWAY,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        return (status, Json(json!({"error": format!("cut failed: {e}")}))).into_response();
    }

    let bytes = match tokio::fs::read(&tmp_path).await {
        Ok(b) => b,
        Err(e) => {
            error!(error = %e, "read tempfile failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "read failed"})),
            )
                .into_response();
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "audio/wav")
        .header("cache-control", "no-store")
        .body(Body::from(bytes))
        .unwrap()
}
