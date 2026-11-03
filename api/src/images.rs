//! Image caching for authors, series, movies, TV shows, seasons, and people.
//!
//! Downloads and caches images from external URLs locally to avoid
//! dependency on external services and improve load times. Images are
//! stored as-is (no format conversion) without file extensions.
//!
//! Storage structure:
//! - `{storage_root}/images/authors/{author_id}`
//! - `{storage_root}/images/series/{series_id}`
//! - `{storage_root}/images/movies/{movie_id}`
//! - `{storage_root}/images/movies/{movie_id}_backdrop`
//! - `{storage_root}/images/movies/{movie_id}_logo`
//! - `{storage_root}/images/tv_shows/{show_id}`
//! - `{storage_root}/images/tv_shows/{show_id}_backdrop`
//! - `{storage_root}/images/tv_shows/{show_id}_logo`
//! - `{storage_root}/images/seasons/{season_id}`
//! - `{storage_root}/images/episodes/{episode_id}`
//! - `{storage_root}/images/people/{person_id}`
//! - `{storage_root}/images/extracted/{book_id}`
//! - `{storage_root}/images/iptv_channels/{channel_id}`

use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::video_metadata::types::MediaImage;

/// Single image entry in the `images` JSONB column.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImageEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_at: Option<String>, // ISO 8601
}

impl ImageEntry {
    /// Returns true if this entry has a usable image (either cached or with a fetchable source URL).
    pub fn has_image(&self) -> bool {
        self.cached_at.is_some() || self.source_url.as_ref().is_some_and(|u| !u.is_empty())
    }
}

/// SQL predicate mirroring [`ImageEntry::has_image`] for `{table}.images -> key`.
pub fn has_image_sql(table: &str, key: &str) -> String {
    format!(
        "({table}.images->'{key}'->>'cached_at' IS NOT NULL \
         OR COALESCE({table}.images->'{key}'->>'source_url', '') <> '')"
    )
}

/// Container for all images on an entity. Stored as JSONB, deserialized with serde.
pub type ImageMap = HashMap<String, ImageEntry>;

/// Construct a local endpoint URL if an image is available for the given key.
///
/// `base_path` is the path prefix (e.g. "/api/v1/admin/movies/")
/// `suffix` is the path suffix after the ID (e.g. "/poster")
pub fn endpoint_url(
    images: &ImageMap,
    key: &str,
    base_path: &str,
    id: Uuid,
    suffix: &str,
) -> Option<String> {
    images
        .get(key)
        .filter(|e| e.has_image())
        .map(|_| format!("{}{}{}", base_path, id, suffix))
}

/// Get the source URL for an image key, if present and non-empty.
pub fn source_url(images: &ImageMap, key: &str) -> Option<String> {
    images
        .get(key)
        .and_then(|e| e.source_url.as_ref())
        .filter(|u| !u.is_empty())
        .cloned()
}

/// Update the cached_at timestamp for an image key in an entity's images JSONB.
pub async fn mark_cached(
    pool: &sqlx::PgPool,
    table: &str,
    id: Uuid,
    image_key: &str,
) -> Result<(), ImageError> {
    let now = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();

    // Two-step: ensure the image key object exists, then set cached_at within it.
    // jsonb_set with create_missing only creates leaf keys, not intermediate objects.
    let sql = format!(
        "UPDATE {table} SET images = jsonb_set(\
            CASE WHEN images ? '{key}' THEN images \
            ELSE jsonb_set(images, '{{{key}}}', '{{}}'::jsonb, true) END, \
            '{{{key},cached_at}}', to_jsonb($2::text), true\
        ) WHERE id = $1",
        table = table,
        key = image_key,
    );

    sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .bind(&now)
        .execute(pool)
        .await
        .map_err(|e| ImageError::IoError(std::io::Error::other(e.to_string())))?;

    Ok(())
}

/// Type of image to cache.
#[derive(Debug, Clone, Copy)]
pub enum ImageType {
    /// Author profile image.
    Author,
    /// Series cover image.
    Series,
    /// Movie poster image.
    MoviePoster,
    /// Movie backdrop image.
    MovieBackdrop,
    /// Movie logo image (transparent title overlay).
    MovieLogo,
    /// TV show poster image.
    TvShowPoster,
    /// TV show backdrop image.
    TvShowBackdrop,
    /// TV show logo image (transparent title overlay).
    TvShowLogo,
    /// Season poster image.
    SeasonPoster,
    /// Person profile image.
    PersonImage,
    /// Episode still image.
    EpisodeStill,
    /// Cover image extracted from an ebook file.
    BookExtracted,
    /// IPTV channel icon/logo.
    IptvChannelIcon,
}

impl ImageType {
    /// Get the subdirectory name for this image type.
    fn subdir(&self) -> &'static str {
        match self {
            ImageType::Author => "authors",
            ImageType::Series => "series",
            ImageType::MoviePoster | ImageType::MovieBackdrop | ImageType::MovieLogo => "movies",
            ImageType::TvShowPoster | ImageType::TvShowBackdrop | ImageType::TvShowLogo => {
                "tv_shows"
            }
            ImageType::SeasonPoster => "seasons",
            ImageType::PersonImage => "people",
            ImageType::EpisodeStill => "episodes",
            ImageType::BookExtracted => "extracted",
            ImageType::IptvChannelIcon => "iptv_channels",
        }
    }

    /// Get the filename suffix for this image type.
    fn suffix(&self) -> &'static str {
        match self {
            ImageType::MovieBackdrop | ImageType::TvShowBackdrop => "_backdrop",
            ImageType::MovieLogo | ImageType::TvShowLogo => "_logo",
            _ => "",
        }
    }
}

/// Error type for image caching operations.
#[derive(Debug)]
pub enum ImageError {
    /// Network error fetching image.
    NetworkError(String),
    /// IO error writing image.
    IoError(std::io::Error),
    /// Invalid image data.
    InvalidImage(String),
    /// Image not found (no cache and no source URL).
    NotFound(String),
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageError::NetworkError(msg) => write!(f, "network error: {}", msg),
            ImageError::IoError(e) => write!(f, "io error: {}", e),
            ImageError::InvalidImage(msg) => write!(f, "invalid image: {}", msg),
            ImageError::NotFound(msg) => write!(f, "not found: {}", msg),
        }
    }
}

impl std::error::Error for ImageError {}

impl From<std::io::Error> for ImageError {
    fn from(e: std::io::Error) -> Self {
        ImageError::IoError(e)
    }
}

/// Get the local path for a cached image.
pub fn local_image_path(storage_root: &str, image_type: ImageType, entity_id: Uuid) -> PathBuf {
    PathBuf::from(storage_root)
        .join("images")
        .join(image_type.subdir())
        .join(format!("{}{}", entity_id, image_type.suffix()))
}

/// Check if a cached image exists.
pub fn cached_image_exists(storage_root: &str, image_type: ImageType, entity_id: Uuid) -> bool {
    local_image_path(storage_root, image_type, entity_id).exists()
}

/// Download and cache an external image locally.
///
/// This function:
/// 1. Downloads the image from the external URL
/// 2. Validates it's a supported image format
/// 3. Saves to the local cache as-is (no format conversion)
///
/// If the image already exists locally, this will overwrite it (for refresh).
pub async fn cache_image(
    storage_root: &str,
    image_type: ImageType,
    entity_id: Uuid,
    external_url: &str,
) -> Result<PathBuf, ImageError> {
    let client = crate::http::new_client();

    // Download the image
    let response = client
        .get(external_url)
        .send()
        .await
        .map_err(|e| ImageError::NetworkError(e.to_string()))?;

    if !response.status().is_success() {
        return Err(ImageError::NetworkError(format!(
            "HTTP {}: {}",
            response.status(),
            external_url
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| ImageError::NetworkError(e.to_string()))?;

    save_image_bytes(storage_root, image_type, entity_id, &bytes).await
}

/// Save raw image bytes to the local cache.
///
/// Validates the bytes are a recognized image format (JPEG, PNG, GIF, WebP)
/// and stores them as-is without conversion.
pub async fn save_image_bytes(
    storage_root: &str,
    image_type: ImageType,
    entity_id: Uuid,
    bytes: &[u8],
) -> Result<PathBuf, ImageError> {
    if bytes.is_empty() {
        return Err(ImageError::InvalidImage("empty image data".to_string()));
    }

    // Validate it's a recognized image format
    validate_image(bytes)?;

    // Ensure directory exists
    let path = local_image_path(storage_root, image_type, entity_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }

    // Write the image as-is (no conversion)
    fs::write(&path, bytes).await?;

    debug!(
        image_type = ?image_type,
        entity_id = %entity_id,
        size = bytes.len(),
        "Saved image locally"
    );

    Ok(path)
}

/// Validate that bytes are a supported image format.
pub fn validate_image(bytes: &[u8]) -> Result<(), ImageError> {
    if is_svg(bytes) {
        return Ok(());
    }

    let kind = infer::get(bytes);
    match kind {
        Some(t) if t.mime_type().starts_with("image/") => Ok(()),
        Some(t) => Err(ImageError::InvalidImage(format!(
            "not an image: {}",
            t.mime_type()
        ))),
        None => {
            // Fallback: try loading with the image crate
            image::load_from_memory(bytes)
                .map(|_| ())
                .map_err(|e| ImageError::InvalidImage(format!("unrecognized format: {}", e)))
        }
    }
}

/// Detect the MIME type of cached image bytes.
pub fn detect_content_type(bytes: &[u8]) -> &'static str {
    if is_svg(bytes) {
        return "image/svg+xml";
    }
    infer::get(bytes)
        .map(|t| t.mime_type())
        .unwrap_or("application/octet-stream")
}

/// Check if bytes look like an SVG (XML-based, not detected by magic bytes).
fn is_svg(bytes: &[u8]) -> bool {
    let check_len = bytes.len().min(1024);
    let prefix = std::str::from_utf8(&bytes[..check_len]).unwrap_or("");
    let trimmed = prefix.trim_start();
    trimmed.starts_with("<svg") || (trimmed.starts_with("<?xml") && trimmed.contains("<svg"))
}

/// Load a cached image from disk.
pub async fn load_cached_image(
    storage_root: &str,
    image_type: ImageType,
    entity_id: Uuid,
) -> Result<Vec<u8>, ImageError> {
    let storage_root = storage_root.to_owned();
    tokio::task::spawn_blocking(move || {
        load_cached_image_sync(&storage_root, image_type, entity_id)
    })
    .await
    .map_err(|e| ImageError::IoError(std::io::Error::other(format!("Join error: {e}"))))?
}

/// Load a cached image from disk (synchronous, for use inside spawn_blocking).
pub fn load_cached_image_sync(
    storage_root: &str,
    image_type: ImageType,
    entity_id: Uuid,
) -> Result<Vec<u8>, ImageError> {
    let path = local_image_path(storage_root, image_type, entity_id);
    std::fs::read(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            warn!(
                image_type = ?image_type,
                entity_id = %entity_id,
                "Cached image not found"
            );
        }
        ImageError::IoError(e)
    })
}

/// Delete a cached image.
pub async fn delete_cached_image(
    storage_root: &str,
    image_type: ImageType,
    entity_id: Uuid,
) -> Result<(), ImageError> {
    let path = local_image_path(storage_root, image_type, entity_id);
    match fs::remove_file(&path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ImageError::IoError(e)),
    }
}

/// Try to cache an image, logging any errors but not failing.
///
/// This is useful for background caching where failures shouldn't block the main operation.
pub async fn try_cache_image(
    storage_root: &str,
    image_type: ImageType,
    entity_id: Uuid,
    external_url: Option<&str>,
) {
    let Some(url) = external_url else {
        return;
    };

    if url.is_empty() {
        return;
    }

    if let Err(e) = cache_image(storage_root, image_type, entity_id, url).await {
        debug!(
            image_type = ?image_type,
            entity_id = %entity_id,
            url = %url,
            error = %e,
            "Failed to cache image"
        );
    }
}

/// Pause after each art download made ahead of a viewer. The image CDN takes no API key,
/// but a first sweep over a whole library is a thousand-odd requests, and spread out they
/// look like a household browsing rather than a scraper.
pub const ART_DOWNLOAD_PACE: std::time::Duration = std::time::Duration::from_millis(250);

/// The art a browse page's backdrop and grid use, in the order a viewer sees it, then the
/// logo the player opens on.
pub const MOVIE_ART: &[(&str, ImageType)] = &[
    ("backdrop", ImageType::MovieBackdrop),
    ("poster", ImageType::MoviePoster),
    ("logo", ImageType::MovieLogo),
];
pub const SHOW_ART: &[(&str, ImageType)] = &[
    ("backdrop", ImageType::TvShowBackdrop),
    ("poster", ImageType::TvShowPoster),
    ("logo", ImageType::TvShowLogo),
];

/// Puts an entity's images on disk now rather than on first view: the backdrop behind a
/// browse page changes with every card a viewer passes. Keys already cached are left
/// alone, and a failed download is logged, not raised; the lazy path still covers it.
/// Each download is followed by [`ART_DOWNLOAD_PACE`].
///
/// `table` is a code constant (`movies`, `tv_shows`), never input.
pub async fn warm_entity_images(
    pool: &sqlx::PgPool,
    storage_root: &str,
    table: &str,
    entity_id: Uuid,
    keys: &[(&str, ImageType)],
) {
    let images: ImageMap = match sqlx::query_scalar::<_, serde_json::Value>(sqlx::AssertSqlSafe(
        format!("SELECT images FROM {table} WHERE id = $1"),
    ))
    .bind(entity_id)
    .fetch_optional(pool)
    .await
    {
        Ok(Some(value)) => serde_json::from_value(value).unwrap_or_default(),
        Ok(None) => return,
        Err(e) => {
            debug!(table, entity_id = %entity_id, error = %e, "Failed to read images to warm");
            return;
        }
    };
    for &(key, image_type) in keys {
        let Some(entry) = images.get(key) else {
            continue;
        };
        if entry.cached_at.is_some() && cached_image_exists(storage_root, image_type, entity_id) {
            continue;
        }
        let Some(url) = entry.source_url.as_deref().filter(|u| !u.is_empty()) else {
            continue;
        };
        let fetched = cache_image(storage_root, image_type, entity_id, url).await;
        tokio::time::sleep(ART_DOWNLOAD_PACE).await;
        match fetched {
            Ok(_) => {
                if let Err(e) = mark_cached(pool, table, entity_id, key).await {
                    debug!(table, key, entity_id = %entity_id, error = %e, "Failed to mark image cached");
                }
            }
            Err(e) => {
                debug!(table, key, entity_id = %entity_id, url, error = %e, "Failed to warm image")
            }
        }
    }
}

/// Build an [`ImageMap`] from a provider's generic image list.
///
/// Takes the first image of each `image_type` as the primary `source_url`.
/// Existing entries in `base` are preserved (not overwritten).
pub fn image_map_from_media_images(images: &[MediaImage], base: ImageMap) -> ImageMap {
    let mut map = base;
    for img in images {
        map.entry(img.image_type.clone())
            .or_insert_with(|| ImageEntry {
                source_url: Some(img.url.clone()),
                cached_at: None,
            });
    }
    map
}

/// Build an image response with caching headers.
///
/// Sets `Cache-Control: public, max-age=86400` (24h) and an `ETag` derived from the
/// entity ID and `cached_at` timestamp. If the request includes a matching
/// `If-None-Match` header, returns 304 Not Modified without the body.
pub fn image_response(bytes: Vec<u8>, etag: &str, request_headers: &HeaderMap) -> Response {
    let etag_value = format!("\"{etag}\"");

    // Check If-None-Match for conditional request
    if let Some(inm) = request_headers.get(IF_NONE_MATCH)
        && let Ok(inm_str) = inm.to_str()
        && (inm_str == etag_value || inm_str == "*")
    {
        return (
            StatusCode::NOT_MODIFIED,
            [
                (CACHE_CONTROL, "public, max-age=86400"),
                (ETAG, &etag_value),
            ],
        )
            .into_response();
    }

    (
        [
            (CONTENT_TYPE, detect_content_type(&bytes)),
            (CACHE_CONTROL, "public, max-age=86400"),
            (ETAG, etag_value.as_str()),
        ],
        bytes,
    )
        .into_response()
}

/// Serve a cached entity image, lazy-fetching from the JSONB `source_url` if not yet cached.
///
/// Returns an image response on success, or an `ImageError` that callers should map to
/// their own error type.
#[allow(clippy::too_many_arguments)]
pub async fn serve_entity_image(
    pool: &sqlx::PgPool,
    storage_root: &str,
    table: &str,
    image_key: &str,
    image_type: ImageType,
    entity_id: Uuid,
    images: &ImageMap,
    request_headers: &HeaderMap,
) -> Result<Response, ImageError> {
    let entry = images.get(image_key);
    let is_cached = entry.is_some_and(|e| e.cached_at.is_some());
    let file_exists = cached_image_exists(storage_root, image_type, entity_id);

    let etag_for = |cached_at: &str| format!("{entity_id}-{cached_at}");

    // Serve from disk cache only if the DB agrees the image is cached.
    // A missing cached_at means the source_url was changed and the file is stale.
    if is_cached && file_exists {
        let cached_at = entry.unwrap().cached_at.as_deref().unwrap();
        let etag = etag_for(cached_at);

        // Fast 304 path: check If-None-Match before loading bytes from disk
        let etag_value = format!("\"{etag}\"");
        if let Some(inm) = request_headers.get(IF_NONE_MATCH)
            && let Ok(inm_str) = inm.to_str()
            && (inm_str == etag_value || inm_str == "*")
        {
            return Ok((
                StatusCode::NOT_MODIFIED,
                [
                    (CACHE_CONTROL, "public, max-age=86400"),
                    (ETAG, etag_value.as_str()),
                ],
            )
                .into_response());
        }

        let bytes = load_cached_image(storage_root, image_type, entity_id).await?;
        return Ok(image_response(bytes, &etag, request_headers));
    }

    let bytes = fetch_uncached_image(
        pool,
        storage_root,
        table,
        image_key,
        image_type,
        entity_id,
        images,
    )
    .await?;
    // Use a fresh timestamp for the etag since we just cached
    let etag = format!("{entity_id}-fresh");
    Ok(image_response(bytes, &etag, request_headers))
}

/// An entity's image bytes, from the disk cache when the DB agrees it is current,
/// otherwise downloaded from its source URL and cached.
pub async fn load_entity_image(
    pool: &sqlx::PgPool,
    storage_root: &str,
    table: &str,
    image_key: &str,
    image_type: ImageType,
    entity_id: Uuid,
    images: &ImageMap,
) -> Result<Vec<u8>, ImageError> {
    let is_cached = images.get(image_key).is_some_and(|e| e.cached_at.is_some());
    if is_cached && cached_image_exists(storage_root, image_type, entity_id) {
        return load_cached_image(storage_root, image_type, entity_id).await;
    }
    fetch_uncached_image(
        pool,
        storage_root,
        table,
        image_key,
        image_type,
        entity_id,
        images,
    )
    .await
}

async fn fetch_uncached_image(
    pool: &sqlx::PgPool,
    storage_root: &str,
    table: &str,
    image_key: &str,
    image_type: ImageType,
    entity_id: Uuid,
    images: &ImageMap,
) -> Result<Vec<u8>, ImageError> {
    // Delete stale file when source_url changed (cached_at cleared but old file lingers)
    if cached_image_exists(storage_root, image_type, entity_id) {
        let stale_path = local_image_path(storage_root, image_type, entity_id);
        let _ = fs::remove_file(&stale_path).await;
    }

    if let Some(url) = source_url(images, image_key) {
        return match cache_image(storage_root, image_type, entity_id, &url).await {
            Ok(_) => {
                let _ = mark_cached(pool, table, entity_id, image_key).await;
                load_cached_image(storage_root, image_type, entity_id).await
            }
            Err(e) => {
                warn!(entity_id = %entity_id, image_key, error = %e, "Failed to cache image from source URL");
                Err(ImageError::NotFound(format!(
                    "No {image_key} available for {entity_id}"
                )))
            }
        };
    }

    Err(ImageError::NotFound(format!(
        "No {image_key} available for {entity_id}"
    )))
}

/// A book's cover from wherever it lives: an uploaded cover, the series image cache
/// used for discovered books, the cover extracted from its ebook, or — downloaded and
/// cached now — the cover URL from its metadata.
pub async fn load_book_cover(
    pool: &sqlx::PgPool,
    storage_root: &str,
    book_id: Uuid,
) -> Option<Vec<u8>> {
    let uploaded = PathBuf::from(storage_root)
        .join("covers")
        .join(format!("{book_id}.jpg"));
    for path in [
        uploaded,
        local_image_path(storage_root, ImageType::Series, book_id),
        local_image_path(storage_root, ImageType::BookExtracted, book_id),
    ] {
        if let Ok(bytes) = fs::read(&path).await {
            return Some(bytes);
        }
    }

    let url = crate::repositories::books::get_cover_url(pool, book_id)
        .await
        .ok()
        .flatten()?;
    let path = cache_image(storage_root, ImageType::Series, book_id, &url)
        .await
        .ok()?;
    let bytes = fs::read(&path).await.ok()?;
    let _ = mark_cached(pool, "books", book_id, "cover").await;
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_image_path() {
        let storage_root = "/data";
        let id = Uuid::nil();

        let author_path = local_image_path(storage_root, ImageType::Author, id);
        assert_eq!(
            author_path.to_string_lossy(),
            "/data/images/authors/00000000-0000-0000-0000-000000000000"
        );

        let series_path = local_image_path(storage_root, ImageType::Series, id);
        assert_eq!(
            series_path.to_string_lossy(),
            "/data/images/series/00000000-0000-0000-0000-000000000000"
        );

        let movie_poster = local_image_path(storage_root, ImageType::MoviePoster, id);
        assert_eq!(
            movie_poster.to_string_lossy(),
            "/data/images/movies/00000000-0000-0000-0000-000000000000"
        );

        let movie_backdrop = local_image_path(storage_root, ImageType::MovieBackdrop, id);
        assert_eq!(
            movie_backdrop.to_string_lossy(),
            "/data/images/movies/00000000-0000-0000-0000-000000000000_backdrop"
        );

        let tv_poster = local_image_path(storage_root, ImageType::TvShowPoster, id);
        assert_eq!(
            tv_poster.to_string_lossy(),
            "/data/images/tv_shows/00000000-0000-0000-0000-000000000000"
        );

        let tv_backdrop = local_image_path(storage_root, ImageType::TvShowBackdrop, id);
        assert_eq!(
            tv_backdrop.to_string_lossy(),
            "/data/images/tv_shows/00000000-0000-0000-0000-000000000000_backdrop"
        );

        let season = local_image_path(storage_root, ImageType::SeasonPoster, id);
        assert_eq!(
            season.to_string_lossy(),
            "/data/images/seasons/00000000-0000-0000-0000-000000000000"
        );

        let person = local_image_path(storage_root, ImageType::PersonImage, id);
        assert_eq!(
            person.to_string_lossy(),
            "/data/images/people/00000000-0000-0000-0000-000000000000"
        );

        let movie_logo = local_image_path(storage_root, ImageType::MovieLogo, id);
        assert_eq!(
            movie_logo.to_string_lossy(),
            "/data/images/movies/00000000-0000-0000-0000-000000000000_logo"
        );

        let tv_logo = local_image_path(storage_root, ImageType::TvShowLogo, id);
        assert_eq!(
            tv_logo.to_string_lossy(),
            "/data/images/tv_shows/00000000-0000-0000-0000-000000000000_logo"
        );

        let book_extracted = local_image_path(storage_root, ImageType::BookExtracted, id);
        assert_eq!(
            book_extracted.to_string_lossy(),
            "/data/images/extracted/00000000-0000-0000-0000-000000000000"
        );

        let iptv_channel = local_image_path(storage_root, ImageType::IptvChannelIcon, id);
        assert_eq!(
            iptv_channel.to_string_lossy(),
            "/data/images/iptv_channels/00000000-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn test_svg_detection() {
        assert!(is_svg(b"<svg xmlns=\"http://www.w3.org/2000/svg\">"));
        assert!(is_svg(b"<?xml version=\"1.0\"?><svg>"));
        assert!(is_svg(b"  \n  <svg>"));
        assert!(!is_svg(b"<html>"));
        assert!(!is_svg(b"\x89PNG"));
    }

    #[test]
    fn test_validate_svg() {
        let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"100\"><circle cx=\"50\" cy=\"50\" r=\"40\"/></svg>";
        assert!(validate_image(svg).is_ok());
    }
}
