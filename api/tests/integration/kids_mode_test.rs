use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use uuid::Uuid;

// A Kids profile sees only titles rated teen-and-under, or ones an admin has allowed by
// hand. Unrated titles stay out until someone decides about them.

pub(crate) async fn kids_client(server: &TestServer, email: &str) -> TestClient {
    let user_id = create_user_with_role(server, email, "viewer").await;
    server
        .admin_client()
        .patch_json(
            &format!("/api/v1/admin/users/{user_id}"),
            &json!({ "is_kids": true }),
        )
        .await
        .assert_ok();
    login(server, email).await
}

async fn login(server: &TestServer, email: &str) -> TestClient {
    let client = TestClient::new(&server.api_url);
    let resp = client
        .post_json(
            "/api/v1/admin/login",
            &json!({ "email": email, "password": "testpass123" }),
        )
        .await;
    resp.assert_ok();
    let token = resp.json::<Value>()["token"].as_str().unwrap().to_string();
    client.with_auth(&token)
}

pub(crate) async fn seed_movie(
    server: &TestServer,
    title: &str,
    rating: Option<&str>,
    genres: &[&str],
    kids_override: Option<bool>,
) -> Uuid {
    let id = Uuid::new_v4();
    let genres: Vec<String> = genres.iter().map(|g| g.to_string()).collect();
    sqlx::query(
        "INSERT INTO movies (id, title, content_rating, genres, kids_override) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(title)
    .bind(rating)
    .bind(&genres)
    .bind(kids_override)
    .execute(&server.pool)
    .await
    .unwrap();
    create_stub_movie_file(&server.pool, id).await;
    id
}

pub(crate) async fn seed_show(
    server: &TestServer,
    title: &str,
    rating: Option<&str>,
    genres: &[&str],
    kids_override: Option<bool>,
) -> Uuid {
    let id = Uuid::new_v4();
    let genres: Vec<String> = genres.iter().map(|g| g.to_string()).collect();
    sqlx::query(
        "INSERT INTO tv_shows (id, title, content_rating, genres, kids_override) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(title)
    .bind(rating)
    .bind(&genres)
    .bind(kids_override)
    .execute(&server.pool)
    .await
    .unwrap();
    id
}

/// The six cases every catalogue is checked against, and the titles a Kids profile keeps.
async fn seed_catalogue<F, Fut>(seed: F) -> BTreeSet<String>
where
    F: Fn(&'static str, Option<&'static str>, &'static [&'static str], Option<bool>) -> Fut,
    Fut: std::future::Future<Output = Uuid>,
{
    seed("Toybox", Some("G"), &["Family", "Animation"], None).await;
    seed("Webslinger", Some("PG-13"), &["Action"], None).await;
    seed("Tiny Toons", Some("TV-Y7"), &["Family"], None).await;
    seed("Gorefest", Some("R"), &["Horror"], None).await;
    seed("Mystery Reel", None, &["Documentary"], None).await;
    seed("Home Movies", None, &["Family"], Some(true)).await;
    seed("Edgy Cartoon", Some("PG"), &["Comedy"], Some(false)).await;
    ["Toybox", "Webslinger", "Tiny Toons", "Home Movies"]
        .into_iter()
        .map(String::from)
        .collect()
}

fn titles(resp: &TestResponse) -> BTreeSet<String> {
    resp.json::<Value>()["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn kids_profile_lists_only_kids_safe_movies_and_shows() {
    let server = TestServer::start_api_only().await;
    let kids = kids_client(&server, "kid@test.com").await;
    let adult = create_user_client(&server, "parent@test.com", "viewer").await;

    let movies_allowed = seed_catalogue(|t, r, g, o| seed_movie(&server, t, r, g, o)).await;
    let shows_allowed = seed_catalogue(|t, r, g, o| seed_show(&server, t, r, g, o)).await;

    for (path, allowed) in [
        ("/api/v1/admin/movies?is_in_library=true", &movies_allowed),
        ("/api/v1/admin/tv/shows", &shows_allowed),
    ] {
        let resp = kids.get(path).await;
        resp.assert_ok();
        assert_eq!(&titles(&resp), allowed, "kids {path}");

        let resp = adult.get(path).await;
        resp.assert_ok();
        assert_eq!(titles(&resp).len(), 7, "adult {path} sees everything");
    }
}

#[tokio::test]
async fn kids_profile_cannot_open_a_blocked_title() {
    let server = TestServer::start_api_only().await;
    let kids = kids_client(&server, "kid@test.com").await;
    let adult = create_user_client(&server, "parent@test.com", "viewer").await;
    let gore = seed_movie(&server, "Gorefest", Some("R"), &[], None).await;
    let toy = seed_movie(&server, "Toybox", Some("G"), &[], None).await;
    let show = seed_show(&server, "Late Night", Some("TV-MA"), &[], None).await;

    kids.get(&format!("/api/v1/admin/movies/{gore}"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    kids.get(&format!("/api/v1/admin/tv/shows/{show}"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    kids.get(&format!("/api/v1/admin/movies/{toy}"))
        .await
        .assert_ok();
    adult
        .get(&format!("/api/v1/admin/movies/{gore}"))
        .await
        .assert_ok();
    adult
        .get(&format!("/api/v1/admin/tv/shows/{show}"))
        .await
        .assert_ok();
}

#[tokio::test]
async fn kids_profile_cannot_start_playback_of_a_blocked_title() {
    let server = TestServer::start_api_only().await;
    let kids = kids_client(&server, "kid@test.com").await;
    let gore = seed_movie(&server, "Gorefest", Some("R"), &[], None).await;

    let resp = kids
        .post_json(
            "/api/v1/admin/playback/sessions",
            &json!({
                "media_type": "movie",
                "media_id": gore.to_string(),
                "capabilities": { "video_codecs": ["h264"], "audio_codecs": ["aac"], "containers": ["mp4"] },
            }),
        )
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
    assert!(resp.text().contains("Kids profile"), "{}", resp.text());
}

#[tokio::test]
async fn genre_filter_narrows_movie_and_show_lists() {
    let server = TestServer::start_api_only().await;
    let admin = server.admin_client();
    seed_catalogue(|t, r, g, o| seed_movie(&server, t, r, g, o)).await;
    seed_catalogue(|t, r, g, o| seed_show(&server, t, r, g, o)).await;
    let family: BTreeSet<String> = ["Toybox", "Tiny Toons", "Home Movies"]
        .into_iter()
        .map(String::from)
        .collect();

    for path in [
        "/api/v1/admin/movies?is_in_library=true&genre=Family",
        "/api/v1/admin/tv/shows?genre=family",
    ] {
        let resp = admin.get(path).await;
        resp.assert_ok();
        assert_eq!(titles(&resp), family, "{path}");
    }
}

#[tokio::test]
async fn me_and_user_list_report_kids_profiles() {
    let server = TestServer::start_api_only().await;
    let kids = kids_client(&server, "kid@test.com").await;

    let me = kids.get("/api/v1/admin/me").await;
    me.assert_ok();
    assert_eq!(me.json::<Value>()["is_kids"], true);

    let admin_me = server.admin_client().get("/api/v1/admin/me").await;
    assert_eq!(admin_me.json::<Value>()["is_kids"], false);

    let users = server.admin_client().get("/api/v1/admin/users").await;
    users.assert_ok();
    let kid = users
        .json::<Value>()
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["email"] == "kid@test.com")
        .cloned()
        .unwrap();
    assert_eq!(kid["is_kids"], true);
}

/// The override is the admin's call and outlives anything metadata does to the rating.
#[tokio::test]
async fn admin_override_decides_and_survives_a_rating_change() {
    let server = TestServer::start_api_only().await;
    let kids = kids_client(&server, "kid@test.com").await;
    let admin = server.admin_client();
    let reel = seed_movie(&server, "Mystery Reel", None, &[], None).await;
    let show = seed_show(&server, "Unrated Show", None, &[], None).await;

    let visible = |path: &'static str, title: &'static str| {
        let kids = &kids;
        async move { titles(&kids.get(path).await).contains(title) }
    };
    assert!(!visible("/api/v1/admin/movies?is_in_library=true", "Mystery Reel").await);

    admin
        .patch_json(
            &format!("/api/v1/admin/movies/{reel}"),
            &json!({ "kids_override": true }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    admin
        .patch_json(
            &format!("/api/v1/admin/tv/shows/{show}"),
            &json!({ "kids_override": true }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert!(visible("/api/v1/admin/movies?is_in_library=true", "Mystery Reel").await);
    assert!(visible("/api/v1/admin/tv/shows", "Unrated Show").await);

    let detail = admin
        .get(&format!("/api/v1/admin/movies/{reel}"))
        .await
        .json::<Value>();
    assert_eq!(detail["kids_override"], true);
    assert_eq!(detail["kids_allowed"], true);

    admin
        .patch_json(
            &format!("/api/v1/admin/movies/{reel}"),
            &json!({ "content_rating": "R" }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert!(visible("/api/v1/admin/movies?is_in_library=true", "Mystery Reel").await);

    admin
        .patch_json(
            &format!("/api/v1/admin/movies/{reel}"),
            &json!({ "kids_override": null }),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert!(!visible("/api/v1/admin/movies?is_in_library=true", "Mystery Reel").await);
}

#[tokio::test]
async fn only_admins_set_kids_override() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;
    let reel = seed_movie(&server, "Mystery Reel", None, &[], None).await;
    viewer
        .patch_json(
            &format!("/api/v1/admin/movies/{reel}"),
            &json!({ "kids_override": true }),
        )
        .await
        .assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn kids_profile_is_refused_unrated_surfaces() {
    let server = TestServer::start_api_only().await;
    let kids = kids_client(&server, "kid@test.com").await;
    for path in [
        "/api/v1/admin/livetv/channels",
        "/api/v1/admin/iptv/channels",
        "/api/v1/admin/chat/status",
        "/api/v1/admin/video-metadata/search/movies?q=gore",
        "/api/v1/admin/sports/leagues",
    ] {
        let resp = kids.get(path).await;
        resp.assert_status(StatusCode::FORBIDDEN);
        assert!(
            resp.text().contains("Kids profile"),
            "{path}: {}",
            resp.text()
        );
        assert_ne!(
            server.admin_client().get(path).await.status,
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
}

async fn seed_discover_section(
    server: &TestServer,
    key: &str,
    title: &str,
    kids: bool,
    order: i32,
) {
    let section = json!({
        "title": title,
        "section_key": key,
        "sort_order": order,
        "kids": kids,
        "items": [{
            "provider": "tmdb", "provider_id": format!("{key}-1"), "media_type": "movie",
            "title": format!("{title} pick"), "identifiers": {}
        }],
    });
    api::repositories::metadata_cache::set(
        &server.pool,
        "discover",
        key,
        &section,
        time::OffsetDateTime::now_utc() + time::Duration::hours(1),
    )
    .await
    .unwrap();
}

/// Discover shelves are shared by the household; Kids profiles get the ones built for them.
#[tokio::test]
async fn kids_profile_gets_kids_discover_shelves_and_safe_recent_titles() {
    let server = TestServer::start_api_only().await;
    let kids = kids_client(&server, "kid@test.com").await;
    let adult = create_user_client(&server, "parent@test.com", "viewer").await;
    seed_discover_section(&server, "trending_movies", "Trending Movies", false, 0).await;
    seed_discover_section(&server, "kids_movies", "Family Movies", true, 1).await;
    seed_movie(&server, "Toybox", Some("G"), &[], None).await;
    seed_movie(&server, "Gorefest", Some("R"), &[], None).await;

    let shelves = |resp: &TestResponse| -> Vec<String> {
        resp.json::<Value>()["sections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["section_key"].as_str().unwrap().to_string())
            .collect()
    };
    let recent = |resp: &TestResponse| -> BTreeSet<String> {
        resp.json::<Value>()["recently_added"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["title"].as_str().unwrap().to_string())
            .collect()
    };

    let resp = kids.get("/api/v1/admin/discover").await;
    resp.assert_ok();
    assert_eq!(shelves(&resp), vec!["kids_movies"]);
    assert!(recent(&resp).contains("Toybox"));
    assert!(!recent(&resp).contains("Gorefest"));

    let resp = adult.get("/api/v1/admin/discover").await;
    resp.assert_ok();
    assert_eq!(shelves(&resp), vec!["trending_movies"]);
    assert!(recent(&resp).contains("Gorefest"));
}

/// Search hits are filtered after the index answers: the index knows nothing of profiles.
#[tokio::test]
async fn kids_search_filter_drops_blocked_titles() {
    use api::search::{MovieDocument, SearchHit, UnifiedSearchResponse};

    let server = TestServer::start_api_only().await;
    let toy = seed_movie(&server, "Zebracorn Toybox", Some("G"), &[], None).await;
    let gore = seed_movie(&server, "Zebracorn Gorefest", Some("R"), &[], None).await;
    let hit = |id: Uuid, title: &str| {
        SearchHit::Movie(MovieDocument {
            id: id.to_string(),
            title: title.into(),
            year: None,
            directors: vec![],
            genres: vec![],
            poster_url: None,
            is_in_library: true,
            want_status: "none".into(),
            first_letter: "Z".into(),
            rating: None,
            runtime: None,
            content_rating: None,
            description: None,
        })
    };
    let mut response = UnifiedSearchResponse {
        top_hits: vec![hit(gore, "Zebracorn Gorefest")],
        remaining: vec![
            hit(toy, "Zebracorn Toybox"),
            hit(Uuid::new_v4(), "Not In Library"),
        ],
    };

    api::kids::filter_search(&server.pool, &mut response)
        .await
        .unwrap();

    let titles: Vec<String> = response
        .top_hits
        .iter()
        .chain(&response.remaining)
        .map(|h| match h {
            SearchHit::Movie(m) => m.title.clone(),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(titles, vec!["Zebracorn Toybox".to_string()]);
}

async fn seed_episode(server: &TestServer, show_id: Uuid, air_date: time::Date) -> Uuid {
    let season_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order) VALUES ($1, $2, '1', 1)",
    )
    .bind(season_id)
    .bind(show_id)
    .execute(&server.pool)
    .await
    .unwrap();
    let episode_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, air_date)
         VALUES ($1, $2, '1', 1, $3)",
    )
    .bind(episode_id)
    .bind(season_id)
    .bind(air_date)
    .execute(&server.pool)
    .await
    .unwrap();
    episode_id
}

#[tokio::test]
async fn kids_profile_cannot_open_episodes_or_calendar_entries_of_a_blocked_show() {
    let server = TestServer::start_api_only().await;
    let kids = kids_client(&server, "kid@test.com").await;
    let today = time::OffsetDateTime::now_utc().date();
    let late = seed_show(&server, "Late Night", Some("TV-MA"), &[], None).await;
    let cartoon = seed_show(&server, "Cartoon Hour", Some("TV-Y"), &[], None).await;
    let late_ep = seed_episode(&server, late, today).await;
    seed_episode(&server, cartoon, today).await;
    sqlx::query("UPDATE tv_shows SET is_monitored = TRUE WHERE id = ANY($1)")
        .bind(vec![late, cartoon])
        .execute(&server.pool)
        .await
        .unwrap();

    kids.get(&format!("/api/v1/admin/tv/episodes/{late_ep}"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    server
        .admin_client()
        .get(&format!("/api/v1/admin/tv/episodes/{late_ep}"))
        .await
        .assert_ok();

    let calendar = format!(
        "/api/v1/admin/calendar?from={today}&to={}",
        today.next_day().unwrap()
    );
    let shown = |resp: &TestResponse| -> BTreeSet<String> {
        resp.json::<Value>()["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["title"].as_str().unwrap().to_string())
            .collect()
    };
    let resp = kids.get(&calendar).await;
    resp.assert_ok();
    assert_eq!(shown(&resp), BTreeSet::from(["Cartoon Hour".to_string()]));
    let resp = server.admin_client().get(&calendar).await;
    assert_eq!(shown(&resp).len(), 2);
}

/// Outside the library a title is only playable from a Kids Discover shelf.
#[tokio::test]
async fn kids_profile_streams_only_titles_from_its_shelves() {
    let server = TestServer::start_api_only().await;
    let kids = kids_client(&server, "kid@test.com").await;
    seed_discover_section(&server, "kids_movies", "Family Movies", true, 0).await;
    seed_discover_section(&server, "trending_movies", "Trending Movies", false, 1).await;
    let kids_message = |resp: &TestResponse| resp.text().contains("Kids profile");

    let shelf_detail = kids
        .get("/api/v1/admin/discover/movie/tmdb/kids_movies-1")
        .await;
    assert!(!kids_message(&shelf_detail), "{}", shelf_detail.text());
    let other_detail = kids
        .get("/api/v1/admin/discover/movie/tmdb/trending_movies-1")
        .await;
    other_detail.assert_status(StatusCode::NOT_FOUND);
    assert!(kids_message(&other_detail));

    let search = kids
        .post_json(
            "/api/v1/admin/stream/search",
            &json!({ "tmdb_id": 999, "media_type": "movie", "title": "Gorefest" }),
        )
        .await;
    search.assert_status(StatusCode::NOT_FOUND);
    assert!(kids_message(&search));

    let gore = seed_movie(&server, "Gorefest", Some("R"), &[], None).await;
    let search = kids
        .post_json("/api/v1/admin/stream/search", &json!({ "movie_id": gore }))
        .await;
    assert!(kids_message(&search));
    let toy = seed_movie(&server, "Toybox", Some("G"), &[], None).await;
    let search = kids
        .post_json("/api/v1/admin/stream/search", &json!({ "movie_id": toy }))
        .await;
    assert!(!kids_message(&search), "{}", search.text());
}

/// The Categories picker: every genre the profile can actually see, with how many titles.
#[tokio::test]
async fn genre_lists_count_only_what_the_profile_can_see() {
    let server = TestServer::start_api_only().await;
    let kids = kids_client(&server, "kid@test.com").await;
    seed_catalogue(|t, r, g, o| seed_movie(&server, t, r, g, o)).await;
    seed_catalogue(|t, r, g, o| seed_show(&server, t, r, g, o)).await;

    let genres = |resp: &TestResponse| -> Vec<(String, i64)> {
        resp.json::<Value>()["genres"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| {
                (
                    g["name"].as_str().unwrap().to_string(),
                    g["count"].as_i64().unwrap(),
                )
            })
            .collect()
    };
    for path in [
        "/api/v1/admin/movies/genres",
        "/api/v1/admin/tv/shows/genres",
    ] {
        let adult = server.admin_client().get(path).await;
        adult.assert_ok();
        assert_eq!(
            genres(&adult),
            vec![
                ("Action".to_string(), 1),
                ("Animation".to_string(), 1),
                ("Comedy".to_string(), 1),
                ("Documentary".to_string(), 1),
                ("Family".to_string(), 3),
                ("Horror".to_string(), 1),
            ],
            "{path}"
        );
        let resp = kids.get(path).await;
        resp.assert_ok();
        assert_eq!(
            genres(&resp),
            vec![
                ("Action".to_string(), 1),
                ("Animation".to_string(), 1),
                ("Family".to_string(), 3),
            ],
            "kids {path}"
        );
    }
}

/// Each genre tile carries the backdrop of a random title in it the profile can see.
#[tokio::test]
async fn genre_backdrops_come_from_visible_titles_with_one() {
    let server = TestServer::start_api_only().await;
    let kids = kids_client(&server, "kid@test.com").await;
    let cached = json!({ "backdrop": { "cached_at": "2024-01-01T00:00:00Z" } });
    let sourced = json!({ "backdrop": { "source_url": "https://img.test/b.jpg" } });
    let unusable = json!({ "backdrop": { "source_url": "" }, "poster": { "cached_at": "2024-01-01T00:00:00Z" } });

    for (table, base) in [
        ("movies", "/api/v1/admin/movies"),
        ("tv_shows", "/api/v1/admin/tv/shows"),
    ] {
        let seed = |title: &'static str,
                    rating: &'static str,
                    genres: &'static [&'static str],
                    images: Value| {
            let server = &server;
            async move {
                let id = if table == "movies" {
                    seed_movie(server, title, Some(rating), genres, None).await
                } else {
                    seed_show(server, title, Some(rating), genres, None).await
                };
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "UPDATE {table} SET images = $1 WHERE id = $2"
                )))
                .bind(&images)
                .bind(id)
                .execute(&server.pool)
                .await
                .unwrap();
                format!("{base}/{id}/backdrop")
            }
        };
        let toybox = seed("Toybox", "G", &["Family"], cached.clone()).await;
        let toons = seed("Tiny Toons", "TV-Y7", &["Family"], sourced.clone()).await;
        seed("Blank Family", "G", &["Family"], unusable.clone()).await;
        let gore = seed("Gorefest", "R", &["Horror"], cached.clone()).await;
        seed("Teddy Scare", "G", &["Horror"], json!({})).await;
        seed("Reel", "G", &["Documentary"], unusable.clone()).await;

        let path = format!("{base}/genres");
        let admin = server.admin_client();
        for _ in 0..10 {
            for (client, who) in [(&admin, "adult"), (&kids, "kids")] {
                let resp = client.get(&path).await;
                resp.assert_ok();
                let body = resp.json::<Value>();
                let backdrop = |name: &str| -> Value {
                    body["genres"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|g| g["name"] == name)
                        .unwrap_or_else(|| panic!("{who} {path}: no {name} in {body}"))["backdrop_url"]
                        .clone()
                };
                let family = backdrop("Family");
                assert!(
                    family == json!(toybox) || family == json!(toons),
                    "{who} {path}: Family backdrop {family}"
                );
                let horror = if who == "adult" {
                    json!(gore)
                } else {
                    Value::Null
                };
                assert_eq!(backdrop("Horror"), horror, "{who} {path}");
                assert_eq!(backdrop("Documentary"), Value::Null, "{who} {path}");
            }
        }
    }
}

/// "Now available" lands in every household inbox, but a Kids profile only hears about
/// titles it could open.
#[tokio::test]
async fn kids_inbox_only_hears_about_titles_it_can_open() {
    let server = TestServer::start_api_only().await;
    let kids = kids_client(&server, "kid@test.com").await;
    let adult = create_user_client(&server, "parent@test.com", "viewer").await;
    let gore = seed_movie(&server, "Gorefest", Some("R"), &[], None).await;
    let toy = seed_movie(&server, "Toybox", Some("G"), &[], None).await;
    for (id, title) in [(gore, "Gorefest"), (toy, "Toybox")] {
        api::notifications::in_app::notify_household(
            &server.pool,
            &api::notifications::in_app::InAppNotification {
                title: "Movie available".into(),
                message: format!("{title} is now available"),
                entity_type: Some("movie".into()),
                entity_id: Some(id),
                entity_url: None,
            },
        )
        .await
        .unwrap();
    }
    let inbox = |resp: &TestResponse| -> BTreeSet<String> {
        resp.json::<Value>()["notifications"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["message"].as_str().unwrap().to_string())
            .collect()
    };
    let resp = kids.get("/api/v1/admin/notifications/inbox").await;
    resp.assert_ok();
    assert_eq!(
        inbox(&resp),
        BTreeSet::from(["Toybox is now available".to_string()])
    );
    assert_eq!(
        inbox(&adult.get("/api/v1/admin/notifications/inbox").await).len(),
        2
    );
}

#[tokio::test]
async fn kids_profile_has_no_activity_log() {
    let server = TestServer::start_api_only().await;
    let kids = kids_client(&server, "kid@test.com").await;
    kids.get("/api/v1/library/activity")
        .await
        .assert_status(StatusCode::FORBIDDEN);
    server
        .admin_client()
        .get("/api/v1/library/activity")
        .await
        .assert_ok();
}
