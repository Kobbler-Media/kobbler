//! `recommend` and `whats_new`.
//!
//! The load-bearing assertion in this file is a negative one: **no embedder.**
//! `recommend` sends Meili a filtered browse and nothing that resembles a vector
//! search, because transcript embeddings went in at schema v15 and came out at
//! v23 on RAM cost. The quality comes from the model reasoning over a shortlist
//! that already carries the synopsis, not from a similarity score.
//!
//! The other half is watch state. It is per-user, it is deliberately not in the
//! search documents — `MovieDocument.watched` was removed at v32 because it read
//! an arbitrary user's row — so the subtraction happens in Rust, scoped by
//! `user_id`, and these tests set up a second household member to prove it.
//!
//! Meili is a wiremock. No LLM, no GPU.

use crate::common::*;
use api::AppState;
use api::chatbot::tools::dispatch::{self, Ctx};
use api::chatbot::tools::read::ToolOutcome;
use api::repositories::chat_sessions::{self, NewSession};
use api::routes::AuthUser;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// =============================================================================
// Harness
// =============================================================================

fn auth(user_id: Uuid, role: &str) -> AuthUser {
    AuthUser {
        user_id,
        role: role.to_string(),
        is_kids: false,
    }
}

async fn session_for(state: &AppState, user_id: Uuid) -> Uuid {
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

/// Every call goes through `dispatch`, because that is where the role check is.
async fn call(
    state: &AppState,
    who: &AuthUser,
    session_id: Uuid,
    tool: &str,
    args: Value,
) -> ToolOutcome {
    let ctx = Ctx {
        state,
        auth: who,
        session_id,
        context_ref: None,
    };
    dispatch::dispatch(&ctx, tool, &args).await
}

/// A Meili standing in for the real one, answering every `/multi-search` with
/// one hit list per requested index.
///
/// Keep the returned server bound for the whole test. `MockServer::start` checks
/// one out of a process-wide pool, and dropping it hands that *same address* to
/// the next test that starts one, which resets the mocks — so a dropped guard
/// means a concurrent test answers this test's search with its own documents.
#[must_use]
async fn meili_returning(server: &TestServer, per_index: Vec<(&str, Vec<Value>)>) -> MockServer {
    let mock = MockServer::start().await;
    let results: Vec<Value> = per_index
        .into_iter()
        .map(|(index, hits)| {
            json!({"indexUid": index, "hits": hits, "estimatedTotalHits": hits.len()})
        })
        .collect();
    Mock::given(method("POST"))
        .and(path("/multi-search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": results})))
        .mount(&mock)
        .await;
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({"meili_url": mock.uri(), "meili_master_key": "test-master-key"}),
        )
        .await
        .assert_ok();
    mock
}

/// The bodies kobbler actually sent to the index.
async fn sent_queries(mock: &MockServer) -> Vec<Value> {
    mock.received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .flat_map(|body| {
            body["queries"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
        })
        .collect()
}

fn movie(id: Uuid, title: &str, year: i64) -> Value {
    json!({
        "id": id.to_string(),
        "title": title,
        "year": year,
        "genres": ["Horror"],
        "runtime": 98,
        "rating": 7.4,
        "description": format!("{title} is a slow-burn horror film about a quiet town."),
    })
}

fn show(id: Uuid, title: &str) -> Value {
    json!({
        "id": id.to_string(),
        "title": title,
        "year": 2019,
        "genres": ["Horror"],
        "rating": 8.1,
        "available_episode_count": 10,
        "description": format!("{title} is a horror series."),
    })
}

fn book(id: Uuid, title: &str) -> Value {
    json!({
        "id": id.to_string(),
        "title": title,
        "authors": ["Stephen King"],
        "genres": ["Horror"],
        "has_audiobook": false,
        "description": format!("{title} is a horror novel."),
    })
}

async fn create_movie_row(pool: &sqlx::PgPool, title: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO movies (id, title, year) VALUES ($1, $2, 1976)")
        .bind(id)
        .bind(title)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn mark_watched(pool: &sqlx::PgPool, movie_id: Uuid, user_id: Uuid) {
    sqlx::query(
        "INSERT INTO movie_watch_states (id, movie_id, user_id, watched, watched_at)
         VALUES ($1, $2, $3, true, NOW())",
    )
    .bind(Uuid::now_v7())
    .bind(movie_id)
    .bind(user_id)
    .execute(pool)
    .await
    .unwrap();
}

fn candidate_ids(outcome: &ToolOutcome) -> Vec<String> {
    outcome.observation["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect()
}

// =============================================================================
// recommend — vibe to filters
// =============================================================================

#[tokio::test]
async fn a_vibe_becomes_filters_and_a_shortlist_with_synopses() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let hits: Vec<Value> = (0..25)
        .map(|n| movie(Uuid::now_v7(), &format!("Horror {n}"), 1976))
        .collect();
    let meili = meili_returning(&server, vec![("movies", hits)]).await;

    let outcome = call(
        state,
        &who,
        session,
        "recommend",
        json!({"media_type": "movie", "genre": "Horror", "years": "1970-1985"}),
    )
    .await;

    assert!(outcome.ok, "{}", outcome.detail);
    assert_eq!(outcome.observation["count"], 25);
    assert_eq!(outcome.observation["pick"], 3);
    assert_eq!(outcome.observation["filters"]["genre"], "Horror");
    assert_eq!(outcome.observation["filters"]["year_from"], 1970);
    assert_eq!(outcome.observation["filters"]["year_to"], 1985);
    assert!(
        outcome.observation["candidates"][0]["description"]
            .as_str()
            .is_some_and(|d| d.contains("slow-burn")),
        "the model needs the synopsis to reason over: {}",
        outcome.observation["candidates"][0]
    );

    let queries = sent_queries(&meili).await;
    let movies = queries
        .iter()
        .find(|q| q["indexUid"] == "movies")
        .expect("a movies query");
    let filter = movies["filter"].as_str().expect("a filter");
    assert!(filter.contains("genres = \"Horror\""), "{filter}");
    assert!(filter.contains("year >= 1970"), "{filter}");
    assert!(filter.contains("year <= 1985"), "{filter}");
    assert!(
        filter.contains("is_in_library = true"),
        "recommending something the library does not have is not a recommendation: {filter}"
    );
}

/// The constraint that outlives this phase: no vector database, ever.
#[tokio::test]
async fn the_index_is_never_asked_for_a_semantic_search() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let session = session_for(state, user_id).await;

    let meili = meili_returning(
        &server,
        vec![("movies", vec![movie(Uuid::now_v7(), "Carrie", 1976)])],
    )
    .await;

    call(
        state,
        &auth(user_id, "viewer"),
        session,
        "recommend",
        json!({"genre": "Horror"}),
    )
    .await;

    for query in sent_queries(&meili).await {
        let text = query.to_string();
        for forbidden in ["hybrid", "vector", "semanticRatio", "embedder"] {
            assert!(
                !text.contains(forbidden),
                "{forbidden} reached the index — the embedder was reverted at v23: {text}"
            );
        }
    }
}

/// Ties on `rating` used to fall back to Meili's internal document order, which
/// is not stable across a reindex — the same question answered differently after
/// every rebuild. Every recommend query must end in a total order.
#[tokio::test]
async fn every_shortlist_query_ends_in_a_deterministic_key() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let session = session_for(state, user_id).await;

    let meili = meili_returning(
        &server,
        vec![
            ("movies", vec![movie(Uuid::now_v7(), "Carrie", 1976)]),
            ("tv_shows", vec![show(Uuid::now_v7(), "Twin Peaks")]),
            ("books", vec![book(Uuid::now_v7(), "The Shining")]),
        ],
    )
    .await;

    for args in [
        json!({"genre": "Horror"}),
        json!({"media_type": "movie", "genre": "Horror"}),
        json!({"media_type": "show", "genre": "Horror"}),
        json!({"media_type": "book", "genre": "Horror"}),
    ] {
        call(state, &auth(user_id, "viewer"), session, "recommend", args).await;
    }

    let queries = sent_queries(&meili).await;
    assert!(
        queries.iter().any(|q| q["indexUid"] == "books"),
        "the books index was never queried, so this proves nothing: {queries:?}"
    );
    for query in &queries {
        let sort = query["sort"]
            .as_array()
            .unwrap_or_else(|| panic!("no sort on {query}"));
        assert_eq!(
            sort.last().and_then(Value::as_str),
            Some("id:asc"),
            "ties fall back to index order on {}: {sort:?}",
            query["indexUid"]
        );
    }

    let movies = queries
        .iter()
        .find(|q| q["indexUid"] == "movies")
        .expect("a movies query");
    assert_eq!(
        movies["sort"],
        json!(["rating:desc", "id:asc"]),
        "rating still leads; id only breaks the tie"
    );
}

#[tokio::test]
async fn the_shortlist_is_capped_so_it_fits_the_prompt() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let session = session_for(state, user_id).await;

    let movies: Vec<Value> = (0..40)
        .map(|n| movie(Uuid::now_v7(), &format!("Film {n}"), 1980))
        .collect();
    let shows: Vec<Value> = (0..40)
        .map(|n| show(Uuid::now_v7(), &format!("Show {n}")))
        .collect();
    let _meili = meili_returning(&server, vec![("movies", movies), ("tv_shows", shows)]).await;

    let outcome = call(
        state,
        &auth(user_id, "viewer"),
        session,
        "recommend",
        json!({"genre": "Horror"}),
    )
    .await;

    let count = outcome.observation["count"].as_u64().unwrap();
    assert!((20..=40).contains(&count), "shortlist was {count}");
    // Both kinds survive the truncation rather than forty films crowding out
    // every show.
    let kinds: Vec<&str> = outcome.observation["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["type"].as_str().unwrap())
        .collect();
    assert!(
        kinds.contains(&"movie") && kinds.contains(&"tv_show"),
        "{kinds:?}"
    );
}

// =============================================================================
// recommend — watch state, which the index cannot hold
// =============================================================================

#[tokio::test]
async fn films_the_caller_has_already_seen_are_left_out() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let session = session_for(state, user_id).await;

    let seen = create_movie_row(&state.pool, "Carrie").await;
    let unseen = create_movie_row(&state.pool, "The Fog").await;
    mark_watched(&state.pool, seen, user_id).await;

    let _meili = meili_returning(
        &server,
        vec![(
            "movies",
            vec![movie(seen, "Carrie", 1976), movie(unseen, "The Fog", 1980)],
        )],
    )
    .await;

    let outcome = call(
        state,
        &auth(user_id, "viewer"),
        session,
        "recommend",
        json!({"media_type": "movie", "genre": "Horror"}),
    )
    .await;

    assert_eq!(candidate_ids(&outcome), vec![unseen.to_string()]);
    assert_eq!(outcome.observation["watch_state_applied"], true);
}

/// "What should *we* watch" — the query nothing off-the-shelf can answer,
/// because nothing else knows what you have both already seen.
#[tokio::test]
async fn a_second_viewers_watch_state_is_subtracted_too() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let her = create_user_with_role(&server, "her@test.com", "viewer").await;
    let him = create_user_with_role(&server, "him@test.com", "viewer").await;
    let session = session_for(state, her).await;

    let she_saw = create_movie_row(&state.pool, "Carrie").await;
    let he_saw = create_movie_row(&state.pool, "The Fog").await;
    let neither = create_movie_row(&state.pool, "Suspiria").await;
    mark_watched(&state.pool, she_saw, her).await;
    mark_watched(&state.pool, he_saw, him).await;

    let _meili = meili_returning(
        &server,
        vec![(
            "movies",
            vec![
                movie(she_saw, "Carrie", 1976),
                movie(he_saw, "The Fog", 1980),
                movie(neither, "Suspiria", 1977),
            ],
        )],
    )
    .await;

    let alone = call(
        state,
        &auth(her, "viewer"),
        session,
        "recommend",
        json!({"media_type": "movie"}),
    )
    .await;
    assert_eq!(alone.observation["viewers"], 1);
    assert!(candidate_ids(&alone).contains(&he_saw.to_string()));

    let together = call(
        state,
        &auth(her, "viewer"),
        session,
        "recommend",
        json!({"media_type": "movie", "also_for": "him"}),
    )
    .await;
    assert_eq!(together.observation["viewers"], 2);
    assert_eq!(candidate_ids(&together), vec![neither.to_string()]);
}

#[tokio::test]
async fn an_unknown_second_person_is_said_out_loud_rather_than_ignored() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let her = create_user_with_role(&server, "her@test.com", "viewer").await;
    let session = session_for(state, her).await;

    let _meili = meili_returning(
        &server,
        vec![("movies", vec![movie(Uuid::now_v7(), "Suspiria", 1977)])],
    )
    .await;

    let outcome = call(
        state,
        &auth(her, "viewer"),
        session,
        "recommend",
        json!({"media_type": "movie", "also_for": "nobody"}),
    )
    .await;

    assert!(outcome.ok);
    assert!(
        outcome.observation["also_for_note"]
            .as_str()
            .is_some_and(|n| n.contains("nobody")),
        "{}",
        outcome.observation
    );
}

#[tokio::test]
async fn someone_who_has_seen_everything_gets_an_honest_answer_not_a_repeat() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let session = session_for(state, user_id).await;

    let a = create_movie_row(&state.pool, "Carrie").await;
    let b = create_movie_row(&state.pool, "The Fog").await;
    mark_watched(&state.pool, a, user_id).await;
    mark_watched(&state.pool, b, user_id).await;

    let _meili = meili_returning(
        &server,
        vec![(
            "movies",
            vec![movie(a, "Carrie", 1976), movie(b, "The Fog", 1980)],
        )],
    )
    .await;

    let outcome = call(
        state,
        &auth(user_id, "viewer"),
        session,
        "recommend",
        json!({"media_type": "movie", "genre": "Horror"}),
    )
    .await;

    assert!(outcome.ok, "an honest answer is still a successful call");
    assert_eq!(outcome.observation["count"], 0);
    assert_eq!(outcome.observation["all_seen"], true);
    assert_eq!(outcome.observation["matched_before_filtering"], 2);
    assert!(
        outcome.observation["note"]
            .as_str()
            .is_some_and(|n| n.contains("already been watched")),
        "{}",
        outcome.observation
    );
}

#[tokio::test]
async fn a_rewatch_keeps_what_has_already_been_seen() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let session = session_for(state, user_id).await;

    let seen = create_movie_row(&state.pool, "Carrie").await;
    mark_watched(&state.pool, seen, user_id).await;
    let _meili =
        meili_returning(&server, vec![("movies", vec![movie(seen, "Carrie", 1976)])]).await;

    let outcome = call(
        state,
        &auth(user_id, "viewer"),
        session,
        "recommend",
        json!({"media_type": "movie", "include_watched": true}),
    )
    .await;

    assert_eq!(candidate_ids(&outcome), vec![seen.to_string()]);
    assert_eq!(outcome.observation["watch_state_applied"], false);
}

#[tokio::test]
async fn nothing_matching_is_said_plainly() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let session = session_for(state, user_id).await;
    let _meili = meili_returning(&server, vec![("movies", vec![])]).await;

    let outcome = call(
        state,
        &auth(user_id, "viewer"),
        session,
        "recommend",
        json!({"media_type": "movie", "genre": "Klezmer"}),
    )
    .await;

    assert!(outcome.ok);
    assert_eq!(outcome.observation["count"], 0);
    assert!(outcome.observation["all_seen"].is_null());
}

/// Read is free: recommending is not an action and needs no privilege.
#[tokio::test]
async fn a_viewer_may_ask_for_a_recommendation() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user_id = create_user_with_role(&server, "viewer@test.com", "viewer").await;
    let session = session_for(state, user_id).await;
    let _meili = meili_returning(
        &server,
        vec![("movies", vec![movie(Uuid::now_v7(), "Suspiria", 1977)])],
    )
    .await;

    let outcome = call(
        state,
        &auth(user_id, "viewer"),
        session,
        "recommend",
        json!({"media_type": "movie"}),
    )
    .await;
    assert!(outcome.ok, "{}", outcome.detail);
}

// =============================================================================
// whats_new — pull, not push
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
    let id: Uuid = resp.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    // The calendar only answers for monitored shows, and a new one is not.
    sqlx::query("UPDATE tv_shows SET is_monitored = TRUE, content_category = 'tv' WHERE id = $1")
        .bind(id)
        .execute(&server.pool)
        .await
        .unwrap();
    id
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

/// `air_time` is set only when TVMaze genuinely published one. A NULL means the
/// day is all anyone knows, and the copy must not invent an hour.
async fn create_episode(
    pool: &sqlx::PgPool,
    season_id: Uuid,
    number: i32,
    days_from_today: i64,
    with_air_time: bool,
) -> Uuid {
    let id = Uuid::now_v7();
    let air_date = time::OffsetDateTime::now_utc().date() + time::Duration::days(days_from_today);
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title, air_date, air_time)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(season_id)
    .bind(number.to_string())
    .bind(number)
    .bind(format!("Episode {number}"))
    .bind(air_date)
    .bind(match with_air_time {
        true => Some(air_date.with_hms(21, 0, 0).unwrap().assume_utc()),
        false => None,
    })
    .execute(pool)
    .await
    .unwrap();
    id
}

fn item_for(observation: &Value, id: Uuid) -> &Value {
    observation["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|i| i["id"] == id.to_string())
        .unwrap_or_else(|| panic!("{id} missing from {observation}"))
}

#[tokio::test]
async fn whats_new_answers_both_directions() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let show = create_show(&server, "someShow").await;
    let season = create_season(&state.pool, show, 24).await;
    let landed = create_episode(&state.pool, season, 1, -2, false).await;
    let due = create_episode(&state.pool, season, 2, 3, false).await;

    let past = call(
        state,
        &who,
        session,
        "whats_new",
        json!({"window": "past_week"}),
    )
    .await;
    assert!(past.ok, "{}", past.detail);
    assert_eq!(past.observation["window"], "past_week");
    let past_ids: Vec<&str> = past.observation["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert!(past_ids.contains(&landed.to_string().as_str()));
    assert!(!past_ids.contains(&due.to_string().as_str()));

    let coming = call(
        state,
        &who,
        session,
        "whats_new",
        json!({"window": "coming_week"}),
    )
    .await;
    let coming_ids: Vec<&str> = coming.observation["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert!(coming_ids.contains(&due.to_string().as_str()));
    assert!(!coming_ids.contains(&landed.to_string().as_str()));
}

/// TMDB dates are unreliable for binge drops. The hedge rides as data so the
/// model cannot forget to pass it on.
#[tokio::test]
async fn the_air_date_hedge_travels_with_the_answer() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let session = session_for(state, user_id).await;

    let outcome = call(
        state,
        &auth(user_id, "viewer"),
        session,
        "whats_new",
        json!({"window": "coming_week"}),
    )
    .await;

    assert_eq!(outcome.observation["date_confidence"], "approximate");
    let caveat = outcome.observation["caveat"].as_str().expect("a caveat");
    assert!(caveat.contains("TMDB"), "{caveat}");
    assert!(caveat.to_lowercase().contains("air_time"), "{caveat}");
}

#[tokio::test]
async fn only_a_tvmaze_timed_episode_carries_a_time() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let session = session_for(state, user_id).await;

    let show = create_show(&server, "someShow").await;
    let season = create_season(&state.pool, show, 24).await;
    let timed = create_episode(&state.pool, season, 1, 2, true).await;
    let untimed = create_episode(&state.pool, season, 2, 3, false).await;

    let outcome = call(
        state,
        &auth(user_id, "viewer"),
        session,
        "whats_new",
        json!({"window": "coming_week"}),
    )
    .await;

    assert!(
        item_for(&outcome.observation, timed)["air_time"]
            .as_str()
            .is_some_and(|t| t.contains("21:00")),
        "{}",
        item_for(&outcome.observation, timed)
    );
    assert!(
        item_for(&outcome.observation, untimed)
            .get("air_time")
            .is_none(),
        "a TMDB-only episode must not be given an invented hour: {}",
        item_for(&outcome.observation, untimed)
    );
}

#[tokio::test]
async fn an_unknown_window_is_refused_rather_than_guessed() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let session = session_for(state, user_id).await;

    let outcome = call(
        state,
        &auth(user_id, "viewer"),
        session,
        "whats_new",
        json!({"window": "next_year"}),
    )
    .await;
    assert!(!outcome.ok);
    assert!(outcome.detail.contains("next_year"), "{}", outcome.detail);
}
