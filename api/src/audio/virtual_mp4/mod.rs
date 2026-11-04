mod moov;
mod scan;
mod store;

use std::path::Path;

use bytes::Bytes;
use uuid::Uuid;

use crate::streaming::byte_range::Part;

#[derive(Debug, thiserror::Error)]
pub enum VirtualMp4Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ffmpeg failed: {0}")]
    Ffmpeg(#[from] kob_ffmpeg::FfmpegError),
    #[error("cannot index as mp3: {0}")]
    Unsupported(String),
    #[error("task join error: {0}")]
    Join(String),
}

#[derive(Debug, Clone)]
pub struct VirtualMp4 {
    header: Bytes,
    source_start: u64,
    source_len: u64,
}

impl VirtualMp4 {
    pub fn parts(&self, source: tokio::fs::File) -> Vec<Part> {
        vec![
            Part::Bytes(self.header.clone()),
            Part::File {
                file: source,
                offset: self.source_start,
                len: self.source_len,
            },
        ]
    }
}

// Browsers and AVFoundation place a seek in an MP3 by its Xing TOC or average bitrate and
// then report the requested time, so on VBR files the clock runs seconds off the audio. The
// same frames behind an MP4 sample table seek exactly everywhere. Only ftyp+moov is stored;
// the mdat payload is the source's own frame bytes, served in place.
pub async fn prepare(
    storage_root: &str,
    file_id: Uuid,
    source: &Path,
) -> Result<VirtualMp4, VirtualMp4Error> {
    let index = Path::new(storage_root)
        .join("audio_mp4_index")
        .join(file_id.to_string());
    let _lock = crate::jobs::file_lock::lock_file(file_id).await;
    let stamp = store::stamp(source).await?;
    if let Some(fresh) = store::load(&index, stamp).await {
        return Ok(fresh);
    }
    let path = source.to_owned();
    let mp4 = tokio::task::spawn_blocking(move || build(&path))
        .await
        .map_err(|e| VirtualMp4Error::Join(e.to_string()))??;
    store::save(&index, &mp4, stamp).await?;
    Ok(mp4)
}

fn build(source: &Path) -> Result<VirtualMp4, VirtualMp4Error> {
    let table = scan::scan(source)?;
    Ok(VirtualMp4 {
        header: moov::header(&table).into(),
        source_start: table.first_byte(),
        source_len: table.end_byte() - table.first_byte(),
    })
}
