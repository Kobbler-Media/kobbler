//! Which of a book's files a reader opens.
//!
//! The web reader's `/file` download and the app's reader pack must agree on
//! the document: foliate writes epubcfis against the file it was served, and
//! the pack's CFI sidecar is built from the file it was built from. Both go
//! through [`select`].

use std::path::PathBuf;

use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::AppState;
use crate::convert::ConversionPipeline;
use crate::repositories::book_files;

/// EPUB before KEPUB: KEPUB is a Kobo artefact (koboSpans around every
/// sentence, the `div#book-columns` wrapper), and when both exist the plain
/// EPUB is the more faithful document — and the one `ebook_segments` is
/// extracted from, so read-along CFIs point where they say they do.
pub const PACK_FORMATS: &[&str] = &["EPUB", "KEPUB"];

pub const DOWNLOAD_FORMATS: &[&str] = &[
    "EPUB", "KEPUB", "AZW3", "AZW", "MOBI", "CBZ", "CBR", "PDF", "FB2",
];

const EPUB_CONVERTIBLE_FORMATS: &[&str] = &[
    "MOBI", "AZW", "AZW3", "FB2", "PDF", "DOCX", "LIT", "PDB", "RTF", "TXT", "HTML",
];

#[derive(Debug, Clone)]
pub struct ReaderFile {
    pub id: Uuid,
    pub path: PathBuf,
    pub format: String,
    pub revision: Uuid,
}

/// The first of `formats` the book has on disk, else an on-demand EPUB
/// conversion of a convertible source.
pub async fn select(state: &AppState, book_id: Uuid, formats: &[&str]) -> Option<ReaderFile> {
    let root = &state.config.storage_root;
    if let Some(file) = find(&state.pool, root, book_id, formats).await {
        return Some(file);
    }
    convert_to_epub(state, book_id).await?;
    find(&state.pool, root, book_id, &["EPUB"]).await
}

/// The first of `formats` the book has on disk.
pub async fn find(
    pool: &PgPool,
    storage_root: &str,
    book_id: Uuid,
    formats: &[&str],
) -> Option<ReaderFile> {
    for fmt in formats {
        if let Ok(Some((id, path, revision))) =
            book_files::get_revision_by_format(pool, book_id, fmt).await
        {
            // A row can outlive its file: the file was deleted from disk and
            // the row was not. Fall through to the next format — or to the
            // on-demand conversion, which rebuilds the file and refreshes the
            // row.
            let full = crate::audio::path::resolve_file_path(storage_root, &path);
            if tokio::fs::try_exists(&full).await.unwrap_or(false) {
                return Some(ReaderFile {
                    id,
                    path: full,
                    format: (*fmt).to_string(),
                    revision,
                });
            }
        }
    }
    None
}

/// The native pipeline (boko) handles the common formats in milliseconds;
/// the rare Calibre fallback (PDF/FB2/RTF/TXT/HTML/DOCX/LIT/PDB) is covered
/// by the client's HTTP timeout.
async fn convert_to_epub(state: &AppState, book_id: Uuid) -> Option<()> {
    let files = book_files::list_by_book(&state.pool, book_id).await.ok()?;
    // The first convertible source that is actually on disk. A row can point
    // at a file that has gone away; skipping it lets a second source save the
    // request instead of 404ing on the first ghost.
    let convertible = files.iter().filter(|f| {
        EPUB_CONVERTIBLE_FORMATS
            .iter()
            .any(|fmt| f.format.eq_ignore_ascii_case(fmt))
    });
    let mut source = None;
    for f in convertible {
        let path = crate::audio::path::resolve_file_path(&state.config.storage_root, &f.file_path);
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            source = Some(f);
            break;
        }
    }
    let source = source?;

    let source_path =
        crate::audio::path::resolve_file_path(&state.config.storage_root, &source.file_path);

    // The staleness baseline for the repair sweep: what the source looked
    // like when this copy was made.
    let source_mtime = tokio::fs::metadata(&source_path)
        .await
        .ok()
        .and_then(|m| m.modified().ok())
        .map(crate::repositories::media_root_files::mtime_from_system_time);

    let output_dir = std::path::Path::new(&state.config.storage_root)
        .join("books")
        .join(book_id.to_string());

    let converted_path = match ConversionPipeline::new()
        .convert_to_epub(&source_path, &output_dir)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, book_id = %book_id, "on-demand EPUB conversion failed");
            return None;
        }
    };

    let file_size = tokio::fs::metadata(&converted_path)
        .await
        .map(|m| m.len() as i64)
        .unwrap_or(0);

    let row_id = match book_files::upsert_full(
        &state.pool,
        book_files::BookFileFullInput {
            book_id,
            format: "EPUB".to_string(),
            size_bytes: file_size,
            file_path: converted_path.to_string_lossy().to_string(),
            source_path: Some(source.file_path.clone()),
            source_mtime,
            file_hash: None,
            source_type: Some("on_demand_conversion".to_string()),
            source_metadata: Some(json!({ "source_format": source.format })),
        },
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(error = %e, book_id = %book_id, "failed to register converted EPUB");
            return None;
        }
    };

    // The file was just written; if the row was a ghost marked unavailable,
    // that state is stale now rather than waiting for the next availability
    // pass.
    let _ = book_files::set_available(&state.pool, row_id).await;

    tracing::info!(
        book_id = %book_id,
        source_format = %source.format,
        "on-demand EPUB conversion succeeded"
    );
    crate::jobs::enqueue_reader_pack(&state.pool, state.nats.publisher(), book_id).await;
    Some(())
}
