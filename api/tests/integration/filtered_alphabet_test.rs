use crate::common::*;
use crate::kids_mode_test::{kids_client, seed_movie, seed_show};
use reqwest::StatusCode;
use serde_json::Value;
use uuid::Uuid;

// Browse pages narrowed by genre (or TV's "Continuing") keep their A–Z bar only if the
// bar and its jump target answer for the same narrowed list.

type Title = (
    &'static str,
    Option<&'static str>,
    &'static [&'static str],
    Option<bool>,
);

const CATALOGUE: [Title; 8] = [
    ("Tango", Some("PG-13"), &["Action"], None),
    ("Tiny Toons", Some("TV-Y7"), &["Family"], None),
    ("Toybox", Some("G"), &["Family", "Animation"], None),
    ("Home Movies", None, &["Family"], Some(true)),
    ("Grim Family", Some("R"), &["Family"], None),
    ("9 Lives", Some("PG"), &["Family"], None),
    ("Webslinger", Some("PG-13"), &["Action"], None),
    ("Alpha", Some("G"), &["Drama"], None),
];

const CONTINUING: [&str; 3] = ["Toybox", "Webslinger", "Grim Family"];

async fn seed_movies(server: &TestServer) {
    for (title, rating, genres, kids_override) in CATALOGUE {
        seed_movie(server, title, rating, genres, kids_override).await;
    }
}

async fn seed_shows(server: &TestServer) {
    for (title, rating, genres, kids_override) in CATALOGUE {
        let id: Uuid = seed_show(server, title, rating, genres, kids_override).await;
        let status = if CONTINUING.contains(&title) {
            "continuing"
        } else {
            "ended"
        };
        sqlx::query("UPDATE tv_shows SET status = $2 WHERE id = $1")
            .bind(id)
            .bind(status)
            .execute(&server.pool)
            .await
            .unwrap();
    }
    seed_show(server, "Bundesliga", None, &["Family"], None).await;
    sqlx::query("UPDATE tv_shows SET content_category = 'sport' WHERE title = 'Bundesliga'")
        .execute(&server.pool)
        .await
        .unwrap();
}

async fn assert_letters(client: &TestClient, path: &str, expected: &[&str]) {
    let resp = client.get(path).await;
    resp.assert_ok();
    let letters: Vec<String> = resp.json::<Value>()["letters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap().to_string())
        .collect();
    assert_eq!(letters, expected, "{path}");
}

async fn assert_first(client: &TestClient, path: &str, expected: Option<&str>) {
    let resp = client.get(path).await;
    match expected {
        Some(title) => {
            resp.assert_ok();
            assert_eq!(
                resp.json::<Value>()["title"].as_str(),
                Some(title),
                "{path}"
            );
        }
        None => {
            resp.assert_status(StatusCode::NOT_FOUND);
        }
    }
}

#[tokio::test]
async fn movie_alphabet_bar_honours_genre() {
    let server = TestServer::start_api_only().await;
    seed_movies(&server).await;
    let admin = server.admin_client();
    let kids = kids_client(&server, "kid@test.com").await;
    let letters = "/api/v1/admin/movies/available-letters";
    let first = "/api/v1/admin/movies/first-by-letter";

    for (client, query, expected) in [
        (&admin, "", &["A", "G", "H", "T", "W", "#"][..]),
        (&admin, "?genre=", &["A", "G", "H", "T", "W", "#"]),
        (&admin, "?genre=Family", &["G", "H", "T", "#"]),
        (&admin, "?genre=fAMILY", &["G", "H", "T", "#"]),
        (&admin, "?genre=Western", &[]),
        (&kids, "", &["A", "H", "T", "W", "#"]),
        (&kids, "?genre=Family", &["H", "T", "#"]),
    ] {
        assert_letters(client, &format!("{letters}{query}"), expected).await;
    }

    for (client, query, expected) in [
        (&admin, "letter=T", Some("Tango")),
        (&admin, "letter=T&genre=Family", Some("Tiny Toons")),
        (&admin, "letter=t&genre=family", Some("Tiny Toons")),
        (&admin, "letter=%23&genre=Family", Some("9 Lives")),
        (&admin, "letter=W&genre=Family", None),
        (&admin, "letter=G&genre=Family", Some("Grim Family")),
        (&kids, "letter=G&genre=Family", None),
        (&kids, "letter=T&genre=Family", Some("Tiny Toons")),
    ] {
        assert_first(client, &format!("{first}?{query}"), expected).await;
    }
}

#[tokio::test]
async fn tv_alphabet_bar_honours_genre_and_status() {
    let server = TestServer::start_api_only().await;
    seed_shows(&server).await;
    let admin = server.admin_client();
    let kids = kids_client(&server, "kid@test.com").await;
    let letters = "/api/v1/admin/tv/shows/available-letters";
    let first = "/api/v1/admin/tv/shows/first-by-letter";

    for (client, query, expected) in [
        (&admin, "?genre=Family", &["G", "H", "T", "#"][..]),
        (&admin, "?status=continuing", &["G", "T", "W"]),
        (&admin, "?status=continuing&genre=Family", &["G", "T"]),
        (&admin, "?status=upcoming", &[]),
        (&kids, "", &["A", "H", "T", "W", "#"]),
        (&kids, "?genre=Family", &["H", "T", "#"]),
        (&kids, "?status=continuing&genre=Family", &["T"]),
    ] {
        assert_letters(client, &format!("{letters}{query}"), expected).await;
    }

    for (client, query, expected) in [
        (&admin, "letter=T", Some("Tango")),
        (&admin, "letter=T&genre=Family", Some("Tiny Toons")),
        (&admin, "letter=T&status=continuing", Some("Toybox")),
        (&admin, "letter=A&status=continuing", None),
        (&admin, "letter=B&genre=Family", None),
        (
            &admin,
            "letter=G&status=continuing&genre=Family",
            Some("Grim Family"),
        ),
        (&kids, "letter=G&status=continuing&genre=Family", None),
        (
            &kids,
            "letter=T&status=continuing&genre=Family",
            Some("Toybox"),
        ),
    ] {
        assert_first(client, &format!("{first}?{query}"), expected).await;
    }
}
