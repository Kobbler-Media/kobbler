//! Live TV: the guide the bot reads, and the recording it sets.
//!
//! The assertions are about side effects and about honesty, in that order:
//!
//! * `iptv_recordings` gains **exactly one** row on the happy path, and **none**
//!   at all when the call was unconfirmed, stale, ambiguous or made by a viewer;
//! * an empty guide is reported as an empty guide, never as an empty schedule.
//!
//! No LLM, no GPU, no Meili, no network. Everything here is Postgres and clocks.

#![allow(clippy::expect_fun_call)]
#![allow(dead_code)]
#![allow(unused_imports)]
// The shared `tests/common/` harness trips this; every test target that pulls it
// in allows it at the crate root rather than each one rewriting the harness.
#![allow(clippy::collapsible_if)]

mod common;

use api::AppState;
use api::chatbot::tools::Tool;
use api::chatbot::tools::dispatch::{self, Ctx};
use api::chatbot::tools::read::ToolOutcome;
use api::repositories::chat_sessions::{self, NewSession};
use api::routes::AuthUser;
use common::*;
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime, UtcOffset};
use uuid::Uuid;

// =============================================================================
// Harness
// =============================================================================

/// Every call goes through `dispatch`, because that is where the role check
/// lives. Calling `iptv::*` directly would test the tool and skip the gate.
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

fn auth(user_id: Uuid, role: &str) -> AuthUser {
    AuthUser {
        user_id,
        role: role.to_string(),
        is_kids: false,
    }
}

async fn session(state: &AppState, user_id: Uuid) -> Uuid {
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

/// There is no confirmation step: the call acts.
async fn confirmed(
    state: &AppState,
    auth: &AuthUser,
    session_id: Uuid,
    args: Value,
) -> ToolOutcome {
    call(state, auth, session_id, "record_this", args).await
}

// =============================================================================
// Fixtures
// =============================================================================

async fn channel(pool: &sqlx::PgPool, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO iptv_channels (id, name, normalized_name, enabled, display_order)
         VALUES ($1, $2, $3, true, 0)",
    )
    .bind(id)
    .bind(name)
    .bind(name.to_lowercase())
    .execute(pool)
    .await
    .expect("channel");
    id
}

async fn programme(
    pool: &sqlx::PgPool,
    channel_id: Uuid,
    title: &str,
    start: OffsetDateTime,
    stop: OffsetDateTime,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO iptv_epg (id, channel_id, title, start_time, stop_time)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(channel_id)
    .bind(title)
    .bind(start)
    .bind(stop)
    .execute(pool)
    .await
    .expect("epg");
    id
}

fn local_offset() -> UtcOffset {
    UtcOffset::from_whole_seconds(chrono::Local::now().offset().local_minus_utc())
        .unwrap_or(UtcOffset::UTC)
}

/// A slot that is inside the `tonight` window whatever time the suite runs at:
/// the window is local 18:00 to local midnight, clamped forward to now, so a
/// programme spanning the whole evening always overlaps it.
fn this_evening(now: OffsetDateTime) -> (OffsetDateTime, OffsetDateTime) {
    let offset = local_offset();
    let start = now
        .to_offset(offset)
        .date()
        .with_hms(18, 0, 0)
        .expect("18:00")
        .assume_offset(offset);
    (start, start + Duration::hours(6))
}

async fn recording_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM iptv_recordings")
        .fetch_one(pool)
        .await
        .expect("count")
}

fn titles(outcome: &ToolOutcome) -> Vec<String> {
    outcome.observation["programmes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| p["title"].as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

// =============================================================================
// Registry consistency
// =============================================================================

/// `Tool::ALL` is hand-maintained, and a variant left out of it is a tool the
/// model is never told about — `parse`, `schemas` and the prompt all read the
/// array, not the enum. The match below is exhaustive on purpose: adding a
/// variant without adding it here fails to compile, which is the only way to
/// catch the omission at all.
#[test]
fn every_variant_is_advertised_dispatched_and_described() {
    fn in_all(tool: Tool) -> bool {
        match tool {
            Tool::ResolveMedia
            | Tool::SearchLibrary
            | Tool::LibraryStats
            | Tool::FindPassage
            | Tool::WhatsDownloading
            | Tool::RecentlyAdded
            | Tool::NextAiring
            | Tool::SeasonStatus
            | Tool::Recommend
            | Tool::WhatsNew
            | Tool::ListReleases
            | Tool::GrabRelease
            | Tool::Regrab
            | Tool::WantIt
            | Tool::CancelDownload
            | Tool::AddToWatchlist
            | Tool::SetNotificationPrefs
            | Tool::WhatsOn
            | Tool::RecordThis
            | Tool::PersonCredits
            | Tool::CastOf
            | Tool::LivetvSearchGuide
            | Tool::LivetvScheduleRecording
            | Tool::LivetvCreateSeriesRule
            | Tool::LivetvListRecordings
            | Tool::LivetvCancel => Tool::ALL.contains(&tool),
        }
    }
    for tool in [Tool::WhatsOn, Tool::RecordThis] {
        assert!(in_all(tool), "{} is missing from Tool::ALL", tool.name());
    }
    for tool in Tool::ALL {
        assert!(in_all(tool), "{} is missing from Tool::ALL", tool.name());
        assert_eq!(Tool::parse(tool.name()), Some(tool), "{}", tool.name());
        assert!(
            tool.schema()["function"]["parameters"]["properties"].is_object(),
            "{} has no parameters",
            tool.name()
        );
    }
    assert_eq!(Tool::parse("whats_on"), Some(Tool::WhatsOn));
    assert_eq!(Tool::parse("record_this"), Some(Tool::RecordThis));
    let names: Vec<&str> = Tool::ALL.iter().map(|t| t.name()).collect();
    assert!(names.contains(&"whats_on") && names.contains(&"record_this"));
}

/// Reading the guide is free; recording is not.
#[test]
fn whats_on_is_open_and_record_this_is_not() {
    use api::chatbot::tools::Role;
    assert_eq!(Tool::WhatsOn.required_role(), Role::Any);
    assert_eq!(Tool::RecordThis.required_role(), Role::Contributor);
}

// =============================================================================
// whats_on
// =============================================================================

/// Every read case shares one server on purpose. Standing a `TestServer` up per
/// assertion put the suite over the five-minute window after which a parallel
/// run's cleanup drops the template database out from under it.
#[tokio::test]
async fn the_guide_is_read_honestly_and_narrowly() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "guide@test.com", "viewer").await;
    let auth = auth(user, "viewer");
    let sid = session(state, user).await;

    let now = OffsetDateTime::now_utc();
    let one = channel(&server.pool, "BBC One").await;
    let two = channel(&server.pool, "BBC Two").await;
    let sport = channel(&server.pool, "Sky Sports Main Event").await;
    let dave = channel(&server.pool, "Dave").await;
    let film4 = channel(&server.pool, "Film4").await;
    let itv = channel(&server.pool, "ITV1").await;
    let _ = dave;

    programme(
        &server.pool,
        one,
        "The Six O'Clock News",
        now - Duration::minutes(30),
        now + Duration::minutes(30),
    )
    .await;
    programme(
        &server.pool,
        one,
        "Much Later",
        now + Duration::hours(6),
        now + Duration::hours(7),
    )
    .await;

    let (evening_start, evening_end) = this_evening(now);
    programme(
        &server.pool,
        two,
        "Evening Film",
        evening_start,
        evening_end,
    )
    .await;
    programme(
        &server.pool,
        two,
        "In An Hour",
        now + Duration::hours(1),
        now + Duration::hours(2),
    )
    .await;

    programme(
        &server.pool,
        sport,
        "The Big Match",
        now - Duration::minutes(5),
        now + Duration::minutes(25),
    )
    .await;

    programme(
        &server.pool,
        film4,
        "Next Week",
        now + Duration::days(5),
        now + Duration::days(5) + Duration::hours(2),
    )
    .await;

    for i in 0..30i64 {
        programme(
            &server.pool,
            itv,
            &format!("Slot {i}"),
            now + Duration::minutes(i * 5),
            now + Duration::minutes(i * 5 + 5),
        )
        .await;
    }

    // "now" is the instant, not the evening.
    let out = call(
        state,
        &auth,
        sid,
        "whats_on",
        json!({"channel": "BBC One", "when": "now"}),
    )
    .await;
    assert!(out.ok, "{}", out.observation);
    assert_eq!(out.observation["found"], json!(true));
    assert_eq!(titles(&out), vec!["The Six O'Clock News"]);
    assert_eq!(out.observation["programmes"][0]["on_now"], json!(true));

    // "tonight" reaches the evening; "later" reaches the next few hours. Neither
    // is arithmetic the model was asked to do.
    let tonight = call(
        state,
        &auth,
        sid,
        "whats_on",
        json!({"channel": "BBC Two", "when": "tonight"}),
    )
    .await;
    assert!(
        titles(&tonight).contains(&"Evening Film".to_string()),
        "tonight missed the evening: {}",
        tonight.observation
    );
    let later = call(
        state,
        &auth,
        sid,
        "whats_on",
        json!({"channel": "BBC Two", "when": "later"}),
    )
    .await;
    assert!(
        titles(&later).contains(&"In An Hour".to_string()),
        "later missed the next hour: {}",
        later.observation
    );

    // A named channel is the only channel.
    let named = call(
        state,
        &auth,
        sid,
        "whats_on",
        json!({"channel": "Sky Sports Main Event"}),
    )
    .await;
    assert_eq!(
        titles(&named),
        vec!["The Big Match"],
        "{}",
        named.observation
    );

    // No guide at all is not the same answer as nothing being on.
    let blank = call(state, &auth, sid, "whats_on", json!({"channel": "Dave"})).await;
    assert_eq!(blank.observation["found"], json!(false));
    assert_eq!(blank.observation["guide_data"], json!("none"));
    assert!(
        blank.observation["note"]
            .as_str()
            .unwrap_or_default()
            .contains("no guide data"),
        "note did not name the missing listings: {}",
        blank.observation
    );

    // A guide that simply does not cover the window says that instead.
    let outside = call(
        state,
        &auth,
        sid,
        "whats_on",
        json!({"channel": "Film4", "when": "now"}),
    )
    .await;
    assert_eq!(outside.observation["found"], json!(false));
    assert_eq!(outside.observation["guide_data"], json!("outside_window"));

    // An unknown channel is named back, never guessed at.
    let unknown = call(state, &auth, sid, "whats_on", json!({"channel": "Zorg TV"})).await;
    assert_eq!(unknown.observation["found"], json!(false));
    assert_eq!(unknown.observation["unknown_channel"], json!("Zorg TV"));

    // A wide guide is cut in the tool, before it can drown a 12B model.
    let wide = call(
        state,
        &auth,
        sid,
        "whats_on",
        json!({"channel": "ITV1", "when": "later"}),
    )
    .await;
    let showing = wide.observation["showing"].as_u64().expect("showing");
    assert!(showing <= 12, "{showing} programmes is too many");
    assert_eq!(wide.observation["truncated"], json!(true));
    assert_eq!(titles(&wide).len() as u64, showing);
    let narrow = call(
        state,
        &auth,
        sid,
        "whats_on",
        json!({"channel": "ITV1", "when": "later", "limit": 3}),
    )
    .await;
    assert_eq!(narrow.observation["showing"], json!(3));

    // Reading the guide is free: everything above ran as a viewer, including the
    // channel-less sweep.
    let sweep = call(state, &auth, sid, "whats_on", json!({})).await;
    assert!(
        sweep.ok,
        "a viewer was refused the guide: {}",
        sweep.observation
    );
    assert_eq!(sweep.observation["found"], json!(true));
}

// =============================================================================
// record_this
// =============================================================================

#[tokio::test]
async fn a_confirmed_recording_schedules_exactly_one_row_and_says_so() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "rec@test.com", "contributor").await;
    let auth = auth(user, "contributor");
    let sid = session(state, user).await;

    let now = OffsetDateTime::now_utc();
    let ch = channel(&server.pool, "BBC One").await;
    let epg = programme(
        &server.pool,
        ch,
        "Match of the Day",
        now + Duration::hours(3),
        now + Duration::hours(4),
    )
    .await;
    let args = json!({"programme_id": epg.to_string()});

    let out = confirmed(state, &auth, sid, args.clone()).await;
    assert!(out.ok, "{}", out.observation);
    assert_eq!(out.observation["scheduled"], json!(true));
    assert_eq!(recording_count(&server.pool).await, 1);

    let row = sqlx::query_as::<_, (Uuid, Option<Uuid>, String, String)>(
        "SELECT channel_id, epg_id, title, status FROM iptv_recordings",
    )
    .fetch_one(&server.pool)
    .await
    .expect("row");
    assert_eq!(row.0, ch);
    assert_eq!(row.1, Some(epg));
    assert_eq!(row.2, "Match of the Day");
    assert_eq!(row.3, "scheduled");

    // Every action needs a terminal message, and a recording that fires in three
    // hours is exactly where silence reads as broken.
    let say = out.observation["say"].as_str().unwrap_or_default();
    assert!(
        say.contains("Match of the Day") && say.contains("BBC One"),
        "the terminal message named neither the programme nor the channel: {say}"
    );

    // Asking twice is the normal shape of a conversation; two ffmpeg captures of
    // one programme is not.
    let again = confirmed(state, &auth, sid, args).await;
    assert_eq!(again.observation["already_scheduled"], json!(true));
    assert_eq!(recording_count(&server.pool).await, 1);

    // Live TV cannot be recorded after the fact.
    let over = programme(
        &server.pool,
        ch,
        "Last Night's Film",
        now - Duration::hours(4),
        now - Duration::hours(2),
    )
    .await;
    let past = call(
        state,
        &auth,
        sid,
        "record_this",
        json!({"programme_id": over.to_string()}),
    )
    .await;
    assert_eq!(past.observation["already_over"], json!(true));
    assert_eq!(past.observation["scheduled"], json!(false));
    assert_eq!(recording_count(&server.pool).await, 1);
}

/// Two channels showing the same thing is a question, not a coin flip.
#[tokio::test]
async fn ambiguity_is_a_question_and_a_miss_is_admitted() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "ambig@test.com", "contributor").await;
    let auth = auth(user, "contributor");
    let sid = session(state, user).await;

    let now = OffsetDateTime::now_utc();
    let one = channel(&server.pool, "BBC One").await;
    let sport = channel(&server.pool, "Sky Sports Main Event").await;
    for ch in [one, sport] {
        programme(
            &server.pool,
            ch,
            "The Big Match",
            now + Duration::hours(2),
            now + Duration::hours(4),
        )
        .await;
    }

    let out = call(
        state,
        &auth,
        sid,
        "record_this",
        json!({"title": "The Big Match"}),
    )
    .await;
    assert_eq!(
        out.observation["ambiguous"],
        json!(true),
        "{}",
        out.observation
    );
    assert_eq!(out.observation["scheduled"], json!(false));
    let candidates = out.observation["candidates"]
        .as_array()
        .expect("candidates");
    assert_eq!(candidates.len(), 2);
    assert!(candidates.iter().all(|c| c["programme_id"].is_string()));
    assert_eq!(recording_count(&server.pool).await, 0);

    // Nothing in the guide is said plainly rather than promised.
    let missing = call(
        state,
        &auth,
        sid,
        "record_this",
        json!({"title": "Something Invented"}),
    )
    .await;
    assert_eq!(missing.observation["found"], json!(false));
    assert_eq!(missing.observation["scheduled"], json!(false));
    assert_eq!(recording_count(&server.pool).await, 0);

    // Naming the channel settles it, and only then does it schedule.
    let settled = confirmed(
        state,
        &auth,
        sid,
        json!({"title": "The Big Match", "channel": "Sky Sports Main Event"}),
    )
    .await;
    assert_eq!(settled.observation["scheduled"], json!(true));
    assert_eq!(recording_count(&server.pool).await, 1);
    let channel_of: Uuid = sqlx::query_scalar("SELECT channel_id FROM iptv_recordings")
        .fetch_one(&server.pool)
        .await
        .expect("channel");
    assert_eq!(channel_of, sport, "it recorded the wrong channel");
}

/// The refusal has to happen in `dispatch`, before the tool, or "no permission"
/// is something we say after having already scheduled it.
#[tokio::test]
async fn a_viewer_is_refused_before_anything_is_written() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "viewer-rec@test.com", "viewer").await;
    let auth = auth(user, "viewer");
    let sid = session(state, user).await;

    let now = OffsetDateTime::now_utc();
    let ch = channel(&server.pool, "BBC One").await;
    let epg = programme(
        &server.pool,
        ch,
        "Match of the Day",
        now + Duration::hours(3),
        now + Duration::hours(4),
    )
    .await;

    for args in [
        json!({"programme_id": epg.to_string()}),
        json!({"programme_id": epg.to_string(), "confirm": "yes", "confirm_token": Uuid::now_v7().to_string()}),
        json!({"title": "Match of the Day"}),
    ] {
        let out = call(state, &auth, sid, "record_this", args).await;
        assert!(!out.ok, "a viewer got through: {}", out.observation);
        assert!(
            out.detail.contains("permission"),
            "the refusal did not say why: {}",
            out.detail
        );
    }
    assert_eq!(recording_count(&server.pool).await, 0);
}
