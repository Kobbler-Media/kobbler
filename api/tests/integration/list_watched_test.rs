use crate::common::*;
use crate::dashboard_test::{
    create_episode, create_movie, create_season, create_show, mark_watched,
};
use serde_json::{Value, json};
use uuid::Uuid;

/// Library grids mark a title watched for the viewer: a movie once seen, a show or league
/// once every episode in the library is.
#[tokio::test]
async fn library_lists_mark_watched_titles() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;

    let seen_movie = create_movie(&server, "Seen Movie").await;
    let fresh_movie = create_movie(&server, "Fresh Movie").await;
    create_stub_movie_file(pool, seen_movie).await;
    create_stub_movie_file(pool, fresh_movie).await;
    server
        .admin_client()
        .put_json(
            &format!("/api/v1/admin/movies/{seen_movie}/watch-state"),
            &json!({ "watched": true }),
        )
        .await
        .assert_ok();

    let seen_show = show_with_episodes(&server, "Seen Show", &[true, true]).await;
    let half_show = show_with_episodes(&server, "Half Show", &[true, false]).await;
    let seen_league = show_with_episodes(&server, "Seen League", &[true]).await;
    let half_league = show_with_episodes(&server, "Half League", &[false, true]).await;
    sqlx::query("UPDATE tv_shows SET content_category = 'sport' WHERE id = ANY($1)")
        .bind([seen_league, half_league])
        .execute(pool)
        .await
        .unwrap();

    let cases = [
        ("/api/v1/admin/movies", seen_movie, fresh_movie),
        ("/api/v1/admin/tv/shows", seen_show, half_show),
        ("/api/v1/admin/sports/leagues", seen_league, half_league),
    ];
    let other = create_user_client(&server, "other@test.com", "viewer").await;
    for (path, seen, unseen) in cases {
        for query in [String::new(), format!("?around={unseen}")] {
            let url = format!("{path}{query}");
            let mine = watched_flags(server.admin_client().get(&url).await.json());
            assert_eq!(mine.get(&seen), Some(&true), "{url}: seen");
            assert_eq!(mine.get(&unseen), Some(&false), "{url}: unseen");

            let theirs = watched_flags(other.get(&url).await.json());
            assert_eq!(theirs.get(&seen), Some(&false), "{url}: another viewer");
        }
    }
}

async fn show_with_episodes(server: &TestServer, title: &str, watched: &[bool]) -> Uuid {
    let pool = &server.pool;
    let show = create_show(server, title).await;
    let season = create_season(pool, show, 1).await;
    for (i, &w) in watched.iter().enumerate() {
        let ep = create_episode(pool, season, i as i32 + 1).await;
        create_stub_episode_file(pool, ep).await;
        if w {
            mark_watched(server, ep).await;
        }
    }
    // Not in the library, so it does not hold the show back from counting as watched.
    create_episode(pool, season, watched.len() as i32 + 1).await;
    show
}

fn watched_flags(resp: Value) -> std::collections::HashMap<Uuid, bool> {
    resp["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["id"].as_str().unwrap().parse().unwrap(),
                i["watched"].as_bool().unwrap_or(false),
            )
        })
        .collect()
}
