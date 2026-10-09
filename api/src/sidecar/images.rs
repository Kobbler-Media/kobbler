//! Image sidecar discovery and writing.
//!
//! Discovers well-known image files (poster.jpg, fanart.jpg, cover.jpg, etc.) that
//! Kodi/Jellyfin/Plex/Audiobookshelf leave alongside media files. Also writes cached
//! images back to the filesystem.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::debug;

/// An image discovered adjacent to a media file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredImage {
    /// ImageMap key: "poster", "backdrop", "logo", "cover", "still", "banner", "landscape"
    pub image_key: String,
    /// Absolute path to the image file
    pub file_path: String,
}

pub type DiscoveredImages = Vec<DiscoveredImage>;

/// Image extensions to check (case-insensitive).
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

/// Movie image filenames and their ImageMap keys, in priority order.
/// First match for each key wins.
const MOVIE_IMAGE_MAP: &[(&str, &str)] = &[
    ("poster", "poster"),
    ("folder", "poster"), // fallback
    ("fanart", "backdrop"),
    ("clearlogo", "logo"),
    ("logo", "logo"),
    ("banner", "banner"),
    ("landscape", "landscape"),
];

/// TV show image filenames and their ImageMap keys.
const TV_SHOW_IMAGE_MAP: &[(&str, &str)] = &[
    ("poster", "poster"),
    ("fanart", "backdrop"),
    ("clearlogo", "logo"),
    ("banner", "banner"),
];

/// Book image filenames and their ImageMap keys.
const BOOK_IMAGE_MAP: &[(&str, &str)] = &[
    ("cover", "cover"),
    ("folder", "cover"), // fallback
];

/// Discover image sidecar files in a movie directory.
pub async fn discover_movie_images(movie_dir: &Path) -> DiscoveredImages {
    discover_images_from_map(movie_dir, MOVIE_IMAGE_MAP).await
}

/// Discover image sidecar files in a TV show root directory.
pub async fn discover_tv_show_images(show_dir: &Path) -> DiscoveredImages {
    discover_images_from_map(show_dir, TV_SHOW_IMAGE_MAP).await
}

/// Discover season poster images in a TV show root directory.
///
/// Looks for `season{NN}-poster.jpg` and `season-specials-poster.jpg` patterns.
/// Returns a map of season number -> discovered image.
pub async fn discover_season_images(show_dir: &Path) -> HashMap<u32, DiscoveredImage> {
    let show_dir = show_dir.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut result = HashMap::new();

        let entries = match std::fs::read_dir(&show_dir) {
            Ok(e) => e,
            Err(_) => return result,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !is_image_file(&path) {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let stem_lower = stem.to_lowercase();

            // season-specials-poster -> season 0
            if stem_lower == "season-specials-poster" {
                result.entry(0).or_insert_with(|| DiscoveredImage {
                    image_key: "poster".to_string(),
                    file_path: path.to_string_lossy().to_string(),
                });
                continue;
            }

            // season{NN}-poster -> season NN
            if let Some(rest) = stem_lower.strip_prefix("season")
                && let Some(num_str) = rest.strip_suffix("-poster")
                && let Ok(num) = num_str.parse::<u32>()
            {
                result.entry(num).or_insert_with(|| DiscoveredImage {
                    image_key: "poster".to_string(),
                    file_path: path.to_string_lossy().to_string(),
                });
            }
        }

        result
    })
    .await
    .unwrap_or_default()
}

/// Discover episode thumbnail images for a specific episode file.
///
/// Looks for `{episode_stem}-thumb.{ext}` adjacent to the episode file.
pub async fn discover_episode_images(episode_file: &Path) -> DiscoveredImages {
    let episode_file = episode_file.to_owned();
    tokio::task::spawn_blocking(move || {
        let Some(dir) = episode_file.parent() else {
            return vec![];
        };
        let Some(stem) = episode_file.file_stem().and_then(|s| s.to_str()) else {
            return vec![];
        };

        let thumb_stem = format!("{stem}-thumb");

        for ext in IMAGE_EXTENSIONS {
            let candidate = dir.join(format!("{thumb_stem}.{ext}"));
            if candidate.exists() {
                return vec![DiscoveredImage {
                    image_key: "still".to_string(),
                    file_path: candidate.to_string_lossy().to_string(),
                }];
            }
        }

        vec![]
    })
    .await
    .unwrap_or_default()
}

/// Discover image sidecar files in a book directory.
pub async fn discover_book_images(book_dir: &Path) -> DiscoveredImages {
    discover_images_from_map(book_dir, BOOK_IMAGE_MAP).await
}

/// Reverse mapping: get the canonical sidecar filename for an image key + media type.
///
/// Returns the filename (e.g., "poster.jpg") that should be written for a given key.
/// Returns `None` if the key is not a recognized sidecar image key for the media type.
pub fn sidecar_filename(image_key: &str, media_type: &str) -> Option<&'static str> {
    match media_type {
        "movie" => match image_key {
            "poster" => Some("poster.jpg"),
            "backdrop" => Some("fanart.jpg"),
            "logo" => Some("clearlogo.png"),
            "banner" => Some("banner.jpg"),
            "landscape" => Some("landscape.jpg"),
            _ => None,
        },
        "tv" => match image_key {
            "poster" => Some("poster.jpg"),
            "backdrop" => Some("fanart.jpg"),
            "logo" => Some("clearlogo.png"),
            "banner" => Some("banner.jpg"),
            _ => None,
        },
        "book" => match image_key {
            "cover" => Some("cover.jpg"),
            _ => None,
        },
        _ => None,
    }
}

/// Write a cached image to the filesystem as a sidecar file.
///
/// Reads the image from the local cache and writes it to `target_dir` using the
/// canonical filename for the given image key. Does NOT overwrite existing files.
///
/// Returns `true` if the file was written, `false` if it already existed.
pub async fn write_image_sidecar(
    storage_root: &str,
    image_type: crate::images::ImageType,
    entity_id: uuid::Uuid,
    target_dir: &Path,
    image_key: &str,
    media_type: &str,
) -> Result<bool, std::io::Error> {
    let Some(filename) = sidecar_filename(image_key, media_type) else {
        return Ok(false);
    };

    let target_path = target_dir.join(filename);

    // Check existence and load cached image on the blocking pool
    let storage_root = storage_root.to_owned();
    let target_path_clone = target_path.clone();
    let load_result = tokio::task::spawn_blocking(move || {
        if target_path_clone.exists() {
            return Ok(None); // already exists
        }
        if !crate::images::cached_image_exists(&storage_root, image_type, entity_id) {
            return Ok(None); // no cached image
        }
        match crate::images::load_cached_image_sync(&storage_root, image_type, entity_id) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) => Err(std::io::Error::other(format!(
                "Failed to load cached image: {e}"
            ))),
        }
    })
    .await
    .map_err(|e| std::io::Error::other(format!("Join error: {e}")))?;

    let Some(bytes) = load_result? else {
        return Ok(false);
    };

    // Write on the blocking pool
    let write_path = target_path.clone();
    tokio::task::spawn_blocking(move || std::fs::write(&write_path, &bytes))
        .await
        .map_err(|e| std::io::Error::other(format!("Join error: {e}")))?
        .map_err(|e| std::io::Error::other(format!("Write error: {e}")))?;

    debug!(path = %target_path.display(), "Wrote image sidecar");
    Ok(true)
}

// ============================================================================
// Internal helpers
// ============================================================================

/// Check if a file has an image extension.
fn is_image_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| IMAGE_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
}

/// Generic discovery: scan a directory for well-known filenames from a mapping table.
async fn discover_images_from_map(dir: &Path, map: &[(&str, &str)]) -> DiscoveredImages {
    let dir = dir.to_owned();
    let map: Vec<(String, String)> = map
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    tokio::task::spawn_blocking(move || {
        let mut result = DiscoveredImages::new();
        let mut found_keys = std::collections::HashSet::new();

        for (filename_stem, image_key) in &map {
            if found_keys.contains(image_key.as_str()) {
                continue;
            }
            for ext in IMAGE_EXTENSIONS {
                let candidate = dir.join(format!("{filename_stem}.{ext}"));
                if candidate.exists() {
                    found_keys.insert(image_key.as_str());
                    result.push(DiscoveredImage {
                        image_key: image_key.to_string(),
                        file_path: candidate.to_string_lossy().to_string(),
                    });
                    break;
                }
            }
        }

        result
    })
    .await
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_data_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sidecar-images")
    }

    #[tokio::test]
    async fn test_discover_movie_images_anchorman2() {
        let test_data = test_data_dir();
        let dir = test_data.join("Movies/Anchorman 2 - The Legend Continues (2013)");
        let images = discover_movie_images(&dir).await;

        let keys: Vec<&str> = images.iter().map(|i| i.image_key.as_str()).collect();
        assert!(keys.contains(&"poster"), "should find poster: {keys:?}");
        assert!(keys.contains(&"backdrop"), "should find backdrop: {keys:?}");
    }

    #[tokio::test]
    async fn test_discover_movie_images_anchorman1() {
        let test_data = test_data_dir();
        let dir = test_data.join("Movies/Anchorman The Legend of Ron Burgundy (2004)");
        let images = discover_movie_images(&dir).await;

        let keys: Vec<&str> = images.iter().map(|i| i.image_key.as_str()).collect();
        assert!(
            keys.contains(&"backdrop"),
            "should find backdrop (fanart.jpg): {keys:?}"
        );
        assert!(
            keys.contains(&"logo"),
            "should find logo (logo.png): {keys:?}"
        );
        assert!(keys.contains(&"banner"), "should find banner: {keys:?}");
        assert!(
            keys.contains(&"landscape"),
            "should find landscape: {keys:?}"
        );
    }

    #[tokio::test]
    async fn test_discover_tv_show_images() {
        let test_data = test_data_dir();
        let dir = test_data.join("TV/Barry");
        let images = discover_tv_show_images(&dir).await;

        let keys: Vec<&str> = images.iter().map(|i| i.image_key.as_str()).collect();
        assert!(keys.contains(&"poster"), "should find poster: {keys:?}");
        assert!(keys.contains(&"backdrop"), "should find backdrop: {keys:?}");
        assert!(
            keys.contains(&"logo"),
            "should find logo (clearlogo.png): {keys:?}"
        );
        assert!(keys.contains(&"banner"), "should find banner: {keys:?}");
    }

    #[tokio::test]
    async fn test_discover_season_images_barry() {
        let test_data = test_data_dir();
        let dir = test_data.join("TV/Barry");
        let seasons = discover_season_images(&dir).await;

        assert!(seasons.contains_key(&1), "should find season 1 poster");
        assert!(seasons.contains_key(&2), "should find season 2 poster");
        assert!(seasons.contains_key(&3), "should find season 3 poster");
        assert!(seasons.contains_key(&4), "should find season 4 poster");
        assert!(!seasons.contains_key(&0), "Barry has no specials poster");
    }

    #[tokio::test]
    async fn test_discover_season_images_shrinking() {
        let test_data = test_data_dir();
        let dir = test_data.join("TV/Shrinking");
        let seasons = discover_season_images(&dir).await;

        assert!(seasons.contains_key(&1), "should find season 1 poster");
        assert!(seasons.contains_key(&2), "should find season 2 poster");
        assert!(seasons.contains_key(&3), "should find season 3 poster");
        assert!(seasons.contains_key(&0), "Shrinking has specials poster");
    }

    #[tokio::test]
    async fn test_discover_episode_images() {
        let test_data = test_data_dir();
        let episode =
            test_data.join("TV/Barry/Season 01/Barry - 1x01 - Chapter One- Make Your Mark.mkv");
        let images = discover_episode_images(&episode).await;

        let keys: Vec<&str> = images.iter().map(|i| i.image_key.as_str()).collect();
        assert!(
            keys.contains(&"still"),
            "should find episode thumb: {keys:?}"
        );
    }

    #[tokio::test]
    async fn test_discover_book_images_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let images = discover_book_images(dir.path()).await;
        assert!(images.is_empty(), "empty dir should have no cover images");
    }

    #[tokio::test]
    async fn test_sidecar_filename_mapping() {
        // Movies
        assert_eq!(sidecar_filename("poster", "movie"), Some("poster.jpg"));
        assert_eq!(sidecar_filename("backdrop", "movie"), Some("fanart.jpg"));
        assert_eq!(sidecar_filename("logo", "movie"), Some("clearlogo.png"));
        assert_eq!(sidecar_filename("banner", "movie"), Some("banner.jpg"));
        assert_eq!(
            sidecar_filename("landscape", "movie"),
            Some("landscape.jpg")
        );
        assert_eq!(sidecar_filename("unknown", "movie"), None);

        // TV
        assert_eq!(sidecar_filename("poster", "tv"), Some("poster.jpg"));
        assert_eq!(sidecar_filename("backdrop", "tv"), Some("fanart.jpg"));
        assert_eq!(sidecar_filename("logo", "tv"), Some("clearlogo.png"));
        assert_eq!(sidecar_filename("banner", "tv"), Some("banner.jpg"));

        // Books
        assert_eq!(sidecar_filename("cover", "book"), Some("cover.jpg"));
        assert_eq!(sidecar_filename("poster", "book"), None);

        // Unknown media type
        assert_eq!(sidecar_filename("poster", "music"), None);
    }

    #[tokio::test]
    async fn test_discover_movie_images_nonexistent_dir() {
        let dir = std::path::PathBuf::from("/tmp/nonexistent-sidecar-test");
        let images = discover_movie_images(&dir).await;
        assert!(images.is_empty());
    }
}
