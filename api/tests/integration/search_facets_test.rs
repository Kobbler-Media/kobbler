use crate::common::*;
use reqwest::StatusCode;

// The test harness uses SqlFallbackBackend, whose facet lookups error so the
// route falls through to a 500 rather than a 404.
// These tests confirm the routes are registered and reach the handler — the
// Meili-backed facet logic itself is exercised against a live Meilisearch.

#[tokio::test]
async fn book_facets_route_is_registered() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    let resp = client.get("/api/v1/library/books/facets").await;
    resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn movie_facets_route_is_registered() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    let resp = client.get("/api/v1/admin/movies/facets").await;
    resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn tv_facets_route_is_registered() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    let resp = client.get("/api/v1/admin/tv/shows/facets").await;
    resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
}
