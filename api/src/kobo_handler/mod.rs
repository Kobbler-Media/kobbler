//! Kobbler implementation of the ProxyHandler trait.

pub mod entity_scan;
mod extractors;
mod handlers;
mod logger;
mod types;
mod url_rewrite;

use std::sync::Arc;

use kobo_proxy::{
    ClassifyResult, ProxyDecision, ProxyHandler, ProxyRequest, ProxyResponse, ResponseBody,
    UpstreamConfig, async_trait,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tracing::{debug, error, info};
use uuid::Uuid;

use crate::config::{Config, KoboMode};
use crate::kobo::{self, DeviceLookupError};
use crate::repositories::{books, devices};
use crate::runtime_config::{RuntimeConfig, RuntimeConfigManager};

pub use logger::DatabaseLogger;
pub use types::{BookSummary, RequestContext};

/// Kobbler's implementation of ProxyHandler.
pub struct KobblerHandler {
    pool: PgPool,
    config: Arc<Config>,
    runtime_config: RuntimeConfigManager,
    nats_publisher: nats_jobs::Publisher,
}

impl KobblerHandler {
    pub fn new(
        pool: PgPool,
        config: Arc<Config>,
        runtime_config: RuntimeConfigManager,
        nats_publisher: nats_jobs::Publisher,
    ) -> Self {
        Self {
            pool,
            config,
            runtime_config,
            nats_publisher,
        }
    }

    /// Get a snapshot of runtime config.
    async fn runtime_config(&self) -> RuntimeConfig {
        self.runtime_config.get().await
    }

    /// Build request context from a proxy request.
    ///
    /// This extracts and computes commonly-needed values once at the entry point,
    /// so they can be passed through to handler functions cleanly.
    fn context<'a>(&self, req: &'a ProxyRequest, rt_config: &RuntimeConfig) -> RequestContext<'a> {
        let device_token = req.device_token.as_deref().unwrap_or("");
        let base_url = self.build_base_url(device_token, rt_config);
        let body = req.body.as_deref();
        let sync_token_header = req.get_header("x-kobo-synctoken");

        RequestContext {
            device_token,
            base_url,
            body,
            sync_token_header,
        }
    }

    /// Build the base URL for a device's API requests.
    ///
    /// Uses the configured public_proxy_url which should include the full
    /// public path (e.g., "https://example.com/kobo").
    fn build_base_url(&self, device_token: &str, rt_config: &RuntimeConfig) -> String {
        let proxy_url = self.config.public_proxy_url(&rt_config.public_proxy_url);
        format!("{}/{}", proxy_url, device_token)
    }

    /// Look up device context.
    async fn lookup_device(&self, token: &str) -> Result<kobo::DeviceContext, DeviceLookupError> {
        kobo::lookup_device(&self.pool, token).await
    }

    /// Get book summary if it's our book.
    async fn get_book_summary(&self, book_id: &str) -> Option<BookSummary> {
        let id = Uuid::parse_str(book_id).ok()?;
        let summary = books::get_summary(&self.pool, id).await.ok()??;
        Some(BookSummary {
            title: summary.title,
            authors: summary.authors,
        })
    }

    /// Check if a tag ID belongs to us.
    async fn is_our_tag(&self, tag_id: &str) -> bool {
        let Ok(id) = Uuid::parse_str(tag_id) else {
            return false;
        };
        kobo::tag_exists(&self.pool, id).await
    }

    /// Check if a series ID belongs to us.
    async fn is_our_series(&self, series_id: &str) -> bool {
        let Ok(id) = Uuid::parse_str(series_id) else {
            return false;
        };
        kobo::series_exists(&self.pool, id).await
    }
}

#[async_trait]
impl ProxyHandler for KobblerHandler {
    async fn classify(&self, req: &ProxyRequest) -> ClassifyResult {
        let path = &req.path;
        let method = req.method.as_str();
        let rt_config = self.runtime_config().await;
        let mode = rt_config.kobo_mode;

        debug!(path = %path, method = %method, mode = ?mode, "Classifying request");

        // In passthrough mode, Kobbler is invisible - forward everything unchanged
        if mode == KoboMode::Passthrough {
            return ProxyDecision::Passthrough.into();
        }

        // Library sync
        if path == "/v1/library/sync" && method == "GET" {
            // In merge mode: modify to merge our data with Kobo's
            // In local mode: intercept to return only our data (handled by stub)
            return if mode == KoboMode::Merge {
                ProxyDecision::Modify.into()
            } else {
                ProxyDecision::Intercept.into()
            };
        }

        // Initialization
        if path == "/v1/initialization" && method == "GET" {
            // In merge mode: modify to inject our URLs
            // In local mode: intercept to return only our URLs (handled by stub)
            return if mode == KoboMode::Merge {
                ProxyDecision::Modify.into()
            } else {
                ProxyDecision::Intercept.into()
            };
        }

        // Download - intercept if it's our book
        if path.starts_with("/download/") {
            if let Some(book_id) = extractors::extract_book_id(path)
                && let Some(summary) = self.get_book_summary(book_id).await
            {
                return ClassifyResult::with_notes(ProxyDecision::Intercept, summary.to_note());
            }
            return ProxyDecision::Passthrough.into();
        }

        // Cover images - intercept if it's our book, passthrough otherwise (to cdn.kobo.com)
        if path.starts_with("/book-images/") && path.ends_with("/image.jpg") {
            if let Some(book_id) = extractors::extract_book_id(path)
                && let Some(summary) = self.get_book_summary(book_id).await
            {
                return ClassifyResult::with_notes(ProxyDecision::Intercept, summary.to_note());
            }
            return ProxyDecision::Passthrough.into();
        }

        // Book metadata - intercept if it's our book, modify for URL rewriting if enabled
        if path.starts_with("/v1/library/") && path.ends_with("/metadata") {
            if let Some(book_id) = extractors::extract_book_id(path)
                && let Some(summary) = self.get_book_summary(book_id).await
            {
                return ClassifyResult::with_notes(ProxyDecision::Intercept, summary.to_note());
            }
            return if rt_config.rewrite_kobo_urls {
                ProxyDecision::Modify.into()
            } else {
                ProxyDecision::Passthrough.into()
            };
        }

        // Reading state - intercept if it's our book, modify for URL rewriting if enabled
        if path.starts_with("/v1/library/") && path.ends_with("/state") {
            if let Some(book_id) = extractors::extract_book_id(path)
                && let Some(summary) = self.get_book_summary(book_id).await
            {
                return ClassifyResult::with_notes(ProxyDecision::Intercept, summary.to_note());
            }
            return if rt_config.rewrite_kobo_urls {
                ProxyDecision::Modify.into()
            } else {
                ProxyDecision::Passthrough.into()
            };
        }

        // Delete book - intercept if it's our book
        if path.starts_with("/v1/library/") && method == "DELETE" && !path.contains("/tags/") {
            if let Some(book_id) = extractors::extract_book_id(path)
                && let Some(summary) = self.get_book_summary(book_id).await
            {
                return ClassifyResult::with_notes(ProxyDecision::Intercept, summary.to_note());
            }
            return ProxyDecision::Passthrough.into();
        }

        // Tags - intercept if it's our tag
        if path.starts_with("/v1/library/tags") {
            // POST /v1/library/tags creates a new tag - always intercept
            if path == "/v1/library/tags" && method == "POST" {
                return ProxyDecision::Intercept.into();
            }

            // Other tag operations - check if it's our tag
            if let Some(tag_id) = extractors::extract_tag_id(path)
                && self.is_our_tag(tag_id).await
            {
                return ProxyDecision::Intercept.into();
            }
            return if rt_config.rewrite_kobo_urls {
                ProxyDecision::Modify.into()
            } else {
                ProxyDecision::Passthrough.into()
            };
        }

        // Product reviews - intercept if it's our book (prevent local UUIDs leaking to Kobo)
        if path.starts_with("/v1/products/") && path.ends_with("/reviews") {
            if let Some(product_id) = extractors::extract_product_id(path)
                && let Some(summary) = self.get_book_summary(product_id).await
            {
                return ClassifyResult::with_notes(ProxyDecision::Intercept, summary.to_note());
            }
            return ProxyDecision::Passthrough.into();
        }

        // Product nextread - intercept if it's our book
        // For unknown books (including Kobo books), passthrough to get upstream recommendations
        if path.starts_with("/v1/products/") && path.ends_with("/nextread") {
            if let Some(product_id) = extractors::extract_product_id(path)
                && let Some(summary) = self.get_book_summary(product_id).await
            {
                return ClassifyResult::with_notes(ProxyDecision::Intercept, summary.to_note());
            }
            return if rt_config.rewrite_kobo_urls {
                ProxyDecision::Modify.into()
            } else {
                ProxyDecision::Passthrough.into()
            };
        }

        // Product recommendations - intercept if it's our book
        if path.starts_with("/v1/products/") && path.ends_with("/recommendations") {
            if let Some(product_id) = extractors::extract_product_id(path)
                && let Some(summary) = self.get_book_summary(product_id).await
            {
                return ClassifyResult::with_notes(ProxyDecision::Intercept, summary.to_note());
            }
            return if rt_config.rewrite_kobo_urls {
                ProxyDecision::Modify.into()
            } else {
                ProxyDecision::Passthrough.into()
            };
        }

        // Product related - intercept if it's our book (reuse nextread recommendations)
        if path.starts_with("/v1/products/") && path.ends_with("/related") {
            if let Some(product_id) = extractors::extract_product_id(path)
                && let Some(summary) = self.get_book_summary(product_id).await
            {
                return ClassifyResult::with_notes(ProxyDecision::Intercept, summary.to_note());
            }
            return if rt_config.rewrite_kobo_urls {
                ProxyDecision::Modify.into()
            } else {
                ProxyDecision::Passthrough.into()
            };
        }

        // Product book details - intercept if it's our book
        if path.starts_with("/v1/products/books/")
            && !path.starts_with("/v1/products/books/series/")
        {
            if let Some(book_id) = extractors::extract_product_book_id(path)
                && let Some(summary) = self.get_book_summary(book_id).await
            {
                return ClassifyResult::with_notes(ProxyDecision::Intercept, summary.to_note());
            }
            return if rt_config.rewrite_kobo_urls {
                ProxyDecision::Modify.into()
            } else {
                ProxyDecision::Passthrough.into()
            };
        }

        // Rating - intercept if it's our book
        if path.contains("/rating/") && path.starts_with("/v1/products/") && method == "POST" {
            if let Some((product_id, _)) = extractors::extract_rating_from_path(path)
                && let Some(summary) = self.get_book_summary(product_id).await
            {
                return ClassifyResult::with_notes(ProxyDecision::Intercept, summary.to_note());
            }
            return ProxyDecision::Passthrough.into();
        }

        // Series books - intercept if it's our series
        if path.starts_with("/v1/products/books/series/") {
            if let Some(series_id) = extractors::extract_series_id(path)
                && self.is_our_series(series_id).await
            {
                return ProxyDecision::Intercept.into();
            }
            return if rt_config.rewrite_kobo_urls {
                ProxyDecision::Modify.into()
            } else {
                ProxyDecision::Passthrough.into()
            };
        }

        // Recommendations feedback - always intercept to prevent local UUIDs leaking.
        // Body contains FeedbackItems with RevisionIds that may be local book UUIDs.
        if path == "/v1/user/recommendations/feedback" && method == "POST" {
            return ProxyDecision::Intercept.into();
        }

        // Catch-all: any /v1/products/{uuid}/... path referencing a local book — intercept
        // to prevent leaking internal UUIDs to Kobo for endpoints we don't explicitly handle.
        if path.starts_with("/v1/products/")
            && let Some(product_id) = extractors::extract_product_id(path)
            && let Some(summary) = self.get_book_summary(product_id).await
        {
            return ClassifyResult::with_alert(
                ProxyDecision::Intercept,
                summary.to_note(),
                format!(
                    "Intercepted unhandled product endpoint for local book: {}",
                    summary.title
                ),
            );
        }

        // Everything else: passthrough (or modify if URL rewriting enabled)
        if rt_config.rewrite_kobo_urls {
            ProxyDecision::Modify.into()
        } else {
            ProxyDecision::Passthrough.into()
        }
    }

    async fn handle(&self, req: ProxyRequest) -> ProxyResponse {
        let path = &req.path;
        let method = req.method.as_str();
        let rt_config = self.runtime_config().await;
        let ctx = self.context(&req, &rt_config);

        info!(path = %path, method = %method, "Handling intercepted request");

        // Download
        if path.starts_with("/download/")
            && let Some(rest) = path.strip_prefix("/download/")
        {
            let parts: Vec<&str> = rest.split('/').collect();
            if parts.len() >= 2
                && let Ok(book_id) = Uuid::parse_str(parts[0])
            {
                return handlers::handle_download(
                    &self.pool,
                    &self.config.storage_root,
                    ctx.device_token,
                    book_id,
                    parts[1],
                )
                .await;
            }
            return ProxyResponse::json(400, &json!({"error": "invalid download path"}));
        }

        // Cover images: /book-images/{id}/{w}/{h}/{g}/image.jpg or /book-images/{id}/{w}/{h}/{q}/{g}/image.jpg
        if path.starts_with("/book-images/") && path.ends_with("/image.jpg") {
            let parts: Vec<&str> = path.split('/').collect();
            // /book-images/{id}/{w}/{h}/false/image.jpg = 7 parts
            // /book-images/{id}/{w}/{h}/{q}/{g}/image.jpg = 8 parts
            if parts.len() >= 7
                && let Ok(book_id) = Uuid::parse_str(parts[2])
            {
                let width = kobo::parse_dimension(parts[3]);
                let height = kobo::parse_dimension(parts[4]);
                let (quality, greyscale) = if parts.len() >= 8 {
                    (
                        kobo::parse_quality(parts[5]),
                        kobo::is_greyscale_flag(parts[6].trim_end_matches("/image.jpg")),
                    )
                } else {
                    (
                        80,
                        kobo::is_greyscale_flag(parts[5].trim_end_matches("/image.jpg")),
                    )
                };
                return handlers::handle_cover(
                    &self.pool,
                    &self.config.storage_root,
                    book_id,
                    width,
                    height,
                    quality,
                    greyscale,
                )
                .await;
            }
            return ProxyResponse::json(400, &json!({"error": "invalid cover path"}));
        }

        // Book metadata
        if path.starts_with("/v1/library/") && path.ends_with("/metadata") {
            if let Some(book_id_str) = extractors::extract_book_id(path)
                && let Ok(book_id) = Uuid::parse_str(book_id_str)
            {
                return handlers::handle_metadata(&self.pool, &ctx, book_id).await;
            }
            return ProxyResponse::json(400, &json!({"error": "invalid book id"}));
        }

        // Reading state
        if path.starts_with("/v1/library/") && path.ends_with("/state") {
            if let Some(book_id_str) = extractors::extract_book_id(path)
                && let Ok(book_id) = Uuid::parse_str(book_id_str)
            {
                return match method {
                    "GET" => handlers::handle_get_reading_state(&self.pool, &ctx, book_id).await,
                    "PUT" => {
                        handlers::handle_update_reading_state(
                            &self.pool,
                            &self.nats_publisher,
                            &ctx,
                            book_id,
                        )
                        .await
                    }
                    _ => ProxyResponse::json(405, &json!({"error": "method not allowed"})),
                };
            }
            return ProxyResponse::json(400, &json!({"error": "invalid book id"}));
        }

        // Delete book
        if path.starts_with("/v1/library/") && method == "DELETE" && !path.contains("/tags/") {
            if let Some(book_id_str) = extractors::extract_book_id(path)
                && let Ok(book_id) = Uuid::parse_str(book_id_str)
            {
                return handlers::handle_delete_book(&self.pool, &ctx, book_id).await;
            }
            return ProxyResponse::json(400, &json!({"error": "invalid book id"}));
        }

        // Create tag
        if path == "/v1/library/tags" && method == "POST" {
            return handlers::handle_create_tag(&self.pool, &ctx).await;
        }

        // Tag operations
        if path.starts_with("/v1/library/tags/") {
            if let Some(tag_id_str) = extractors::extract_tag_id(path)
                && let Ok(tag_id) = Uuid::parse_str(tag_id_str)
            {
                // /v1/library/tags/{id}/items/delete
                if path.ends_with("/items/delete") && method == "POST" {
                    return handlers::handle_remove_tag_items(&self.pool, &ctx, tag_id).await;
                }
                // /v1/library/tags/{id}/items
                if path.ends_with("/items") && method == "POST" {
                    return handlers::handle_add_tag_items(&self.pool, &ctx, tag_id).await;
                }
                // /v1/library/tags/{id} PUT (rename)
                if method == "PUT" {
                    return handlers::handle_rename_tag(&self.pool, &ctx, tag_id).await;
                }
                // /v1/library/tags/{id} DELETE
                if method == "DELETE" {
                    return handlers::handle_delete_tag(&self.pool, &ctx, tag_id).await;
                }
            }
            return ProxyResponse::json(400, &json!({"error": "invalid tag id"}));
        }

        // Product reviews - return empty reviews for local books
        if path.starts_with("/v1/products/") && path.ends_with("/reviews") {
            return ProxyResponse::json(
                200,
                &json!({"Items": [], "TotalPageCount": 0, "CurrentPageIndex": 0}),
            );
        }

        // Product nextread - return recommendations for local books
        if path.starts_with("/v1/products/") && path.ends_with("/nextread") {
            if let Some(product_id_str) = extractors::extract_product_id(path)
                && let Ok(product_id) = Uuid::parse_str(product_id_str)
            {
                return handlers::handle_nextread(&self.pool, &ctx, product_id).await;
            }
            return ProxyResponse::json(200, &json!({}));
        }

        // Product recommendations - return paginated recommendations for local books
        if path.starts_with("/v1/products/") && path.ends_with("/recommendations") {
            if let Some(product_id_str) = extractors::extract_product_id(path)
                && let Ok(product_id) = Uuid::parse_str(product_id_str)
            {
                return handlers::handle_recommendations(&self.pool, &ctx, product_id).await;
            }
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

        // Product related - return recommendations (reuse nextread logic)
        if path.starts_with("/v1/products/") && path.ends_with("/related") {
            if let Some(product_id_str) = extractors::extract_product_id(path)
                && let Ok(product_id) = Uuid::parse_str(product_id_str)
            {
                return handlers::handle_nextread(&self.pool, &ctx, product_id).await;
            }
            return ProxyResponse::json(200, &json!({}));
        }

        // Product book details
        if path.starts_with("/v1/products/books/")
            && !path.starts_with("/v1/products/books/series/")
        {
            if let Some(book_id_str) = extractors::extract_product_book_id(path)
                && let Ok(book_id) = Uuid::parse_str(book_id_str)
            {
                return handlers::handle_product_book(&self.pool, &ctx, book_id).await;
            }
            return ProxyResponse::json(404, &json!({"error": "not found"}));
        }

        // Rating
        if path.contains("/rating/") && path.starts_with("/v1/products/") {
            if let Some((product_id_str, rating)) = extractors::extract_rating_from_path(path)
                && let Ok(book_id) = Uuid::parse_str(product_id_str)
            {
                return handlers::handle_rating(
                    &self.pool,
                    &self.nats_publisher,
                    &ctx,
                    book_id,
                    rating,
                )
                .await;
            }
            return ProxyResponse::json(400, &json!({"error": "invalid rating path"}));
        }

        // Recommendations feedback - extract ratings for local books
        if path == "/v1/user/recommendations/feedback" && method == "POST" {
            return handlers::handle_recommendations_feedback(
                &self.pool,
                &self.nats_publisher,
                &ctx,
            )
            .await;
        }

        // Series books - return books in the series
        if path.starts_with("/v1/products/books/series/") {
            if let Some(series_id_str) = extractors::extract_series_id(path)
                && let Ok(series_id) = Uuid::parse_str(series_id_str)
            {
                return handlers::handle_series_books(&self.pool, &ctx, series_id).await;
            }
            return ProxyResponse::json(400, &json!({"error": "invalid series id"}));
        }

        // Catch-all for product endpoints we intercepted but don't have specific handlers for
        // (e.g. /prices). Return safe empty response to avoid leaking UUIDs.
        if path.starts_with("/v1/products/") {
            return ProxyResponse::json(200, &json!({"Items": []}));
        }

        // Initialization - return our resources (for local mode)
        if path == "/v1/initialization" {
            return handlers::handle_initialization(&ctx);
        }

        // Sync - return our data (for local mode)
        if path == "/v1/library/sync" {
            return handlers::handle_sync(&self.pool, &ctx, &rt_config).await;
        }

        ProxyResponse::json(404, &json!({"error": "not found"}))
    }

    async fn modify(&self, req: &ProxyRequest, mut resp: ProxyResponse) -> ProxyResponse {
        let path = &req.path;
        let rt_config = self.runtime_config().await;
        let ctx = self.context(req, &rt_config);

        info!(path = %path, upstream_status = resp.status, "Modifying response");

        // CRITICAL: Pass through upstream errors so the device can handle them
        // (e.g., 401 ExpiredToken triggers token refresh on the device)
        if resp.status >= 400 {
            info!(
                path = %path,
                status = resp.status,
                "Passing through upstream error to device"
            );
            return resp;
        }

        // Get device's upstream URL for URL rewriting
        let device_upstream = self
            .upstream_for(ctx.device_token)
            .await
            .map(|c| c.base_url)
            .unwrap_or_else(|| rt_config.kobo_storeapi_base_url.clone());

        // Initialization - merge our URLs into Kobo's successful response
        if path == "/v1/initialization" {
            // Parse upstream response - should have Resources (body should be buffered)
            let upstream_resources: Option<Value> = resp
                .body
                .as_bytes()
                .and_then(|b| serde_json::from_slice(b).ok())
                .and_then(|v: Value| v.get("Resources").cloned());

            let mut merged = kobo::merge_resources(upstream_resources, &ctx.base_url);

            // Optionally rewrite ALL kobo URLs to pass through our proxy (for debugging)
            if rt_config.rewrite_kobo_urls {
                url_rewrite::rewrite_kobo_urls(&mut merged, &ctx.base_url, &device_upstream, true);
            }

            // Keep upstream headers (including x-kobo-apitoken), just update body
            let headers = resp.headers.clone();

            return ProxyResponse::json_with_headers(
                resp.status,
                &json!({"Resources": merged}),
                headers,
            );
        }

        // Sync - merge our entitlements into Kobo's successful response
        if path == "/v1/library/sync" {
            let device = match self.lookup_device(ctx.device_token).await {
                Ok(d) => d,
                Err(_) => return resp, // Just return upstream if we can't identify device
            };

            // Decode our JWT sync token from the request
            let incoming_header = ctx.sync_token_header.unwrap_or("");
            let sync_token = kobo::SyncToken::decode(incoming_header);
            let last_sync_at = sync_token.last_sync_at();

            // Build our sync response using token-carried cursor
            let our_sync = kobo::build_sync_response(
                &self.pool,
                &ctx.base_url,
                device,
                rt_config.kobo_sync_batch_size,
                last_sync_at,
            )
            .await;

            // Capture upstream's sync token for round-tripping
            let upstream_kt = resp
                .get_header("x-kobo-synctoken")
                .unwrap_or("")
                .to_string();

            // Parse upstream response body (should be buffered)
            let mut upstream_items: Vec<Value> = match resp.body.as_bytes() {
                Some(b) => match serde_json::from_slice(b) {
                    Ok(items) => items,
                    Err(e) => {
                        error!(error = %e, "Failed to parse upstream sync response");
                        return ProxyResponse::json(502, &json!({"error": "upstream error"}));
                    }
                },
                None => vec![],
            };

            // Optionally rewrite ALL kobo URLs in upstream items (for debugging)
            if rt_config.rewrite_kobo_urls {
                for item in &mut upstream_items {
                    url_rewrite::rewrite_kobo_urls(item, &ctx.base_url, &device_upstream, true);
                }
            }

            // Defer DB sync state update until response is delivered (best-effort for UI)
            let on_sent = handlers::build_sync_on_sent(self.pool.clone(), &our_sync);

            // Build new token with updated cursor and upstream kobo token
            let new_token = if let Some(max_ts) = our_sync.max_processed_timestamp {
                sync_token.with_last_sync(max_ts)
            } else {
                sync_token
            }
            .with_kobo_token(upstream_kt);

            // Merge: prepend our items to Kobo's items
            let mut merged_items = our_sync.items;
            merged_items.extend(upstream_items);

            let body_bytes = serde_json::to_vec(&merged_items).unwrap_or_default();
            let body_len = body_bytes.len();
            let continue_sync = our_sync.continue_sync;

            let mut response = ProxyResponse {
                status: resp.status,
                reason: resp.reason,
                headers: resp.headers.clone(),
                body: ResponseBody::Buffered(body_bytes),
                source_file_path: None,
                on_sent,
            };

            // Replace upstream sync token with our JWT token
            response.set_header("x-kobo-synctoken", new_token.encode());
            if continue_sync {
                response.set_header("x-kobo-sync", "continue");
            }
            response.set_header("Content-Type", "application/json; charset=utf-8");
            response.set_header("Content-Length", body_len.to_string());

            return response;
        }

        // Generic URL rewriting for all other responses
        if rt_config.rewrite_kobo_urls {
            // Rewrite Location header for redirects (302, 301, etc.)
            if let Some(location) = resp.get_header("Location")
                && let Some(rewritten) =
                    url_rewrite::try_rewrite_url(location, &ctx.base_url, &device_upstream, false)
            {
                resp.set_header("Location", rewritten);
            }

            // Rewrite URLs in JSON body if present
            if let Some(body_bytes) = resp.body.as_bytes()
                && let Ok(mut json) = serde_json::from_slice::<Value>(body_bytes)
            {
                url_rewrite::rewrite_kobo_urls(&mut json, &ctx.base_url, &device_upstream, false);

                let new_body = serde_json::to_vec(&json).unwrap_or_default();
                resp.body = ResponseBody::Buffered(new_body.clone());
                resp.set_header("Content-Length", new_body.len().to_string());
            }
        }

        resp
    }

    async fn stub(&self, req: &ProxyRequest) -> ProxyResponse {
        let path = &req.path;
        let rt_config = self.runtime_config().await;
        let ctx = self.context(req, &rt_config);

        // For initialization, return our resources
        if path == "/v1/initialization" {
            return handlers::handle_initialization(&ctx);
        }

        // For sync, return only our data
        if path == "/v1/library/sync" {
            // We need to run async code but stub is sync
            // Return empty sync for now - the actual sync will happen in handle()
            // when proxy is disabled and classify returns Modify
            // Actually, when proxy is disabled, classify returns Modify but we never
            // call upstream - so modify() gets called with empty response
            // Let's handle this properly in the modify path
            return ProxyResponse::json(200, &json!([]));
        }

        handlers::get_stub(path, &req.headers)
    }

    async fn upstream_for(&self, device_token: &str) -> Option<UpstreamConfig> {
        // Get device-specific URL, fallback to system default
        let rt_config = self.runtime_config().await;
        let default_url = rt_config.kobo_storeapi_base_url.clone();

        let (_, kobo_api_url) = devices::get_by_token(&self.pool, device_token)
            .await
            .ok()
            .flatten()?;

        let url = kobo_api_url.unwrap_or(default_url);

        Some(UpstreamConfig {
            base_url: url,
            timeout_ms: 30_000,
        })
    }

    async fn proxy_enabled(&self) -> bool {
        self.runtime_config()
            .await
            .kobo_mode
            .should_proxy_upstream()
    }

    async fn transform_upstream_headers(
        &self,
        _req: &ProxyRequest,
        mut headers: Vec<(String, String)>,
    ) -> Vec<(String, String)> {
        for (name, value) in headers.iter_mut() {
            if name.eq_ignore_ascii_case("x-kobo-synctoken") {
                // Try to decode as our JWT. If it fails, it's a real Kobo token — leave as-is.
                if let Some(sync_token) = kobo::SyncToken::try_decode(value) {
                    *value = sync_token.kt;
                }
                break;
            }
        }
        headers
    }
}
