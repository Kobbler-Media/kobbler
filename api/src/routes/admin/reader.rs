//! Web reader route handlers.
//!
//! Provides endpoints for the in-browser EPUB reader:
//! - Book file download for reading
//! - Reading progress save (auto-creates web reader device)

use crate::streaming::byte_range::file_stream;
use axum::{
    body::Body,
    extract::{Json, Path, Query, State},
    http::{Response, StatusCode, header},
    response::IntoResponse,
};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use crate::AppState;
use crate::jobs::handlers::queue_sync_job_for_user;
use crate::reader_pack;
use crate::repositories::{self, devices, reading_states, user_book_data};
use crate::routes::AuthUser;
use crate::sync_providers::SyncAction;
use kob_ebook::reader_pack::Position;

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

    let requested = query.format.as_deref().map(str::to_uppercase);
    let formats: Vec<&str> = match &requested {
        Some(fmt) => vec![fmt.as_str()],
        None => reader_pack::file::DOWNLOAD_FORMATS.to_vec(),
    };
    let Some(reader_file) = reader_pack::file::select(&state, book_id, &formats).await else {
        return (StatusCode::NOT_FOUND, "no readable file found").into_response();
    };
    let matched_format = reader_file.format;
    let full_path = reader_file.path;

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

    let stream = file_stream(file);
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
    /// The app's reader-pack position; translated to epubcfi when no cfi is sent.
    pub position: Option<Position>,
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

    let pack = if payload.cfi.is_some() || payload.position.is_some() {
        reader_pack::for_book(&state, book_id).await
    } else {
        None
    };
    let cfi = payload.cfi.clone().or_else(|| {
        let pack = pack.as_deref()?;
        reader_pack::position::to_cfi(pack, payload.position.as_ref()?)
    });

    // Update bookmark with progress and CFI
    if let Err(e) = reading_states::update_bookmark(
        &state.pool,
        state_id,
        Some(percent),
        None,
        cfi.as_deref(),
        cfi.as_ref().map(|_| "cfi"),
        Some("web-reader"),
    )
    .await
    {
        tracing::error!(error = %e, "failed to update bookmark");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to save progress").into_response();
    }

    let kobo = pack.as_deref().and_then(|pack| {
        let position = reader_pack::position::from_cfi(pack, cfi.as_deref()?)?;
        reader_pack::position::to_kobo(pack, &position)
    });
    if let Err(e) = reading_states::set_kobo_location(
        &state.pool,
        state_id,
        kobo.as_ref().map(|k| (k.span.as_str(), k.source.as_str())),
    )
    .await
    {
        tracing::warn!(error = %e, "failed to store Kobo location");
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
    /// The position in the app's reader pack, when asked for with `?position=true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
}

#[derive(Deserialize)]
pub struct ProgressQuery {
    #[serde(default)]
    pub position: bool,
}

/// The most recent reading position across all of the user's devices — web
/// reader, app and Kobo — as epubcfi (what foliate resumes from), plus the
/// app's pack position on request.
pub async fn get_reader_progress(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<String>,
    Query(query): Query<ProgressQuery>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    let rows = match reading_states::list_for_user_book(&state.pool, auth.user_id, book_id).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "failed to get reading state");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };
    let Some(latest) = rows
        .into_iter()
        .filter(|r| r.progress_percent.is_some() || r.location_value.is_some())
        .max_by_key(|r| r.bookmark_last_modified)
    else {
        return Json(ReaderProgressResponse {
            progress_percent: None,
            cfi: None,
            position: None,
        })
        .into_response();
    };

    let is_kobo = latest.location_type.as_deref() == Some("KoboSpan");
    let pack = if query.position || is_kobo {
        reader_pack::for_book(&state, book_id).await
    } else {
        None
    };
    let cfi = if is_kobo {
        pack.as_deref().and_then(|pack| {
            let position = reader_pack::position::from_kobo(
                pack,
                latest.location_source.as_deref()?,
                latest.location_value.as_deref()?,
            )?;
            reader_pack::position::to_cfi(pack, &position)
        })
    } else if latest.location_type.as_deref() == Some("cfi") {
        latest.location_value
    } else {
        None
    };
    let position = if query.position {
        pack.as_deref()
            .zip(cfi.as_deref())
            .and_then(|(pack, cfi)| reader_pack::position::from_cfi(pack, cfi))
    } else {
        None
    };

    Json(ReaderProgressResponse {
        progress_percent: latest.progress_percent,
        cfi,
        position,
    })
    .into_response()
}
