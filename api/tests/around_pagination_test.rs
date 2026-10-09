use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

const TITLES: [&str; 5] = ["Alpha", "Bravo", "Charlie", "Delta", "Echo"];

async fn create_entity(server: &TestServer, path: &str, title: &str) -> String {
    let resp = server
        .admin_client()
        .post_json(path, &json!({ "title": title, "year": 2024 }))
        .await;
    resp.assert_status(StatusCode::CREATED);
    let json: Value = resp.json();
    json["id"].as_str().unwrap().to_string()
}

async fn seed(server: &TestServer, create_path: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for title in TITLES {
        ids.push(create_entity(server, create_path, title).await);
    }
    ids
}

async fn titles_around(
    server: &TestServer,
    list_path: &str,
    target: &str,
    around_before: i64,
    limit: i64,
) -> Vec<String> {
    let resp = server
        .admin_client()
        .get(&format!(
            "{list_path}?around={target}&around_before={around_before}&limit={limit}"
        ))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap().to_string())
        .collect()
}

async fn assert_around_windows(server: &TestServer, create_path: &str, list_path: &str) {
    let ids = seed(server, create_path).await;
    let charlie = &ids[2];

    assert_eq!(
        titles_around(server, list_path, charlie, 0, 3).await,
        vec!["Charlie", "Delta", "Echo"],
        "around_before=0 must start the window at the around target"
    );

    assert_eq!(
        titles_around(server, list_path, charlie, 2, 4).await,
        vec!["Alpha", "Bravo", "Charlie", "Delta"],
        "around_before=2 must return two leading rows then the target"
    );
}

#[tokio::test]
async fn test_movies_around_before_zero_starts_at_target() {
    let server = TestServer::start_api_only().await;
    assert_around_windows(&server, "/api/v1/admin/movies", "/api/v1/admin/movies").await;
}

#[tokio::test]
async fn test_shows_around_before_zero_starts_at_target() {
    let server = TestServer::start_api_only().await;
    assert_around_windows(&server, "/api/v1/admin/tv/shows", "/api/v1/admin/tv/shows").await;
}

#[tokio::test]
async fn test_leagues_around_before_zero_starts_at_target() {
    let server = TestServer::start_api_only().await;
    assert_around_windows(
        &server,
        "/api/v1/admin/sports/leagues",
        "/api/v1/admin/sports/leagues",
    )
    .await;
}

#[tokio::test]
async fn test_books_around_before_zero_starts_at_target() {
    let server = TestServer::start_api_only().await;
    let mut ids = Vec::new();
    for title in TITLES {
        ids.push(create_book(&server, title).await.to_string());
    }

    assert_eq!(
        titles_around(&server, "/api/v1/library/books", &ids[2], 0, 3).await,
        vec!["Charlie", "Delta", "Echo"],
        "around_before=0 must start the window at the around target"
    );

    assert_eq!(
        titles_around(&server, "/api/v1/library/books", &ids[2], 2, 4).await,
        vec!["Alpha", "Bravo", "Charlie", "Delta"],
        "around_before=2 must return two leading rows then the target"
    );
}

async fn list_page(server: &TestServer, list_path: &str, query: &str) -> Value {
    let resp = server
        .admin_client()
        .get(&format!("{list_path}?{query}"))
        .await;
    resp.assert_ok();
    resp.json()
}

fn titles_of(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap().to_string())
        .collect()
}

fn prev_of(page: &Value) -> Option<&str> {
    page["prev_cursor"].as_str()
}

fn next_of(page: &Value) -> &str {
    page["next_cursor"]
        .as_str()
        .expect("expected a next_cursor on this page")
}

async fn assert_prev_cursor_semantics(server: &TestServer, create_path: &str, list_path: &str) {
    let ids = seed(server, create_path).await;
    let (alpha, charlie, delta) = (&ids[0], &ids[2], &ids[3]);

    let first = list_page(server, list_path, "limit=3").await;
    assert_eq!(titles_of(&first), vec!["Alpha", "Bravo", "Charlie"]);
    assert_eq!(
        prev_of(&first),
        None,
        "a forward page with no cursor is at the start of the collection"
    );

    let second = list_page(
        server,
        list_path,
        &format!("cursor={}&limit=3", next_of(&first)),
    )
    .await;
    assert_eq!(titles_of(&second), vec!["Delta", "Echo"]);
    assert!(
        prev_of(&second).is_some(),
        "a forward page reached by cursor has rows before it"
    );

    let at_target = list_page(
        server,
        list_path,
        &format!("around={charlie}&around_before=0&limit=3"),
    )
    .await;
    assert_eq!(titles_of(&at_target), vec!["Charlie", "Delta", "Echo"]);
    assert!(
        prev_of(&at_target).is_some(),
        "around_before=0 on a target with rows before it must still page backwards"
    );

    let at_start = list_page(
        server,
        list_path,
        &format!("around={alpha}&around_before=0&limit=3"),
    )
    .await;
    assert_eq!(titles_of(&at_start), vec!["Alpha", "Bravo", "Charlie"]);
    assert_eq!(
        prev_of(&at_start),
        None,
        "an around window whose target is the first row of the collection has nothing before it"
    );

    let leading = list_page(
        server,
        list_path,
        &format!("around={delta}&around_before=1&limit=3"),
    )
    .await;
    assert_eq!(titles_of(&leading), vec!["Charlie", "Delta", "Echo"]);
    assert!(
        prev_of(&leading).is_some(),
        "an around window with leading rows that does not reach the start has rows before it"
    );

    let leading_to_start = list_page(
        server,
        list_path,
        &format!("around={charlie}&around_before=2&limit=4"),
    )
    .await;
    assert_eq!(
        titles_of(&leading_to_start),
        vec!["Alpha", "Bravo", "Charlie", "Delta"]
    );
    assert_eq!(
        prev_of(&leading_to_start),
        None,
        "an around window whose leading rows reach the start has nothing before it"
    );
}

async fn assert_backward_paging(server: &TestServer, create_path: &str, list_path: &str) {
    seed(server, create_path).await;

    let p1 = list_page(server, list_path, "limit=2").await;
    assert_eq!(titles_of(&p1), vec!["Alpha", "Bravo"]);
    let p2 = list_page(
        server,
        list_path,
        &format!("cursor={}&limit=2", next_of(&p1)),
    )
    .await;
    assert_eq!(titles_of(&p2), vec!["Charlie", "Delta"]);
    let p3 = list_page(
        server,
        list_path,
        &format!("cursor={}&limit=2", next_of(&p2)),
    )
    .await;
    assert_eq!(titles_of(&p3), vec!["Echo"]);

    let back_from_p3 = list_page(
        server,
        list_path,
        &format!(
            "cursor={}&direction=backward&limit=2",
            prev_of(&p3).expect("page 3 has rows before it")
        ),
    )
    .await;
    assert_eq!(
        titles_of(&back_from_p3),
        vec!["Charlie", "Delta"],
        "a backward page must return the rows immediately before the cursor"
    );
    assert!(
        prev_of(&back_from_p3).is_some(),
        "the backward window still has Alpha and Bravo before it"
    );

    let back_to_start = list_page(
        server,
        list_path,
        &format!(
            "cursor={}&direction=backward&limit=2",
            prev_of(&p2).expect("page 2 has rows before it")
        ),
    )
    .await;
    assert_eq!(titles_of(&back_to_start), vec!["Alpha", "Bravo"]);
    assert_eq!(
        prev_of(&back_to_start),
        None,
        "a backward page that reaches the first row has nothing before it"
    );
}

#[tokio::test]
async fn test_movies_prev_cursor_only_when_rows_precede_window() {
    let server = TestServer::start_api_only().await;
    assert_prev_cursor_semantics(&server, "/api/v1/admin/movies", "/api/v1/admin/movies").await;
}

#[tokio::test]
async fn test_shows_prev_cursor_only_when_rows_precede_window() {
    let server = TestServer::start_api_only().await;
    assert_prev_cursor_semantics(&server, "/api/v1/admin/tv/shows", "/api/v1/admin/tv/shows").await;
}

#[tokio::test]
async fn test_leagues_prev_cursor_only_when_rows_precede_window() {
    let server = TestServer::start_api_only().await;
    assert_prev_cursor_semantics(
        &server,
        "/api/v1/admin/sports/leagues",
        "/api/v1/admin/sports/leagues",
    )
    .await;
}

#[tokio::test]
async fn test_movies_backward_paging_windows() {
    let server = TestServer::start_api_only().await;
    assert_backward_paging(&server, "/api/v1/admin/movies", "/api/v1/admin/movies").await;
}

#[tokio::test]
async fn test_shows_backward_paging_windows() {
    let server = TestServer::start_api_only().await;
    assert_backward_paging(&server, "/api/v1/admin/tv/shows", "/api/v1/admin/tv/shows").await;
}

#[tokio::test]
async fn test_leagues_backward_paging_windows() {
    let server = TestServer::start_api_only().await;
    assert_backward_paging(
        &server,
        "/api/v1/admin/sports/leagues",
        "/api/v1/admin/sports/leagues",
    )
    .await;
}

/// Books order by sort_title (the DB trigger strips the leading article), matching the
/// alphabet bar and first-by-letter. Ordering by the raw title filed "The Bravo" under
/// T while the bar advertised it under B, and a cursor carrying the raw title then
/// disagreed with the ORDER BY, so paging skipped rows.
#[tokio::test]
async fn test_books_sort_by_sort_title_not_raw_title() {
    let server = TestServer::start_api_only().await;
    for title in ["Alpha", "Charlie", "The Bravo"] {
        create_book(&server, title).await;
    }

    let all = list_page(&server, "/api/v1/library/books", "limit=10").await;
    assert_eq!(
        titles_of(&all),
        vec!["Alpha", "The Bravo", "Charlie"],
        "the list must order by sort_title, not the raw title"
    );

    let p1 = list_page(&server, "/api/v1/library/books", "limit=2").await;
    assert_eq!(titles_of(&p1), vec!["Alpha", "The Bravo"]);
    let p2 = list_page(
        &server,
        "/api/v1/library/books",
        &format!("cursor={}&limit=2", next_of(&p1)),
    )
    .await;
    assert_eq!(
        titles_of(&p2),
        vec!["Charlie"],
        "the cursor must carry the same key the query orders by"
    );

    let resp = server
        .admin_client()
        .get("/api/v1/library/books/first-by-letter?letter=B")
        .await;
    resp.assert_ok();
    let target: Value = resp.json();
    assert_eq!(
        target["title"].as_str(),
        Some("The Bravo"),
        "the alphabet bar's jump target must agree with the list order"
    );
}

// =============================================================================
// Alphabet bars without a search index
// =============================================================================

async fn letters_of(server: &TestServer, path: &str) -> Vec<String> {
    let resp = server.admin_client().get(path).await;
    resp.assert_ok();
    resp.json::<Value>()["letters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap().to_string())
        .collect()
}

/// Every bar is served from the search index and falls back to SQL when it is not
/// there — which these tests always are, and which a kobbler whose Meili is down is
/// too. Three of the four used to answer 500 and take the page's alphabet with them.
#[tokio::test]
async fn test_every_alphabet_bar_answers_without_the_search_index() {
    let server = TestServer::start_api_only().await;

    for (create_path, letters_path) in [
        (
            "/api/v1/admin/movies",
            "/api/v1/admin/movies/available-letters",
        ),
        (
            "/api/v1/admin/tv/shows",
            "/api/v1/admin/tv/shows/available-letters",
        ),
        (
            "/api/v1/admin/sports/leagues",
            "/api/v1/admin/sports/leagues/available-letters",
        ),
    ] {
        create_entity(&server, create_path, "Alpha").await;
        create_entity(&server, create_path, "1984").await;

        assert_eq!(
            letters_of(&server, letters_path).await,
            vec!["A".to_string(), "#".to_string()],
            "{letters_path} must list its letters with # last"
        );
    }
}
