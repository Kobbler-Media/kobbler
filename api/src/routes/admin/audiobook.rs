//! Audiobook streaming and playback route handlers.
//!
//! Provides endpoints for:
//! - Serving audio files with HTTP Range support for seeking
//! - Returning chapter lists for audiobook files
//! - Getting and saving audiobook playback positions

use axum::{
    extract::{Json, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{error, warn};
use uuid::Uuid;

use crate::AppState;
use crate::audio::auto_chapters;
use crate::enrichment_config::stt_config_tag;
use crate::jobs::{self, JobContext};
use crate::metadata::audnexus;
use crate::reader_pack;
use crate::repositories::audiobook_chapters::ChapterInput;
use crate::repositories::{
    audiobook_bookmarks, audiobook_chapters, audiobook_files, audiobook_transcripts, books,
    cross_format_sync, devices, ebook_segments, entitlements, reading_states, user_book_data,
};
use crate::routes::{AuthUser, PlaybackAuth};

// =============================================================================
// Audio File Streaming
// =============================================================================

/// Serve an audio file with HTTP Range support for seeking.
pub async fn stream_audio(
    State(state): State<AppState>,
    _auth: PlaybackAuth,
    Path((book_id, file_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };
    let file_id = match Uuid::parse_str(&file_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid file id").into_response(),
    };

    // Get file info and verify it belongs to this book
    let file_info = match audiobook_files::get_by_id_for_book(&state.pool, file_id, book_id).await {
        Ok(Some(info)) => info,
        Ok(None) => return (StatusCode::NOT_FOUND, "audio file not found").into_response(),
        Err(e) => {
            error!(error = %e, "failed to look up audiobook file");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let storage_path = match &file_info.file_path {
        Some(p) => p.clone(),
        None => return (StatusCode::NOT_FOUND, "file has no storage path").into_response(),
    };

    let full_path =
        crate::audio::path::resolve_file_path(&state.config.storage_root, &storage_path);

    let file = match tokio::fs::File::open(&full_path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warn!(path = %full_path.display(), "audio file not found on disk");
            return (StatusCode::NOT_FOUND, "file not found on disk").into_response();
        }
        Err(e) => {
            error!(error = %e, "failed to open audio file");
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to read file").into_response();
        }
    };

    let file_size = match file.metadata().await {
        Ok(m) => m.len(),
        Err(e) => {
            error!(error = %e, "failed to get file metadata");
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to read file").into_response();
        }
    };

    let content_type = audio_content_type(&file_info.format);

    crate::streaming::byte_range::file_response(file, file_size, content_type, &headers).await
}

/// Map book file format to audio MIME type.
fn audio_content_type(format: &str) -> &'static str {
    match format.to_uppercase().as_str() {
        "M4B" | "M4A" => "audio/mp4",
        "MP3" => "audio/mpeg",
        "FLAC" => "audio/flac",
        "OGG" | "OPUS" => "audio/ogg",
        _ => "application/octet-stream",
    }
}

// =============================================================================
// Audio File List
// =============================================================================

#[derive(Serialize)]
pub struct AudioFileResponse {
    pub id: String,
    pub file_index: i32,
    pub format: String,
    pub file_name: String,
    pub file_size: i64,
    pub duration_seconds: Option<f64>,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intro_end_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outro_start_seconds: Option<f64>,
}

/// Return the list of audio files for a book.
pub async fn list_audio_files(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    match audiobook_files::list_by_book(&state.pool, book_id).await {
        Ok(files) => {
            let response: Vec<AudioFileResponse> = files
                .into_iter()
                .map(|f| {
                    let file_name = std::path::Path::new(&f.file_path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(&f.file_path)
                        .to_string();
                    AudioFileResponse {
                        id: f.id.to_string(),
                        file_index: f.file_index,
                        format: f.format,
                        file_name,
                        file_size: f.file_size,
                        duration_seconds: f.duration_seconds,
                        available: f.available,
                        intro_end_seconds: f.intro_end_seconds,
                        outro_start_seconds: f.outro_start_seconds,
                    }
                })
                .collect();
            Json(response).into_response()
        }
        Err(e) => {
            error!(error = %e, "failed to list audio files");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

// =============================================================================
// Chapter List
// =============================================================================

#[derive(Serialize)]
pub struct ChapterResponse {
    pub id: String,
    pub chapter_index: i32,
    pub title: Option<String>,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub source: String,
}

/// Return the chapter list for an audiobook file.
pub async fn get_chapters(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path((book_id, file_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };
    let file_id = match Uuid::parse_str(&file_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid file id").into_response(),
    };

    // Verify file belongs to this book
    match audiobook_files::get_by_id_for_book(&state.pool, file_id, book_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, "audio file not found").into_response(),
        Err(e) => {
            error!(error = %e, "failed to look up audiobook file");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    }

    match audiobook_chapters::get_chapters(&state.pool, file_id).await {
        Ok(chapters) => {
            let response: Vec<ChapterResponse> = chapters
                .into_iter()
                .map(|c| ChapterResponse {
                    id: c.id.to_string(),
                    chapter_index: c.chapter_index,
                    title: c.title,
                    start_seconds: c.start_seconds,
                    end_seconds: c.end_seconds,
                    source: c.source,
                })
                .collect();
            Json(response).into_response()
        }
        Err(e) => {
            error!(error = %e, "failed to get chapters");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

// =============================================================================
// Playback Position
// =============================================================================

#[derive(Serialize)]
pub struct AudioPositionResponse {
    pub file_id: Option<String>,
    pub position_seconds: Option<f64>,
    pub progress_percent: Option<f64>,
}

/// Get saved audiobook playback position.
pub async fn get_audio_position(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    // Find web reader device (reuse for audiobook playback)
    let device_id = match devices::get_web_reader_for_user(&state.pool, auth.user_id).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            return Json(AudioPositionResponse {
                file_id: None,
                position_seconds: None,
                progress_percent: None,
            })
            .into_response();
        }
        Err(e) => {
            error!(error = %e, "failed to get web reader device");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Get reading state for this device + book
    let state_id = match reading_states::get_id(&state.pool, auth.user_id, device_id, book_id).await
    {
        Ok(Some(id)) => id,
        Ok(None) => {
            return Json(AudioPositionResponse {
                file_id: None,
                position_seconds: None,
                progress_percent: None,
            })
            .into_response();
        }
        Err(e) => {
            error!(error = %e, "failed to get reading state");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    match reading_states::get_full_by_id(&state.pool, state_id, book_id).await {
        Ok(Some(row)) => {
            if row.location_type.as_deref() == Some("audiobook") {
                // location_value stores "file_id:seconds" for audiobook positions
                let (file_id, position) = parse_audio_location(row.location_value.as_deref());
                Json(AudioPositionResponse {
                    file_id,
                    position_seconds: position,
                    progress_percent: row.progress_percent,
                })
                .into_response()
            } else {
                Json(AudioPositionResponse {
                    file_id: None,
                    position_seconds: None,
                    progress_percent: None,
                })
                .into_response()
            }
        }
        Ok(None) => Json(AudioPositionResponse {
            file_id: None,
            position_seconds: None,
            progress_percent: None,
        })
        .into_response(),
        Err(e) => {
            error!(error = %e, "failed to get reading state");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// Parse "file_id:seconds" from location_value.
fn parse_audio_location(value: Option<&str>) -> (Option<String>, Option<f64>) {
    match value {
        Some(v) if v.contains(':') => {
            let (file_id, seconds_str) = v.split_once(':').unwrap();
            let seconds = seconds_str.parse::<f64>().ok();
            (Some(file_id.to_string()), seconds)
        }
        Some(v) => {
            // Legacy: just seconds
            let seconds = v.parse::<f64>().ok();
            (None, seconds)
        }
        None => (None, None),
    }
}

#[derive(Deserialize)]
pub struct SaveAudioPositionRequest {
    pub file_id: Uuid,
    pub position_seconds: f64,
    pub progress_percent: f64,
}

/// Save audiobook playback position.
pub async fn save_audio_position(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Path(book_id): Path<String>,
    Json(payload): Json<SaveAudioPositionRequest>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    // Get or create web reader device (reused for audiobook playback)
    let device_id = match devices::get_or_create_web_reader(&state.pool, auth.user_id).await {
        Ok(id) => id,
        Err(e) => {
            error!(error = %e, "failed to get/create web reader device");
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
                error!(error = %e, "failed to upsert reading state");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }
        };

    let percent = payload.progress_percent.clamp(0.0, 100.0);
    let location_value = format!("{}:{}", payload.file_id, payload.position_seconds);

    // Update bookmark with audiobook position
    if let Err(e) = reading_states::update_bookmark(
        &state.pool,
        state_id,
        Some(percent),
        None,
        Some(&location_value),
        Some("audiobook"),
        Some("web-player"),
    )
    .await
    {
        error!(error = %e, "failed to update bookmark");
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to save position").into_response();
    }

    // Update reading status based on progress
    let new_status = if percent >= 95.0 {
        "Finished"
    } else if percent > 0.0 {
        "Reading"
    } else {
        "ReadyToRead"
    };
    let _ = reading_states::update_status(&state.pool, state_id, new_status).await;
    let _ = reading_states::touch(&state.pool, state_id).await;

    // Propagate status to user_book_data (drives dashboard "in progress" and "finished" states)
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

    StatusCode::NO_CONTENT.into_response()
}

// =============================================================================
// Unified Chapter Timeline
// =============================================================================

#[derive(Serialize)]
pub struct UnifiedChapterResponse {
    pub id: String,
    pub chapter_index: i32,
    pub title: Option<String>,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub file_id: String,
    pub file_path: String,
    pub global_start_seconds: f64,
    pub global_end_seconds: f64,
    pub source: String,
}

/// Build the unified chapter list for a book: project per-file
/// `start_seconds` onto a book-wide timeline, renumber `chapter_index`
/// globally, and number the silence detector's per-file "Chapter N" titles
/// book-wide. Every other source's titles are the book's own and are kept,
/// even when they read "Chapter 7".
///
/// A file without chapters of its own continues the chapter before it, so
/// that chapter's book-wide end runs on to wherever the next one starts.
fn build_unified_chapters(
    chapters: &[crate::repositories::audiobook_chapters::ChapterWithFileInfo],
) -> Vec<UnifiedChapterResponse> {
    let mut result: Vec<UnifiedChapterResponse> = chapters
        .iter()
        .enumerate()
        .map(|(i, ch)| {
            let global_end = ch.file_offset_seconds + ch.end_seconds;
            let global_end_seconds = match chapters.get(i + 1) {
                Some(next) if next.audiobook_file_id == ch.audiobook_file_id => global_end,
                Some(next) => global_end.max(next.file_offset_seconds + next.start_seconds),
                None => global_end.max(ch.book_duration_seconds),
            };
            UnifiedChapterResponse {
                id: ch.id.to_string(),
                chapter_index: ch.chapter_index,
                title: ch.title.clone(),
                start_seconds: ch.start_seconds,
                end_seconds: ch.end_seconds,
                file_id: ch.audiobook_file_id.to_string(),
                file_path: ch.file_path.clone(),
                global_start_seconds: ch.file_offset_seconds + ch.start_seconds,
                global_end_seconds,
                source: ch.source.clone(),
            }
        })
        .collect();

    for (i, ch) in result.iter_mut().enumerate() {
        ch.chapter_index = i as i32;
        if ch.title.is_none() || ch.source == "detected" {
            ch.title = Some(format!("Chapter {}", i + 1));
        }
    }

    result
}

/// Get all chapters for a book with global timestamps computed from file durations.
pub async fn get_unified_chapters(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    match audiobook_chapters::get_all_for_book(&state.pool, book_id).await {
        Ok(chapters) => Json(build_unified_chapters(&chapters)).into_response(),
        Err(e) => {
            error!(error = %e, "failed to get unified chapters");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

// =============================================================================
// Chapter CRUD
// =============================================================================

#[derive(Deserialize)]
pub struct ChapterInputRequest {
    pub chapter_index: i32,
    pub title: Option<String>,
    pub start_seconds: f64,
    pub end_seconds: f64,
}

/// Replace all chapters for a specific audio file.
pub async fn replace_file_chapters(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path((book_id, file_id)): Path<(String, String)>,
    Json(payload): Json<Vec<ChapterInputRequest>>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };
    let file_id = match Uuid::parse_str(&file_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid file id").into_response(),
    };

    // Verify file belongs to book
    match audiobook_files::get_by_id_for_book(&state.pool, file_id, book_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, "audio file not found").into_response(),
        Err(e) => {
            error!(error = %e, "failed to look up audiobook file");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    }

    let chapters: Vec<ChapterInput> = payload
        .into_iter()
        .map(|c| ChapterInput {
            chapter_index: c.chapter_index,
            title: c.title,
            start_seconds: c.start_seconds,
            end_seconds: c.end_seconds,
            file_path: None,
            source: "embedded".to_string(),
        })
        .collect();

    match audiobook_chapters::replace_chapters(&state.pool, file_id, &chapters).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            error!(error = %e, "failed to replace chapters");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct UpdateChapterRequest {
    pub title: Option<String>,
    pub start_seconds: f64,
    pub end_seconds: f64,
}

/// Update a single chapter's title and timestamps.
pub async fn update_single_chapter(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path((_book_id, chapter_id)): Path<(String, String)>,
    Json(payload): Json<UpdateChapterRequest>,
) -> impl IntoResponse {
    let chapter_id = match Uuid::parse_str(&chapter_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid chapter id").into_response(),
    };

    match audiobook_chapters::update_chapter(
        &state.pool,
        chapter_id,
        payload.title.as_deref(),
        payload.start_seconds,
        payload.end_seconds,
    )
    .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            error!(error = %e, "failed to update chapter");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// Delete a single chapter by ID.
pub async fn delete_single_chapter(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path((_book_id, chapter_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let chapter_id = match Uuid::parse_str(&chapter_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid chapter id").into_response(),
    };

    match audiobook_chapters::delete_chapter(&state.pool, chapter_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "chapter not found").into_response(),
        Err(e) => {
            error!(error = %e, "failed to delete chapter");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

// =============================================================================
// Chapter Generation
// =============================================================================

#[derive(Deserialize)]
pub struct GenerateChaptersQuery {
    #[serde(default)]
    pub force: bool,
}

#[derive(Serialize)]
pub struct GenerateChaptersResponse {
    pub generated: usize,
}

/// Auto-generate one chapter per file for a multi-file audiobook.
pub async fn generate_chapters(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<String>,
    Query(query): Query<GenerateChaptersQuery>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    let files = match audiobook_files::list_by_book(&state.pool, book_id).await {
        Ok(f) => f,
        Err(e) => {
            error!(error = %e, "failed to list audio files");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let existing = match audiobook_chapters::get_all_for_book(&state.pool, book_id).await {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "failed to get existing chapters");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    if !query.force && !existing.is_empty() {
        return (StatusCode::CONFLICT, "Chapters already exist").into_response();
    }

    // If force, delete existing chapters for each file
    if query.force {
        for file in &files {
            if let Err(e) = audiobook_chapters::delete_chapters(&state.pool, file.id).await {
                error!(error = %e, file_id = %file.id, "failed to delete chapters");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }
        }
    }

    let file_data: Vec<(Uuid, String, Option<f64>)> = files
        .iter()
        .map(|f| (f.id, f.file_path.clone(), f.duration_seconds))
        .collect();

    let generated = auto_chapters::generate_file_per_chapter(&file_data);
    let mut total = 0usize;

    for (file_id, chapters) in &generated {
        if let Err(e) = audiobook_chapters::store_chapters(&state.pool, *file_id, chapters).await {
            error!(error = %e, "failed to insert generated chapters");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
        total += chapters.len();
    }

    Json(GenerateChaptersResponse { generated: total }).into_response()
}

// =============================================================================
// Audio Probe
// =============================================================================

/// Enqueue a job to probe audio files for a book.
pub async fn probe_audio_files(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    let display = crate::jobs::fetch_job_display(&state.pool, "book", book_id).await;
    let context = JobContext::for_entity("book", book_id, "Probing audiobook files")
        .with_display_opt(display);
    match jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "audiobook_probe",
        json!({"book_id": book_id}),
        context,
    )
    .await
    {
        Ok(_) => StatusCode::ACCEPTED.into_response(),
        Err(e) => {
            error!(error = %e, "failed to enqueue audiobook probe job");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to enqueue job").into_response()
        }
    }
}

// =============================================================================
// Fetch Chapters from Audnexus
// =============================================================================

#[derive(Deserialize)]
pub struct FetchChaptersRequest {
    pub asin: String,
}

/// Fetch chapters from Audnexus by ASIN, store ASIN in book identifiers,
/// split chapters across files, and return the unified chapter list.
pub async fn fetch_audnexus_chapters(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<String>,
    Json(payload): Json<FetchChaptersRequest>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    // Store ASIN in book identifiers
    if let Err(e) =
        books::update_identifiers(&state.pool, book_id, json!({"asin": payload.asin})).await
    {
        error!(error = %e, "failed to store ASIN in identifiers");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    // Fetch chapters from Audnexus
    let chapters: Vec<(String, f64, f64)> =
        match audnexus::fetch_chapters_by_asin(&payload.asin).await {
            Ok(ch) => ch,
            Err(e) => {
                error!(error = %e, asin = %payload.asin, "failed to fetch Audnexus chapters");
                return (StatusCode::BAD_GATEWAY, format!("Audnexus error: {e}")).into_response();
            }
        };

    if chapters.is_empty() {
        return (StatusCode::NOT_FOUND, "No chapters returned from Audnexus").into_response();
    }

    // Get audio files for splitting
    let files = match audiobook_files::list_by_book(&state.pool, book_id).await {
        Ok(f) => f,
        Err(e) => {
            error!(error = %e, "failed to list audio files");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    if files.is_empty() {
        return (StatusCode::NOT_FOUND, "No audio files for this book").into_response();
    }

    // Split and store chapters across files
    if files.len() == 1 {
        let inputs: Vec<ChapterInput> = chapters
            .iter()
            .enumerate()
            .map(|(i, (title, start, end))| ChapterInput {
                chapter_index: i as i32,
                title: if title.is_empty() {
                    None
                } else {
                    Some(title.clone())
                },
                start_seconds: *start,
                end_seconds: *end,
                file_path: None,
                source: "provider".to_string(),
            })
            .collect();
        if let Err(e) =
            audiobook_chapters::replace_chapters(&state.pool, files[0].id, &inputs).await
        {
            error!(error = %e, "failed to store chapters");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    } else {
        // Multi-file: split by duration boundaries
        let all_durations_known = files.iter().all(|f| f.duration_seconds.is_some());
        if !all_durations_known {
            return (
                StatusCode::CONFLICT,
                "Not all audio files have duration data. Run probe first.",
            )
                .into_response();
        }

        let mut cumulative_offset = 0.0;
        for file in &files {
            let file_duration = file.duration_seconds.unwrap_or(0.0);
            let file_end = cumulative_offset + file_duration;

            let mut file_chapters = Vec::new();
            for (title, start, end) in &chapters {
                if *start < file_end && *end > cumulative_offset {
                    let local_start = (*start - cumulative_offset).max(0.0);
                    let local_end = (*end - cumulative_offset).min(file_duration);
                    file_chapters.push(ChapterInput {
                        chapter_index: file_chapters.len() as i32,
                        title: if title.is_empty() {
                            None
                        } else {
                            Some(title.clone())
                        },
                        start_seconds: local_start,
                        end_seconds: local_end,
                        file_path: None,
                        source: "provider".to_string(),
                    });
                }
            }

            if let Err(e) =
                audiobook_chapters::replace_chapters(&state.pool, file.id, &file_chapters).await
            {
                error!(error = %e, file_id = %file.id, "failed to store split chapters");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }

            cumulative_offset = file_end;
        }
    }

    match audiobook_chapters::get_all_for_book(&state.pool, book_id).await {
        Ok(all_chapters) => Json(build_unified_chapters(&all_chapters)).into_response(),
        Err(e) => {
            error!(error = %e, "failed to get unified chapters after fetch");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

// =============================================================================
// Transcript & Cross-Format Sync
// =============================================================================

/// Check transcript availability for a book.
/// GET /api/v1/admin/books/{book_id}/transcript/status
pub async fn transcript_status(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    let has_audio = audiobook_transcripts::has_transcript(&state.pool, book_id)
        .await
        .unwrap_or(false);
    let has_ebook = ebook_segments::has_segments(&state.pool, book_id)
        .await
        .unwrap_or(false);
    let transcript_mode = state.runtime_config.get().await.transcript_mode.clone();

    Json(json!({
        "has_audio_transcript": has_audio,
        "has_ebook_segments": has_ebook,
        "transcript_mode": transcript_mode,
    }))
    .into_response()
}

/// Enqueue audiobook transcription job.
/// POST /api/v1/admin/books/{book_id}/transcribe
pub async fn transcribe_audiobook(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    let config = state.runtime_config.get().await;
    if config.transcript_mode == "off" {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": "Transcription is disabled (transcript_mode=off)"})),
        )
            .into_response();
    }

    let title = books::get_title(&state.pool, book_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();

    jobs::enqueue_audiobook_transcribe_manual(&state.pool, state.nats.publisher(), book_id, &title)
        .await;

    (StatusCode::ACCEPTED, Json(json!({"status": "enqueued"}))).into_response()
}

/// Enqueue ebook text extraction job.
/// POST /api/v1/admin/books/{book_id}/extract-text
pub async fn extract_ebook_text(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    let title = books::get_title(&state.pool, book_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();

    jobs::enqueue_ebook_extract(&state.pool, state.nats.publisher(), book_id, &title).await;

    (StatusCode::ACCEPTED, Json(json!({"status": "enqueued"}))).into_response()
}

/// Queue transcription for every audiobook without a transcript from the
/// current STT config, skipping books already queued. Unlike the backfill this
/// queues the whole library at once; the job still skips up-to-date files.
/// POST /api/v1/admin/books/bulk-transcribe
pub async fn bulk_transcribe(State(state): State<AppState>, _auth: AuthUser) -> impl IntoResponse {
    let config = state.runtime_config.get().await;
    if config.transcript_mode == "off" {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": "Transcription is disabled (transcript_mode=off)"})),
        )
            .into_response();
    }

    let tag = stt_config_tag(&config);
    let stale = match audiobook_transcripts::stale_books(&state.pool, &tag).await {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "Failed to query books for bulk transcribe");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let mut queued = 0u64;
    for book in &stale {
        if let Ok(true) = jobs::has_active_job(
            &state.pool,
            "audiobook_transcribe",
            "book_id",
            &book.id.to_string(),
        )
        .await
        {
            continue;
        }
        jobs::enqueue_audiobook_transcribe(
            &state.pool,
            state.nats.publisher(),
            book.id,
            &book.title,
        )
        .await;
        queued += 1;
    }

    (StatusCode::OK, Json(json!({"queued": queued}))).into_response()
}

/// Bulk chapter detection for all books with transcripts.
/// POST /api/v1/admin/books/bulk-chapter-detect
pub async fn bulk_chapter_detect(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> impl IntoResponse {
    // Find all books with transcript segments
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT af.book_id
        FROM audiobook_files af
        WHERE EXISTS (
            SELECT 1 FROM audiobook_transcripts at
            WHERE at.audiobook_file_id = af.id
        )
        "#
    )
    .fetch_all(&state.pool)
    .await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "Failed to query books for bulk chapter detect");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let mut queued = 0u64;
    for row in &rows {
        jobs::enqueue_chapter_detect(&state.pool, state.nats.publisher(), row.book_id, "bulk")
            .await;
        queued += 1;
    }

    (StatusCode::OK, Json(json!({"queued": queued}))).into_response()
}

/// Re-detect silence-based chapters for a single book.
/// POST /api/v1/admin/books/{book_id}/detect-chapters
pub async fn detect_silence_chapters_endpoint(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    let title = books::get_title(&state.pool, book_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| book_id.to_string());

    let display = crate::jobs::fetch_job_display(&state.pool, "book", book_id).await;
    let context = jobs::JobContext::for_entity(
        "book",
        book_id,
        format!("Re-detecting chapters for '{}'", title),
    )
    .with_display_opt(display);
    match jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "chapter_redetect",
        json!({"book_id": book_id}),
        context,
    )
    .await
    {
        Ok(_) => StatusCode::ACCEPTED.into_response(),
        Err(e) => {
            error!(error = %e, "failed to enqueue chapter_redetect job");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to enqueue job").into_response()
        }
    }
}

/// Detect text-cue chapters from transcript for a single book.
/// POST /api/v1/admin/books/{book_id}/detect-text-chapters
pub async fn detect_text_chapters_endpoint(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    let title = books::get_title(&state.pool, book_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| book_id.to_string());

    jobs::enqueue_chapter_detect(&state.pool, state.nats.publisher(), book_id, &title).await;

    StatusCode::ACCEPTED.into_response()
}

/// Bulk chapter re-detection for all books with detected chapters.
/// POST /api/v1/admin/books/bulk-redetect-chapters
pub async fn bulk_redetect_chapters(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> impl IntoResponse {
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT af.book_id, b.title
        FROM audiobook_files af
        JOIN books b ON b.id = af.book_id
        WHERE EXISTS (
            SELECT 1 FROM audiobook_chapters ac
            WHERE ac.audiobook_file_id = af.id AND ac.source = 'detected'
        )
        "#
    )
    .fetch_all(&state.pool)
    .await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "Failed to query books for bulk chapter redetect");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let mut queued = 0u64;
    for row in &rows {
        let title = row.title.as_str();
        jobs::enqueue_chapter_redetect(&state.pool, state.nats.publisher(), row.book_id, title)
            .await;
        queued += 1;
    }

    (StatusCode::OK, Json(json!({"queued": queued}))).into_response()
}

/// Bulk ebook text extraction for all books with ebook files but no segments.
/// POST /api/v1/admin/books/bulk-ebook-extract
pub async fn bulk_ebook_extract(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> impl IntoResponse {
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT bf.book_id, b.title
        FROM book_files bf
        JOIN books b ON b.id = bf.book_id
        WHERE NOT EXISTS (
            SELECT 1 FROM ebook_segments es
            WHERE es.book_file_id = bf.id
        )
        "#
    )
    .fetch_all(&state.pool)
    .await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "Failed to query books for bulk ebook extract");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let mut queued = 0u64;
    for row in &rows {
        let title = row.title.as_str();
        jobs::enqueue_ebook_extract(&state.pool, state.nats.publisher(), row.book_id, title).await;
        queued += 1;
    }

    (StatusCode::OK, Json(json!({"queued": queued}))).into_response()
}

#[derive(Debug, Default, Deserialize)]
pub struct RepairQuery {
    #[serde(default)]
    pub dry_run: bool,
}

/// Sweep every converted book file for damage and re-make the broken ones.
/// POST /api/v1/admin/books/repair-ebooks?dry_run=true
///
/// The sweep runs as a job because it reads every derived file off the media
/// root; the response only says it started. Results land on the job, which
/// lists each damaged file and what was wrong with it.
pub async fn repair_ebooks(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<RepairQuery>,
) -> impl IntoResponse {
    enqueue_repair_sweep(
        &state,
        "ebook_repair",
        query.dry_run,
        if query.dry_run {
            "Checking converted ebooks for damage"
        } else {
            "Repairing damaged converted ebooks"
        },
    )
    .await
}

/// File away every audiobook's other editions, then re-settle its chapters and
/// rewrite the ones that come out different. POST /api/v1/admin/books/repair-audiobook-chapters?dry_run=true
pub async fn repair_audiobook_chapters(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<RepairQuery>,
) -> impl IntoResponse {
    enqueue_repair_sweep(
        &state,
        "audiobook_chapter_repair",
        query.dry_run,
        if query.dry_run {
            "Checking audiobook editions and chapters"
        } else {
            "Repairing audiobook editions and chapters"
        },
    )
    .await
}

async fn enqueue_repair_sweep(
    state: &AppState,
    job_type: &str,
    dry_run: bool,
    description: &str,
) -> axum::response::Response {
    match jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        job_type,
        json!({ "dry_run": dry_run }),
        JobContext::new(description.to_string()),
    )
    .await
    {
        Ok(job_id) => (
            StatusCode::ACCEPTED,
            Json(json!({ "job_id": job_id, "dry_run": dry_run })),
        )
            .into_response(),
        Err(e) => {
            error!(error = %e, job_type, "Failed to enqueue repair sweep");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to queue repair").into_response()
        }
    }
}

/// Run the KEPUB catch-up now instead of waiting for its schedule.
/// POST /api/v1/admin/books/kepubify-synced
///
/// The work itself lives in the `kepub_check` job, which also runs every few
/// minutes — this only asks for it sooner. Doing it that way means one
/// implementation decides what needs converting, so a manual run and a
/// scheduled one can never disagree.
///
/// Two things follow from a book gaining a KEPUB. The device prefers it over
/// the EPUB, so it re-downloads the book — and a page whose whole body is one
/// image comes back scaled to fill the screen instead of sitting at its own
/// pixel size in the middle of the page, which is what makes maps and plates
/// legible. The original EPUB is left exactly where it is; this only adds.
pub async fn kepubify_synced(State(state): State<AppState>, _auth: AuthUser) -> impl IntoResponse {
    let outstanding = entitlements::list_synced_without_kepub(&state.pool)
        .await
        .map(|b| b.len())
        .unwrap_or(0);

    let context = JobContext::new("Checking synced books for a missing KEPUB".to_string());
    match jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "kepub_check",
        json!({}),
        context,
    )
    .await
    {
        Ok(job_id) => (
            StatusCode::ACCEPTED,
            Json(json!({ "job_id": job_id, "outstanding": outstanding })),
        )
            .into_response(),
        Err(e) => {
            error!(error = %e, "Failed to enqueue the KEPUB catch-up");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to queue").into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct TranscriptSearchQuery {
    pub q: String,
}

/// Search transcript segments for a book via MeiliSearch.
/// GET /api/v1/admin/books/{book_id}/transcript/search?q=...
pub async fn search_transcript(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<String>,
    Query(query): Query<TranscriptSearchQuery>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    if query.q.trim().is_empty() {
        return Json(json!([])).into_response();
    }

    match state
        .search
        .search_transcript_segments(&query.q, Some(book_id), 40)
        .await
    {
        Ok(results) => Json(truncate_transcript_results(results, 20)).into_response(),
        Err(e) => {
            error!(error = %e, "Transcript search failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Search failed"})),
            )
                .into_response()
        }
    }
}

/// Search transcripts across all books.
/// GET /api/v1/admin/search/transcripts?q=...
pub async fn search_all_transcripts(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<TranscriptSearchQuery>,
) -> impl IntoResponse {
    if query.q.trim().is_empty() {
        return Json(json!([])).into_response();
    }

    match state
        .search
        .search_transcript_segments(&query.q, None, 60)
        .await
    {
        Ok(results) => Json(truncate_transcript_results(results, 30)).into_response(),
        Err(e) => {
            error!(error = %e, "Transcript search failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Search failed"})),
            )
                .into_response()
        }
    }
}

/// Truncate transcript search results to the given limit.
fn truncate_transcript_results(
    mut results: Vec<crate::search::TranscriptSegmentDocument>,
    limit: usize,
) -> Vec<crate::search::TranscriptSegmentDocument> {
    results.truncate(limit);
    results
}

#[derive(Debug, Deserialize)]
pub struct SyncPositionQuery {
    /// "audio" or "ebook"
    pub from: String,
    /// Audio timestamp in seconds (when from=audio)
    pub seconds: Option<f64>,
    /// Ebook CFI (when from=ebook)
    pub cfi: Option<String>,
    /// The audio file `seconds` is in (when from=audio)
    pub file_id: Option<Uuid>,
}

/// Get cross-format sync position.
/// GET /api/v1/admin/books/{book_id}/sync-position?from=audio&seconds=1234
/// GET /api/v1/admin/books/{book_id}/sync-position?from=ebook&cfi=...
pub async fn get_sync_position(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<String>,
    Query(query): Query<SyncPositionQuery>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    match query.from.as_str() {
        "audio" => {
            let seconds = match query.seconds {
                Some(s) => s,
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"error": "seconds required when from=audio"})),
                    )
                        .into_response();
                }
            };
            match cross_format_sync::find_ebook_position(
                &state.pool,
                book_id,
                seconds,
                query.file_id,
            )
            .await
            {
                Ok(Some(sp)) => Json(json!({
                    "cfi": sp.ebook_cfi,
                    "file_id": sp.ebook_file_id.to_string(),
                    "confidence": sp.confidence,
                    "text_snippet": "",
                }))
                .into_response(),
                Ok(None) => (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error": "No sync point found"})),
                )
                    .into_response(),
                Err(e) => {
                    error!(error = %e, "Sync position lookup failed");
                    (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
                }
            }
        }
        "ebook" => {
            let cfi = match &query.cfi {
                Some(c) => c,
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"error": "cfi required when from=ebook"})),
                    )
                        .into_response();
                }
            };
            match nearest_sync_point_before(&state, book_id, cfi).await {
                Ok(Some(sp)) => Json(json!({
                    "seconds": sp.audio_seconds,
                    "file_id": sp.audio_file_id.to_string(),
                    "confidence": sp.confidence,
                    "text_snippet": "",
                }))
                .into_response(),
                Ok(None) => (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error": "No sync point found"})),
                )
                    .into_response(),
                Err(e) => {
                    error!(error = %e, "Sync position lookup failed");
                    (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
                }
            }
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "from must be 'audio' or 'ebook'"})),
        )
            .into_response(),
    }
}

/// The last sync point at or before `cfi` in reading order. CFIs do not sort
/// as strings (`/6/10` is after `/6/2`), so both sides are placed in the
/// book's reader pack and compared there.
async fn nearest_sync_point_before(
    state: &AppState,
    book_id: Uuid,
    cfi: &str,
) -> crate::repositories::Result<Option<cross_format_sync::SyncPoint>> {
    let points = cross_format_sync::get_for_book(&state.pool, book_id).await?;
    let Some(pack) = reader_pack::for_book(state, book_id).await else {
        return Ok(None);
    };
    let Some(target) = reader_pack::position::from_cfi(&pack, cfi) else {
        return Ok(None);
    };
    let target = reader_pack::position::order(&target);
    Ok(points
        .into_iter()
        .filter_map(|sp| {
            let at = reader_pack::position::order(&reader_pack::position::from_cfi(
                &pack,
                &sp.ebook_cfi,
            )?);
            (at <= target).then_some((at, sp))
        })
        .max_by_key(|(at, _)| *at)
        .map(|(_, sp)| sp))
}

// =============================================================================
// Transcript Segments & Read-Along Data
// =============================================================================

/// GET /api/v1/admin/books/{book_id}/transcript/segments
///
/// Returns all transcript segments for a book, ordered by file_index + segment_index.
/// Used by the listen page subtitle mode.
pub async fn get_transcript_segments(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    match audiobook_transcripts::get_for_book(&state.pool, book_id).await {
        Ok(segments) => {
            let result: Vec<serde_json::Value> = segments
                .iter()
                .map(|s| {
                    json!({
                        "id": s.id,
                        "file_id": s.audiobook_file_id,
                        "segment_index": s.segment_index,
                        "start_seconds": s.start_seconds,
                        "end_seconds": s.end_seconds,
                        "text": s.text,
                    })
                })
                .collect();
            Json(result).into_response()
        }
        Err(e) => {
            error!(error = %e, "Failed to get transcript segments");
            (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
        }
    }
}

/// GET /api/v1/admin/books/{book_id}/read-along-data
///
/// Returns all data needed for audiobook read-along mode in one request:
/// cross-format sync points, transcript segments, and audio file IDs.
pub async fn get_read_along_data(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<String>,
) -> impl IntoResponse {
    let book_id = match Uuid::parse_str(&book_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid book id").into_response(),
    };

    let sync_points = match cross_format_sync::get_for_book(&state.pool, book_id).await {
        Ok(sp) => sp,
        Err(e) => {
            error!(error = %e, "Failed to get sync points");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let transcript_segments = match audiobook_transcripts::get_for_book(&state.pool, book_id).await
    {
        Ok(ts) => ts,
        Err(e) => {
            error!(error = %e, "Failed to get transcript segments");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let audio_files = match audiobook_files::list_by_book(&state.pool, book_id).await {
        Ok(af) => af,
        Err(e) => {
            error!(error = %e, "Failed to get audio files");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let has_ebook_segments: bool = ebook_segments::has_segments(&state.pool, book_id)
        .await
        .unwrap_or_default();

    let pack = if sync_points.is_empty() {
        None
    } else {
        reader_pack::for_book(&state, book_id).await
    };

    Json(json!({
        "sync_points": sync_points.iter().map(|sp| json!({
            "audio_file_id": sp.audio_file_id,
            "audio_seconds": sp.audio_seconds,
            "ebook_file_id": sp.ebook_file_id,
            "ebook_cfi": sp.ebook_cfi,
            "position": pack.as_deref().and_then(|p| reader_pack::position::from_cfi(p, &sp.ebook_cfi)),
            "confidence": sp.confidence,
        })).collect::<Vec<_>>(),
        "transcript_segments": transcript_segments.iter().map(|s| json!({
            "id": s.id,
            "file_id": s.audiobook_file_id,
            "segment_index": s.segment_index,
            "start_seconds": s.start_seconds,
            "end_seconds": s.end_seconds,
            "text": s.text,
        })).collect::<Vec<_>>(),
        "audio_file_ids": audio_files.iter().map(|f| f.id.to_string()).collect::<Vec<_>>(),
        "has_ebook_segments": has_ebook_segments,
    }))
    .into_response()
}

// =============================================================================
// Audio Peaks
// =============================================================================

/// Serve pre-generated audio peaks for waveform visualization.
pub async fn serve_audio_peaks(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path((book_id, file_id)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    // Verify the file belongs to this book
    let file = audiobook_files::get_by_id_for_book(&state.pool, file_id, book_id).await;
    if file.is_err() || file.as_ref().unwrap().is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let peaks_path = std::path::PathBuf::from(&state.config.storage_root)
        .join("audio_peaks")
        .join(format!("{file_id}.peaks"));

    match tokio::fs::read(&peaks_path).await {
        Ok(data) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/octet-stream"),
                (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            ],
            data,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

// =============================================================================
// Audiobook Bookmarks
// =============================================================================

#[derive(Deserialize)]
pub struct CreateBookmarkRequest {
    file_id: Option<Uuid>,
    position_seconds: f64,
    note: Option<String>,
}

pub async fn create_bookmark(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<Uuid>,
    Json(req): Json<CreateBookmarkRequest>,
) -> impl IntoResponse {
    match audiobook_bookmarks::create(
        &state.pool,
        auth.user_id,
        book_id,
        req.file_id,
        req.position_seconds,
        req.note.as_deref(),
    )
    .await
    {
        Ok(id) => (StatusCode::CREATED, Json(json!({ "id": id }))).into_response(),
        Err(e) => {
            warn!(error = %e, "Failed to create bookmark");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn list_bookmarks(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(book_id): Path<Uuid>,
) -> impl IntoResponse {
    match audiobook_bookmarks::list_by_book(&state.pool, book_id, auth.user_id).await {
        Ok(bookmarks) => Json(bookmarks).into_response(),
        Err(e) => {
            warn!(error = %e, "Failed to list bookmarks");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn delete_bookmark(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((_book_id, bookmark_id)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    match audiobook_bookmarks::delete(&state.pool, bookmark_id, auth.user_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            warn!(error = %e, "Failed to delete bookmark");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
