use crate::common::*;
use crate::kids_mode_test::{kids_client, seed_movie, seed_show};
use api::repositories::metadata_cache;
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

async fn seed_shelf(
    server: &TestServer,
    key: &str,
    sort_order: i32,
    kids: bool,
    items: &[(&str, &str)],
) {
    let items: Vec<Value> = items
        .iter()
        .enumerate()
        .map(|(i, (media_type, title))| {
            json!({ "provider": "tmdb", "provider_id": i.to_string(), "media_type": media_type, "title": title })
        })
        .collect();
    metadata_cache::set(
        &server.pool,
        "discover",
        key,
        &json!({ "title": key, "section_key": key, "sort_order": sort_order, "kids": kids, "items": items }),
        OffsetDateTime::now_utc() + time::Duration::hours(1),
    )
    .await
    .unwrap();
}

async fn seed_catalogue(server: &TestServer) {
    seed_movie(server, "Breakin'", Some("PG"), &[], None).await;
    seed_show(server, "Breaking Bad", Some("TV-MA"), &[], None).await;
    seed_shelf(
        server,
        "trending_tv",
        0,
        false,
        &[("tv", "Breaking Bad"), ("tv", "The Last of Us")],
    )
    .await;
    seed_shelf(
        server,
        "popular_movies",
        3,
        false,
        &[("movie", "Jailbreak"), ("movie", "Breakfast at Tiffany's")],
    )
    .await;
    seed_shelf(
        server,
        "kids_tv",
        0,
        true,
        &[("tv", "Bluey"), ("tv", "Break Point Kids")],
    )
    .await;
}

fn suggestions(resp: &TestResponse) -> Vec<(String, String, String)> {
    resp.assert_ok();
    resp.json::<Value>()["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["title"].as_str().unwrap().to_string(),
                s["media_type"].as_str().unwrap().to_string(),
                s["source"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn s(title: &str, media_type: &str, source: &str) -> (String, String, String) {
    (title.into(), media_type.into(), source.into())
}

#[tokio::test]
async fn suggestions_merge_library_titles_with_tmdb_shelves() {
    let server = TestServer::start_api_only().await;
    seed_catalogue(&server).await;
    let client = server.admin_client();
    let path = "/api/v1/admin/search/suggestions";

    assert_eq!(
        suggestions(&client.get(&format!("{path}?q=break")).await),
        vec![
            s("Breakin'", "movie", "library"),
            s("Breaking Bad", "tv", "library"),
            s("Breakfast at Tiffany's", "movie", "tmdb"),
        ],
    );
    assert_eq!(
        suggestions(&client.get(&format!("{path}?q=break&media=movie")).await),
        vec![
            s("Breakin'", "movie", "library"),
            s("Breakfast at Tiffany's", "movie", "tmdb")
        ],
    );
    assert_eq!(
        suggestions(&client.get(&format!("{path}?q=bad")).await),
        vec![s("Breaking Bad", "tv", "library")],
    );
    assert_eq!(
        suggestions(&client.get(&format!("{path}?q=")).await),
        vec![
            s("Breaking Bad", "tv", "tmdb"),
            s("The Last of Us", "tv", "tmdb"),
            s("Jailbreak", "movie", "tmdb"),
            s("Breakfast at Tiffany's", "movie", "tmdb"),
        ],
    );
    assert_eq!(
        suggestions(&client.get(&format!("{path}?q=&limit=1")).await),
        vec![s("Breaking Bad", "tv", "tmdb")],
    );
    assert!(suggestions(&client.get(&format!("{path}?q=%25")).await).is_empty());
}

#[tokio::test]
async fn kids_suggestions_keep_to_kids_titles_and_shelves() {
    let server = TestServer::start_api_only().await;
    seed_catalogue(&server).await;
    let kids = kids_client(&server, "kid@test.com").await;
    let path = "/api/v1/admin/search/suggestions";

    assert_eq!(
        suggestions(&kids.get(&format!("{path}?q=break")).await),
        vec![
            s("Breakin'", "movie", "library"),
            s("Break Point Kids", "tv", "tmdb")
        ],
    );
    assert_eq!(
        suggestions(&kids.get(&format!("{path}?q=")).await),
        vec![
            s("Bluey", "tv", "tmdb"),
            s("Break Point Kids", "tv", "tmdb")
        ],
    );
}

#[tokio::test]
async fn suggestions_need_auth() {
    let server = TestServer::start_api_only().await;
    TestClient::new(&server.api_url)
        .get("/api/v1/admin/search/suggestions?q=break")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

async fn seed_tmdb_titles(server: &TestServer) {
    sqlx::query(
        "INSERT INTO tmdb_titles (media_type, tmdb_id, original_title, title, popularity) VALUES
           ('tv', 1, 'Breaking Bad', NULL, 90.0),
           ('movie', 2, 'Breaking Away', NULL, 12.0),
           ('movie', 3, 'Breaking Away', NULL, 2.0),
           ('tv', 4, 'Prison Break', NULL, 40.0),
           ('movie', 5, '기생충', 'Parasite', 60.0),
           ('movie', 6, 'Le Fabuleux Destin d''Amélie Poulain', 'Amélie', 30.0),
           ('movie', 7, 'Unbreakable', NULL, 80.0)",
    )
    .execute(&server.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn export_titles_rank_with_the_library_and_shelves_by_popularity() {
    let server = TestServer::start_api_only().await;
    seed_catalogue(&server).await;
    seed_tmdb_titles(&server).await;
    let client = server.admin_client();
    let path = "/api/v1/admin/search/suggestions";

    assert_eq!(
        suggestions(&client.get(&format!("{path}?q=break")).await),
        vec![
            s("Breaking Bad", "tv", "library"),
            s("Breaking Away", "movie", "tmdb"),
            s("Breakin'", "movie", "library"),
            s("Breakfast at Tiffany's", "movie", "tmdb"),
            s("Prison Break", "tv", "tmdb"),
        ],
    );
    assert_eq!(
        suggestions(
            &client
                .get(&format!("{path}?q=break&media=tv&limit=2"))
                .await
        ),
        vec![
            s("Breaking Bad", "tv", "library"),
            s("Prison Break", "tv", "tmdb"),
        ],
    );
    assert_eq!(
        suggestions(&client.get(&format!("{path}?q=PARA")).await),
        vec![s("Parasite", "movie", "tmdb")],
    );
    assert_eq!(
        suggestions(&client.get(&format!("{path}?q=기생")).await),
        vec![s("Parasite", "movie", "tmdb")],
    );
    assert_eq!(
        suggestions(&client.get(&format!("{path}?q=destin")).await),
        vec![s("Amélie", "movie", "tmdb")],
    );
    assert!(
        !suggestions(&client.get(&format!("{path}?q=")).await)
            .iter()
            .any(|(title, _, _)| title == "Prison Break")
    );
}

#[tokio::test]
async fn kids_suggestions_include_export_titles_but_not_library_titles_hidden_from_them() {
    let server = TestServer::start_api_only().await;
    seed_catalogue(&server).await;
    seed_tmdb_titles(&server).await;
    let kids = kids_client(&server, "kid@test.com").await;

    assert_eq!(
        suggestions(&kids.get("/api/v1/admin/search/suggestions?q=break").await),
        vec![
            s("Breaking Away", "movie", "tmdb"),
            s("Breakin'", "movie", "library"),
            s("Break Point Kids", "tv", "tmdb"),
            s("Prison Break", "tv", "tmdb"),
        ],
    );
    assert_eq!(
        suggestions(&kids.get("/api/v1/admin/search/suggestions?q=para").await),
        vec![s("Parasite", "movie", "tmdb")],
    );
}

async fn enable_book_titles(server: &TestServer) {
    server
        .runtime_config
        .update(api::runtime_config::RuntimeConfigUpdate {
            book_title_suggestions_enabled: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
}

async fn seed_books(server: &TestServer) {
    create_book_with_author(server, "Dune Messiah", "Frank Herbert").await;
    create_book_with_author(server, "The Way of Kings", "Brandon Sanderson").await;
    sqlx::query(
        "INSERT INTO hardcover_titles (hardcover_id, title, author, users_count) VALUES
           (1, 'Dune', 'Frank Herbert', 15037),
           (2, 'Dune Messiah', 'Frank Herbert', 5000),
           (3, 'Mistborn: The Final Empire', 'Brandon Sanderson', 9000),
           (4, 'Sand Dunes', 'Ana Ruiz', 10),
           (5, 'The Sandman', 'Neil Gaiman', 3000),
           (6, 'Good Omens', 'Neil Gaiman', 2000)",
    )
    .execute(&server.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn book_suggestions_put_library_books_and_authors_before_hardcover_titles() {
    let server = TestServer::start_api_only().await;
    seed_books(&server).await;
    let admin = server.admin_client();
    let kids = kids_client(&server, "kid@test.com").await;
    let path = "/api/v1/admin/search/suggestions";

    assert_eq!(
        suggestions(&admin.get(&format!("{path}?q=dune&media=book")).await),
        vec![s("Dune Messiah", "book", "library")],
    );
    enable_book_titles(&server).await;

    for client in [&admin, &kids] {
        assert_eq!(
            suggestions(&client.get(&format!("{path}?q=dune&media=book")).await),
            vec![
                s("Dune Messiah", "book", "library"),
                s("Dune", "book", "hardcover"),
                s("Sand Dunes", "book", "hardcover"),
            ],
        );
        assert_eq!(
            suggestions(&client.get(&format!("{path}?q=sand&media=book")).await),
            vec![
                s("Sand Dunes", "book", "hardcover"),
                s("Brandon Sanderson", "author", "library"),
                s("The Sandman", "book", "hardcover"),
            ],
        );
        assert_eq!(
            suggestions(&client.get(&format!("{path}?q=GAI&media=book")).await),
            vec![s("Neil Gaiman", "author", "hardcover")],
        );
        assert_eq!(
            suggestions(&client.get(&format!("{path}?q=herb&media=book")).await),
            vec![s("Frank Herbert", "author", "library")],
        );
    }
    assert!(suggestions(&admin.get(&format!("{path}?q=dune")).await).is_empty());
    assert!(suggestions(&admin.get(&format!("{path}?q=&media=book")).await).is_empty());
}

#[tokio::test]
async fn all_media_suggestions_interleave_tmdb_and_hardcover_by_rank() {
    let server = TestServer::start_api_only().await;
    seed_books(&server).await;
    enable_book_titles(&server).await;
    sqlx::query(
        "INSERT INTO tmdb_titles (media_type, tmdb_id, original_title, title, popularity) VALUES
           ('movie', 1, 'Dune: Part Two', NULL, 90.0),
           ('movie', 2, 'Dune', NULL, 80.0)",
    )
    .execute(&server.pool)
    .await
    .unwrap();

    assert_eq!(
        suggestions(
            &server
                .admin_client()
                .get("/api/v1/admin/search/suggestions?q=dune&media=movie,tv,book")
                .await
        ),
        vec![
            s("Dune Messiah", "book", "library"),
            s("Dune: Part Two", "movie", "tmdb"),
            s("Dune", "book", "hardcover"),
            s("Sand Dunes", "book", "hardcover"),
        ],
    );
}

async fn seed_live_channel(
    server: &TestServer,
    tuner_id: Uuid,
    name: &str,
    lcn: i32,
    enabled: bool,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO livetv_channels (tuner_id, lcn, scan_name, frequency, program_number, enabled)
         VALUES ($1, $2, $3, 0, $2, $4) RETURNING id",
    )
    .bind(tuner_id)
    .bind(lcn)
    .bind(name)
    .bind(enabled)
    .fetch_one(&server.pool)
    .await
    .unwrap()
}

async fn seed_airing(
    server: &TestServer,
    channel_id: Uuid,
    title: &str,
    episode_title: Option<&str>,
    starts_in_mins: i64,
) {
    let start = OffsetDateTime::now_utc() + time::Duration::minutes(starts_in_mins);
    sqlx::query(
        "INSERT INTO livetv_guide (channel_id, start_time, end_time, title, episode_title)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(channel_id)
    .bind(start)
    .bind(start + time::Duration::minutes(30))
    .bind(title)
    .bind(episode_title)
    .execute(&server.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn live_tv_suggestions_list_channels_then_guide_titles_by_soonest_airing() {
    let server = TestServer::start_api_only().await;
    let tuner_id: Uuid = sqlx::query_scalar(
        "INSERT INTO livetv_tuners (device_id, friendly_name, base_url, tuner_count)
         VALUES ('T1', 'Tuner', 'http://127.0.0.1:1', 2) RETURNING id",
    )
    .fetch_one(&server.pool)
    .await
    .unwrap();
    let abc = seed_live_channel(&server, tuner_id, "ABC News", 2, true).await;
    let nine = seed_live_channel(&server, tuner_id, "Nine", 9, true).await;
    let hidden = seed_live_channel(&server, tuner_id, "ABC Kids", 22, false).await;
    seed_airing(&server, nine, "Abbott Elementary", None, 120).await;
    seed_airing(&server, nine, "Abbott Elementary", None, 30).await;
    seed_airing(&server, abc, "ABC Late News", None, 10).await;
    seed_airing(&server, abc, "Absolutely Fabulous", None, 60 * 60).await;
    seed_airing(&server, abc, "Abba Gold", None, -90).await;
    seed_airing(&server, nine, "Gardening", Some("Abundance"), 15).await;
    seed_airing(&server, hidden, "Abyss Divers", None, 5).await;
    let path = "/api/v1/admin/search/suggestions";

    assert_eq!(
        suggestions(
            &server
                .admin_client()
                .get(&format!("{path}?q=ab&media=livetv"))
                .await
        ),
        vec![
            s("ABC News", "livetv", "library"),
            s("ABC Late News", "livetv", "guide"),
            s("Abbott Elementary", "livetv", "guide"),
        ],
    );
    assert!(
        suggestions(
            &server
                .admin_client()
                .get(&format!("{path}?q=&media=livetv"))
                .await
        )
        .is_empty()
    );
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;
    assert!(suggestions(&viewer.get(&format!("{path}?q=ab&media=livetv")).await).is_empty());
    let kids = kids_client(&server, "kid@test.com").await;
    assert!(suggestions(&kids.get(&format!("{path}?q=ab&media=livetv")).await).is_empty());
}

#[tokio::test]
async fn iptv_suggestions_list_enabled_channels_in_the_configured_languages() {
    let server = TestServer::start_api_only().await;
    sqlx::query(
        "INSERT INTO iptv_channels (name, normalized_name, language, enabled, display_order) VALUES
           ('UK: BBC One HD', 'uk bbc one hd', 'en', true, 2),
           ('BBC Two', 'bbc two', NULL, true, 1),
           ('DE: BBC World', 'de bbc world', 'de', true, 0),
           ('BBC Three', 'bbc three', 'en', false, 0),
           ('Abbcd', 'abbcd', 'en', true, 0)",
    )
    .execute(&server.pool)
    .await
    .unwrap();
    let path = "/api/v1/admin/search/suggestions?q=bbc&media=iptv";

    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;
    for client in [&server.admin_client(), &viewer] {
        assert_eq!(
            suggestions(&client.get(path).await),
            vec![
                s("BBC Two", "iptv", "library"),
                s("UK: BBC One HD", "iptv", "library"),
            ],
        );
    }
    let kids = kids_client(&server, "kid@test.com").await;
    assert!(suggestions(&kids.get(path).await).is_empty());
}

async fn seed_library_movie_with_tmdb(
    server: &TestServer,
    title: &str,
    tmdb_id: i32,
    popularity: f32,
) {
    let id = seed_movie(server, title, Some("PG"), &[], None).await;
    sqlx::query(
        "UPDATE movies SET identifiers = jsonb_build_object('tmdb_id', $2::text) WHERE id = $1",
    )
    .bind(id)
    .bind(tmdb_id.to_string())
    .execute(&server.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tmdb_titles (media_type, tmdb_id, original_title, popularity)
         VALUES ('movie', $1, $2, $3)",
    )
    .bind(tmdb_id)
    .bind(title)
    .bind(popularity)
    .execute(&server.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn titles_the_query_starts_rank_first_and_by_popularity() {
    let server = TestServer::start_api_only().await;
    seed_library_movie_with_tmdb(&server, "Breakin'", 9001, 3.0).await;
    seed_library_movie_with_tmdb(&server, "Breakin' 2: Electric Boogaloo", 9002, 1.5).await;
    seed_movie(&server, "Bread and Tulips", Some("PG"), &[], None).await;
    seed_show(&server, "The Brew Crew", Some("TV-PG"), &[], None).await;
    seed_shelf(
        &server,
        "popular_movies",
        0,
        false,
        &[("movie", "Brest Fortress")],
    )
    .await;
    sqlx::query(
        "INSERT INTO tmdb_titles (media_type, tmdb_id, original_title, title, popularity) VALUES
           ('tv', 1396, 'Breaking Bad', NULL, 450.0),
           ('movie', 50, 'Breaking Dawn', NULL, 60.0),
           ('tv', 60, 'Prison Break', NULL, 300.0)",
    )
    .execute(&server.pool)
    .await
    .unwrap();
    let client = server.admin_client();
    let path = "/api/v1/admin/search/suggestions";
    let breakin = vec![
        s("Breaking Bad", "tv", "tmdb"),
        s("Breaking Dawn", "movie", "tmdb"),
        s("Breakin'", "movie", "library"),
        s("Breakin' 2: Electric Boogaloo", "movie", "library"),
    ];

    assert_eq!(
        suggestions(&client.get(&format!("{path}?q=breakin")).await),
        breakin,
    );
    assert_eq!(
        suggestions(&client.get(&format!("{path}?q=break")).await),
        [breakin.clone(), vec![s("Prison Break", "tv", "tmdb")]].concat(),
    );
    assert_eq!(
        suggestions(&client.get(&format!("{path}?q=bre")).await),
        [
            breakin,
            vec![
                s("Bread and Tulips", "movie", "library"),
                s("Brest Fortress", "movie", "tmdb"),
                s("Prison Break", "tv", "tmdb"),
                s("The Brew Crew", "tv", "library"),
            ],
        ]
        .concat(),
    );
}

#[tokio::test]
async fn sports_suggestions_keep_leagues_apart_from_tv() {
    let server = TestServer::start_api_only().await;
    seed_catalogue(&server).await;
    let league = seed_show(&server, "Breakdance World Series", None, &[], None).await;
    sqlx::query("UPDATE tv_shows SET content_category = 'sport' WHERE id = $1")
        .bind(league)
        .execute(&server.pool)
        .await
        .unwrap();
    let client = server.admin_client();
    let path = "/api/v1/admin/search/suggestions";

    assert_eq!(
        suggestions(&client.get(&format!("{path}?q=break&media=sports")).await),
        vec![s("Breakdance World Series", "sports", "library")],
    );
    assert_eq!(
        suggestions(&client.get(&format!("{path}?q=break&media=tv")).await),
        vec![s("Breaking Bad", "tv", "library")],
    );
    assert_eq!(
        suggestions(&client.get(&format!("{path}?q=break")).await),
        vec![
            s("Breakin'", "movie", "library"),
            s("Breaking Bad", "tv", "library"),
            s("Breakfast at Tiffany's", "movie", "tmdb"),
        ],
    );
    assert_eq!(
        suggestions(&client.get(&format!("{path}?q=break&media=tv,sports")).await),
        vec![
            s("Breakdance World Series", "sports", "library"),
            s("Breaking Bad", "tv", "library"),
        ],
    );
    assert!(suggestions(&client.get(&format!("{path}?q=&media=sports")).await).is_empty());
}
