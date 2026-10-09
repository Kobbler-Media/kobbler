//! MP3 header repair: ensure files have a Xing/Info VBR header.
//!
//! Why: MP3s without a Xing/Info header force consumers (web browsers'
//! `<audio>` seek logic, ffprobe, etc.) to estimate duration from bitrate.
//! When the estimate disagrees with reality (common with old transcoders
//! that didn't write VBR tags), browser seeks land at the wrong byte
//! position and read-along desyncs from the audio. Adding the header makes
//! seek byte-accurate.

use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::process::Command;
use tracing::info;

#[derive(Debug, thiserror::Error)]
pub enum RepairError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ffmpeg failed: {0}")]
    Ffmpeg(String),
    #[error("verify failed: {0}")]
    Verify(String),
}

/// Ensure an MP3 file has a Xing/Info header. No-op for non-MP3 files or
/// files that already have one. Returns `Ok(true)` if the file was repaired.
///
/// Uses `ffmpeg -c copy -write_xing 1` so audio frames are byte-identical;
/// only the leading header changes. Repair is atomic (write to temp file
/// alongside, then rename).
pub async fn ensure_xing_header(path: &Path) -> Result<bool, RepairError> {
    if !is_mp3_extension(path) {
        return Ok(false);
    }
    if has_xing_header(path).await? {
        return Ok(false);
    }

    let tmp_path = tmp_path_for(path);

    let output = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-c", "copy", "-write_xing", "1", "-f", "mp3"])
        .arg(&tmp_path)
        .output()
        .await?;

    if !output.status.success() {
        let _ = fs::remove_file(&tmp_path).await;
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(RepairError::Ffmpeg(stderr));
    }

    if !has_xing_header(&tmp_path).await? {
        let _ = fs::remove_file(&tmp_path).await;
        return Err(RepairError::Verify(
            "Xing header not present after ffmpeg remux".into(),
        ));
    }

    // Preserve original permissions
    if let Ok(meta) = fs::metadata(path).await {
        let _ = fs::set_permissions(&tmp_path, meta.permissions()).await;
    }
    fs::rename(&tmp_path, path).await?;
    info!(path = %path.display(), "added missing Xing header to MP3");
    Ok(true)
}

fn is_mp3_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mp3"))
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".xing.tmp.mp3");
    PathBuf::from(s)
}

async fn has_xing_header(path: &Path) -> Result<bool, std::io::Error> {
    let mut f = fs::File::open(path).await?;
    let mut head = Vec::with_capacity(16384);
    (&mut f).take(16384).read_to_end(&mut head).await?;

    let mut offset = 0usize;
    if head.len() >= 10 && &head[..3] == b"ID3" {
        let sz = (((head[6] & 0x7F) as usize) << 21)
            | (((head[7] & 0x7F) as usize) << 14)
            | (((head[8] & 0x7F) as usize) << 7)
            | ((head[9] & 0x7F) as usize);
        offset = 10 + sz;
        if offset >= head.len() {
            // ID3 tag larger than our initial read — re-read at offset
            f.seek(std::io::SeekFrom::Start(offset as u64)).await?;
            head.clear();
            (&mut f).take(2048).read_to_end(&mut head).await?;
            offset = 0;
        }
    }

    if offset >= head.len() {
        return Ok(false);
    }
    let end = (offset + 1500).min(head.len());
    let window = &head[offset..end];
    Ok(contains_subseq(window, b"Xing") || contains_subseq(window, b"Info"))
}

fn contains_subseq(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_xing_in_window() {
        let mut buf = vec![0u8; 200];
        buf[100..104].copy_from_slice(b"Xing");
        assert!(contains_subseq(&buf, b"Xing"));
        assert!(!contains_subseq(&buf, b"Info"));
    }

    #[test]
    fn detects_info_in_window() {
        let mut buf = vec![0u8; 200];
        buf[50..54].copy_from_slice(b"Info");
        assert!(contains_subseq(&buf, b"Info"));
    }

    #[test]
    fn rejects_short_data() {
        assert!(!contains_subseq(b"Xi", b"Xing"));
        assert!(!contains_subseq(b"", b"Xing"));
    }

    #[test]
    fn is_mp3_case_insensitive() {
        assert!(is_mp3_extension(Path::new("foo.mp3")));
        assert!(is_mp3_extension(Path::new("foo.MP3")));
        assert!(is_mp3_extension(Path::new("foo.Mp3")));
        assert!(!is_mp3_extension(Path::new("foo.m4a")));
        assert!(!is_mp3_extension(Path::new("foo")));
    }

    #[test]
    fn tmp_path_preserves_dir() {
        let p = Path::new("/some/dir/file.mp3");
        let t = tmp_path_for(p);
        assert_eq!(t.parent(), Some(Path::new("/some/dir")));
        assert_eq!(t.file_name().unwrap(), "file.mp3.xing.tmp.mp3");
    }
}
