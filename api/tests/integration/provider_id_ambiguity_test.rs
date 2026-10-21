//! Migration 0093 dropped the unique `identifiers->>'tvdb_id'` indexes on
//! purpose: TVDB models a show reboot as a single entity where TMDB has one
//! entity per reboot, so a tvdb_id legitimately matches several rows.
//!
//! Every site that resolves an entity from a non-unique provider id therefore
//! has to agree on one rule, or two reboots take turns overwriting each other's
//! title/year/status/images on every sync:
//!
//!   1. prefer the narrowest unambiguous key the caller can supply
//!      (a co-identifier on a UNIQUE index for the upserts, the candidate's
//!      year for file linking),
//!   2. and when the match is still ambiguous, take the oldest row under a
//!      *total* order — `created_at ASC, id ASC` — so the winner cannot change
//!      after a VACUUM.

use crate::common::*;
use api::db::DbPool;
use api::media_root_resolve::{ResolveContext, link_to_library};
use api::repositories::{
    media_root_files, media_roots, movies, people, tv_episodes, tv_seasons, tv_shows,
};
use serde_json::{Value, json};
use uuid::Uuid;

const TVDB: &str = "75000";

async fn seed_show(pool: &DbPool, title: &str, year: i32, identifiers: Value) -> Uuid {
    tv_shows::create(
        pool,
        tv_shows::TvShowInput {
            title: title.to_string(),
            year: Some(year),
            identifiers: Some(identifiers),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

async fn backdate(pool: &DbPool, table: &str, id: Uuid, days: i32) {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {table} SET created_at = NOW() - ($2 || ' days')::interval WHERE id = $1"
    )))
    .bind(id)
    .bind(days.to_string())
    .execute(pool)
    .await
    .unwrap();
}

async fn show_title(pool: &DbPool, id: Uuid) -> String {
    sqlx::query_scalar::<_, String>("SELECT title FROM tv_shows WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

// =============================================================================
// #6 — provider-id upserts must not resolve an arbitrary row
// =============================================================================

/// A TVDB sync that carries the reboot's own TMDB id must land on the reboot,
/// not on whichever row Postgres returns first.
#[tokio::test]
async fn tvdb_upsert_prefers_the_row_whose_unique_tmdb_id_matches() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let original = seed_show(
        pool,
        "Battlestar Galactica",
        1978,
        json!({"tmdb_id": "1000", "tvdb_id": TVDB}),
    )
    .await;
    let reboot = seed_show(
        pool,
        "Battlestar Galactica",
        2004,
        json!({"tmdb_id": "2000", "tvdb_id": TVDB}),
    )
    .await;

    let resolved = tv_shows::upsert_by_provider(
        pool,
        "tvdb",
        TVDB,
        tv_shows::TvShowInput {
            title: "Battlestar Galactica (2004)".to_string(),
            year: Some(2004),
            identifiers: Some(json!({"tmdb_id": "2000"})),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(
        resolved, reboot,
        "TVDB upsert carrying tmdb_id 2000 must resolve the reboot, not the original"
    );
    assert_eq!(
        show_title(pool, original).await,
        "Battlestar Galactica",
        "the 1978 original must not be overwritten by the reboot's sync"
    );
}

/// With no co-identifier to go on the match is genuinely ambiguous, so the rule
/// is "oldest row wins" — and it has to hold when the oldest row is *not* the
/// one a sequential scan happens to return first.
#[tokio::test]
async fn tvdb_upsert_falls_back_to_the_oldest_row_not_the_first_scanned() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    // Inserted first (so it is first in the heap) but logically the newer row.
    let newer = seed_show(pool, "Newer Reboot", 2020, json!({"tvdb_id": TVDB})).await;
    let older = seed_show(pool, "Older Original", 1995, json!({"tvdb_id": TVDB})).await;
    backdate(pool, "tv_shows", older, 400).await;

    let resolved = tv_shows::upsert_by_provider(
        pool,
        "tvdb",
        TVDB,
        tv_shows::TvShowInput {
            title: "Synced From TVDB".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(
        resolved, older,
        "ambiguous tvdb match must take the oldest row, not the first one scanned"
    );
    assert_eq!(
        show_title(pool, newer).await,
        "Newer Reboot",
        "the newer row must be left alone"
    );

    // Stable across calls: the same row wins every time.
    let again = tv_shows::upsert_by_provider(
        pool,
        "tvdb",
        TVDB,
        tv_shows::TvShowInput {
            title: "Synced From TVDB Again".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(again, older, "the winner must not change between syncs");
}

#[tokio::test]
async fn tvdb_movie_upsert_prefers_the_row_whose_unique_tmdb_id_matches() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let original = movies::create(
        pool,
        movies::MovieInput {
            title: "The Thing".to_string(),
            year: Some(1982),
            identifiers: Some(json!({"tmdb_id": "1000", "tvdb_id": TVDB})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let remake = movies::create(
        pool,
        movies::MovieInput {
            title: "The Thing".to_string(),
            year: Some(2011),
            identifiers: Some(json!({"tmdb_id": "2000", "tvdb_id": TVDB})),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let resolved = movies::upsert_by_provider(
        pool,
        "tvdb",
        TVDB,
        movies::MovieInput {
            title: "The Thing (2011)".to_string(),
            year: Some(2011),
            identifiers: Some(json!({"tmdb_id": "2000"})),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(resolved, remake, "must resolve the remake by its TMDB id");
    let original_title: String = sqlx::query_scalar("SELECT title FROM movies WHERE id = $1")
        .bind(original)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(original_title, "The Thing");
}

#[tokio::test]
async fn tvdb_person_upsert_falls_back_to_the_oldest_row_not_the_first_scanned() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let newer = people::create(
        pool,
        people::PersonInput {
            name: "Newer Record".to_string(),
            identifiers: Some(json!({"tvdb_id": TVDB})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let older = people::create(
        pool,
        people::PersonInput {
            name: "Older Record".to_string(),
            identifiers: Some(json!({"tvdb_id": TVDB})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    backdate(pool, "people", older, 400).await;

    let resolved = people::upsert_by_provider(
        pool,
        "tvdb",
        TVDB,
        people::PersonInput {
            name: "Synced Person".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(resolved, older);
    let newer_name: String = sqlx::query_scalar("SELECT name FROM people WHERE id = $1")
        .bind(newer)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(newer_name, "Newer Record");
}

// =============================================================================
// #7 — file linking must not pick an arbitrary show sharing a provider id
// =============================================================================

async fn seed_episode(pool: &DbPool, show_id: Uuid, season: i32, episode: i32) -> Uuid {
    let season_id = tv_seasons::upsert(
        pool,
        tv_seasons::SeasonInput {
            show_id,
            season_identifier: season.to_string(),
            season_sort_order: season,
            title: Some(format!("Season {season}")),
            description: None,
            images: None,
            air_date: None,
            episode_count: None,
            is_monitored: false,
            is_special: false,
            auto_want_mode: "inherit".to_string(),
        },
    )
    .await
    .unwrap();

    tv_episodes::create(
        pool,
        tv_episodes::EpisodeInput {
            season_id,
            episode_identifier: episode.to_string(),
            episode_sort_order: episode,
            title: Some(format!("Episode {episode}")),
            description: None,
            images: None,
            air_date: None,
            air_time: None,
            runtime: None,
            identifiers: json!({}),
            country: None,
        },
    )
    .await
    .unwrap()
}

/// `link_to_library` used to take whichever show the planner returned for a
/// shared tvdb_id. When that was the wrong reboot the episode lookup failed and
/// the user saw "No episode S01E01 in '<title>'" — a provider-shaped error for
/// a query-ordering bug.
#[tokio::test]
async fn link_to_library_picks_the_show_matching_the_candidate_year() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    // The original is both first in the heap *and* the oldest row, so neither a
    // sequential scan nor the "oldest wins" tie-break can rescue this on its
    // own: only the candidate's year names the right reboot.
    let original = seed_show(pool, "Battlestar Galactica", 1978, json!({"tvdb_id": TVDB})).await;
    let reboot = seed_show(pool, "Battlestar Galactica", 2004, json!({"tvdb_id": TVDB})).await;
    backdate(pool, "tv_shows", original, 400).await;

    let reboot_episode = seed_episode(pool, reboot, 1, 1).await;
    seed_episode(pool, original, 9, 9).await;

    let root_dir = tempfile::TempDir::new().unwrap();
    let root_id = media_roots::create(
        pool,
        media_roots::MediaRootInput {
            path: root_dir.path().to_string_lossy().to_string(),
            name: "TV".to_string(),
            media_type: "tv".to_string(),
            auto_add: false,
            naming_template: None,
            default_entitlement_scope: None,
            write_sidecars: false,
            write_images: false,
            is_default: false,
        },
    )
    .await
    .unwrap();
    let root = media_roots::get_by_id(pool, root_id)
        .await
        .unwrap()
        .unwrap();

    let file_path = root_dir.path().join("Battlestar.Galactica.S01E01.mkv");
    let (file_id, _) = media_root_files::upsert_by_path(
        pool,
        media_root_files::MediaRootFileInput {
            media_root_id: root_id,
            file_path: file_path.to_string_lossy().to_string(),
            file_name: "Battlestar.Galactica.S01E01.mkv".to_string(),
            file_size: Some(1024),
            file_mtime: None,
            file_fingerprint: None,
        },
    )
    .await
    .unwrap();
    media_root_files::set_interpreted(
        pool,
        file_id,
        &json!({
            "path": file_path.to_string_lossy(),
            "success": true,
            "media_type": "tv",
            "show_name": "Battlestar Galactica",
            "season": 1,
            "episodes": [1],
        }),
    )
    .await
    .unwrap();
    let file = media_root_files::get_by_id(pool, file_id)
        .await
        .unwrap()
        .unwrap();

    let ctx = ResolveContext {
        pool,
        nats: &server.nats_publisher,
        search: server.search_backend(),
    };
    let outcome = link_to_library(
        &ctx,
        &root,
        &file,
        file_id,
        &json!({
            "provider": "tvdb",
            "provider_id": TVDB,
            "title": "Battlestar Galactica",
            "year": 2004,
        }),
    )
    .await
    .expect("link_to_library must find the 2004 reboot's episode");

    assert_eq!(outcome.entity_type, "tv_episode");
    assert_eq!(
        outcome.entity_id, reboot_episode,
        "the file must land on the reboot named by the candidate year"
    );
}

/// Without a year to go on the rule is the same one #6 uses: oldest row wins,
/// under a total order, so re-running the matcher cannot split one release
/// across two show records.
#[tokio::test]
async fn find_existing_tv_show_is_stable_when_titles_collide() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let newer = seed_show(pool, "The Office", 2005, json!({"tmdb_id": "2000"})).await;
    let older = seed_show(pool, "The Office", 2001, json!({"tmdb_id": "1000"})).await;
    backdate(pool, "tv_shows", older, 400).await;

    let first = api::jobs::handlers::find_existing_tv_show(pool, "The Office", None).await;
    let second = api::jobs::handlers::find_existing_tv_show(pool, "the office", None).await;

    assert_eq!(
        first,
        Some(older),
        "title-only matching must take the oldest row, not the first scanned"
    );
    assert_eq!(first, second, "repeated matcher runs must agree");
    assert_ne!(first, Some(newer));
}
