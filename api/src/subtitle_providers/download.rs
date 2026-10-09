//! Shared subtitle download-and-save logic.
//!
//! Used by both the HTTP download endpoint and the auto-download path in the
//! subtitle search job handler.

use std::path::PathBuf;

use sqlx::PgPool;
use uuid::Uuid;

use super::SubtitleProviderRegistry;
use crate::repositories::external_subtitles;

/// Errors from subtitle download operations.
#[derive(Debug, thiserror::Error)]
pub enum SubtitleDownloadError {
    #[error("Provider error: {0}")]
    Provider(#[from] super::SubtitleProviderError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Database error: {0}")]
    Database(#[from] crate::repositories::RepositoryError),
    #[error("{0}")]
    Other(String),
}

/// Download a subtitle from a provider and save it alongside the video file.
///
/// Returns `(subtitle_record_id, file_path)` on success.
#[allow(clippy::too_many_arguments)]
pub async fn download_and_save_subtitle(
    pool: &PgPool,
    registry: &SubtitleProviderRegistry,
    provider: &str,
    subtitle_id: &str,
    video_file_path: &str,
    movie_file_id: Option<Uuid>,
    episode_file_id: Option<Uuid>,
    language: Option<&str>,
    is_forced: bool,
    is_hearing_impaired: bool,
) -> Result<(Uuid, String), SubtitleDownloadError> {
    let download = registry.download(provider, subtitle_id).await?;

    let video_path = PathBuf::from(video_file_path);
    let video_stem = video_path.file_stem().unwrap_or_default().to_string_lossy();
    let video_dir = video_path
        .parent()
        .ok_or_else(|| SubtitleDownloadError::Other("Cannot determine video directory".into()))?;

    let lang_part = language.unwrap_or("und");
    let forced_part = if is_forced { ".forced" } else { "" };
    let sub_filename = format!(
        "{}.{}{}.{}",
        video_stem, lang_part, forced_part, download.format
    );
    let sub_path = video_dir.join(&sub_filename);

    tokio::fs::write(&sub_path, &download.content).await?;

    let file_size = download.content.len() as i64;

    let input = external_subtitles::ExternalSubtitleInput {
        movie_file_id,
        episode_file_id,
        file_path: sub_path.to_string_lossy().to_string(),
        file_size: Some(file_size),
        format: download.format,
        language: language.map(String::from),
        title: None,
        is_forced,
        is_hearing_impaired,
        source: provider.to_string(),
        source_id: Some(subtitle_id.to_string()),
        source_metadata: None,
    };

    let sub_id = external_subtitles::create(pool, &input).await?;

    Ok((sub_id, sub_path.to_string_lossy().to_string()))
}
