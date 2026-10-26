use super::placement;
use crate::AppState;
use crate::events::LibraryEvent;
use crate::file_link_mode::FileLinkMode;
use crate::media_root_naming::{self, NamingContext};
use crate::metadata::ProviderRegistry;
use crate::repositories::{
    audio_tracks, books, external_subtitles, ingest_queue, ingest_tokens, media_root_files,
    media_roots, movie_files, movies, subtitle_tracks, tv_episode_files, tv_episodes, tv_seasons,
    tv_shows, wanted_attempts,
};
use crate::runtime_config::RuntimeConfig;
use crate::watcher::{
    FileMode, IngestMode, IngestOptions, compute_content_hash, process_queue_item,
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use futures::StreamExt;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

/// Type alias — wire format is now the shared nats_jobs type.
pub type KobloadCallbackRequest = nats_jobs::DownloadCompletedEvent;
/// Type alias — wire format is now the shared nats_jobs type.
pub type KobloadFileInfo = nats_jobs::DownloadFileInfo;
/// Type alias — wire format is now the shared nats_jobs type.
pub type KobloadFileGroup = nats_jobs::FileGroupInfo;

/// Classifies per-file failures in the download/copy loop so the outer
/// handler can pick the right HTTP status (500 for us-failed, 502 for
/// kobload-failed) and surface the real reason instead of a generic message.
#[derive(Clone, Copy)]
enum FailureKind {
    /// Kobload-side failure: fetch error, non-2xx response, or mid-stream read error.
    Upstream,
    /// Local failure: disk create/write/rename/stat, DB insert, etc.
    Local,
}

/// Record the *first* failure seen in a per-file loop. Don't overwrite —
/// we want the original cause, not the last symptom.
fn note_failure(slot: &mut Option<(FailureKind, String)>, kind: FailureKind, reason: String) {
    if slot.is_none() {
        *slot = Some((kind, reason));
    }
}

/// Finish a downloaded file: flush it, commit it, and close it.
///
/// Call this after the last chunk and *before* the rename into the library.
/// None of the download loops used to do anything of the sort — the write
/// handle stayed open across the rename, the DB insert, and the whole
/// probe/extract/analyze chain, and was only closed when the enclosing loop
/// iteration ended, seconds after ingest had declared the file complete. That
/// cost us twice:
///
/// - The file was given its final library name and handed to readers while its
///   tail was still buffered. It worked only because ~2s happened to elapse
///   before anything read it; nothing enforced that ordering, and a crash in
///   the window leaves a correctly-named, incomplete file in the library.
/// - The media lives on a CIFS mount (`closetimeo=1`), where the server stamps
///   `LastWriteTime` at close. The mtime therefore jumped about a second *after*
///   ingest finished, `media_analyze` saw its (size, mtime) snapshot move
///   underneath a completed decode, and threw that decode away — on every
///   auto-ingested file.
///
/// `sync_all` rather than a bare close on purpose: network filesystems tend to
/// report write-back errors at commit time, and a download that could not be
/// committed is exactly the thing that must not be renamed into the library.
///
/// Dropping a `tokio::fs::File` only *schedules* the close on the blocking
/// pool, which is no ordering guarantee against the rename that follows, so
/// this takes the std handle and closes it on a blocking task it waits for.
async fn finalize_download(mut file: fs::File) -> std::io::Result<()> {
    file.flush().await?;
    let std_file = file.into_std().await;
    tokio::task::spawn_blocking(move || std_file.sync_all())
        .await
        .map_err(|e| std::io::Error::other(format!("close task panicked: {e}")))?
}

/// Build the HTTP status and body for an ingest handler when
/// `ingested_count == 0`. 502 only for genuine upstream (kobload) failures;
/// 500 when kobbler itself couldn't write/persist.
fn failure_response_parts(last_failure: Option<&(FailureKind, String)>) -> (StatusCode, String) {
    match last_failure {
        Some((FailureKind::Upstream, reason)) => (
            StatusCode::BAD_GATEWAY,
            format!("no files ingested (upstream): {reason}"),
        ),
        Some((FailureKind::Local, reason)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("no files ingested (local): {reason}"),
        ),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "no files ingested (local): unknown".to_string(),
        ),
    }
}

/// Supported ebook extensions for filtering downloaded files
pub const EBOOK_EXTENSIONS: &[&str] =
    &["epub", "kepub", "mobi", "azw", "azw3", "pdf", "cbz", "cbr"];

/// Supported video file extensions
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts", "m2ts",
];

/// Supported audiobook file extensions
pub const AUDIOBOOK_EXTENSIONS: &[&str] = &["m4b", "m4a", "mp3", "flac", "ogg", "opus"];

/// Supported subtitle file extensions
pub const SUBTITLE_EXTENSIONS: &[&str] = &["srt", "ass", "ssa", "sub", "idx", "vtt"];

/// Normalize freeform language strings to ISO 639-1 codes.
///
/// Handles ISO 639-2/B (eng), ISO 639-2/T (fra), full names (English), and
/// passes through already-structured strings (2-char codes, region codes like "en-US").
fn normalize_language(lang: &str) -> String {
    // Already structured (region codes like "en-US", "pt-BR", "zh-Hant", "es-419")
    if lang.contains('-') {
        return lang.to_string();
    }

    match lang.to_lowercase().as_str() {
        "en" | "eng" | "english" => "en".to_string(),
        "fr" | "fre" | "fra" | "french" => "fr".to_string(),
        "de" | "ger" | "deu" | "german" => "de".to_string(),
        "es" | "spa" | "spanish" => "es".to_string(),
        "it" | "ita" | "italian" => "it".to_string(),
        "ja" | "jpn" | "japanese" => "ja".to_string(),
        "ko" | "kor" | "korean" => "ko".to_string(),
        "zh" | "chi" | "zho" | "chinese" => "zh".to_string(),
        "pt" | "por" | "portuguese" => "pt".to_string(),
        "ru" | "rus" | "russian" => "ru".to_string(),
        "ar" | "ara" | "arabic" => "ar".to_string(),
        "nl" | "dut" | "nld" | "dutch" => "nl".to_string(),
        "pl" | "pol" | "polish" => "pl".to_string(),
        "sv" | "swe" | "swedish" => "sv".to_string(),
        "no" | "nor" | "nob" | "nno" | "norwegian" => "no".to_string(),
        "da" | "dan" | "danish" => "da".to_string(),
        "fi" | "fin" | "finnish" => "fi".to_string(),
        "cs" | "cze" | "ces" | "czech" => "cs".to_string(),
        "hu" | "hun" | "hungarian" => "hu".to_string(),
        "ro" | "rum" | "ron" | "romanian" => "ro".to_string(),
        "tr" | "tur" | "turkish" => "tr".to_string(),
        "el" | "gre" | "ell" | "greek" => "el".to_string(),
        "he" | "heb" | "hebrew" => "he".to_string(),
        "th" | "tha" | "thai" => "th".to_string(),
        "vi" | "vie" | "vietnamese" => "vi".to_string(),
        "uk" | "ukr" | "ukrainian" => "uk".to_string(),
        "bg" | "bul" | "bulgarian" => "bg".to_string(),
        "hr" | "hrv" | "croatian" => "hr".to_string(),
        "sk" | "slk" | "slo" | "slovak" => "sk".to_string(),
        _ => lang.to_string(),
    }
}

/// Internal typed metadata for Kobload ingest callbacks.
/// This is Kobbler's interpretation of the opaque metadata blob.
#[derive(Debug, Default, Deserialize)]
struct IngestMetadata {
    // TV
    show_id: Option<Uuid>,
    season_id: Option<Uuid>,
    episode_id: Option<Uuid>,
    episode_map: Option<HashMap<String, Uuid>>,
    source_type: Option<String>,
    show_title: Option<String>,
    // Movie
    movie_id: Option<Uuid>,
    // Book
    book_id: Option<Uuid>,
    // Common
    title: Option<String>,
    year: Option<serde_json::Value>,
    release_title: Option<String>,
    // Regrab intent, stamped by crate::file_link_mode::tag_metadata on the way out.
    // Field name must stay in step with file_link_mode::METADATA_KEY.
    additional_copy: Option<bool>,
    // Original metadata (from want request, preserved for sub-routing)
    original_metadata: Option<Box<IngestMetadata>>,
}

impl IngestMetadata {
    /// Resolve a field with fallback to original_metadata.
    /// Returns the top-level value if present, otherwise the value from original_metadata.
    fn resolve(&self) -> ResolvedMeta<'_> {
        ResolvedMeta {
            top: self,
            om: self.original_metadata.as_deref(),
        }
    }
}

/// Helper for resolving metadata fields with original_metadata fallback.
struct ResolvedMeta<'a> {
    top: &'a IngestMetadata,
    om: Option<&'a IngestMetadata>,
}

impl ResolvedMeta<'_> {
    fn source_type(&self) -> Option<&str> {
        self.top
            .source_type
            .as_deref()
            .or_else(|| self.om.and_then(|m| m.source_type.as_deref()))
    }

    fn show_id(&self) -> Option<Uuid> {
        self.top.show_id.or_else(|| self.om.and_then(|m| m.show_id))
    }

    fn season_id(&self) -> Option<Uuid> {
        self.top
            .season_id
            .or_else(|| self.om.and_then(|m| m.season_id))
    }

    fn episode_id(&self) -> Option<Uuid> {
        self.top
            .episode_id
            .or_else(|| self.om.and_then(|m| m.episode_id))
    }

    fn movie_id(&self) -> Option<Uuid> {
        self.top
            .movie_id
            .or_else(|| self.om.and_then(|m| m.movie_id))
    }

    fn book_id(&self) -> Option<Uuid> {
        self.top.book_id.or_else(|| self.om.and_then(|m| m.book_id))
    }

    fn episode_map(&self) -> HashMap<String, Uuid> {
        self.top
            .episode_map
            .clone()
            .or_else(|| self.om.and_then(|m| m.episode_map.clone()))
            .unwrap_or_default()
    }

    fn show_title(&self) -> Option<&str> {
        self.top
            .show_title
            .as_deref()
            .or_else(|| self.om.and_then(|m| m.show_title.as_deref()))
    }

    fn title(&self) -> Option<&str> {
        self.top
            .title
            .as_deref()
            .or_else(|| self.om.and_then(|m| m.title.as_deref()))
    }

    fn year(&self) -> Option<i64> {
        let val = self
            .top
            .year
            .as_ref()
            .or_else(|| self.om.and_then(|m| m.year.as_ref()));
        val.and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
    }

    fn release_title(&self) -> Option<&str> {
        self.top
            .release_title
            .as_deref()
            .or_else(|| self.om.and_then(|m| m.release_title.as_deref()))
    }

    /// Whether this download was asked for as an *extra* copy (a regrab) rather
    /// than a replacement. Absent — every scheduler, every RSS grab, every
    /// re-import — means replace, exactly as before.
    fn link_mode(&self) -> FileLinkMode {
        FileLinkMode::from_additional_copy(
            self.top
                .additional_copy
                .or_else(|| self.om.and_then(|m| m.additional_copy))
                .unwrap_or(false),
        )
    }
}

/// Check if a media root's path is accessible. If not, sets scan error with
/// backoff and returns a 503 response with the media_root_id for deferral.
pub(super) async fn check_ingest_media_root(
    state: &AppState,
    root: &media_roots::EnabledMediaRoot,
) -> Option<axum::response::Response> {
    let root_path = std::path::Path::new(&root.path).to_owned();
    let exists = tokio::task::spawn_blocking(move || root_path.exists())
        .await
        .unwrap_or(false);
    if exists {
        return None;
    }
    // Set scan error so the UI reflects the unavailability
    let next_retry = time::OffsetDateTime::now_utc()
        + time::Duration::minutes(std::cmp::min(60, 2_i64.pow(root.error_count as u32)));
    let _ = media_roots::set_scan_error(
        &state.pool,
        root.id,
        &format!("Directory not found: {}", root.path),
        next_retry,
    )
    .await;
    tracing::warn!(
        media_root_id = %root.id,
        path = %root.path,
        "Media root unavailable during ingest, deferring"
    );
    Some(
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"media_root_id": root.id, "error": "media_root_unavailable"})),
        )
            .into_response(),
    )
}

/// Parsed book metadata extracted from a `ParsedRelease` for book ingest.
#[derive(Debug)]
pub struct ParsedBookMedia {
    pub title: String,
    pub author: Option<String>,
    pub narrator: Option<String>,
    pub series: Option<String>,
    pub is_audiobook: bool,
}

/// Extract parsed book metadata from a typed `ParsedRelease`.
/// Returns None if the media type isn't book or audiobook.
fn extract_book_from_interpretation(
    release_interpretation: Option<&nats_jobs::ParsedRelease>,
) -> Option<ParsedBookMedia> {
    let interp = release_interpretation?;
    match &interp.media {
        nats_jobs::ParsedMedia::Book(book) => Some(ParsedBookMedia {
            title: book.title.clone(),
            author: book.author.clone(),
            narrator: None,
            series: book.series.clone(),
            is_audiobook: false,
        }),
        nats_jobs::ParsedMedia::Audiobook(ab) => Some(ParsedBookMedia {
            title: ab.title.clone(),
            author: ab.author.clone(),
            narrator: ab.narrator.clone(),
            series: ab.series.clone(),
            is_audiobook: true,
        }),
        _ => None,
    }
}

/// MIME types that indicate ebook content.
const EBOOK_MIME_TYPES: &[&str] = &[
    "application/epub+zip",
    "application/pdf",
    "application/x-mobipocket-ebook",
    "application/vnd.comicbook+zip",
    "application/vnd.comicbook-rar",
];

/// Extension trait adding kobbler-specific helpers to the shared `DownloadFileInfo`.
pub trait FileInfoExt {
    fn is_ebook(&self) -> bool;
    fn is_audiobook(&self) -> bool;
    fn is_video(&self) -> bool;
    fn is_subtitle(&self) -> bool;
    fn is_main_content(&self) -> bool;
    fn is_non_content(&self) -> bool;
    fn filename(&self) -> &str;
    fn normalized_name(&self) -> String;
}

impl FileInfoExt for KobloadFileInfo {
    /// Check if this file is an ebook based on extension or MIME type.
    fn is_ebook(&self) -> bool {
        if EBOOK_EXTENSIONS.contains(&self.extension.to_lowercase().as_str()) {
            return true;
        }
        if let Some(ref mime) = self.mime_type {
            return EBOOK_MIME_TYPES.contains(&mime.as_str());
        }
        false
    }

    /// Check if this file is an audiobook based on extension.
    fn is_audiobook(&self) -> bool {
        AUDIOBOOK_EXTENSIONS.contains(&self.extension.to_lowercase().as_str())
    }

    /// Check if this file is a video based on extension.
    fn is_video(&self) -> bool {
        VIDEO_EXTENSIONS.contains(&self.extension.to_lowercase().as_str())
    }

    /// Check if this file is a subtitle based on classification role or extension.
    fn is_subtitle(&self) -> bool {
        if let Some(ref c) = self.classification
            && c.role == nats_jobs::FileRole::Subtitle
        {
            return true;
        }
        SUBTITLE_EXTENSIONS.contains(&self.extension.to_lowercase().as_str())
    }

    /// Check if this file is main content based on classification role.
    fn is_main_content(&self) -> bool {
        self.classification
            .as_ref()
            .is_some_and(|c| c.role == nats_jobs::FileRole::MainContent)
    }

    /// Check if this file is a sample/trailer/extras based on classification role.
    fn is_non_content(&self) -> bool {
        self.classification.as_ref().is_some_and(|c| {
            matches!(
                c.role,
                nats_jobs::FileRole::Sample
                    | nats_jobs::FileRole::Trailer
                    | nats_jobs::FileRole::Extras
                    | nats_jobs::FileRole::Proof
                    | nats_jobs::FileRole::Metadata
            )
        })
    }

    /// Get the filename portion of the path (after last / or after ! for archives)
    fn filename(&self) -> &str {
        // For archive paths like "archive.zip!/inner/path.epub", get "path.epub"
        // For regular paths like "dir/file.epub", get "file.epub"
        self.path
            .rsplit_once('/')
            .map(|(_, name)| name)
            .or_else(|| {
                self.path
                    .rsplit_once('!')
                    .map(|(_, name)| name.trim_start_matches('/'))
            })
            .unwrap_or(&self.path)
    }

    /// Normalize the filename for comparison (lowercase, strip extension, normalize whitespace)
    fn normalized_name(&self) -> String {
        let filename = self.filename();
        // Strip extension
        let name = filename
            .rsplit_once('.')
            .map(|(n, _)| n)
            .unwrap_or(filename);
        // Normalize: lowercase, replace underscores with spaces, collapse whitespace
        name.to_lowercase()
            .replace('_', " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Result of classifying downloaded files
#[derive(Debug)]
pub enum FileClassification {
    /// All ebook files appear to be the same book (different formats)
    SingleBook { ebooks: Vec<KobloadFileInfo> },
    /// Multiple distinct books detected - needs user review
    MultipleBooks { ebooks: Vec<KobloadFileInfo> },
    /// No ebook files found
    NoEbooks,
}

/// Classify a list of files into single book, multiple books, or no ebooks.
/// Uses Kobload's classification and file_groups when available for more accurate grouping.
pub fn classify_files(
    files: &[KobloadFileInfo],
    file_groups: Option<&[KobloadFileGroup]>,
) -> FileClassification {
    let mut ebooks: Vec<KobloadFileInfo> = files
        .iter()
        .filter(|f| f.is_ebook() || f.is_audiobook())
        .cloned()
        .collect();
    // Sort by Kobload score (highest first) when available
    ebooks.sort_by_key(|b| std::cmp::Reverse(b.score.unwrap_or(0)));

    if ebooks.is_empty() {
        return FileClassification::NoEbooks;
    }

    // Multi-file audiobooks (e.g. split MP3s) are always a single book
    let all_audio = ebooks.iter().all(|f| f.is_audiobook());
    if all_audio {
        return FileClassification::SingleBook { ebooks };
    }

    if ebooks.len() == 1 {
        return FileClassification::SingleBook { ebooks };
    }

    // Strategy 1: Use Kobload's file classification group_key if available
    // Files with the same group_key are alternate formats of the same content
    let has_group_keys = ebooks.iter().any(|f| {
        f.classification
            .as_ref()
            .and_then(|c| c.group_key.as_deref())
            .is_some()
    });

    if has_group_keys {
        let unique_groups: std::collections::HashSet<_> = ebooks
            .iter()
            .filter_map(|f| {
                f.classification
                    .as_ref()
                    .and_then(|c| c.group_key.as_deref())
            })
            .collect();
        // If all ebooks share the same group_key (or have no key), it's a single book
        return if unique_groups.len() <= 1 {
            FileClassification::SingleBook { ebooks }
        } else {
            FileClassification::MultipleBooks { ebooks }
        };
    }

    // Strategy 2: Use Kobload's file_groups if provided
    if let Some(groups) = file_groups {
        let ebook_groups: Vec<_> = groups
            .iter()
            .filter(|g| {
                g.formats
                    .iter()
                    .any(|f| EBOOK_EXTENSIONS.contains(&f.as_str()))
            })
            .collect();
        return if ebook_groups.len() <= 1 {
            FileClassification::SingleBook { ebooks }
        } else {
            FileClassification::MultipleBooks { ebooks }
        };
    }

    // Strategy 3: Fallback to name-based matching
    let first_name = ebooks[0].normalized_name();
    let all_same = ebooks.iter().all(|f| f.normalized_name() == first_name);

    if all_same {
        FileClassification::SingleBook { ebooks }
    } else {
        FileClassification::MultipleBooks { ebooks }
    }
}

/// Marks a 204 response as "acknowledged, but nothing was imported".
///
/// Genuine imports and no-op acknowledgements both return 204, so
/// [`process_kobload_ingest`] needs an explicit signal to tell them apart.
/// Internal to the ingest path — Kobload never sees this header.
const NOT_IMPORTED_HEADER: &str = "x-kobbler-not-imported";

/// A 204 response tagged as "nothing imported". See [`NOT_IMPORTED_HEADER`].
fn not_imported_response() -> axum::response::Response {
    (StatusCode::NO_CONTENT, [(NOT_IMPORTED_HEADER, "1")]).into_response()
}

/// Drop the internal not-imported marker before a response leaves over HTTP.
fn strip_internal_headers(mut response: axum::response::Response) -> axum::response::Response {
    response.headers_mut().remove(NOT_IMPORTED_HEADER);
    response
}

/// Outcome of processing a Kobload ingest callback.
pub enum IngestOutcome {
    /// The ingest was processed successfully (new import).
    Imported,
    /// The callback was acknowledged but nothing was imported — Kobload
    /// reported a non-completed status (failed, cancelled, ...), so there were
    /// no files to place. Distinct from `Imported`: reporting these as imported
    /// made Kobload stamp `import_confirmed_at` on a failed download, which
    /// permanently suppressed the re-search for that episode.
    NotImported,
    /// The ingest was already processed (idempotency hit).
    AlreadyProcessed,
    /// The target media root is temporarily unavailable (e.g. NAS down).
    /// Contains the media_root_id so the caller can store the deferred ingest.
    Deferred(Uuid),
}

/// The naming template renders the same path for every copy of an episode, so a
/// regrab would rename its download straight over the file it is meant to sit
/// beside — losing the old copy on disk while the DB still shows two rows. Only
/// `AddCopy` takes a suffix; every other path keeps the template's name verbatim.
async fn non_clobbering_path(path: PathBuf, mode: FileLinkMode) -> PathBuf {
    if !mode.is_add_copy() || !fs::try_exists(&path).await.unwrap_or(false) {
        return path;
    }
    let Some(dir) = path.parent().map(PathBuf::from) else {
        return path;
    };
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = path.extension().and_then(|s| s.to_str()).map(String::from);
    for n in 2..100 {
        let name = match ext {
            Some(ref ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = dir.join(name);
        if !fs::try_exists(&candidate).await.unwrap_or(false) {
            return candidate;
        }
    }
    path
}

pub(super) fn sanitize_filename(value: &str) -> String {
    let cleaned = value
        .replace(['/', '\\'], "-")
        .split('\0')
        .next()
        .unwrap_or("")
        .to_string();
    let cleaned = crate::media_root_naming::sanitize_for_filesystem(&cleaned);
    if cleaned.is_empty() {
        "download.epub".to_string()
    } else {
        cleaned
    }
}

/// Wanted book metadata retrieved from database for ingest (from books table)
#[derive(Debug)]
#[allow(dead_code)] // Fields used for future metadata override support
struct WantedMetadata {
    id: Uuid,
    title: String,
    author: Option<String>,
    series: Option<String>,
    series_index: Option<f64>,
    isbn10: Option<String>,
    isbn13: Option<String>,
    provider: String,
    provider_id: Option<String>,
    cover_url: Option<String>, // extracted from images JSONB
}

pub async fn kobload_ingest(
    State(state): State<AppState>,
    Path(path_token): Path<String>,
    Json(payload): Json<KobloadCallbackRequest>,
) -> axum::response::Response {
    // Validate token against ingest_tokens table (same tokens work for Calibre and webhooks)
    match ingest_tokens::is_valid(&state.pool, &path_token).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!("Ingest callback auth failed - invalid or revoked token");
            return (StatusCode::UNAUTHORIZED, "invalid ingest token").into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to validate ingest token");
            return (StatusCode::INTERNAL_SERVER_ERROR, "token validation failed").into_response();
        }
    }

    // Check idempotency key synchronously — don't enqueue a duplicate job
    if let Some(idempotency_key) = payload.idempotency_key
        && let Ok(Some(_)) =
            wanted_attempts::check_idempotency_key(&state.pool, idempotency_key).await
    {
        tracing::info!(
            download_id = %payload.download_id,
            idempotency_key = %idempotency_key,
            "Duplicate HTTP callback detected via idempotency key"
        );
        return StatusCode::OK.into_response();
    }

    let effective_status = if payload.recoverable_error.is_some() {
        "kobload_error"
    } else {
        payload.status.as_str()
    };

    // For completed/downloaded status: enqueue async job and return 202
    // For failed/error/kobload_error: process synchronously (fast, no file downloads)
    match effective_status {
        "completed" | "downloaded" => {
            let download_id = payload.download_id;
            let job_payload = serde_json::json!({
                "request": serde_json::to_value(&payload).unwrap_or_default(),
                "response_mode": "http",
            });

            let display =
                crate::jobs::fetch_job_display(&state.pool, "download", download_id).await;
            let context = nats_jobs::JobContext {
                description: format!("Ingesting download {}", download_id),
                entity_type: Some("download".to_string()),
                entity_id: Some(download_id),
                display,
            };

            match crate::jobs::enqueue(
                &state.pool,
                state.nats.publisher(),
                "kobload_ingest",
                job_payload,
                context,
            )
            .await
            {
                Ok(job_id) => {
                    tracing::info!(
                        download_id = %download_id,
                        job_id = %job_id,
                        "Enqueued kobload_ingest job from HTTP callback"
                    );
                    StatusCode::ACCEPTED.into_response()
                }
                Err(e) => {
                    tracing::error!(
                        download_id = %download_id,
                        error = %e,
                        "Failed to enqueue kobload_ingest job, processing synchronously"
                    );
                    // Fallback: process synchronously if job enqueue fails
                    strip_internal_headers(
                        process_kobload_ingest_impl(
                            &state,
                            payload,
                            false,
                            tokio_util::sync::CancellationToken::new(),
                        )
                        .await,
                    )
                }
            }
        }
        _ => {
            // Failed/error/kobload_error: process synchronously (no file downloads needed)
            strip_internal_headers(
                process_kobload_ingest_impl(
                    &state,
                    payload,
                    false,
                    tokio_util::sync::CancellationToken::new(),
                )
                .await,
            )
        }
    }
}

/// Process a Kobload ingest callback (internal implementation).
/// This is called by the HTTP endpoint after token validation, and by retry_callback.
pub async fn process_kobload_ingest(
    state: &AppState,
    payload: KobloadCallbackRequest,
    skip_http_confirm: bool,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<IngestOutcome, String> {
    // Early idempotency check — detect duplicates before doing any work
    if let Some(idempotency_key) = payload.idempotency_key {
        match wanted_attempts::check_idempotency_key(&state.pool, idempotency_key).await {
            Ok(Some(attempt_id)) => {
                tracing::info!(
                    download_id = %payload.download_id,
                    idempotency_key = %idempotency_key,
                    attempt_id = %attempt_id,
                    "Duplicate ingest detected via idempotency key, skipping"
                );
                return Ok(IngestOutcome::AlreadyProcessed);
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    download_id = %payload.download_id,
                    error = %e,
                    "Failed to check idempotency key, proceeding anyway"
                );
            }
        }
    }

    let response = process_kobload_ingest_impl(state, payload, skip_http_confirm, cancel).await;
    let status = response.status();
    // Success and "acknowledged but nothing imported" both return 204, so the
    // status code alone cannot tell them apart. The per-entity handlers tag the
    // latter with `NOT_IMPORTED_HEADER`.
    let not_imported = response.headers().contains_key(NOT_IMPORTED_HEADER);
    if (status.is_success() || status == StatusCode::NO_CONTENT) && not_imported {
        Ok(IngestOutcome::NotImported)
    } else if status.is_success() || status == StatusCode::NO_CONTENT {
        Ok(IngestOutcome::Imported)
    } else if status == StatusCode::SERVICE_UNAVAILABLE {
        // Media root unavailable — extract media_root_id from response body
        let body = axum::body::to_bytes(response.into_body(), 4096).await.ok();
        let media_root_id = body
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v.get("media_root_id")?.as_str()?.parse::<Uuid>().ok());
        if let Some(id) = media_root_id {
            Ok(IngestOutcome::Deferred(id))
        } else {
            Err("Ingest returned 503 but no media_root_id".to_string())
        }
    } else {
        Err(format!("Ingest returned status {}", status))
    }
}

async fn process_kobload_ingest_impl(
    state: &AppState,
    payload: KobloadCallbackRequest,
    skip_http_confirm: bool,
    cancel: tokio_util::sync::CancellationToken,
) -> axum::response::Response {
    // Check idempotency key first - if we've already processed this callback, return success
    if let Some(idempotency_key) = payload.idempotency_key {
        match wanted_attempts::check_idempotency_key(&state.pool, idempotency_key).await {
            Ok(Some(attempt_id)) => {
                tracing::info!(
                    download_id = %payload.download_id,
                    idempotency_key = %idempotency_key,
                    attempt_id = %attempt_id,
                    "Duplicate callback detected via idempotency key, returning success"
                );
                return StatusCode::OK.into_response();
            }
            Ok(None) => {
                // New key, proceed with processing
            }
            Err(e) => {
                tracing::warn!(
                    download_id = %payload.download_id,
                    error = %e,
                    "Failed to check idempotency key, proceeding anyway"
                );
            }
        }
    }

    let runtime = state.runtime_config.get().await;
    let download_id = payload.download_id.to_string();

    // Handle recoverable errors from Kobload (e.g., can't access download files)
    // These should show as "kobload_error" status so user knows to check Kobload
    let (effective_status, message) = if let Some(ref recoverable_err) = payload.recoverable_error {
        tracing::info!(
            download_id = %payload.download_id,
            error = %recoverable_err,
            "Kobload reported recoverable error - waiting for resolution"
        );
        ("kobload_error".to_string(), Some(recoverable_err.clone()))
    } else {
        (payload.status.to_lowercase(), payload.error.clone())
    };

    // Deserialize metadata once into typed struct for cleaner field access.
    let meta: IngestMetadata = serde_json::from_value(payload.metadata.clone()).unwrap_or_default();
    let resolved = meta.resolve();

    // Route by source_type from callback metadata.
    // Movie/TV handlers manage their own attempt status updates and entity ID extraction.
    // The default (book) path falls through to the existing logic below.
    // Check top-level first, then fall back to original_metadata (watch-system downloads
    // nest the entity metadata under original_metadata).
    let source_type = resolved.source_type().unwrap_or("book");

    match source_type {
        "movie" => {
            return ingest_movie(
                state,
                &runtime,
                payload,
                &effective_status,
                message.as_deref(),
                skip_http_confirm,
                cancel.clone(),
            )
            .await;
        }
        "tv_episode" | "sport_event" => {
            return ingest_tv_episode(
                state,
                &runtime,
                payload,
                &effective_status,
                message.as_deref(),
                skip_http_confirm,
                cancel.clone(),
            )
            .await;
        }
        "tv_season" => {
            return ingest_tv_season(
                state,
                &runtime,
                payload,
                &effective_status,
                message.as_deref(),
                skip_http_confirm,
                cancel.clone(),
            )
            .await;
        }
        _ => {} // Fall through to existing book ingest logic
    }

    // Store files_json and update attempt status.
    // For completed downloads, set to "ingesting" — we'll mark "completed" after files are downloaded.
    let attempt_status = if effective_status == "completed"
        || effective_status == "complete"
        || effective_status == "downloaded"
    {
        "ingesting".to_string()
    } else {
        effective_status.clone()
    };
    let files_json = serde_json::to_value(&payload.files).ok();
    let attempt_result = wanted_attempts::update_status_returning_with_idempotency(
        &state.pool,
        &download_id,
        &attempt_status,
        message.as_deref(),
        files_json,
        payload.idempotency_key,
    )
    .await;

    let (book_id, download_url_from_attempt): (Option<Uuid>, Option<String>) = match attempt_result
    {
        Ok(Some((bid, _movie_id, du))) => (bid, du),
        _ => {
            // Fallback: extract book_id from callback metadata (sent in want_request)
            let fallback_book_id = resolved.book_id();
            if let Some(bid) = fallback_book_id {
                tracing::info!(
                    download_id = %payload.download_id,
                    book_id = %bid,
                    "No wanted_attempt found for download, using book_id from callback metadata"
                );
            }
            (fallback_book_id, None)
        }
    };

    let release_info = payload.release_info.clone();

    // Broadcast status update event for real-time UI updates
    if let Some(book_id) = book_id {
        let _ = state
            .library_events
            .send(LibraryEvent::wanted_status_updated(
                book_id,
                attempt_status.clone(),
            ));
    }

    // If recoverable error, acknowledge but don't process files
    if payload.recoverable_error.is_some() {
        return not_imported_response();
    }

    // If not completed, reset want_status so re-search can find a replacement
    if effective_status != "completed"
        && effective_status != "complete"
        && effective_status != "downloaded"
    {
        if let Some(book_id) = book_id {
            let _ = books::set_want_status(&state.pool, book_id, "wanted").await;
            let _ = state
                .library_events
                .send(LibraryEvent::library_updated(Some(book_id)));
        }
        return not_imported_response();
    }

    // Classify the downloaded files
    let classification = classify_files(&payload.files, payload.file_groups.as_deref());

    match classification {
        FileClassification::NoEbooks => {
            tracing::warn!(download_id = %payload.download_id, "No ebook files found in download");
            // Update attempt status to failed
            let _ = wanted_attempts::update_status(
                &state.pool,
                &download_id,
                "failed",
                Some("No ebook files found"),
            )
            .await;
            // Broadcast status update event
            if let Some(book_id) = book_id {
                let _ = state
                    .library_events
                    .send(LibraryEvent::wanted_status_updated(
                        book_id,
                        "failed".to_string(),
                    ));
            }
            (StatusCode::BAD_REQUEST, "no ebook files in download").into_response()
        }
        FileClassification::MultipleBooks { ebooks } => {
            // Group files by normalized name — each group is a separate book
            let groups = group_ebooks_by_name(ebooks);
            tracing::info!(
                download_id = %payload.download_id,
                group_count = groups.len(),
                "Multiple distinct books detected - ingesting each group separately"
            );

            // If we have a wanted book, try to match one group to it by title
            let wanted_title = if book_id.is_some() {
                resolved.title().map(|s| s.to_lowercase())
            } else {
                None
            };

            let mut matched_wanted = false;
            for (_name, group) in &groups {
                // Match this group to the wanted book if titles are similar
                let group_book_id = if !matched_wanted {
                    if let Some(ref wanted) = wanted_title {
                        let group_name = group[0].normalized_name();
                        if group_name.contains(wanted) || wanted.contains(&group_name) {
                            matched_wanted = true;
                            book_id
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                ingest_single_book(
                    state,
                    &runtime,
                    payload.download_id,
                    &resolved,
                    group_book_id,
                    group.clone(),
                    release_info.as_ref(),
                    download_url_from_attempt.as_deref(),
                    payload.release_interpretation.as_ref(),
                    skip_http_confirm,
                    cancel.clone(),
                )
                .await;
            }

            // If wanted book wasn't matched to any group, mark attempt as completed anyway
            if !matched_wanted && let Some(book_id) = book_id {
                let _ = wanted_attempts::update_status(
                    &state.pool,
                    &download_id,
                    "completed",
                    Some("Multi-book download ingested to queue"),
                )
                .await;
                let _ = state
                    .library_events
                    .send(LibraryEvent::wanted_status_updated(
                        book_id,
                        "downloading".to_string(),
                    ));
            }

            StatusCode::NO_CONTENT.into_response()
        }
        FileClassification::SingleBook { ebooks } => {
            tracing::info!(
                download_id = %payload.download_id,
                file_count = ebooks.len(),
                formats = ?ebooks.iter().map(|f| &f.extension).collect::<Vec<_>>(),
                "Single book with multiple formats - proceeding with ingest"
            );
            // Continue with ingest
            ingest_single_book(
                state,
                &runtime,
                payload.download_id,
                &resolved,
                book_id,
                ebooks,
                release_info.as_ref(),
                download_url_from_attempt.as_deref(),
                payload.release_interpretation.as_ref(),
                skip_http_confirm,
                cancel.clone(),
            )
            .await
        }
    }
}

/// Group ebooks by their normalized name (each group = one book with possibly multiple formats)
fn group_ebooks_by_name(ebooks: Vec<KobloadFileInfo>) -> Vec<(String, Vec<KobloadFileInfo>)> {
    let mut groups: std::collections::BTreeMap<String, Vec<KobloadFileInfo>> =
        std::collections::BTreeMap::new();
    for ebook in ebooks {
        let name = ebook.normalized_name();
        groups.entry(name).or_default().push(ebook);
    }
    groups.into_iter().collect()
}

/// Build source provenance JSON from release info
fn build_source_provenance(
    download_id: Uuid,
    release_info: Option<&nats_jobs::ReleaseInfo>,
    download_url: Option<&str>,
) -> serde_json::Value {
    let mut provenance = serde_json::json!({
        "download_id": download_id.to_string(),
    });

    if let Some(info) = release_info {
        provenance["release_title"] = serde_json::json!(info.title);
        if let Some(ref indexer) = info.indexer {
            provenance["indexer"] = serde_json::json!(indexer);
        }
        if let Some(size) = info.size {
            provenance["size"] = serde_json::json!(size);
        }
        if let Some(ref protocol) = info.protocol {
            provenance["protocol"] = serde_json::json!(protocol);
        }
        if let Some(ref info_hash) = info.info_hash {
            provenance["info_hash"] = serde_json::json!(info_hash);
        }
    }

    if let Some(url) = download_url {
        provenance["download_url"] = serde_json::json!(url);
    }

    provenance
}

/// Ingest a single book (possibly with multiple format files)
#[allow(clippy::too_many_arguments)]
async fn ingest_single_book(
    state: &AppState,
    runtime: &RuntimeConfig,
    download_id: Uuid,
    meta_resolved: &ResolvedMeta<'_>,
    book_id: Option<Uuid>,
    ebooks: Vec<KobloadFileInfo>,
    release_info: Option<&nats_jobs::ReleaseInfo>,
    download_url: Option<&str>,
    release_interpretation: Option<&nats_jobs::ParsedRelease>,
    skip_http_confirm: bool,
    cancel: tokio_util::sync::CancellationToken,
) -> axum::response::Response {
    // Extract parsed book metadata from Kobload's release interpretation (fallback source)
    let parsed_book = extract_book_from_interpretation(release_interpretation);

    // Get wanted book metadata if available (from consolidated books table)
    let wanted_metadata = if let Some(book_id) = book_id {
        books::get_wanted_metadata(&state.pool, book_id)
            .await
            .ok()
            .flatten()
            .map(|meta| {
                // Determine provider from identifiers
                let (provider, provider_id) = if let Some(hc_id) =
                    meta.identifiers.get("hardcover").and_then(|v| v.as_str())
                {
                    ("hardcover".to_string(), Some(hc_id.to_string()))
                } else {
                    ("manual".to_string(), None)
                };
                WantedMetadata {
                    id: book_id,
                    title: meta.title,
                    author: meta.author,
                    series: meta.series_name,
                    series_index: meta.series_index,
                    isbn10: meta
                        .identifiers
                        .get("isbn10")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    isbn13: meta
                        .identifiers
                        .get("isbn13")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    provider,
                    provider_id,
                    cover_url: crate::images::source_url(&meta.images, "cover"),
                }
            })
    } else {
        None
    };

    // Log if we're using parsed metadata as fallback
    if wanted_metadata.is_none() && parsed_book.is_some() {
        tracing::info!(
            download_id = %download_id,
            parsed_title = ?parsed_book.as_ref().map(|p| &p.title),
            parsed_author = ?parsed_book.as_ref().and_then(|p| p.author.as_ref()),
            "Using Kobload release interpretation as metadata source (no wanted book linked)"
        );
    }

    // Download path and ingest settings
    let target_dir = PathBuf::from(&state.config.storage_root).join("downloads");
    let ingest_mode = IngestMode::Auto;
    let file_mode = FileMode::Copy;

    // Get Kobload base URL and API key
    let kobload_base_url = match runtime.kobload_base_url.clone() {
        Some(url) if !url.trim().is_empty() => url,
        _ => return (StatusCode::BAD_REQUEST, "kobload_base_url not configured").into_response(),
    };

    let api_key = match runtime.kobload_api_key.clone() {
        Some(key) if !key.trim().is_empty() => key,
        _ => return (StatusCode::BAD_REQUEST, "kobload_api_key not configured").into_response(),
    };

    if let Err(err) = fs::create_dir_all(&target_dir).await {
        tracing::error!(error = %err, path = %target_dir.display(), "Failed to ensure download directory exists");
        let body = format!(
            "no files ingested (local): create download dir {}: {err}",
            target_dir.display()
        );
        if book_id.is_some() {
            let _ = wanted_attempts::update_status(
                &state.pool,
                &download_id.to_string(),
                "failed",
                Some(&body),
            )
            .await;
        }
        return (StatusCode::INTERNAL_SERVER_ERROR, body).into_response();
    }

    // Download and ingest each ebook file.
    // Use a long timeout since ebook files can be large (especially audiobooks/comics).
    let client = crate::http::builder()
        .timeout(std::time::Duration::from_secs(86400))
        .read_timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client");
    let mut first_ingest_id: Option<Uuid> = None;
    let mut all_ingest_ids: Vec<Uuid> = Vec::new();
    let mut ingested_count = 0;
    let mut last_failure: Option<(FailureKind, String)> = None;
    // Additional audio file paths to process after the first file creates the book.
    // These bypass the ingest queue (book_ingest_source has 1:1 PK with books).
    let mut extra_audio_paths: Vec<PathBuf> = Vec::new();

    for ebook in &ebooks {
        // Build file URL: prefer the URL provided by kobload, fall back to path-based
        let file_url = if let Some(ref url) = ebook.url {
            format!("{}{}", kobload_base_url.trim_end_matches('/'), url,)
        } else {
            format!(
                "{}/api/v1/downloads/{}/files/{}",
                kobload_base_url.trim_end_matches('/'),
                download_id,
                urlencoding::encode(&ebook.path)
            )
        };

        // Determine filename: wanted_metadata > parsed_book > release_title > download_id
        let base_name = wanted_metadata
            .as_ref()
            .map(|w| sanitize_filename(&w.title))
            .or_else(|| parsed_book.as_ref().map(|p| sanitize_filename(&p.title)))
            .unwrap_or_else(|| {
                meta_resolved
                    .release_title()
                    .map(sanitize_filename)
                    .unwrap_or_else(|| format!("kobload-{}", download_id))
            });
        let file_name = format!("{}.{}", base_name, ebook.extension);
        let file_path = target_dir.join(format!("{}-{}", download_id, file_name));

        // Download the file
        tracing::info!(
            download_id = %download_id,
            path = %ebook.path,
            extension = %ebook.extension,
            "Downloading ebook file from Kobload"
        );

        let response = match client.get(&file_url).bearer_auth(&api_key).send().await {
            Ok(resp) => resp,
            Err(err) => {
                tracing::error!(error = %err, path = %ebook.path, "Failed to fetch file from Kobload");
                note_failure(
                    &mut last_failure,
                    FailureKind::Upstream,
                    format!("fetch: {err}"),
                );
                continue;
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            tracing::error!(
                status = %status,
                path = %ebook.path,
                "Kobload file fetch failed"
            );
            note_failure(
                &mut last_failure,
                FailureKind::Upstream,
                format!("kobload fetch returned {status}"),
            );
            continue;
        }

        let mut file = match fs::File::create(&file_path).await {
            Ok(file) => file,
            Err(err) => {
                tracing::error!(error = %err, "Failed to create ingest file");
                note_failure(
                    &mut last_failure,
                    FailureKind::Local,
                    format!("create file: {err}"),
                );
                continue;
            }
        };

        let mut body = response.bytes_stream();
        let mut write_failure: Option<(FailureKind, String)> = None;
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::warn!(download_id = %download_id, "Download cancelled");
                    write_failure = Some((FailureKind::Local, "download cancelled".to_string()));
                    break;
                }
                chunk = body.next() => chunk,
            };
            let Some(chunk) = chunk else { break };
            match chunk {
                Ok(bytes) => {
                    if let Err(err) = file.write_all(&bytes).await {
                        tracing::error!(error = %err, "Failed to write file chunk");
                        write_failure = Some((FailureKind::Local, format!("write chunk: {err}")));
                        break;
                    }
                }
                Err(err) => {
                    tracing::error!(error = %err, "Failed to read file stream");
                    write_failure = Some((FailureKind::Upstream, format!("read stream: {err}")));
                    break;
                }
            }
        }

        if let Some((kind, reason)) = write_failure {
            note_failure(&mut last_failure, kind, reason);
            let _ = fs::remove_file(&file_path).await;
            continue;
        }

        // Finish and close the file before it is renamed or read. See
        // `finalize_download`.
        if let Err(err) = finalize_download(file).await {
            tracing::error!(error = %err, "Failed to finalize book download");
            note_failure(
                &mut last_failure,
                FailureKind::Local,
                format!("finalize file: {err}"),
            );
            let _ = fs::remove_file(&file_path).await;
            continue;
        }

        let file_meta = match fs::metadata(&file_path).await {
            Ok(meta) => meta,
            Err(err) => {
                tracing::error!(error = %err, "Failed to stat file");
                note_failure(
                    &mut last_failure,
                    FailureKind::Local,
                    format!("stat file: {err}"),
                );
                continue;
            }
        };

        let file_size = file_meta.len() as i64;
        let file_hash: Option<String> = compute_content_hash(&file_path).await.ok();
        let initial_status = match ingest_mode {
            IngestMode::Auto => "converting",
            IngestMode::Manual => "extracting",
        };

        // Build source_metadata for provider linking, cover fetching, and source provenance
        let source_provenance = build_source_provenance(download_id, release_info, download_url);
        let source_metadata = {
            let mut meta = wanted_metadata
                .as_ref()
                .map(|w| {
                    let mut m = serde_json::json!({
                        "provider": w.provider
                    });
                    if let Some(ref external_id) = w.provider_id {
                        m["external_id"] = serde_json::json!(external_id);
                    }
                    if let Some(ref cover_url) = w.cover_url {
                        m["cover_url"] = serde_json::json!(cover_url);
                    }
                    if let Some(ref isbn13) = w.isbn13 {
                        m["isbn13"] = serde_json::json!(isbn13);
                    }
                    if let Some(ref isbn10) = w.isbn10 {
                        m["isbn10"] = serde_json::json!(isbn10);
                    }
                    m
                })
                .unwrap_or_else(|| serde_json::json!({}));
            // Add source provenance for tracking download history
            meta["source_type"] = serde_json::json!("kobload");
            meta["source_provenance"] = source_provenance;
            Some(meta)
        };

        // Build pending_authors JSONB from wanted metadata, fall back to parsed book
        let pending_authors = wanted_metadata
            .as_ref()
            .and_then(|w| w.author.as_ref().map(|a| serde_json::json!([{"name": a}])))
            .or_else(|| {
                parsed_book
                    .as_ref()
                    .and_then(|p| p.author.as_ref().map(|a| serde_json::json!([{"name": a}])))
            });

        // Build pending_series JSONB from wanted metadata, fall back to parsed book
        let pending_series = wanted_metadata
            .as_ref()
            .and_then(|w| w.series.as_ref().map(|s| serde_json::json!({"name": s})))
            .or_else(|| {
                parsed_book
                    .as_ref()
                    .and_then(|p| p.series.as_ref().map(|s| serde_json::json!({"name": s})))
            });

        // Title: prefer wanted_metadata, fall back to parsed_book
        let ingest_title = wanted_metadata
            .as_ref()
            .map(|w| w.title.clone())
            .or_else(|| parsed_book.as_ref().map(|p| p.title.clone()));

        // Additional audio files bypass the ingest queue (book_ingest_source
        // has a 1:1 PK with books). They're processed directly after the first
        // file creates the book.
        if first_ingest_id.is_some() && ebook.is_audiobook() {
            extra_audio_paths.push(file_path);
            ingested_count += 1;
            continue;
        }

        let input = ingest_queue::IngestQueueInput {
            file_path: file_path.to_string_lossy().to_string(),
            file_name: file_name.clone(),
            file_size,
            file_hash,
            source_mtime: None,
            source_format: ebook.extension.clone(),
            title: ingest_title,
            pending_authors,
            pending_series,
            source_metadata,
            status: initial_status.to_string(),
            update_target_book_id: book_id,
            ..Default::default()
        };
        let ingest_id = match ingest_queue::create(&state.pool, input).await {
            Ok(id) => id,
            Err(err) => {
                tracing::error!(error = %err, "Failed to insert ingest queue item");
                note_failure(
                    &mut last_failure,
                    FailureKind::Local,
                    format!("insert ingest queue: {err}"),
                );
                continue;
            }
        };

        // Set ISBN and series_index only for NEW books (not when linking to existing wanted book)
        if book_id.is_none()
            && let Some(ref wm) = wanted_metadata
        {
            // Store ISBN-13 and ISBN-10 as separate identifiers (industry standard)
            let mut identifiers = serde_json::Map::new();
            if let Some(ref isbn13) = wm.isbn13 {
                identifiers.insert("isbn13".to_string(), serde_json::json!(isbn13));
            }
            if let Some(ref isbn10) = wm.isbn10 {
                identifiers.insert("isbn10".to_string(), serde_json::json!(isbn10));
            }
            if !identifiers.is_empty() {
                let _ = crate::repositories::books::update_identifiers(
                    &state.pool,
                    ingest_id,
                    serde_json::Value::Object(identifiers),
                )
                .await;
            }
            if let Some(series_index) = wm.series_index {
                let _ = crate::repositories::book_ingest_source::update(
                    &state.pool,
                    ingest_id,
                    crate::repositories::book_ingest_source::BookIngestSourceUpdate {
                        pending_series_index: Some(series_index),
                        ..Default::default()
                    },
                )
                .await;
            }
        }

        // Set narrator from parsed audiobook metadata
        if let Some(ref pb) = parsed_book
            && pb.is_audiobook
            && pb.narrator.is_some()
        {
            let _ = crate::repositories::books::update(
                &state.pool,
                ingest_id,
                crate::repositories::books::BookUpdate {
                    narrator: pb.narrator.clone(),
                    want_format: Some("audiobook".to_string()),
                    ..Default::default()
                },
            )
            .await;
        }

        // Fetch and store cover if cover_url is available in wanted metadata
        if let Some(ref wm) = wanted_metadata
            && let Some(ref cover_url) = wm.cover_url
        {
            let registry = ProviderRegistry::new(state.pool.clone());
            match registry.fetch_cover(&wm.provider, cover_url).await {
                Ok(cover_data) => {
                    tracing::info!(
                        ingest_id = %ingest_id,
                        provider = %wm.provider,
                        cover_size = cover_data.len(),
                        "Fetched cover image from provider"
                    );
                    if let Err(err) =
                        ingest_queue::update_cover(&state.pool, ingest_id, &cover_data).await
                    {
                        tracing::warn!(error = %err, "Failed to store cover in ingest queue");
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        provider = %wm.provider,
                        cover_url = %cover_url,
                        "Failed to fetch cover from provider"
                    );
                }
            }
        }

        if first_ingest_id.is_none() {
            first_ingest_id = Some(ingest_id);
        }
        all_ingest_ids.push(ingest_id);
        ingested_count += 1;

        // For manual mode, trigger metadata extraction for each file
        if ingest_mode == IngestMode::Manual {
            let display = crate::jobs::fetch_job_display(&state.pool, "ingest", ingest_id).await;
            let context = nats_jobs::JobContext::for_entity(
                "ingest",
                ingest_id,
                format!(
                    "Extracting metadata for '{}'",
                    wanted_metadata
                        .as_ref()
                        .map(|w| w.title.as_str())
                        .unwrap_or(&file_name)
                ),
            )
            .with_display_opt(display);
            let _ = crate::jobs::enqueue(
                &state.pool,
                state.nats.publisher(),
                "metadata_extraction",
                serde_json::json!({}),
                context,
            )
            .await;
        }
    }

    // Auto mode: process all files. First file creates the book, remaining files
    // are linked to the same book as additional formats.
    if ingest_mode == IngestMode::Auto && !all_ingest_ids.is_empty() {
        let pool = state.pool.clone();
        let storage_root = state.config.storage_root.clone();
        let library_events = state.library_events.clone();
        let nats_publisher = state.nats.publisher().clone();
        let runtime_config = state.runtime_config.clone();
        let ingest_ids = all_ingest_ids.clone();
        let extra_audio = std::mem::take(&mut extra_audio_paths);
        tokio::spawn(async move {
            // Process first file — creates the book
            let first_id = ingest_ids[0];
            match process_queue_item(
                &pool,
                &storage_root,
                &library_events,
                &nats_publisher,
                &runtime_config,
                first_id,
                IngestOptions::auto(file_mode),
            )
            .await
            {
                Ok(created_book_id) => {
                    // Process remaining ingest queue items (non-audio additional formats)
                    for &extra_id in &ingest_ids[1..] {
                        if let Err(e) = process_queue_item(
                            &pool,
                            &storage_root,
                            &library_events,
                            &nats_publisher,
                            &runtime_config,
                            extra_id,
                            IngestOptions::auto(file_mode),
                        )
                        .await
                        {
                            tracing::error!(error = %e, ingest_id = %extra_id, "Failed to auto-ingest additional format");
                        }
                    }

                    // Process additional audio files directly (bypassed ingest queue)
                    for audio_path in &extra_audio {
                        let format = audio_path
                            .extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or("mp3")
                            .to_uppercase();
                        let file_size = tokio::fs::metadata(audio_path)
                            .await
                            .map(|m| m.len() as i64)
                            .unwrap_or(0);
                        let dest_path = if file_mode == FileMode::Link {
                            audio_path.clone()
                        } else {
                            let book_dir = PathBuf::from(&storage_root)
                                .join("books")
                                .join(created_book_id.to_string());
                            let _ = tokio::fs::create_dir_all(&book_dir).await;
                            let file_name = audio_path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            let dest = book_dir.join(&file_name);
                            if let Err(e) = tokio::fs::copy(audio_path, &dest).await {
                                tracing::error!(error = %e, path = %audio_path.display(), "Failed to copy extra audio file");
                                continue;
                            }
                            dest
                        };

                        if let Err(e) = crate::repositories::audiobook_files::upsert_by_path(
                            &pool,
                            crate::repositories::audiobook_files::AudiobookFileInput {
                                book_id: created_book_id,
                                format: format.clone(),
                                file_path: dest_path.to_string_lossy().to_string(),
                                file_size,
                                source_type: Some("kobload".to_string()),
                                ..Default::default()
                            },
                        )
                        .await
                        {
                            tracing::error!(error = %e, path = %audio_path.display(), "Failed to create audiobook_files record");
                            continue;
                        }

                        // Probe for metadata and chapters
                        if let Ok(probe) = crate::audio::probe::probe_audio(&dest_path).await
                            && let Some(dur) = probe.duration_seconds
                        {
                            let _ = crate::repositories::books::update(
                                &pool,
                                created_book_id,
                                crate::repositories::books::BookUpdate {
                                    duration_seconds: Some(dur as i32),
                                    ..Default::default()
                                },
                            )
                            .await;
                        }

                        tracing::info!(
                            book_id = %created_book_id,
                            path = %audio_path.display(),
                            "Added extra audio file to book"
                        );
                    }

                    if !extra_audio.is_empty() {
                        let title = crate::repositories::books::get_title(&pool, created_book_id)
                            .await
                            .ok()
                            .flatten()
                            .unwrap_or_default();
                        crate::jobs::enqueue_audiobook_probe(
                            &pool,
                            &nats_publisher,
                            created_book_id,
                            &title,
                        )
                        .await;
                        let _ = library_events.send(crate::events::LibraryEvent::library_updated(
                            Some(created_book_id),
                        ));
                    }
                }
                Err(err) => {
                    tracing::error!(error = %err, "Failed to auto-ingest first file");
                }
            }
        });
    }

    // Confirm import to Kobload so it can delete the files.
    // Skip when processing via NATS job — the job handler sends the result via NATS instead.
    if ingested_count > 0 {
        if !skip_http_confirm {
            let confirm_url = format!(
                "{}/api/v1/downloads/{}/imported",
                kobload_base_url.trim_end_matches('/'),
                download_id
            );
            if let Err(err) = client.post(&confirm_url).bearer_auth(&api_key).send().await {
                tracing::warn!(error = %err, "Failed to confirm import to Kobload");
            }
        }

        // Mark book as in library and update attempt status to completed
        if let Some(book_id) = book_id {
            let _ = books::clear_rejected_releases(&state.pool, book_id).await;
            // Update the attempt status
            let _ = wanted_attempts::update_status_by_book(
                &state.pool,
                book_id,
                &download_id.to_string(),
                "completed",
            )
            .await;
            // Clean up watch pattern and broadcast acquisition event
            crate::wanted::on_book_acquired(&state.runtime_config, &state.library_events, book_id)
                .await;

            // Record activity
            crate::routes::library::helpers::record_activity_for_entity(
                &state.pool,
                "imported",
                book_id,
                crate::routes::library::helpers::requester_of(&state.pool, book_id).await,
            )
            .await;

            // Write sidecar and image files next to the book file
            crate::sidecar::write_back::write_book_sidecars(&state.pool, book_id).await;
            crate::sidecar::write_back::write_book_images(
                &state.pool,
                &state.config.storage_root,
                book_id,
            )
            .await;
        }
    }

    if ingested_count == 0 {
        let (status, body) = failure_response_parts(last_failure.as_ref());
        // Mark attempt as failed since no files could be downloaded
        if let Some(book_id) = book_id {
            let _ = wanted_attempts::update_status(
                &state.pool,
                &download_id.to_string(),
                "failed",
                Some(&body),
            )
            .await;
            let _ = state
                .library_events
                .send(LibraryEvent::wanted_status_updated(
                    book_id,
                    "failed".to_string(),
                ));
        }
        return (status, body).into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

/// Ingest a movie download from Kobload.
///
/// Handles its own attempt status update, file download, movie_files record creation,
/// and library state update. Called from the source_type routing in `process_kobload_ingest_impl`.
async fn ingest_movie(
    state: &AppState,
    runtime: &RuntimeConfig,
    payload: KobloadCallbackRequest,
    effective_status: &str,
    message: Option<&str>,
    skip_http_confirm: bool,
    cancel: tokio_util::sync::CancellationToken,
) -> axum::response::Response {
    let download_id = payload.download_id.to_string();
    let meta: IngestMetadata = serde_json::from_value(payload.metadata.clone()).unwrap_or_default();
    let resolved = meta.resolve();
    let link_mode = resolved.link_mode();

    // Update attempt status (same pattern as book path)
    let attempt_status = if effective_status == "completed"
        || effective_status == "complete"
        || effective_status == "downloaded"
    {
        "ingesting".to_string()
    } else {
        effective_status.to_string()
    };
    let files_json = serde_json::to_value(&payload.files).ok();
    let _ = wanted_attempts::update_status_returning_with_idempotency(
        &state.pool,
        &download_id,
        &attempt_status,
        message,
        files_json,
        payload.idempotency_key,
    )
    .await;

    // Extract movie_id from callback metadata (top-level or inside original_metadata
    // for watch-system downloads that nest entity metadata)
    let movie_id = resolved.movie_id();

    // Broadcast status update for real-time UI
    if let Some(movie_id) = movie_id {
        let _ = state
            .library_events
            .send(LibraryEvent::wanted_status_updated(
                movie_id,
                attempt_status.clone(),
            ));
    }

    // If recoverable error, acknowledge but don't process files
    if payload.recoverable_error.is_some() {
        return not_imported_response();
    }

    // If not completed, reset want_status so re-search can find a replacement
    if effective_status != "completed"
        && effective_status != "complete"
        && effective_status != "downloaded"
    {
        if let Some(movie_id) = movie_id {
            let _ = movies::set_want_status(&state.pool, movie_id, "wanted").await;
            let _ = state
                .library_events
                .send(LibraryEvent::movie_updated(movie_id));
        }
        return not_imported_response();
    }

    let movie_id = match movie_id {
        Some(id) => id,
        None => {
            tracing::error!(
                download_id = %payload.download_id,
                "Movie ingest callback missing movie_id in metadata"
            );
            let _ = wanted_attempts::update_status(
                &state.pool,
                &download_id,
                "failed",
                Some("Missing movie_id in callback metadata"),
            )
            .await;
            return (StatusCode::BAD_REQUEST, "missing movie_id in metadata").into_response();
        }
    };

    // Collect old files for deferred cleanup after successful download
    let available_files = movie_files::list_by_movie(&state.pool, movie_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f.available)
        .collect::<Vec<_>>();
    // Any file already in the library makes this a replacement — an upgrade, a
    // repack, a re-download to the same path — whatever Kobload calls it. Notifiers
    // update the item's existing post rather than announcing it a second time.
    let replaces_a_file = payload.is_upgrade || !available_files.is_empty();
    let upgrade_old_files = if payload.is_upgrade {
        available_files
    } else {
        Vec::new()
    };

    // Filter files to video and subtitle extensions, excluding samples/trailers/extras
    let media_files: Vec<&KobloadFileInfo> = payload
        .files
        .iter()
        .filter(|f| (f.is_video() || f.is_subtitle()) && !f.is_non_content())
        .collect();

    if media_files.is_empty() {
        tracing::warn!(
            download_id = %payload.download_id,
            movie_id = %movie_id,
            "No video or subtitle files found in movie download"
        );
        let _ = wanted_attempts::update_status(
            &state.pool,
            &download_id,
            "failed",
            Some("No video files found"),
        )
        .await;
        let _ = state
            .library_events
            .send(LibraryEvent::wanted_status_updated(
                movie_id,
                "failed".to_string(),
            ));
        return (StatusCode::BAD_REQUEST, "no video files in download").into_response();
    }

    let title = resolved.title().unwrap_or("Unknown Movie");
    let placement = match placement::movie(
        state,
        runtime,
        movie_id,
        title,
        resolved.year().map(|y| y as i32),
    )
    .await
    {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let target_dir = placement.dir.clone();

    // Get Kobload base URL and API key
    let kobload_base_url = match runtime.kobload_base_url.clone() {
        Some(url) if !url.trim().is_empty() => url,
        _ => return (StatusCode::BAD_REQUEST, "kobload_base_url not configured").into_response(),
    };
    let api_key = match runtime.kobload_api_key.clone() {
        Some(key) if !key.trim().is_empty() => key,
        _ => return (StatusCode::BAD_REQUEST, "kobload_api_key not configured").into_response(),
    };

    if let Err(err) = fs::create_dir_all(&target_dir).await {
        tracing::error!(
            error = %err,
            path = %target_dir.display(),
            "Failed to create movie target directory"
        );
        let body = format!(
            "no files ingested (local): create movie dir {}: {err}",
            target_dir.display()
        );
        let _ =
            wanted_attempts::update_status(&state.pool, &download_id, "failed", Some(&body)).await;
        return (StatusCode::INTERNAL_SERVER_ERROR, body).into_response();
    }

    let client = crate::http::builder()
        .timeout(std::time::Duration::from_secs(86400))
        .read_timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client");

    let mut ingested_count = 0;
    let mut subtitle_paths: Vec<String> = Vec::new();
    let mut last_failure: Option<(FailureKind, String)> = None;

    for file in &media_files {
        let file_url = if let Some(ref url) = file.url {
            format!("{}{}", kobload_base_url.trim_end_matches('/'), url)
        } else {
            format!(
                "{}/api/v1/downloads/{}/files/{}",
                kobload_base_url.trim_end_matches('/'),
                payload.download_id,
                urlencoding::encode(&file.path)
            )
        };

        let file_path = placement.file_path(&file.extension, file.filename());
        let file_path = non_clobbering_path(file_path, link_mode).await;
        // Download to hidden temp file to prevent scanner from picking it up mid-transfer
        let temp_path = target_dir.join(format!(
            ".kobbler-dl-{}.{}",
            Uuid::now_v7(),
            file.extension.to_lowercase()
        ));

        tracing::info!(
            download_id = %payload.download_id,
            movie_id = %movie_id,
            path = %file.path,
            extension = %file.extension,
            "Downloading movie file from Kobload"
        );

        let response = match client.get(&file_url).bearer_auth(&api_key).send().await {
            Ok(resp) => resp,
            Err(err) => {
                tracing::error!(error = %err, path = %file.path, "Failed to fetch movie file from Kobload");
                note_failure(
                    &mut last_failure,
                    FailureKind::Upstream,
                    format!("fetch: {err}"),
                );
                continue;
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            tracing::error!(
                status = %status,
                path = %file.path,
                "Kobload movie file fetch failed"
            );
            note_failure(
                &mut last_failure,
                FailureKind::Upstream,
                format!("kobload fetch returned {status}"),
            );
            continue;
        }

        let mut out_file = match fs::File::create(&temp_path).await {
            Ok(f) => f,
            Err(err) => {
                tracing::error!(error = %err, "Failed to create movie temp file on disk");
                note_failure(
                    &mut last_failure,
                    FailureKind::Local,
                    format!("create temp file: {err}"),
                );
                continue;
            }
        };

        let mut body = response.bytes_stream();
        let mut write_failure: Option<(FailureKind, String)> = None;
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::warn!(download_id = %payload.download_id, "Movie download cancelled");
                    write_failure = Some((FailureKind::Local, "download cancelled".to_string()));
                    break;
                }
                chunk = body.next() => chunk,
            };
            let Some(chunk) = chunk else { break };
            match chunk {
                Ok(bytes) => {
                    if let Err(err) = out_file.write_all(&bytes).await {
                        tracing::error!(error = %err, "Failed to write movie file chunk");
                        write_failure = Some((FailureKind::Local, format!("write chunk: {err}")));
                        break;
                    }
                }
                Err(err) => {
                    tracing::error!(error = %err, "Failed to read movie file stream");
                    write_failure = Some((FailureKind::Upstream, format!("read stream: {err}")));
                    break;
                }
            }
        }

        if let Some((kind, reason)) = write_failure {
            note_failure(&mut last_failure, kind, reason);
            let _ = fs::remove_file(&temp_path).await;
            continue;
        }

        // Finish and close the file before it is renamed or read. See
        // `finalize_download`.
        if let Err(err) = finalize_download(out_file).await {
            tracing::error!(error = %err, "Failed to finalize movie download");
            note_failure(
                &mut last_failure,
                FailureKind::Local,
                format!("finalize file: {err}"),
            );
            let _ = fs::remove_file(&temp_path).await;
            continue;
        }

        // Rename from hidden temp path to final template-derived name
        if let Err(err) = fs::rename(&temp_path, &file_path).await {
            tracing::error!(
                error = %err,
                from = %temp_path.display(),
                to = %file_path.display(),
                "Failed to rename movie temp file to final path"
            );
            note_failure(
                &mut last_failure,
                FailureKind::Local,
                format!("rename temp file: {err}"),
            );
            let _ = fs::remove_file(&temp_path).await;
            continue;
        }

        // Track subtitle paths separately for inclusion in movie_files records
        if file.is_subtitle() {
            subtitle_paths.push(file_path.to_string_lossy().to_string());
            ingested_count += 1;
            continue;
        }

        // Create movie_files record for video files
        let file_size = fs::metadata(&file_path).await.ok().map(|m| m.len() as i64);
        let source_metadata =
            build_source_provenance(payload.download_id, payload.release_info.as_ref(), None);

        // Extract video metadata from kobload's file_metadata
        let vm = file.file_metadata.as_ref().and_then(|m| m.video.as_ref());

        let movie_file_id = match movie_files::create_or_update(
            &state.pool,
            movie_files::MovieFileInput {
                movie_id,
                file_path: file_path.to_string_lossy().to_string(),
                file_size,
                file_fingerprint: None,
                format: file.extension.clone(),
                quality: None,
                languages: None,
                subtitles: if subtitle_paths.is_empty() {
                    None
                } else {
                    Some(subtitle_paths.clone())
                },
                source_type: Some("kobload".to_string()),
                source_metadata: Some(source_metadata),
                video_codec: vm.map(|v| v.codec.clone()),
                video_width: vm.map(|v| v.width as i32),
                video_height: vm.map(|v| v.height as i32),
                video_bitrate_kbps: vm.and_then(|v| v.bitrate_kbps.map(|b| b as i32)),
                framerate: vm.and_then(|v| v.framerate),
                bit_depth: vm.and_then(|v| v.bit_depth.map(|b| b as i16)),
                is_hdr: vm.is_some_and(|v| v.is_hdr),
                hdr_format: vm.and_then(|v| v.hdr_format.clone()),
                is_interlaced: vm.is_some_and(|v| v.is_interlaced),
                duration_seconds: file.file_metadata.as_ref().and_then(|m| m.duration_seconds),
                container: file
                    .file_metadata
                    .as_ref()
                    .and_then(|m| m.container.clone()),
                overall_bitrate_kbps: file
                    .file_metadata
                    .as_ref()
                    .and_then(|m| m.bitrate_kbps.map(|b| b as i32)),
            },
        )
        .await
        {
            Ok(id) => id,
            Err(err) => {
                tracing::error!(error = %err, "Failed to create movie_files record");
                note_failure(
                    &mut last_failure,
                    FailureKind::Local,
                    format!("create movie_files row: {err}"),
                );
                continue;
            }
        };

        let replaced = movie_files::replace_other_files_for_movie(
            &state.pool,
            movie_id,
            movie_file_id,
            link_mode,
        )
        .await
        .unwrap_or_default();
        crate::repositories::cleanup_replaced_files(&replaced).await;

        // Insert audio and subtitle tracks from file metadata (inline so the UI
        // sees them immediately, before the probe job lands).
        if let Some(ref fm) = file.file_metadata {
            persist_tracks_for_movie_file(&state.pool, movie_file_id, fm).await;
        }

        // Create external_subtitles records for subtitle files linked to this video file
        if !subtitle_paths.is_empty() {
            for sub_path in &subtitle_paths {
                persist_external_subtitle(&state.pool, Some(movie_file_id), None, sub_path).await;
            }
        }

        placement::announce_file(
            state,
            &placement,
            &file_path,
            placement::FileKind::Movie,
            movie_file_id,
            movie_id,
        )
        .await;

        ingested_count += 1;
    }

    // Confirm import to Kobload so it can clean up.
    // Skip when processing via NATS job — the job handler sends the result via NATS instead.
    if ingested_count > 0 {
        if !skip_http_confirm {
            let confirm_url = format!(
                "{}/api/v1/downloads/{}/imported",
                kobload_base_url.trim_end_matches('/'),
                payload.download_id
            );
            if let Err(err) = client.post(&confirm_url).bearer_auth(&api_key).send().await {
                tracing::warn!(error = %err, "Failed to confirm movie import to Kobload");
            }
        }

        // Upgrade: now that new files are downloaded, clean up old files
        for old_file in &upgrade_old_files {
            tracing::info!(
                movie_id = %movie_id,
                old_file_id = %old_file.id,
                old_path = %old_file.file_path,
                quality_score = ?payload.quality_score,
                "Upgrade: marking old movie file as unavailable"
            );
            let _ = movie_files::set_available(&state.pool, old_file.id, false).await;
            if let Err(e) = fs::remove_file(&old_file.file_path).await {
                tracing::warn!(
                    error = %e,
                    path = %old_file.file_path,
                    "Failed to delete old movie file during upgrade (may already be gone)"
                );
            }
        }

        // A terminal want_status, mirroring the TV paths: without it an imported movie
        // sits on 'searching' forever, satisfaction encoded only by the derived
        // is_in_library flag.
        placement::complete_movie(state, movie_id).await;

        // Update attempt status to completed
        let _ = wanted_attempts::update_status(&state.pool, &download_id, "completed", None).await;

        // Confirm want to Kobload (deletes pattern or starts upgrade window) and broadcast
        crate::wanted::on_entity_acquired(
            &state.runtime_config,
            &state.library_events,
            crate::wanted::SOURCE_SYSTEM,
            &movie_id.to_string(),
            movie_id,
            replaces_a_file,
        )
        .await;
    }

    if ingested_count == 0 {
        let (status, body) = failure_response_parts(last_failure.as_ref());
        let _ =
            wanted_attempts::update_status(&state.pool, &download_id, "failed", Some(&body)).await;
        let _ = state
            .library_events
            .send(LibraryEvent::wanted_status_updated(
                movie_id,
                "failed".to_string(),
            ));
        return (status, body).into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

/// Ingest a single TV episode download from Kobload.
///
/// Downloads video/subtitle files, creates tv_episode_files records, and marks the episode
/// as in-library. Called from the source_type routing in `process_kobload_ingest_impl`.
#[allow(clippy::too_many_arguments)]
async fn ingest_tv_episode(
    state: &AppState,
    runtime: &RuntimeConfig,
    payload: KobloadCallbackRequest,
    effective_status: &str,
    message: Option<&str>,
    skip_http_confirm: bool,
    cancel: tokio_util::sync::CancellationToken,
) -> axum::response::Response {
    let download_id = payload.download_id.to_string();
    let meta: IngestMetadata = serde_json::from_value(payload.metadata.clone()).unwrap_or_default();
    let resolved = meta.resolve();
    let link_mode = resolved.link_mode();

    // Update attempt status (same pattern as movie path)
    let attempt_status = if effective_status == "completed"
        || effective_status == "complete"
        || effective_status == "downloaded"
    {
        "ingesting".to_string()
    } else {
        effective_status.to_string()
    };
    let files_json = serde_json::to_value(&payload.files).ok();
    let _ = wanted_attempts::update_status_returning_with_idempotency(
        &state.pool,
        &download_id,
        &attempt_status,
        message,
        files_json,
        payload.idempotency_key,
    )
    .await;

    // Extract episode_id, show_id, season_id from callback metadata
    // (check top-level then original_metadata for watch-system downloads)
    let episode_id = resolved.episode_id();
    let show_id = resolved.show_id();
    let season_id = resolved.season_id();

    // Broadcast status update for real-time UI
    if let Some(episode_id) = episode_id {
        let _ = state
            .library_events
            .send(LibraryEvent::wanted_status_updated(
                episode_id,
                attempt_status.clone(),
            ));
    }

    // If recoverable error, acknowledge but don't process files
    if payload.recoverable_error.is_some() {
        return not_imported_response();
    }

    // If not completed, reset want_status so re-search can find a replacement
    if effective_status != "completed"
        && effective_status != "complete"
        && effective_status != "downloaded"
    {
        if let Some(episode_id) = episode_id {
            let _ = tv_episodes::set_want_status(&state.pool, episode_id, "wanted").await;
            if let Some(show_id) = show_id {
                let _ = state
                    .library_events
                    .send(LibraryEvent::tv_show_updated(show_id));
            }
        }
        return not_imported_response();
    }

    let episode_id = match episode_id {
        Some(id) => id,
        None => {
            tracing::error!(
                download_id = %payload.download_id,
                "TV episode ingest callback missing episode_id in metadata"
            );
            let _ = wanted_attempts::update_status(
                &state.pool,
                &download_id,
                "failed",
                Some("Missing episode_id in callback metadata"),
            )
            .await;
            return (StatusCode::BAD_REQUEST, "missing episode_id in metadata").into_response();
        }
    };

    // Collect old files for deferred cleanup after successful download
    let available_files = tv_episode_files::list_by_episode(&state.pool, episode_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f.available)
        .collect::<Vec<_>>();
    // Any file already in the library makes this a replacement — an upgrade, a
    // repack, a re-download to the same path — whatever Kobload calls it. Notifiers
    // update the item's existing post rather than announcing it a second time.
    let replaces_a_file = payload.is_upgrade || !available_files.is_empty();
    let upgrade_old_files = if payload.is_upgrade {
        available_files
    } else {
        Vec::new()
    };

    // Filter files to video and subtitle extensions, excluding samples/trailers/extras
    let media_files: Vec<&KobloadFileInfo> = payload
        .files
        .iter()
        .filter(|f| (f.is_video() || f.is_subtitle()) && !f.is_non_content())
        .collect();

    if media_files.is_empty() {
        tracing::warn!(
            download_id = %payload.download_id,
            episode_id = %episode_id,
            "No video or subtitle files found in TV episode download"
        );
        let _ = wanted_attempts::update_status(
            &state.pool,
            &download_id,
            "failed",
            Some("No video files found"),
        )
        .await;
        let _ = state
            .library_events
            .send(LibraryEvent::wanted_status_updated(
                episode_id,
                "failed".to_string(),
            ));
        return (StatusCode::BAD_REQUEST, "no video files in download").into_response();
    }

    let naming = placement::EpisodeNaming::load(
        state,
        episode_id,
        show_id,
        season_id,
        resolved.show_title(),
    )
    .await;
    let placement = match placement::episode(state, runtime, &naming).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let target_dir = placement.dir.clone();

    // Get Kobload base URL and API key
    let kobload_base_url = match runtime.kobload_base_url.clone() {
        Some(url) if !url.trim().is_empty() => url,
        _ => return (StatusCode::BAD_REQUEST, "kobload_base_url not configured").into_response(),
    };
    let api_key = match runtime.kobload_api_key.clone() {
        Some(key) if !key.trim().is_empty() => key,
        _ => return (StatusCode::BAD_REQUEST, "kobload_api_key not configured").into_response(),
    };

    if let Err(err) = fs::create_dir_all(&target_dir).await {
        tracing::error!(
            error = %err,
            path = %target_dir.display(),
            "Failed to create TV episode target directory"
        );
        let body = format!(
            "no files ingested (local): create tv episode dir {}: {err}",
            target_dir.display()
        );
        let _ =
            wanted_attempts::update_status(&state.pool, &download_id, "failed", Some(&body)).await;
        return (StatusCode::INTERNAL_SERVER_ERROR, body).into_response();
    }

    let client = crate::http::builder()
        .timeout(std::time::Duration::from_secs(86400))
        .read_timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client");

    let mut ingested_count = 0;
    let mut subtitle_paths: Vec<String> = Vec::new();
    let mut last_failure: Option<(FailureKind, String)> = None;

    for file in &media_files {
        let file_url = if let Some(ref url) = file.url {
            format!("{}{}", kobload_base_url.trim_end_matches('/'), url)
        } else {
            format!(
                "{}/api/v1/downloads/{}/files/{}",
                kobload_base_url.trim_end_matches('/'),
                payload.download_id,
                urlencoding::encode(&file.path)
            )
        };

        let file_path = placement.file_path(&file.extension, file.filename());
        let file_path = non_clobbering_path(file_path, link_mode).await;
        // Download to hidden temp file to prevent scanner from picking it up mid-transfer
        let temp_path = target_dir.join(format!(
            ".kobbler-dl-{}.{}",
            Uuid::now_v7(),
            file.extension.to_lowercase()
        ));

        tracing::info!(
            download_id = %payload.download_id,
            episode_id = %episode_id,
            path = %file.path,
            extension = %file.extension,
            "Downloading TV episode file from Kobload"
        );

        let response = match client.get(&file_url).bearer_auth(&api_key).send().await {
            Ok(resp) => resp,
            Err(err) => {
                tracing::error!(error = %err, path = %file.path, "Failed to fetch TV episode file from Kobload");
                note_failure(
                    &mut last_failure,
                    FailureKind::Upstream,
                    format!("fetch: {err}"),
                );
                continue;
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            tracing::error!(
                status = %status,
                path = %file.path,
                "Kobload TV episode file fetch failed"
            );
            note_failure(
                &mut last_failure,
                FailureKind::Upstream,
                format!("kobload fetch returned {status}"),
            );
            continue;
        }

        let mut out_file = match fs::File::create(&temp_path).await {
            Ok(f) => f,
            Err(err) => {
                tracing::error!(error = %err, "Failed to create TV episode temp file on disk");
                note_failure(
                    &mut last_failure,
                    FailureKind::Local,
                    format!("create temp file: {err}"),
                );
                continue;
            }
        };

        let mut body = response.bytes_stream();
        let mut write_failure: Option<(FailureKind, String)> = None;
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::warn!(download_id = %payload.download_id, "TV episode download cancelled");
                    write_failure = Some((FailureKind::Local, "download cancelled".to_string()));
                    break;
                }
                chunk = body.next() => chunk,
            };
            let Some(chunk) = chunk else { break };
            match chunk {
                Ok(bytes) => {
                    if let Err(err) = out_file.write_all(&bytes).await {
                        tracing::error!(error = %err, "Failed to write TV episode file chunk");
                        write_failure = Some((FailureKind::Local, format!("write chunk: {err}")));
                        break;
                    }
                }
                Err(err) => {
                    tracing::error!(error = %err, "Failed to read TV episode file stream");
                    write_failure = Some((FailureKind::Upstream, format!("read stream: {err}")));
                    break;
                }
            }
        }

        if let Some((kind, reason)) = write_failure {
            note_failure(&mut last_failure, kind, reason);
            let _ = fs::remove_file(&temp_path).await;
            continue;
        }

        // Finish and close the file before it is renamed or read. See
        // `finalize_download`.
        if let Err(err) = finalize_download(out_file).await {
            tracing::error!(error = %err, "Failed to finalize TV episode download");
            note_failure(
                &mut last_failure,
                FailureKind::Local,
                format!("finalize file: {err}"),
            );
            let _ = fs::remove_file(&temp_path).await;
            continue;
        }

        // Rename from hidden temp path to final template-derived name
        if let Err(err) = fs::rename(&temp_path, &file_path).await {
            tracing::error!(
                error = %err,
                from = %temp_path.display(),
                to = %file_path.display(),
                "Failed to rename TV episode temp file to final path"
            );
            note_failure(
                &mut last_failure,
                FailureKind::Local,
                format!("rename temp file: {err}"),
            );
            let _ = fs::remove_file(&temp_path).await;
            continue;
        }

        // Track subtitle paths separately
        if file.is_subtitle() {
            subtitle_paths.push(file_path.to_string_lossy().to_string());
            ingested_count += 1;
            continue;
        }

        // Create tv_episode_files record for video files
        let file_size = fs::metadata(&file_path).await.ok().map(|m| m.len() as i64);
        let source_metadata =
            build_source_provenance(payload.download_id, payload.release_info.as_ref(), None);

        // Extract video metadata from kobload's file_metadata
        let vm = file.file_metadata.as_ref().and_then(|m| m.video.as_ref());

        let ep_file_id = match tv_episode_files::create_or_update(
            &state.pool,
            tv_episode_files::EpisodeFileInput {
                file_path: file_path.to_string_lossy().to_string(),
                file_size,
                file_fingerprint: None,
                format: file.extension.clone(),
                quality: None,
                languages: None,
                subtitles: if subtitle_paths.is_empty() {
                    None
                } else {
                    Some(subtitle_paths.clone())
                },
                source_type: Some("kobload".to_string()),
                source_metadata: Some(source_metadata),
                video_codec: vm.map(|v| v.codec.clone()),
                video_width: vm.map(|v| v.width as i32),
                video_height: vm.map(|v| v.height as i32),
                video_bitrate_kbps: vm.and_then(|v| v.bitrate_kbps.map(|b| b as i32)),
                framerate: vm.and_then(|v| v.framerate),
                bit_depth: vm.and_then(|v| v.bit_depth.map(|b| b as i16)),
                is_hdr: vm.is_some_and(|v| v.is_hdr),
                hdr_format: vm.and_then(|v| v.hdr_format.clone()),
                is_interlaced: vm.is_some_and(|v| v.is_interlaced),
                duration_seconds: file.file_metadata.as_ref().and_then(|m| m.duration_seconds),
                container: file
                    .file_metadata
                    .as_ref()
                    .and_then(|m| m.container.clone()),
                overall_bitrate_kbps: file
                    .file_metadata
                    .as_ref()
                    .and_then(|m| m.bitrate_kbps.map(|b| b as i32)),
            },
        )
        .await
        {
            Ok(id) => id,
            Err(err) => {
                tracing::error!(error = %err, "Failed to create tv_episode_files record");
                note_failure(
                    &mut last_failure,
                    FailureKind::Local,
                    format!("create tv_episode_files row: {err}"),
                );
                continue;
            }
        };

        // Link file to episode via junction table
        match tv_episode_files::link_episodes_with_mode(
            &state.pool,
            ep_file_id,
            &[episode_id],
            link_mode,
        )
        .await
        {
            Ok(replaced) => crate::repositories::cleanup_replaced_files(&replaced).await,
            Err(err) => tracing::error!(error = %err, "Failed to link episode file to episode"),
        }

        // Insert audio and subtitle tracks from file metadata (inline so the UI
        // sees them immediately, before the probe job lands).
        if let Some(ref fm) = file.file_metadata {
            persist_tracks_for_episode_file(&state.pool, ep_file_id, fm).await;
        }

        // Create external_subtitles records for subtitle files
        if !subtitle_paths.is_empty() {
            for sub_path in &subtitle_paths {
                persist_external_subtitle(&state.pool, None, Some(ep_file_id), sub_path).await;
            }
        }

        placement::announce_file(
            state,
            &placement,
            &file_path,
            placement::FileKind::Episode,
            ep_file_id,
            episode_id,
        )
        .await;

        ingested_count += 1;
    }

    // Confirm import to Kobload so it can clean up.
    // Skip when processing via NATS job — the job handler sends the result via NATS instead.
    if ingested_count > 0 {
        if !skip_http_confirm {
            let confirm_url = format!(
                "{}/api/v1/downloads/{}/imported",
                kobload_base_url.trim_end_matches('/'),
                payload.download_id
            );
            if let Err(err) = client.post(&confirm_url).bearer_auth(&api_key).send().await {
                tracing::warn!(error = %err, "Failed to confirm TV episode import to Kobload");
            }
        }

        // Upgrade: now that new files are downloaded, clean up old files
        for old_file in &upgrade_old_files {
            tracing::info!(
                episode_id = %episode_id,
                old_file_id = %old_file.id,
                old_path = %old_file.file_path,
                quality_score = ?payload.quality_score,
                "Upgrade: marking old TV episode file as unavailable"
            );
            let _ = tv_episode_files::set_available(&state.pool, old_file.id, false).await;
            if let Err(e) = fs::remove_file(&old_file.file_path).await {
                tracing::warn!(
                    error = %e,
                    path = %old_file.file_path,
                    "Failed to delete old TV episode file during upgrade (may already be gone)"
                );
            }
        }

        // A terminal want_status: left on 'searching', the stale-watch reconciler re-wanted
        // already-imported events. kobload still drives quality upgrades on its own.
        placement::complete_episode(state, episode_id, show_id).await;

        // Update attempt status to completed
        let _ = wanted_attempts::update_status(&state.pool, &download_id, "completed", None).await;

        // Confirm want to Kobload (deletes pattern or starts upgrade window) and broadcast
        crate::wanted::on_entity_acquired(
            &state.runtime_config,
            &state.library_events,
            crate::wanted::SOURCE_SYSTEM,
            &episode_id.to_string(),
            episode_id,
            replaces_a_file,
        )
        .await;
    }

    if ingested_count == 0 {
        let (status, body) = failure_response_parts(last_failure.as_ref());
        let _ =
            wanted_attempts::update_status(&state.pool, &download_id, "failed", Some(&body)).await;
        let _ = state
            .library_events
            .send(LibraryEvent::wanted_status_updated(
                episode_id,
                "failed".to_string(),
            ));
        return (status, body).into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

/// Broadcast a season pack's status to the UI, and announce it once.
///
/// Every episode the pack carries is still broadcast: a pack may cover only part
/// of a season, so the episode rows are what the UI can safely draw, and marking
/// the whole season "downloading" would be wrong on screen.
///
/// The announcement is a different question. One download is one piece of news
/// however many episodes rode in it, and fanning it out per episode is the same
/// sentence a dozen times over — a dozen notifications, a dozen lines in
/// #releases, a dozen messages in whatever chat thread asked for it. So the
/// episodes go quietly and the season says it once. A pack carrying a single
/// episode is already one piece of news and says it itself.
fn announce_pack(
    library_events: &tokio::sync::broadcast::Sender<LibraryEvent>,
    season_id: Option<Uuid>,
    episodes: &[Uuid],
    status: &str,
) {
    let speaks_for_them = season_id.filter(|_| episodes.len() > 1);
    for ep_id in episodes {
        let _ = library_events.send(match speaks_for_them {
            Some(_) => LibraryEvent::wanted_status_updated_quietly(*ep_id, status.to_string()),
            None => LibraryEvent::wanted_status_updated(*ep_id, status.to_string()),
        });
    }
    if let Some(season_id) = speaks_for_them {
        let _ = library_events.send(LibraryEvent::wanted_status_updated(
            season_id,
            status.to_string(),
        ));
    }
}

/// Ingest a TV season download from Kobload.
///
/// Downloads video/subtitle files for an entire season, matches filenames to episodes via
/// episode_map, creates tv_episode_files records, and marks each matched episode as in-library.
/// Called from the source_type routing in `process_kobload_ingest_impl`.
#[allow(clippy::too_many_arguments)]
async fn ingest_tv_season(
    state: &AppState,
    runtime: &RuntimeConfig,
    payload: KobloadCallbackRequest,
    effective_status: &str,
    message: Option<&str>,
    skip_http_confirm: bool,
    cancel: tokio_util::sync::CancellationToken,
) -> axum::response::Response {
    let download_id = payload.download_id.to_string();
    let meta: IngestMetadata = serde_json::from_value(payload.metadata.clone()).unwrap_or_default();
    let resolved = meta.resolve();
    let link_mode = resolved.link_mode();

    // Update attempt status (same pattern as movie path)
    let attempt_status = if effective_status == "completed"
        || effective_status == "complete"
        || effective_status == "downloaded"
    {
        "ingesting".to_string()
    } else {
        effective_status.to_string()
    };
    let files_json = serde_json::to_value(&payload.files).ok();
    let _ = wanted_attempts::update_status_returning_with_idempotency(
        &state.pool,
        &download_id,
        &attempt_status,
        message,
        files_json,
        payload.idempotency_key,
    )
    .await;

    // Extract show_id, season_id, and episode_map from callback metadata
    // (check top-level then original_metadata for watch-system downloads)
    let show_id = resolved.show_id();
    let season_id = resolved.season_id();
    let episode_map: HashMap<String, Uuid> = resolved.episode_map();

    // Every episode is broadcast — a season download may only cover a subset of
    // them, and marking the whole season as "downloading" is wrong for the rows
    // the UI draws. The announcement is the pack's, once.
    announce_pack(
        &state.library_events,
        season_id,
        &episode_map.values().copied().collect::<Vec<_>>(),
        &attempt_status,
    );

    // If recoverable error, acknowledge but don't process files
    if payload.recoverable_error.is_some() {
        return not_imported_response();
    }

    // If not completed, reset want_status for affected episodes so re-search can find a replacement
    if effective_status != "completed"
        && effective_status != "complete"
        && effective_status != "downloaded"
    {
        let episode_ids: Vec<Uuid> = episode_map.values().copied().collect();
        if !episode_ids.is_empty() {
            let _ = tv_episodes::set_want_status_batch(&state.pool, &episode_ids, "wanted").await;
            if let Some(show_id) = show_id {
                let _ = state
                    .library_events
                    .send(LibraryEvent::tv_show_updated(show_id));
            }
        }
        return not_imported_response();
    }

    let season_id = match season_id {
        Some(id) => id,
        None => {
            tracing::error!(
                download_id = %payload.download_id,
                "TV season ingest callback missing season_id in metadata"
            );
            let _ = wanted_attempts::update_status(
                &state.pool,
                &download_id,
                "failed",
                Some("Missing season_id in callback metadata"),
            )
            .await;
            return (StatusCode::BAD_REQUEST, "missing season_id in metadata").into_response();
        }
    };

    // Get show details for directory naming and media root lookup
    let show_row = if let Some(sid) = show_id {
        tv_shows::get(&state.pool, sid).await.ok().flatten()
    } else {
        None
    };
    let show_title = show_row
        .as_ref()
        .map(|s| s.title.clone())
        .or_else(|| resolved.show_title().map(String::from))
        .unwrap_or_else(|| "Unknown Show".to_string());
    let show_year = show_row.as_ref().and_then(|s| s.year).map(|y| y as u32);

    let season_identifier = tv_seasons::get(&state.pool, season_id)
        .await
        .ok()
        .flatten()
        .map(|s| s.season_identifier)
        .unwrap_or_else(|| "1".to_string());

    // Filter files to video and subtitle extensions, excluding samples/trailers/extras
    let media_files: Vec<&KobloadFileInfo> = payload
        .files
        .iter()
        .filter(|f| (f.is_video() || f.is_subtitle()) && !f.is_non_content())
        .collect();

    if media_files.is_empty() {
        tracing::warn!(
            download_id = %payload.download_id,
            season_id = %season_id,
            "No video or subtitle files found in TV season download"
        );
        let _ = wanted_attempts::update_status(
            &state.pool,
            &download_id,
            "failed",
            Some("No video files found"),
        )
        .await;
        // Reset episodes back to "wanted" so they can be re-searched
        let episode_ids: Vec<Uuid> = episode_map.values().copied().collect();
        if !episode_ids.is_empty() {
            let _ = tv_episodes::set_want_status_batch(&state.pool, &episode_ids, "wanted").await;
        }
        announce_pack(
            &state.library_events,
            Some(season_id),
            &episode_ids,
            "failed",
        );
        if let Some(show_id) = show_id {
            let _ = state
                .library_events
                .send(LibraryEvent::tv_show_updated(show_id));
        }
        return (StatusCode::BAD_REQUEST, "no video files in download").into_response();
    }

    // Build target directory: use media root if show has one, fall back to default for type
    let entity_root_id = show_row.as_ref().and_then(|s| s.media_root_id);
    let media_root = if let Some(root_id) = entity_root_id {
        media_roots::get_enabled_by_id(&state.pool, root_id)
            .await
            .ok()
            .flatten()
    } else {
        let default = media_roots::get_default_for_type(&state.pool, "tv")
            .await
            .ok()
            .flatten();
        // Set media_root_id on the show if resolved from default
        if let Some(ref root) = default
            && let Some(sid) = show_id
        {
            let _ = tv_shows::update(
                &state.pool,
                sid,
                tv_shows::TvShowUpdate {
                    media_root_id: Some(Some(root.id)),
                    ..Default::default()
                },
            )
            .await;
        }
        default
    };

    // Check media root availability before any filesystem operations
    if let Some(ref root) = media_root
        && let Some(resp) = check_ingest_media_root(state, root).await
    {
        return resp;
    }

    let season_num: u32 = season_identifier.parse().unwrap_or(1);

    // For season packs, compute directory-only for download phase.
    // Per-episode renaming happens after episode matching.
    let tv_template = media_root.as_ref().map(|root| {
        root.naming_template
            .as_deref()
            .unwrap_or_else(|| media_root_naming::default_tv_template())
            .to_string()
    });
    // Look up existing show directory once — reused for season dir and per-episode renaming
    let existing_show_dir = if let Some(ref root) = media_root
        && let Some(sid) = show_id
    {
        media_root_naming::find_existing_file_for_entity(&state.pool, root.id, "tv_show", sid)
            .await
            .and_then(|f| media_root_naming::extract_dir_from_existing(&root.path, &f, 1))
    } else {
        None
    };

    let target_dir = if let Some(ref root) = media_root {
        let template = tv_template.as_deref().unwrap();
        let ctx = NamingContext {
            show_title: Some(show_title.clone()),
            year: show_year,
            season: Some(season_num),
            episode: Some(1), // placeholder for directory portion
            ext: Some("mkv".to_string()),
            ..Default::default()
        };
        let fresh = media_root_naming::compute_target_path(&root.path, template, &ctx);
        let full_path = if let Some(ref show_dir) = existing_show_dir {
            media_root_naming::reuse_existing_dir(&root.path, show_dir, &fresh, 1)
        } else {
            fresh
        };
        full_path.parent().unwrap_or(&full_path).to_path_buf()
    } else {
        let video_root = runtime
            .video_storage_root
            .clone()
            .unwrap_or_else(|| format!("{}/video", state.config.storage_root));
        PathBuf::from(&video_root)
            .join("tv")
            .join(sanitize_filename(&show_title))
            .join(format!("Season {}", season_identifier))
    };

    // Get Kobload base URL and API key
    let kobload_base_url = match runtime.kobload_base_url.clone() {
        Some(url) if !url.trim().is_empty() => url,
        _ => return (StatusCode::BAD_REQUEST, "kobload_base_url not configured").into_response(),
    };
    let api_key = match runtime.kobload_api_key.clone() {
        Some(key) if !key.trim().is_empty() => key,
        _ => return (StatusCode::BAD_REQUEST, "kobload_api_key not configured").into_response(),
    };

    if let Err(err) = fs::create_dir_all(&target_dir).await {
        tracing::error!(
            error = %err,
            path = %target_dir.display(),
            "Failed to create TV season target directory"
        );
        let body = format!(
            "no files ingested (local): create tv season dir {}: {err}",
            target_dir.display()
        );
        let _ =
            wanted_attempts::update_status(&state.pool, &download_id, "failed", Some(&body)).await;
        return (StatusCode::INTERNAL_SERVER_ERROR, body).into_response();
    }

    let client = crate::http::builder()
        .timeout(std::time::Duration::from_secs(86400))
        .read_timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client");

    // Episodes already in the library before this pack: a file landing on one of
    // them is a replacement (see the single-episode path), whatever Kobload calls it.
    let had_files: std::collections::HashSet<Uuid> =
        tv_episode_files::episodes_with_files(&state.pool, season_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();

    let video_file_count = media_files.iter().filter(|f| f.is_video()).count();
    let mut ingested_count = 0;
    let mut matched_episodes: Vec<Uuid> = Vec::new();
    let mut subtitle_paths: Vec<String> = Vec::new();
    let mut last_failure: Option<(FailureKind, String)> = None;

    for file in &media_files {
        let file_url = if let Some(ref url) = file.url {
            format!("{}{}", kobload_base_url.trim_end_matches('/'), url)
        } else {
            format!(
                "{}/api/v1/downloads/{}/files/{}",
                kobload_base_url.trim_end_matches('/'),
                payload.download_id,
                urlencoding::encode(&file.path)
            )
        };

        // Download to hidden temp file to prevent scanner from picking it up mid-transfer
        let temp_path = target_dir.join(format!(
            ".kobbler-dl-{}.{}",
            Uuid::now_v7(),
            file.extension.to_lowercase()
        ));

        tracing::info!(
            download_id = %payload.download_id,
            season_id = %season_id,
            path = %file.path,
            extension = %file.extension,
            "Downloading TV season file from Kobload"
        );

        let response = match client.get(&file_url).bearer_auth(&api_key).send().await {
            Ok(resp) => resp,
            Err(err) => {
                tracing::error!(error = %err, path = %file.path, "Failed to fetch TV season file from Kobload");
                note_failure(
                    &mut last_failure,
                    FailureKind::Upstream,
                    format!("fetch: {err}"),
                );
                continue;
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            tracing::error!(
                status = %status,
                path = %file.path,
                "Kobload TV season file fetch failed"
            );
            note_failure(
                &mut last_failure,
                FailureKind::Upstream,
                format!("kobload fetch returned {status}"),
            );
            continue;
        }

        let mut out_file = match fs::File::create(&temp_path).await {
            Ok(f) => f,
            Err(err) => {
                tracing::error!(error = %err, "Failed to create TV season temp file on disk");
                note_failure(
                    &mut last_failure,
                    FailureKind::Local,
                    format!("create temp file: {err}"),
                );
                continue;
            }
        };

        let mut body = response.bytes_stream();
        let mut write_failure: Option<(FailureKind, String)> = None;
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::warn!(download_id = %payload.download_id, "TV season download cancelled");
                    write_failure = Some((FailureKind::Local, "download cancelled".to_string()));
                    break;
                }
                chunk = body.next() => chunk,
            };
            let Some(chunk) = chunk else { break };
            match chunk {
                Ok(bytes) => {
                    if let Err(err) = out_file.write_all(&bytes).await {
                        tracing::error!(error = %err, "Failed to write TV season file chunk");
                        write_failure = Some((FailureKind::Local, format!("write chunk: {err}")));
                        break;
                    }
                }
                Err(err) => {
                    tracing::error!(error = %err, "Failed to read TV season file stream");
                    write_failure = Some((FailureKind::Upstream, format!("read stream: {err}")));
                    break;
                }
            }
        }

        if let Some((kind, reason)) = write_failure {
            note_failure(&mut last_failure, kind, reason);
            let _ = fs::remove_file(&temp_path).await;
            continue;
        }

        // Finish and close the file before it is renamed or read. See
        // `finalize_download`.
        if let Err(err) = finalize_download(out_file).await {
            tracing::error!(error = %err, "Failed to finalize TV season download");
            note_failure(
                &mut last_failure,
                FailureKind::Local,
                format!("finalize file: {err}"),
            );
            let _ = fs::remove_file(&temp_path).await;
            continue;
        }

        // For subtitles: rename from temp to sanitized name (no episode-specific naming needed yet)
        if file.is_subtitle() {
            let sub_path = target_dir.join(sanitize_filename(file.filename()));
            if let Err(err) = fs::rename(&temp_path, &sub_path).await {
                tracing::warn!(error = %err, "Failed to rename subtitle temp file");
                note_failure(
                    &mut last_failure,
                    FailureKind::Local,
                    format!("rename subtitle temp file: {err}"),
                );
                let _ = fs::remove_file(&temp_path).await;
            } else {
                subtitle_paths.push(sub_path.to_string_lossy().to_string());
            }
            ingested_count += 1;
            continue;
        }

        // Determine episode number from Kobload's structured data.
        // Priority: per-file matched_episode > release_info.matched_target > legacy fallback
        let ep_numbers: Vec<u32> = if let Some(ref ep_str) = file.matched_episode
            && let Ok(ep_num) = ep_str.parse::<u32>()
        {
            // Per-file identification from Kobload's classification
            vec![ep_num]
        } else if let Some(ref ri) = payload.release_info
            && let Some(nats_jobs::MatchedTarget::Episode { ref episode, .. }) = ri.matched_target
            && let Ok(ep_num) = episode.parse::<u32>()
            && video_file_count == 1
        {
            // Release-level match target (safe only for single-file downloads)
            tracing::info!(
                filename = %file.filename(),
                episode = ep_num,
                "Using release_info.matched_target for episode identification"
            );
            vec![ep_num]
        } else if let Some(ref ri) = payload.release_info
            && let Some(nats_jobs::MatchedTarget::SceneEpisode { tvdb_episode, .. }) =
                ri.matched_target
            && video_file_count == 1
        {
            // XEM scene-mapped episode
            tracing::info!(
                filename = %file.filename(),
                episode = tvdb_episode,
                "Using release_info.matched_target (scene episode) for episode identification"
            );
            vec![tvdb_episode]
        } else {
            tracing::error!(
                filename = %file.filename(),
                download_id = %payload.download_id,
                has_matched_episode = file.matched_episode.is_some(),
                has_release_info = payload.release_info.is_some(),
                video_file_count = video_file_count,
                episode_map_len = episode_map.len(),
                "No structured episode data from Kobload — cannot determine which episode this file belongs to"
            );
            note_failure(
                &mut last_failure,
                FailureKind::Local,
                format!("no episode classification for {}", file.filename()),
            );
            let _ = fs::remove_file(&temp_path).await;
            continue;
        };

        // Rename from temp to template-derived name using the first episode number
        let first_ep = ep_numbers[0];
        let file_path = if let Some(ref root) = media_root {
            let template = tv_template
                .as_deref()
                .unwrap_or_else(|| media_root_naming::default_tv_template());
            let ep_row = tv_episodes::get_by_season_and_identifier(
                &state.pool,
                season_id,
                &first_ep.to_string(),
            )
            .await
            .ok()
            .flatten();
            let ep_num_val = ep_row
                .as_ref()
                .map(|e| e.episode_sort_order as u32)
                .unwrap_or(first_ep as u32);
            let ep_title_val = ep_row.as_ref().and_then(|e| e.title.clone());
            let ctx = NamingContext {
                show_title: Some(show_title.clone()),
                year: show_year,
                season: Some(season_num),
                episode: Some(ep_num_val),
                episode_title: ep_title_val,
                ext: Some(file.extension.to_lowercase()),
                ..Default::default()
            };
            let fresh = media_root_naming::compute_target_path(&root.path, template, &ctx);
            let named = if let Some(ref show_dir) = existing_show_dir {
                media_root_naming::reuse_existing_dir(&root.path, show_dir, &fresh, 1)
            } else {
                fresh
            };
            let named = non_clobbering_path(named, link_mode).await;
            if let Some(parent) = named.parent() {
                let _ = fs::create_dir_all(parent).await;
            }
            match fs::rename(&temp_path, &named).await {
                Ok(()) => named,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        from = %temp_path.display(),
                        to = %named.display(),
                        "Failed to rename season file to template name, keeping sanitized name"
                    );
                    let fallback = target_dir.join(sanitize_filename(file.filename()));
                    if let Err(e2) = fs::rename(&temp_path, &fallback).await {
                        tracing::error!(error = %e2, "Failed to rename season temp file at all");
                        note_failure(
                            &mut last_failure,
                            FailureKind::Local,
                            format!("rename temp file: {e2}"),
                        );
                        let _ = fs::remove_file(&temp_path).await;
                        continue;
                    }
                    fallback
                }
            }
        } else {
            let fallback = target_dir.join(sanitize_filename(file.filename()));
            if let Err(err) = fs::rename(&temp_path, &fallback).await {
                tracing::error!(error = %err, "Failed to rename season temp file");
                note_failure(
                    &mut last_failure,
                    FailureKind::Local,
                    format!("rename temp file: {err}"),
                );
                let _ = fs::remove_file(&temp_path).await;
                continue;
            }
            fallback
        };

        let file_size = fs::metadata(&file_path).await.ok().map(|m| m.len() as i64);
        let source_metadata =
            build_source_provenance(payload.download_id, payload.release_info.as_ref(), None);

        // Extract video metadata from kobload's file_metadata
        let vm = file.file_metadata.as_ref().and_then(|m| m.video.as_ref());

        // Resolve all episode UUIDs for this file
        let mut episode_ids = Vec::new();
        for ep_num in &ep_numbers {
            let ep_key = ep_num.to_string();
            let episode_id = if let Some(&eid) = episode_map.get(&ep_key) {
                Some(eid)
            } else {
                tv_episodes::get_by_season_and_identifier(&state.pool, season_id, &ep_key)
                    .await
                    .ok()
                    .flatten()
                    .map(|e| e.id)
            };
            if let Some(eid) = episode_id {
                episode_ids.push(eid);
            } else {
                tracing::warn!(
                    episode_number = ep_num,
                    season_id = %season_id,
                    "No matching episode found for episode number"
                );
            }
        }

        if episode_ids.is_empty() {
            note_failure(
                &mut last_failure,
                FailureKind::Local,
                format!("no matching episode rows for {:?}", ep_numbers),
            );
            continue;
        }

        let episode_file_id = match tv_episode_files::create_or_update(
            &state.pool,
            tv_episode_files::EpisodeFileInput {
                file_path: file_path.to_string_lossy().to_string(),
                file_size,
                file_fingerprint: None,
                format: file.extension.clone(),
                quality: None,
                languages: None,
                subtitles: if subtitle_paths.is_empty() {
                    None
                } else {
                    Some(subtitle_paths.clone())
                },
                source_type: Some("kobload".to_string()),
                source_metadata: Some(source_metadata.clone()),
                video_codec: vm.map(|v| v.codec.clone()),
                video_width: vm.map(|v| v.width as i32),
                video_height: vm.map(|v| v.height as i32),
                video_bitrate_kbps: vm.and_then(|v| v.bitrate_kbps.map(|b| b as i32)),
                framerate: vm.and_then(|v| v.framerate),
                bit_depth: vm.and_then(|v| v.bit_depth.map(|b| b as i16)),
                is_hdr: vm.is_some_and(|v| v.is_hdr),
                hdr_format: vm.and_then(|v| v.hdr_format.clone()),
                is_interlaced: vm.is_some_and(|v| v.is_interlaced),
                duration_seconds: file.file_metadata.as_ref().and_then(|m| m.duration_seconds),
                container: file
                    .file_metadata
                    .as_ref()
                    .and_then(|m| m.container.clone()),
                overall_bitrate_kbps: file
                    .file_metadata
                    .as_ref()
                    .and_then(|m| m.bitrate_kbps.map(|b| b as i32)),
            },
        )
        .await
        {
            Ok(id) => id,
            Err(err) => {
                tracing::error!(error = %err, "Failed to create tv_episode_files record");
                note_failure(
                    &mut last_failure,
                    FailureKind::Local,
                    format!("create tv_episode_files row: {err}"),
                );
                continue;
            }
        };

        // Link file to all episodes via junction table
        match tv_episode_files::link_episodes_with_mode(
            &state.pool,
            episode_file_id,
            &episode_ids,
            link_mode,
        )
        .await
        {
            Ok(replaced) => crate::repositories::cleanup_replaced_files(&replaced).await,
            Err(err) => tracing::error!(error = %err, "Failed to link episode file to episodes"),
        }

        // Insert audio and subtitle tracks once (inline so the UI sees them
        // immediately, before the probe job lands).
        if let Some(ref fm) = file.file_metadata {
            persist_tracks_for_episode_file(&state.pool, episode_file_id, fm).await;
        }

        // Always enqueue media_probe. Even when kobload supplied metadata we still
        // need this to chain media_extract + media_analyze + skip_detect — those
        // hops only fire from media_probe.
        {
            let probe_display =
                crate::jobs::fetch_job_display(&state.pool, "tv_episode_file", episode_file_id)
                    .await;
            let probe_context = crate::jobs::JobContext::for_entity(
                "tv_episode_file",
                episode_file_id,
                "Probing media file".to_string(),
            )
            .with_display_opt(probe_display);
            let _ = crate::jobs::enqueue(
                &state.pool,
                state.nats.publisher(),
                "media_probe",
                serde_json::json!({"file_id": episode_file_id, "media_type": "episode"}),
                probe_context,
            )
            .await;
        }

        for &episode_id in &episode_ids {
            let _ = tv_episodes::clear_rejected_releases(&state.pool, episode_id).await;

            if !matched_episodes.contains(&episode_id) {
                matched_episodes.push(episode_id);
            }
        }

        // Create external_subtitles records linked to this episode file
        if !subtitle_paths.is_empty() {
            for sub_path in &subtitle_paths {
                persist_external_subtitle(&state.pool, None, Some(episode_file_id), sub_path).await;
            }
        }

        // Register in media_root_files so the file appears as 'added' immediately
        // Link to the primary episode (first ep_number) for this physical file
        if let Some(ref root) = media_root
            && let Some(first_ep_id) = episode_map.get(&ep_numbers[0].to_string()).copied()
        {
            let fname = file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let fsize = fs::metadata(&file_path).await.ok().map(|m| m.len() as i64);
            if let Err(e) = media_root_files::upsert_added(
                &state.pool,
                root.id,
                &file_path.to_string_lossy(),
                &fname,
                fsize,
                "tv_episode",
                first_ep_id,
            )
            .await
            {
                tracing::warn!(error = %e, "Failed to register TV season file in media_root_files");
            }
        }

        ingested_count += 1;
    }

    // Confirm import to Kobload so it can clean up.
    // Skip when processing via NATS job — the job handler sends the result via NATS instead.
    if ingested_count > 0 {
        if !skip_http_confirm {
            let confirm_url = format!(
                "{}/api/v1/downloads/{}/imported",
                kobload_base_url.trim_end_matches('/'),
                payload.download_id
            );
            if let Err(err) = client.post(&confirm_url).bearer_auth(&api_key).send().await {
                tracing::warn!(error = %err, "Failed to confirm TV season import to Kobload");
            }
        }

        // Move every episode the pack actually delivered to a terminal want_status,
        // mirroring the single-episode path. Without this they stay at 'searching'
        // even though the file is on disk, which leaves the season looking
        // outstanding to every want/reconcile predicate downstream.
        if !matched_episodes.is_empty() {
            let _ = tv_episodes::set_want_status_batch(&state.pool, &matched_episodes, "imported")
                .await;
        }

        // Update attempt status to completed
        let _ = wanted_attempts::update_status(&state.pool, &download_id, "completed", None).await;

        // Confirm the season-level want to Kobload (watch pattern cleanup) and let
        // the pack announce itself once, as the season. `false` back means either
        // that there was only ever one episode to talk about, or that Kobload
        // still has episodes outstanding on this pattern — the season is not here,
        // so saying it is would claim episodes nobody has. Either way the episodes
        // go back to speaking for themselves.
        let announced_as_season = crate::wanted::on_entity_acquired_announcing(
            &state.runtime_config,
            &state.library_events,
            crate::wanted::SOURCE_SYSTEM,
            &season_id.to_string(),
            season_id,
            matched_episodes.len() > 1,
            payload.is_upgrade || matched_episodes.iter().all(|ep| had_files.contains(ep)),
        )
        .await;
        // Confirm each matched episode. The UI hears every one of them either way.
        for ep_id in &matched_episodes {
            crate::wanted::on_entity_acquired_announcing(
                &state.runtime_config,
                &state.library_events,
                crate::wanted::SOURCE_SYSTEM,
                &ep_id.to_string(),
                *ep_id,
                !announced_as_season,
                payload.is_upgrade || had_files.contains(ep_id),
            )
            .await;
        }

        // The activity feed follows the announcement. A feed that lists ten
        // episodes beside a notification that said "Season 2" is two accounts of
        // one download, and the one with ten rows in it buries the rest of the
        // day. Whoever asked for the season asked for every episode in it, so
        // the attribution comes off any of them.
        match announced_as_season {
            true => {
                let requester = match matched_episodes.first() {
                    Some(first) => {
                        crate::routes::library::helpers::requester_of(&state.pool, *first).await
                    }
                    None => None,
                };
                crate::routes::library::helpers::record_activity_for_entity(
                    &state.pool,
                    "imported",
                    season_id,
                    requester,
                )
                .await;
            }
            false => {
                for ep_id in &matched_episodes {
                    crate::routes::library::helpers::record_activity_for_entity(
                        &state.pool,
                        "imported",
                        *ep_id,
                        crate::routes::library::helpers::requester_of(&state.pool, *ep_id).await,
                    )
                    .await;
                }
            }
        }

        // Reset episodes that were in this download's episode_map but didn't match any files,
        // so they go back to "wanted" and can be re-searched individually.
        let unmatched_ids: Vec<Uuid> = episode_map
            .values()
            .filter(|id| !matched_episodes.contains(id))
            .copied()
            .collect();
        if !unmatched_ids.is_empty() {
            let _ = tv_episodes::set_want_status_batch(&state.pool, &unmatched_ids, "wanted").await;
        }

        // Write sidecar and image files for the show and each episode
        if let Some(sid) = show_id {
            crate::sidecar::write_back::write_tvshow_sidecars(&state.pool, sid).await;
            crate::sidecar::write_back::write_tvshow_images(
                &state.pool,
                &state.config.storage_root,
                sid,
            )
            .await;
        }

        // Push the new library state to Kobload immediately. `confirm_want` above
        // carries no body, so it cannot correct Kobload's stored `episodes_needed`
        // — and that snapshot is what its backlog search reads to decide which
        // episodes are still missing. Without this the correction waits on the
        // hourly drift reconciler, and in the meantime a backlog pass re-downloads
        // episodes this pack just delivered.
        if let Some(sid) = show_id
            && let Err(e) = crate::jobs::handlers::auto_want::enqueue_tv_want_refresh(
                state.nats.publisher(),
                sid,
                season_id,
            )
            .await
        {
            tracing::warn!(
                error = %e,
                season_id = %season_id,
                "Failed to refresh Kobload want after season ingest"
            );
        }

        tracing::info!(
            download_id = %payload.download_id,
            season_id = %season_id,
            matched_episodes = matched_episodes.len(),
            ingested_files = ingested_count,
            "TV season ingest complete"
        );

        // Notify frontend so the TV show page refetches updated episode statuses
        if let Some(show_id) = show_id {
            let _ = state
                .library_events
                .send(LibraryEvent::tv_show_updated(show_id));
        }
    }

    if ingested_count == 0 {
        let (status, body) = failure_response_parts(last_failure.as_ref());
        let _ =
            wanted_attempts::update_status(&state.pool, &download_id, "failed", Some(&body)).await;
        // Reset episodes back to "wanted" so they can be re-searched
        let episode_ids: Vec<Uuid> = episode_map.values().copied().collect();
        if !episode_ids.is_empty() {
            let _ = tv_episodes::set_want_status_batch(&state.pool, &episode_ids, "wanted").await;
        }
        announce_pack(
            &state.library_events,
            Some(season_id),
            &episode_ids,
            "failed",
        );
        if let Some(show_id) = show_id {
            let _ = state
                .library_events
                .send(LibraryEvent::tv_show_updated(show_id));
        }
        return (status, body).into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

// =============================================================================
// Track Persistence Helpers
// =============================================================================

/// Persist audio and subtitle tracks from kobload metadata for a movie file.
async fn persist_tracks_for_movie_file(
    pool: &sqlx::PgPool,
    movie_file_id: uuid::Uuid,
    fm: &nats_jobs::FileMetadata,
) {
    // Delete existing tracks before inserting (supports re-ingest without duplicates)
    let _ = audio_tracks::delete_by_movie_file(pool, movie_file_id).await;
    let _ = subtitle_tracks::delete_by_movie_file(pool, movie_file_id).await;

    // Audio tracks — use stream_index from kobload metadata when available,
    // otherwise fall back to enumerate index offset by 1 (video stream is 0).
    let audio_inputs: Vec<audio_tracks::AudioTrackInput> = fm
        .audio
        .iter()
        .enumerate()
        .map(|(i, a)| audio_tracks::AudioTrackInput {
            movie_file_id: Some(movie_file_id),
            episode_file_id: None,
            track_index: a.stream_index.map_or((i + 1) as i32, |idx| idx as i32),
            codec: a.codec.clone(),
            channels: a.channels as i16,
            bitrate_kbps: a.bitrate_kbps.map(|b| b as i32),
            sample_rate: a.sample_rate.map(|s| s as i32),
            language: a.language.as_ref().map(|l| normalize_language(l)),
            title: a.title.clone(),
            is_default: i == 0,
        })
        .collect();

    if !audio_inputs.is_empty()
        && let Err(err) = audio_tracks::create_batch(pool, &audio_inputs).await
    {
        tracing::warn!(error = %err, "Failed to insert audio tracks for movie file");
    }

    // Subtitle tracks — use stream_index from kobload metadata when available,
    // otherwise fall back to enumerate index offset by 1 (video) + audio count.
    let audio_count = fm.audio.len();
    let sub_inputs: Vec<subtitle_tracks::SubtitleTrackInput> = fm
        .subtitles
        .iter()
        .enumerate()
        .map(|(i, s)| subtitle_tracks::SubtitleTrackInput {
            movie_file_id: Some(movie_file_id),
            episode_file_id: None,
            track_index: s
                .stream_index
                .map_or((i + 1 + audio_count) as i32, |idx| idx as i32),
            codec: s.codec.clone(),
            language: s.language.as_ref().map(|l| normalize_language(l)),
            title: s.title.clone(),
            is_forced: s.is_forced,
            is_default: s.is_default,
            is_hearing_impaired: false,
        })
        .collect();

    if !sub_inputs.is_empty()
        && let Err(err) = subtitle_tracks::create_batch(pool, &sub_inputs).await
    {
        tracing::warn!(error = %err, "Failed to insert subtitle tracks for movie file");
    }
}

/// Persist audio and subtitle tracks from kobload metadata for an episode file.
async fn persist_tracks_for_episode_file(
    pool: &sqlx::PgPool,
    episode_file_id: uuid::Uuid,
    fm: &nats_jobs::FileMetadata,
) {
    // Delete existing tracks before inserting (supports re-ingest without duplicates)
    let _ = audio_tracks::delete_by_episode_file(pool, episode_file_id).await;
    let _ = subtitle_tracks::delete_by_episode_file(pool, episode_file_id).await;

    // Audio tracks — use stream_index from kobload metadata when available,
    // otherwise fall back to enumerate index offset by 1 (video stream is 0).
    let audio_inputs: Vec<audio_tracks::AudioTrackInput> = fm
        .audio
        .iter()
        .enumerate()
        .map(|(i, a)| audio_tracks::AudioTrackInput {
            movie_file_id: None,
            episode_file_id: Some(episode_file_id),
            track_index: a.stream_index.map_or((i + 1) as i32, |idx| idx as i32),
            codec: a.codec.clone(),
            channels: a.channels as i16,
            bitrate_kbps: a.bitrate_kbps.map(|b| b as i32),
            sample_rate: a.sample_rate.map(|s| s as i32),
            language: a.language.as_ref().map(|l| normalize_language(l)),
            title: a.title.clone(),
            is_default: i == 0,
        })
        .collect();

    if !audio_inputs.is_empty()
        && let Err(err) = audio_tracks::create_batch(pool, &audio_inputs).await
    {
        tracing::warn!(error = %err, "Failed to insert audio tracks for episode file");
    }

    // Subtitle tracks — use stream_index from kobload metadata when available,
    // otherwise fall back to enumerate index offset by 1 (video) + audio count.
    let audio_count = fm.audio.len();
    let sub_inputs: Vec<subtitle_tracks::SubtitleTrackInput> = fm
        .subtitles
        .iter()
        .enumerate()
        .map(|(i, s)| subtitle_tracks::SubtitleTrackInput {
            movie_file_id: None,
            episode_file_id: Some(episode_file_id),
            track_index: s
                .stream_index
                .map_or((i + 1 + audio_count) as i32, |idx| idx as i32),
            codec: s.codec.clone(),
            language: s.language.as_ref().map(|l| normalize_language(l)),
            title: s.title.clone(),
            is_forced: s.is_forced,
            is_default: s.is_default,
            is_hearing_impaired: false,
        })
        .collect();

    if !sub_inputs.is_empty()
        && let Err(err) = subtitle_tracks::create_batch(pool, &sub_inputs).await
    {
        tracing::warn!(error = %err, "Failed to insert subtitle tracks for episode file");
    }
}

/// Parse subtitle filename hints for language, forced, and hearing-impaired flags.
/// Handles common naming conventions like: `movie.en.forced.srt`, `movie.en.sdh.srt`
pub fn parse_subtitle_filename_hints(filename: &str) -> (Option<String>, bool, bool) {
    let stem = std::path::Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);

    let parts: Vec<&str> = stem.rsplit('.').collect();

    let mut language = None;
    let mut is_forced = false;
    let mut is_hi = false;

    for part in &parts {
        let lower = part.to_lowercase();
        match lower.as_str() {
            "forced" | "force" => is_forced = true,
            "sdh" | "hi" | "cc" => is_hi = true,
            _ if (lower.len() == 2 || lower.len() == 3)
                // Likely a language code (en, eng, fr, fre, etc.)
                && language.is_none() =>
            {
                language = Some(lower);
            }
            _ => {}
        }
    }

    (language, is_forced, is_hi)
}

/// Create an external_subtitles record for a subtitle file alongside a video.
async fn persist_external_subtitle(
    pool: &sqlx::PgPool,
    movie_file_id: Option<uuid::Uuid>,
    episode_file_id: Option<uuid::Uuid>,
    subtitle_path: &str,
) {
    let filename = std::path::Path::new(subtitle_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(subtitle_path);

    let (lang, is_forced, is_hi) = parse_subtitle_filename_hints(filename);
    let lang = lang.map(|l| normalize_language(&l));

    let ext = std::path::Path::new(subtitle_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("srt")
        .to_lowercase();

    let file_size = tokio::fs::metadata(subtitle_path)
        .await
        .ok()
        .map(|m| m.len() as i64);

    let input = external_subtitles::ExternalSubtitleInput {
        movie_file_id,
        episode_file_id,
        file_path: subtitle_path.to_string(),
        file_size,
        format: ext,
        language: lang,
        title: None,
        is_forced,
        is_hearing_impaired: is_hi,
        source: "local".to_string(),
        source_id: None,
        source_metadata: None,
    };

    if let Err(err) = external_subtitles::upsert_by_path(pool, &input).await {
        tracing::warn!(error = %err, path = subtitle_path, "Failed to create external subtitle record");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a pack says, and to whom. Every episode reaches the UI; the news is
    /// the season's, once.
    #[test]
    fn a_pack_broadcasts_every_episode_and_announces_once() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);
        let season_id = Uuid::now_v7();
        let episodes: Vec<Uuid> = (0..3).map(|_| Uuid::now_v7()).collect();

        announce_pack(&tx, Some(season_id), &episodes, "downloading");

        let mut announced = Vec::new();
        let mut quiet = Vec::new();
        while let Ok(LibraryEvent::WantedStatusUpdated {
            wanted_id,
            status,
            announce,
            ..
        }) = rx.try_recv()
        {
            assert_eq!(status, "downloading");
            match announce {
                true => announced.push(wanted_id),
                false => quiet.push(wanted_id),
            }
        }

        assert_eq!(
            quiet, episodes,
            "the UI draws episode rows, so it gets them"
        );
        assert_eq!(
            announced,
            vec![season_id],
            "one download is one piece of news",
        );
    }

    /// A pack of one is already one piece of news, and the episode is what it is
    /// about — "Season 1 (13 episodes)" would name twelve nobody has.
    #[test]
    fn a_single_episode_pack_speaks_for_itself() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);
        let episode_id = Uuid::now_v7();

        announce_pack(&tx, Some(Uuid::now_v7()), &[episode_id], "downloading");

        let sent: Vec<LibraryEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(sent.len(), 1);
        assert!(matches!(
            &sent[0],
            LibraryEvent::WantedStatusUpdated {
                wanted_id,
                announce: true,
                ..
            } if *wanted_id == episode_id
        ));
    }

    #[test]
    fn test_sanitize_filename_smb_unsafe_chars() {
        assert_eq!(
            sanitize_filename("WHAT HAVE I DONE?.epub"),
            "WHAT HAVE I DONE.epub"
        );
        assert_eq!(sanitize_filename("Star*Wars.mkv"), "StarWars.mkv");
        assert_eq!(
            sanitize_filename(r#"He "said" hello.pdf"#),
            "He said hello.pdf"
        );
        assert_eq!(sanitize_filename("<intro>.mp3"), "intro.mp3");
        assert_eq!(sanitize_filename("foo|bar.m4b"), "foo-bar.m4b");
        assert_eq!(
            sanitize_filename("Title: Subtitle.epub"),
            "Title - Subtitle.epub"
        );
    }

    /// The producer stamps the key, the consumer reads a serde field with the
    /// same name, and the two live in different files — so pin them together.
    #[test]
    fn regrab_intent_survives_the_round_trip_to_ingest() {
        let tagged = crate::file_link_mode::tag_metadata(
            serde_json::json!({ "source_type": "tv_episode" }),
            FileLinkMode::AddCopy,
        );
        let meta: IngestMetadata = serde_json::from_value(tagged).unwrap();
        assert_eq!(meta.resolve().link_mode(), FileLinkMode::AddCopy);
    }

    /// A want-created download arrives with kobbler's own metadata nested one
    /// level down, so the intent has to be resolved through that fallback too.
    #[test]
    fn regrab_intent_survives_nested_want_metadata() {
        let inner = crate::file_link_mode::tag_metadata(
            serde_json::json!({ "source_type": "tv_season" }),
            FileLinkMode::AddCopy,
        );
        let meta: IngestMetadata =
            serde_json::from_value(serde_json::json!({ "original_metadata": inner })).unwrap();
        assert_eq!(meta.resolve().link_mode(), FileLinkMode::AddCopy);
    }

    /// Everything that is not a regrab — schedulers, RSS, re-imports, and every
    /// callback minted before this field existed — must still mean replace.
    #[test]
    fn absent_or_false_intent_means_replace() {
        let absent: IngestMetadata =
            serde_json::from_value(serde_json::json!({ "source_type": "movie" })).unwrap();
        assert_eq!(absent.resolve().link_mode(), FileLinkMode::Replace);

        let explicit: IngestMetadata = serde_json::from_value(crate::file_link_mode::tag_metadata(
            serde_json::json!({ "source_type": "movie" }),
            FileLinkMode::Replace,
        ))
        .unwrap();
        assert_eq!(explicit.resolve().link_mode(), FileLinkMode::Replace);
    }

    #[test]
    fn test_sanitize_filename_preserves_existing_behavior() {
        // Slashes replaced with hyphens
        assert_eq!(sanitize_filename("foo/bar.epub"), "foo-bar.epub");
        assert_eq!(sanitize_filename("foo\\bar.epub"), "foo-bar.epub");
        // Null bytes truncate
        assert_eq!(sanitize_filename("foo\0bar.epub"), "foo");
        // Empty becomes default
        assert_eq!(sanitize_filename(""), "download.epub");
        // Whitespace-only becomes default
        assert_eq!(sanitize_filename("   "), "download.epub");
    }
}
