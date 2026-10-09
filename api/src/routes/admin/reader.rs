//! Web reader route handlers.
//!
//! Provides endpoints for the in-browser EPUB reader:
//! - Book file download for reading
//! - Reading progress save (auto-creates web reader device)

use axum::{
    body::Body,
    extract::{Json, Path, Query, State},
    http::{Response, StatusCode, header},
    response::IntoResponse,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::AppState;
use crate::convert::ConversionPipeline;
use crate::jobs::handlers::queue_sync_job_for_user;
use crate::repositories::{self, book_files, devices, reading_states, user_book_data};
use crate::routes::AuthUser;
use crate::sync_providers::SyncAction;

/// Query params for book file download.
#[derive(Deserialize)]
pub struct FileQuery {
    /// Requested format (e.g. "EPUB", "KEPUB"). Defaults to EPUB, falling back
    /// through the other readable formats.
    pub format: Option<String>,
}

/// Download a book file for the web reader.
pub async fn download_book_file(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<String>,
    Query(query): Query<FileQuery>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    // Determine format: explicit request, or try preferred order.
    //
    // EPUB before KEPUB. KEPUB is a Kobo artefact — koboSpans around every
    // sentence and the `div#book-columns` wrapper — and when both exist the
    // plain EPUB is the more faithful thing to hand a browser. It also keeps
    // the reader on the same document `ebook_segments` was extracted from, so
    // the CFIs that drive read-along still point where they say they do.
    // KEPUB stays in the list for books that only have one.
    let formats_to_try: Vec<String> = if let Some(fmt) = &query.format {
        vec![fmt.to_uppercase()]
    } else {
        vec![
            "EPUB", "KEPUB", "AZW3", "AZW", "MOBI", "CBZ", "CBR", "PDF", "FB2",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    };

    let mut file_path = None;
    let mut matched_format = String::new();
    for fmt in &formats_to_try {
        if let Ok(Some(path)) = book_files::get_path_by_format(&state.pool, book_id, fmt).await {
            // A row can outlive its file: the file was deleted from disk and
            // the row was not. Serving it would 404 deep in the open, so fall
            // through to the next format — or to the on-demand conversion,
            // which rebuilds the file and refreshes the row.
            let full = crate::audio::path::resolve_file_path(&state.config.storage_root, &path);
            if tokio::fs::try_exists(&full).await.unwrap_or(false) {
                file_path = Some(path);
                matched_format = fmt.clone();
                break;
            }
        }
    }

    // On-demand EPUB conversion: if no file found, try converting a source file to EPUB
    if file_path.is_none()
        && let Some((path, fmt)) = try_convert_to_epub(&state, book_id).await
    {
        file_path = Some(path);
        matched_format = fmt;
    }

    let file_path = match file_path {
        Some(p) => p,
        None => return (StatusCode::NOT_FOUND, "no readable file found").into_response(),
    };

    let full_path = crate::audio::path::resolve_file_path(&state.config.storage_root, &file_path);

    let file = match tokio::fs::File::open(&full_path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!(path = %full_path.display(), "book file not found on disk");
            return (StatusCode::NOT_FOUND, "file not found on disk").into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to open book file");
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to read file").into_response();
        }
    };

    let file_size = match file.metadata().await {
        Ok(m) => m.len(),
        Err(e) => {
            tracing::error!(error = %e, "failed to get file metadata");
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to read file").into_response();
        }
    };

    let content_type = match matched_format.as_str() {
        "EPUB" | "KEPUB" => "application/epub+zip",
        "MOBI" | "AZW" | "AZW3" => "application/x-mobipocket-ebook",
        "PDF" => "application/pdf",
        "FB2" => "application/x-fictionbook+xml",
        "CBZ" => "application/vnd.comicbook+zip",
        "CBR" => "application/vnd.comicbook-rar",
        _ => "application/octet-stream",
    };

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, file_size)
        .header(header::CONTENT_DISPOSITION, "inline")
        .header(header::CACHE_CONTROL, "private, max-age=3600")
        .body(body)
        .unwrap_or_else(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to build response",
            )
                .into_response()
        })
}

/// Request body for saving reader progress.
#[derive(Deserialize)]
pub struct SaveProgressRequest {
    pub progress_percent: f64,
    pub cfi: Option<String>,
    pub minutes_since_last_save: Option<i32>,
    /// True when the reader is being closed (triggers reading time sync).
    #[serde(default)]
    pub closing: bool,
}

/// Save reading progress from the web reader.
/// Auto-creates web reader device on first use.
pub async fn save_reader_progress(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<String>,
    Json(payload): Json<SaveProgressRequest>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    // Verify book exists
    if !repositories::books::exists(&state.pool, book_id)
        .await
        .unwrap_or(false)
    {
        return (StatusCode::NOT_FOUND, "book not found").into_response();
    }

    // Get or create web reader device
    let device_id = match devices::get_or_create_web_reader(&state.pool, auth.user_id).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(error = %e, "failed to get/create web reader device");
            return (StatusCode::INTERNAL_SERVER_ERROR, "device error").into_response();
        }
    };

    // Upsert reading state
    let state_id =
        match reading_states::upsert_with_children(&state.pool, auth.user_id, device_id, book_id)
            .await
        {
            Ok(Some(id)) => id,
            Ok(None) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to create reading state",
                )
                    .into_response();
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to upsert reading state");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }
        };

    let percent = payload.progress_percent.clamp(0.0, 100.0);

    // Update bookmark with progress and CFI
    if let Err(e) = reading_states::update_bookmark(
        &state.pool,
        state_id,
        Some(percent),
        None,
        payload.cfi.as_deref(),
        payload.cfi.as_ref().map(|_| "cfi"),
        Some("web-reader"),
    )
    .await
    {
        tracing::error!(error = %e, "failed to update bookmark");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to save progress").into_response();
    }

    // Get old status before updating (for sync transition detection)
    let old_status = reading_states::get_status(&state.pool, state_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "ReadyToRead".to_string());

    // Update status based on progress
    let new_status = if percent >= 95.0 {
        "Finished"
    } else if percent > 0.0 {
        "Reading"
    } else {
        "ReadyToRead"
    };
    let _ = reading_states::update_status(&state.pool, state_id, new_status).await;
    let _ = reading_states::touch(&state.pool, state_id).await;

    // Add reading time if provided (increment, not replace)
    if let Some(minutes) = payload.minutes_since_last_save
        && minutes > 0
    {
        let _ = reading_states::add_reading_time_by_state(&state.pool, state_id, minutes).await;
    }

    // Update user_book_data status
    let user_status = if percent >= 95.0 {
        Some("finished")
    } else if percent > 0.0 {
        Some("reading")
    } else {
        None
    };
    if let Some(s) = user_status {
        let _ = user_book_data::update_reading_status(&state.pool, auth.user_id, book_id, Some(s))
            .await;
    }

    // Queue sync jobs for all enabled sync providers
    let enabled_providers =
        repositories::user_sync_providers::list_enabled_for_user(&state.pool, auth.user_id)
            .await
            .unwrap_or_default();

    for provider_config in enabled_providers {
        let provider_id = &provider_config.provider_type;
        let user_id = auth.user_id;

        // Status transitions
        if old_status == "ReadyToRead" && new_status == "Reading" {
            let _ = queue_sync_job_for_user(
                &state.pool,
                state.nats.publisher(),
                Some(user_id),
                device_id,
                book_id,
                provider_id,
                SyncAction::Started,
            )
            .await;
        }
        if new_status == "Finished" && old_status != "Finished" {
            let _ = queue_sync_job_for_user(
                &state.pool,
                state.nats.publisher(),
                Some(user_id),
                device_id,
                book_id,
                provider_id,
                SyncAction::Finished,
            )
            .await;
        }

        // Progress sync only on close to avoid hammering the API
        if payload.closing {
            let today = Utc::now().format("%Y-%m-%d").to_string();
            let page_count = repositories::books::get_page_count(&state.pool, book_id)
                .await
                .ok()
                .flatten();
            let _ = queue_sync_job_for_user(
                &state.pool,
                state.nats.publisher(),
                Some(user_id),
                device_id,
                book_id,
                provider_id,
                SyncAction::Progress {
                    percent,
                    page_count,
                    started_at: Some(today),
                },
            )
            .await;
        }
    }

    StatusCode::NO_CONTENT.into_response()
}

/// Response for getting reader progress.
#[derive(serde::Serialize)]
pub struct ReaderProgressResponse {
    pub progress_percent: Option<f64>,
    pub cfi: Option<String>,
}

/// Get saved reading progress for the web reader.
pub async fn get_reader_progress(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    // Find web reader device
    let device_id = match devices::get_web_reader_for_user(&state.pool, auth.user_id).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            return Json(ReaderProgressResponse {
                progress_percent: None,
                cfi: None,
            })
            .into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to get web reader device");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Get reading state for this device + book
    let state_id = match reading_states::get_id(&state.pool, auth.user_id, device_id, book_id).await
    {
        Ok(Some(id)) => id,
        Ok(None) => {
            return Json(ReaderProgressResponse {
                progress_percent: None,
                cfi: None,
            })
            .into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to get reading state");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Get bookmark data
    match reading_states::get_full_by_id(&state.pool, state_id, book_id).await {
        Ok(Some(row)) => Json(ReaderProgressResponse {
            progress_percent: row.progress_percent,
            cfi: row.location_value,
        })
        .into_response(),
        Ok(None) => Json(ReaderProgressResponse {
            progress_percent: None,
            cfi: None,
        })
        .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to get reading state");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// Formats that can be converted to EPUB via ebook-convert (Calibre).
const EPUB_CONVERTIBLE_FORMATS: &[&str] = &[
    "MOBI", "AZW", "AZW3", "FB2", "PDF", "DOCX", "LIT", "PDB", "RTF", "TXT", "HTML",
];

/// Attempt on-demand conversion of a source file to EPUB.
///
/// Returns `Some((path, "EPUB"))` on success, `None` if no convertible source
/// exists or conversion fails. The native pipeline (boko) handles the common
/// formats in milliseconds; the rare Calibre fallback (PDF/FB2/RTF/TXT/HTML/
/// DOCX/LIT/PDB) is covered by the client's HTTP timeout.
async fn try_convert_to_epub(state: &AppState, book_id: Uuid) -> Option<(String, String)> {
    let files = book_files::list_by_book(&state.pool, book_id).await.ok()?;
    // The first convertible source that is actually on disk. A row can point
    // at a file that has gone away; skipping it lets a second source save the
    // request instead of 404ing on the first ghost.
    let source = files
        .iter()
        .filter(|f| {
            EPUB_CONVERTIBLE_FORMATS
                .iter()
                .any(|fmt| f.format.eq_ignore_ascii_case(fmt))
        })
        .find(|f| {
            crate::audio::path::resolve_file_path(&state.config.storage_root, &f.file_path).exists()
        })?;

    let source_path =
        crate::audio::path::resolve_file_path(&state.config.storage_root, &source.file_path);

    // The staleness baseline for the repair sweep: what the source looked
    // like when this copy was made.
    let source_mtime = tokio::fs::metadata(&source_path)
        .await
        .ok()
        .and_then(|m| m.modified().ok())
        .map(crate::repositories::media_root_files::mtime_from_system_time);

    let output_dir = std::path::Path::new(&state.config.storage_root)
        .join("books")
        .join(book_id.to_string());

    let pipeline = ConversionPipeline::new();
    let converted_path = match pipeline.convert_to_epub(&source_path, &output_dir).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, book_id = %book_id, "on-demand EPUB conversion failed");
            return None;
        }
    };

    let file_size = tokio::fs::metadata(&converted_path)
        .await
        .map(|m| m.len() as i64)
        .unwrap_or(0);

    let path_str = converted_path.to_string_lossy().to_string();

    let row_id = match book_files::upsert_full(
        &state.pool,
        book_files::BookFileFullInput {
            book_id,
            format: "EPUB".to_string(),
            size_bytes: file_size,
            file_path: path_str.clone(),
            source_path: Some(source.file_path.clone()),
            source_mtime,
            file_hash: None,
            source_type: Some("on_demand_conversion".to_string()),
            source_metadata: Some(json!({ "source_format": source.format })),
        },
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(error = %e, book_id = %book_id, "failed to register converted EPUB");
            return None;
        }
    };

    // The file was just written; if the row was a ghost marked unavailable,
    // that state is stale now rather than waiting for the next availability
    // pass.
    let _ = book_files::set_available(&state.pool, row_id).await;

    tracing::info!(
        book_id = %book_id,
        source_format = %source.format,
        "on-demand EPUB conversion succeeded"
    );
    Some((path_str, "EPUB".to_string()))
}
