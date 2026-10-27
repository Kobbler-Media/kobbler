//! Integration tests for the sidecar pipeline.
//!
//! Tests the full media root pipeline (scan → interpret → match → enrich → write-sidecars)
//! with wiremock-mocked TMDB responses. Verifies that NFO sidecar metadata flows through
//! correctly and drives matching/enrichment.

use crate::common::*;
use api::jobs::handlers::{
    MediaRootEnrichHandler, MediaRootInterpretHandler, MediaRootMatchHandler, MediaRootScanHandler,
    MediaRootWriteSidecarsHandler,
};
use api::jobs::worker::JobHandler;
use api::repositories::{media_root_files, movies, tv_shows};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

// =============================================================================
// Helpers
// =============================================================================

/// Create a media root via the API and return its id + TempDir.
async fn create_media_root(server: &TestServer, media_type: &str) -> (Uuid, TempDir) {
    let root_dir = TempDir::new().unwrap();
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/media-roots",
            &json!({
                "path": root_dir.path().to_string_lossy(),
                "name": format!("Test {}", media_type),
                "media_type": media_type,
                "auto_add": true,
                "write_sidecars": true,
            }),
        )
        .await;
    resp.assert_status(reqwest::StatusCode::CREATED);
    let id: Uuid = resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    (id, root_dir)
}

/// Set up TMDB provider with wiremock base_url.
async fn setup_tmdb_mock(server: &TestServer, mock_server: &MockServer) {
    let mock_url = mock_server.uri();
    sqlx::query(
        r#"
        UPDATE video_metadata_providers
        SET config = jsonb_build_object('api_key', 'test-tmdb-key', 'base_url', $1),
            enabled = true
        WHERE provider_type = 'tmdb'
        "#,
    )
    .bind(&mock_url)
    .execute(&server.pool)
    .await
    .expect("Failed to configure TMDB mock");
}

/// Run the scan handler for a media root.
async fn run_scan(server: &TestServer, root_id: Uuid) -> Value {
    let handler = MediaRootScanHandler::new(
        server.pool.clone(),
        server.nats_publisher.clone(),
        server.search_backend_arc(),
        std::sync::Arc::new(tokio::sync::Semaphore::new(8)),
        server.runtime_config.clone(),
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
    );
    handler
        .execute(
            Uuid::now_v7(),
            json!({ "media_root_id": root_id }),
            CancellationToken::new(),
        )
        .await
        .expect("Scan handler failed")
}

/// Run the interpret handler for a media root.
async fn run_interpret(server: &TestServer, root_id: Uuid) -> Value {
    let handler = MediaRootInterpretHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
    );
    handler
        .execute(
            Uuid::now_v7(),
            json!({ "media_root_id": root_id }),
            CancellationToken::new(),
        )
        .await
        .expect("Interpret handler failed")
}

/// Run the match handler for a media root.
async fn run_match(server: &TestServer, root_id: Uuid) -> Value {
    let handler = MediaRootMatchHandler::new(
        server.pool.clone(),
        server.nats_publisher.clone(),
        server.search_backend_arc(),
        tokio::sync::broadcast::channel(16).0,
    );
    handler
        .execute(
            Uuid::now_v7(),
            json!({ "media_root_id": root_id }),
            CancellationToken::new(),
        )
        .await
        .expect("Match handler failed")
}

/// Run the enrich handler for a media root (catch-up orchestrator).
/// Returns {"enqueued": N} — entities are queued, not enriched inline.
async fn run_enrich(server: &TestServer, root_id: Uuid) -> Value {
    let handler = MediaRootEnrichHandler::new(
        server.pool.clone(),
        server.nats_publisher.clone(),
        server.storage_root(),
    );
    handler
        .execute(
            Uuid::now_v7(),
            json!({ "media_root_id": root_id }),
            CancellationToken::new(),
        )
        .await
        .expect("Enrich handler failed")
}

/// Run per-movie enrichment directly (equivalent to what the job worker does).
async fn run_movie_enrich(server: &TestServer, movie_id: Uuid) {
    api::jobs::handlers::movie_enrich::enrich_movie(&server.pool, movie_id, false)
        .await
        .expect("Movie enrichment failed");
}

/// Run per-TV-show enrichment directly (equivalent to what the job worker does).
async fn run_tv_show_enrich(server: &TestServer, show_id: Uuid) {
    api::jobs::handlers::tv_show_enrich::enrich_tv_show(&server.pool, show_id, false)
        .await
        .expect("TV show enrichment failed");
}

/// Run the write-sidecars handler for a media root.
async fn run_write_sidecars(server: &TestServer, root_id: Uuid) -> Value {
    let handler = MediaRootWriteSidecarsHandler::new(
        server.pool.clone(),
        std::sync::Arc::new(tokio::sync::Semaphore::new(8)),
    );
    handler
        .execute(
            Uuid::now_v7(),
            json!({ "media_root_id": root_id }),
            CancellationToken::new(),
        )
        .await
        .expect("WriteSidecars handler failed")
}

/// Create a movie directory with a dummy video file and optional NFO.
fn populate_movie_dir(root_dir: &TempDir, dir_name: &str, nfo_content: Option<&str>) {
    let movie_dir = root_dir.path().join(dir_name);
    std::fs::create_dir_all(&movie_dir).unwrap();
    std::fs::write(movie_dir.join("movie.mkv"), b"fake video").unwrap();
    if let Some(nfo) = nfo_content {
        std::fs::write(movie_dir.join("movie.nfo"), nfo).unwrap();
    }
}

/// Create a TV show directory structure with episode files and optional tvshow.nfo.
fn populate_tv_dir(
    root_dir: &TempDir,
    show_name: &str,
    seasons_episodes: &[(u32, &[&str])],
    tvshow_nfo: Option<&str>,
) {
    let show_dir = root_dir.path().join(show_name);
    std::fs::create_dir_all(&show_dir).unwrap();
    if let Some(nfo) = tvshow_nfo {
        std::fs::write(show_dir.join("tvshow.nfo"), nfo).unwrap();
    }
    for (season_num, episodes) in seasons_episodes {
        let season_dir = show_dir.join(format!("Season {:02}", season_num));
        std::fs::create_dir_all(&season_dir).unwrap();
        for ep_name in *episodes {
            std::fs::write(season_dir.join(ep_name), b"fake video").unwrap();
        }
    }
}

/// Get files for a media root with a specific match status.
async fn get_files(server: &TestServer, root_id: Uuid, status: Option<&str>) -> Vec<Value> {
    let files = media_root_files::list_for_root(&server.pool, root_id, status, 1000, 0)
        .await
        .unwrap();
    files
        .iter()
        .map(|f| {
            json!({
                "id": f.id,
                "file_path": f.file_path,
                "file_name": f.file_name,
                "match_status": f.match_status,
                "entity_type": f.entity_type,
                "entity_id": f.entity_id,
                "interpretation": f.interpretation,
                "sidecar_metadata": f.sidecar_metadata,
                "sidecar_images": f.sidecar_images,
                "match_candidates": f.match_candidates,
            })
        })
        .collect()
}

// =============================================================================
// TMDB mock response builders
// =============================================================================

fn tmdb_find_response(movie_ids: &[i64], tv_ids: &[i64]) -> Value {
    json!({
        "movie_results": movie_ids.iter().map(|id| json!({"id": id})).collect::<Vec<_>>(),
        "tv_results": tv_ids.iter().map(|id| json!({"id": id})).collect::<Vec<_>>(),
    })
}

fn tmdb_movie_search_response(results: &[(i64, &str, &str)]) -> Value {
    json!({
        "results": results.iter().map(|(id, title, date)| json!({
            "id": id,
            "title": title,
            "overview": format!("Overview for {}", title),
            "release_date": date,
            "poster_path": "/test-poster.jpg",
            "vote_average": 7.5,
            "vote_count": 1000,
        })).collect::<Vec<_>>(),
    })
}

fn tmdb_movie_detail(id: i64, title: &str, year: &str, imdb_id: &str) -> Value {
    json!({
        "id": id,
        "title": title,
        "original_title": title,
        "overview": format!("{} is a great movie.", title),
        "tagline": "A test tagline",
        "runtime": 120,
        "release_date": format!("{}-07-09", year),
        "poster_path": "/poster.jpg",
        "backdrop_path": "/backdrop.jpg",
        "imdb_id": imdb_id,
        "vote_average": 7.5,
        "vote_count": 5000,
        "genres": [{"name": "Comedy"}, {"name": "Drama"}],
        "credits": {
            "cast": [
                {"id": 1001, "name": "Test Actor", "character": "Main Character", "profile_path": "/actor.jpg", "order": 0}
            ],
            "crew": [
                {"id": 2001, "name": "Test Director", "department": "Directing", "job": "Director", "profile_path": "/director.jpg"}
            ]
        },
        "images": {
            "posters": [{"file_path": "/poster.jpg", "width": 500, "height": 750, "iso_639_1": "en"}],
            "backdrops": [{"file_path": "/backdrop.jpg", "width": 1920, "height": 1080, "iso_639_1": "en"}],
            "logos": []
        },
        "release_dates": {
            "results": [
                {"iso_3166_1": "US", "release_dates": [{"certification": "PG-13"}]}
            ]
        }
    })
}

fn tmdb_tv_detail(id: i64, name: &str, year: &str) -> Value {
    json!({
        "id": id,
        "name": name,
        "overview": format!("{} is a great show.", name),
        "status": "Ended",
        "first_air_date": format!("{}-03-25", year),
        "poster_path": "/poster.jpg",
        "backdrop_path": "/backdrop.jpg",
        "vote_average": 8.5,
        "vote_count": 3000,
        "episode_run_time": [30],
        "genres": [{"name": "Drama"}, {"name": "Crime"}],
        "networks": [{"name": "HBO"}],
        "seasons": [
            {"season_number": 1, "episode_count": 8, "air_date": format!("{}-03-25", year), "poster_path": "/s1-poster.jpg", "name": "Season 1"}
        ],
        "credits": {
            "cast": [
                {"id": 3001, "name": "Lead Actor", "character": "Protagonist", "profile_path": "/lead.jpg", "order": 0}
            ],
            "crew": [
                {"id": 4001, "name": "Show Creator", "department": "Writing", "job": "Creator", "profile_path": null}
            ]
        },
        "images": {
            "posters": [{"file_path": "/poster.jpg", "width": 500, "height": 750, "iso_639_1": "en"}],
            "backdrops": [{"file_path": "/backdrop.jpg", "width": 1920, "height": 1080, "iso_639_1": "en"}],
            "logos": []
        },
        "content_ratings": {
            "results": [
                {"iso_3166_1": "US", "rating": "TV-MA"}
            ]
        }
    })
}

fn tmdb_season_detail(show_name: &str, season_number: u32, episodes: &[(&str, u32)]) -> Value {
    json!({
        "season_number": season_number,
        "name": format!("Season {}", season_number),
        "overview": format!("Season {} of {}", season_number, show_name),
        "air_date": "2018-03-25",
        "poster_path": "/season-poster.jpg",
        "episodes": episodes.iter().map(|(name, ep_num)| json!({
            "episode_number": ep_num,
            "name": name,
            "overview": format!("Episode {} of {}", ep_num, show_name),
            "air_date": "2018-04-01",
            "runtime": 30,
            "still_path": "/episode-still.jpg",
        })).collect::<Vec<_>>(),
    })
}

// =============================================================================
// Test 1: Movie scan with NFO containing IMDB ID → cross-lookup
// =============================================================================

#[tokio::test]
async fn test_movie_scan_with_nfo_imdb_crosslookup() {
    let server = TestServer::start_api_only().await;
    let mock_server = MockServer::start().await;
    setup_tmdb_mock(&server, &mock_server).await;

    let (root_id, root_dir) = create_media_root(&server, "movie").await;

    // Create movie dir with NFO containing IMDB ID
    let nfo = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<movie>
    <title>Anchorman: The Legend of Ron Burgundy</title>
    <year>2004</year>
    <id>tt0357413</id>
</movie>"#;
    populate_movie_dir(&root_dir, "Anchorman (2004)", Some(nfo));

    // Mock TMDB /find for IMDB cross-lookup
    Mock::given(method("GET"))
        .and(path("/find/tt0357413"))
        .and(query_param("external_source", "imdb_id"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tmdb_find_response(&[8699], &[])))
        .expect(1)
        .mount(&mock_server)
        .await;

    // Mock TMDB /movie/8699 for detail fetch (used by both match and enrich)
    Mock::given(method("GET"))
        .and(path("/movie/8699"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tmdb_movie_detail(
            8699,
            "Anchorman: The Legend of Ron Burgundy",
            "2004",
            "tt0357413",
        )))
        .mount(&mock_server)
        .await;

    // Phase 1: Scan
    let scan_result = run_scan(&server, root_id).await;
    assert!(scan_result["new_files"].as_u64().unwrap() > 0);

    // Verify sidecar metadata was discovered
    let files = get_files(&server, root_id, None).await;
    assert_eq!(files.len(), 1, "Should have one scanned file");
    let sidecar = &files[0]["sidecar_metadata"];
    assert!(sidecar.is_object(), "Should have sidecar metadata from NFO");

    // Phase 2: Interpret
    let interp_result = run_interpret(&server, root_id).await;
    assert!(interp_result["interpreted"].as_u64().unwrap() > 0);

    // Verify interpretation has sidecar_ids with IMDB ID
    let files = get_files(&server, root_id, Some("identified")).await;
    assert_eq!(files.len(), 1, "File should be identified");
    let interp = &files[0]["interpretation"];
    assert!(interp.is_object(), "Should have interpretation");
    let sidecar_ids = &interp["sidecar_ids"];
    assert_eq!(
        sidecar_ids["imdb_id"], "tt0357413",
        "Interpretation should have IMDB ID from NFO"
    );

    // Phase 3: Match (should cross-lookup IMDB→TMDB via /find, then use TMDB ID)
    let match_result = run_match(&server, root_id).await;
    assert!(
        match_result["added"].as_u64().unwrap() > 0,
        "Should match via IMDB→TMDB cross-lookup"
    );

    // Verify movie entity was created
    let files = get_files(&server, root_id, Some("added")).await;
    assert_eq!(files.len(), 1, "File should be added");
    let entity_id: Uuid = files[0]["entity_id"].as_str().unwrap().parse().unwrap();
    let movie = movies::get(&server.pool, entity_id)
        .await
        .unwrap()
        .expect("Movie entity should exist");
    assert_eq!(movie.title, "Anchorman: The Legend of Ron Burgundy");

    // Phase 4: Enrich — orchestrator enqueues, then run per-entity enrichment
    let enrich_result = run_enrich(&server, root_id).await;
    // Match handler now enqueues enrichment directly, so the catch-up orchestrator
    // may report 0 if the job is already active (has_active_job guard).
    let _enqueued = enrich_result["enqueued"].as_u64().unwrap();
    run_movie_enrich(&server, entity_id).await;

    // Verify movie was enriched with details from TMDB mock
    let movie = movies::get_with_details(&server.pool, entity_id)
        .await
        .unwrap()
        .expect("Movie should still exist");
    assert!(
        movie.description.is_some(),
        "Movie should have description after enrichment"
    );
    assert_eq!(
        movie.rating,
        Some(7.5),
        "TMDB's 0-10 vote_average is stored as-is"
    );
}

// =============================================================================
// Test 2: TV show matched via title search → full pipeline
// =============================================================================

#[tokio::test]
async fn test_tv_scan_search_match_enrich() {
    let server = TestServer::start_api_only().await;
    let mock_server = MockServer::start().await;
    setup_tmdb_mock(&server, &mock_server).await;

    let (root_id, root_dir) = create_media_root(&server, "tv").await;

    // Create TV show dir (tvshow.nfo is NOT used per-episode; match falls back to search)
    populate_tv_dir(
        &root_dir,
        "Barry",
        &[(1, &["Barry - 1x01 - Chapter One.mkv"])],
        None,
    );

    // Mock TMDB /search/tv for title search
    Mock::given(method("GET"))
        .and(path("/search/tv"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{
                "id": 73107,
                "name": "Barry",
                "overview": "A hit man from the Midwest moves to Los Angeles.",
                "first_air_date": "2018-03-25",
                "poster_path": "/poster.jpg",
                "vote_average": 8.3,
                "vote_count": 1500,
            }]
        })))
        .mount(&mock_server)
        .await;

    // Mock TMDB /tv/73107 for show details
    Mock::given(method("GET"))
        .and(path("/tv/73107"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(tmdb_tv_detail(73107, "Barry", "2018")),
        )
        .mount(&mock_server)
        .await;

    // Mock TMDB /tv/73107/season/1 for season details
    Mock::given(method("GET"))
        .and(path("/tv/73107/season/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tmdb_season_detail(
            "Barry",
            1,
            &[("Chapter One: Make Your Mark", 1)],
        )))
        .mount(&mock_server)
        .await;

    // Phase 1: Scan
    let scan_result = run_scan(&server, root_id).await;
    assert!(scan_result["new_files"].as_u64().unwrap() > 0);

    // Phase 2: Interpret
    let interp_result = run_interpret(&server, root_id).await;
    assert!(interp_result["interpreted"].as_u64().unwrap() > 0);

    // Phase 3: Match (via title search)
    let match_result = run_match(&server, root_id).await;
    assert!(
        match_result["added"].as_u64().unwrap() > 0,
        "Should match via title search"
    );

    // Verify TV show entity was created
    let files = get_files(&server, root_id, Some("added")).await;
    assert_eq!(files.len(), 1, "Episode file should be added");
    let entity_id: Uuid = files[0]["entity_id"].as_str().unwrap().parse().unwrap();

    // The entity_id is for the episode — find the show
    let show_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT ts.show_id FROM tv_episodes te JOIN tv_seasons ts ON te.season_id = ts.id WHERE te.id = $1",
    )
    .bind(entity_id)
    .fetch_one(&server.pool)
    .await
    .expect("Should find show for episode");

    let show = tv_shows::get(&server.pool, show_id)
        .await
        .unwrap()
        .expect("TV show entity should exist");
    assert_eq!(show.title, "Barry");

    // Phase 4: Enrich — orchestrator enqueues, then run per-entity enrichment
    let enrich_result = run_enrich(&server, root_id).await;
    // Match handler now enqueues enrichment directly, so the catch-up orchestrator
    // may report 0 if the job is already active (has_active_job guard).
    let _enqueued = enrich_result["enqueued"].as_u64().unwrap();
    run_tv_show_enrich(&server, show_id).await;

    // Verify show was enriched
    let show = tv_shows::get_with_details(&server.pool, show_id)
        .await
        .unwrap()
        .expect("Show should still exist");
    assert!(
        show.description.is_some(),
        "Show should have description after enrichment"
    );
    assert_eq!(
        show.rating,
        Some(8.5),
        "TMDB's 0-10 vote_average is stored as-is"
    );
}

// =============================================================================
// Test 3: Movie sidecar writing after enrichment
// =============================================================================

#[tokio::test]
async fn test_movie_sidecar_writing_after_enrichment() {
    let server = TestServer::start_api_only().await;
    let mock_server = MockServer::start().await;
    setup_tmdb_mock(&server, &mock_server).await;

    let (root_id, root_dir) = create_media_root(&server, "movie").await;

    // Create movie dir without any pre-existing NFO
    populate_movie_dir(&root_dir, "Fight Club (1999)", None);

    // Mock TMDB search
    Mock::given(method("GET"))
        .and(path("/search/movie"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(tmdb_movie_search_response(&[(
                550,
                "Fight Club",
                "1999-10-15",
            )])),
        )
        .mount(&mock_server)
        .await;

    // Mock TMDB movie detail
    Mock::given(method("GET"))
        .and(path("/movie/550"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tmdb_movie_detail(
            550,
            "Fight Club",
            "1999",
            "tt0137523",
        )))
        .mount(&mock_server)
        .await;

    // Run the full pipeline
    run_scan(&server, root_id).await;
    run_interpret(&server, root_id).await;
    run_match(&server, root_id).await;
    run_enrich(&server, root_id).await;

    // Verify entity was created and matched
    let files = get_files(&server, root_id, Some("added")).await;
    assert_eq!(files.len(), 1, "Should have one added file");
    let entity_id: Uuid = files[0]["entity_id"].as_str().unwrap().parse().unwrap();
    let movie = movies::get(&server.pool, entity_id)
        .await
        .unwrap()
        .expect("Movie should exist");
    assert_eq!(movie.title, "Fight Club");

    // Run per-entity enrichment (enqueued by catch-up orchestrator)
    run_movie_enrich(&server, entity_id).await;

    // Run write-sidecars
    run_write_sidecars(&server, root_id).await;

    // Verify NFO was written to disk
    let nfo_path = root_dir.path().join("Fight Club (1999)/movie.nfo");
    assert!(
        nfo_path.exists(),
        "movie.nfo should have been written by write-sidecars"
    );

    // Parse the NFO and verify contents
    let nfo_content = std::fs::read_to_string(&nfo_path).unwrap();
    assert!(
        nfo_content.contains("Fight Club"),
        "NFO should contain movie title"
    );
    assert!(
        nfo_content.contains("tt0137523") || nfo_content.contains("550"),
        "NFO should contain IMDB or TMDB ID"
    );
    assert!(
        nfo_content.contains("<rating>7.5</rating>"),
        "Kodi reads NFO ratings on the 0-10 scale: {nfo_content}"
    );
}

// =============================================================================
// Test 4: Ambiguous movie match
// =============================================================================

#[tokio::test]
async fn test_movie_ambiguous_match() {
    let server = TestServer::start_api_only().await;
    let mock_server = MockServer::start().await;
    setup_tmdb_mock(&server, &mock_server).await;

    let (root_id, root_dir) = create_media_root(&server, "movie").await;

    // Create a movie dir with a generic name that could match multiple results
    populate_movie_dir(&root_dir, "Legend (2015)", None);

    // Mock TMDB search returning multiple non-exact matches
    Mock::given(method("GET"))
        .and(path("/search/movie"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(tmdb_movie_search_response(&[
                (263115, "Legend", "2015-09-09"),
                (81, "Legends of the Fall", "1994-12-23"),
                (14836, "I Am Legend", "2007-12-14"),
            ])),
        )
        .mount(&mock_server)
        .await;

    // Mock detail for case where match picks first exact-title match
    Mock::given(method("GET"))
        .and(path("/movie/263115"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tmdb_movie_detail(
            263115,
            "Legend",
            "2015",
            "tt3569230",
        )))
        .mount(&mock_server)
        .await;

    // Run scan → interpret → match
    run_scan(&server, root_id).await;
    run_interpret(&server, root_id).await;
    let _match_result = run_match(&server, root_id).await;

    // "Legend" should match exactly (case-insensitive) to the first result
    let files = get_files(&server, root_id, None).await;
    assert_eq!(files.len(), 1);

    let match_status = files[0]["match_status"].as_str().unwrap();
    // It could be "added" (if exact title match picked first result)
    // or "ambiguous" (if heuristic didn't apply)
    assert!(
        match_status == "added" || match_status == "ambiguous",
        "Should be added or ambiguous, got: {}",
        match_status
    );
}

// =============================================================================
// Test 5: TV show with multiple seasons and episodes
// =============================================================================

#[tokio::test]
async fn test_tv_multi_season_matching() {
    let server = TestServer::start_api_only().await;
    let mock_server = MockServer::start().await;
    setup_tmdb_mock(&server, &mock_server).await;

    let (root_id, root_dir) = create_media_root(&server, "tv").await;

    // Create TV show dir with multiple seasons (no tvshow.nfo — match via search)
    populate_tv_dir(
        &root_dir,
        "The Wire",
        &[
            (
                1,
                &[
                    "The Wire - 1x01 - The Target.mkv",
                    "The Wire - 1x02 - The Detail.mkv",
                ],
            ),
            (2, &["The Wire - 2x01 - Ebb Tide.mkv"]),
        ],
        None,
    );

    // Mock TMDB /search/tv for title search
    Mock::given(method("GET"))
        .and(path("/search/tv"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{
                "id": 1438,
                "name": "The Wire",
                "overview": "Told from the points of view of both the police and their targets.",
                "first_air_date": "2002-06-02",
                "poster_path": "/poster.jpg",
                "vote_average": 8.7,
                "vote_count": 2000,
            }]
        })))
        .mount(&mock_server)
        .await;

    // Mock TMDB /tv/1438 for show details (with 2 seasons)
    let mut tv_detail = tmdb_tv_detail(1438, "The Wire", "2002");
    tv_detail["seasons"] = json!([
        {"season_number": 1, "episode_count": 13, "air_date": "2002-06-02", "poster_path": "/s1.jpg", "name": "Season 1"},
        {"season_number": 2, "episode_count": 12, "air_date": "2003-06-01", "poster_path": "/s2.jpg", "name": "Season 2"},
    ]);
    Mock::given(method("GET"))
        .and(path("/tv/1438"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tv_detail))
        .mount(&mock_server)
        .await;

    // Mock season 1
    Mock::given(method("GET"))
        .and(path("/tv/1438/season/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tmdb_season_detail(
            "The Wire",
            1,
            &[
                ("The Target", 1),
                ("The Detail", 2),
                ("The Buys", 3),
                ("Old Cases", 4),
            ],
        )))
        .mount(&mock_server)
        .await;

    // Mock season 2
    Mock::given(method("GET"))
        .and(path("/tv/1438/season/2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tmdb_season_detail(
            "The Wire",
            2,
            &[("Ebb Tide", 1), ("Collateral Damage", 2)],
        )))
        .mount(&mock_server)
        .await;

    // Run full pipeline
    run_scan(&server, root_id).await;
    run_interpret(&server, root_id).await;
    run_match(&server, root_id).await;

    // Verify all 3 episodes were matched
    let files = get_files(&server, root_id, Some("added")).await;
    assert_eq!(
        files.len(),
        3,
        "All 3 episode files should be added: {:?}",
        files
            .iter()
            .map(|f| f["file_name"].as_str().unwrap())
            .collect::<Vec<_>>()
    );

    // Verify show entity exists
    let entity_id: Uuid = files[0]["entity_id"].as_str().unwrap().parse().unwrap();
    let show_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT ts.show_id FROM tv_episodes te JOIN tv_seasons ts ON te.season_id = ts.id WHERE te.id = $1",
    )
    .bind(entity_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();

    let show = tv_shows::get(&server.pool, show_id)
        .await
        .unwrap()
        .expect("Show should exist");
    assert_eq!(show.title, "The Wire");

    // Verify 2 seasons were created
    let season_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tv_seasons WHERE show_id = $1")
            .bind(show_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert_eq!(season_count, 2, "Should have 2 seasons");

    // Run enrich — orchestrator enqueues, then run per-entity enrichment
    let enrich_result = run_enrich(&server, root_id).await;
    // Match handler now enqueues enrichment directly, so the catch-up orchestrator
    // may report 0 if the job is already active (has_active_job guard).
    let _enqueued = enrich_result["enqueued"].as_u64().unwrap();
    run_tv_show_enrich(&server, show_id).await;

    let show = tv_shows::get_with_details(&server.pool, show_id)
        .await
        .unwrap()
        .expect("Show should exist after enrich");
    assert!(show.description.is_some(), "Show should be enriched");
}
