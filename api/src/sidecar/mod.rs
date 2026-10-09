//! Sidecar metadata parsing and writing.
//!
//! Reads metadata from sidecar files adjacent to media files (Kodi NFO, Audiobookshelf JSON,
//! Calibre OPF) and writes them back after enrichment.

pub mod book_json;
pub mod book_opf;
pub mod images;
pub mod nfo;
pub mod write_back;

use crate::media_root_naming::sanitize_for_filesystem;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Which sidecar format a piece of metadata came from or should be written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidecarFormat {
    Nfo,
    AudiobookshelfJson,
    CalibreOpf,
}

/// The type of NFO file — determines both the root XML element and the filename.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NfoType {
    Movie,
    TvShow,
    Episode,
}

impl NfoType {
    /// The root XML element for this NFO type.
    pub fn root_tag(self) -> &'static str {
        match self {
            NfoType::Movie => "movie",
            NfoType::TvShow => "tvshow",
            NfoType::Episode => "episodedetails",
        }
    }

    /// The filename to use for this NFO type.
    pub fn filename(self, stem: &str) -> String {
        match self {
            NfoType::Movie => "movie.nfo".to_string(),
            NfoType::TvShow => "tvshow.nfo".to_string(),
            NfoType::Episode => format!("{}.nfo", sanitize_for_filesystem(stem)),
        }
    }
}

/// Infer the NFO type from metadata fields. Used as a fallback when `nfo_type`
/// is not explicitly set (e.g. externally-authored NFOs parsed without a root element).
pub fn infer_nfo_type(meta: &SidecarMetadata) -> NfoType {
    if let Some(ref v) = meta.video {
        if v.season.is_some() && v.episode.is_some() {
            return NfoType::Episode;
        }
        if v.status.is_some() {
            return NfoType::TvShow;
        }
    }
    NfoType::Movie
}

/// What kind of media we're looking for sidecars of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
    Movie,
    TvShow,
    TvEpisode,
    Book,
}

/// A cast member from a sidecar file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CastEntry {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb: Option<String>,
}

/// External identifiers extracted from a sidecar.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SidecarIdentifiers {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tvmaze_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isbn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openlibrary_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hardcover_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kobbler_id: Option<String>,
    /// Unknown uniqueid types preserved for round-trip (e.g. sonarr, radarr).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_uniqueids: Vec<(String, String)>,
}

impl SidecarIdentifiers {
    pub fn has_any(&self) -> bool {
        self.imdb_id.is_some()
            || self.tmdb_id.is_some()
            || self.tvdb_id.is_some()
            || self.tvmaze_id.is_some()
            || self.isbn.is_some()
            || self.asin.is_some()
            || self.openlibrary_id.is_some()
            || self.hardcover_id.is_some()
            || self.kobbler_id.is_some()
    }
}

/// Video-specific metadata from a sidecar.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VideoSidecar {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nfo_type: Option<NfoType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aired: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpaa: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub studio: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cast: Vec<CastEntry>,
}

/// Book-specific metadata from a sidecar.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BookSidecar {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub narrators: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_index: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub abridged: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chapters: Vec<SidecarChapter>,
}

/// A chapter entry from a sidecar file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarChapter {
    pub title: Option<String>,
    pub start: f64,
    pub end: f64,
}

/// Metadata extracted from a sidecar file, unified across formats.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SidecarMetadata {
    pub source_format: Option<SidecarFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    #[serde(default)]
    pub identifiers: SidecarIdentifiers,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoSidecar>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub book: Option<BookSidecar>,
}

/// Discover and parse sidecar files adjacent to a media file.
///
/// Returns the first successfully parsed sidecar, preferring more specific files
/// (e.g. `{stem}.nfo` over `movie.nfo`).
pub async fn read_sidecar(media_path: &Path, media_type: MediaType) -> Option<SidecarMetadata> {
    let parent = media_path.parent()?;

    match media_type {
        MediaType::Movie => {
            // Try {filename}.nfo first, then movie.nfo
            if let Some(stem) = media_path.file_stem().and_then(|s| s.to_str()) {
                let specific_nfo = parent.join(format!("{stem}.nfo"));
                if let Some(meta) = try_parse_nfo(&specific_nfo).await {
                    return Some(meta);
                }
            }
            let movie_nfo = parent.join("movie.nfo");
            try_parse_nfo(&movie_nfo).await
        }
        MediaType::TvShow => {
            let tvshow_nfo = parent.join("tvshow.nfo");
            try_parse_nfo(&tvshow_nfo).await
        }
        MediaType::TvEpisode => {
            if let Some(stem) = media_path.file_stem().and_then(|s| s.to_str()) {
                let episode_nfo = parent.join(format!("{stem}.nfo"));
                try_parse_nfo(&episode_nfo).await
            } else {
                None
            }
        }
        MediaType::Book => {
            // Try metadata.json first, then metadata.opf
            let json_path = parent.join("metadata.json");
            if let Some(meta) = try_parse_book_json(&json_path).await {
                return Some(meta);
            }
            let opf_path = parent.join("metadata.opf");
            try_parse_book_opf(&opf_path).await
        }
    }
}

/// Discover a tvshow.nfo in a directory (for TV show root detection during scan).
pub async fn read_tvshow_nfo(dir: &Path) -> Option<SidecarMetadata> {
    let tvshow_nfo = dir.join("tvshow.nfo");
    try_parse_nfo(&tvshow_nfo).await
}

/// Write sidecar files for a matched/enriched entity.
///
/// Returns paths of files actually written.
pub async fn write_sidecar(
    dir: &Path,
    filename_stem: &str,
    metadata: &SidecarMetadata,
    formats: &[SidecarFormat],
) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut written = Vec::new();

    for format in formats {
        match format {
            SidecarFormat::Nfo => {
                let nfo_t = metadata
                    .video
                    .as_ref()
                    .and_then(|v| v.nfo_type)
                    .unwrap_or_else(|| infer_nfo_type(metadata));
                let nfo_name = nfo_t.filename(filename_stem);
                let path = dir.join(&nfo_name);
                match nfo::write_nfo(&path, metadata).await {
                    Ok(()) => written.push(path),
                    Err(e) => warn!(path = %path.display(), error = %e, "Failed to write NFO"),
                }
            }
            SidecarFormat::AudiobookshelfJson => {
                let path = dir.join("metadata.json");
                match book_json::write_metadata_json(&path, metadata).await {
                    Ok(()) => written.push(path),
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "Failed to write metadata.json")
                    }
                }
            }
            SidecarFormat::CalibreOpf => {
                let path = dir.join("metadata.opf");
                match book_opf::write_opf(&path, metadata).await {
                    Ok(()) => written.push(path),
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "Failed to write metadata.opf")
                    }
                }
            }
        }
    }

    Ok(written)
}

async fn try_parse_nfo(path: &Path) -> Option<SidecarMetadata> {
    let path = path.to_owned();
    let result = tokio::task::spawn_blocking(move || {
        if !path.exists() {
            return None;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                debug!(path = %path.display(), error = %e, "Failed to read NFO file");
                return None;
            }
        };
        match nfo::parse_nfo(&content) {
            Ok(meta) => Some(meta),
            Err(e) => {
                debug!(path = %path.display(), error = %e, "Failed to parse NFO file");
                None
            }
        }
    })
    .await;
    result.ok().flatten()
}

async fn try_parse_book_json(path: &Path) -> Option<SidecarMetadata> {
    let path = path.to_owned();
    let result = tokio::task::spawn_blocking(move || {
        if !path.exists() {
            return None;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                debug!(path = %path.display(), error = %e, "Failed to read metadata.json");
                return None;
            }
        };
        match book_json::parse_metadata_json(&content) {
            Ok(meta) => Some(meta),
            Err(e) => {
                debug!(path = %path.display(), error = %e, "Failed to parse metadata.json");
                None
            }
        }
    })
    .await;
    result.ok().flatten()
}

async fn try_parse_book_opf(path: &Path) -> Option<SidecarMetadata> {
    let path = path.to_owned();
    let result = tokio::task::spawn_blocking(move || {
        if !path.exists() {
            return None;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                debug!(path = %path.display(), error = %e, "Failed to read metadata.opf");
                return None;
            }
        };
        match book_opf::parse_opf(&content) {
            Ok(meta) => Some(meta),
            Err(e) => {
                debug!(path = %path.display(), error = %e, "Failed to parse metadata.opf");
                None
            }
        }
    })
    .await;
    result.ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sidecar_identifiers_has_any() {
        let empty = SidecarIdentifiers::default();
        assert!(!empty.has_any());

        let with_tmdb = SidecarIdentifiers {
            tmdb_id: Some("12345".to_string()),
            ..Default::default()
        };
        assert!(with_tmdb.has_any());
    }

    #[test]
    fn test_infer_nfo_type_movie() {
        let meta = SidecarMetadata {
            video: Some(VideoSidecar::default()),
            ..Default::default()
        };
        assert_eq!(infer_nfo_type(&meta), NfoType::Movie);
    }

    #[test]
    fn test_infer_nfo_type_tvshow() {
        let meta = SidecarMetadata {
            video: Some(VideoSidecar {
                status: Some("Returning Series".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(infer_nfo_type(&meta), NfoType::TvShow);
    }

    #[test]
    fn test_infer_nfo_type_episode() {
        let meta = SidecarMetadata {
            video: Some(VideoSidecar {
                season: Some(1),
                episode: Some(3),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(infer_nfo_type(&meta), NfoType::Episode);
    }

    #[test]
    fn test_nfo_type_filename_movie() {
        assert_eq!(NfoType::Movie.filename("anything"), "movie.nfo");
    }

    #[test]
    fn test_nfo_type_filename_tvshow() {
        assert_eq!(NfoType::TvShow.filename("anything"), "tvshow.nfo");
    }

    #[test]
    fn test_nfo_type_filename_episode() {
        assert_eq!(NfoType::Episode.filename("S01E03"), "S01E03.nfo");
    }

    #[test]
    fn test_nfo_type_root_tag() {
        assert_eq!(NfoType::Movie.root_tag(), "movie");
        assert_eq!(NfoType::TvShow.root_tag(), "tvshow");
        assert_eq!(NfoType::Episode.root_tag(), "episodedetails");
    }
}
