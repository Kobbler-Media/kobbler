//! Book move operations — relocating books between storage locations and media roots.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::path::Path as FsPath;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::media_root_naming::{self, NamingContext};
use crate::repositories::{audiobook_files, book_files, books, media_root_files, media_roots};
use crate::routes::AuthUser;
use sqlx::Row;

#[derive(Deserialize)]
pub struct MoveToMediaRootRequest {
    pub media_root_id: Uuid,
}

#[derive(Serialize)]
struct MoveResult {
    moved_files: usize,
    skipped: usize,
    errors: Vec<String>,
}

/// File to be moved with its current path and DB ID.
struct FileToMove {
    id: Uuid,
    path: String,
    kind: FileKind,
    /// DB format (e.g. "EPUB", "KEPUB", "PDF") — used to derive the file extension.
    format: Option<String>,
}

enum FileKind {
    BookFile,
    AudiobookFile,
}

/// POST /api/v1/admin/books/{book_id}/move-to-media-root
pub async fn move_book_to_media_root(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<Uuid>,
    Json(payload): Json<MoveToMediaRootRequest>,
) -> impl IntoResponse {
    // Load media root
    let root = match media_roots::get_by_id(&state.pool, payload.media_root_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, "Media root not found").into_response();
        }
        Err(e) => {
            error!(error = %e, "Failed to load media root");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    // Verify media type is compatible with books
    if root.media_type != "book" && root.media_type != "audiobook" {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "Media root type '{}' is not compatible with books",
                root.media_type
            ),
        )
            .into_response();
    }

    // Load book metadata for naming template (runtime query, same pattern as media_root_reorganize)
    let book_meta = match sqlx::query(
        r#"
        SELECT
            b.title,
            (SELECT a.name FROM book_authors ba JOIN authors a ON a.id = ba.author_id
             WHERE ba.book_id = b.id ORDER BY ba.author_id LIMIT 1) AS author,
            s.name AS series_name,
            bs.series_index
        FROM books b
        LEFT JOIN book_series bs ON bs.book_id = b.id AND bs.is_primary = TRUE
        LEFT JOIN series s ON s.id = bs.series_id
        WHERE b.id = $1
        "#,
    )
    .bind(book_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, "Book not found").into_response();
        }
        Err(e) => {
            error!(error = %e, "Failed to load book metadata");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    let book_title: String = book_meta.get("title");
    let book_author: Option<String> = book_meta.get("author");
    let book_series: Option<String> = book_meta.get("series_name");
    let book_series_index: Option<f64> = book_meta.get("series_index");

    let template = root
        .naming_template
        .as_deref()
        .unwrap_or_else(|| media_root_naming::default_book_template());

    // Collect all files to move
    let mut files_to_move: Vec<FileToMove> = Vec::new();

    match book_files::list_by_book(&state.pool, book_id).await {
        Ok(files) => {
            for f in files {
                files_to_move.push(FileToMove {
                    id: f.id,
                    path: f.file_path,
                    kind: FileKind::BookFile,
                    format: Some(f.format),
                });
            }
        }
        Err(e) => {
            error!(error = %e, "Failed to list book files");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    }

    match audiobook_files::list_by_book(&state.pool, book_id).await {
        Ok(files) => {
            for f in files {
                files_to_move.push(FileToMove {
                    id: f.id,
                    path: f.file_path,
                    kind: FileKind::AudiobookFile,
                    format: Some(f.format),
                });
            }
        }
        Err(e) => {
            error!(error = %e, "Failed to list audiobook files");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    }

    // If no files, just update media_root_id
    if files_to_move.is_empty() {
        if let Err(e) = books::set_media_root_id(&state.pool, book_id, Some(root.id)).await {
            error!(error = %e, "Failed to set media_root_id");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
        return Json(MoveResult {
            moved_files: 0,
            skipped: 0,
            errors: vec![],
        })
        .into_response();
    }

    // Phase 1: Compute all target paths and check for conflicts
    let mut moves: Vec<(usize, std::path::PathBuf)> = Vec::new();
    let mut skipped = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for (i, file) in files_to_move.iter().enumerate() {
        let ext = file
            .format
            .as_deref()
            .map(media_root_naming::format_to_extension)
            .or_else(|| {
                FsPath::new(&file.path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s.to_string())
            });

        let ctx = NamingContext {
            title: Some(book_title.clone()),
            author: book_author.clone(),
            series: book_series.clone(),
            series_index: book_series_index.map(|i| i as u32),
            ext,
            ..Default::default()
        };

        let target = media_root_naming::compute_target_path(&root.path, template, &ctx);
        let target_str = target.to_string_lossy().to_string();

        if target_str == file.path {
            skipped += 1;
            continue;
        }

        if tokio::fs::try_exists(&target).await.unwrap_or(false) {
            errors.push(format!(
                "Target already exists: {} (source: {})",
                target_str, file.path
            ));
            continue;
        }

        if !tokio::fs::try_exists(&file.path).await.unwrap_or(false) {
            errors.push(format!("Source file missing: {}", file.path));
            continue;
        }

        moves.push((i, target));
    }

    if !errors.is_empty() && moves.is_empty() {
        // All files had errors, no moves possible
        return Json(MoveResult {
            moved_files: 0,
            skipped,
            errors,
        })
        .into_response();
    }

    // Phase 2: Move files on disk
    let mut moved_on_disk: Vec<(usize, std::path::PathBuf, String)> = Vec::new(); // (index, target, old_path)

    for (idx, target) in &moves {
        let file = &files_to_move[*idx];

        // Create parent directories
        if let Some(parent) = target.parent()
            && let Err(e) = tokio::fs::create_dir_all(parent).await
        {
            errors.push(format!(
                "Failed to create directory {}: {}",
                parent.display(),
                e
            ));
            continue;
        }

        // Try rename first, fall back to copy+delete for cross-device
        let move_result = tokio::fs::rename(&file.path, target).await;
        match move_result {
            Ok(()) => {
                moved_on_disk.push((*idx, target.clone(), file.path.clone()));
            }
            Err(e) if e.raw_os_error() == Some(18) => {
                // EXDEV: cross-device link, fall back to copy+delete
                match tokio::fs::copy(&file.path, target).await {
                    Ok(_) => {
                        if let Err(e) = tokio::fs::remove_file(&file.path).await {
                            warn!(path = %file.path, error = %e, "Copied but failed to remove source");
                        }
                        moved_on_disk.push((*idx, target.clone(), file.path.clone()));
                    }
                    Err(e) => {
                        errors.push(format!(
                            "Failed to copy {} to {}: {}",
                            file.path,
                            target.display(),
                            e
                        ));
                    }
                }
            }
            Err(e) => {
                errors.push(format!(
                    "Failed to move {} to {}: {}",
                    file.path,
                    target.display(),
                    e
                ));
            }
        }
    }

    // Phase 3: Update DB
    let mut moved_count = 0usize;
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            error!(error = %e, "Failed to begin transaction");
            // Files are moved on disk but DB not updated — report error
            errors.push("Database transaction failed; files moved but DB not updated".to_string());
            return Json(MoveResult {
                moved_files: 0,
                skipped,
                errors,
            })
            .into_response();
        }
    };

    for (idx, target, _old_path) in &moved_on_disk {
        let file = &files_to_move[*idx];
        let target_str = target.to_string_lossy().to_string();

        let db_result = match file.kind {
            FileKind::BookFile => book_files::update_path(&mut *tx, file.id, &target_str).await,
            FileKind::AudiobookFile => {
                audiobook_files::update_file_path(&mut *tx, file.id, &target_str).await
            }
        };

        if let Err(e) = db_result {
            error!(file_id = %file.id, error = %e, "Failed to update file path in DB");
            errors.push(format!("DB update failed for file {}: {}", file.id, e));
            continue;
        }

        // Register in media_root_files
        let file_name = target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let file_size = tokio::fs::metadata(target)
            .await
            .ok()
            .map(|m| m.len() as i64);

        if let Err(e) = media_root_files::upsert_added(
            &state.pool,
            root.id,
            &target_str,
            &file_name,
            file_size,
            "book",
            book_id,
        )
        .await
        {
            warn!(error = %e, "Failed to create media_root_files entry (non-fatal)");
        }

        moved_count += 1;
    }

    // Clean up old media_root_files entries for source paths
    for (_idx, _target, old_path) in &moved_on_disk {
        if let Err(e) = media_root_files::delete_by_path(&mut *tx, old_path).await {
            warn!(path = %old_path, error = %e, "Failed to delete old media_root_files entry (non-fatal)");
        }
    }

    // Set media_root_id on the book
    if let Err(e) = books::set_media_root_id(&mut *tx, book_id, Some(root.id)).await {
        error!(error = %e, "Failed to set media_root_id");
        errors.push("Failed to update book's media_root_id".to_string());
    }

    if let Err(e) = tx.commit().await {
        error!(error = %e, "Failed to commit transaction");
        errors.push("Transaction commit failed".to_string());
    }

    // Clean up empty parent directories from source paths (best-effort, after commit)
    for (_idx, _target, old_path) in &moved_on_disk {
        let mut dir = FsPath::new(old_path).parent();
        while let Some(parent) = dir {
            // Stop at filesystem root or if directory is not empty
            if parent == FsPath::new("/") {
                break;
            }
            match tokio::fs::remove_dir(parent).await {
                Ok(()) => {
                    info!(path = %parent.display(), "Removed empty source directory");
                    dir = parent.parent();
                }
                Err(_) => break, // Not empty or other error — stop walking up
            }
        }
    }

    info!(
        book_id = %book_id,
        media_root = %root.name,
        moved = moved_count,
        skipped = skipped,
        errors = errors.len(),
        "Move book to media root complete"
    );

    Json(MoveResult {
        moved_files: moved_count,
        skipped,
        errors,
    })
    .into_response()
}
