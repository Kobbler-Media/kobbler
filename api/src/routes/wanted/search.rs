//! Search and queue operations for books via Kobload.
//!
//! These handlers work for any book (wanted or in-library) since they
//! only need book metadata (title, identifiers) to search.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use uuid::Uuid;

use crate::AppState;
use crate::kobload::CreateDownloadRequest as KobloadDownloadRequest;
use crate::repositories::{books, wanted_attempts};
use crate::routes::AuthUser;
use crate::routes::helpers::build_kobload_client;

use super::{QueueWantedRequest, SearchReleasesResponse, WantedReleasesQuery};

/// GET /api/v1/admin/books/{book_id}/releases
pub async fn search_releases(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Query(query): Query<WantedReleasesQuery>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let metadata = match books::get_wanted_metadata(&state.pool, id).await {
        Ok(Some(m)) => m,
        Ok(None) => return (StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch book");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let title = metadata.title;
    let author = metadata.author;

    let override_query = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let isbn10 = metadata
        .identifiers
        .get("isbn10")
        .and_then(|v| v.as_str())
        .map(String::from);
    let isbn13 = metadata
        .identifiers
        .get("isbn13")
        .and_then(|v| v.as_str())
        .map(String::from);
    let identifiers = nats_jobs::BookIdentifiers {
        isbn10: isbn10.clone(),
        isbn13: isbn13.clone(),
        hardcover_id: None,
        goodreads_id: None,
        openlibrary_id: None,
    };
    let isbn = isbn13.clone().or(isbn10.clone());
    let alternate_titles = crate::wanted::book_titles::book_alternate_titles(
        &crate::metadata::ProviderRegistry::new(state.pool.clone()),
        &crate::wanted::book_titles::BookRef {
            title: &title,
            series: metadata.series_name.as_deref(),
            position: metadata.series_index,
            hardcover_id: metadata
                .identifiers
                .get("hardcover")
                .and_then(|v| v.as_str()),
        },
        crate::wanted::book_titles::Lookup::Fetch,
    )
    .await;
    let media_want = if metadata.want_format == "audiobook" {
        nats_jobs::MediaWant::Audiobook {
            title: title.clone(),
            author: author.clone(),
            narrator: metadata.narrator,
            isbn: isbn.clone(),
            format_preferences: crate::routes::ingest::AUDIOBOOK_EXTENSIONS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            identifiers: Some(identifiers),
            series: metadata.series_name,
            series_position: metadata.series_index.map(|idx| idx.to_string()),
            release_date: metadata.pubdate.map(|dt| dt.date().to_string()),
            alternate_titles: alternate_titles.clone(),
        }
    } else {
        nats_jobs::MediaWant::Book {
            title: title.clone(),
            author: author.clone(),
            isbn: isbn.clone(),
            format_preferences: crate::routes::ingest::EBOOK_EXTENSIONS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            identifiers: Some(identifiers),
            series: metadata.series_name,
            series_position: metadata.series_index.map(|idx| idx.to_string()),
            release_date: metadata.pubdate.map(|dt| dt.date().to_string()),
            alternate_titles: alternate_titles.clone(),
        }
    };

    // "any" drops the category filter entirely, for hunting releases that were
    // filed under Other/Misc rather than Books.
    let any_scope = query.scope.as_deref() == Some("any");

    // A free-text query overrides the structured terms, but must NOT drop the
    // media type with them: Kobload derives its category filter from one or the
    // other, and with neither it falls back to an uncategorised search across
    // every indexer.
    let media_type_key = if metadata.want_format == "audiobook" {
        "audiobooks"
    } else {
        "ebooks"
    };

    let (primary_query, media_want, media_type) = if let Some(override_query) = override_query {
        // Free text: no media_want, so the media type is what carries the
        // category filter — unless the caller deliberately dropped it.
        let media_type = (!any_scope).then(|| media_type_key.to_string());
        (override_query, None, media_type)
    } else {
        // Structured search derives its own categories from media_want.
        (String::new(), Some(media_want), None)
    };

    let runtime = state.runtime_config.get().await;
    let client = match build_kobload_client(&runtime) {
        Ok(client) => client,
        Err((status, message)) => return (status, message).into_response(),
    };

    let results = client
        .search_scoped(&primary_query, None, media_want.clone(), media_type)
        .await;
    match results {
        Ok(mut results) => {
            // Annotate with rejected release info
            let rejected = books::get_rejected_releases(&state.pool, id)
                .await
                .unwrap_or(serde_json::json!([]));
            crate::routes::library::search_download::annotate_rejected_releases(
                &mut results,
                &rejected,
            );
            Json(SearchReleasesResponse { results }).into_response()
        }
        Err(err) => {
            tracing::error!(error = ?err, "Failed to search Kobload");
            let (status, message) = match &err {
                crate::kobload::KobloadError::Status(s, body) => {
                    let status =
                        StatusCode::from_u16(s.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
                    let msg = serde_json::from_str::<serde_json::Value>(body)
                        .ok()
                        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(String::from))
                        .unwrap_or_else(|| format!("{err}"));
                    (status, msg)
                }
                crate::kobload::KobloadError::Request(_) => {
                    (StatusCode::BAD_GATEWAY, format!("{err}"))
                }
            };
            (status, Json(serde_json::json!({ "message": message }))).into_response()
        }
    }
}

/// POST /api/v1/admin/books/{book_id}/queue
pub async fn queue_book_release(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<QueueWantedRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_contributor() {
        return e.into_response();
    }
    let metadata = match books::get_wanted_metadata(&state.pool, id).await {
        Ok(Some(m)) => m,
        Ok(None) => return (StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch book");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    let title = metadata.title;
    let author = metadata.author;
    let identifiers = metadata.identifiers;
    let series = metadata.series_name;
    let series_index = metadata.series_index;

    let release = payload.release.clone();
    let protocol = release
        .get("protocol")
        .and_then(|v| v.as_str())
        .map(|v| v.to_lowercase())
        .ok_or((StatusCode::BAD_REQUEST, "release protocol missing"));

    let protocol = match protocol {
        Ok(value) => value,
        Err((status, message)) => return (status, message).into_response(),
    };

    let download_url = release
        .get("download_url")
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
        .ok_or((StatusCode::BAD_REQUEST, "release download_url missing"));

    let download_url = match download_url {
        Ok(value) => value,
        Err((status, message)) => return (status, message).into_response(),
    };

    let runtime = state.runtime_config.get().await;

    let client = match build_kobload_client(&runtime) {
        Ok(client) => client,
        Err((status, message)) => return (status, message).into_response(),
    };

    let callback_url = match runtime.kobload_ingest_url.as_deref() {
        Some(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "Kobload Ingest URL not configured. Go to Settings > System > Kobload and set the Ingest URL.",
            )
                .into_response();
        }
    };

    let provider = identifiers
        .get("hardcover")
        .map(|_| "hardcover")
        .unwrap_or("manual");

    let source = if payload.source.trim().is_empty() {
        "manual".to_string()
    } else {
        payload.source.clone()
    };

    let metadata = serde_json::json!({
        "book_id": id,
        "provider": provider,
        "title": title,
        "author": author,
        "series": series,
        "series_index": series_index,
        "release_title": release.get("title").cloned().unwrap_or(serde_json::Value::Null),
        "release_file_name": release.get("file_name").cloned().unwrap_or(serde_json::Value::Null),
    });

    let name = release
        .get("file_name")
        .and_then(|v| v.as_str())
        .or_else(|| release.get("title").and_then(|v| v.as_str()))
        .unwrap_or("download")
        .to_string();

    let request = KobloadDownloadRequest {
        source,
        protocol: protocol.clone(),
        name,
        download_url: download_url.clone(),
        callback_url,
        auth_token: None,
        metadata,
        release: Some(release.clone()),
    };

    let (status, message, download_id) = match client.create_download(&request).await {
        Ok(response) => (response.status, None, Some(response.id)),
        Err(err) => (
            "failed".to_string(),
            Some(format!("Kobload error: {err}")),
            None,
        ),
    };

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "Failed to begin transaction");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    if let Err(e) = wanted_attempts::create(
        &mut *tx,
        wanted_attempts::WantedAttemptInput {
            book_id: Some(id),
            movie_id: None,
            source: payload.source.clone(),
            external_id: download_id.map(|id| id.to_string()),
            download_url: Some(download_url.clone()),
            status: status.clone(),
            message: message.clone(),
            user_id: Some(auth.user_id),
            ..Default::default()
        },
    )
    .await
    {
        tracing::error!(error = %e, "Failed to insert wanted attempt");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    // Update book's updated_at timestamp
    if let Err(e) = books::touch(&mut *tx, id).await {
        tracing::error!(error = %e, "Failed to update book timestamp");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, "Failed to commit transaction");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    if status == "failed" {
        let error_msg = message.unwrap_or_else(|| "kobload download failed".to_string());
        return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "message": error_msg })),
        )
            .into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}
