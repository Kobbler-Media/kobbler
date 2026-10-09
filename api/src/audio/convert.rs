//! Audiobook conversion: M4B to MP3Z for Kobo devices.
//!
//! Splits an audiobook into per-chapter MP3 files using kob-ffmpeg, then
//! bundles them into an MP3Z (ZIP of MP3s) that Kobo readers can play.

use std::path::{Path, PathBuf};

use tracing::info;

use crate::repositories::audiobook_chapters::ChapterInput;

/// Errors from audiobook conversion.
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ffmpeg failed: {0}")]
    FfmpegFailed(#[from] kob_ffmpeg::FfmpegError),
    #[error("zip error: {0}")]
    ZipError(String),
    #[error("task join error: {0}")]
    JoinError(String),
}

/// Split an audiobook into per-chapter MP3 files using kob-ffmpeg.
///
/// Each chapter is extracted as `chapter_NN.mp3` in `output_dir`.
/// Returns the list of created MP3 file paths, in chapter order.
pub async fn split_to_mp3_chapters(
    input_path: &Path,
    chapters: &[ChapterInput],
    output_dir: &Path,
) -> Result<Vec<PathBuf>, ConvertError> {
    tokio::fs::create_dir_all(output_dir).await?;

    // kob-ffmpeg's split_chapters_to_mp3 uses embedded chapters from the file.
    // We need per-chapter extraction with caller-provided chapter boundaries,
    // so we use the lower-level audio segment extraction for each chapter.
    let input = input_path.to_owned();
    let out_dir = output_dir.to_owned();
    // Collect just what we need to avoid requiring Clone on ChapterInput
    let chapter_data: Vec<(i32, f64, f64)> = chapters
        .iter()
        .map(|ch| (ch.chapter_index, ch.start_seconds, ch.end_seconds))
        .collect();

    tokio::task::spawn_blocking(move || {
        let mut paths = Vec::with_capacity(chapter_data.len());

        for &(chapter_index, start_seconds, end_seconds) in &chapter_data {
            let filename = format!("chapter_{:03}.mp3", chapter_index);
            let output_path = out_dir.join(&filename);

            // Use kob-ffmpeg probe + audio segment extraction
            kob_ffmpeg::audio::split_audio_segment(
                &input,
                &output_path,
                start_seconds,
                end_seconds,
                "libmp3lame",
                2, // quality (VBR ~190kbps)
                None,
            )?;

            info!(
                chapter = chapter_index,
                path = %output_path.display(),
                "extracted chapter MP3"
            );
            paths.push(output_path);
        }

        Ok(paths)
    })
    .await
    .map_err(|e| ConvertError::JoinError(e.to_string()))?
}

/// Convert any audio file to WAV (16-bit PCM, mono, 22050 Hz).
pub async fn convert_to_wav(input_path: &Path, output_path: &Path) -> Result<(), ConvertError> {
    let input = input_path.to_owned();
    let output = output_path.to_owned();

    tokio::task::spawn_blocking(move || {
        kob_ffmpeg::audio::convert_to_wav(&input, &output, None).map_err(ConvertError::FfmpegFailed)
    })
    .await
    .map_err(|e| ConvertError::JoinError(e.to_string()))?
}

/// Bundle MP3 chapter files into an MP3Z (ZIP archive) for Kobo devices.
pub async fn create_mp3z(mp3_dir: &Path, output_path: &Path) -> Result<(), ConvertError> {
    let mp3_dir = mp3_dir.to_owned();
    let output_path = output_path.to_owned();

    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::create(&output_path)?;
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        let mut entries: Vec<_> = std::fs::read_dir(&mp3_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("mp3"))
            })
            .collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let path = entry.path();
            let name = entry.file_name();
            zip.start_file(name.to_string_lossy(), options)
                .map_err(|e| ConvertError::ZipError(e.to_string()))?;
            let data = std::fs::read(&path)?;
            std::io::Write::write_all(&mut zip, &data)?;
        }

        zip.finish()
            .map_err(|e| ConvertError::ZipError(e.to_string()))?;
        Ok(())
    })
    .await
    .map_err(|e| ConvertError::ZipError(format!("task join error: {e}")))?
}
