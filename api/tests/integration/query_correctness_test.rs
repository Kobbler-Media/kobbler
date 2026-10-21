//! Per-user scoping and deterministic row selection.
//!
//! Every test here sets up the *second* one — a second user, a second edition, a
//! second author with the same name — because each of these queries is correct
//! for a library that only ever has one.
//!
//! The determinism tests deliberately insert the row that must *lose* first, so
//! an unordered query returns it in heap order and the assertion fails.

use crate::common::*;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Helpers
// =============================================================================

fn find_book(items: &[Value], book_id: Uuid) -> Option<&Value> {
    items
        .iter()
        .find(|b| b["id"].as_str().is_some_and(|id| id == book_id.to_string()))
}

/// Create a user with the contributor role and log in as them.
async fn user_with_client(server: &TestServer, email: &str) -> (Uuid, TestClient) {
    let id = create_user_with_role(server, email, "contributor").await;
    let resp = server
        .api_client()
        .post_json(
            "/api/v1/admin/login",
            &serde_json::json!({ "email": email, "password": "testpass123" }),
        )
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    let token = json["token"].as_str().expect("no token in login response");
    (id, server.api_client().with_auth(token))
}

async fn list_books(client: &TestClient, filter: &str) -> Vec<Value> {
    let resp = client
        .get(&format!("/api/v1/library/books?filter={}", filter))
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    json["items"].as_array().cloned().unwrap_or_default()
}

async fn set_last_sync(server: &TestServer, device_id: Uuid, seconds_ago: i32) {
    sqlx::query(
        "INSERT INTO device_sync_state (device_id, last_sync_at)
         VALUES ($1, NOW() - ($2::int * INTERVAL '1 second'))
         ON CONFLICT (device_id) DO UPDATE SET last_sync_at = EXCLUDED.last_sync_at",
    )
    .bind(device_id)
    .bind(seconds_ago)
    .execute(&server.pool)
    .await
    .expect("failed to set device_sync_state");
}

async fn insert_author(
    server: &TestServer,
    name: &str,
    created_days_ago: i32,
    monitored: Option<(&str, i32)>,
) -> Uuid {
    let id = Uuid::new_v4();
    let (is_monitored, mode, started_days_ago) = match monitored {
        Some((mode, started)) => (true, mode.to_string(), Some(started)),
        None => (false, "none".to_string(), None),
    };
    sqlx::query(
        r#"
        INSERT INTO authors (id, name, normalized_name, created_at, is_monitored, auto_want_mode,
                             monitoring_started_at)
        VALUES ($1, $2, LOWER($2), NOW() - make_interval(days => $3), $4, $5,
                CASE WHEN $6::int IS NULL THEN NULL ELSE NOW() - make_interval(days => $6::int) END)
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(created_days_ago)
    .bind(is_monitored)
    .bind(mode)
    .bind(started_days_ago)
    .execute(&server.pool)
    .await
    .expect("failed to insert author");
    id
}

async fn insert_series(server: &TestServer, name: &str, created_days_ago: i32) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO series (id, name, sort_key, created_at)
         VALUES ($1, $2, LOWER($2), NOW() - make_interval(days => $3))",
    )
    .bind(id)
    .bind(name)
    .bind(created_days_ago)
    .execute(&server.pool)
    .await
    .expect("failed to insert series");
    id
}

/// Insert a book directly. `identifiers` is merged as-is; `in_library` adds a
/// book_files row, which is what `match_series_discovered_to_library` uses to
/// tell a library book from a series-discovered stub.
async fn insert_book(
    server: &TestServer,
    title: &str,
    created_days_ago: i32,
    author_id: Uuid,
    in_library: bool,
    identifiers: Value,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO books (id, title, want_status, created_at, identifiers)
         VALUES ($1, $2, 'none', NOW() - make_interval(days => $3), $4)",
    )
    .bind(id)
    .bind(title)
    .bind(created_days_ago)
    .bind(&identifiers)
    .execute(&server.pool)
    .await
    .expect("failed to insert book");

    sqlx::query("INSERT INTO book_authors (book_id, author_id) VALUES ($1, $2)")
        .bind(id)
        .bind(author_id)
        .execute(&server.pool)
        .await
        .expect("failed to link author");

    if in_library {
        sqlx::query(
            "INSERT INTO book_files (id, book_id, format, size_bytes, file_path)
             VALUES ($1, $2, 'EPUB', 1024, $3)",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(format!("/tmp/{}.epub", id))
        .execute(&server.pool)
        .await
        .expect("failed to insert book file");
    }

    id
}

async fn add_to_series(server: &TestServer, book_id: Uuid, series_id: Uuid, index: Option<f64>) {
    sqlx::query(
        "INSERT INTO book_series (book_id, series_id, series_index, is_primary)
         VALUES ($1, $2, $3, TRUE)",
    )
    .bind(book_id)
    .bind(series_id)
    .bind(index)
    .execute(&server.pool)
    .await
    .expect("failed to add book to series");
}

async fn book_exists(server: &TestServer, book_id: Uuid) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM books WHERE id = $1")
        .bind(book_id)
        .fetch_one(&server.pool)
        .await
        .unwrap()
        > 0
}

async fn book_identifiers(server: &TestServer, book_id: Uuid) -> Value {
    sqlx::query_scalar::<_, Value>("SELECT identifiers FROM books WHERE id = $1")
        .bind(book_id)
        .fetch_one(&server.pool)
        .await
        .unwrap()
}

// =============================================================================
// #1 — Kobo sync state must be computed from the calling user's own devices
// =============================================================================

#[tokio::test]
async fn test_synced_state_is_scoped_to_the_calling_user() {
    let server = TestServer::start_api_only().await;

    let (_, alice) = user_with_client(&server, "alice_sync@test.com").await;
    let (bob_id, _) = user_with_client(&server, "bob_sync@test.com").await;
    let (bob_device, _) = create_device(&server, bob_id).await;

    // Bob sent this book to his Kobo. Alice never did.
    let book_id = create_book(&server, "Only Bobs Kobo Has This").await;
    entitle_book_to_device(&server, book_id, bob_device).await;

    let all = list_books(&alice, "all").await;
    let row = find_book(&all, book_id).expect("book should be listed for Alice");
    assert_eq!(
        row["synced"], false,
        "a book entitled only to another user's device must not read as Synced for Alice"
    );

    let synced = list_books(&alice, "synced").await;
    assert!(
        find_book(&synced, book_id).is_none(),
        "Alice's ?filter=synced must not return a book only Bob's device has"
    );

    // The book detail badge reads the same state.
    let detail = alice
        .get(&format!("/api/v1/library/books/{}", book_id))
        .await;
    detail.assert_ok();
    let detail: Value = detail.json();
    assert_eq!(
        detail["synced"], false,
        "book detail Synced badge must be Alice's state, not Bob's"
    );
}

#[tokio::test]
async fn test_device_entitlement_still_reads_as_synced_for_its_owner() {
    let server = TestServer::start_api_only().await;

    let (alice_id, alice) = user_with_client(&server, "alice_own@test.com").await;
    let (alice_device, _) = create_device(&server, alice_id).await;

    let book_id = create_book(&server, "Alices Own Kobo Book").await;
    entitle_book_to_device(&server, book_id, alice_device).await;

    let synced = list_books(&alice, "synced").await;
    assert!(
        find_book(&synced, book_id).is_some(),
        "scoping must not break the owner's own DEVICE entitlement"
    );
}

#[tokio::test]
async fn test_pending_sync_ignores_another_users_stale_kobo() {
    let server = TestServer::start_api_only().await;

    let (alice_id, alice) = user_with_client(&server, "alice_stale@test.com").await;
    let (alice_device, _) = create_device(&server, alice_id).await;

    let (bob_id, _) = user_with_client(&server, "bob_stale@test.com").await;
    let (bob_device, _) = create_device(&server, bob_id).await;

    let book_id = create_book(&server, "Not Pending For Alice").await;
    entitle_book(&server, book_id).await;

    // Alice's Kobo has synced since the book was entitled. Bob's has been in a
    // drawer for six months.
    set_last_sync(&server, alice_device, 0).await;
    set_last_sync(&server, bob_device, 180 * 86_400).await;

    let all = list_books(&alice, "all").await;
    let row = find_book(&all, book_id).expect("book should be listed for Alice");
    assert_eq!(
        row["pending_sync"], false,
        "Bob's stale Kobo must not make Alice's library read PENDING"
    );

    let pending = list_books(&alice, "pending").await;
    assert!(
        find_book(&pending, book_id).is_none(),
        "Alice's ?filter=pending must not include a book her own Kobo already has"
    );

    let synced = list_books(&alice, "synced").await;
    assert!(
        find_book(&synced, book_id).is_some(),
        "the book is synced from Alice's point of view"
    );
}

#[tokio::test]
async fn test_pending_sync_still_fires_for_the_users_own_stale_device() {
    let server = TestServer::start_api_only().await;

    let (alice_id, alice) = user_with_client(&server, "alice_pending@test.com").await;
    let (alice_device, _) = create_device(&server, alice_id).await;

    let book_id = create_book(&server, "Pending For Alice").await;
    entitle_book(&server, book_id).await;
    set_last_sync(&server, alice_device, 3_600).await;
    touch_book(&server, book_id).await;

    let pending = list_books(&alice, "pending").await;
    assert!(
        find_book(&pending, book_id).is_some(),
        "a book modified after Alice's own device last synced is still pending"
    );
}

/// The genre listing and the alphabet bar build their sync columns from the same
/// helpers but have no other test, and `first-by-letter` numbers its parameters
/// differently for `#` than for a letter. Both would fail as SQL, not as a
/// wrong answer, so a smoke test is enough.
#[tokio::test]
async fn test_scoped_sync_columns_render_for_genre_and_letter_listings() {
    let server = TestServer::start_api_only().await;
    let (alice_id, alice) = user_with_client(&server, "alice_smoke@test.com").await;
    let (device, _) = create_device(&server, alice_id).await;

    let book_id = create_book(&server, "Alphabet Soup").await;
    // The genre listing is library-only, so the book needs a file.
    upload_epub(&server, book_id, test_epub_bytes()).await;
    entitle_book_to_device(&server, book_id, device).await;
    set_last_sync(&server, device, 0).await;

    let genre_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO genres (id, name, normalized_name)
         VALUES ($1, 'Smoke Test Genre', 'smoke test genre')",
    )
    .bind(genre_id)
    .execute(&server.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO book_genres (book_id, genre_id) VALUES ($1, $2)")
        .bind(book_id)
        .bind(genre_id)
        .execute(&server.pool)
        .await
        .unwrap();

    for filter in ["all", "synced", "pending"] {
        for letter in ["A", "#"] {
            let resp = alice
                .get(&format!(
                    "/api/v1/library/books/first-by-letter?letter={}&filter={}",
                    urlencoding::encode(letter),
                    filter
                ))
                .await;
            assert!(
                resp.status.is_success() || resp.status == reqwest::StatusCode::NOT_FOUND,
                "first-by-letter letter={letter} filter={filter} returned {}",
                resp.status
            );
        }
    }

    let resp = alice
        .get(&format!("/api/v1/library/genres/{}/books", genre_id))
        .await;
    resp.assert_ok();
    let items: Vec<Value> = resp.json();
    let row = items
        .iter()
        .find(|b| b["id"].as_str() == Some(&book_id.to_string()))
        .expect("the genre listing should include the book");
    assert_eq!(row["synced"], true, "Alice's own device has this book");
    assert_eq!(row["pending_sync"], false);
}

// =============================================================================
// #5 — match_series_discovered_to_library merges into an arbitrary library row
// =============================================================================

#[tokio::test]
async fn test_series_match_prefers_the_library_book_at_the_same_series_position() {
    let server = TestServer::start_api_only().await;

    let author = insert_author(&server, "Terry Pratchett", 10, None).await;
    let series = insert_series(&server, "Discworld", 10).await;

    // The omnibus was catalogued first, so an unordered scan reaches it first.
    let omnibus = insert_book(
        &server,
        "The Long War",
        9,
        author,
        true,
        serde_json::json!({}),
    )
    .await;
    add_to_series(&server, omnibus, series, Some(99.0)).await;

    let single_volume = insert_book(
        &server,
        "The Long War",
        8,
        author,
        true,
        serde_json::json!({}),
    )
    .await;
    add_to_series(&server, single_volume, series, Some(2.0)).await;

    let discovered = insert_book(
        &server,
        "The Long War",
        1,
        author,
        false,
        serde_json::json!({"isbn": "9780062067777"}),
    )
    .await;
    add_to_series(&server, discovered, series, Some(2.0)).await;

    let merged = api::repositories::books::match_series_discovered_to_library(&server.pool, series)
        .await
        .expect("match should succeed");
    assert_eq!(merged, 1, "exactly one discovered row should be merged");

    assert_eq!(
        book_identifiers(&server, single_volume).await["isbn"],
        "9780062067777",
        "the ISBN belongs to the volume at the same series position, not the omnibus"
    );
    assert!(
        book_identifiers(&server, omnibus)
            .await
            .get("isbn")
            .is_none(),
        "the omnibus must not acquire the single volume's ISBN"
    );
    assert!(
        !book_exists(&server, discovered).await,
        "the discovered stub is consumed by the merge"
    );
}

#[tokio::test]
async fn test_series_match_falls_back_to_the_oldest_library_book() {
    let server = TestServer::start_api_only().await;

    let author = insert_author(&server, "Iain Banks", 10, None).await;
    let series = insert_series(&server, "Culture", 10).await;

    // Neither library edition is in the series, so only recency separates them.
    // The newer one is inserted first so heap order returns it first.
    let newer = insert_book(
        &server,
        "Use of Weapons",
        2,
        author,
        true,
        serde_json::json!({}),
    )
    .await;
    let older = insert_book(
        &server,
        "Use of Weapons",
        9,
        author,
        true,
        serde_json::json!({}),
    )
    .await;

    let discovered = insert_book(
        &server,
        "Use of Weapons",
        1,
        author,
        false,
        serde_json::json!({"asin": "B002TXZR6Q"}),
    )
    .await;
    add_to_series(&server, discovered, series, Some(3.0)).await;

    api::repositories::books::match_series_discovered_to_library(&server.pool, series)
        .await
        .expect("match should succeed");

    assert_eq!(
        book_identifiers(&server, older).await["asin"],
        "B002TXZR6Q",
        "with nothing else to separate them the oldest library row wins"
    );
    assert!(
        book_identifiers(&server, newer).await.get("asin").is_none(),
        "the newer duplicate must not acquire the ASIN"
    );
}

// =============================================================================
// #9 — "primary monitored author" where no primary flag exists
// =============================================================================

#[tokio::test]
async fn test_series_monitoring_prefers_the_most_permissive_author() {
    let server = TestServer::start_api_only().await;

    let series = insert_series(&server, "The Expanse", 10).await;

    // The 'none' author is inserted first so an unordered LIMIT 1 finds it first.
    let quiet = insert_author(&server, "Ty Franck", 10, Some(("none", 30))).await;
    let eager = insert_author(&server, "Daniel Abraham", 9, Some(("all", 20))).await;

    let book = insert_book(
        &server,
        "Leviathan Wakes",
        5,
        quiet,
        true,
        serde_json::json!({}),
    )
    .await;
    sqlx::query("INSERT INTO book_authors (book_id, author_id) VALUES ($1, $2)")
        .bind(book)
        .bind(eager)
        .execute(&server.pool)
        .await
        .unwrap();
    add_to_series(&server, book, series, Some(1.0)).await;

    let monitoring = api::repositories::authors::get_series_monitoring(&server.pool, series)
        .await
        .expect("query should succeed")
        .expect("a monitored author exists for this series");

    assert_eq!(
        monitoring.auto_want_mode, "all",
        "an author asking for everything outranks one left at the 'none' default"
    );

    let started = monitoring
        .monitoring_started_at
        .expect("the winning author has a monitoring_started_at");
    let expected: OffsetDateTime =
        sqlx::query_scalar("SELECT monitoring_started_at FROM authors WHERE id = $1")
            .bind(eager)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert_eq!(
        started, expected,
        "the timestamp must come from the same author as the mode"
    );
}

#[tokio::test]
async fn test_series_monitoring_breaks_mode_ties_on_earliest_monitored() {
    let server = TestServer::start_api_only().await;

    let series = insert_series(&server, "Wayfarers", 10).await;
    let late = insert_author(&server, "Becky Chambers", 10, Some(("new_only", 5))).await;
    let early = insert_author(&server, "B. Chambers", 9, Some(("new_only", 90))).await;

    let book = insert_book(
        &server,
        "A Closed and Common Orbit",
        5,
        late,
        true,
        serde_json::json!({}),
    )
    .await;
    sqlx::query("INSERT INTO book_authors (book_id, author_id) VALUES ($1, $2)")
        .bind(book)
        .bind(early)
        .execute(&server.pool)
        .await
        .unwrap();
    add_to_series(&server, book, series, Some(2.0)).await;

    let monitoring = api::repositories::authors::get_series_monitoring(&server.pool, series)
        .await
        .unwrap()
        .expect("a monitored author exists");

    let expected: OffsetDateTime =
        sqlx::query_scalar("SELECT monitoring_started_at FROM authors WHERE id = $1")
            .bind(early)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert_eq!(
        monitoring.monitoring_started_at,
        Some(expected),
        "equal modes tie-break on the earliest monitoring_started_at (wants more books)"
    );
}

// =============================================================================
// #12 — name lookups where duplicate names are an explicit feature (mig 0031)
// =============================================================================

#[tokio::test]
async fn test_find_author_by_name_returns_the_oldest_duplicate() {
    let server = TestServer::start_api_only().await;

    // Newer first so heap order returns the row that must lose.
    let newer = insert_author(&server, "John Smith", 1, None).await;
    let older = insert_author(&server, "John Smith", 30, None).await;

    let found = api::repositories::authors::find_by_name(&server.pool, "john smith")
        .await
        .expect("lookup should succeed")
        .expect("an author named John Smith exists");

    assert_eq!(
        found.0, older,
        "a duplicate-name lookup must resolve to the oldest row, matching get_or_create"
    );
    assert_ne!(found.0, newer);
}

#[tokio::test]
async fn test_ingest_edit_links_the_oldest_duplicate_author_and_series() {
    let server = TestServer::start_api_only().await;

    let newer_author = insert_author(&server, "John Smith", 1, None).await;
    let older_author = insert_author(&server, "John Smith", 30, None).await;
    let newer_series = insert_series(&server, "Foundation", 1).await;
    let older_series = insert_series(&server, "Foundation", 30).await;

    let item = add_queue_item(&server, "/tmp/dup.epub", "dup.epub", "EPUB").await;

    let resp = server
        .admin_client()
        .patch_json(
            &format!("/api/v1/library/books/{}", item),
            &serde_json::json!({
                "title": "Duplicate Name Book",
                "authors": [{ "name": "John Smith" }],
                "series_name": "Foundation",
            }),
        )
        .await;
    resp.assert_success();

    let linked_author: Option<Uuid> =
        sqlx::query_scalar("SELECT author_id FROM book_authors WHERE book_id = $1")
            .bind(item)
            .fetch_optional(&server.pool)
            .await
            .unwrap();
    assert_eq!(
        linked_author,
        Some(older_author),
        "an ingest edit must link the oldest author of that name, not an arbitrary one"
    );
    assert_ne!(linked_author, Some(newer_author));

    let linked_series: Option<Uuid> =
        sqlx::query_scalar("SELECT series_id FROM book_series WHERE book_id = $1")
            .bind(item)
            .fetch_optional(&server.pool)
            .await
            .unwrap();
    assert_eq!(
        linked_series,
        Some(older_series),
        "an ingest edit must link the oldest series of that name"
    );
    assert_ne!(linked_series, Some(newer_series));
}
