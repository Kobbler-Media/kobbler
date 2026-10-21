//! URL rewriting tests - verify Kobo URLs in passthrough responses are rewritten.
//!
//! When `rewrite_kobo_urls` is enabled:
//! - Metadata for store books should have DownloadUrls rewritten with __upstream_host
//! - Other Kobo domain URLs in responses should be rewritten to go through the proxy

use crate::common::*;
use api::config::KoboMode;
use serde_json::Value;

// =============================================================================
// Metadata URL Rewriting Tests
// =============================================================================

#[tokio::test]
async fn test_store_book_metadata_urls_rewritten_when_enabled() {
    let kobo_mock = KoboMock::start().await;
    kobo_mock.add_simple_book("store-rewrite-book", "Store Rewrite Book");

    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;
    enable_url_rewriting(&server).await;

    let user_id = create_user(&server, "urlrewrite@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Request metadata for store book - should have URLs rewritten
    let resp = server
        .kobo_client()
        .kobo_metadata(&device_token, "store-rewrite-book")
        .await;
    resp.assert_ok();

    let metadata: Value = resp.json();

    // Check that DownloadUrls have been rewritten with __upstream_host
    let download_urls = metadata
        .get("DownloadUrls")
        .and_then(|d| d.as_array())
        .expect("Should have DownloadUrls");

    assert!(
        !download_urls.is_empty(),
        "Should have at least one download URL"
    );

    let url = download_urls[0]["Url"]
        .as_str()
        .expect("DownloadUrl should have Url field");

    // URL should be rewritten to go through proxy with __upstream_host
    assert!(
        url.contains("__upstream_host="),
        "Download URL should contain __upstream_host parameter: {}",
        url
    );
    assert!(
        url.contains("store.kobo.com"),
        "Download URL should reference store.kobo.com in __upstream_host: {}",
        url
    );
    assert!(
        !url.starts_with("https://store.kobo.com"),
        "Download URL should NOT point directly to store.kobo.com: {}",
        url
    );
}

#[tokio::test]
async fn test_store_book_metadata_urls_not_rewritten_when_disabled() {
    let kobo_mock = KoboMock::start().await;
    kobo_mock.add_simple_book("store-no-rewrite", "Store No Rewrite Book");

    // Start server WITHOUT enabling URL rewriting (default is disabled)
    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;

    let user_id = create_user(&server, "norewrite@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Request metadata for store book - URLs should NOT be rewritten
    let resp = server
        .kobo_client()
        .kobo_metadata(&device_token, "store-no-rewrite")
        .await;
    resp.assert_ok();

    let metadata: Value = resp.json();

    let download_urls = metadata
        .get("DownloadUrls")
        .and_then(|d| d.as_array())
        .expect("Should have DownloadUrls");

    assert!(
        !download_urls.is_empty(),
        "Should have at least one download URL"
    );

    let url = download_urls[0]["Url"]
        .as_str()
        .expect("DownloadUrl should have Url field");

    // URL should still point directly to store.kobo.com (not rewritten)
    assert!(
        url.starts_with("https://store.kobo.com"),
        "Download URL should point directly to store.kobo.com when rewriting disabled: {}",
        url
    );
    assert!(
        !url.contains("__upstream_host="),
        "Download URL should NOT contain __upstream_host when disabled: {}",
        url
    );
}

#[tokio::test]
async fn test_local_book_metadata_not_affected_by_url_rewriting() {
    let kobo_mock = KoboMock::start().await;

    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;
    enable_url_rewriting(&server).await;

    let user_id = create_user(&server, "localurl@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Create local book
    let local_book = create_book(&server, "Local URL Book").await;
    entitle_book(&server, local_book).await;

    // Request metadata for local book - should be handled locally, not rewritten
    let resp = server
        .kobo_client()
        .kobo_metadata(&device_token, &local_book.to_string())
        .await;
    resp.assert_ok();

    // Verify it was NOT sent to upstream
    assert!(
        !kobo_mock.was_called("GET", &local_book.to_string()),
        "Local book metadata should NOT go to upstream"
    );
}

// =============================================================================
// Sync URL Rewriting Tests
// =============================================================================

#[tokio::test]
async fn test_sync_urls_rewritten_when_enabled() {
    let kobo_mock = KoboMock::start().await;
    kobo_mock.add_simple_book("sync-rewrite-book", "Sync Rewrite Book");

    let server = TestServer::start_with_mode(KoboMode::Merge, None).await;
    enable_url_rewriting(&server).await;

    let user_id = create_user(&server, "syncrewrite@test.com").await;
    let (device_id, device_token) = create_device(&server, user_id).await;
    set_device_upstream(&server, device_id, kobo_mock.url()).await;

    // Sync - store books should have URLs rewritten
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    // Find the store book in sync response
    let store_item = items.iter().find(|item| {
        item.get("NewEntitlement")
            .and_then(|ne| ne.get("BookEntitlement"))
            .and_then(|be| be.get("Id"))
            .and_then(|id| id.as_str())
            == Some("sync-rewrite-book")
    });

    assert!(
        store_item.is_some(),
        "Store book should be in sync response"
    );

    // Check DownloadUrls in the sync item
    if let Some(item) = store_item
        && let Some(urls) = item
            .get("NewEntitlement")
            .and_then(|ne| ne.get("BookMetadata"))
            .and_then(|bm| bm.get("DownloadUrls"))
            .and_then(|du| du.as_array())
    {
        for url_obj in urls {
            if let Some(url) = url_obj.get("Url").and_then(|u| u.as_str()) {
                assert!(
                    url.contains("__upstream_host="),
                    "Sync DownloadUrl should be rewritten: {}",
                    url
                );
            }
        }
    }
}
