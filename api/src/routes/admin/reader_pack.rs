//! Reader pack routes: the app's pre-digested book (see `crate::reader_pack`).

use std::io::Read;
use std::path::PathBuf;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use crate::AppState;
use crate::reader_pack::{self, LoadedPack, ReaderFile};
use crate::routes::AuthUser;

async fn open_pack(
    state: &AppState,
    book_id: &str,
) -> Result<(ReaderFile, std::sync::Arc<LoadedPack>), Response> {
    let book_id = Uuid::parse_str(book_id)
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid book id").into_response())?;
    let file = reader_pack::file::select(state, book_id, reader_pack::file::PACK_FORMATS)
        .await
        .ok_or_else(|| (StatusCode::NOT_FOUND, "no readable file found").into_response())?;
    let pack = reader_pack::ensure(&state.config.storage_root, &file)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, book_id = %book_id, "reader pack build failed");
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                "book could not be prepared for reading",
            )
                .into_response()
        })?;
    Ok((file, pack))
}

pub async fn get_reader_pack_manifest(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(book_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    match open_pack(&state, &book_id).await {
        Ok((_, pack)) => serve_gz(&headers, &pack.etag, pack.manifest_path()).await,
        Err(r) => r,
    }
}

pub async fn get_reader_pack_chapter(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path((book_id, chapter)): Path<(String, usize)>,
    headers: HeaderMap,
) -> Response {
    match open_pack(&state, &book_id).await {
        Ok((_, pack)) if chapter < pack.hrefs.len() => {
            serve_gz(&headers, &pack.etag, pack.chapter_path(chapter)).await
        }
        Ok(_) => (StatusCode::NOT_FOUND, "no such chapter").into_response(),
        Err(r) => r,
    }
}

/// Images and fonts, read straight out of the book's archive.
pub async fn get_reader_pack_asset(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path((book_id, asset)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let (file, pack) = match open_pack(&state, &book_id).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    if not_modified(&headers, &pack.etag) {
        return not_modified_response(&pack.etag);
    }
    let source = file.path.clone();
    let name = asset.clone();
    let bytes = tokio::task::spawn_blocking(move || {
        kob_ebook::reader_pack::read_asset(&source, &name).ok()
    })
    .await;
    let Ok(Some(bytes)) = bytes else {
        return (StatusCode::NOT_FOUND, "no such asset").into_response();
    };
    Response::builder()
        .header(header::CONTENT_TYPE, content_type(&asset))
        .header(header::ETAG, &pack.etag)
        .header(header::CACHE_CONTROL, "private, max-age=86400")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn content_type(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or_default();
    match ext {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "otf" => "font/otf",
        "ttf" => "font/ttf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn not_modified(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag))
}

fn not_modified_response(etag: &str) -> Response {
    Response::builder()
        .status(StatusCode::NOT_MODIFIED)
        .header(header::ETAG, etag)
        .body(Body::empty())
        .unwrap_or_else(|_| StatusCode::NOT_MODIFIED.into_response())
}

/// Packs are stored gzipped; clients that don't accept gzip get it inflated.
async fn serve_gz(headers: &HeaderMap, etag: &str, path: PathBuf) -> Response {
    if not_modified(headers, etag) {
        return not_modified_response(etag);
    }
    let accepts_gzip = headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|e| e.trim().starts_with("gzip")));
    let body = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
        let raw = std::fs::read(path)?;
        if accepts_gzip {
            return Ok(raw);
        }
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(raw.as_slice()).read_to_end(&mut out)?;
        Ok(out)
    })
    .await;
    let Ok(Ok(body)) = body else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to read reader pack",
        )
            .into_response();
    };
    let mut response = Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, "private, no-cache")
        .header(header::VARY, "Accept-Encoding");
    if accepts_gzip {
        response = response.header(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }
    response
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
