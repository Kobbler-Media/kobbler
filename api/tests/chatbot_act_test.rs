//! The half where the bot does things.
//!
//! Every test here is really one of three questions:
//!
//! * **can a viewer make it act?** (no, and the refusal happens before the write)
//! * **can it act without being confirmed?** (no, once, and not tomorrow)
//! * **does it act on the right thing?** (the `MediaRef` decides, never the model)
//!
//! Downloading the wrong show is the failure that ends the trial, so the
//! assertions are about side effects — rows written, want statuses flipped —
//! rather than about what the observation says happened.
//!
//! No LLM, no GPU, no network beyond a wiremock standing in for Kobload.

#![allow(clippy::expect_fun_call)]
#![allow(dead_code)]
#![allow(unused_imports)]
// The shared `tests/common/` harness trips this; every test target that pulls it
// in allows it at the crate root rather than each one rewriting the harness.
#![allow(clippy::collapsible_if)]

mod common;

use api::AppState;
use api::chatbot::tools::act;
use api::chatbot::tools::dispatch::{self, Ctx};
use api::chatbot::tools::read::ToolOutcome;
use api::repositories::chat_sessions::{self, NewSession};
use api::routes::AuthUser;
use common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// =============================================================================
// Harness
// =============================================================================

/// Every act tool goes through `dispatch`, because that is where the role check
/// lives. Calling `act::*` directly would test the tool and skip the gate.
async fn call(
    state: &AppState,
    auth: &AuthUser,
    session_id: Uuid,
    tool: &str,
    args: Value,
) -> ToolOutcome {
    let ctx = Ctx {
        state,
        auth,
        session_id,
        context_ref: None,
    };
    dispatch::dispatch(&ctx, tool, &args).await
}

/// A session with no conversation address — the Command Palette's shape.
async fn web_session(state: &AppState, user_id: Uuid) -> Uuid {
    chat_sessions::create_session(
        &state.pool,
        NewSession {
            transport: "web",
            space: None,
            thread: None,
            user_id: Some(user_id),
        },
    )
    .await
    .expect("session")
    .id
}

/// A session with a real address — Google Chat's shape, and the only one a
/// watch can be registered against.
async fn addressed_session(state: &AppState, user_id: Uuid) -> Uuid {
    chat_sessions::create_session(
        &state.pool,
        NewSession {
            transport: "google_chat",
            space: Some("spaces/AAA"),
            thread: Some("threads/BBB"),
            user_id: Some(user_id),
        },
    )
    .await
    .expect("session")
    .id
}

fn auth(user_id: Uuid, role: &str) -> AuthUser {
    AuthUser {
        user_id,
        role: role.to_string(),
    }
}

/// A `resolve_media` result, persisted the way the loop persists one, so an act
/// tool can find the candidate its ref came from.
async fn persist_resolution(state: &AppState, session_id: Uuid, candidate: Value) -> Uuid {
    let message_id =
        chat_sessions::append_message(&state.pool, session_id, "assistant", "", "running")
            .await
            .expect("assistant message");
    let steps = json!([{
        "index": 0,
        "call_id": "r1",
        "tool": "resolve_media",
        "arguments": "{}",
        "summary": "resolving",
        "state": "done",
        "detail": "",
        "observation": {"ok": true, "resolution": "single", "candidates": [candidate]},
    }]);
    chat_sessions::finish_message(&state.pool, message_id, "", &steps, "complete")
        .await
        .expect("steps");
    message_id
}

/// There is no confirmation step: the call acts. Kept as a named helper so the
/// tests read as "the user asked for this".
async fn confirmed(
    state: &AppState,
    auth: &AuthUser,
    session_id: Uuid,
    tool: &str,
    args: Value,
) -> ToolOutcome {
    call(state, auth, session_id, tool, args).await
}

// =============================================================================
// Fixtures
// =============================================================================

async fn create_show(server: &TestServer, title: &str) -> Uuid {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/tv/shows",
            &json!({"title": title, "year": 2024}),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
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

async fn create_movie(server: &TestServer, title: &str) -> Uuid {
    let resp = server
        .admin_client()
        .post_json(
            "/api/v1/admin/movies",
            &json!({"title": title, "year": 2024}),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

async fn create_book(server: &TestServer, title: &str) -> Uuid {
    let resp = server
        .admin_client()
        .post_json("/api/v1/admin/wanted", &json!({"title": title}))
        .await;
    resp.assert_ok();
    resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

async fn episode_want_status(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar::<_, String>("SELECT want_status FROM tv_episodes WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn count(pool: &sqlx::PgPool, sql: &'static str, id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>(sql)
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Point kobbler at a wiremock Kobload that returns `releases` for any search
/// and accepts any download.
async fn kobload_returning(server: &TestServer, releases: Value) -> MockServer {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": releases})))
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/downloads"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": Uuid::now_v7(), "status": "queued"})),
        )
        .mount(&mock)
        .await;
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "kobload_base_url": mock.uri(),
                "kobload_api_key": "test-key",
                "kobload_ingest_url": format!("{}/api/v1/ingest/{}", server.api_url, server.ingest_token),
            }),
        )
        .await
        .assert_ok();
    mock
}

fn release(title: &str, hash: &str, seeders: i64) -> Value {
    json!({
        "title": title,
        "info_hash": hash,
        "protocol": "torrent",
        "download_url": format!("magnet:?xt=urn:btih:{hash}"),
        "size": 1_000_000_000i64,
        "seeders": seeders,
        "file_name": format!("{title}.mkv"),
    })
}

// =============================================================================
// The phrasing the tool descriptions promise
// =============================================================================

/// British "series 3" means season 3, and the act tools depend on it: the ref
/// they are handed is only as good as the season the resolver parsed.
///
/// The model fills the `season` slot itself (measured 5/5 on the live box), but
/// the resolver re-parses the query text as well, so the path survives the model
/// dropping the slot — which it has done before.
#[test]
fn british_series_numbering_resolves_to_a_season() {
    use api::chatbot::resolve::MediaTypeHint;
    use api::chatbot::resolve::phrasing;

    for query in [
        "series 3 of someShow",
        "can you get series 3 of someShow",
        "season 3 of someShow",
    ] {
        let parsed = phrasing::parse(query);
        assert_eq!(parsed.season, Some(3), "{query}");
        assert_eq!(parsed.episode, None, "{query}");
        assert_eq!(
            parsed.media_type,
            Some(MediaTypeHint::Season),
            "{query} must not resolve to a book series"
        );
    }
}

// =============================================================================
// Role gating — constraint 5
// =============================================================================

/// Every act tool, refused for a viewer, with nothing written on the way past.
///
/// The check is in `dispatch`, so a refusal cannot have had a side effect —
/// this asserts that rather than trusting it.
#[tokio::test]
async fn a_viewer_is_refused_every_act_tool_and_writes_nothing() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let viewer_id = create_user_with_role(&server, "viewer@example.com", "viewer").await;
    let viewer = auth(viewer_id, "viewer");
    let session = web_session(state, viewer_id).await;

    let show = create_show(&server, "someShow").await;
    let season = create_season(&state.pool, show, 24).await;
    let episode = create_episode(&state.pool, season, 1).await;
    let episode_ref = format!("episode:{episode}");

    for (tool, args) in [
        (
            "want_it",
            json!({"ref": episode_ref, "confirm": "yes", "confirm_token": "anything"}),
        ),
        ("list_releases", json!({"ref": episode_ref})),
        (
            "grab_release",
            json!({"ref": episode_ref, "release_index": 0}),
        ),
        ("regrab", json!({"ref": episode_ref})),
        ("cancel_download", json!({"ref": episode_ref})),
        ("add_to_watchlist", json!({"ref": format!("show:{show}")})),
    ] {
        let outcome = call(state, &viewer, session, tool, args).await;
        assert!(!outcome.ok, "{tool} ran for a viewer");
        assert!(
            outcome.detail.contains("permission"),
            "{tool} refused for the wrong reason: {}",
            outcome.detail
        );
    }

    assert_eq!(episode_want_status(&state.pool, episode).await, "none");
    assert_eq!(
        count(
            &state.pool,
            "SELECT COUNT(*) FROM wanted_attempts WHERE episode_id = $1",
            episode
        )
        .await,
        0,
        "a refused tool wrote a download attempt"
    );
    assert_eq!(
        count(
            &state.pool,
            "SELECT COUNT(*) FROM watchlist_items WHERE user_id = $1",
            viewer_id
        )
        .await,
        0,
        "a refused tool wrote a watchlist row"
    );
}

/// Muting yourself needs no privilege. A bot a viewer cannot silence is a bot
/// she blocks in week one.
#[tokio::test]
async fn a_viewer_may_still_mute_itself() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let viewer_id = create_user_with_role(&server, "quiet@example.com", "viewer").await;
    let viewer = auth(viewer_id, "viewer");
    let session = web_session(state, viewer_id).await;

    let outcome = call(
        state,
        &viewer,
        session,
        "set_notification_prefs",
        json!({"action": "mute", "duration": "indefinitely"}),
    )
    .await;
    assert!(outcome.ok, "{}", outcome.detail);
    assert_eq!(outcome.observation["enabled"], json!(false));

    let enabled: bool =
        sqlx::query_scalar("SELECT enabled FROM chat_notification_prefs WHERE user_id = $1")
            .bind(viewer_id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert!(!enabled);

    let back = call(
        state,
        &viewer,
        session,
        "set_notification_prefs",
        json!({"action": "resume"}),
    )
    .await;
    assert!(back.ok);
    assert_eq!(back.observation["enabled"], json!(true));
}

/// "shut up for a couple of hours" is a timed mute, not an off switch.
#[tokio::test]
async fn a_timed_mute_sets_an_end_rather_than_disabling() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "twohours@example.com", "viewer").await;
    let session = web_session(state, user).await;

    let outcome = call(
        state,
        &auth(user, "viewer"),
        session,
        "set_notification_prefs",
        json!({"action": "mute", "duration": "a_couple_of_hours"}),
    )
    .await;
    assert!(outcome.ok, "{}", outcome.detail);
    assert_eq!(outcome.observation["enabled"], json!(true));
    assert!(outcome.observation["muted_until"].is_i64());
    assert!(
        outcome.observation["say"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "every action needs a terminal message"
    );
}

/// The tool's argument surface is `notifications::prefs`'s, not a second copy of
/// it — a word the schema accepts that the logic cannot apply is a dead end the
/// model will find.
#[test]
fn the_prefs_schema_is_the_one_the_prefs_module_publishes() {
    use api::chatbot::tools::Tool;

    let schema = Tool::SetNotificationPrefs.schema();
    assert_eq!(
        schema["function"]["parameters"],
        api::notifications::prefs::tool_parameters()
    );
    assert_eq!(
        schema["function"]["description"],
        json!(api::notifications::prefs::tool_description())
    );
}

// =============================================================================
// Never act on a guess — constraint 6
// =============================================================================

/// A ref the resolver did not stamp is refused outright. The model must not be
/// able to type a title and have something downloaded.
#[tokio::test]
async fn a_ref_the_resolver_did_not_produce_is_refused_with_no_side_effect() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "c7@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = web_session(state, user).await;

    for bad in ["someShow season 24", "episode:not-a-uuid", "", "widget:1"] {
        let outcome = call(state, &who, session, "want_it", json!({"ref": bad})).await;
        assert!(!outcome.ok, "{bad} was accepted as a ref");
        assert!(
            outcome.observation.get("pending_confirmation").is_none(),
            "{bad} produced a confirmation"
        );
    }
    let missing = call(state, &who, session, "want_it", json!({})).await;
    assert!(!missing.ok);
    assert!(
        missing.detail.contains("resolve_media"),
        "{}",
        missing.detail
    );

    assert_eq!(
        count(
            &state.pool,
            "SELECT COUNT(*) FROM wanted_attempts WHERE user_id = $1",
            user
        )
        .await,
        0
    );
}

/// "We don't have this show at all" is not one request, it is several. Asking
/// which season is the whole point — picking one is the failure that ends the
/// trial. The ask lands BEFORE the confirmation gate (no offer, no token), so
/// this test drives a plain call rather than the offer/confirm round trip.
#[tokio::test]
async fn a_show_we_do_not_have_asks_which_season_rather_than_picking() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "c8@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = web_session(state, user).await;

    persist_resolution(
        state,
        session,
        json!({"ref": "provider:tmdb:1396:show", "title": "someShow", "in_library": false}),
    )
    .await;

    let outcome = call(
        state,
        &who,
        session,
        "want_it",
        json!({"ref": "provider:tmdb:1396:show"}),
    )
    .await;
    assert_eq!(outcome.observation["needs_answer"], json!(true));
    assert_eq!(outcome.observation["target"], json!("someShow"));
    assert!(outcome.observation.get("done").is_none());
    assert_eq!(
        count(
            &state.pool,
            "SELECT COUNT(*) FROM tv_shows WHERE $1 = $1",
            user
        )
        .await,
        0,
        "asking which season created a show anyway"
    );
}

/// The resolver offers the show because several shows share the name, and the
/// model picked the right one off the year. The season number rides along as
/// an argument and the tool composes the season ref the model would otherwise
/// have to invent — the `provider:tmdb:243360:season:2` failure, closed.
#[tokio::test]
async fn a_provider_show_ref_plus_a_season_composes_the_season_ref() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "compose@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = web_session(state, user).await;

    persist_resolution(
        state,
        session,
        json!({
            "ref": "provider:tmdb:243360:show",
            "title": "Ludwig",
            "in_library": false,
            "provider_payload": {"title": "Ludwig", "year": 2024},
        }),
    )
    .await;

    let outcome = confirmed(
        state,
        &who,
        session,
        "want_it",
        json!({"ref": "provider:tmdb:243360:show", "season": 2}),
    )
    .await;
    assert!(
        !outcome.detail.contains("I do not know how to get"),
        "the composed season ref fell off the routing table: {}",
        outcome.detail
    );
}

// =============================================================================
// Routing — the MediaRef decides, never the model
// =============================================================================

/// Each local ref shape lands on its own acquisition path, observed by the row
/// it changed rather than by what the tool said.
#[tokio::test]
async fn want_it_routes_each_local_ref_shape_to_the_right_place() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "route@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = web_session(state, user).await;

    // episode
    let show = create_show(&server, "someShow").await;
    let season = create_season(&state.pool, show, 24).await;
    let episode = create_episode(&state.pool, season, 1).await;
    let out = confirmed(
        state,
        &who,
        session,
        "want_it",
        json!({"ref": format!("episode:{episode}")}),
    )
    .await;
    assert!(out.ok, "{}", out.detail);
    assert_eq!(episode_want_status(&state.pool, episode).await, "wanted");

    // season — flips every missing episode, not just one
    let season2 = create_season(&state.pool, show, 25).await;
    let e1 = create_episode(&state.pool, season2, 1).await;
    let e2 = create_episode(&state.pool, season2, 2).await;
    let out = confirmed(
        state,
        &who,
        session,
        "want_it",
        json!({"ref": format!("season:{season2}")}),
    )
    .await;
    assert!(out.ok, "{}", out.detail);
    assert_eq!(episode_want_status(&state.pool, e1).await, "wanted");
    assert_eq!(episode_want_status(&state.pool, e2).await, "wanted");

    // movie
    let movie = create_movie(&server, "someFilm").await;
    let out = confirmed(
        state,
        &who,
        session,
        "want_it",
        json!({"ref": format!("movie:{movie}")}),
    )
    .await;
    assert!(out.ok, "{}", out.detail);
    let want: String = sqlx::query_scalar("SELECT want_status FROM movies WHERE id = $1")
        .bind(movie)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(want, "wanted");

    // book
    let book = create_book(&server, "someBook").await;
    let out = confirmed(
        state,
        &who,
        session,
        "want_it",
        json!({"ref": format!("book:{book}")}),
    )
    .await;
    assert!(out.ok, "{}", out.detail);
    let want: String = sqlx::query_scalar("SELECT want_status FROM books WHERE id = $1")
        .bind(book)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(want, "wanted");
}

/// An `author:` or `series:` ref means "everything new by this" — the same tool,
/// a different ref, against machinery that already exists.
#[tokio::test]
async fn an_author_or_series_ref_turns_on_monitoring_rather_than_downloading() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "monitor@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = web_session(state, user).await;

    // Through the repository, not raw SQL: `authors.normalized_name` is NOT NULL
    // and it is the repository that derives it.
    let author =
        api::repositories::authors::get_or_create_by_name(&state.pool, "Brandon Sanderson")
            .await
            .unwrap();

    let out = confirmed(
        state,
        &who,
        session,
        "want_it",
        json!({"ref": format!("author:{author}")}),
    )
    .await;
    assert!(out.ok, "{}", out.detail);
    assert_eq!(out.observation["monitoring"], json!("author"));
    let (monitored, mode): (bool, String) =
        sqlx::query_as("SELECT is_monitored, auto_want_mode FROM authors WHERE id = $1")
            .bind(author)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert!(monitored);
    assert_eq!(mode, "new_only");

    let series = api::repositories::series::get_or_create_with_author_overlap(
        &state.pool,
        "Mistborn",
        &[author],
        None,
    )
    .await
    .unwrap()
    .expect("series");
    let out = confirmed(
        state,
        &who,
        session,
        "want_it",
        json!({"ref": format!("series:{series}")}),
    )
    .await;
    assert!(out.ok, "{}", out.detail);
    let monitored: bool = sqlx::query_scalar("SELECT is_monitored FROM series WHERE id = $1")
        .bind(series)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert!(monitored);
}

/// A book nobody has yet is created and wanted through `/admin/wanted`, which
/// is the books' not-in-library path.
#[tokio::test]
async fn a_provider_book_ref_creates_the_wanted_record_with_the_right_format() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "newbook@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = web_session(state, user).await;

    let out = confirmed(
        state,
        &who,
        session,
        "want_it",
        json!({
            "ref": "provider:hardcover:99001:book",
            "format": "audiobook",
            "provider_payload": {
                "title": "The Way of Kings",
                "authors": ["Brandon Sanderson"],
                "identifiers": {"hardcover": "99001"},
            },
        }),
    )
    .await;
    assert!(out.ok, "{}", out.detail);

    let (want, format): (String, Option<String>) =
        sqlx::query_as("SELECT want_status, want_format FROM books WHERE title = $1")
            .bind("The Way of Kings")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(want, "wanted");
    assert_eq!(format.as_deref(), Some("audiobook"));
}

/// A film nobody has yet is created from the provider payload and then wanted.
#[tokio::test]
async fn a_provider_movie_ref_creates_the_film_and_wants_it() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "newfilm@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = web_session(state, user).await;

    let out = confirmed(
        state,
        &who,
        session,
        "want_it",
        json!({
            "ref": "provider:tmdb:549509:movie",
            "provider_payload": {"title": "The Holdovers", "year": 2023},
        }),
    )
    .await;
    assert!(out.ok, "{}", out.detail);

    let (want, tmdb): (String, Option<String>) =
        sqlx::query_as("SELECT want_status, identifiers->>'tmdb_id' FROM movies WHERE title = $1")
            .bind("The Holdovers")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(want, "wanted");
    assert_eq!(tmdb.as_deref(), Some("549509"));
}

/// The two provider video forms reach the discover handlers rather than falling
/// off the end of the routing table. Without a metadata provider configured they
/// fail there, which is a different failure from "I don't know how to get that"
/// — and that difference is the routing assertion.
#[tokio::test]
async fn provider_season_and_episode_refs_reach_the_discover_path() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "discover@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = web_session(state, user).await;

    for reference in ["provider:tmdb:1396:s24", "provider:tmdb:1396:s24e07"] {
        let out = confirmed(
            state,
            &who,
            session,
            "want_it",
            json!({
                "ref": reference,
                "provider_payload": {"title": "someShow", "year": 2024},
            }),
        )
        .await;
        assert!(
            !out.detail.contains("I do not know how to get"),
            "{reference} fell off the routing table: {}",
            out.detail
        );
    }
}

/// The metadata for something not in the library comes off the resolver's own
/// candidate, never off anything the model retyped — otherwise a row called
/// "that show" gets created.
#[tokio::test]
async fn a_not_in_library_want_takes_its_title_from_the_resolution_not_the_model() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "payload@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = web_session(state, user).await;

    persist_resolution(
        state,
        session,
        json!({
            "ref": "provider:tmdb:549509:movie",
            "title": "The Holdovers",
            "in_library": false,
            "provider_payload": {
                "provider": "tmdb",
                "provider_id": "549509",
                "title": "The Holdovers",
                "year": 2023,
            },
        }),
    )
    .await;

    let done = call(
        state,
        &who,
        session,
        "want_it",
        json!({"ref": "provider:tmdb:549509:movie"}),
    )
    .await;
    assert!(done.ok, "{}", done.detail);

    let (want, year): (String, Option<i32>) =
        sqlx::query_as("SELECT want_status, year FROM movies WHERE title = $1")
            .bind("The Holdovers")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(want, "wanted");
    assert_eq!(year, Some(2023));
}

/// With no resolution to draw on there is no title, and creating a row named
/// after the ref is worse than refusing.
#[tokio::test]
async fn a_not_in_library_want_with_no_resolution_refuses_rather_than_inventing_a_title() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "notitle@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = web_session(state, user).await;

    let out = confirmed(
        state,
        &who,
        session,
        "want_it",
        json!({"ref": "provider:tmdb:549509:movie"}),
    )
    .await;
    assert!(!out.ok, "{}", out.detail);
    assert_eq!(
        count(
            &state.pool,
            "SELECT COUNT(*) FROM movies WHERE $1 = $1",
            user
        )
        .await,
        0,
        "a want with no metadata created a film anyway"
    );
}

// =============================================================================
// Season wants report what is already here
// =============================================================================

/// "You've already got 22 of those, grabbing the other 2" — the counts come from
/// the library, and a season with nothing missing is not re-queued.
#[tokio::test]
async fn a_season_want_reports_what_is_already_here_and_does_not_requeue() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "season@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = web_session(state, user).await;

    let show = create_show(&server, "someShow").await;
    let season = create_season(&state.pool, show, 24).await;
    let mut here = Vec::new();
    for n in 1..=3 {
        let ep = create_episode(&state.pool, season, n).await;
        if n <= 2 {
            create_stub_episode_file(&state.pool, ep).await;
            here.push(ep);
        }
    }

    let out = confirmed(
        state,
        &who,
        session,
        "want_it",
        json!({"ref": format!("season:{season}")}),
    )
    .await;
    assert!(out.ok, "{}", out.detail);
    assert_eq!(out.observation["already_have"], json!(2));
    assert_eq!(out.observation["newly_wanted"], json!(1));
    let say = out.observation["say"].as_str().unwrap();
    assert!(say.contains("already got 2"), "{say}");
    assert!(say.contains("other 1"), "{say}");

    // Nothing missing now: say so, do not queue again.
    let again = confirmed(
        state,
        &who,
        session,
        "want_it",
        json!({"ref": format!("season:{season}")}),
    )
    .await;
    assert_eq!(again.observation["queued"], json!(false));
    assert_eq!(again.observation["newly_wanted"], json!(0));
}

// =============================================================================
// list_releases / grab_release / regrab
// =============================================================================

/// A wide result set drowns a 12B model, so the truncation is server-side and
/// not a suggestion.
#[tokio::test]
async fn a_release_list_is_truncated_before_the_model_sees_it() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "list@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = web_session(state, user).await;

    // One 1080p copy, and it is past the window the model is shown. Saying "there
    // aren't any 1080p versions" off the visible rows is the failure this guards.
    let many: Vec<Value> = (0..20)
        .map(|i| {
            let res = if i == 19 { "1080p" } else { "2160p" };
            release(
                &format!("someShow.S24E01.{res}.WEB-DL-{i}"),
                &format!("hash{i}"),
                100 - i,
            )
        })
        .collect();
    let _kobload = kobload_returning(&server, json!(many)).await;

    let show = create_show(&server, "someShow").await;
    let season = create_season(&state.pool, show, 24).await;
    let episode = create_episode(&state.pool, season, 1).await;

    let out = call(
        state,
        &who,
        session,
        "list_releases",
        json!({"ref": format!("episode:{episode}")}),
    )
    .await;
    assert!(out.ok, "{}", out.detail);
    let releases = out.observation["releases"].as_array().unwrap();
    assert_eq!(releases.len(), 5, "the model was handed the whole indexer");
    assert!(releases[0].get("release_index").is_some());
    assert_eq!(releases[0]["resolution"], json!("2160p"));
    assert_eq!(releases[0]["seeders"], json!(100));
    assert_eq!(
        out.observation["total_found"],
        json!(20),
        "the window has to say what it is a window onto"
    );
    assert_eq!(
        out.observation["resolutions_available"],
        json!(["2160p", "1080p"]),
        "the tally is over the search, not over the rows"
    );

    let filtered = call(
        state,
        &who,
        session,
        "list_releases",
        json!({"ref": format!("episode:{episode}"), "resolution": "1080p"}),
    )
    .await;
    assert!(filtered.ok, "{}", filtered.detail);
    let rows = filtered.observation["releases"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "the one 1080p copy is reachable");
    assert_eq!(
        rows[0]["release_index"],
        json!(19),
        "a filtered row still indexes into the unfiltered search"
    );
}

/// A regrab is not a picker. It rules out the copy on disk and hands the search
/// back to Kobload, which is the only place the quality profile lives — the chat
/// bot choosing off a raw release list is how a 2160p copy got past a profile
/// that does not allow one.
#[tokio::test]
async fn regrab_hands_the_pick_back_to_kobload() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "regrab@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = web_session(state, user).await;

    // Deliberately the shape that used to bite: the best release by seeders is
    // above what any sane profile allows.
    let kobload = kobload_returning(
        &server,
        json!([
            release("someShow.S24E01.2160p.REMUX", "bbbb2222", 900),
            release("someShow.S24E01.1080p.WEB-DL", "cccc3333", 400),
        ]),
    )
    .await;

    let show = create_show(&server, "someShow").await;
    let season = create_season(&state.pool, show, 24).await;
    let episode = create_episode(&state.pool, season, 1).await;
    let file = create_stub_episode_file(&state.pool, episode).await;
    sqlx::query("UPDATE tv_episode_files SET source_metadata = $2 WHERE id = $1")
        .bind(file)
        .bind(json!({"info_hash": "aaaa1111", "release_title": "someShow.S24E01.CURRENT"}))
        .execute(&state.pool)
        .await
        .unwrap();

    let out = confirmed(
        state,
        &who,
        session,
        "regrab",
        json!({"ref": format!("episode:{episode}")}),
    )
    .await;
    assert!(out.ok, "{}", out.detail);
    assert_eq!(out.observation["done"], json!(true));
    assert_eq!(out.observation["blocklisted_current_release"], json!(true));

    assert_eq!(
        count(
            &state.pool,
            "SELECT COUNT(*) FROM wanted_attempts WHERE episode_id = $1",
            episode
        )
        .await,
        0,
        "the chat bot queued a release it chose itself"
    );
    assert!(
        !kobload
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .any(|r| r.url.path() == "/api/v1/search"),
        "a regrab must not go anywhere near the interactive release list"
    );

    assert_eq!(episode_want_status(&state.pool, episode).await, "wanted");

    // The copy on disk goes on the blocklist, which is what reaches Kobload as
    // excluded_releases — otherwise its own best pick is the broken file.
    let rejected: Value =
        sqlx::query_scalar("SELECT rejected_releases FROM tv_episodes WHERE id = $1")
            .bind(episode)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    let entries = rejected.as_array().expect("rejected_releases is an array");
    assert_eq!(entries.len(), 1, "{rejected}");
    assert_eq!(entries[0]["info_hash"], json!("aaaa1111"));
    assert_eq!(
        entries[0]["release_title"],
        json!("someShow.S24E01.CURRENT")
    );
}

/// Video carries the additive flag all the way to the link step, so a regrab of
/// an episode keeps the old file and says so. A book has no such flag, and the
/// copy must not pretend otherwise.
#[tokio::test]
async fn regrab_only_promises_the_old_file_where_the_flag_reaches() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "additive@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = web_session(state, user).await;

    let _kobload = kobload_returning(&server, json!([])).await;

    let show = create_show(&server, "someShow").await;
    let season = create_season(&state.pool, show, 24).await;
    let episode = create_episode(&state.pool, season, 1).await;
    let file = create_stub_episode_file(&state.pool, episode).await;
    sqlx::query("UPDATE tv_episode_files SET source_metadata = $2 WHERE id = $1")
        .bind(file)
        .bind(json!({"info_hash": "aaaa1111"}))
        .execute(&state.pool)
        .await
        .unwrap();

    let out = confirmed(
        state,
        &who,
        session,
        "regrab",
        json!({"ref": format!("episode:{episode}")}),
    )
    .await;
    assert_eq!(out.observation["keeps_existing_file"], json!(true));
    let say = out.observation["say"].as_str().unwrap();
    assert!(say.contains("stays where it is"), "{say}");
}

/// A regrab of a book must say it replaces, because `book_files` upserts on
/// `(book_id, format)` and there is no additive flag on that path.
#[tokio::test]
async fn a_book_regrab_says_it_replaces_because_it_does() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "bookregrab@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = web_session(state, user).await;

    let _kobload = kobload_returning(&server, json!([])).await;

    let book = create_book(&server, "someBook").await;
    let file = create_stub_book_file(&state.pool, book).await;
    sqlx::query(
        "UPDATE book_files SET source_metadata = $2, source_type = 'kobload' WHERE id = $1",
    )
    .bind(file)
    .bind(json!({"info_hash": "cccc1111"}))
    .execute(&state.pool)
    .await
    .unwrap();

    let out = confirmed(
        state,
        &who,
        session,
        "regrab",
        json!({"ref": format!("book:{book}")}),
    )
    .await;
    assert!(out.ok, "{}", out.detail);
    assert_eq!(out.observation["keeps_existing_file"], json!(false));
    let say = out.observation["say"].as_str().unwrap();
    assert!(say.contains("replaces the copy"), "{say}");
}

// =============================================================================
// Terminal messages — constraint 8
// =============================================================================

/// Anything Kobload takes away has to end in a message, which means a watch.
/// Without one the action is silent, and silence reads as broken.
#[tokio::test]
async fn an_asynchronous_action_registers_a_watch_on_an_addressed_conversation() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "watch@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = addressed_session(state, user).await;

    let show = create_show(&server, "someShow").await;
    let season = create_season(&state.pool, show, 24).await;
    let episode = create_episode(&state.pool, season, 1).await;

    let out = confirmed(
        state,
        &who,
        session,
        "want_it",
        json!({"ref": format!("episode:{episode}")}),
    )
    .await;
    assert!(out.ok, "{}", out.detail);
    assert_eq!(out.observation["watching"], json!(true));

    let (kind, watcher, label, message_id): (String, Option<Uuid>, String, Option<String>) =
        sqlx::query_as(
            "SELECT entity_kind, user_id, label, message_id FROM chat_watches WHERE entity_id = $1",
        )
        .bind(episode)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(kind, "episode");
    assert_eq!(watcher, Some(user), "the watch must know who asked");
    assert!(
        label.contains("someShow"),
        "the watch has to name the thing or every later message says 'your download': {label}"
    );
    // No transport message exists yet — the turn has not been posted. The
    // dispatcher posts the first update and records the id it gets back.
    assert_eq!(message_id, None);
}

/// A season want registers a watch per episode it actually queued — and none for
/// the ones already here.
#[tokio::test]
async fn a_season_want_watches_only_what_it_queued() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "seasonwatch@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = addressed_session(state, user).await;

    let show = create_show(&server, "someShow").await;
    let season = create_season(&state.pool, show, 24).await;
    let have = create_episode(&state.pool, season, 1).await;
    create_stub_episode_file(&state.pool, have).await;
    let missing = create_episode(&state.pool, season, 2).await;

    confirmed(
        state,
        &who,
        session,
        "want_it",
        json!({"ref": format!("season:{season}")}),
    )
    .await;

    assert_eq!(
        count(
            &state.pool,
            "SELECT COUNT(*) FROM chat_watches WHERE entity_id = $1",
            missing
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &state.pool,
            "SELECT COUNT(*) FROM chat_watches WHERE entity_id = $1",
            have
        )
        .await,
        0,
        "watched an episode that was never queued"
    );
}

/// A synchronous action still says something definite; the Command Palette
/// watches its own stream, so there is nothing to register and the tool says so
/// rather than failing.
#[tokio::test]
async fn a_web_session_has_no_watch_to_register_and_says_so() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "websess@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = web_session(state, user).await;

    let show = create_show(&server, "someShow").await;
    let season = create_season(&state.pool, show, 24).await;
    let episode = create_episode(&state.pool, season, 1).await;

    let out = confirmed(
        state,
        &who,
        session,
        "want_it",
        json!({"ref": format!("episode:{episode}")}),
    )
    .await;
    assert!(out.ok);
    assert_eq!(out.observation["watching"], json!(false));
    assert!(
        out.observation["say"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
}

// =============================================================================
// The cheap verbs
// =============================================================================

/// The safest verb there is, and the natural answer when something is not out
/// yet. An episode ref lists its show, because "my list" is about shows.
#[tokio::test]
async fn adding_to_the_watchlist_costs_nothing_and_is_attributed() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "list2@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = web_session(state, user).await;

    let show = create_show(&server, "someShow").await;
    let season = create_season(&state.pool, show, 24).await;
    let episode = create_episode(&state.pool, season, 1).await;

    let out = call(
        state,
        &who,
        session,
        "add_to_watchlist",
        json!({"ref": format!("episode:{episode}")}),
    )
    .await;
    assert!(out.ok, "{}", out.detail);

    let listed: Option<Uuid> = sqlx::query_scalar(
        "SELECT media_id FROM watchlist_items WHERE user_id = $1 AND media_type = 'show'",
    )
    .bind(user)
    .fetch_optional(&state.pool)
    .await
    .unwrap();
    assert_eq!(listed, Some(show), "an episode ref must list its show");

    let attributed: Option<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM activity_log WHERE entity_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(show)
    .fetch_optional(&state.pool)
    .await
    .unwrap()
    .flatten();
    assert_eq!(attributed, Some(user));

    // Nothing was downloaded.
    assert_eq!(
        count(
            &state.pool,
            "SELECT COUNT(*) FROM wanted_attempts WHERE user_id = $1",
            user
        )
        .await,
        0
    );
}

/// Stopping is the safe direction, so it does not ask twice — but it also does
/// not invent a download to stop.
#[tokio::test]
async fn cancelling_when_nothing_is_downloading_says_so_rather_than_failing() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "cancel@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = web_session(state, user).await;

    let show = create_show(&server, "someShow").await;
    let season = create_season(&state.pool, show, 24).await;
    let episode = create_episode(&state.pool, season, 1).await;

    let out = call(
        state,
        &who,
        session,
        "cancel_download",
        json!({"ref": format!("episode:{episode}")}),
    )
    .await;
    assert!(out.ok, "{}", out.detail);
    assert_eq!(out.observation["done"], json!(false));
    assert!(
        out.observation["say"]
            .as_str()
            .is_some_and(|s| s.contains("Nothing is downloading"))
    );
}

/// Nothing here deletes: cancelling stops the download and leaves the files.
#[tokio::test]
async fn cancelling_a_download_leaves_the_files_alone() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "cancel2@example.com", "contributor").await;
    let who = auth(user, "contributor");
    let session = web_session(state, user).await;

    let mock = MockServer::start().await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&mock)
        .await;
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({"kobload_base_url": mock.uri(), "kobload_api_key": "test-key"}),
        )
        .await
        .assert_ok();

    let show = create_show(&server, "someShow").await;
    let season = create_season(&state.pool, show, 24).await;
    let episode = create_episode(&state.pool, season, 1).await;
    let file = create_stub_episode_file(&state.pool, episode).await;

    let download_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO wanted_attempts (id, episode_id, source, external_id, status, user_id, created_at)
         VALUES ($1, $2, 'chat', $3, 'downloading', $4, NOW())",
    )
    .bind(Uuid::now_v7())
    .bind(episode)
    .bind(download_id.to_string())
    .bind(user)
    .execute(&state.pool)
    .await
    .unwrap();

    let out = call(
        state,
        &who,
        session,
        "cancel_download",
        json!({"ref": format!("episode:{episode}")}),
    )
    .await;
    assert!(out.ok, "{}", out.detail);
    assert_eq!(out.observation["done"], json!(true));

    let status: String =
        sqlx::query_scalar("SELECT status FROM wanted_attempts WHERE external_id = $1")
            .bind(download_id.to_string())
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(status, "cancelled");
    assert_eq!(
        count(
            &state.pool,
            "SELECT COUNT(*) FROM tv_episode_files WHERE id = $1",
            file
        )
        .await,
        1,
        "cancelling deleted a file"
    );
}

// =============================================================================
// recently_added scoping
// =============================================================================

/// "What audiobooks just came in" is not the same question as "what books just came in":
/// an audiobook bought for a book shelved years ago is new to the listener, and the
/// book's own age would bury it.
#[tokio::test]
async fn recently_added_scopes_audiobooks_to_their_own_arrival() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let admin_id = create_user_with_role(&server, "admin-recent@example.com", "admin").await;
    let admin = auth(admin_id, "admin");
    let session = web_session(state, admin_id).await;

    let ebook = create_book(&server, "Ebook Only").await;
    add_ebook_file(&server, ebook).await;
    let audiobook = create_book(&server, "Audio Only").await;
    add_audiobook_file(&server, audiobook).await;

    let titles = |outcome: &ToolOutcome| -> Vec<String> {
        outcome.observation["items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|i| i["title"].as_str().unwrap_or_default().to_string())
            .collect()
    };

    let books = call(
        state,
        &admin,
        session,
        "recently_added",
        json!({"media_type": "book"}),
    )
    .await;
    let listed = titles(&books);
    assert!(listed.contains(&"Ebook Only".to_string()));
    assert!(listed.contains(&"Audio Only".to_string()));

    let audiobooks = call(
        state,
        &admin,
        session,
        "recently_added",
        json!({"media_type": "audiobook"}),
    )
    .await;
    assert_eq!(titles(&audiobooks), vec!["Audio Only".to_string()]);
}
