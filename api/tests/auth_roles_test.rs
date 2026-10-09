use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;

// =============================================================================
// /me endpoint
// =============================================================================

#[tokio::test]
async fn test_me_returns_user_info() {
    let server = TestServer::start_api_only().await;

    let resp = server.admin_client().get("/api/v1/admin/me").await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["role"], "admin");
    assert!(json["user_id"].is_string());
    assert!(json["email"].is_string());
}

#[tokio::test]
async fn test_me_returns_viewer_role() {
    let server = TestServer::start_api_only().await;
    let client = create_user_client(&server, "viewer@test.com", "viewer").await;

    let resp = client.get("/api/v1/admin/me").await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["role"], "viewer");
}

// =============================================================================
// Login response includes role
// =============================================================================

#[tokio::test]
async fn test_login_returns_role() {
    let server = TestServer::start_api_only().await;
    create_user_with_role(&server, "contrib@test.com", "contributor").await;

    let client = TestClient::new(&server.api_url);
    let resp = client
        .post_json(
            "/api/v1/admin/login",
            &serde_json::json!({
                "email": "contrib@test.com",
                "password": "testpass123"
            }),
        )
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["role"], "contributor");
    assert!(json["token"].is_string());
}

// =============================================================================
// Viewer cannot access admin routes (403)
// =============================================================================

#[tokio::test]
async fn test_viewer_cannot_create_user() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;

    let resp = viewer
        .post_json(
            "/api/v1/admin/users",
            &serde_json::json!({
                "email": "sneaky@test.com",
                "password": "password123"
            }),
        )
        .await;
    resp.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_viewer_cannot_access_config() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;

    let resp = viewer.get("/api/v1/admin/config").await;
    resp.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_viewer_cannot_access_ingest_tokens() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;

    let resp = viewer.get("/api/v1/admin/ingest-tokens").await;
    resp.assert_status(StatusCode::FORBIDDEN);
}

// =============================================================================
// Contributor cannot access admin-only routes (403)
// =============================================================================

#[tokio::test]
async fn test_contributor_cannot_access_config() {
    let server = TestServer::start_api_only().await;
    let contrib = create_user_client(&server, "contrib@test.com", "contributor").await;

    let resp = contrib.get("/api/v1/admin/config").await;
    resp.assert_status(StatusCode::FORBIDDEN);
}

// =============================================================================
// Admin can access everything
// =============================================================================

#[tokio::test]
async fn test_admin_can_list_users() {
    let server = TestServer::start_api_only().await;

    let resp = server.admin_client().get("/api/v1/admin/users").await;
    resp.assert_ok();
}

#[tokio::test]
async fn test_admin_can_access_config() {
    let server = TestServer::start_api_only().await;

    let resp = server.admin_client().get("/api/v1/admin/config").await;
    resp.assert_ok();
}

// =============================================================================
// All roles can access library routes
// =============================================================================

#[tokio::test]
async fn test_viewer_can_access_library() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;

    let resp = viewer.get("/api/v1/library/books").await;
    resp.assert_ok();
}

#[tokio::test]
async fn test_viewer_can_access_dashboard() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;

    let resp = viewer.get("/api/v1/admin/dashboard").await;
    resp.assert_ok();
}

// =============================================================================
// Device ownership scoping
// =============================================================================

#[tokio::test]
async fn test_viewer_only_sees_own_devices() {
    let server = TestServer::start_api_only().await;

    // Create viewer + their device
    let viewer_id = create_user_with_role(&server, "viewer@test.com", "viewer");
    let viewer = create_user_client(&server, "viewer2@test.com", "viewer").await;

    // Admin creates a device for the admin user
    let admin_user_id = {
        let resp = server.admin_client().get("/api/v1/admin/me").await;
        let json: Value = resp.json();
        json["user_id"]
            .as_str()
            .unwrap()
            .parse::<uuid::Uuid>()
            .unwrap()
    };
    create_device(&server, admin_user_id).await;

    // Viewer lists devices — should not see admin's device
    let resp = viewer.get("/api/v1/admin/devices").await;
    resp.assert_ok();

    let devices: Vec<Value> = resp.json();
    // Viewer has no devices, so should see empty list
    assert!(devices.is_empty());

    // Cleanup: use viewer_id future
    let _ = viewer_id.await;
}

#[tokio::test]
async fn test_viewer_cannot_access_other_users_device() {
    let server = TestServer::start_api_only().await;

    // Admin creates a device for themselves
    let admin_user_id = {
        let resp = server.admin_client().get("/api/v1/admin/me").await;
        let json: Value = resp.json();
        json["user_id"]
            .as_str()
            .unwrap()
            .parse::<uuid::Uuid>()
            .unwrap()
    };
    let (device_id, _) = create_device(&server, admin_user_id).await;

    // Viewer tries to access admin's device
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;
    let resp = viewer
        .get(&format!("/api/v1/admin/devices/{}/activity", device_id))
        .await;
    resp.assert_status(StatusCode::FORBIDDEN);
}

// =============================================================================
// IDOR prevention: create_device ignores user_id for non-admins
// =============================================================================

#[tokio::test]
async fn test_viewer_cannot_create_device_for_other_user() {
    let server = TestServer::start_api_only().await;

    let admin_user_id = {
        let resp = server.admin_client().get("/api/v1/admin/me").await;
        let json: Value = resp.json();
        json["user_id"]
            .as_str()
            .unwrap()
            .parse::<uuid::Uuid>()
            .unwrap()
    };

    // Viewer tries to create a device with admin's user_id
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;
    let resp = viewer
        .post_json(
            "/api/v1/admin/devices",
            &serde_json::json!({
                "user_id": admin_user_id,
                "name": "Sneaky Device"
            }),
        )
        .await;
    resp.assert_ok();

    // The device should be created under the viewer's ID, not admin's
    let json: Value = resp.json();
    let device_id: uuid::Uuid = json["device_id"].as_str().unwrap().parse().unwrap();

    // Viewer can see it (it's their device)
    let resp = viewer
        .get(&format!("/api/v1/admin/devices/{}/activity", device_id))
        .await;
    resp.assert_ok();

    // Admin's device list should NOT include the viewer's device
    // (well it should since admin sees all, but let's verify the device has the right user)
    let resp = viewer.get("/api/v1/admin/devices").await;
    resp.assert_ok();
    let devices: Vec<Value> = resp.json();
    assert_eq!(devices.len(), 1);
    // Device should NOT have admin's user_id
    assert_ne!(
        devices[0]["user_id"].as_str().unwrap(),
        admin_user_id.to_string()
    );
}

// =============================================================================
// Last admin protection
// =============================================================================

#[tokio::test]
async fn test_cannot_demote_last_admin() {
    let server = TestServer::start_api_only().await;

    let admin_user_id = {
        let resp = server.admin_client().get("/api/v1/admin/me").await;
        let json: Value = resp.json();
        json["user_id"].as_str().unwrap().to_string()
    };

    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/users/{}", admin_user_id),
            &serde_json::json!({ "role": "viewer" }),
        )
        .await;
    resp.assert_status(StatusCode::CONFLICT);
}

// =============================================================================
// User creation with role
// =============================================================================

#[tokio::test]
async fn test_create_user_with_role() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/users",
            &serde_json::json!({
                "email": "contrib@test.com",
                "password": "testpass123",
                "role": "contributor"
            }),
        )
        .await;
    resp.assert_ok();

    // Verify role in user list
    let resp = server.admin_client().get("/api/v1/admin/users").await;
    resp.assert_ok();
    let users: Vec<Value> = resp.json();
    let contrib = users
        .iter()
        .find(|u| u["email"] == "contrib@test.com")
        .expect("contributor not found");
    assert_eq!(contrib["role"], "contributor");
}

#[tokio::test]
async fn test_create_user_defaults_to_viewer() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/users",
            &serde_json::json!({
                "email": "noob@test.com",
                "password": "testpass123"
            }),
        )
        .await;
    resp.assert_ok();

    let resp = server.admin_client().get("/api/v1/admin/users").await;
    resp.assert_ok();
    let users: Vec<Value> = resp.json();
    let noob = users
        .iter()
        .find(|u| u["email"] == "noob@test.com")
        .expect("user not found");
    assert_eq!(noob["role"], "viewer");
}

#[tokio::test]
async fn test_invalid_role_rejected() {
    let server = TestServer::start_api_only().await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/users",
            &serde_json::json!({
                "email": "hacker@test.com",
                "password": "testpass123",
                "role": "superadmin"
            }),
        )
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
}

// =============================================================================
// add_book_all_devices is admin-only
// =============================================================================

#[tokio::test]
async fn test_viewer_cannot_add_book_all_devices() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "Test Book").await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;

    let resp = viewer
        .post(&format!("/api/v1/admin/books/{}/devices/all", book_id))
        .await;
    resp.assert_status(StatusCode::FORBIDDEN);
}

// =============================================================================
// Browse surfaces are open to every authenticated role
// =============================================================================

/// Read-only endpoints that power Home, the command palette and metadata
/// pickers. These must never 403 — a blank home page for a non-admin is a bug.
const OPEN_BROWSE_ROUTES: &[&str] = &[
    "/api/v1/admin/discover",
    "/api/v1/admin/discover/watch-providers",
    "/api/v1/admin/search?q=test",
    "/api/v1/admin/search/status",
    "/api/v1/admin/video-metadata/providers",
    "/api/v1/admin/video-metadata/search/movies?q=matrix",
    "/api/v1/admin/video-metadata/search/tv?q=friends",
    "/api/v1/admin/metadata/providers",
    "/api/v1/admin/chat/status",
    // Per-user integration rows, secrets masked in the response.
    "/api/v1/admin/video-sync/providers",
    // Push subscriptions are per-user; everyone can opt in.
    "/api/v1/admin/notifications/webpush/vapid-key",
];

/// Endpoints that expose provider credentials or mutate server settings.
const ADMIN_ONLY_ROUTES: &[&str] = &[
    "/api/v1/admin/config",
    "/api/v1/admin/ingest-tokens",
    "/api/v1/admin/subtitle-providers",
    "/api/v1/admin/iptv/providers",
    "/api/v1/admin/notifications/agents",
    "/api/v1/admin/jobs/concurrency",
];

async fn assert_routes_not_forbidden(client: &TestClient, routes: &[&str], role: &str) {
    for route in routes {
        let resp = client.get(route).await;
        assert_ne!(
            resp.status,
            StatusCode::FORBIDDEN,
            "{role} was denied browse route {route}"
        );
    }
}

async fn assert_routes_forbidden(client: &TestClient, routes: &[&str], role: &str) {
    for route in routes {
        let resp = client.get(route).await;
        assert_eq!(
            resp.status,
            StatusCode::FORBIDDEN,
            "{role} was allowed admin-only route {route}"
        );
    }
}

#[tokio::test]
async fn test_contributor_can_reach_browse_routes() {
    let server = TestServer::start_api_only().await;
    let contrib = create_user_client(&server, "contrib@test.com", "contributor").await;

    assert_routes_not_forbidden(&contrib, OPEN_BROWSE_ROUTES, "contributor").await;
    assert_routes_forbidden(&contrib, ADMIN_ONLY_ROUTES, "contributor").await;
}

#[tokio::test]
async fn test_viewer_can_reach_browse_routes() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;

    assert_routes_not_forbidden(&viewer, OPEN_BROWSE_ROUTES, "viewer").await;
    assert_routes_forbidden(&viewer, ADMIN_ONLY_ROUTES, "viewer").await;
}

#[tokio::test]
async fn test_discover_returns_home_sections_for_viewer() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;

    let resp = viewer.get("/api/v1/admin/discover").await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert!(json["watch_next"].is_array(), "watch_next missing");
    assert!(json["recently_added"].is_array(), "recently_added missing");
    assert!(json["sections"].is_array(), "sections missing");
}

#[tokio::test]
async fn test_discover_writes_still_require_contributor() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;

    let resp = viewer.post("/api/v1/admin/discover/refresh").await;
    resp.assert_status(StatusCode::FORBIDDEN);

    let resp = viewer
        .post_json(
            "/api/v1/admin/discover/want-episode",
            &serde_json::json!({
                "provider": "tmdb",
                "provider_id": "1399",
                "season_number": 1,
                "episode_number": 1,
                "title": "Game of Thrones"
            }),
        )
        .await;
    resp.assert_status(StatusCode::FORBIDDEN);

    let resp = viewer
        .post_json(
            "/api/v1/admin/discover/want-season",
            &serde_json::json!({
                "provider": "tmdb",
                "provider_id": "1399",
                "season_number": 1,
                "title": "Game of Thrones"
            }),
        )
        .await;
    resp.assert_status(StatusCode::FORBIDDEN);
}

// =============================================================================
// Provider credentials are never sent to non-admins
// =============================================================================

fn assert_all_config_fields_blank(providers: &[Value], route: &str, role: &str) {
    for provider in providers {
        let fields = provider["config_fields"]
            .as_array()
            .unwrap_or_else(|| panic!("{route}: config_fields missing"));
        assert!(
            fields.is_empty(),
            "{role} received provider config for {route}: {provider}"
        );
    }
}

fn assert_some_config_fields_present(providers: &[Value], route: &str) {
    let any = providers
        .iter()
        .any(|p| p["config_fields"].as_array().is_some_and(|f| !f.is_empty()));
    assert!(any, "{route}: admin saw no provider config fields at all");
}

#[tokio::test]
async fn test_provider_lists_hide_config_from_non_admins() {
    let server = TestServer::start_api_only().await;
    let contrib = create_user_client(&server, "contrib@test.com", "contributor").await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;

    for route in [
        "/api/v1/admin/metadata/providers",
        "/api/v1/admin/video-metadata/providers",
    ] {
        let resp = server.admin_client().get(route).await;
        resp.assert_ok();
        let admin_providers: Vec<Value> = resp.json();
        assert_some_config_fields_present(&admin_providers, route);

        for (client, role) in [(&contrib, "contributor"), (&viewer, "viewer")] {
            let resp = client.get(route).await;
            resp.assert_ok();
            let providers: Vec<Value> = resp.json();
            assert_all_config_fields_blank(&providers, route, role);
        }
    }
}

// =============================================================================
// User list scoping (Devices page needs owner names)
// =============================================================================

#[tokio::test]
async fn test_contributor_sees_full_user_list() {
    let server = TestServer::start_api_only().await;
    create_user_with_role(&server, "someone@test.com", "viewer").await;
    let contrib = create_user_client(&server, "contrib@test.com", "contributor").await;

    let resp = contrib.get("/api/v1/admin/users").await;
    resp.assert_ok();

    let users: Vec<Value> = resp.json();
    assert!(
        users.iter().any(|u| u["email"] == "someone@test.com"),
        "contributor should see other users"
    );
}

#[tokio::test]
async fn test_viewer_only_sees_own_user_record() {
    let server = TestServer::start_api_only().await;
    create_user_with_role(&server, "someone@test.com", "viewer").await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;

    let resp = viewer.get("/api/v1/admin/users").await;
    resp.assert_ok();

    let users: Vec<Value> = resp.json();
    assert_eq!(users.len(), 1, "viewer should only see themselves");
    assert_eq!(users[0]["email"], "viewer@test.com");
}

#[tokio::test]
async fn test_non_admin_cannot_mutate_users() {
    let server = TestServer::start_api_only().await;
    let target_id = create_user_with_role(&server, "target@test.com", "viewer").await;
    let contrib = create_user_client(&server, "contrib@test.com", "contributor").await;

    let resp = contrib
        .patch_json(
            &format!("/api/v1/admin/users/{}", target_id),
            &serde_json::json!({ "role": "admin" }),
        )
        .await;
    resp.assert_status(StatusCode::FORBIDDEN);

    let resp = contrib
        .delete(&format!("/api/v1/admin/users/{}", target_id))
        .await;
    resp.assert_status(StatusCode::FORBIDDEN);
}

// =============================================================================
// Kobload quality profiles: contributor tooling, not viewer tooling
// =============================================================================

/// The media-root picker in MovieForm/TvShowForm/MoveToMediaRootModal needs the
/// collection; per-root browsing stays admin-only.
#[tokio::test]
async fn test_media_root_list_is_contributor_but_detail_is_admin() {
    let server = TestServer::start_api_only().await;
    let contrib = create_user_client(&server, "contrib@test.com", "contributor").await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;

    let resp = contrib.get("/api/v1/admin/media-roots").await;
    resp.assert_ok();

    let resp = viewer.get("/api/v1/admin/media-roots").await;
    resp.assert_status(StatusCode::FORBIDDEN);

    let fake = uuid::Uuid::new_v4();
    for route in [
        format!("/api/v1/admin/media-roots/{fake}"),
        format!("/api/v1/admin/media-roots/{fake}/files"),
        format!("/api/v1/admin/media-roots/{fake}/directories"),
    ] {
        let resp = contrib.get(&route).await;
        resp.assert_status(StatusCode::FORBIDDEN);
    }
}

// =============================================================================
// Web push is per-user
// =============================================================================

async fn subscribe_push(client: &TestClient, endpoint: &str) {
    let resp = client
        .post_json(
            "/api/v1/admin/notifications/webpush/subscribe",
            &serde_json::json!({
                "endpoint": endpoint,
                "p256dh": "test-p256dh",
                "auth": "test-auth",
                "user_agent": "integration-test"
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
}

#[tokio::test]
async fn test_every_role_can_subscribe_to_push() {
    let server = TestServer::start_api_only().await;
    let contrib = create_user_client(&server, "contrib@test.com", "contributor").await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;

    subscribe_push(&contrib, "https://push.example/contrib").await;
    subscribe_push(&viewer, "https://push.example/viewer").await;

    let subs = api::repositories::webpush_subscriptions::list_all(&server.pool)
        .await
        .expect("list subscriptions");
    assert_eq!(subs.len(), 2);
    assert!(
        subs.iter().all(|s| !s.user_id.is_nil()),
        "subscriptions must be attributed to a user"
    );
}

#[tokio::test]
async fn test_cannot_unsubscribe_another_users_endpoint() {
    let server = TestServer::start_api_only().await;
    let contrib = create_user_client(&server, "contrib@test.com", "contributor").await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;

    subscribe_push(&contrib, "https://push.example/contrib").await;

    let resp = viewer
        .delete_json(
            "/api/v1/admin/notifications/webpush/subscribe",
            &serde_json::json!({ "endpoint": "https://push.example/contrib" }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    let subs = api::repositories::webpush_subscriptions::list_all(&server.pool)
        .await
        .expect("list subscriptions");
    assert_eq!(
        subs.len(),
        1,
        "another user's subscription must survive their unsubscribe call"
    );
}

/// Imports mean "something is available to watch" and reach everyone; grabs and
/// failures are operational and stay with admin + contributor.
#[tokio::test]
async fn test_operational_push_excludes_viewers() {
    let server = TestServer::start_api_only().await;
    let contrib = create_user_client(&server, "contrib@test.com", "contributor").await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;

    subscribe_push(&contrib, "https://push.example/contrib").await;
    subscribe_push(&viewer, "https://push.example/viewer").await;

    let all = api::repositories::webpush_subscriptions::list_all(&server.pool)
        .await
        .expect("list all");
    assert_eq!(all.len(), 2, "on_import reaches every subscriber");

    let operational = api::repositories::webpush_subscriptions::list_for_roles(
        &server.pool,
        &["admin", "contributor"],
    )
    .await
    .expect("list for roles");
    let endpoints: Vec<&str> = operational.iter().map(|s| s.endpoint.as_str()).collect();
    assert_eq!(endpoints, vec!["https://push.example/contrib"]);
}

/// Viewers browse the guide and keep their own favourites, but must not touch
/// shared channels, groups or recordings.
#[tokio::test]
async fn test_iptv_shared_state_requires_contributor() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;

    viewer.get("/api/v1/admin/iptv/channels").await.assert_ok();
    viewer
        .get("/api/v1/admin/iptv/favourites")
        .await
        .assert_ok();

    let fake = uuid::Uuid::new_v4();
    viewer
        .post_json(
            "/api/v1/admin/iptv/groups",
            &serde_json::json!({ "name": "Sneaky", "slug": "sneaky" }),
        )
        .await
        .assert_status(StatusCode::FORBIDDEN);
    viewer
        .delete(&format!("/api/v1/admin/iptv/groups/{fake}"))
        .await
        .assert_status(StatusCode::FORBIDDEN);
    viewer
        .delete(&format!("/api/v1/admin/iptv/recordings/{fake}"))
        .await
        .assert_status(StatusCode::FORBIDDEN);
    viewer
        .post_json(
            "/api/v1/admin/iptv/channels/bulk-enable",
            &serde_json::json!({ "channel_ids": [], "enabled": false }),
        )
        .await
        .assert_status(StatusCode::FORBIDDEN);
}

/// Viewers can watch the job list but must not act on it.
#[tokio::test]
async fn test_job_mutations_require_contributor() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;

    viewer.get("/api/v1/admin/jobs").await.assert_ok();

    let fake = uuid::Uuid::new_v4();
    viewer
        .delete("/api/v1/admin/jobs/completed")
        .await
        .assert_status(StatusCode::FORBIDDEN);
    viewer
        .delete(&format!("/api/v1/admin/jobs/{fake}"))
        .await
        .assert_status(StatusCode::FORBIDDEN);
    viewer
        .post(&format!("/api/v1/admin/jobs/{fake}/cancel"))
        .await
        .assert_status(StatusCode::FORBIDDEN);
    viewer
        .post(&format!("/api/v1/admin/jobs/{fake}/retry"))
        .await
        .assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_quality_profiles_require_contributor() {
    let server = TestServer::start_api_only().await;
    let contrib = create_user_client(&server, "contrib@test.com", "contributor").await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;

    let resp = contrib.get("/api/v1/admin/kobload/quality-profiles").await;
    assert_ne!(resp.status, StatusCode::FORBIDDEN);

    let resp = viewer.get("/api/v1/admin/kobload/quality-profiles").await;
    resp.assert_status(StatusCode::FORBIDDEN);
}
