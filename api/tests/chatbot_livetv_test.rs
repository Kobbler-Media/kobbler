//! Recording broadcast TV from chat (Decision 17).
//!
//! The tools are trivial; the clarification behaviour is the feature, so that is
//! what these assert. Three ambiguities the guide really contains, and one
//! resolution:
//!
//! * a series on two channels is a **question**, never a guess;
//! * nine airings of one episode are **one** proposal and a suppression count,
//!   never nine recordings;
//! * a full tuner bank **offers the later repeat** rather than failing quietly.
//!
//! Side effects are counted, not inferred: `livetv_recordings` and
//! `livetv_series_rules` gain exactly the rows the conversation agreed to and
//! never one more.

use crate::common::*;
use api::AppState;
use api::chatbot::tools::dispatch::{self, Ctx};
use api::chatbot::tools::read::ToolOutcome;
use api::chatbot::tools::{Role, Tool};
use api::repositories::chat_sessions::{self, NewSession};
use api::repositories::{livetv_channels, livetv_guide, livetv_tuners};
use api::routes::AuthUser;
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

// =============================================================================
// Harness
// =============================================================================

/// Every call goes through `dispatch`, because that is where the role floor is
/// enforced. Calling `livetv::*` directly would test the tool and skip the gate.
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
    tool: &str,
    args: Value,
) -> ToolOutcome {
    call(state, auth, session_id, tool, args).await
}

// =============================================================================
// Fixtures
// =============================================================================

async fn make_tuner(pool: &sqlx::PgPool, device_id: &str, tuners: i32) -> Uuid {
    livetv_tuners::upsert(
        pool,
        livetv_tuners::CreateTuner {
            device_id: device_id.to_string(),
            friendly_name: "Test Quatro".to_string(),
            model_number: Some("HDHR5-4DT".to_string()),
            firmware_version: Some("20260326".to_string()),
            base_url: "http://127.0.0.1:1".to_string(),
            device_auth: Some("test-auth".to_string()),
            tuner_count: tuners,
        },
    )
    .await
    .expect("tuner")
    .id
}

async fn make_channel(pool: &sqlx::PgPool, tuner_id: Uuid, lcn: i32, name: &str, hd: bool) -> Uuid {
    livetv_channels::upsert_scan(
        pool,
        livetv_channels::UpsertScanChannel {
            tuner_id,
            lcn: Some(lcn),
            scan_name: Some(name.to_string()),
            frequency: 226_500_000 + i64::from(lcn),
            program_number: lcn,
            onid: Some(12931),
            tsid: Some(1793),
            modulation: Some("auto".to_string()),
            video_codec: Some("MPEG4".to_string()),
            audio_codec: Some("AC3".to_string()),
            hd,
            signal_strength: Some(80),
            signal_quality: Some(90),
            initial_kind: "tv".to_string(),
            initial_enabled: true,
        },
    )
    .await
    .expect("channel")
    .0
    .id
}

struct Airing<'a> {
    channel_id: Uuid,
    title: &'a str,
    series_id: &'a str,
    episode: &'a str,
    start_mins: i64,
}

async fn make_guide(pool: &sqlx::PgPool, a: Airing<'_>) -> Uuid {
    let now = OffsetDateTime::now_utc();
    livetv_guide::upsert(
        pool,
        livetv_guide::UpsertGuideEntry {
            channel_id: a.channel_id,
            start_time: now + Duration::minutes(a.start_mins),
            end_time: now + Duration::minutes(a.start_mins + 60),
            title: a.title.to_string(),
            episode_number: Some(a.episode.to_string()),
            episode_title: None,
            synopsis: Some("A synopsis".to_string()),
            original_airdate: None,
            series_id: Some(a.series_id.to_string()),
            genres: vec!["Reality".to_string()],
            image_url: None,
            poster_url: None,
            is_first: false,
            episode_key: Some(format!("{}|E:{}", a.series_id, a.episode)),
            episode_key_kind: Some("episode".to_string()),
        },
    )
    .await
    .expect("guide entry")
}

async fn recording_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM livetv_recordings WHERE status NOT IN ('cancelled', 'moved')",
    )
    .fetch_one(pool)
    .await
    .expect("count")
}

async fn rule_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM livetv_series_rules")
        .fetch_one(pool)
        .await
        .expect("count")
}

fn first_match(outcome: &ToolOutcome) -> &Value {
    outcome.observation["matches"]
        .as_array()
        .and_then(|m| m.first())
        .unwrap_or_else(|| panic!("no matches in {}", outcome.observation))
}

fn ask_about(outcome: &ToolOutcome) -> Vec<String> {
    outcome.observation["ask_about"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

// =============================================================================
// Registry consistency
// =============================================================================

/// A variant left out of `Tool::ALL` is a tool the model is never told about:
/// `parse`, `schemas` and the prompt all read the array, not the enum.
#[test]
fn every_livetv_tool_is_advertised_named_and_gated() {
    let expected = [
        (Tool::LivetvSearchGuide, "livetv_search_guide"),
        (Tool::LivetvScheduleRecording, "livetv_schedule_recording"),
        (Tool::LivetvCreateSeriesRule, "livetv_create_series_rule"),
        (Tool::LivetvListRecordings, "livetv_list_recordings"),
        (Tool::LivetvCancel, "livetv_cancel"),
    ];
    for (tool, name) in expected {
        assert!(
            Tool::ALL.contains(&tool),
            "{name} is missing from Tool::ALL"
        );
        assert_eq!(tool.name(), name);
        assert_eq!(Tool::parse(name), Some(tool));
        // They schedule work, so none of them is open to a viewer.
        assert_eq!(tool.required_role(), Role::Contributor, "{name}");
        assert!(
            tool.schema()["function"]["parameters"]["properties"].is_object(),
            "{name} has no parameters"
        );
    }
}

// =============================================================================
// The ambiguities
// =============================================================================

/// The whole search surface in one server: two channels carrying one series,
/// nine airings of one episode across three clusters, and an unmatched query.
///
/// Standing a `TestServer` up per assertion put comparable suites over the
/// five-minute window after which a parallel run's cleanup drops the template
/// database out from under them.
#[tokio::test]
async fn an_ambiguous_request_surfaces_the_ambiguity_instead_of_scheduling() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "livetv-search@test.com", "contributor").await;
    let auth = auth(user, "contributor");
    let sid = session(state, user).await;

    let tuner = make_tuner(&server.pool, "SEARCH1", 4).await;
    let first_run = make_channel(&server.pool, tuner, 9, "Nine", true).await;
    let back_catalogue = make_channel(&server.pool, tuner, 83, "9Life", false).await;

    // One episode, nine airings, three clusters: a simulcast pair at +60, a
    // second pair at +63 (inside the 5 min tolerance, so the same cluster), and
    // two genuine later repeats.
    let series = "C9481421EN93EP";
    let mut ids = Vec::new();
    for (channel, offset) in [
        (first_run, 60),
        (back_catalogue, 62),
        (first_run, 63),
        (back_catalogue, 64),
        (first_run, 61),
        (back_catalogue, 63),
        (first_run, 62),
        (back_catalogue, 300),
        (first_run, 600),
    ] {
        ids.push(
            make_guide(
                &server.pool,
                Airing {
                    channel_id: channel,
                    title: "The Block",
                    series_id: series,
                    episode: "S09E57",
                    start_mins: offset,
                },
            )
            .await,
        );
    }

    let found = call(
        state,
        &auth,
        sid,
        "livetv_search_guide",
        json!({"title": "block"}),
    )
    .await;
    assert!(found.ok, "search failed: {}", found.observation);
    let m = first_match(&found);

    // Nine airings, and the tool proposes exactly one of them.
    assert_eq!(m["total_airings"], json!(9), "{}", found.observation);
    let episodes = m["episodes"].as_array().expect("episodes");
    assert_eq!(
        episodes.len(),
        1,
        "one episode key must be one proposal: {}",
        found.observation
    );
    assert!(
        episodes[0]["record"]["guide_id"].is_string(),
        "no airing proposed: {}",
        found.observation
    );
    assert_eq!(
        episodes[0]["duplicate_airings_suppressed"],
        json!(8),
        "the suppression count must be stated: {}",
        found.observation
    );
    assert!(
        !episodes[0]["why_suppressed"]
            .as_array()
            .expect("why_suppressed")
            .is_empty(),
        "suppression must carry a reason: {}",
        found.observation
    );

    // The series airs on two channels, so which one is a question.
    let asks = ask_about(&found);
    assert!(
        asks.contains(&"which_channel".to_string()),
        "two channels did not raise which_channel: {asks:?}"
    );
    assert!(
        asks.contains(&"one_off_or_series".to_string()),
        "several airings did not raise one_off_or_series: {asks:?}"
    );
    let channels = m["on_channels"].as_array().expect("on_channels");
    assert_eq!(channels.len(), 2, "{}", found.observation);

    // ...and a rule that does not say which channel creates nothing.
    let asked = call(
        state,
        &auth,
        sid,
        "livetv_create_series_rule",
        json!({"guide_id": ids[0]}),
    )
    .await;
    assert_eq!(asked.observation["created"], json!(false));
    assert_eq!(asked.observation["ask"], json!("which_channel"));
    assert_eq!(
        asked.observation["channels"]
            .as_array()
            .expect("channels")
            .len(),
        2
    );
    assert_eq!(
        rule_count(&server.pool).await,
        0,
        "a rule was created anyway"
    );
    assert_eq!(recording_count(&server.pool).await, 0);

    // An empty result says the listings are missing, not that nothing is on.
    let nothing = call(
        state,
        &auth,
        sid,
        "livetv_search_guide",
        json!({"title": "a programme that is not in this guide"}),
    )
    .await;
    assert_eq!(nothing.observation["found"], json!(false));
    assert!(
        nothing.observation["note"]
            .as_str()
            .expect("note")
            .contains("48 hours"),
        "{}",
        nothing.observation
    );
}

/// Nothing is scheduled without a confirmation, and confirming schedules exactly
/// one row however many airings of that episode the guide carries.
#[tokio::test]
async fn scheduling_records_one_airing_of_nine() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "livetv-record@test.com", "contributor").await;
    let auth = auth(user, "contributor");
    let sid = session(state, user).await;

    let tuner = make_tuner(&server.pool, "RECORD1", 4).await;
    let hd = make_channel(&server.pool, tuner, 90, "Ten HD", true).await;
    let sd = make_channel(&server.pool, tuner, 10, "Ten", false).await;

    let series = "TRAITORS01";
    let mut ids = Vec::new();
    for (channel, offset) in [(hd, 90), (sd, 92), (hd, 400), (sd, 402)] {
        ids.push(
            make_guide(
                &server.pool,
                Airing {
                    channel_id: channel,
                    title: "The Traitors",
                    series_id: series,
                    episode: "S02E05",
                    start_mins: offset,
                },
            )
            .await,
        );
    }

    let done = call(
        state,
        &auth,
        sid,
        "livetv_schedule_recording",
        json!({"guide_id": ids[0]}),
    )
    .await;
    assert_eq!(
        done.observation["scheduled"],
        json!(true),
        "{}",
        done.observation
    );
    assert_eq!(
        recording_count(&server.pool).await,
        1,
        "one airing must mean one row"
    );
    assert_eq!(
        done.observation["duplicate_airings_suppressed"],
        json!(3),
        "the reply must say how many airings it dropped: {}",
        done.observation
    );
    assert_eq!(done.observation["channel"], json!("Ten HD"));
    assert!(
        done.observation["say"]
            .as_str()
            .expect("say")
            .contains("Ten HD")
    );

    // Asking again is the normal shape of a conversation and must not double-book.
    let again = confirmed(
        state,
        &auth,
        sid,
        "livetv_schedule_recording",
        json!({"guide_id": ids[0]}),
    )
    .await;
    assert_eq!(again.observation["already_scheduled"], json!(true));
    assert_eq!(recording_count(&server.pool).await, 1);

    // Nor may the simulcast twin of an episode already claimed be booked again.
    let twin = confirmed(
        state,
        &auth,
        sid,
        "livetv_schedule_recording",
        json!({"guide_id": ids[1]}),
    )
    .await;
    assert_eq!(
        twin.observation["scheduled"],
        json!(false),
        "{}",
        twin.observation
    );
    assert_eq!(twin.observation["already_have_this_episode"], json!(true));
    assert_eq!(recording_count(&server.pool).await, 1);
}

/// Decision 6.6 said out loud: a full tuner bank is an alternative to offer, not
/// a silent failure.
#[tokio::test]
async fn a_clash_offers_the_later_repeat_and_schedules_nothing() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "livetv-clash@test.com", "contributor").await;
    let auth = auth(user, "contributor");
    let sid = session(state, user).await;

    // One tuner, so one channel at a time.
    let tuner = make_tuner(&server.pool, "CLASH1", 1).await;
    let busy = make_channel(&server.pool, tuner, 7, "Seven", true).await;
    let wanted = make_channel(&server.pool, tuner, 9, "Nine", true).await;

    let occupier = make_guide(
        &server.pool,
        Airing {
            channel_id: busy,
            title: "Something Else",
            series_id: "OTHER01",
            episode: "S01E01",
            start_mins: 100,
        },
    )
    .await;
    let first = make_guide(
        &server.pool,
        Airing {
            channel_id: wanted,
            title: "The Block",
            series_id: "BLOCK01",
            episode: "S09E58",
            start_mins: 100,
        },
    )
    .await;
    let repeat = make_guide(
        &server.pool,
        Airing {
            channel_id: wanted,
            title: "The Block",
            series_id: "BLOCK01",
            episode: "S09E58",
            start_mins: 700,
        },
    )
    .await;

    let taken = confirmed(
        state,
        &auth,
        sid,
        "livetv_schedule_recording",
        json!({"guide_id": occupier}),
    )
    .await;
    assert_eq!(
        taken.observation["scheduled"],
        json!(true),
        "{}",
        taken.observation
    );
    assert_eq!(recording_count(&server.pool).await, 1);

    // The single tuner is now committed for that hour, so the clash is real.
    let clash = call(
        state,
        &auth,
        sid,
        "livetv_schedule_recording",
        json!({"guide_id": first}),
    )
    .await;
    assert_eq!(
        clash.observation["scheduled"],
        json!(false),
        "{}",
        clash.observation
    );
    assert_eq!(clash.observation["tuner_clash"], json!(true));
    assert_eq!(
        clash.observation["alternative"]["guide_id"],
        json!(repeat.to_string()),
        "the later repeat was not offered: {}",
        clash.observation
    );
    assert!(
        !clash.observation["pending_confirmation"]
            .as_bool()
            .unwrap_or(false),
        "a clash must be answered, not confirmed"
    );
    assert_eq!(
        recording_count(&server.pool).await,
        1,
        "a clash scheduled something anyway"
    );

    // The search result flags the same clash, so the model can raise it first.
    let searched = call(
        state,
        &auth,
        sid,
        "livetv_search_guide",
        json!({"title": "block"}),
    )
    .await;
    assert!(
        ask_about(&searched).contains(&"tuner_clash".to_string()),
        "the search did not flag the clash: {}",
        searched.observation
    );

    // Taking the offered repeat works, and the original slot stays untouched.
    let moved = confirmed(
        state,
        &auth,
        sid,
        "livetv_schedule_recording",
        json!({"guide_id": repeat}),
    )
    .await;
    assert_eq!(
        moved.observation["scheduled"],
        json!(true),
        "{}",
        moved.observation
    );
    assert_eq!(recording_count(&server.pool).await, 2);
}

/// A one-off and a standing rule are different objects, and the rule defaults to
/// new episodes only. Cancelling is addressed by an id from the list, never by a
/// title the model typed.
#[tokio::test]
async fn a_series_rule_is_new_only_by_default_and_cancelling_needs_an_id() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "livetv-rule@test.com", "contributor").await;
    let auth = auth(user, "contributor");
    let sid = session(state, user).await;

    let tuner = make_tuner(&server.pool, "RULE1", 4).await;
    let channel = make_channel(&server.pool, tuner, 2, "ABC", true).await;
    let series = "HARDQUIZ01";
    let first = make_guide(
        &server.pool,
        Airing {
            channel_id: channel,
            title: "Hard Quiz",
            series_id: series,
            episode: "S08E01",
            start_mins: 120,
        },
    )
    .await;
    make_guide(
        &server.pool,
        Airing {
            channel_id: channel,
            title: "Hard Quiz",
            series_id: series,
            episode: "S08E02",
            start_mins: 1_560,
        },
    )
    .await;

    // One channel, so there is nothing to ask about and scope may default.
    let rule = confirmed(
        state,
        &auth,
        sid,
        "livetv_create_series_rule",
        json!({"guide_id": first}),
    )
    .await;
    assert_eq!(
        rule.observation["created"],
        json!(true),
        "{}",
        rule.observation
    );
    assert_eq!(
        rule.observation["new_only"],
        json!(true),
        "a series rule must default to new episodes only"
    );
    assert_eq!(rule_count(&server.pool).await, 1);
    let rule_id = rule.observation["rule_id"].as_str().expect("rule_id");
    assert!(
        rule.observation["queued_now"].as_i64().unwrap_or(0) > 0,
        "the rule queued nothing: {}",
        rule.observation
    );

    let listed = call(
        state,
        &auth,
        sid,
        "livetv_list_recordings",
        json!({"status": "scheduled"}),
    )
    .await;
    let rules = listed.observation["series_rules"]
        .as_array()
        .expect("series_rules");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["rule_id"], json!(rule_id));
    let recordings = listed.observation["recordings"]
        .as_array()
        .expect("recordings");
    assert!(!recordings.is_empty(), "{}", listed.observation);
    let recording_id = recordings[0]["recording_id"]
        .as_str()
        .expect("recording_id");

    // A cancel with no id is refused rather than guessed at.
    let blind = call(state, &auth, sid, "livetv_cancel", json!({})).await;
    assert!(!blind.ok, "{}", blind.observation);
    assert_eq!(rule_count(&server.pool).await, 1);

    let dropped = call(
        state,
        &auth,
        sid,
        "livetv_cancel",
        json!({"recording_id": recording_id}),
    )
    .await;
    assert_eq!(
        dropped.observation["cancelled"],
        json!(true),
        "{}",
        dropped.observation
    );

    // Turning a rule off stops new episodes; it does not silently bin the ones
    // already queued, and it says so.
    let off = call(
        state,
        &auth,
        sid,
        "livetv_cancel",
        json!({"rule_id": rule_id}),
    )
    .await;
    assert_eq!(
        off.observation["disabled"],
        json!(true),
        "{}",
        off.observation
    );
    assert!(off.observation["still_queued"].is_number());
    let remaining = sqlx::query_scalar::<_, bool>("SELECT enabled FROM livetv_series_rules")
        .fetch_one(&server.pool)
        .await
        .expect("rule");
    assert!(!remaining, "the rule is still enabled");
}

/// The role floor is checked in `dispatch`, before the tool runs, so a viewer
/// never gets as far as an offer.
#[tokio::test]
async fn a_viewer_cannot_read_the_guide_or_book_a_tuner() {
    let server = TestServer::start_api_only().await;
    let state = &server.app_state;
    let user = create_user_with_role(&server, "livetv-viewer@test.com", "viewer").await;
    let viewer = auth(user, "viewer");
    let sid = session(state, user).await;

    for (tool, args) in [
        ("livetv_search_guide", json!({"title": "anything"})),
        (
            "livetv_schedule_recording",
            json!({"guide_id": Uuid::now_v7()}),
        ),
        (
            "livetv_create_series_rule",
            json!({"guide_id": Uuid::now_v7()}),
        ),
        ("livetv_list_recordings", json!({})),
        ("livetv_cancel", json!({"recording_id": Uuid::now_v7()})),
    ] {
        let refused = call(state, &viewer, sid, tool, args).await;
        assert!(
            !refused.ok,
            "{tool} ran for a viewer: {}",
            refused.observation
        );
        assert!(
            refused.detail.contains("permission"),
            "{tool}: {}",
            refused.detail
        );
    }
    assert_eq!(recording_count(&server.pool).await, 0);
}
