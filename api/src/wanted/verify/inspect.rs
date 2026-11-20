use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;
use uuid::Uuid;

use super::judge::{Found, name_tokens, surname};

/// The opening of a book: title page, copyright page, the first chapters. A
/// name that appears nowhere in it is not the book's author.
const OPENING_SEGMENTS: usize = 12;
const OPENING_PDF_PAGES: u32 = 12;
/// Audiobooks open with "Title, written by Author, read by Narrator".
const OPENING_SECONDS: u32 = 240;
const STT_TIMEOUT: Duration = Duration::from_secs(600);

const AUDIO_EXTENSIONS: &[&str] = &["m4b", "m4a", "mp3", "flac", "ogg", "opus", "aac", "wav"];

pub struct Inspector {
    /// Speech-to-text service for audiobooks that name no author.
    pub stt_url: Option<String>,
    /// Where intermediate files go. Must be visible to the STT service, which
    /// reads by path.
    pub scratch_dir: PathBuf,
}

/// What the file says about itself, or `None` when it can't be read. An
/// unreadable file is not evidence either way.
pub async fn inspect(path: &Path, authors: &[String], inspector: &Inspector) -> Option<Found> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    let surnames: Vec<String> = authors.iter().filter_map(|a| surname(a)).collect();

    if AUDIO_EXTENSIONS.contains(&ext.as_str()) {
        let probe = crate::audio::probe::probe_audio(path).await.ok()?;
        let mentions_author = match (&probe.author, surnames.is_empty()) {
            (None, false) => narration_mentions(path, &surnames, inspector).await,
            _ => None,
        };
        return Some(Found {
            title: probe.album.or(probe.title),
            authors: probe.author.into_iter().collect(),
            year: probe.year.as_deref().and_then(year_of),
            language: probe.language,
            mentions_author,
        });
    }

    let mut found = if ext == "pdf" {
        pdf_info(path).await
    } else {
        let owned = path.to_path_buf();
        let meta = tokio::task::spawn_blocking(move || kob_ebook::read_metadata(&owned))
            .await
            .ok()?
            .ok()?;
        Found {
            title: meta.title.filter(|t| !t.trim().is_empty()),
            authors: meta.authors.into_iter().filter(|a| named(a)).collect(),
            year: meta.pubdate.as_deref().and_then(year_of),
            language: meta.language,
            mentions_author: None,
        }
    };
    if found.authors.is_empty() && !surnames.is_empty() {
        found.mentions_author = opening_text(path, &ext, inspector)
            .await
            .map(|text| mentions(&text, &surnames));
    }
    Some(found)
}

/// Converters write "Unknown" where a book names no author.
fn named(author: &str) -> bool {
    let author = author.trim();
    !author.is_empty() && !author.eq_ignore_ascii_case("unknown")
}

fn year_of(date: &str) -> Option<i32> {
    date.trim()
        .get(..4)
        .and_then(|y| y.parse().ok())
        .filter(|y| (1450..=2200).contains(y))
}

fn mentions(text: &str, surnames: &[String]) -> bool {
    let words = name_tokens(text);
    surnames.iter().any(|s| words.contains(s))
}

async fn opening_text(path: &Path, ext: &str, inspector: &Inspector) -> Option<String> {
    match ext {
        "epub" | "kepub" => epub_opening(path.to_path_buf()).await,
        "pdf" => {
            let out = Command::new("pdftotext")
                .args(["-l", &OPENING_PDF_PAGES.to_string()])
                .arg(path)
                .arg("-")
                .output()
                .await
                .ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
        }
        _ if kob_ebook::native_supports(path) => {
            let owned = path.to_path_buf();
            let epub =
                tokio::task::spawn_blocking(move || kob_ebook::convert_to_epub_bytes(&owned))
                    .await
                    .ok()?
                    .ok()?;
            let converted = inspector
                .scratch_dir
                .join(format!("{}.epub", Uuid::now_v7()));
            tokio::fs::write(&converted, epub).await.ok()?;
            let text = epub_opening(converted.clone()).await;
            let _ = tokio::fs::remove_file(&converted).await;
            text
        }
        _ => None,
    }
}

async fn epub_opening(path: PathBuf) -> Option<String> {
    tokio::task::spawn_blocking(move || {
        crate::ebook::extract::extract_epub_text(&path)
            .ok()
            .map(|segments| {
                segments
                    .into_iter()
                    .take(OPENING_SEGMENTS)
                    .map(|s| s.text)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
    })
    .await
    .ok()
    .flatten()
}

async fn pdf_info(path: &Path) -> Found {
    let Ok(out) = Command::new("pdfinfo").arg(path).output().await else {
        return Found::default();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let field = |name: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(name))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    Found {
        title: field("Title:"),
        authors: field("Author:").into_iter().filter(|a| named(a)).collect(),
        year: field("CreationDate:").and_then(|d| {
            d.split_whitespace()
                .find_map(|w| w.parse::<i32>().ok().filter(|y| (1450..=2200).contains(y)))
        }),
        ..Found::default()
    }
}

async fn narration_mentions(
    path: &Path,
    surnames: &[String],
    inspector: &Inspector,
) -> Option<bool> {
    let stt_url = inspector.stt_url.as_deref()?;
    let clip = inspector
        .scratch_dir
        .join(format!("{}-opening.wav", Uuid::now_v7()));
    let cut = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-t",
            &OPENING_SECONDS.to_string(),
            "-i",
        ])
        .arg(path)
        .args(["-ac", "1", "-ar", "16000"])
        .arg(&clip)
        .status()
        .await
        .ok()?;
    let result = if cut.success() {
        tokio::time::timeout(
            STT_TIMEOUT,
            crate::audio::stt::transcribe_audio(stt_url, &clip),
        )
        .await
        .ok()
        .and_then(|r| r.ok())
    } else {
        None
    };
    let _ = tokio::fs::remove_file(&clip).await;
    let text = result?
        .segments
        .into_iter()
        .map(|s| s.text)
        .collect::<Vec<_>>()
        .join(" ");
    Some(mentions(&text, surnames))
}
