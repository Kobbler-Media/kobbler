//! Handler implementations for Kobo requests.

use kobo_proxy::{ProxyResponse, ResponseBody};
use serde_json::{Value, json};
use sqlx::PgPool;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::images::{self, ImageType};
use crate::kobo::{self, DeviceContext, DeviceLookupError, ReadingStateUpdate, TagItemInput};
use crate::repositories::{authors, books, series, user_book_data, user_sync_providers};
use crate::runtime_config::RuntimeConfig;
use crate::sync_providers::types::SyncAction;

use super::types::RequestContext;

/// Look up device context.
pub async fn lookup_device(pool: &PgPool, token: &str) -> Result<DeviceContext, DeviceLookupError> {
    kobo::lookup_device(pool, token).await
}

/// Handle download request.
pub async fn handle_download(
    pool: &PgPool,
    storage_root: &str,
    _device_token: &str,
    book_id: Uuid,
    format: &str,
) -> ProxyResponse {
    match kobo::prepare_download(pool, storage_root, book_id, format).await {
        Ok(info) => {
            info!(
                book_id = %book_id,
                format = %format,
                bytes = info.file_size,
                "Streaming book download via proxy"
            );

            let headers = vec![
                ("Content-Type".to_string(), info.content_type.to_string()),
                (
                    "Content-Disposition".to_string(),
                    info.content_disposition.clone(),
                ),
                ("Accept-Ranges".to_string(), "bytes".to_string()),
                ("Content-Length".to_string(), info.file_size.to_string()),
            ];

            // Stream the file instead of buffering in memory
            // Include source file path for blob download support without duplicate storage
            ProxyResponse::stream(200, info.file, info.file_size as u64, headers)
                .with_source_file_path(info.file_path)
        }
        Err(e) => {
            warn!(book_id = %book_id, format = %format, error = %e, "Download failed");
            ProxyResponse::json(404, &json!({"error": "file not found"}))
        }
    }
}

/// Handle cover image request.
pub async fn handle_cover(
    pool: &PgPool,
    storage_root: &str,
    book_id: Uuid,
    width: Option<u32>,
    height: Option<u32>,
    quality: u8,
    greyscale: bool,
) -> ProxyResponse {
    // 1. Try standard covers/ location
    let original = if let Ok(bytes) = kobo::load_cover_original(storage_root, book_id).await {
        bytes
    // 2. Try series image cache
    } else if let Ok(bytes) = tokio::fs::read(images::local_image_path(
        storage_root,
        ImageType::Series,
        book_id,
    ))
    .await
    {
        bytes
    // 3. Try extracted cover from ebook file
    } else if let Ok(bytes) = tokio::fs::read(images::local_image_path(
        storage_root,
        ImageType::BookExtracted,
        book_id,
    ))
    .await
    {
        bytes
    // 4. Lazy-cache from DB source URL
    } else if let Some(bytes) = lazy_cache_cover(pool, storage_root, book_id).await {
        bytes
    } else {
        return ProxyResponse::json(404, &json!({"error": "cover not found"}));
    };

    let variant = kobo::CoverVariant {
        width,
        height,
        greyscale,
        quality,
    };

    match kobo::load_or_build_cover_variant(storage_root, book_id, original, variant).await {
        Ok(bytes) => {
            info!(
                book_id = %book_id,
                width = ?width,
                height = ?height,
                quality = quality,
                greyscale = greyscale,
                bytes = bytes.len(),
                "Serving cover via proxy"
            );

            let headers = vec![
                ("Content-Type".to_string(), "image/jpeg".to_string()),
                ("Content-Length".to_string(), bytes.len().to_string()),
            ];

            ProxyResponse {
                status: 200,
                reason: "OK".to_string(),
                headers,
                body: ResponseBody::Buffered(bytes),
                source_file_path: None,
                on_sent: None,
            }
        }
        Err(e) => {
            warn!(book_id = %book_id, error = %e, "Failed to build cover variant");
            ProxyResponse::json(500, &json!({"error": "failed to build cover"}))
        }
    }
}

/// Try to load a cover by fetching the source URL from the DB and caching it locally.
async fn lazy_cache_cover(pool: &PgPool, storage_root: &str, book_id: Uuid) -> Option<Vec<u8>> {
    let url = books::get_cover_url(pool, book_id).await.ok().flatten()?;
    let path = images::cache_image(storage_root, ImageType::Series, book_id, &url)
        .await
        .ok()?;
    let bytes = tokio::fs::read(&path).await.ok()?;
    let _ = images::mark_cached(pool, "books", book_id, "cover").await;
    Some(bytes)
}

/// Handle book metadata request.
pub async fn handle_metadata(
    pool: &PgPool,
    ctx: &RequestContext<'_>,
    book_id: Uuid,
) -> ProxyResponse {
    match kobo::fetch_book_metadata(pool, book_id).await {
        Some(input) => {
            let metadata = kobo::build_metadata(pool, &ctx.base_url, input).await;
            ProxyResponse::json(200, &json!([metadata]))
        }
        None => ProxyResponse::json(404, &json!({"error": "book not found"})),
    }
}

/// Handle get reading state request.
pub async fn handle_get_reading_state(
    pool: &PgPool,
    ctx: &RequestContext<'_>,
    book_id: Uuid,
) -> ProxyResponse {
    let device = match lookup_device(pool, ctx.device_token).await {
        Ok(d) => d,
        Err(_) => return ProxyResponse::json(401, &json!({"error": "unauthorized"})),
    };

    // Ensure this device has a reading state (creates if missing)
    kobo::ensure_reading_state(pool, device.user_id, device.device_id, book_id).await;

    // Decide what to serve this device: its own stored bytes (byte-identical
    // echo) unless another device has meaningfully progressed.
    match kobo::resolve_state_for_device(pool, device.user_id, device.device_id, book_id).await {
        kobo::ResolvedState::RawPayload(payload) => ProxyResponse::json(200, &json!([payload])),
        kobo::ResolvedState::Synthesized(response) => ProxyResponse::json(200, &json!([*response])),
        kobo::ResolvedState::None => ProxyResponse::json(200, &json!([])),
    }
}

/// Handle update reading state request.
pub async fn handle_update_reading_state(
    pool: &PgPool,
    nats_publisher: &nats_jobs::Publisher,
    ctx: &RequestContext<'_>,
    book_id: Uuid,
) -> ProxyResponse {
    let device = match lookup_device(pool, ctx.device_token).await {
        Ok(d) => d,
        Err(_) => return ProxyResponse::json(401, &json!({"error": "unauthorized"})),
    };

    let body = ctx.body.unwrap_or(&[]);
    // Parse once as raw Value so we can store the exact sub-object Nickel sent,
    // and once as a typed struct for structured DB updates + sync jobs.
    let raw: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "Failed to parse reading state update");
            return ProxyResponse::json(400, &json!({"error": "invalid request body"}));
        }
    };
    let payload: crate::kobo::types::UpdateReadingStateRequest =
        match serde_json::from_value(raw.clone()) {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, "Failed to parse reading state update");
                return ProxyResponse::json(400, &json!({"error": "invalid request body"}));
            }
        };

    let raw_first = raw
        .get("ReadingStates")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let update = payload
        .reading_states
        .into_iter()
        .next()
        .map(ReadingStateUpdate::from);

    if let Some(update) = update {
        match kobo::update_reading_state(
            pool,
            nats_publisher,
            device.user_id,
            device.device_id,
            book_id,
            update,
            raw_first,
        )
        .await
        {
            Ok(result) => ProxyResponse::json(200, &result),
            Err(e) => {
                warn!(error = %e, "Failed to update reading state");
                ProxyResponse::json(500, &json!({"error": "failed to update"}))
            }
        }
    } else {
        ProxyResponse::json(
            200,
            &json!({
                "RequestResult": "Success",
                "UpdateResults": []
            }),
        )
    }
}

/// Handle book deletion request.
pub async fn handle_delete_book(
    pool: &PgPool,
    ctx: &RequestContext<'_>,
    book_id: Uuid,
) -> ProxyResponse {
    if lookup_device(pool, ctx.device_token).await.is_err() {
        return ProxyResponse::json(401, &json!({"error": "unauthorized"}));
    }

    match kobo::remove_book_entitlement(pool, book_id).await {
        Ok(()) => ProxyResponse {
            status: 204,
            reason: "No Content".to_string(),
            headers: Vec::new(),
            body: ResponseBody::Buffered(Vec::new()),
            source_file_path: None,
            on_sent: None,
        },
        Err(e) => {
            warn!(book_id = %book_id, error = %e, "Failed to remove entitlement");
            ProxyResponse::json(500, &json!({"error": "failed to remove"}))
        }
    }
}

/// Handle create tag request.
pub async fn handle_create_tag(pool: &PgPool, ctx: &RequestContext<'_>) -> ProxyResponse {
    let device = match lookup_device(pool, ctx.device_token).await {
        Ok(d) => d,
        Err(_) => return ProxyResponse::json(401, &json!({"error": "unauthorized"})),
    };

    let body = ctx.body.unwrap_or(&[]);
    let payload: crate::kobo::types::TagRequest = match serde_json::from_slice(body) {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "Failed to parse create tag request");
            return ProxyResponse::json(400, &json!({"error": "invalid request body"}));
        }
    };

    let items = payload.items.map(|items| {
        items
            .into_iter()
            .filter(|i| i.item_type == "ProductRevisionTagItem")
            .filter_map(|i| Uuid::parse_str(&i.revision_id).ok())
            .map(|id| TagItemInput { revision_id: id })
            .collect()
    });

    match kobo::create_tag(pool, device.user_id, &payload.name, items).await {
        Ok(shelf_id) => ProxyResponse::json(200, &json!(shelf_id.to_string())),
        Err(e) => {
            warn!(error = %e, "Failed to create tag");
            ProxyResponse::json(400, &json!({"error": "failed to create tag"}))
        }
    }
}

/// Handle rename tag request.
pub async fn handle_rename_tag(
    pool: &PgPool,
    ctx: &RequestContext<'_>,
    tag_id: Uuid,
) -> ProxyResponse {
    let device = match lookup_device(pool, ctx.device_token).await {
        Ok(d) => d,
        Err(_) => return ProxyResponse::json(401, &json!({"error": "unauthorized"})),
    };

    let body = ctx.body.unwrap_or(&[]);
    let payload: crate::kobo::types::TagRequest = match serde_json::from_slice(body) {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "Failed to parse rename tag request");
            return ProxyResponse::json(400, &json!({"error": "invalid request body"}));
        }
    };

    match kobo::rename_tag(pool, device.user_id, tag_id, &payload.name).await {
        Ok(()) => ProxyResponse {
            status: 204,
            reason: "No Content".to_string(),
            headers: Vec::new(),
            body: ResponseBody::Buffered(Vec::new()),
            source_file_path: None,
            on_sent: None,
        },
        Err(e) => {
            warn!(error = %e, "Failed to rename tag");
            ProxyResponse::json(400, &json!({"error": "failed to rename tag"}))
        }
    }
}

/// Handle delete tag request.
pub async fn handle_delete_tag(
    pool: &PgPool,
    ctx: &RequestContext<'_>,
    tag_id: Uuid,
) -> ProxyResponse {
    let device = match lookup_device(pool, ctx.device_token).await {
        Ok(d) => d,
        Err(_) => return ProxyResponse::json(401, &json!({"error": "unauthorized"})),
    };

    match kobo::delete_tag(pool, device.user_id, tag_id).await {
        Ok(()) => ProxyResponse {
            status: 204,
            reason: "No Content".to_string(),
            headers: Vec::new(),
            body: ResponseBody::Buffered(Vec::new()),
            source_file_path: None,
            on_sent: None,
        },
        Err(e) => {
            warn!(error = %e, "Failed to delete tag");
            ProxyResponse::json(400, &json!({"error": "failed to delete tag"}))
        }
    }
}

/// Handle add tag items request.
pub async fn handle_add_tag_items(
    pool: &PgPool,
    ctx: &RequestContext<'_>,
    tag_id: Uuid,
) -> ProxyResponse {
    if lookup_device(pool, ctx.device_token).await.is_err() {
        return ProxyResponse::json(401, &json!({"error": "unauthorized"}));
    }

    let body = ctx.body.unwrap_or(&[]);
    let payload: crate::kobo::types::TagRequest = match serde_json::from_slice(body) {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "Failed to parse add tag items request");
            return ProxyResponse::json(400, &json!({"error": "invalid request body"}));
        }
    };

    let items: Vec<TagItemInput> = payload
        .items
        .unwrap_or_default()
        .into_iter()
        .filter(|i| i.item_type == "ProductRevisionTagItem")
        .filter_map(|i| Uuid::parse_str(&i.revision_id).ok())
        .map(|id| TagItemInput { revision_id: id })
        .collect();

    match kobo::add_tag_items(pool, tag_id, items).await {
        Ok(()) => ProxyResponse {
            status: 204,
            reason: "No Content".to_string(),
            headers: Vec::new(),
            body: ResponseBody::Buffered(Vec::new()),
            source_file_path: None,
            on_sent: None,
        },
        Err(e) => {
            warn!(error = %e, "Failed to add tag items");
            ProxyResponse::json(400, &json!({"error": "failed to add items"}))
        }
    }
}

/// Handle remove tag items request.
pub async fn handle_remove_tag_items(
    pool: &PgPool,
    ctx: &RequestContext<'_>,
    tag_id: Uuid,
) -> ProxyResponse {
    if lookup_device(pool, ctx.device_token).await.is_err() {
        return ProxyResponse::json(401, &json!({"error": "unauthorized"}));
    }

    let body = ctx.body.unwrap_or(&[]);
    let payload: crate::kobo::types::TagRequest = match serde_json::from_slice(body) {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "Failed to parse remove tag items request");
            return ProxyResponse::json(400, &json!({"error": "invalid request body"}));
        }
    };

    let items: Vec<TagItemInput> = payload
        .items
        .unwrap_or_default()
        .into_iter()
        .filter(|i| i.item_type == "ProductRevisionTagItem")
        .filter_map(|i| Uuid::parse_str(&i.revision_id).ok())
        .map(|id| TagItemInput { revision_id: id })
        .collect();

    match kobo::remove_tag_items(pool, tag_id, items).await {
        Ok(()) => ProxyResponse {
            status: 204,
            reason: "No Content".to_string(),
            headers: Vec::new(),
            body: ResponseBody::Buffered(Vec::new()),
            source_file_path: None,
            on_sent: None,
        },
        Err(e) => {
            warn!(error = %e, "Failed to remove tag items");
            ProxyResponse::json(400, &json!({"error": "failed to remove items"}))
        }
    }
}

/// Handle series books request.
pub async fn handle_series_books(
    pool: &PgPool,
    ctx: &RequestContext<'_>,
    series_id: Uuid,
) -> ProxyResponse {
    // Query books in this series with their metadata using repository
    let rows = match series::list_books(pool, series_id).await {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "Failed to fetch series books");
            return ProxyResponse::json(500, &json!({"error": "internal error"}));
        }
    };

    let mut items = Vec::new();
    for row in rows {
        let input = kobo::metadata::MetadataInput {
            book_id: row.id,
            title: row.title,
            description: row.description,
            language: row.language,
            publisher: row.publisher,
            pubdate: row.pubdate,
            series_id: row.series_id,
            series_name: row.series_name,
            series_index: row.series_index,
            identifiers: row.identifiers,
            page_count: None,
            rating: None,
        };

        let metadata = kobo::build_metadata(pool, &ctx.base_url, input).await;
        items.push(metadata);
    }

    let item_count = items.len();
    ProxyResponse::json(
        200,
        &json!({
            "Items": items,
            "ItemCount": item_count,
            "TotalPageCount": 1,
            "TotalItemCount": item_count,
            "CurrentPageIndex": 0,
            "ItemsPerPage": 100,
            "Filters": {
                "KoboLoveEnabled": ["True", "False"],
                "SubscriptionsAvailable": ["True", "False"]
            },
            "VersionCode": 2
        }),
    )
}

/// Handle nextread request - returns book recommendations.
pub async fn handle_nextread(
    pool: &PgPool,
    ctx: &RequestContext<'_>,
    book_id: Uuid,
) -> ProxyResponse {
    let device = match lookup_device(pool, ctx.device_token).await {
        Ok(d) => d,
        Err(_) => {
            // If we can't identify the device, return empty object (matches Kobo API)
            return ProxyResponse::json(200, &json!({}));
        }
    };

    // Get recommendations (next in series, then same author)
    let recommendations =
        kobo::get_recommendations(pool, book_id, device.user_id, device.device_id, 10).await;

    // If no recommendations, return empty object (matches Kobo API)
    if recommendations.is_empty() {
        info!(
            book_id = %book_id,
            "No nextread recommendations found"
        );
        return ProxyResponse::json(200, &json!({}));
    }

    // Build product JSON for each recommendation
    let products: Vec<Value> = recommendations
        .iter()
        .map(kobo::build_product_recommendation)
        .collect();

    info!(
        book_id = %book_id,
        count = products.len(),
        "Returning nextread recommendations"
    );

    // Response format: { "book-id": [products] }
    let mut resp = serde_json::Map::new();
    resp.insert(book_id.to_string(), json!(products));
    ProxyResponse::json(200, &Value::Object(resp))
}

/// Handle product recommendations - returns paginated recommendations.
pub async fn handle_recommendations(
    pool: &PgPool,
    ctx: &RequestContext<'_>,
    book_id: Uuid,
) -> ProxyResponse {
    let device = match lookup_device(pool, ctx.device_token).await {
        Ok(d) => d,
        Err(_) => {
            return ProxyResponse::json(
                200,
                &json!({
                    "Items": [],
                    "ItemCount": 0,
                    "TotalPageCount": 0,
                    "TotalItemCount": 0,
                    "CurrentPageIndex": 0,
                    "ItemsPerPage": 10
                }),
            );
        }
    };

    // Get recommendations (next in series, then same author)
    let recommendations =
        kobo::get_recommendations(pool, book_id, device.user_id, device.device_id, 10).await;

    let items: Vec<Value> = recommendations
        .iter()
        .map(|r| json!({"Book": kobo::build_product_recommendation(r)}))
        .collect();

    let count = items.len();

    info!(
        book_id = %book_id,
        count = count,
        "Returning product recommendations"
    );

    ProxyResponse::json(
        200,
        &json!({
            "Items": items,
            "ItemCount": count,
            "TotalPageCount": if count > 0 { 1 } else { 0 },
            "TotalItemCount": count,
            "CurrentPageIndex": 0,
            "ItemsPerPage": 10,
            "VersionCode": 2
        }),
    )
}

/// Handle product book details request.
pub async fn handle_product_book(
    pool: &PgPool,
    _ctx: &RequestContext<'_>,
    book_id: Uuid,
) -> ProxyResponse {
    // Build a RecommendationCandidate from the book metadata so we can reuse build_product_recommendation
    let input = match kobo::fetch_book_metadata(pool, book_id).await {
        Some(input) => input,
        None => return ProxyResponse::json(404, &json!({"error": "book not found"})),
    };

    // Fetch authors for the product JSON
    let author_names = authors::get_book_author_names(pool, book_id)
        .await
        .unwrap_or_default();

    let candidate = kobo::recommendations::RecommendationCandidate {
        book_id: input.book_id,
        title: input.title,
        description: input.description,
        language: input.language,
        publisher: input.publisher,
        pubdate: input.pubdate,
        series_id: input.series_id,
        series_name: input.series_name,
        series_index: input.series_index,
        identifiers: input.identifiers,
        authors: author_names,
        reason: kobo::recommendations::RecommendationReason::SameAuthor, // doesn't matter for product
        progress_percent: None,
    };

    let product = kobo::build_product_recommendation(&candidate);

    info!(book_id = %book_id, "Returning product book details");
    ProxyResponse::json(200, &product)
}

/// Handle rating request from device.
pub async fn handle_rating(
    pool: &PgPool,
    nats_publisher: &nats_jobs::Publisher,
    ctx: &RequestContext<'_>,
    book_id: Uuid,
    rating: i32,
) -> ProxyResponse {
    let device = match lookup_device(pool, ctx.device_token).await {
        Ok(d) => d,
        Err(_) => return ProxyResponse::json(401, &json!({"error": "unauthorized"})),
    };

    // Save the rating
    if let Err(e) =
        user_book_data::update_rating(pool, device.user_id, book_id, Some(rating), "kobo").await
    {
        warn!(error = %e, book_id = %book_id, "Failed to save rating");
        return ProxyResponse::json(500, &json!({"error": "failed to save rating"}));
    }

    info!(book_id = %book_id, rating = rating, "Saved book rating from Kobo device");

    // Queue sync to all enabled external providers
    let providers = user_sync_providers::list_enabled_for_device(pool, device.device_id)
        .await
        .unwrap_or_default();
    for (provider_id, _api_key) in &providers {
        if let Err(e) = crate::jobs::handlers::queue_sync_job_for_user(
            pool,
            nats_publisher,
            Some(device.user_id),
            device.device_id,
            book_id,
            provider_id,
            SyncAction::Rating { stars: rating },
        )
        .await
        {
            warn!(error = %e, book_id = %book_id, provider = %provider_id, "Failed to queue rating sync");
        }
    }

    ProxyResponse::json(200, &json!({}))
}

/// Handle recommendations feedback - extract ratings for local books.
///
/// The Kobo device sends POST /v1/user/recommendations/feedback with a body like:
/// `{"FeedbackItems": [{"FeedbackType": "Rate4Star", "RevisionId": "<uuid>"}]}`
///
/// We intercept this entirely to prevent local book UUIDs from leaking to the Kobo store.
/// For any RevisionId that matches a local book, we save the rating.
pub async fn handle_recommendations_feedback(
    pool: &PgPool,
    nats_publisher: &nats_jobs::Publisher,
    ctx: &RequestContext<'_>,
) -> ProxyResponse {
    let device = match lookup_device(pool, ctx.device_token).await {
        Ok(d) => d,
        Err(_) => return ProxyResponse::json(401, &json!({"error": "unauthorized"})),
    };

    let body = ctx.body.unwrap_or(&[]);
    let payload: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "Failed to parse recommendations feedback body");
            return ProxyResponse::json(400, &json!({"error": "invalid request body"}));
        }
    };

    let items = match payload.get("FeedbackItems").and_then(|v| v.as_array()) {
        Some(items) => items,
        None => {
            return ProxyResponse {
                status: 204,
                reason: "No Content".to_string(),
                headers: Vec::new(),
                body: ResponseBody::Buffered(Vec::new()),
                source_file_path: None,
                on_sent: None,
            };
        }
    };

    for item in items {
        let feedback_type = item
            .get("FeedbackType")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let revision_id = item
            .get("RevisionId")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let Some(rating) = parse_feedback_rating(feedback_type) else {
            info!(feedback_type = %feedback_type, "Ignoring non-rating feedback type");
            continue;
        };

        let Ok(book_id) = Uuid::parse_str(revision_id) else {
            continue;
        };

        // Check if this is a local book
        let is_local = crate::repositories::books::get_summary(pool, book_id)
            .await
            .ok()
            .flatten()
            .is_some();

        if !is_local {
            info!(revision_id = %revision_id, "Skipping feedback for non-local book");
            continue;
        }

        // Save the rating (same as handle_rating)
        if let Err(e) =
            user_book_data::update_rating(pool, device.user_id, book_id, Some(rating), "kobo").await
        {
            warn!(error = %e, book_id = %book_id, "Failed to save rating from feedback");
            continue;
        }

        info!(book_id = %book_id, rating = rating, "Saved book rating from Kobo recommendations feedback");

        // Queue sync to all enabled external providers
        let providers = user_sync_providers::list_enabled_for_device(pool, device.device_id)
            .await
            .unwrap_or_default();
        for (provider_id, _api_key) in &providers {
            if let Err(e) = crate::jobs::handlers::queue_sync_job_for_user(
                pool,
                nats_publisher,
                Some(device.user_id),
                device.device_id,
                book_id,
                provider_id,
                SyncAction::Rating { stars: rating },
            )
            .await
            {
                warn!(error = %e, book_id = %book_id, provider = %provider_id, "Failed to queue rating sync from feedback");
            }
        }
    }

    ProxyResponse {
        status: 204,
        reason: "No Content".to_string(),
        headers: Vec::new(),
        body: ResponseBody::Buffered(Vec::new()),
        source_file_path: None,
        on_sent: None,
    }
}

/// Parse a Kobo FeedbackType string like "Rate4Star" into a 1-5 rating.
fn parse_feedback_rating(feedback_type: &str) -> Option<i32> {
    match feedback_type {
        "Rate1Star" => Some(1),
        "Rate2Star" => Some(2),
        "Rate3Star" => Some(3),
        "Rate4Star" => Some(4),
        "Rate5Star" => Some(5),
        _ => None,
    }
}

/// Handle library sync (local mode - our data only).
#[allow(dead_code)]
pub async fn handle_sync(
    pool: &PgPool,
    ctx: &RequestContext<'_>,
    runtime_config: &RuntimeConfig,
) -> ProxyResponse {
    let device = match lookup_device(pool, ctx.device_token).await {
        Ok(d) => d,
        Err(_) => return ProxyResponse::json(401, &json!({"error": "unauthorized"})),
    };

    // Decode sync token from request headers (device round-trips it)
    let incoming_token = ctx.sync_token_header.unwrap_or("");
    let sync_token = kobo::SyncToken::decode(incoming_token);
    let last_sync_at = sync_token.last_sync_at();

    let sync_response = kobo::build_sync_response(
        pool,
        &ctx.base_url,
        device,
        runtime_config.kobo_sync_batch_size,
        last_sync_at,
    )
    .await;

    // Return 500 if there was a database error
    if let Some(error) = sync_response.error {
        return ProxyResponse::json(500, &json!({"error": error}));
    }

    // Build new token with updated last_sync timestamp
    let new_token = if let Some(max_ts) = sync_response.max_processed_timestamp {
        sync_token.with_last_sync(max_ts)
    } else {
        sync_token
    };

    let body = serde_json::to_vec(&sync_response.items).unwrap_or_default();
    let mut headers = vec![
        (
            "Content-Type".to_string(),
            "application/json; charset=utf-8".to_string(),
        ),
        ("Content-Length".to_string(), body.len().to_string()),
        ("x-kobo-synctoken".to_string(), new_token.encode()),
    ];
    if sync_response.continue_sync {
        headers.push(("x-kobo-sync".to_string(), "continue".to_string()));
    }

    // Defer DB sync state update until response is delivered (best-effort for UI)
    let on_sent = build_sync_on_sent(pool.clone(), &sync_response);

    ProxyResponse {
        status: 200,
        reason: "OK".to_string(),
        headers,
        body: ResponseBody::Buffered(body),
        source_file_path: None,
        on_sent,
    }
}

/// Build an `on_sent` callback that advances sync state after delivery.
pub fn build_sync_on_sent(
    pool: PgPool,
    sync_response: &kobo::SyncResponse,
) -> Option<kobo_proxy::OnSentCallback> {
    let timestamp = sync_response.max_processed_timestamp?;
    let device_id = sync_response.device_id;
    Some(Box::new(move || {
        Box::pin(async move {
            kobo::commit_sync_state(&pool, device_id, timestamp).await;
        })
    }))
}

/// Handle initialization (for modify - our URLs injected).
pub fn handle_initialization(ctx: &RequestContext<'_>) -> ProxyResponse {
    let resources = kobo::build_initialization_resources(&ctx.base_url);

    let headers = vec![("x-kobo-apitoken".to_string(), "e30=".to_string())];

    ProxyResponse::json_with_headers(200, &json!({"Resources": resources}), headers)
}

/// Get stub response for passthrough endpoints.
pub fn get_stub(path: &str, headers: &[(String, String)]) -> ProxyResponse {
    let body = if path.starts_with("/v1/user/loyalty/benefits") {
        json!({"Benefits": {}})
    } else if path.starts_with("/v1/analytics/gettests") {
        let test_key = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("x-kobo-userkey"))
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        json!({
            "Result": "Success",
            "TestKey": test_key,
            "Tests": {}
        })
    } else {
        json!({})
    };

    ProxyResponse::json(200, &body)
}
