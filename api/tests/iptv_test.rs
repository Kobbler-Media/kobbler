use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

// =============================================================================
// Provider CRUD
// =============================================================================

#[tokio::test]
async fn test_iptv_provider_crud() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    // Create (returns 201 CREATED)
    let resp = client
        .post_json(
            "/api/v1/admin/iptv/providers",
            &json!({
                "name": "Test Provider",
                "url": "http://example.com:8080",
                "username": "user1",
                "password": "pass1"
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let body: Value = resp.json();
    let provider_id = body["id"].as_str().unwrap().to_string();

    // List
    let resp = client.get("/api/v1/admin/iptv/providers").await;
    resp.assert_ok();
    let providers: Vec<Value> = resp.json();
    assert!(!providers.is_empty());

    // Get
    let resp = client
        .get(&format!("/api/v1/admin/iptv/providers/{provider_id}"))
        .await;
    resp.assert_ok();
    let provider: Value = resp.json();
    assert_eq!(provider["name"], "Test Provider");

    // Update (returns 204)
    let resp = client
        .patch_json(
            &format!("/api/v1/admin/iptv/providers/{provider_id}"),
            &json!({ "name": "Updated Provider" }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify update
    let resp = client
        .get(&format!("/api/v1/admin/iptv/providers/{provider_id}"))
        .await;
    resp.assert_ok();
    let provider: Value = resp.json();
    assert_eq!(provider["name"], "Updated Provider");

    // Delete
    let resp = client
        .delete(&format!("/api/v1/admin/iptv/providers/{provider_id}"))
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);
}

// =============================================================================
// Groups
// =============================================================================

#[tokio::test]
async fn test_iptv_groups_list() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    let resp = client.get("/api/v1/admin/iptv/groups").await;
    resp.assert_ok();
    let groups: Vec<Value> = resp.json();
    // Should have pre-seeded groups
    assert!(groups.len() >= 7, "expected at least 7 pre-seeded groups");
}

// =============================================================================
// Channels (empty state)
// =============================================================================

#[tokio::test]
async fn test_iptv_channels_empty() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    let resp = client.get("/api/v1/admin/iptv/channels").await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(body["total"], 0);
}

// =============================================================================
// Favourites
// =============================================================================

#[tokio::test]
async fn test_iptv_favourites_empty() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    let resp = client.get("/api/v1/admin/iptv/favourites").await;
    resp.assert_ok();
    let favs: Vec<Value> = resp.json();
    assert!(favs.is_empty());
}

// =============================================================================
// Watch History (empty state)
// =============================================================================

#[tokio::test]
async fn test_iptv_history_empty() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    let resp = client.get("/api/v1/admin/iptv/history").await;
    resp.assert_ok();
    let history: Vec<Value> = resp.json();
    assert!(history.is_empty());
}

// =============================================================================
// Recordings
// =============================================================================

#[tokio::test]
async fn test_iptv_recordings_empty() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    let resp = client.get("/api/v1/admin/iptv/recordings").await;
    resp.assert_ok();
    let recordings: Vec<Value> = resp.json();
    assert!(recordings.is_empty());
}

// =============================================================================
// Unmatched Sources (empty state)
// =============================================================================

#[tokio::test]
async fn test_iptv_unmatched_empty() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    let resp = client.get("/api/v1/admin/iptv/unmatched").await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(body["total"], 0);
}

// =============================================================================
// Normalization unit tests
// =============================================================================

#[test]
fn test_normalize_stream_name() {
    use api::iptv::normalization::normalize_stream_name;

    let result = normalize_stream_name("UK: BBC ONE (FHD)");
    assert_eq!(result.normalized_name, "bbc one");
    assert_eq!(result.quality, "fhd");
    assert_eq!(result.country.as_deref(), Some("GB"));
    assert_eq!(result.language.as_deref(), Some("en"));

    let result = normalize_stream_name("ESPN HD");
    assert_eq!(result.normalized_name, "espn");
    assert_eq!(result.quality, "hd");

    let result = normalize_stream_name("FR: Canal+ 4K");
    assert_eq!(result.normalized_name, "canal plus");
    assert_eq!(result.quality, "4k");
    assert_eq!(result.country.as_deref(), Some("FR"));
    assert_eq!(result.language.as_deref(), Some("fr"));
}

// =============================================================================
// Quality priority
// =============================================================================

#[test]
fn test_quality_priority() {
    use api::iptv::quality::compute_priority;

    assert!(compute_priority("4k") < compute_priority("fhd"));
    assert!(compute_priority("fhd") < compute_priority("hd"));
    assert!(compute_priority("hd") < compute_priority("sd"));
}

// =============================================================================
// Settings
// =============================================================================

#[tokio::test]
async fn test_iptv_settings_fresh_db() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    // GET on fresh DB should return sensible defaults (no crash)
    let resp = client.get("/api/v1/admin/iptv/settings").await;
    resp.assert_ok();
    let settings: Value = resp.json();
    // Should have runtime config defaults even with no iptv_config row
    assert!(settings["auto_create_channels"].is_boolean());

    // PATCH settings — creates the singleton iptv_config row
    let resp = client
        .patch_json(
            "/api/v1/admin/iptv/settings",
            &json!({
                "default_format": "ts",
                "recording_path": "/tmp/recordings",
                "epg_update_interval_secs": 3600,
                "auto_create_channels": true
            }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // GET again — should reflect the updated values
    let resp = client.get("/api/v1/admin/iptv/settings").await;
    resp.assert_ok();
    let settings: Value = resp.json();
    assert_eq!(settings["default_format"], "ts");
    assert_eq!(settings["recording_path"], "/tmp/recordings");
    assert_eq!(settings["epg_update_interval_secs"], 3600);

    // PATCH again with partial update — should COALESCE
    let resp = client
        .patch_json(
            "/api/v1/admin/iptv/settings",
            &json!({ "recording_path": "/mnt/recordings" }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    let resp = client.get("/api/v1/admin/iptv/settings").await;
    resp.assert_ok();
    let settings: Value = resp.json();
    assert_eq!(settings["default_format"], "ts"); // unchanged
    assert_eq!(settings["recording_path"], "/mnt/recordings"); // updated
}

// =============================================================================
// Auth required
// =============================================================================

#[tokio::test]
async fn test_iptv_requires_auth() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);

    let resp = client.get("/api/v1/admin/iptv/providers").await;
    resp.assert_status(StatusCode::UNAUTHORIZED);

    let resp = client.get("/api/v1/admin/iptv/channels").await;
    resp.assert_status(StatusCode::UNAUTHORIZED);

    let resp = client.get("/api/v1/admin/iptv/groups").await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Orphaned channel cleanup on provider deletion
// =============================================================================

#[tokio::test]
async fn test_delete_provider_cleans_orphaned_channels() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    // Create two providers
    let resp = client
        .post_json(
            "/api/v1/admin/iptv/providers",
            &json!({
                "name": "Provider A",
                "url": "http://a.example.com",
                "username": "u",
                "password": "p"
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let provider_a_id: String = resp.json::<Value>()["id"].as_str().unwrap().to_string();

    let resp = client
        .post_json(
            "/api/v1/admin/iptv/providers",
            &json!({
                "name": "Provider B",
                "url": "http://b.example.com",
                "username": "u",
                "password": "p"
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let provider_b_id: String = resp.json::<Value>()["id"].as_str().unwrap().to_string();

    // Insert channels and sources directly
    let chan_only_a = uuid::Uuid::new_v4();
    let chan_shared = uuid::Uuid::new_v4();

    sqlx::query(
        "INSERT INTO iptv_channels (id, name, normalized_name, enabled) VALUES ($1, $2, $3, true), ($4, $5, $6, true)",
    )
    .bind(chan_only_a)
    .bind("Channel Only A")
    .bind("channel only a")
    .bind(chan_shared)
    .bind("Channel Shared")
    .bind("channel shared")
    .execute(&server.pool)
    .await
    .unwrap();

    let pa: uuid::Uuid = provider_a_id.parse().unwrap();
    let pb: uuid::Uuid = provider_b_id.parse().unwrap();

    // chan_only_a: source only from provider A
    sqlx::query(
        "INSERT INTO iptv_sources (id, channel_id, provider_id, remote_stream_id, stream_type) VALUES ($1, $2, $3, 1, 'live')",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(chan_only_a)
    .bind(pa)
    .execute(&server.pool)
    .await
    .unwrap();

    // chan_shared: sources from both providers
    sqlx::query(
        "INSERT INTO iptv_sources (id, channel_id, provider_id, remote_stream_id, stream_type) VALUES ($1, $2, $3, 2, 'live'), ($4, $5, $6, 3, 'live')",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(chan_shared)
    .bind(pa)
    .bind(uuid::Uuid::new_v4())
    .bind(chan_shared)
    .bind(pb)
    .execute(&server.pool)
    .await
    .unwrap();

    // Delete provider A
    let resp = client
        .delete(&format!("/api/v1/admin/iptv/providers/{provider_a_id}"))
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // chan_only_a should be deleted (orphaned — no remaining sources)
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM iptv_channels WHERE id = $1")
        .bind(chan_only_a)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(count.0, 0, "orphaned channel should be deleted");

    // chan_shared should still exist (has source from provider B)
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM iptv_channels WHERE id = $1")
        .bind(chan_shared)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(count.0, 1, "shared channel should survive");
}

// =============================================================================
// EPG grid with channel_ids filter
// =============================================================================

#[tokio::test]
async fn test_iptv_epg_grid_with_channel_ids() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    let ch1 = uuid::Uuid::new_v4();
    let ch2 = uuid::Uuid::new_v4();
    let ch3 = uuid::Uuid::new_v4();

    // Insert 3 channels
    sqlx::query(
        "INSERT INTO iptv_channels (id, name, normalized_name, enabled) VALUES ($1, $2, $3, true), ($4, $5, $6, true), ($7, $8, $9, true)",
    )
    .bind(ch1).bind("Channel Alpha").bind("channel alpha")
    .bind(ch2).bind("Channel Beta").bind("channel beta")
    .bind(ch3).bind("Channel Gamma").bind("channel gamma")
    .execute(&server.pool)
    .await
    .unwrap();

    let now = time::OffsetDateTime::now_utc();
    let later = now + time::Duration::hours(1);

    // Insert EPG entries for each channel
    sqlx::query(
        "INSERT INTO iptv_epg (id, channel_id, title, start_time, stop_time, categories) VALUES ($1, $2, $3, $4, $5, '{}'::text[]), ($6, $7, $8, $9, $10, '{}'::text[]), ($11, $12, $13, $14, $15, '{}'::text[])",
    )
    .bind(uuid::Uuid::new_v4()).bind(ch1).bind("Show A").bind(now).bind(later)
    .bind(uuid::Uuid::new_v4()).bind(ch2).bind("Show B").bind(now).bind(later)
    .bind(uuid::Uuid::new_v4()).bind(ch3).bind("Show C").bind(now).bind(later)
    .execute(&server.pool)
    .await
    .unwrap();

    // Request EPG grid for only 2 of 3 channels
    let resp = client
        .get(&format!(
            "/api/v1/admin/iptv/epg?channel_ids={},{}",
            ch1, ch2
        ))
        .await;
    resp.assert_ok();
    let body: Value = resp.json();

    let grid = body["grid"].as_array().unwrap();
    assert_eq!(grid.len(), 2, "should only contain 2 requested channels");

    let grid_channel_ids: Vec<&str> = grid
        .iter()
        .map(|entry| entry["channel"]["id"].as_str().unwrap())
        .collect();
    assert!(grid_channel_ids.contains(&ch1.to_string().as_str()));
    assert!(grid_channel_ids.contains(&ch2.to_string().as_str()));
    assert!(!grid_channel_ids.contains(&ch3.to_string().as_str()));

    // Response should have from and to fields (serialized as time values)
    assert!(!body["from"].is_null(), "response should have 'from' field");
    assert!(!body["to"].is_null(), "response should have 'to' field");
}

// =============================================================================
// EPG grid default limit (empty DB)
// =============================================================================

#[tokio::test]
async fn test_iptv_epg_grid_default_limit() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    // Call EPG with no channel_ids on empty DB — should not crash
    let resp = client.get("/api/v1/admin/iptv/epg").await;
    resp.assert_ok();
    let body: Value = resp.json();
    let grid = body["grid"].as_array().unwrap();
    assert!(grid.is_empty(), "empty DB should return empty grid");
}

// =============================================================================
// Favourites EPG endpoint
// =============================================================================

#[tokio::test]
async fn test_iptv_epg_favourites() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    let ch = uuid::Uuid::new_v4();

    // Insert a channel
    sqlx::query(
        "INSERT INTO iptv_channels (id, name, normalized_name, enabled) VALUES ($1, $2, $3, true)",
    )
    .bind(ch)
    .bind("Fav Channel")
    .bind("fav channel")
    .execute(&server.pool)
    .await
    .unwrap();

    // Get admin user ID and add channel as favourite
    let user_id: (uuid::Uuid,) = sqlx::query_as("SELECT id FROM users LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO iptv_favourites (user_id, channel_id, display_order) VALUES ($1, $2, 0)",
    )
    .bind(user_id.0)
    .bind(ch)
    .execute(&server.pool)
    .await
    .unwrap();

    // Insert EPG entries for the channel
    let now = time::OffsetDateTime::now_utc();
    let later = now + time::Duration::hours(1);

    sqlx::query(
        "INSERT INTO iptv_epg (id, channel_id, title, start_time, stop_time, categories) VALUES ($1, $2, $3, $4, $5, '{}'::text[])",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(ch)
    .bind("Favourite Show")
    .bind(now)
    .bind(later)
    .execute(&server.pool)
    .await
    .unwrap();

    // Call favourites EPG
    let resp = client.get("/api/v1/admin/iptv/epg/favourites").await;
    resp.assert_ok();
    let body: Value = resp.json();

    let grid = body["grid"].as_array().unwrap();
    assert_eq!(grid.len(), 1, "should contain the favourite channel");
    assert_eq!(grid[0]["channel"]["id"].as_str().unwrap(), ch.to_string());
    assert_eq!(grid[0]["channel"]["name"], "Fav Channel");

    // has_catchup field should be present on channel
    assert!(
        grid[0]["channel"]["has_catchup"].is_boolean(),
        "channel should have has_catchup field"
    );

    // Programs should be present
    let programs = grid[0]["programs"].as_array().unwrap();
    assert!(!programs.is_empty(), "should have EPG programs");
}

// =============================================================================
// Group favourites CRUD
// =============================================================================

#[tokio::test]
async fn test_iptv_group_favourites_crud() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    // Get groups list (should have pre-seeded groups)
    let resp = client.get("/api/v1/admin/iptv/groups").await;
    resp.assert_ok();
    let groups: Vec<Value> = resp.json();
    assert!(!groups.is_empty(), "should have pre-seeded groups");

    let group_id = groups[0]["id"].as_str().unwrap();

    // Add group as favourite
    let resp = client
        .post(&format!("/api/v1/admin/iptv/favourites/groups/{group_id}"))
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // List favourite groups — should contain the group
    let resp = client.get("/api/v1/admin/iptv/favourites/groups").await;
    resp.assert_ok();
    let fav_groups: Vec<Value> = resp.json();
    let fav_ids: Vec<&str> = fav_groups.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        fav_ids.contains(&group_id),
        "favourite groups should contain the added group"
    );

    // Remove group favourite
    let resp = client
        .delete(&format!("/api/v1/admin/iptv/favourites/groups/{group_id}"))
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // List favourite groups — should be empty
    let resp = client.get("/api/v1/admin/iptv/favourites/groups").await;
    resp.assert_ok();
    let fav_groups: Vec<Value> = resp.json();
    assert!(
        fav_groups.is_empty(),
        "favourite groups should be empty after removal"
    );
}

// =============================================================================
// Watch match deduplication
// =============================================================================

#[tokio::test]
async fn test_iptv_watch_match_dedup() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    let ch1 = uuid::Uuid::new_v4();
    let ch2 = uuid::Uuid::new_v4();

    // Insert 2 channels
    sqlx::query(
        "INSERT INTO iptv_channels (id, name, normalized_name, enabled) VALUES ($1, $2, $3, true), ($4, $5, $6, true)",
    )
    .bind(ch1).bind("Sports Channel 1").bind("sports channel 1")
    .bind(ch2).bind("Sports Channel 2").bind("sports channel 2")
    .execute(&server.pool)
    .await
    .unwrap();

    // Add both as favourites
    let user_id: (uuid::Uuid,) = sqlx::query_as("SELECT id FROM users LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap();

    sqlx::query("INSERT INTO iptv_favourites (user_id, channel_id, display_order) VALUES ($1, $2, 0), ($3, $4, 1)")
        .bind(user_id.0).bind(ch1)
        .bind(user_id.0).bind(ch2)
        .execute(&server.pool)
        .await
        .unwrap();

    // Insert a programme watch pattern
    sqlx::query(
        "INSERT INTO iptv_programme_watches (id, user_id, title_pattern, match_mode, enabled) VALUES ($1, $2, $3, 'contains', true)",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(user_id.0)
    .bind("MotoGP")
    .execute(&server.pool)
    .await
    .unwrap();

    // Insert EPG entries with same title "MotoGP" on BOTH channels at the same time
    let now = time::OffsetDateTime::now_utc();
    let later = now + time::Duration::minutes(30);

    sqlx::query(
        "INSERT INTO iptv_epg (id, channel_id, title, start_time, stop_time, categories) VALUES ($1, $2, $3, $4, $5, '{}'::text[]), ($6, $7, $8, $9, $10, '{}'::text[])",
    )
    .bind(uuid::Uuid::new_v4()).bind(ch1).bind("MotoGP Race").bind(now).bind(later)
    .bind(uuid::Uuid::new_v4()).bind(ch2).bind("MotoGP Race").bind(now).bind(later)
    .execute(&server.pool)
    .await
    .unwrap();

    // Get watch matches — should be deduplicated
    let resp = client.get("/api/v1/admin/iptv/watches/matches").await;
    resp.assert_ok();
    let matches: Vec<Value> = resp.json();
    assert_eq!(
        matches.len(),
        1,
        "same programme on two channels should be deduplicated to 1 match"
    );
}

// =============================================================================
// Merge channels
// =============================================================================

#[tokio::test]
async fn test_iptv_merge_channels() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    // Create a provider first
    let resp = client
        .post_json(
            "/api/v1/admin/iptv/providers",
            &json!({
                "name": "Merge Test Provider",
                "url": "http://merge.example.com",
                "username": "u",
                "password": "p"
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let provider_id: uuid::Uuid = resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let source_ch = uuid::Uuid::new_v4();
    let target_ch = uuid::Uuid::new_v4();

    // Insert 2 channels
    sqlx::query(
        "INSERT INTO iptv_channels (id, name, normalized_name, enabled) VALUES ($1, $2, $3, true), ($4, $5, $6, true)",
    )
    .bind(source_ch).bind("Source Channel").bind("source channel")
    .bind(target_ch).bind("Target Channel").bind("target channel")
    .execute(&server.pool)
    .await
    .unwrap();

    // Insert sources for both channels
    sqlx::query(
        "INSERT INTO iptv_sources (id, channel_id, provider_id, remote_stream_id, stream_type) VALUES ($1, $2, $3, 100, 'live'), ($4, $5, $6, 200, 'live')",
    )
    .bind(uuid::Uuid::new_v4()).bind(source_ch).bind(provider_id)
    .bind(uuid::Uuid::new_v4()).bind(target_ch).bind(provider_id)
    .execute(&server.pool)
    .await
    .unwrap();

    // Merge source into target
    let resp = client
        .post_json(
            "/api/v1/admin/iptv/channels/merge",
            &json!({
                "source_id": source_ch,
                "target_id": target_ch
            }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Source channel should be deleted
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM iptv_channels WHERE id = $1")
        .bind(source_ch)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(count.0, 0, "source channel should be deleted after merge");

    // Target channel should still exist
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM iptv_channels WHERE id = $1")
        .bind(target_ch)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(count.0, 1, "target channel should still exist after merge");

    // All sources should now belong to target channel
    let source_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM iptv_sources WHERE channel_id = $1")
            .bind(target_ch)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert_eq!(
        source_count.0, 2,
        "target channel should have all sources after merge"
    );
}

// =============================================================================
// Settings with epg_retention_days
// =============================================================================

#[tokio::test]
async fn test_iptv_settings_epg_retention() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    // PATCH settings with epg_retention_days
    let resp = client
        .patch_json(
            "/api/v1/admin/iptv/settings",
            &json!({ "epg_retention_days": 14 }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // GET settings and verify
    let resp = client.get("/api/v1/admin/iptv/settings").await;
    resp.assert_ok();
    let settings: Value = resp.json();
    assert_eq!(
        settings["epg_retention_days"], 14,
        "epg_retention_days should be 14"
    );
}

// =============================================================================
// Provider name on sources
// =============================================================================

#[tokio::test]
async fn test_iptv_source_provider_name() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    // Create a provider
    let resp = client
        .post_json(
            "/api/v1/admin/iptv/providers",
            &json!({
                "name": "Source Test Provider",
                "url": "http://source.example.com",
                "username": "u",
                "password": "p"
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let provider_id: uuid::Uuid = resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let ch = uuid::Uuid::new_v4();

    // Insert channel and source
    sqlx::query(
        "INSERT INTO iptv_channels (id, name, normalized_name, enabled) VALUES ($1, $2, $3, true)",
    )
    .bind(ch)
    .bind("Provider Name Test Channel")
    .bind("provider name test channel")
    .execute(&server.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO iptv_sources (id, channel_id, provider_id, remote_stream_id, stream_type) VALUES ($1, $2, $3, 42, 'live')",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(ch)
    .bind(provider_id)
    .execute(&server.pool)
    .await
    .unwrap();

    // Get channel sources
    let resp = client
        .get(&format!("/api/v1/admin/iptv/channels/{ch}/sources"))
        .await;
    resp.assert_ok();
    let sources: Vec<Value> = resp.json();
    assert_eq!(sources.len(), 1);
    assert_eq!(
        sources[0]["provider_name"], "Source Test Provider",
        "source should include provider_name"
    );
}

// =============================================================================
// derive_category unit tests
// =============================================================================

#[test]
fn test_derive_category() {
    use api::iptv::matching::derive_category;

    // 24/7 prefix
    assert_eq!(derive_category("24/7: SEINFELD", None), Some("24/7".into()));

    // RAW marker → vod
    assert_eq!(derive_category("UK: BBC ONE ᴿᴬᵂ", None), Some("vod".into()));

    // PPV prefix
    assert_eq!(derive_category("PPV: UFC 300", None), Some("ppv".into()));

    // Remote category mapping: sports
    assert_eq!(
        derive_category("Sky Sports F1", Some("UK Sports")),
        Some("sports".into())
    );

    // No match with no remote category
    assert_eq!(derive_category("Random Channel", None), None);
}

// =============================================================================
// Recording path config is used by repo function
// =============================================================================

#[tokio::test]
async fn test_recording_path_used_from_config() {
    use api::repositories::iptv_recordings;

    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    // Before any config exists, recording path should be None
    let path = iptv_recordings::get_recording_path(&server.pool)
        .await
        .unwrap();
    assert_eq!(path, None, "no config row yet — should be None");

    // Set recording_path via settings API
    let resp = client
        .patch_json(
            "/api/v1/admin/iptv/settings",
            &json!({ "recording_path": "/mnt/media/recordings" }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Now the repo function should return the configured path
    let path = iptv_recordings::get_recording_path(&server.pool)
        .await
        .unwrap();
    assert_eq!(
        path.as_deref(),
        Some("/mnt/media/recordings"),
        "recording path should come from iptv_config"
    );

    // Update to a different path
    let resp = client
        .patch_json(
            "/api/v1/admin/iptv/settings",
            &json!({ "recording_path": "/data/iptv/recorded" }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    let path = iptv_recordings::get_recording_path(&server.pool)
        .await
        .unwrap();
    assert_eq!(
        path.as_deref(),
        Some("/data/iptv/recorded"),
        "recording path should update"
    );
}
