//! `season_status`.
//!
//! The live failure this replaces, in order: "how many episodes does the
//! current season of someShow have" answered 22 — the *show's*
//! `available_episode_count`, because the seasons projection carries a total
//! and no availability at all. Pushed back, it said 8 in total, 4 held, and
//! then listed 3, off a fuzzy-matched page of episodes truncated at ten. The
//! truth was 6 of 8 with 2 not yet aired, and nothing in Meili can express that
//! last clause at all.
//!
//! So the load-bearing assertions are: the counts are the season's and never
//! the show's, an unaired episode is never a gap, and the `say` line states
//! held, aired and to-come outright so the model has no arithmetic left to get
//! wrong.
//!
//! No Meili, no LLM, no GPU — these are postgres joins.

use crate::common::*;
use api::AppState;
use api::chatbot::tools::dispatch::{self, Ctx};
use api::chatbot::tools::read::ToolOutcome;
use api::repositories::chat_sessions::{self, NewSession};
use api::routes::AuthUser;
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime};
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
async fn call(state: &AppState, who: &AuthUser, session_id: Uuid, args: Value) -> ToolOutcome {
    let ctx = Ctx {
        state,
        auth: who,
        session_id,
        context_ref: None,
        user_message: "",
    };
    dispatch::dispatch(&ctx, "season_status", &args).await
}

async fn show(pool: &sqlx::PgPool, title: &str, year: i32) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO tv_shows (id, title, year, identifiers) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(title)
        .bind(year)
        .bind(json!({"tmdb_id": id.to_string()}))
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn season(
    pool: &sqlx::PgPool,
    show_id: Uuid,
    number: i32,
    episode_count: Option<i32>,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_seasons
             (id, show_id, season_identifier, season_sort_order, is_special, episode_count)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(show_id)
    .bind(number.to_string())
    .bind(number)
    .bind(number == 0)
    .bind(episode_count)
    .execute(pool)
    .await
    .unwrap();
    id
}

fn days_from_now(days: i64) -> time::Date {
    (OffsetDateTime::now_utc() + Duration::days(days)).date()
}

/// One episode, with an air date relative to today and optionally a file behind
/// it. `air_in` of `None` is the episode nobody has dated, which is neither
/// aired nor to come.
async fn episode(
    pool: &sqlx::PgPool,
    season_id: Uuid,
    number: i32,
    air_in: Option<i64>,
    held: bool,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tv_episodes
             (id, season_id, episode_identifier, episode_sort_order, title, air_date)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(season_id)
    .bind(number.to_string())
    .bind(number)
    .bind(format!("Episode {number}"))
    .bind(air_in.map(days_from_now))
    .execute(pool)
    .await
    .unwrap();
    if held {
        let file_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO tv_episode_files (id, file_path, format, available)
             VALUES ($1, $2, 'mkv', true)",
        )
        .bind(file_id)
        .bind(format!("/media/{id}.mkv"))
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tv_episode_file_episodes (episode_file_id, episode_id) VALUES ($1, $2)",
        )
        .bind(file_id)
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    }
    id
}

fn episodes(outcome: &ToolOutcome, key: &str) -> Vec<String> {
    outcome.observation[key]
        .as_array()
        .expect(key)
        .iter()
        .map(|v| v["episode"].as_str().unwrap().to_string())
        .collect()
}

fn say(outcome: &ToolOutcome) -> String {
    outcome.observation["say"]
        .as_str()
        .expect("say")
        .to_string()
}

/// The shape that failed live: an eight-episode season, six aired and held, two
/// still to come, plus a fat back catalogue whose episode count is the number
/// the bot reached for instead.
async fn colin_from_accounts(pool: &sqlx::PgPool) -> Uuid {
    let show_id = show(pool, "someShow", 2021).await;

    let s1 = season(pool, show_id, 1, Some(8)).await;
    for n in 1..=8 {
        episode(pool, s1, n, Some(-900 + i64::from(n) * 7), true).await;
    }
    let s2 = season(pool, show_id, 2, Some(8)).await;
    for n in 1..=8 {
        episode(pool, s2, n, Some(-500 + i64::from(n) * 7), true).await;
    }
    let s3 = season(pool, show_id, 3, Some(8)).await;
    for n in 1..=6 {
        episode(pool, s3, n, Some(-42 + i64::from(n) * 7), true).await;
    }
    for n in 7..=8 {
        episode(pool, s3, n, Some(i64::from(n) - 6), false).await;
    }
    show_id
}

// =============================================================================
// The live failure
// =============================================================================

/// End to end: 8 in the season, 6 aired, 6 held, 2 to come — never 22, and
/// never "2 missing".
#[tokio::test]
async fn the_current_season_is_counted_as_itself_and_not_as_the_whole_show() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    colin_from_accounts(pool).await;

    let out = call(state, &who, session, json!({"show": "someShow"})).await;
    assert!(out.ok, "{:?}", out.observation);
    let o = &out.observation;

    assert_eq!(o["found"], json!(true));
    assert_eq!(
        o["season"],
        json!("3"),
        "the current season is the newest aired one"
    );
    assert_eq!(o["picked_current_season"], json!(true));
    assert_eq!(o["episodes_in_season"], json!(8));
    assert_eq!(o["aired"], json!(6));
    assert_eq!(o["still_to_come"], json!(2));
    assert_eq!(o["held"], json!(6));
    assert_eq!(o["held_of_aired"], json!(6));
    assert_eq!(o["missing_aired_count"], json!(0));
    assert!(o["missing_aired"].as_array().unwrap().is_empty());
    assert_eq!(
        episodes(&out, "still_to_come_episodes"),
        ["S03E07", "S03E08"]
    );

    // 22 is the show's available_episode_count — the number the live answer used.
    let say = say(&out);
    assert!(!say.contains("22"), "{say}");
    assert!(say.contains("has 8 episodes"), "{say}");
    assert!(say.contains("6 episodes have aired"), "{say}");
    assert!(say.contains("you have 6 of 6"), "{say}");
    assert!(say.contains("The other 2 have not aired yet"), "{say}");
    assert!(say.contains("not missing"), "{say}");
}

/// The second half of the live failure: an unaired episode counted as a gap.
/// Only the aired one is actionable, and only it may be named.
#[tokio::test]
async fn only_aired_episodes_can_be_missing() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let show_id = show(pool, "someShow", 2021).await;
    let s = season(pool, show_id, 1, Some(4)).await;
    episode(pool, s, 1, Some(-21), true).await;
    episode(pool, s, 2, Some(-14), false).await;
    episode(pool, s, 3, Some(-7), true).await;
    episode(pool, s, 4, Some(7), false).await;

    let out = call(
        state,
        &who,
        session,
        json!({"show": "someShow", "season": 1}),
    )
    .await;
    let o = &out.observation;

    assert_eq!(o["aired"], json!(3));
    assert_eq!(o["held_of_aired"], json!(2));
    assert_eq!(o["missing_aired_count"], json!(1));
    assert_eq!(episodes(&out, "missing_aired"), ["S01E02"]);
    assert_eq!(o["still_to_come"], json!(1));
    assert_eq!(episodes(&out, "still_to_come_episodes"), ["S01E04"]);

    let say = say(&out);
    assert!(say.contains("1 episode that has aired is missing"), "{say}");
    assert!(say.contains("S01E02"), "{say}");
    assert!(
        !say.contains("S01E04"),
        "the unaired one is not a gap: {say}"
    );
    assert!(say.contains("The other one has not aired yet"), "{say}");
}

// =============================================================================
// Which season "the current one" is
// =============================================================================

/// A season TMDB has announced and nobody has seen is not the current one — it
/// has no episodes yet, which is the wrong answer to "how many episodes does
/// the current season have".
#[tokio::test]
async fn an_announced_season_that_has_not_started_is_not_the_current_one() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let show_id = show(pool, "someShow", 2020).await;
    let s2 = season(pool, show_id, 2, Some(2)).await;
    episode(pool, s2, 1, Some(-30), true).await;
    episode(pool, s2, 2, Some(-23), true).await;
    let s3 = season(pool, show_id, 3, Some(6)).await;
    for n in 1..=6 {
        episode(pool, s3, n, Some(30 + i64::from(n) * 7), false).await;
    }

    let out = call(state, &who, session, json!({"show": "someShow"})).await;
    assert_eq!(out.observation["season"], json!("2"));

    // Asked for by name, the unaired season answers honestly rather than
    // reporting six gaps.
    let out = call(
        state,
        &who,
        session,
        json!({"show": "someShow", "season": 3}),
    )
    .await;
    assert_eq!(out.observation["season"], json!("3"));
    assert_eq!(out.observation["aired"], json!(0));
    assert_eq!(out.observation["missing_aired_count"], json!(0));
    assert!(say(&out).contains("None of them have aired yet"));
}

/// Season 0 is never what "the current season" means, and it is not counted
/// into any ordinary season either.
#[tokio::test]
async fn specials_never_win_the_current_season_and_are_reachable_by_number() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let show_id = show(pool, "someShow", 2020).await;
    let specials = season(pool, show_id, 0, Some(3)).await;
    for n in 1..=3 {
        episode(pool, specials, n, Some(-100), n == 1).await;
    }
    let s1 = season(pool, show_id, 1, Some(2)).await;
    episode(pool, s1, 1, Some(-60), true).await;
    episode(pool, s1, 2, Some(-53), true).await;

    let out = call(state, &who, session, json!({"show": "someShow"})).await;
    assert_eq!(out.observation["season"], json!("1"));
    assert_eq!(out.observation["is_specials"], json!(false));
    assert_eq!(
        out.observation["episodes_in_season"],
        json!(2),
        "the three specials are not part of season 1"
    );

    let out = call(
        state,
        &who,
        session,
        json!({"show": "someShow", "season": 0}),
    )
    .await;
    assert_eq!(out.observation["is_specials"], json!(true));
    assert_eq!(out.observation["episodes_in_season"], json!(3));
    assert_eq!(out.observation["missing_aired_count"], json!(2));
    assert!(
        say(&out).starts_with("The specials of someShow"),
        "{}",
        say(&out)
    );
}

/// Nothing has aired at all: the show has not started, so the season about to
/// is the first one, not the last one announced.
#[tokio::test]
async fn a_show_that_has_not_started_reports_its_first_season() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let show_id = show(pool, "someShow", 2027).await;
    let s1 = season(pool, show_id, 1, Some(1)).await;
    episode(pool, s1, 1, Some(30), false).await;
    let s2 = season(pool, show_id, 2, Some(1)).await;
    episode(pool, s2, 1, Some(400), false).await;

    let out = call(state, &who, session, json!({"show": "someShow"})).await;
    assert_eq!(out.observation["season"], json!("1"));
    assert_eq!(out.observation["still_to_come"], json!(1));
}

// =============================================================================
// Honest failures
// =============================================================================

/// A season number that does not exist comes back as the seasons that do — the
/// same ask-do-not-pick shape `next_airing` and `cast_of` use. Sports number
/// their seasons by year, so this is the normal answer to "season 3" of a
/// competition.
#[tokio::test]
async fn a_season_that_does_not_exist_offers_the_ones_that_do() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let show_id = show(pool, "someSport", 2020).await;
    for year in [2024, 2025] {
        let s = season(pool, show_id, year, Some(1)).await;
        episode(pool, s, 1, Some(-30), true).await;
    }

    let out = call(
        state,
        &who,
        session,
        json!({"show": "someSport", "season": 3}),
    )
    .await;
    assert!(out.ok);
    assert_eq!(out.observation["found"], json!(false));
    let offered: Vec<&str> = out.observation["seasons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["season"].as_str().unwrap())
        .collect();
    assert_eq!(offered, ["2024", "2025"]);
    assert!(
        out.observation["note"]
            .as_str()
            .unwrap()
            .contains("do not pick")
    );
}

/// Two shows share a name: ask, never pick. Same contract as `next_airing`.
#[tokio::test]
async fn an_ambiguous_show_asks_rather_than_choosing() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    show(pool, "someShow Origins", 2004).await;
    show(pool, "someShow Reloaded", 2019).await;

    let out = call(state, &who, session, json!({"show": "someShow"})).await;
    assert!(out.ok);
    assert_eq!(out.observation["ambiguous"], json!(true));
    assert_eq!(out.observation["candidates"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn a_show_nobody_has_is_said_plainly() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let out = call(state, &who, session, json!({"show": "someShow"})).await;
    assert!(out.ok);
    assert_eq!(out.observation["found"], json!(false));
}

/// The episodes we have no row for at all are a different problem from the ones
/// we do not hold, and conflating them is how "8 in total" turns into "4 gaps".
#[tokio::test]
async fn a_season_shorter_than_its_own_listing_says_so() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let show_id = show(pool, "someShow", 2020).await;
    let s = season(pool, show_id, 1, Some(10)).await;
    for n in 1..=4 {
        episode(pool, s, n, Some(-30 + i64::from(n)), true).await;
    }

    let out = call(
        state,
        &who,
        session,
        json!({"show": "someShow", "season": 1}),
    )
    .await;
    assert_eq!(out.observation["episodes_in_season"], json!(4));
    assert_eq!(out.observation["episodes_listed_by_metadata"], json!(10));
    assert!(say(&out).contains("listing says 10"), "{}", say(&out));
}

/// An episode with no air date is neither aired nor to come, and saying so is
/// better than deciding for it.
#[tokio::test]
async fn an_undated_episode_is_reported_as_undated() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let pool = &state.pool;
    let user_id = create_user_with_role(&server, "her@test.com", "viewer").await;
    let who = auth(user_id, "viewer");
    let session = session_for(state, user_id).await;

    let show_id = show(pool, "someShow", 2020).await;
    let s = season(pool, show_id, 1, Some(2)).await;
    episode(pool, s, 1, Some(-30), true).await;
    episode(pool, s, 2, None, false).await;

    let out = call(
        state,
        &who,
        session,
        json!({"show": "someShow", "season": 1}),
    )
    .await;
    assert_eq!(out.observation["aired"], json!(1));
    assert_eq!(out.observation["still_to_come"], json!(0));
    assert_eq!(out.observation["no_air_date"], json!(1));
    assert_eq!(out.observation["missing_aired_count"], json!(0));
    assert_eq!(episodes(&out, "no_air_date_episodes"), ["S01E02"]);
    assert!(say(&out).contains("no air date recorded"), "{}", say(&out));
}

/// Reading how complete a season is costs nothing and changes nothing.
#[tokio::test]
async fn a_viewer_may_count_a_season() {
    use api::chatbot::tools::{Role, Tool};
    assert_eq!(Tool::SeasonStatus.required_role(), Role::Any);
    assert_eq!(Tool::parse("season_status"), Some(Tool::SeasonStatus));
    assert!(Tool::ALL.contains(&Tool::SeasonStatus));
}
