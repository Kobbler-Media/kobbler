//! `manual` has to survive the whole way out to Kobload, because it is the only
//! thing that reopens something already imported.
//!
//! Everything upstream of this is testable against Kobbler's own tables, and
//! everything downstream is Kobload's own test suite. The join between them is a
//! JSON field on a JetStream message, which is exactly where a rename or a
//! forgotten `..` quietly turns a regrab into a no-op. So these read the message.

use crate::common::*;
use api::jobs::handlers::{MovieWantHandler, TvWantHandler};
use api::jobs::worker::JobHandler;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

async fn create_show(server: &TestServer, title: &str) -> Uuid {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({"title": title, "year": 2024}),
        )
        .await;
    resp.assert_status(axum::http::StatusCode::CREATED);
    resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

async fn create_season(pool: &sqlx::PgPool, show_id: Uuid, number: i32) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
         VALUES ($1, $2, $3, $4, true, false)",
    )
    .bind(id)
    .bind(show_id)
    .bind(number.to_string())
    .bind(number)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn create_episode(pool: &sqlx::PgPool, season_id: Uuid, number: i32) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(season_id)
    .bind(number.to_string())
    .bind(number)
    .bind(format!("Episode {number}"))
    .execute(pool)
    .await
    .unwrap();
    id
}

/// The episode-list branch of `tv_want` — what a chat regrab and the web UI's
/// replace-file button both go through.
#[tokio::test]
async fn tv_want_carries_manual_out_to_kobload() {
    let server = TestServer::start_api_only().await;
    configure_kobload(&server).await;

    let show = create_show(&server, "manualWantShow").await;
    let season = create_season(&server.pool, show, 24).await;
    let episode = create_episode(&server.pool, season, 1).await;

    let handler = TvWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    for manual in [false, true] {
        let tip = stream_tip(&server).await;
        handler
            .execute(
                Uuid::now_v7(),
                json!({
                    "show_id": show,
                    "season_id": season,
                    "episode_ids": [episode],
                    "manual": manual,
                    "additional_copy": manual,
                }),
                CancellationToken::new(),
            )
            .await
            .unwrap_or_else(|e| panic!("tv_want manual={manual} should succeed: {e:?}"));

        let sent = want_requests_since(&server, tip, &season.to_string()).await;
        let request = sent
            .last()
            .unwrap_or_else(|| panic!("tv_want manual={manual} published nothing"));

        assert_eq!(
            request["manual"],
            json!(manual),
            "manual={manual} did not reach the want request: {request}"
        );
        // The two ride together on a regrab and are read at opposite ends —
        // `manual` by the search, `additional_copy` by the ingest callback
        // minutes later — so a regrab that loses either is still broken.
        assert_eq!(
            request["metadata"]["additional_copy"],
            json!(manual),
            "additional_copy={manual} did not reach the want metadata: {request}"
        );
    }
}

/// `movie_want` builds its request in a different function, so it can drift on
/// its own.
#[tokio::test]
async fn movie_want_carries_manual_out_to_kobload() {
    let server = TestServer::start_api_only().await;
    configure_kobload(&server).await;

    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({"title": "manualWantFilm", "year": 2024}),
        )
        .await;
    resp.assert_status(axum::http::StatusCode::CREATED);
    let movie: Uuid = resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let handler = MovieWantHandler::new(
        server.pool.clone(),
        server.runtime_config.clone(),
        server.nats_publisher.clone(),
        server.library_events.clone(),
    );

    let tip = stream_tip(&server).await;
    handler
        .execute(
            Uuid::now_v7(),
            json!({"movie_id": movie, "manual": true, "additional_copy": true}),
            CancellationToken::new(),
        )
        .await
        .expect("movie_want should succeed");

    let sent = want_requests_since(&server, tip, &movie.to_string()).await;
    let request = sent.last().expect("movie_want published nothing");
    assert_eq!(request["manual"], json!(true), "{request}");
    assert_eq!(
        request["metadata"]["additional_copy"],
        json!(true),
        "{request}"
    );
}
