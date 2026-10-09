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

/// Create a person via the admin API. Returns the person ID.
async fn create_person(server: &TestServer, name: &str) -> String {
    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/people", &json!({ "name": name }))
        .await;
    resp.assert_status(StatusCode::CREATED);
    resp.json::<Value>()["id"].as_str().unwrap().to_string()
}

/// Create a movie via the admin API. Returns the movie ID.
async fn create_movie(server: &TestServer, title: &str) -> String {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": title, "year": 2024 }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    resp.json::<Value>()["id"].as_str().unwrap().to_string()
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
async fn create_season(server: &TestServer, show_id: &str, num: i32) -> String {
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
async fn create_episode(server: &TestServer, season_id: &str, num: i32, title: &str) -> String {
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

// =============================================================================
// People List / Search
// =============================================================================

#[tokio::test]
async fn test_list_people_empty() {
    let server = TestServer::start_api_only().await;
    let resp = server.admin_client().get("/api/v1/admin/people").await;
    resp.assert_ok();
    let json: Value = resp.json();
    assert!(json["items"].as_array().unwrap().is_empty());
    assert_eq!(json["total_count"], 0);
}

#[tokio::test]
async fn test_list_people_after_create() {
    let server = TestServer::start_api_only().await;
    let person_id = create_person(&server, "Ada Lovelace").await;

    let resp = server.admin_client().get("/api/v1/admin/people").await;
    resp.assert_ok();
    let json = resp.json::<Value>();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], person_id);
    assert_eq!(items[0]["name"], "Ada Lovelace");
    assert_eq!(json["total_count"], 1);
}

#[tokio::test]
async fn test_list_people_with_query() {
    let server = TestServer::start_api_only().await;
    create_person(&server, "Alice Johnson").await;
    create_person(&server, "Bob Smith").await;
    create_person(&server, "Alice Cooper").await;

    // Search for "Alice" -- stub search backend errors so ILIKE fallback fires
    let resp = server
        .admin_client()
        .get("/api/v1/admin/people?q=Alice")
        .await;
    resp.assert_ok();
    let json = resp.json::<Value>();
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let names: Vec<&str> = items.iter().map(|i| i["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"Alice Johnson"));
    assert!(names.contains(&"Alice Cooper"));
}

#[tokio::test]
async fn test_list_people_requires_auth() {
    let server = TestServer::start_api_only().await;
    let no_auth = server.api_client();

    let resp = no_auth.get("/api/v1/admin/people").await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Movie Cast/Crew PUT
// =============================================================================

#[tokio::test]
async fn test_replace_movie_cast() {
    let server = TestServer::start_api_only().await;
    let movie_id = create_movie(&server, "Cast Test Movie").await;
    let person_id = create_person(&server, "Lead Actor").await;

    // PUT cast
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/movies/{}/cast", movie_id),
            &json!([{
                "person_id": person_id,
                "character_name": "Hero",
                "cast_order": 1
            }]),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Verify via GET movie detail
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}", movie_id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let cast = json["cast"].as_array().unwrap();
    assert_eq!(cast.len(), 1);
    assert_eq!(cast[0]["person_id"], person_id);
    assert_eq!(cast[0]["person_name"], "Lead Actor");
    assert_eq!(cast[0]["character_name"], "Hero");
    assert_eq!(cast[0]["cast_order"], 1);
}

#[tokio::test]
async fn test_replace_movie_crew() {
    let server = TestServer::start_api_only().await;
    let movie_id = create_movie(&server, "Crew Test Movie").await;
    let person_id = create_person(&server, "Director Person").await;

    // PUT crew
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/movies/{}/crew", movie_id),
            &json!([{
                "person_id": person_id,
                "department": "Directing",
                "job": "Director"
            }]),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Verify via GET movie detail
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}", movie_id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let crew = json["crew"].as_array().unwrap();
    assert_eq!(crew.len(), 1);
    assert_eq!(crew[0]["person_id"], person_id);
    assert_eq!(crew[0]["person_name"], "Director Person");
    assert_eq!(crew[0]["department"], "Directing");
    assert_eq!(crew[0]["job"], "Director");
}

#[tokio::test]
async fn test_replace_movie_cast_requires_auth() {
    let server = TestServer::start_api_only().await;
    let no_auth = server.api_client();
    let fake_id = uuid::Uuid::new_v4();

    let resp = no_auth
        .put_json(
            &format!("/api/v1/admin/movies/{}/cast", fake_id),
            &json!([]),
        )
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// TV Show Cast/Crew PUT
// =============================================================================

#[tokio::test]
async fn test_replace_tv_show_cast() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Cast Test Show").await;
    let person_id = create_person(&server, "TV Lead").await;

    // PUT cast
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/shows/{}/cast", show_id),
            &json!([{
                "person_id": person_id,
                "character_name": "Protagonist",
                "cast_order": 1
            }]),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Verify via GET show detail
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{}", show_id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let cast = json["cast"].as_array().unwrap();
    assert_eq!(cast.len(), 1);
    assert_eq!(cast[0]["person_id"], person_id);
    assert_eq!(cast[0]["person_name"], "TV Lead");
    assert_eq!(cast[0]["character_name"], "Protagonist");
    assert_eq!(cast[0]["cast_order"], 1);
}

#[tokio::test]
async fn test_replace_tv_show_crew() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Crew Test Show").await;
    let person_id = create_person(&server, "Showrunner Person").await;

    // PUT crew
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/tv/shows/{}/crew", show_id),
            &json!([{
                "person_id": person_id,
                "department": "Production",
                "job": "Executive Producer"
            }]),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // Verify via GET show detail
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{}", show_id))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let crew = json["crew"].as_array().unwrap();
    assert_eq!(crew.len(), 1);
    assert_eq!(crew[0]["person_id"], person_id);
    assert_eq!(crew[0]["person_name"], "Showrunner Person");
    assert_eq!(crew[0]["department"], "Production");
    assert_eq!(crew[0]["job"], "Executive Producer");
}

// =============================================================================
// Episode Still Image
// =============================================================================

#[tokio::test]
async fn test_upload_episode_still() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "Still Test Show").await;
    let season_id = create_season(&server, &show_id, 1).await;
    let ep_id = create_episode(&server, &season_id, 1, "Still Episode").await;
    let jpeg = minimal_jpeg();

    // Upload still
    server
        .admin_client()
        .post_bytes(
            &format!("/api/v1/admin/tv/episodes/{}/still", ep_id),
            jpeg,
            "image/jpeg",
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // GET still back
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/episodes/{}/still", ep_id))
        .await;
    resp.assert_ok();
    assert!(
        !resp.bytes().is_empty(),
        "Episode still bytes should not be empty"
    );
}

#[tokio::test]
async fn test_get_episode_still_404() {
    let server = TestServer::start_api_only().await;
    let show_id = create_show(&server, "No Still Show").await;
    let season_id = create_season(&server, &show_id, 1).await;
    let ep_id = create_episode(&server, &season_id, 1, "No Still Episode").await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/episodes/{}/still", ep_id))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}
