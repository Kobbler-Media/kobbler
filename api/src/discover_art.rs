//! Discover shelves show TMDB art for titles the library does not hold. It is served
//! through here and kept on disk, so a household's devices fetch each picture from TMDB
//! once between them rather than once each, and the shelves keep working as the
//! backdrop behind every page that shows them.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tracing::debug;

use crate::AppState;
use crate::images::{ImageError, image_response, validate_image};

pub const ART_ROUTE: &str = "/api/v1/discover/art";

const TMDB_ART_PREFIX: &str = "https://image.tmdb.org/t/p/";

/// Art no shelf has pointed at for this long is dropped at the next sync.
const UNUSED_ART_TTL: Duration = Duration::from_secs(14 * 24 * 3600);

/// Only TMDB's image host: anything else would make the route an open proxy.
pub fn is_tmdb_art(src: &str) -> bool {
    src.strip_prefix(TMDB_ART_PREFIX)
        .is_some_and(|rest| !rest.is_empty() && !rest.contains(".."))
}

/// What a shelf hands out in place of a TMDB URL. Anything else passes through unchanged.
pub fn art_url(src: &str) -> String {
    if is_tmdb_art(src) {
        format!("{ART_ROUTE}?src={}", urlencoding::encode(src))
    } else {
        src.to_string()
    }
}

fn art_dir(storage_root: &str) -> PathBuf {
    Path::new(storage_root).join("images").join("discover")
}

fn art_key(src: &str) -> String {
    hex::encode(Sha256::digest(src.as_bytes()))
}

/// The bytes behind [src], from disk when they are there and from the source otherwise.
pub async fn cached_art(storage_root: &str, src: &str) -> Result<Vec<u8>, ImageError> {
    let path = art_dir(storage_root).join(art_key(src));
    if let Ok(bytes) = tokio::fs::read(&path).await {
        return Ok(bytes);
    }
    let response = crate::http::new_client()
        .get(src)
        .send()
        .await
        .map_err(|e| ImageError::NetworkError(e.to_string()))?;
    if !response.status().is_success() {
        return Err(ImageError::NetworkError(format!(
            "HTTP {}: {src}",
            response.status()
        )));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| ImageError::NetworkError(e.to_string()))?
        .to_vec();
    validate_image(&bytes)?;
    tokio::fs::create_dir_all(art_dir(storage_root))
        .await
        .map_err(ImageError::IoError)?;
    // Written aside and renamed, so a reader never sees half a file.
    let partial = path.with_extension("part");
    tokio::fs::write(&partial, &bytes)
        .await
        .map_err(ImageError::IoError)?;
    tokio::fs::rename(&partial, &path)
        .await
        .map_err(ImageError::IoError)?;
    Ok(bytes)
}

/// Fetches every shelf picture ahead of the first viewer, then drops art no shelf has
/// used in a while.
pub async fn warm_and_prune<'a>(storage_root: &str, srcs: impl IntoIterator<Item = &'a str>) {
    let mut keep = HashSet::new();
    for src in srcs.into_iter().filter(|src| is_tmdb_art(src)) {
        let key = art_key(src);
        let on_disk = art_dir(storage_root).join(&key).exists();
        keep.insert(key);
        if on_disk {
            continue;
        }
        if let Err(e) = cached_art(storage_root, src).await {
            debug!(src, error = %e, "Failed to warm discover art");
        }
        tokio::time::sleep(crate::images::ART_DOWNLOAD_PACE).await;
    }
    let Ok(mut entries) = tokio::fs::read_dir(art_dir(storage_root)).await else {
        return;
    };
    let cutoff = SystemTime::now() - UNUSED_ART_TTL;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        if keep.contains(&name) {
            // Touch on use, so the age check below measures time since a shelf last had it.
            let _ = filetime_now(&entry.path());
            continue;
        }
        let stale = entry
            .metadata()
            .await
            .and_then(|m| m.modified())
            .is_ok_and(|modified| modified < cutoff);
        if stale {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

fn filetime_now(path: &Path) -> std::io::Result<()> {
    std::fs::File::options()
        .append(true)
        .open(path)?
        .set_modified(SystemTime::now())
}

#[derive(Debug, Deserialize)]
pub struct ArtQuery {
    pub src: String,
}

/// GET /api/v1/discover/art?src=<TMDB image URL>
///
/// Public like the library's own image routes: an <img> tag carries no bearer token.
pub async fn serve(
    State(state): State<AppState>,
    Query(query): Query<ArtQuery>,
    headers: HeaderMap,
) -> Response {
    if !is_tmdb_art(&query.src) {
        return (StatusCode::BAD_REQUEST, "not TMDB art").into_response();
    }
    match cached_art(&state.config.storage_root, &query.src).await {
        Ok(bytes) => image_response(bytes, &art_key(&query.src), &headers),
        Err(e) => {
            debug!(src = %query.src, error = %e, "Discover art unavailable");
            (StatusCode::BAD_GATEWAY, "art unavailable").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_tmdb_images_are_served() {
        assert!(is_tmdb_art("https://image.tmdb.org/t/p/w500/a.jpg"));
        for src in [
            "https://image.tmdb.org/t/p/",
            "http://image.tmdb.org/t/p/w500/a.jpg",
            "https://image.tmdb.org.evil.com/t/p/w500/a.jpg",
            "https://image.tmdb.org/t/p/../../etc",
            "https://example.com/a.jpg",
        ] {
            assert!(!is_tmdb_art(src), "{src}");
        }
    }
}
