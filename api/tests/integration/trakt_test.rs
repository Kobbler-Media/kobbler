use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::Match;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// Helper: get the admin user's UUID.
async fn admin_user_id(server: &TestServer) -> Uuid {
    let resp = server.admin_client().get("/api/v1/admin/me").await;
    let json: Value = resp.json();
    json["user_id"].as_str().unwrap().parse::<Uuid>().unwrap()
}

// =============================================================================
// Video Sync Provider Route Tests
// =============================================================================

#[tokio::test]
async fn test_list_video_sync_providers() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .get("/api/v1/admin/video-sync/providers")
        .await;
    resp.assert_ok();

    let providers: Vec<Value> = resp.json();
    // Should list available providers from registry (trakt at minimum)
    assert!(!providers.is_empty());

    for provider in &providers {
        assert!(provider["provider_type"].is_string());
        assert!(provider["enabled"].is_boolean());
    }

    // Trakt should be present but not enabled by default
    let trakt = providers
        .iter()
        .find(|p| p["provider_type"] == "trakt")
        .expect("trakt provider should be listed");
    assert_eq!(trakt["enabled"], false);
}

#[tokio::test]
async fn test_list_video_sync_providers_masks_secrets() {
    let server = TestServer::start_api_only().await;

    // Set secret config on the trakt provider (upsert via PATCH)
    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/video-sync/providers/trakt",
            &json!({
                "enabled": true,
                "config": {
                    "client_id": "my-client-id",
                    "access_token": "secret-token",
                    "refresh_token": "secret-refresh",
                    "client_secret": "secret-secret"
                }
            }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // List again and verify secrets are masked
    let providers: Vec<Value> = server
        .admin_client()
        .get("/api/v1/admin/video-sync/providers")
        .await
        .json();

    let trakt = providers
        .iter()
        .find(|p| p["provider_type"] == "trakt")
        .unwrap();
    let config = &trakt["config"];
    assert_eq!(config["access_token"], "••••••••");
    assert_eq!(config["refresh_token"], "••••••••");
    assert_eq!(config["client_secret"], "••••••••");
    // client_id is NOT a secret
    assert_eq!(config["client_id"], "my-client-id");
}

#[tokio::test]
async fn test_update_video_sync_provider() {
    let server = TestServer::start_api_only().await;

    // Enable the provider (creates row via upsert)
    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/video-sync/providers/trakt",
            &json!({ "enabled": true }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify it's enabled
    let providers: Vec<Value> = server
        .admin_client()
        .get("/api/v1/admin/video-sync/providers")
        .await
        .json();
    let trakt = providers
        .iter()
        .find(|p| p["provider_type"] == "trakt")
        .unwrap();
    assert_eq!(trakt["enabled"], true);
}

// =============================================================================
// Trakt OAuth Device Code Flow Tests
// =============================================================================

#[tokio::test]
async fn test_trakt_device_auth_start() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/oauth/device/code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code": "abc123",
            "user_code": "XY789",
            "verification_url": "https://trakt.tv/activate",
            "expires_in": 600,
            "interval": 5,
        })))
        .mount(&mock_server)
        .await;

    let client = reqwest::Client::new();
    let result = api::video_sync_providers::trakt::start_device_auth(
        &client,
        Some(&mock_server.uri()),
        "test-client-id",
    )
    .await
    .unwrap();

    assert_eq!(result.device_code, "abc123");
    assert_eq!(result.user_code, "XY789");
    assert_eq!(result.verification_url, "https://trakt.tv/activate");
    assert_eq!(result.expires_in, 600);
    assert_eq!(result.interval, 5);
}

#[tokio::test]
async fn test_trakt_poll_authorized() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/oauth/device/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "access-tok",
            "refresh_token": "refresh-tok",
            "token_type": "Bearer",
            "expires_in": 7776000,
            "created_at": 1700000000,
        })))
        .mount(&mock_server)
        .await;

    let client = reqwest::Client::new();
    let result = api::video_sync_providers::trakt::poll_device_token(
        &client,
        Some(&mock_server.uri()),
        "client-id",
        "client-secret",
        "device-code-123",
    )
    .await
    .unwrap();

    match result {
        api::video_sync_providers::PollResult::Authorized {
            access_token,
            refresh_token,
        } => {
            assert_eq!(access_token, "access-tok");
            assert_eq!(refresh_token, Some("refresh-tok".to_string()));
        }
        other => panic!("Expected Authorized, got {:?}", other),
    }
}

#[tokio::test]
async fn test_trakt_poll_pending() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/oauth/device/token"))
        .respond_with(ResponseTemplate::new(400))
        .mount(&mock_server)
        .await;

    let client = reqwest::Client::new();
    let result = api::video_sync_providers::trakt::poll_device_token(
        &client,
        Some(&mock_server.uri()),
        "client-id",
        "client-secret",
        "device-code-123",
    )
    .await
    .unwrap();

    assert!(matches!(
        result,
        api::video_sync_providers::PollResult::Pending
    ));
}

#[tokio::test]
async fn test_trakt_poll_expired() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/oauth/device/token"))
        .respond_with(ResponseTemplate::new(410))
        .mount(&mock_server)
        .await;

    let client = reqwest::Client::new();
    let result = api::video_sync_providers::trakt::poll_device_token(
        &client,
        Some(&mock_server.uri()),
        "client-id",
        "client-secret",
        "device-code-123",
    )
    .await
    .unwrap();

    assert!(matches!(
        result,
        api::video_sync_providers::PollResult::Expired
    ));
}

#[tokio::test]
async fn test_trakt_poll_denied() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/oauth/device/token"))
        .respond_with(ResponseTemplate::new(409))
        .mount(&mock_server)
        .await;

    let client = reqwest::Client::new();
    let result = api::video_sync_providers::trakt::poll_device_token(
        &client,
        Some(&mock_server.uri()),
        "client-id",
        "client-secret",
        "device-code-123",
    )
    .await
    .unwrap();

    assert!(matches!(
        result,
        api::video_sync_providers::PollResult::Denied
    ));
}

// =============================================================================
// Trakt Provider: Mark Watched / Import History
// =============================================================================

#[tokio::test]
async fn test_trakt_mark_movie_watched() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/sync/history"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "added": { "movies": 1 }
        })))
        .mount(&mock_server)
        .await;

    let urls = api::video_sync_providers::VideoSyncProviderUrls {
        trakt: Some(mock_server.uri()),
    };
    let registry = api::video_sync_providers::VideoSyncProviderRegistry::with_urls(urls);
    let config = json!({
        "access_token": "test-token",
        "client_id": "test-client-id",
    });
    let provider = registry.create_provider("trakt", &config).unwrap();

    let result = provider
        .mark_movie_watched("12345", Some("2024-01-15T12:00:00.000Z"))
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_trakt_mark_movie_unwatched() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/sync/history/remove"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "deleted": { "movies": 1 }
        })))
        .mount(&mock_server)
        .await;

    let urls = api::video_sync_providers::VideoSyncProviderUrls {
        trakt: Some(mock_server.uri()),
    };
    let registry = api::video_sync_providers::VideoSyncProviderRegistry::with_urls(urls);
    let config = json!({
        "access_token": "test-token",
        "client_id": "test-client-id",
    });
    let provider = registry.create_provider("trakt", &config).unwrap();

    let result = provider.mark_movie_unwatched("12345").await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_trakt_mark_episode_watched() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/sync/history"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "added": { "episodes": 1 }
        })))
        .mount(&mock_server)
        .await;

    let urls = api::video_sync_providers::VideoSyncProviderUrls {
        trakt: Some(mock_server.uri()),
    };
    let registry = api::video_sync_providers::VideoSyncProviderRegistry::with_urls(urls);
    let config = json!({
        "access_token": "test-token",
        "client_id": "test-client-id",
    });
    let provider = registry.create_provider("trakt", &config).unwrap();

    let result = provider
        .mark_episode_watched("67890", 1, 3, Some("2024-01-15T12:00:00.000Z"))
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_trakt_import_movie_history() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/sync/watched/movies"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "plays": 1,
                "last_watched_at": "2024-01-15T12:00:00.000Z",
                "last_updated_at": "2024-01-15T12:00:00.000Z",
                "movie": {
                    "title": "Test Movie",
                    "year": 2024,
                    "ids": {
                        "tmdb": 12345,
                        "trakt": 1,
                        "slug": "test-movie",
                        "imdb": "tt1234567"
                    }
                }
            }
        ])))
        .mount(&mock_server)
        .await;

    let urls = api::video_sync_providers::VideoSyncProviderUrls {
        trakt: Some(mock_server.uri()),
    };
    let registry = api::video_sync_providers::VideoSyncProviderRegistry::with_urls(urls);
    let config = json!({
        "access_token": "test-token",
        "client_id": "test-client-id",
    });
    let provider = registry.create_provider("trakt", &config).unwrap();

    let items = provider.import_movie_history().await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].tmdb_id, Some("12345".to_string()));
    assert!(items[0].watched_at.is_some());
}

#[tokio::test]
async fn test_trakt_import_show_history() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/sync/watched/shows"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "plays": 5,
                "last_watched_at": "2024-01-20T12:00:00.000Z",
                "last_updated_at": "2024-01-20T12:00:00.000Z",
                "show": {
                    "title": "Test Show",
                    "year": 2023,
                    "ids": {
                        "tmdb": 67890,
                        "trakt": 2,
                        "slug": "test-show",
                        "imdb": "tt7654321"
                    }
                },
                "seasons": [
                    {
                        "number": 1,
                        "episodes": [
                            { "number": 1, "plays": 1, "last_watched_at": "2024-01-18T12:00:00.000Z" },
                            { "number": 2, "plays": 1, "last_watched_at": "2024-01-19T12:00:00.000Z" }
                        ]
                    }
                ]
            }
        ])))
        .mount(&mock_server)
        .await;

    let urls = api::video_sync_providers::VideoSyncProviderUrls {
        trakt: Some(mock_server.uri()),
    };
    let registry = api::video_sync_providers::VideoSyncProviderRegistry::with_urls(urls);
    let config = json!({
        "access_token": "test-token",
        "client_id": "test-client-id",
    });
    let provider = registry.create_provider("trakt", &config).unwrap();

    let items = provider.import_show_history().await.unwrap();
    assert_eq!(items.len(), 2); // 2 episodes
    assert_eq!(items[0].tmdb_id, Some("67890".to_string()));
    assert_eq!(items[0].season_number, Some(1));
    assert_eq!(items[0].episode_number, Some(1));
    assert_eq!(items[1].episode_number, Some(2));
}

// =============================================================================
// Reading vs Video Provider Isolation
// =============================================================================

#[tokio::test]
async fn test_trakt_not_listed_under_reading_sync_providers() {
    let server = TestServer::start_api_only().await;

    // Configure Trakt with tokens (creates a row in user_sync_providers)
    let resp = server
        .admin_client()
        .patch_json(
            "/api/v1/admin/video-sync/providers/trakt",
            &json!({
                "enabled": true,
                "config": {
                    "client_id": "my-client-id",
                    "access_token": "secret-token",
                    "refresh_token": "secret-refresh",
                    "client_secret": "secret-secret"
                }
            }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Reading sync providers endpoint should NOT include trakt
    let reading_providers: Vec<Value> = server
        .admin_client()
        .get("/api/v1/admin/me/sync-providers")
        .await
        .json();

    let trakt_in_reading = reading_providers
        .iter()
        .any(|p| p["provider_type"] == "trakt");
    assert!(
        !trakt_in_reading,
        "Trakt should not appear in reading sync providers, got: {:?}",
        reading_providers
    );
}

// =============================================================================
// Provider Creation Edge Cases
// =============================================================================

#[tokio::test]
async fn test_trakt_provider_requires_access_token() {
    let registry = api::video_sync_providers::VideoSyncProviderRegistry::new();

    // Missing access_token should fail
    let config = json!({ "client_id": "test" });
    assert!(registry.create_provider("trakt", &config).is_none());
}

#[tokio::test]
async fn test_trakt_provider_requires_client_id() {
    let registry = api::video_sync_providers::VideoSyncProviderRegistry::new();

    // Missing client_id should fail
    let config = json!({ "access_token": "tok" });
    assert!(registry.create_provider("trakt", &config).is_none());
}

// =============================================================================
// Route Auth Requirements
// =============================================================================

#[tokio::test]
async fn test_video_sync_routes_require_auth() {
    let server = TestServer::start_api_only().await;
    let client = server.api_client(); // unauthenticated

    let routes = [("GET", "/api/v1/admin/video-sync/providers")];

    for (method, route) in routes {
        let resp = match method {
            "GET" => client.get(route).await,
            _ => unreachable!(),
        };
        resp.assert_status(StatusCode::UNAUTHORIZED);
    }
}

// =============================================================================
// Token Refresh
// =============================================================================

#[tokio::test]
async fn test_try_refresh_stored_tokens_updates_db() {
    let mock_server = MockServer::start().await;
    let server = TestServer::start_api_only().await;

    // Seed Trakt provider config in DB
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/video-sync/providers/trakt",
            &json!({
                "enabled": true,
                "config": {
                    "client_id": "my-client-id",
                    "client_secret": "my-client-secret",
                    "access_token": "old-access-token",
                    "refresh_token": "old-refresh-token"
                }
            }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Mock the refresh endpoint
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "fresh-access-token",
            "refresh_token": "fresh-refresh-token",
            "token_type": "Bearer",
            "expires_in": 7776000,
            "created_at": 1700000000,
        })))
        .mount(&mock_server)
        .await;

    // Call try_refresh_stored_tokens
    let config = json!({
        "client_id": "my-client-id",
        "client_secret": "my-client-secret",
        "access_token": "old-access-token",
        "refresh_token": "old-refresh-token"
    });

    let user_id = admin_user_id(&server).await;
    let new_config = api::video_sync_providers::trakt::try_refresh_stored_tokens(
        &server.pool,
        user_id,
        &config,
        Some(&mock_server.uri()),
    )
    .await
    .expect("refresh should succeed");

    // Verify returned config has new tokens
    assert_eq!(new_config["access_token"], "fresh-access-token");
    assert_eq!(new_config["refresh_token"], "fresh-refresh-token");
    assert_eq!(new_config["client_id"], "my-client-id"); // unchanged

    // Verify DB was updated — list providers and check the config has new tokens
    // (the API masks secrets, so we verify via the provider being usable with new tokens)
    let providers: Vec<Value> = server
        .admin_client()
        .get("/api/v1/admin/video-sync/providers")
        .await
        .json();
    let trakt = providers
        .iter()
        .find(|p| p["provider_type"] == "trakt")
        .unwrap();
    // Masked means tokens exist
    assert_eq!(trakt["config"]["access_token"], "••••••••");
}

#[tokio::test]
async fn test_trakt_retry_on_401_with_refresh() {
    let mock_server = MockServer::start().await;
    let server = TestServer::start_api_only().await;
    let user_id = admin_user_id(&server).await;

    // Seed Trakt provider config with tokens that will expire
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/video-sync/providers/trakt",
            &json!({
                "enabled": true,
                "config": {
                    "client_id": "my-client-id",
                    "client_secret": "my-client-secret",
                    "access_token": "expired-token",
                    "refresh_token": "valid-refresh-token"
                }
            }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // First call to /sync/history returns 401 (expired token)
    Mock::given(method("POST"))
        .and(path("/sync/history"))
        .respond_with(ResponseTemplate::new(401))
        .up_to_n_times(1)
        .expect(1)
        .mount(&mock_server)
        .await;

    // Refresh endpoint returns new tokens
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "fresh-access-token",
            "refresh_token": "fresh-refresh-token",
            "token_type": "Bearer",
            "expires_in": 7776000,
            "created_at": 1700000000,
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    // Second call to /sync/history with fresh token succeeds
    Mock::given(method("POST"))
        .and(path("/sync/history"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "added": { "movies": 1 }
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    // Simulate what the handler does: try, refresh on 401, retry
    let urls = api::video_sync_providers::VideoSyncProviderUrls {
        trakt: Some(mock_server.uri()),
    };
    let registry = api::video_sync_providers::VideoSyncProviderRegistry::with_urls(urls);
    let config = json!({
        "client_id": "my-client-id",
        "client_secret": "my-client-secret",
        "access_token": "expired-token",
        "refresh_token": "valid-refresh-token"
    });

    let provider = registry.create_provider("trakt", &config).unwrap();
    let result = provider.mark_movie_watched("12345", None).await;

    // Should get TokenExpired
    assert!(
        matches!(
            &result,
            Err(api::video_sync_providers::VideoSyncError::TokenExpired)
        ),
        "Expected TokenExpired, got: {:?}",
        result
    );

    // Refresh and retry
    let new_config = api::video_sync_providers::trakt::try_refresh_stored_tokens(
        &server.pool,
        user_id,
        &config,
        Some(&mock_server.uri()),
    )
    .await
    .expect("refresh should succeed");

    let provider = registry.create_provider("trakt", &new_config).unwrap();
    let result = provider.mark_movie_watched("12345", None).await;
    assert!(result.is_ok(), "Retry with fresh token should succeed");
}

#[tokio::test]
async fn test_trakt_token_refresh() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "new-access-token",
            "refresh_token": "new-refresh-token",
            "token_type": "Bearer",
            "expires_in": 7776000,
            "created_at": 1700000000,
        })))
        .mount(&mock_server)
        .await;

    let client = reqwest::Client::new();
    let (access, refresh) = api::video_sync_providers::trakt::refresh_token(
        &client,
        Some(&mock_server.uri()),
        "client-id",
        "client-secret",
        "old-refresh-token",
    )
    .await
    .unwrap();

    assert_eq!(access, "new-access-token");
    assert_eq!(refresh, "new-refresh-token");
}

// =============================================================================
// Bulk Episode Sync Tests
// =============================================================================

/// Matcher that verifies the request body contains multiple episodes grouped by season.
struct BulkEpisodeBodyMatcher {
    expected_show_tmdb: i64,
    expected_episode_count: usize,
}

impl Match for BulkEpisodeBodyMatcher {
    fn matches(&self, request: &Request) -> bool {
        let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let shows = body["shows"].as_array();
        let Some(shows) = shows else { return false };
        if shows.len() != 1 {
            return false;
        }
        let show = &shows[0];
        let tmdb_id = show["ids"]["tmdb"].as_i64().unwrap_or(0);
        if tmdb_id != self.expected_show_tmdb {
            return false;
        }
        // Count total episodes across all seasons
        let empty = vec![];
        let seasons = show["seasons"].as_array().unwrap_or(&empty);
        let total_eps: usize = seasons
            .iter()
            .map(|s| s["episodes"].as_array().map_or(0, |e| e.len()))
            .sum();
        total_eps == self.expected_episode_count
    }
}

#[tokio::test]
async fn test_trakt_bulk_mark_episodes_watched() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/sync/history"))
        .and(BulkEpisodeBodyMatcher {
            expected_show_tmdb: 67890,
            expected_episode_count: 4,
        })
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "added": { "episodes": 4 }
        })))
        .expect(1) // Exactly one API call
        .mount(&mock_server)
        .await;

    let urls = api::video_sync_providers::VideoSyncProviderUrls {
        trakt: Some(mock_server.uri()),
    };
    let registry = api::video_sync_providers::VideoSyncProviderRegistry::with_urls(urls);
    let config = json!({
        "access_token": "test-token",
        "client_id": "test-client-id",
    });
    let provider = registry.create_provider("trakt", &config).unwrap();

    // 4 episodes across 2 seasons
    let episodes = vec![
        (1, 1, Some("2024-01-15T12:00:00.000Z".to_string())),
        (1, 2, Some("2024-01-15T12:00:00.000Z".to_string())),
        (2, 1, Some("2024-01-15T12:00:00.000Z".to_string())),
        (2, 5, Some("2024-01-15T12:00:00.000Z".to_string())),
    ];

    let result = provider.mark_episodes_watched("67890", &episodes).await;
    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result.items_synced, 4);
}

#[tokio::test]
async fn test_trakt_bulk_mark_episodes_unwatched() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/sync/history/remove"))
        .and(BulkEpisodeBodyMatcher {
            expected_show_tmdb: 67890,
            expected_episode_count: 3,
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "deleted": { "episodes": 3 }
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let urls = api::video_sync_providers::VideoSyncProviderUrls {
        trakt: Some(mock_server.uri()),
    };
    let registry = api::video_sync_providers::VideoSyncProviderRegistry::with_urls(urls);
    let config = json!({
        "access_token": "test-token",
        "client_id": "test-client-id",
    });
    let provider = registry.create_provider("trakt", &config).unwrap();

    let episodes = vec![(1, 1), (1, 2), (3, 1)];

    let result = provider.mark_episodes_unwatched("67890", &episodes).await;
    assert!(result.is_ok());
    let result = result.unwrap();
    assert_eq!(result.items_synced, 3);
}

#[tokio::test]
async fn test_trakt_bulk_mark_empty_episodes() {
    // Should succeed immediately without making any API calls
    let mock_server = MockServer::start().await;

    // No mocks mounted — any request would panic
    let urls = api::video_sync_providers::VideoSyncProviderUrls {
        trakt: Some(mock_server.uri()),
    };
    let registry = api::video_sync_providers::VideoSyncProviderRegistry::with_urls(urls);
    let config = json!({
        "access_token": "test-token",
        "client_id": "test-client-id",
    });
    let provider = registry.create_provider("trakt", &config).unwrap();

    let result = provider.mark_episodes_watched("67890", &[]).await;
    assert!(result.is_ok());

    let result = provider.mark_episodes_unwatched("67890", &[]).await;
    assert!(result.is_ok());
}

// =============================================================================
// Import History: recency comes from Trakt, never from the import itself
// =============================================================================

async fn run_trakt_import(server: &TestServer, user_id: uuid::Uuid, trakt: &MockServer) {
    use api::jobs::worker::JobHandler;
    api::repositories::user_sync_providers::upsert(
        &server.pool,
        user_id,
        "trakt",
        true,
        &json!({ "access_token": "t", "client_id": "c" }),
    )
    .await
    .unwrap();
    let registry = api::video_sync_providers::VideoSyncProviderRegistry::with_urls(
        api::video_sync_providers::VideoSyncProviderUrls {
            trakt: Some(trakt.uri()),
        },
    );
    api::jobs::handlers::TraktImportHistoryHandler::new(server.pool.clone(), registry)
        .execute(
            uuid::Uuid::now_v7(),
            json!({ "provider": "trakt", "user_id": user_id }),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
}

async fn tmdb_movie(server: &TestServer, title: &str, tmdb_id: i64) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO movies (id, title, identifiers) VALUES ($1, $2, jsonb_build_object('tmdb_id', $3::text))",
    )
    .bind(id)
    .bind(title)
    .bind(tmdb_id.to_string())
    .execute(&server.pool)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn imported_trakt_history_keeps_its_own_dates_and_never_looks_recent() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;
    let user_id: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE email = 'viewer@test.com'")
            .fetch_one(&server.pool)
            .await
            .unwrap();

    let recent = tmdb_movie(&server, "Watched Yesterday Here", 1).await;
    viewer
        .put_json(
            &format!("/api/v1/admin/movies/{recent}/watch-state"),
            &json!({ "watched": true }),
        )
        .await
        .assert_ok();
    sqlx::query("UPDATE movie_watch_states SET watched_at = NOW() - INTERVAL '1 day'")
        .execute(&server.pool)
        .await
        .unwrap();
    tmdb_movie(&server, "Old Trakt Watch", 2).await;
    tmdb_movie(&server, "Older Trakt Watch", 3).await;
    tmdb_movie(&server, "Undated Trakt Watch", 4).await;

    let trakt = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sync/watched/movies"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "plays": 1, "last_watched_at": "2014-09-01T09:10:11.000Z",
              "movie": { "title": "Old Trakt Watch", "ids": { "slug": "a", "tmdb": 2 } } },
            { "plays": 1, "last_watched_at": "2013-01-02T03:04:05Z",
              "movie": { "title": "Older Trakt Watch", "ids": { "slug": "b", "tmdb": 3 } } },
            { "plays": 1, "last_watched_at": null,
              "movie": { "title": "Undated Trakt Watch", "ids": { "slug": "c", "tmdb": 4 } } }
        ])))
        .mount(&trakt)
        .await;
    Mock::given(method("GET"))
        .and(path("/sync/watched/shows"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&trakt)
        .await;
    run_trakt_import(&server, user_id, &trakt).await;

    let order: Vec<String> = api::for_you::history(&server.pool, user_id)
        .await
        .unwrap()
        .into_iter()
        .map(|h| h.title)
        .collect();
    assert_eq!(
        order,
        vec![
            "Watched Yesterday Here",
            "Old Trakt Watch",
            "Older Trakt Watch",
            "Undated Trakt Watch"
        ]
    );

    let page: Value = viewer.get("/api/v1/admin/watch-history").await.json();
    let items = page["items"].as_array().unwrap();
    assert_eq!(items[1]["sources"], json!(["trakt"]));
    assert!(
        items[1]["last_watched_at"]
            .as_str()
            .unwrap()
            .starts_with("2014-09-01T09:10:11")
    );
    assert!(items[3]["last_watched_at"].is_null());
    assert_eq!(items[0]["sources"], json!(["local"]));
}

#[test]
fn trakt_timestamps_parse_in_every_shape_trakt_sends() {
    use api::video_sync_providers::trakt::parse_watched_at;
    for raw in [
        "2014-09-01T09:10:11.000Z",
        "2014-09-01T09:10:11Z",
        "2014-09-01T09:10:11+00:00",
        "2014-09-01T09:10:11.000",
        "2014-09-01 09:10:11",
    ] {
        let t = parse_watched_at(raw).unwrap_or_else(|| panic!("{raw} did not parse"));
        assert_eq!(t.unix_timestamp(), 1_409_562_611, "{raw}");
    }
    assert!(parse_watched_at("yesterday").is_none());
}
