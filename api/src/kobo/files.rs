//! File serving for book downloads and covers.

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::path::PathBuf;
use tokio::fs::{self, File};
use tokio::task;
use tracing::warn;
use uuid::Uuid;

use crate::repositories::{authors, book_files, books};

/// Cover variant parameters.
#[derive(Clone, Copy)]
pub struct CoverVariant {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub greyscale: bool,
    pub quality: u8,
}

/// Result of preparing a download.
pub struct DownloadInfo {
    pub file: File,
    pub file_size: usize,
    pub content_type: &'static str,
    #[allow(dead_code)]
    pub filename: String,
    pub content_disposition: String,
    /// The source file path, for logging purposes
    pub file_path: String,
}

/// Error type for file operations.
#[derive(Debug)]
pub enum FileError {
    NotFound,
    IoError(String),
    #[allow(dead_code)]
    InvalidBookId,
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileError::NotFound => write!(f, "file not found"),
            FileError::IoError(e) => write!(f, "io error: {}", e),
            FileError::InvalidBookId => write!(f, "invalid book id"),
        }
    }
}

/// Prepare a download for streaming.
pub async fn prepare_download(
    pool: &PgPool,
    _storage_root: &str,
    book_id: Uuid,
    format: &str,
) -> Result<DownloadInfo, FileError> {
    // Get file path from database
    let path = book_files::get_path_by_format(pool, book_id, format)
        .await
        .map_err(|e| FileError::IoError(e.to_string()))?;

    let path = match path {
        Some(path) => path,
        None => {
            warn!(book_id = %book_id, format = %format, "Download requested but file not found in database");
            return Err(FileError::NotFound);
        }
    };

    // Open file
    let file = File::open(&path).await.map_err(|e| {
        warn!(book_id = %book_id, format = %format, error = %e, "Failed opening book file");
        FileError::NotFound
    })?;

    let metadata = file.metadata().await.map_err(|e| {
        warn!(book_id = %book_id, format = %format, error = %e, "Failed reading file metadata");
        FileError::IoError(e.to_string())
    })?;

    let file_size = metadata.len() as usize;

    let content_type = match format.to_lowercase().as_str() {
        "epub" | "kepub" => "application/epub+zip",
        "m4b" | "m4a" => "audio/mp4",
        "mp3" => "audio/mpeg",
        "mp3z" => "application/zip",
        "flac" => "audio/flac",
        "ogg" | "opus" => "audio/ogg",
        _ => "application/octet-stream",
    };

    let filename = build_download_filename(pool, book_id, format).await;
    let encoded = utf8_percent_encode(&filename, NON_ALPHANUMERIC).to_string();
    let content_disposition = format!(
        "attachment; filename=\"{}\"; filename*=UTF-8''{}",
        sanitize_filename_ascii(&filename),
        encoded
    );

    Ok(DownloadInfo {
        file,
        file_size,
        content_type,
        filename,
        content_disposition,
        file_path: path,
    })
}

/// Build a download filename from book metadata.
pub async fn build_download_filename(pool: &PgPool, book_id: Uuid, format: &str) -> String {
    let title = books::get_title(pool, book_id).await.ok().flatten();
    let author = authors::get_first_for_book(pool, book_id)
        .await
        .ok()
        .flatten();

    let base = match (title, author) {
        (Some(t), Some(a)) if !t.trim().is_empty() => format!("{} - {}", t, a),
        (Some(t), _) if !t.trim().is_empty() => t,
        _ => book_id.to_string(),
    };

    let ext = crate::media_root_naming::format_to_extension(format);

    format!("{}.{}", base, ext)
}

fn sanitize_filename_ascii(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '.' | '-' | '_' | '(' | ')' | '[' | ']')
        {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let trimmed = out.trim().trim_end_matches('.').to_string();
    if trimmed.is_empty() {
        "book".to_string()
    } else {
        trimmed
    }
}

/// Load original cover image for a book.
pub async fn load_cover_original(storage_root: &str, book_id: Uuid) -> Result<Vec<u8>, FileError> {
    let mut path = PathBuf::from(storage_root);
    path.push("covers");
    path.push(format!("{}.jpg", book_id));

    fs::read(&path).await.map_err(|e| {
        warn!(book_id = %book_id, error = %e, "Cover not found");
        FileError::NotFound
    })
}

/// Load or build a cover variant (resized/greyscale).
pub async fn load_or_build_cover_variant(
    storage_root: &str,
    book_id: Uuid,
    original: Vec<u8>,
    variant: CoverVariant,
) -> Result<Vec<u8>, String> {
    // If no transformations needed, return original
    if variant.width.is_none() && variant.height.is_none() && !variant.greyscale {
        return Ok(original);
    }

    let hash = hex_sha256(&original);
    let mut cache_dir = PathBuf::from(storage_root);
    cache_dir.push("covers");
    cache_dir.push("variants");
    let _ = fs::create_dir_all(&cache_dir).await;

    let key = format!(
        "{}_w{}_h{}_q{}_{}",
        hash,
        variant.width.unwrap_or(0),
        variant.height.unwrap_or(0),
        variant.quality,
        if variant.greyscale { "g" } else { "c" }
    );

    let mut cache_file = cache_dir.clone();
    cache_file.push(format!("{}_{}.jpg", book_id, key));

    // Try to load from cache
    if let Ok(bytes) = fs::read(&cache_file).await {
        return Ok(bytes);
    }

    // Build the variant
    let bytes = build_cover_variant_bytes(original, variant).await?;
    let _ = fs::write(&cache_file, &bytes).await;
    Ok(bytes)
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    hex::encode(digest)
}

async fn build_cover_variant_bytes(
    original: Vec<u8>,
    variant: CoverVariant,
) -> Result<Vec<u8>, String> {
    task::spawn_blocking(move || {
        let mut img = image::load_from_memory(&original).map_err(|err| err.to_string())?;
        if let (Some(width), Some(height)) = (variant.width, variant.height) {
            img = img.resize_exact(width, height, FilterType::Lanczos3);
        }
        if variant.greyscale {
            img = img.grayscale();
        }
        let mut out = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut out, variant.quality);
        encoder.encode_image(&img).map_err(|err| err.to_string())?;
        Ok(out)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Parse HTTP Range header.
#[allow(dead_code)]
pub fn parse_range(header: &str, file_size: usize) -> Option<(usize, usize)> {
    let range = header.strip_prefix("bytes=")?;
    let mut parts = range.split('-');
    let start_str = parts.next()?;
    let end_str = parts.next()?;

    let start: usize = if start_str.is_empty() {
        0
    } else {
        start_str.parse().ok()?
    };

    let end: usize = if end_str.is_empty() {
        file_size.saturating_sub(1)
    } else {
        end_str.parse().ok()?
    };

    if start <= end && end < file_size {
        Some((start, end))
    } else {
        None
    }
}

/// Parse dimension string to u32.
pub fn parse_dimension(value: &str) -> Option<u32> {
    let parsed = value.parse::<u32>().ok()?;
    if parsed == 0 { None } else { Some(parsed) }
}

/// Parse quality string to u8.
pub fn parse_quality(value: &str) -> u8 {
    value.parse::<u8>().unwrap_or(80).clamp(1, 100)
}

/// Check if greyscale flag is set.
pub fn is_greyscale_flag(flag: &str) -> bool {
    matches!(flag, "1" | "true" | "True" | "TRUE")
}
