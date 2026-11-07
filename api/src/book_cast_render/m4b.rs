use std::path::{Path, PathBuf};

use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

pub struct Marker {
    pub title: String,
    pub file: PathBuf,
    pub duration_ms: u64,
}

pub struct Tags {
    pub title: String,
    pub author: Option<String>,
}

pub const NARRATOR: &str = "Full cast";

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '=' | ';' | '#' | '\\' | '\n' => {
                out.push('\\');
                out.push(c);
            }
            '\r' => {}
            _ => out.push(c),
        }
    }
    out
}

/// The probe that reads the m4b back on import takes `composer` as the
/// narrator only when it differs from the author (`album_artist`).
pub fn ffmetadata(tags: &Tags, markers: &[Marker]) -> String {
    let mut out = String::from(";FFMETADATA1\n");
    out.push_str(&format!("title={}\n", escape(&tags.title)));
    out.push_str(&format!("album={}\n", escape(&tags.title)));
    if let Some(author) = &tags.author {
        out.push_str(&format!("artist={}\n", escape(author)));
        out.push_str(&format!("album_artist={}\n", escape(author)));
    }
    out.push_str(&format!("composer={NARRATOR}\ngenre=Audiobook\n"));
    let mut start = 0;
    for m in markers {
        let end = start + m.duration_ms;
        out.push_str(&format!(
            "\n[CHAPTER]\nTIMEBASE=1/1000\nSTART={start}\nEND={end}\ntitle={}\n",
            escape(&m.title)
        ));
        start = end;
    }
    out
}

/// ADTS frames stand alone, so the chapters join by plain concatenation and
/// the whole book is remuxed once, without re-encoding.
pub async fn mux(tags: &Tags, markers: &[Marker], work: &Path, out: &Path) -> Result<(), String> {
    let joined = work.join("book.aac");
    let meta = work.join("book.ffmetadata");
    let err = |e: std::io::Error| e.to_string();
    let mut whole = fs::File::create(&joined).await.map_err(err)?;
    for m in markers {
        let bytes = fs::read(&m.file).await.map_err(err)?;
        whole.write_all(&bytes).await.map_err(err)?;
    }
    whole.flush().await.map_err(err)?;
    fs::write(&meta, ffmetadata(tags, markers))
        .await
        .map_err(err)?;

    let output = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "aac",
            "-i",
        ])
        .arg(&joined)
        .args(["-f", "ffmetadata", "-i"])
        .arg(&meta)
        .args([
            "-map",
            "0:a",
            "-map_metadata",
            "1",
            "-map_chapters",
            "1",
            "-c",
            "copy",
            "-movflags",
            "+faststart",
            "-f",
            "ipod",
        ])
        .arg(out)
        .output()
        .await
        .map_err(err)?;
    let _ = fs::remove_file(&joined).await;
    let _ = fs::remove_file(&meta).await;
    if !output.status.success() {
        let _ = fs::remove_file(out).await;
        return Err(format!(
            "muxing the m4b failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}
