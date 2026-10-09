use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

// =============================================================================
// Test Helpers
// =============================================================================

/// Minimal valid 1x1 JPEG image.
fn minimal_jpeg() -> Vec<u8> {
    use image::{ImageBuffer, Rgb, codecs::jpeg::JpegEncoder};
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_pixel(1, 1, Rgb([255, 0, 0]));
    let mut buf = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut buf, 85);
    encoder.encode_image(&img).expect("JPEG encode failed");
    buf
}

/// Create a TV show via the admin API. Returns the show ID.
async fn create_show(server: &TestServer, title: &str) -> String {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({ "title": title, "year": 2024 }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    resp.json::<Value>()["id"].as_str().unwrap().to_string()
}

/// Create a season via the admin API. Returns the season ID.
async fn create_season_api(server: &TestServer, show_id: &str, num: i32) -> String {
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/shows/{}/seasons", show_id),
            &json!({
                "season_identifier": num.to_string(),
                "season_sort_order": num,
                "title": format!("Season {}", num)
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    resp.json::<Value>()["id"].as_str().unwrap().to_string()
}

/// Create an episode via the admin API. Returns the episode ID.
async fn create_episode_api(server: &TestServer, season_id: &str, num: i32, title: &str) -> String {
    let resp = server
        .admin_client()
        .post_json(
            &format!("/api/v1/admin/tv/seasons/{}/episodes", season_id),
            &json!({
                "episode_identifier": num.to_string(),
                "episode_sort_order": num,
                "title": title
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    resp.json::<Value>()["id"].as_str().unwrap().to_string()
}

/// Create a person via the admin API. Returns the person ID.
async fn create_person(server: &TestServer, name: &str) -> String {
    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/people", &json!({ "name": name }))
        .await;
    resp.assert_status(StatusCode::CREATED);
    resp.json::<Value>()["id"].as_str().unwrap().to_string()
}

// =============================================================================
// Season CRUD Tests
// =============================================================================

#[tokio::test]
async fn test_create_season_via_api() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Season Test Show").await;
    let season_id = create_season_api(&server, &show_id, 1).await;

    // Verify season appears in show detail
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{}", show_id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let seasons = json["seasons"].as_array().unwrap();
    assert_eq!(seasons.len(), 1);
    assert_eq!(seasons[0]["id"], season_id);
    assert_eq!(seasons[0]["title"], "Season 1");
}

#[tokio::test]
async fn test_update_season() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Update Season Show").await;
    let season_id = create_season_api(&server, &show_id, 1).await;

    // Update the season title
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/tv/seasons/{}", season_id),
            &json!({ "title": "Season One (Updated)" }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Verify the update via show detail
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{}", show_id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let seasons = json["seasons"].as_array().unwrap();
    assert_eq!(seasons[0]["title"], "Season One (Updated)");
}

#[tokio::test]
async fn test_delete_season() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Delete Season Show").await;
    let season_id = create_season_api(&server, &show_id, 1).await;

    // Delete the season
    server
        .admin_client()
        .delete(&format!("/api/v1/admin/tv/seasons/{}", season_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Verify the season is gone from the show
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{}", show_id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert!(json["seasons"].as_array().unwrap().is_empty());
}

// =============================================================================
// Episode CRUD Tests
// =============================================================================

#[tokio::test]
async fn test_create_episode_via_api() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Episode Test Show").await;
    let season_id = create_season_api(&server, &show_id, 1).await;
    let _ep_id = create_episode_api(&server, &season_id, 1, "Pilot").await;

    // Verify episode appears in show detail
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{}", show_id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let episodes = json["seasons"][0]["episodes"].as_array().unwrap();
    assert_eq!(episodes.len(), 1);
    assert_eq!(episodes[0]["title"], "Pilot");
}

#[tokio::test]
async fn test_delete_episode_via_api() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Delete Episode Show").await;
    let season_id = create_season_api(&server, &show_id, 1).await;
    let ep_id = create_episode_api(&server, &season_id, 1, "Doomed Episode").await;

    // Delete the episode
    server
        .admin_client()
        .delete(&format!("/api/v1/admin/tv/episodes/{}", ep_id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Verify the episode is gone from the show
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{}", show_id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert!(
        json["seasons"][0]["episodes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

// =============================================================================
// People CRUD Tests
// =============================================================================

#[tokio::test]
async fn test_create_person() {
    let server = TestServer::start_api_only().await;
    let id = create_person(&server, "John Smith").await;

    // Verify via GET
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/people/{}", id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert_eq!(json["name"], "John Smith");
}

#[tokio::test]
async fn test_update_person() {
    let server = TestServer::start_api_only().await;
    let id = create_person(&server, "Jane Doe").await;

    // Update name and bio
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/people/{}", id),
            &json!({ "name": "Jane Smith", "bio": "An accomplished actor." }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Verify the update
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/people/{}", id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert_eq!(json["name"], "Jane Smith");
    assert_eq!(json["bio"], "An accomplished actor.");
}

#[tokio::test]
async fn test_delete_person() {
    let server = TestServer::start_api_only().await;
    let id = create_person(&server, "Doomed Person").await;

    // Delete
    server
        .admin_client()
        .delete(&format!("/api/v1/admin/people/{}", id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Verify 404
    server
        .admin_client()
        .get(&format!("/api/v1/admin/people/{}", id))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

// =============================================================================
// Season Poster Tests
// =============================================================================

#[tokio::test]
async fn test_season_poster_upload_and_serve() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Poster Season Show").await;
    let season_id = create_season_api(&server, &show_id, 1).await;
    let jpeg = minimal_jpeg();

    // Upload poster
    server
        .admin_client()
        .post_bytes(
            &format!("/api/v1/admin/tv/seasons/{}/poster", season_id),
            jpeg.clone(),
            "image/jpeg",
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // GET poster back
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/seasons/{}/poster", season_id))
        .await;
    resp.assert_ok();
    assert!(
        !resp.bytes().is_empty(),
        "Season poster bytes should not be empty"
    );
}

// =============================================================================
// Person Image Tests
// =============================================================================

#[tokio::test]
async fn test_person_image_upload_and_serve() {
    let server = TestServer::start_api_only().await;
    let id = create_person(&server, "Photo Person").await;
    let jpeg = minimal_jpeg();

    // Upload image
    server
        .admin_client()
        .post_bytes(
            &format!("/api/v1/admin/people/{}/image", id),
            jpeg.clone(),
            "image/jpeg",
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // GET image back
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/people/{}/image", id))
        .await;
    resp.assert_ok();
    assert!(
        !resp.bytes().is_empty(),
        "Person image bytes should not be empty"
    );
}

// =============================================================================
// Auth Required Tests
// =============================================================================

#[tokio::test]
async fn test_new_endpoints_require_auth() {
    let server = TestServer::start_api_only().await;
    let no_auth = server.api_client();
    let fake_id = uuid::Uuid::new_v4();

    let endpoints: Vec<(&str, String)> = vec![
        (
            "POST",
            format!("/api/v1/admin/tv/shows/{}/seasons", fake_id),
        ),
        ("PATCH", format!("/api/v1/admin/tv/seasons/{}", fake_id)),
        ("DELETE", format!("/api/v1/admin/tv/seasons/{}", fake_id)),
        (
            "POST",
            format!("/api/v1/admin/tv/seasons/{}/episodes", fake_id),
        ),
        ("DELETE", format!("/api/v1/admin/tv/episodes/{}", fake_id)),
        ("POST", "/api/v1/admin/people".to_string()),
        ("PATCH", format!("/api/v1/admin/people/{}", fake_id)),
        ("DELETE", format!("/api/v1/admin/people/{}", fake_id)),
    ];

    for (method, path) in &endpoints {
        let resp = match *method {
            "POST" => no_auth.post_json(path, &json!({})).await,
            "PATCH" => no_auth.patch_json(path, &json!({})).await,
            "DELETE" => no_auth.delete(path).await,
            _ => unreachable!(),
        };
        assert_eq!(
            resp.status,
            StatusCode::UNAUTHORIZED,
            "{} {} should require auth but got {}",
            method,
            path,
            resp.status
        );
    }
}
