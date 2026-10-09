use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

/// Create a person directly in the DB. Returns the person ID.
async fn create_person(server: &TestServer, name: &str) -> String {
    let id = uuid::Uuid::now_v7();
    sqlx::query(
        r#"INSERT INTO people (id, name, sort_name, identifiers) VALUES ($1, $2, $3, '{}'::jsonb)"#,
    )
    .bind(id)
    .bind(name)
    .bind(name)
    .execute(&server.pool)
    .await
    .expect("Failed to create person");
    id.to_string()
}

/// Create a movie and link a person as cast member.
async fn create_movie_with_cast(server: &TestServer, title: &str, person_id: &str) -> String {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({ "title": title, "year": 2024 }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let movie_id = resp.json::<Value>()["id"].as_str().unwrap().to_string();

    sqlx::query(
        r#"INSERT INTO movie_cast (id, movie_id, person_id, character_name, cast_order)
           VALUES ($1, $2::uuid, $3::uuid, 'Character', 1)"#,
    )
    .bind(uuid::Uuid::now_v7())
    .bind(uuid::Uuid::parse_str(&movie_id).unwrap())
    .bind(uuid::Uuid::parse_str(person_id).unwrap())
    .execute(&server.pool)
    .await
    .expect("Failed to create movie cast");

    movie_id
}

/// Give a movie a file, which is what "in the library" actually means.
async fn give_movie_a_file(server: &TestServer, movie_id: &str) {
    sqlx::query(
        r#"INSERT INTO movie_files (id, movie_id, file_path, format, available)
           VALUES ($1, $2::uuid, '/m.mkv', 'mkv', TRUE)"#,
    )
    .bind(uuid::Uuid::now_v7())
    .bind(uuid::Uuid::parse_str(movie_id).unwrap())
    .execute(&server.pool)
    .await
    .expect("Failed to create movie file");
}

/// Create a show and link a person as cast. No episodes, so nothing is held.
async fn create_show_with_cast(server: &TestServer, title: &str, person_id: &str) -> String {
    let show_id = uuid::Uuid::now_v7();
    sqlx::query(
        r#"INSERT INTO tv_shows (id, title, year, identifiers)
           VALUES ($1, $2, 2024, '{}'::jsonb)"#,
    )
    .bind(show_id)
    .bind(title)
    .execute(&server.pool)
    .await
    .expect("Failed to create show");

    sqlx::query(
        r#"INSERT INTO tv_cast (id, show_id, person_id, character_name, cast_order)
           VALUES ($1, $2, $3::uuid, 'Character', 1)"#,
    )
    .bind(uuid::Uuid::now_v7())
    .bind(show_id)
    .bind(uuid::Uuid::parse_str(person_id).unwrap())
    .execute(&server.pool)
    .await
    .expect("Failed to create tv cast");

    show_id.to_string()
}

// =============================================================================
// Auth
// =============================================================================

#[tokio::test]
async fn people_endpoint_requires_auth() {
    let server = TestServer::start_api_only().await;
    let person_id = create_person(&server, "Test Person").await;

    let client = TestClient::new(&server.api_url);
    let resp = client
        .get(&format!("/api/v1/admin/people/{person_id}"))
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Get Person
// =============================================================================

#[tokio::test]
async fn get_person_not_found() {
    let server = TestServer::start_api_only().await;
    let fake_id = uuid::Uuid::now_v7();

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/people/{fake_id}"))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_person_returns_details() {
    let server = TestServer::start_api_only().await;
    let person_id = create_person(&server, "Jane Doe").await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/people/{person_id}"))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    assert_eq!(json["id"], person_id);
    assert_eq!(json["name"], "Jane Doe");
    assert!(json.get("filmography").is_none());
}

// =============================================================================
// Filmography
// =============================================================================

#[tokio::test]
async fn filmography_returns_library_credits_without_tmdb_id() {
    let server = TestServer::start_api_only().await;
    let person_id = create_person(&server, "John Actor").await;
    let movie_id = create_movie_with_cast(&server, "Test Movie", &person_id).await;
    give_movie_a_file(&server, &movie_id).await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/people/{person_id}/filmography"))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let credits = json["credits"].as_array().unwrap();
    assert_eq!(credits.len(), 1);
    assert_eq!(credits[0]["title"], "Test Movie");
    assert_eq!(credits[0]["media_type"], "movie");
    assert_eq!(credits[0]["credit_type"], "cast");
    assert_eq!(credits[0]["in_library"], true);
    assert_eq!(credits[0]["character"], "Character");
}

/// Kobbler holds metadata for far more films than it has files for, and the page
/// used to mark every one of them as held: the flag was computed and then thrown
/// away for a hardcoded `true`. It drives the "in library" counts, the click
/// through, and which credits are offered as wantable — so everything looked
/// owned and nothing could be added.
#[tokio::test]
async fn a_credit_with_no_file_is_not_in_the_library() {
    let server = TestServer::start_api_only().await;
    let person_id = create_person(&server, "John Actor").await;
    create_movie_with_cast(&server, "Metadata Only", &person_id).await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/people/{person_id}/filmography"))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let credits = json["credits"].as_array().unwrap();
    assert_eq!(credits.len(), 1);
    assert_eq!(credits[0]["title"], "Metadata Only");
    assert_eq!(
        credits[0]["in_library"], false,
        "a movie with no file is not in the library"
    );
}

#[tokio::test]
async fn a_movie_credit_with_a_file_is_in_the_library() {
    let server = TestServer::start_api_only().await;
    let person_id = create_person(&server, "John Actor").await;
    let held = create_movie_with_cast(&server, "Held Film", &person_id).await;
    give_movie_a_file(&server, &held).await;
    create_movie_with_cast(&server, "Unheld Film", &person_id).await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/people/{person_id}/filmography"))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let credits = json["credits"].as_array().unwrap();
    let held_row = credits
        .iter()
        .find(|c| c["title"] == "Held Film")
        .expect("held film present");
    let unheld_row = credits
        .iter()
        .find(|c| c["title"] == "Unheld Film")
        .expect("unheld film present");
    assert_eq!(held_row["in_library"], true);
    assert_eq!(unheld_row["in_library"], false);
}

/// The same lie the chat bot told about Dark Matter: a show row is metadata, and
/// holding a show means holding episodes of it. The TV query did not even compute
/// availability — it hardcoded `true` on the strength of the row existing.
#[tokio::test]
async fn a_show_with_no_episodes_is_not_in_the_library() {
    let server = TestServer::start_api_only().await;
    let person_id = create_person(&server, "John Actor").await;
    create_show_with_cast(&server, "Metadata Only Show", &person_id).await;

    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/people/{person_id}/filmography"))
        .await;
    resp.assert_ok();

    let json: Value = resp.json();
    let credits = json["credits"].as_array().unwrap();
    let show = credits
        .iter()
        .find(|c| c["title"] == "Metadata Only Show")
        .expect("show credit present");
    assert_eq!(
        show["in_library"], false,
        "a show with no available episodes is not in the library"
    );
}
