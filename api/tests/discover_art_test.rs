use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

// Discover art is TMDB's, for titles the library does not hold. It is fetched from TMDB
// once, kept on disk, and handed out from there.

fn minimal_jpeg() -> Vec<u8> {
    use image::{ImageBuffer, Rgb, codecs::jpeg::JpegEncoder};
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_pixel(1, 1, Rgb([0, 0, 255]));
    let mut buf = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, 85)
        .encode_image(&img)
        .unwrap();
    buf
}

#[tokio::test]
async fn art_is_fetched_once_and_served_from_disk() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = TestServer::start_api_only().await;
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/t/p/w1280/x.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(minimal_jpeg()))
        .expect(1)
        .mount(&mock)
        .await;
    let src = format!("{}/t/p/w1280/x.jpg", mock.uri());
    let storage = server.storage_root();

    let first = api::discover_art::cached_art(&storage, &src).await.unwrap();
    let second = api::discover_art::cached_art(&storage, &src).await.unwrap();
    assert_eq!(first, minimal_jpeg());
    assert_eq!(second, first);
}

#[tokio::test]
async fn the_art_route_is_not_an_open_proxy() {
    let server = TestServer::start_api_only().await;
    let client = TestClient::new(&server.api_url);
    for src in [
        "https://example.com/a.jpg",
        "http://image.tmdb.org/t/p/w500/a.jpg",
        "file:///etc/passwd",
    ] {
        let resp = client
            .get(&format!(
                "/api/v1/discover/art?src={}",
                urlencoding::encode(src)
            ))
            .await;
        resp.assert_status(StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn shelves_point_their_art_at_the_cache() {
    let server = TestServer::start_api_only().await;
    let section = json!({
        "title": "Trending Movies",
        "section_key": "trending_movies",
        "sort_order": 0,
        "items": [{
            "provider": "tmdb", "provider_id": "1", "media_type": "movie", "title": "Pick",
            "poster_url": "https://image.tmdb.org/t/p/w500/a.jpg",
            "backdrop_url": "https://image.tmdb.org/t/p/w1280/b.jpg",
            "identifiers": {}
        }],
    });
    api::repositories::metadata_cache::set(
        &server.pool,
        "discover",
        "trending_movies",
        &section,
        time::OffsetDateTime::now_utc() + time::Duration::hours(1),
    )
    .await
    .unwrap();

    let resp = server.admin_client().get("/api/v1/admin/discover").await;
    resp.assert_ok();
    let item = resp.json::<Value>()["sections"][0]["items"][0].clone();
    assert_eq!(
        item["poster_url"],
        "/api/v1/discover/art?src=https%3A%2F%2Fimage.tmdb.org%2Ft%2Fp%2Fw500%2Fa.jpg"
    );
    assert_eq!(
        item["backdrop_url"],
        "/api/v1/discover/art?src=https%3A%2F%2Fimage.tmdb.org%2Ft%2Fp%2Fw1280%2Fb.jpg"
    );
}
