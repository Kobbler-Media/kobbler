//! `person_credits` and `cast_of`.
//!
//! Both live and both failed live: "what Ashton Kutcher movies do I have" burnt
//! the whole step budget, because the movies projection carries `directors` and
//! the people projection carries counts, and nothing in Meili joins the two.
//!
//! The load-bearing assertions here are the two things a count cannot say:
//! a title with no file behind it is **not** held however complete its metadata
//! is, and a credit the household dismissed is gone from both directions.
//!
//! No Meili, no LLM, no GPU — these are postgres joins.

use crate::common::*;
use api::AppState;
use api::chatbot::tools::dispatch::{self, Ctx};
use api::chatbot::tools::read::ToolOutcome;
use api::repositories::chat_sessions::{self, NewSession};
use api::routes::AuthUser;
use serde_json::{Value, json};
use uuid::Uuid;

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

async fn person(pool: &sqlx::PgPool, name: &str, tmdb: Option<&str>) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO people (id, name, identifiers) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(name)
        .bind(json!({"tmdb_id": tmdb}))
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn movie(pool: &sqlx::PgPool, title: &str, year: i32, tmdb: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO movies (id, title, year, identifiers) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(title)
        .bind(year)
        .bind(json!({"tmdb_id": tmdb}))
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn give_movie_a_file(pool: &sqlx::PgPool, movie_id: Uuid) {
    sqlx::query(
        "INSERT INTO movie_files (id, movie_id, file_path, format, available)
         VALUES ($1, $2, $3, 'mkv', true)",
    )
    .bind(Uuid::now_v7())
    .bind(movie_id)
    .bind(format!("/media/{movie_id}.mkv"))
    .execute(pool)
    .await
    .unwrap();
}

async fn cast_in_movie(
    pool: &sqlx::PgPool,
    movie_id: Uuid,
    person_id: Uuid,
    character: &str,
    order: i32,
) {
    sqlx::query(
        "INSERT INTO movie_cast (id, movie_id, person_id, character_name, cast_order)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(movie_id)
    .bind(person_id)
    .bind(character)
    .bind(order)
    .execute(pool)
    .await
    .unwrap();
}

async fn directed_movie(pool: &sqlx::PgPool, movie_id: Uuid, person_id: Uuid) {
    sqlx::query(
        "INSERT INTO movie_crew (movie_id, person_id, department, job)
         VALUES ($1, $2, 'Directing', 'Director')",
    )
    .bind(movie_id)
    .bind(person_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn show(pool: &sqlx::PgPool, title: &str, year: i32, tmdb: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO tv_shows (id, title, year, identifiers) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(title)
        .bind(year)
        .bind(json!({"tmdb_id": tmdb}))
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn cast_in_show(
    pool: &sqlx::PgPool,
    show_id: Uuid,
    person_id: Uuid,
    character: &str,
    order: i32,
) {
    sqlx::query(
        "INSERT INTO tv_cast (id, show_id, person_id, character_name, cast_order)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(show_id)
    .bind(person_id)
    .bind(character)
    .bind(order)
    .execute(pool)
    .await
    .unwrap();
}

/// One season, one episode, and optionally a file behind it — the only thing
/// that makes a show "held".
async fn give_show_an_episode(pool: &sqlx::PgPool, show_id: Uuid, with_file: bool) -> Uuid {
    let season_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order)
         VALUES ($1, $2, '1', 1)",
    )
    .bind(season_id)
    .bind(show_id)
    .execute(pool)
    .await
    .unwrap();
    let episode_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title)
         VALUES ($1, $2, '1', 1, 'Pilot')",
    )
    .bind(episode_id)
    .bind(season_id)
    .execute(pool)
    .await
    .unwrap();
    if with_file {
        let file_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO tv_episode_files (id, file_path, format, available)
             VALUES ($1, $2, 'mkv', true)",
        )
        .bind(file_id)
        .bind(format!("/media/{episode_id}.mkv"))
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tv_episode_file_episodes (episode_file_id, episode_id) VALUES ($1, $2)",
        )
        .bind(file_id)
        .bind(episode_id)
        .execute(pool)
        .await
        .unwrap();
    }
    episode_id
}

async fn dismiss(pool: &sqlx::PgPool, person_id: Uuid, tmdb: i32, media_type: &str) {
    sqlx::query(
        "INSERT INTO person_dismissed_credits (person_id, tmdb_id, media_type) VALUES ($1, $2, $3)",
    )
    .bind(person_id)
    .bind(tmdb)
    .bind(media_type)
    .execute(pool)
    .await
    .unwrap();
}

fn titles(outcome: &ToolOutcome, key: &str) -> Vec<String> {
    outcome.observation[key]
        .as_array()
        .expect(key)
        .iter()
        .map(|v| v["title"].as_str().unwrap().to_string())
        .collect()
}

fn names(outcome: &ToolOutcome, key: &str) -> Vec<String> {
    outcome.observation[key]
        .as_array()
        .expect(key)
        .iter()
        .map(|v| v["name"].as_str().unwrap().to_string())
        .collect()
}

// =============================================================================
// person_credits
// =============================================================================

/// The question that failed live, end to end: a name in, held titles out, with
/// the character, and the metadata-only rows counted rather than listed.
#[tokio::test]
async fn a_persons_credits_list_what_is_held_and_only_count_what_is_not() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let actor = person(pool, "Ashton Kutcher", Some("18999")).await;
    let held = movie(pool, "The Butterfly Effect", 2004, "3049").await;
    give_movie_a_file(pool, held).await;
    cast_in_movie(pool, held, actor, "Evan Treborn", 0).await;

    let not_held = movie(pool, "Dude Where's My Car", 2000, "3050").await;
    cast_in_movie(pool, not_held, actor, "Jesse", 0).await;

    let held_show = show(pool, "That 70s Show", 1998, "3051").await;
    give_show_an_episode(pool, held_show, true).await;
    cast_in_show(pool, held_show, actor, "Michael Kelso", 3).await;

    let outcome = call(
        state,
        &who,
        session,
        "person_credits",
        json!({"person": "Ashton Kutcher"}),
    )
    .await;

    assert!(outcome.ok, "{}", outcome.detail);
    assert_eq!(outcome.observation["found"], true);
    assert_eq!(outcome.observation["person"], "Ashton Kutcher");
    assert_eq!(titles(&outcome, "movies"), vec!["The Butterfly Effect"]);
    assert_eq!(titles(&outcome, "shows"), vec!["That 70s Show"]);
    assert_eq!(
        outcome.observation["movies"][0]["character"],
        "Evan Treborn"
    );
    assert_eq!(
        outcome.observation["shows"][0]["character"],
        "Michael Kelso"
    );
    assert_eq!(outcome.observation["in_library"]["movies"], 1);
    assert_eq!(outcome.observation["in_library"]["shows"], 1);
    assert_eq!(outcome.observation["credited_but_not_held"]["movies"], 1);
    assert_eq!(outcome.observation["truncated"], false);

    let say = outcome.observation["say"].as_str().expect("a say line");
    assert!(say.contains("1 film"), "{say}");
    assert!(say.contains("1 show"), "{say}");
    assert!(
        say.contains("don't have"),
        "the not-held count has to reach the reply: {say}"
    );

    // The citation is what makes the answer clickable, and only held things are
    // worth citing.
    assert_eq!(outcome.sources.len(), 2);
    assert!(outcome.sources.iter().any(|s| s.item_type == "movie"));
    assert!(outcome.sources.iter().any(|s| s.item_type == "tv_show"));
}

/// A row in `movies` is metadata, not a file. Enrichment pulls a whole
/// filmography in, so "what do I have" answered from row existence would say
/// yes to almost everything.
#[tokio::test]
async fn a_person_whose_every_credit_is_metadata_only_holds_nothing() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let actor = person(pool, "Nobody Holdsme", None).await;
    let m = movie(pool, "An Unowned Film", 1999, "9001").await;
    cast_in_movie(pool, m, actor, "Extra", 0).await;
    let s = show(pool, "An Unowned Show", 1999, "9002").await;
    give_show_an_episode(pool, s, false).await;
    cast_in_show(pool, s, actor, "Extra", 0).await;

    let outcome = call(
        state,
        &who,
        session,
        "person_credits",
        json!({"person": "Nobody Holdsme"}),
    )
    .await;

    assert!(outcome.ok, "{}", outcome.detail);
    assert_eq!(outcome.observation["found"], true);
    assert_eq!(outcome.observation["in_library"]["movies"], 0);
    assert_eq!(outcome.observation["in_library"]["shows"], 0);
    assert!(titles(&outcome, "movies").is_empty());
    assert!(titles(&outcome, "shows").is_empty());
    assert_eq!(outcome.observation["credited_but_not_held"]["movies"], 1);
    assert_eq!(outcome.observation["credited_but_not_held"]["shows"], 1);
    assert!(outcome.sources.is_empty());
    let say = outcome.observation["say"].as_str().unwrap();
    assert!(say.contains("nothing"), "{say}");
}

/// Nobody by that name is a plain answer, not a failure — the model must say so
/// rather than fill the gap from its own memory.
#[tokio::test]
async fn an_unknown_person_comes_back_found_false_and_not_as_an_error() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let outcome = call(
        state,
        &who,
        session,
        "person_credits",
        json!({"person": "Someone Who Is Not There"}),
    )
    .await;

    assert!(outcome.ok, "{}", outcome.detail);
    assert_eq!(outcome.observation["found"], false);
    assert!(outcome.observation["movies"].is_null());
    let note = outcome.observation["note"].as_str().unwrap();
    assert!(note.contains("do not guess"), "{note}");
}

/// A dismissed credit is the household saying "that is not really them". It has
/// to disappear from the answer, and the join is on the provider id, not ours.
#[tokio::test]
async fn a_dismissed_credit_is_gone_from_the_persons_list() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let actor = person(pool, "Dismissive Dan", Some("777")).await;
    let kept = movie(pool, "A Film He Is Really In", 2010, "4001").await;
    give_movie_a_file(pool, kept).await;
    cast_in_movie(pool, kept, actor, "Himself", 0).await;

    let wrong = movie(pool, "A Film He Is Not In", 2011, "4002").await;
    give_movie_a_file(pool, wrong).await;
    cast_in_movie(pool, wrong, actor, "Someone Else", 0).await;
    dismiss(pool, actor, 4002, "movie").await;

    let wrong_show = show(pool, "A Show He Is Not In", 2012, "4003").await;
    give_show_an_episode(pool, wrong_show, true).await;
    cast_in_show(pool, wrong_show, actor, "Someone Else", 0).await;
    dismiss(pool, actor, 4003, "tv").await;

    let outcome = call(
        state,
        &who,
        session,
        "person_credits",
        json!({"person": "Dismissive Dan"}),
    )
    .await;

    assert!(outcome.ok, "{}", outcome.detail);
    assert_eq!(titles(&outcome, "movies"), vec!["A Film He Is Really In"]);
    assert!(titles(&outcome, "shows").is_empty());
    assert_eq!(
        outcome.observation["credited_but_not_held"]["movies"], 0,
        "a dismissed credit is not a not-held credit, it is no credit"
    );
}

/// Truncating silently would have the bot claim a short filmography.
#[tokio::test]
async fn a_truncated_list_says_so_in_the_line_the_model_reads_out() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let actor = person(pool, "Prolific Pat", Some("888")).await;
    for n in 0..5 {
        let m = movie(
            pool,
            &format!("Film Number {n}"),
            2000 + n,
            &format!("5{n:03}"),
        )
        .await;
        give_movie_a_file(pool, m).await;
        cast_in_movie(pool, m, actor, "Lead", 0).await;
    }

    let outcome = call(
        state,
        &who,
        session,
        "person_credits",
        json!({"person": "Prolific Pat", "limit": 2}),
    )
    .await;

    assert!(outcome.ok, "{}", outcome.detail);
    assert_eq!(titles(&outcome, "movies").len(), 2);
    assert_eq!(
        outcome.observation["in_library"]["movies"], 5,
        "the count is of everything held, not of what fitted"
    );
    assert_eq!(outcome.observation["truncated"], true);
    let say = outcome.observation["say"].as_str().unwrap();
    assert!(say.contains("only listing 2"), "{say}");
}

/// Films only, when she said films.
#[tokio::test]
async fn a_media_type_narrows_the_credits_to_one_kind() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let actor = person(pool, "Both Ways Bev", Some("999")).await;
    let m = movie(pool, "Her Film", 2004, "6001").await;
    give_movie_a_file(pool, m).await;
    cast_in_movie(pool, m, actor, "Her", 0).await;
    let s = show(pool, "Her Show", 2006, "6002").await;
    give_show_an_episode(pool, s, true).await;
    cast_in_show(pool, s, actor, "Her", 0).await;

    let outcome = call(
        state,
        &who,
        session,
        "person_credits",
        json!({"person": "Both Ways Bev", "media_type": "movie"}),
    )
    .await;

    assert_eq!(titles(&outcome, "movies"), vec!["Her Film"]);
    assert!(titles(&outcome, "shows").is_empty());
    assert_eq!(outcome.observation["in_library"]["shows"], 0);
}

// =============================================================================
// cast_of
// =============================================================================

#[tokio::test]
async fn a_films_cast_comes_back_in_billing_order_with_its_director() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let film = movie(pool, "A Very Specific Film", 1999, "7001").await;
    give_movie_a_file(pool, film).await;
    let lead = person(pool, "Lead Actor", Some("7101")).await;
    let second = person(pool, "Second Billed", Some("7102")).await;
    let third = person(pool, "Third Billed", Some("7103")).await;
    // Inserted out of order on purpose: cast_order is the ordering, not insertion.
    cast_in_movie(pool, film, third, "The Friend", 2).await;
    cast_in_movie(pool, film, lead, "The Hero", 0).await;
    cast_in_movie(pool, film, second, "The Villain", 1).await;
    let helmer = person(pool, "Helen Helmer", Some("7104")).await;
    directed_movie(pool, film, helmer).await;

    let outcome = call(
        state,
        &who,
        session,
        "cast_of",
        json!({"title": "A Very Specific Film"}),
    )
    .await;

    assert!(outcome.ok, "{}", outcome.detail);
    assert_eq!(outcome.observation["found"], true);
    assert_eq!(outcome.observation["type"], "movie");
    assert_eq!(outcome.observation["in_library"], true);
    assert_eq!(
        names(&outcome, "cast"),
        vec!["Lead Actor", "Second Billed", "Third Billed"]
    );
    assert_eq!(outcome.observation["cast"][0]["character"], "The Hero");
    assert_eq!(names(&outcome, "crew"), vec!["Helen Helmer"]);
    assert_eq!(outcome.observation["crew"][0]["job"], "Director");
    assert_eq!(outcome.observation["truncated"], false);
    assert_eq!(outcome.sources.len(), 1);
}

#[tokio::test]
async fn a_shows_cast_answers_too_and_a_long_one_is_marked_truncated() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let series = show(pool, "A Very Specific Show", 2001, "7201").await;
    give_show_an_episode(pool, series, true).await;
    for n in 0..6 {
        let p = person(pool, &format!("Regular {n}"), Some(&format!("73{n:02}"))).await;
        cast_in_show(pool, series, p, &format!("Character {n}"), n).await;
    }

    let outcome = call(
        state,
        &who,
        session,
        "cast_of",
        json!({"title": "A Very Specific Show", "limit": 3}),
    )
    .await;

    assert!(outcome.ok, "{}", outcome.detail);
    assert_eq!(outcome.observation["type"], "show");
    assert_eq!(names(&outcome, "cast").len(), 3);
    assert_eq!(outcome.observation["truncated"], true);
    let say = outcome.observation["say"].as_str().unwrap();
    assert!(say.contains("not everyone"), "{say}");
}

/// Knowing the cast of something the household does not own is fine — saying
/// they own it is not.
#[tokio::test]
async fn the_cast_of_something_not_held_is_answered_and_flagged() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let film = movie(pool, "A Film Nobody Downloaded", 2020, "7301").await;
    let p = person(pool, "Some Actor", Some("7302")).await;
    cast_in_movie(pool, film, p, "Someone", 0).await;

    let outcome = call(
        state,
        &who,
        session,
        "cast_of",
        json!({"title": "A Film Nobody Downloaded"}),
    )
    .await;

    assert!(outcome.ok, "{}", outcome.detail);
    assert_eq!(outcome.observation["found"], true);
    assert_eq!(outcome.observation["in_library"], false);
    assert_eq!(names(&outcome, "cast"), vec!["Some Actor"]);
    let say = outcome.observation["say"].as_str().unwrap();
    assert!(say.contains("don't actually have"), "{say}");
}

#[tokio::test]
async fn an_unknown_title_comes_back_found_false() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let outcome = call(
        state,
        &who,
        session,
        "cast_of",
        json!({"title": "Nothing By This Name At All"}),
    )
    .await;

    assert!(outcome.ok, "{}", outcome.detail);
    assert_eq!(outcome.observation["found"], false);
    assert!(outcome.observation["cast"].is_null());
}

/// Picking one of two would be the failure the whole loop is built to avoid.
#[tokio::test]
async fn two_titles_that_both_match_become_a_question_not_a_guess() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let a = movie(pool, "Ambiguous Thing One", 1990, "7401").await;
    give_movie_a_file(pool, a).await;
    let b = movie(pool, "Ambiguous Thing Two", 2005, "7402").await;
    give_movie_a_file(pool, b).await;

    let outcome = call(
        state,
        &who,
        session,
        "cast_of",
        json!({"title": "Ambiguous Thing"}),
    )
    .await;

    assert!(outcome.ok, "{}", outcome.detail);
    assert_eq!(outcome.observation["ambiguous"], true);
    assert_eq!(
        outcome.observation["candidates"].as_array().unwrap().len(),
        2
    );
    let note = outcome.observation["note"].as_str().unwrap();
    assert!(note.contains("do not pick"), "{note}");
}

/// A dismissed credit is symmetric: gone from the person's list, and gone from
/// the title's cast.
#[tokio::test]
async fn a_dismissed_credit_is_gone_from_the_titles_cast_too() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let film = movie(pool, "A Film With A Bad Credit", 2015, "7501").await;
    give_movie_a_file(pool, film).await;
    let real = person(pool, "Genuinely In It", Some("7502")).await;
    let bogus = person(pool, "Not Really In It", Some("7503")).await;
    cast_in_movie(pool, film, real, "The Lead", 0).await;
    cast_in_movie(pool, film, bogus, "Nobody", 1).await;
    dismiss(pool, bogus, 7501, "movie").await;

    let outcome = call(
        state,
        &who,
        session,
        "cast_of",
        json!({"title": "A Film With A Bad Credit"}),
    )
    .await;

    assert_eq!(names(&outcome, "cast"), vec!["Genuinely In It"]);
}

// =============================================================================
// Registry
// =============================================================================

/// Both are reads, so a viewer gets them. A confirmation gate on a question is
/// friction with nothing behind it.
#[tokio::test]
async fn both_tools_are_open_to_a_viewer_and_need_no_confirmation() {
    use api::chatbot::tools::{Role, Tool};

    assert_eq!(Tool::PersonCredits.required_role(), Role::Any);
    assert_eq!(Tool::CastOf.required_role(), Role::Any);
    assert_eq!(Tool::parse("person_credits"), Some(Tool::PersonCredits));
    assert_eq!(Tool::parse("cast_of"), Some(Tool::CastOf));

    for tool in [Tool::PersonCredits, Tool::CastOf] {
        let schema = tool.schema();
        let properties = &schema["function"]["parameters"]["properties"];
        assert!(properties["confirm"].is_null(), "{}", tool.name());
        let required = schema["function"]["parameters"]["required"]
            .as_array()
            .expect("a required list");
        assert_eq!(required.len(), 1, "{} asks for too much", tool.name());
    }
}

/// A guest star has no `tv_cast` row at all — the credit only exists per
/// episode, and a show-level-only query would lose them entirely.
#[tokio::test]
async fn a_guest_star_is_found_through_the_episode_credits() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let series = show(pool, "A Show With Guests", 2011, "7601").await;
    let episode_id = give_show_an_episode(pool, series, true).await;
    let guest = person(pool, "Guesty McGuest", Some("7602")).await;
    sqlx::query(
        "INSERT INTO tv_episode_cast (id, episode_id, person_id, credit_type, character_name, cast_order)
         VALUES ($1, $2, $3, 'guest', 'The Visitor', 0)",
    )
    .bind(Uuid::now_v7())
    .bind(episode_id)
    .bind(guest)
    .execute(pool)
    .await
    .unwrap();

    let outcome = call(
        state,
        &who,
        session,
        "person_credits",
        json!({"person": "Guesty McGuest"}),
    )
    .await;

    assert!(outcome.ok, "{}", outcome.detail);
    assert_eq!(titles(&outcome, "shows"), vec!["A Show With Guests"]);
    assert_eq!(outcome.observation["shows"][0]["guest_in_episodes"], 1);
    assert!(outcome.observation["shows"][0]["character"].is_null());
}
