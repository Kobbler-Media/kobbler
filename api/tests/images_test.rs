use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

/// Minimal valid 1x1 JPEG image.
/// Generated using the `image` crate's JPEG encoder at compile time equivalent.
fn minimal_jpeg() -> Vec<u8> {
    use image::{ImageBuffer, Rgb, codecs::jpeg::JpegEncoder};
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_pixel(1, 1, Rgb([255, 0, 0]));
    let mut buf = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut buf, 85);
    encoder.encode_image(&img).expect("JPEG encode failed");
    buf
}

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

// =============================================================================
// Movie Poster Tests
// =============================================================================

#[tokio::test]
async fn test_upload_and_get_movie_poster() {
    let server = TestServer::start_api_only().await;
    let id = create_movie(&server, "Poster Movie").await;
    let jpeg = minimal_jpeg();

    // Upload poster
    server
        .admin_client()
        .post_bytes(
            &format!("/api/v1/admin/movies/{}/poster", id),
            jpeg.clone(),
            "image/jpeg",
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // GET poster back
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}/poster", id))
        .await;
    resp.assert_ok();
    assert!(!resp.bytes().is_empty(), "Poster bytes should not be empty");
}

// =============================================================================
// Movie Backdrop Tests
// =============================================================================

#[tokio::test]
async fn test_upload_and_get_movie_backdrop() {
    let server = TestServer::start_api_only().await;
    let id = create_movie(&server, "Backdrop Movie").await;
    let jpeg = minimal_jpeg();

    // Upload backdrop
    server
        .admin_client()
        .post_bytes(
            &format!("/api/v1/admin/movies/{}/backdrop", id),
            jpeg.clone(),
            "image/jpeg",
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // GET backdrop back
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}/backdrop", id))
        .await;
    resp.assert_ok();
    assert!(
        !resp.bytes().is_empty(),
        "Backdrop bytes should not be empty"
    );
}

// =============================================================================
// TV Show Poster Tests
// =============================================================================

#[tokio::test]
async fn test_upload_and_get_show_poster() {
    let server = TestServer::start_api_only().await;
    let id = create_show(&server, "Poster Show").await;
    let jpeg = minimal_jpeg();

    // Upload poster
    server
        .admin_client()
        .post_bytes(
            &format!("/api/v1/admin/tv/shows/{}/poster", id),
            jpeg.clone(),
            "image/jpeg",
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // GET poster back
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{}/poster", id))
        .await;
    resp.assert_ok();
    assert!(!resp.bytes().is_empty(), "Poster bytes should not be empty");
}

// =============================================================================
// TV Show Backdrop Tests
// =============================================================================

#[tokio::test]
async fn test_upload_and_get_show_backdrop() {
    let server = TestServer::start_api_only().await;
    let id = create_show(&server, "Backdrop Show").await;
    let jpeg = minimal_jpeg();

    // Upload backdrop
    server
        .admin_client()
        .post_bytes(
            &format!("/api/v1/admin/tv/shows/{}/backdrop", id),
            jpeg.clone(),
            "image/jpeg",
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // GET backdrop back
    let resp = server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{}/backdrop", id))
        .await;
    resp.assert_ok();
    assert!(
        !resp.bytes().is_empty(),
        "Backdrop bytes should not be empty"
    );
}

// =============================================================================
// 404 When No Image Available
// =============================================================================

#[tokio::test]
async fn test_get_movie_poster_returns_404_when_none() {
    let server = TestServer::start_api_only().await;
    let id = create_movie(&server, "No Poster Movie").await;

    server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}/poster", id))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_get_movie_backdrop_returns_404_when_none() {
    let server = TestServer::start_api_only().await;
    let id = create_movie(&server, "No Backdrop Movie").await;

    server
        .admin_client()
        .get(&format!("/api/v1/admin/movies/{}/backdrop", id))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_get_show_poster_returns_404_when_none() {
    let server = TestServer::start_api_only().await;
    let id = create_show(&server, "No Poster Show").await;

    server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{}/poster", id))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_get_show_backdrop_returns_404_when_none() {
    let server = TestServer::start_api_only().await;
    let id = create_show(&server, "No Backdrop Show").await;

    server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/shows/{}/backdrop", id))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

// =============================================================================
// Auth Required
// =============================================================================

// Reading artwork is public and writing it is not. The id is the whole credential
// on the read side, and it only comes from a listing that did require auth —
// which is what lets Media3 fetch the now-playing bitmap, having no way to send a
// header of its own.
#[tokio::test]
async fn test_image_reads_are_public_and_writes_are_not() {
    let server = TestServer::start_api_only().await;
    let id = create_movie(&server, "Auth Movie").await;
    let show_id = create_show(&server, "Auth Show").await;
    let no_auth = server.api_client();

    let endpoints = [
        format!("/api/v1/admin/movies/{}/poster", id),
        format!("/api/v1/admin/movies/{}/backdrop", id),
        format!("/api/v1/admin/tv/shows/{}/poster", show_id),
        format!("/api/v1/admin/tv/shows/{}/backdrop", show_id),
    ];

    for path in &endpoints {
        // Reaches the handler, which has no artwork to hand back for these.
        let resp = no_auth.get(path).await;
        assert_eq!(
            resp.status,
            StatusCode::NOT_FOUND,
            "GET {} should be served without auth but got {}",
            path,
            resp.status
        );

        let resp = no_auth.post_bytes(path, vec![0u8; 10], "image/jpeg").await;
        assert_eq!(
            resp.status,
            StatusCode::UNAUTHORIZED,
            "POST {} should require auth but got {}",
            path,
            resp.status
        );
    }
}

// =============================================================================
// Legacy backdrop sizes
// =============================================================================

#[tokio::test]
async fn legacy_w780_backdrops_are_refetched_at_w1280() {
    let server = TestServer::start_api_only().await;
    let movie = uuid::Uuid::parse_str(&create_movie(&server, "Small Backdrop").await).unwrap();
    let show = uuid::Uuid::parse_str(&create_show(&server, "Small Backdrop Show").await).unwrap();
    let legacy = json!({
        "poster": {"source_url": "https://image.tmdb.org/t/p/w780/p.jpg", "cached_at": "2026-01-01T00:00:00Z"},
        "backdrop": {"source_url": "https://image.tmdb.org/t/p/w780/b.jpg", "cached_at": "2026-01-01T00:00:00Z"}
    });
    for table in ["movies", "tv_shows"] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET images = $1"
        )))
        .bind(&legacy)
        .execute(&server.pool)
        .await
        .unwrap();
    }

    sqlx::raw_sql(include_str!("../migrations/0182_full_size_backdrops.sql"))
        .execute(&server.pool)
        .await
        .unwrap();

    for (table, id) in [("movies", movie), ("tv_shows", show)] {
        let images: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT images FROM {table} WHERE id = $1"
        )))
        .bind(id)
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert_eq!(
            images["backdrop"],
            json!({"source_url": "https://image.tmdb.org/t/p/w1280/b.jpg"}),
            "{table}: backdrop must point at w1280 and drop cached_at so the stale file is replaced"
        );
        assert_eq!(
            images["poster"], legacy["poster"],
            "{table}: posters are untouched"
        );
    }
}

/// Enrichment puts the poster and backdrop on disk up front, so the first viewer to scroll
/// past a title is not the one who waits on TMDB.
#[tokio::test]
async fn warming_caches_poster_and_backdrop_once() {
    use api::images::{ImageType, cached_image_exists, warm_entity_images};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    for file in ["/p.jpg", "/b.jpg"] {
        Mock::given(method("GET"))
            .and(path(file))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(minimal_jpeg()))
            .expect(2)
            .mount(&mock)
            .await;
    }
    let images = json!({
        "poster": {"source_url": format!("{}/p.jpg", mock.uri())},
        "backdrop": {"source_url": format!("{}/b.jpg", mock.uri())}
    });
    let movie = uuid::Uuid::parse_str(&create_movie(&server, "Warm Me").await).unwrap();
    let show = uuid::Uuid::parse_str(&create_show(&server, "Warm Me Too").await).unwrap();
    let storage = server.storage_root();

    for (table, id, poster, backdrop) in [
        (
            "movies",
            movie,
            ImageType::MoviePoster,
            ImageType::MovieBackdrop,
        ),
        (
            "tv_shows",
            show,
            ImageType::TvShowPoster,
            ImageType::TvShowBackdrop,
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET images = $1 WHERE id = $2"
        )))
        .bind(&images)
        .bind(id)
        .execute(&server.pool)
        .await
        .unwrap();
        let keys = [("poster", poster), ("backdrop", backdrop)];
        warm_entity_images(&server.pool, &storage, table, id, &keys).await;
        warm_entity_images(&server.pool, &storage, table, id, &keys).await;

        assert!(cached_image_exists(&storage, poster, id), "{table} poster");
        assert!(
            cached_image_exists(&storage, backdrop, id),
            "{table} backdrop"
        );
        let stored: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT images FROM {table} WHERE id = $1"
        )))
        .bind(id)
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert!(
            stored["backdrop"]["cached_at"].is_string(),
            "{table}: {stored}"
        );
    }
}

/// Titles enriched before warming existed get their art on disk from the daily sweep.
#[tokio::test]
async fn the_art_sweep_warms_what_is_not_on_disk_yet() {
    use api::images::{ImageType, cached_image_exists};
    use api::jobs::handlers::ArtWarmHandler;
    use api::jobs::worker::JobHandler;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/b.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(minimal_jpeg()))
        .mount(&mock)
        .await;
    let movie = uuid::Uuid::parse_str(&create_movie(&server, "Old Title").await).unwrap();
    create_stub_movie_file(&server.pool, movie).await;
    sqlx::query("UPDATE movies SET images = $1 WHERE id = $2")
        .bind(json!({ "backdrop": { "source_url": format!("{}/b.jpg", mock.uri()) } }))
        .bind(movie)
        .execute(&server.pool)
        .await
        .unwrap();

    ArtWarmHandler::new(server.pool.clone(), server.storage_root())
        .execute(
            uuid::Uuid::now_v7(),
            json!({}),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();

    assert!(cached_image_exists(
        &server.storage_root(),
        ImageType::MovieBackdrop,
        movie
    ));
}
