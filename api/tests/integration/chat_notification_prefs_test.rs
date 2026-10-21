//! Preferences are the other half of "not a firehose": the defaults must be
//! quiet, and the quiet-hours window must survive midnight, which is exactly
//! where a naive `BETWEEN` gets it backwards.

use crate::common::*;
use api::notifications::NotificationTrigger;
use api::notifications::prefs::Tz;
use api::notifications::prefs::{
    MuteDuration, PrefAction, PrefTrigger, SetNotificationPrefs, Zone, apply_at, in_quiet_hours,
    parse_wall_clock, resolve_mute_until, should_notify,
};
use api::repositories::chat_notification_prefs::{self, ChatNotificationPrefs};
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Date, Duration, Month, OffsetDateTime, Time, UtcOffset};
use uuid::Uuid;

fn t(h: u8, m: u8) -> Time {
    Time::from_hms(h, m, 0).expect("valid time")
}

fn at(y: i32, mo: Month, d: u8, h: u8, mi: u8) -> OffsetDateTime {
    Date::from_calendar_date(y, mo, d)
        .expect("valid date")
        .with_time(t(h, mi))
        .assume_offset(UtcOffset::UTC)
}

async fn user(pool: &PgPool, email: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO users (id, email, password_hash, role) VALUES ($1, $2, 'x', 'viewer')",
        id,
        email,
    )
    .execute(pool)
    .await
    .expect("insert user");
    id
}

// ============================================================================
// Quiet hours — the midnight wrap
// ============================================================================

#[test]
fn quiet_hours_wrap_midnight() {
    // (from, to, now, expected_quiet)
    let cases: &[(Time, Time, Time, bool, &str)] = &[
        // The normal case: 22:00 -> 07:00, which BETWEEN gets exactly backwards.
        (t(22, 0), t(7, 0), t(21, 59), false, "just before it starts"),
        (t(22, 0), t(7, 0), t(22, 0), true, "inclusive at the start"),
        (t(22, 0), t(7, 0), t(23, 59), true, "before midnight"),
        (t(22, 0), t(7, 0), t(0, 0), true, "midnight itself"),
        (
            t(22, 0),
            t(7, 0),
            t(2, 0),
            true,
            "the 2am the feature exists for",
        ),
        (t(22, 0), t(7, 0), t(6, 59), true, "just before it ends"),
        (t(22, 0), t(7, 0), t(7, 0), false, "exclusive at the end"),
        (t(22, 0), t(7, 0), t(12, 0), false, "the middle of the day"),
        // A same-day window still behaves.
        (
            t(9, 0),
            t(17, 0),
            t(8, 59),
            false,
            "before a daytime window",
        ),
        (t(9, 0), t(17, 0), t(9, 0), true, "inclusive at the start"),
        (
            t(9, 0),
            t(17, 0),
            t(16, 59),
            true,
            "inside a daytime window",
        ),
        (t(9, 0), t(17, 0), t(17, 0), false, "exclusive at the end"),
        (t(9, 0), t(17, 0), t(23, 0), false, "after a daytime window"),
        // Zero width is how "no quiet hours" is stored.
        (
            t(0, 0),
            t(0, 0),
            t(2, 0),
            false,
            "zero-width window is never quiet",
        ),
        (
            t(13, 0),
            t(13, 0),
            t(13, 0),
            false,
            "zero-width at the boundary",
        ),
    ];

    for (from, to, now, expected, why) in cases {
        assert_eq!(
            in_quiet_hours(*from, *to, *now),
            *expected,
            "{why}: {from:?}-{to:?} at {now:?}"
        );
    }
}

/// Quiet hours are opt-in. They used to default to 22:00-07:00, which is judged in
/// the server's zone for anyone who has not set their own — so a household at UTC+10
/// was silenced 08:00-17:00 local and told hours late, which is far harder to spot
/// than never being told at all.
/// Prefs with quiet hours explicitly chosen. The subject of these tests is the zone
/// and the DST rules, not the default, and quiet hours are opt-in now — so the window
/// has to be asked for rather than inherited.
fn nightly(user_id: Uuid) -> ChatNotificationPrefs {
    let mut p = ChatNotificationPrefs::defaults(user_id);
    p.quiet_from = Some(t(22, 0));
    p.quiet_to = Some(t(7, 0));
    p
}

#[test]
fn a_row_with_no_quiet_columns_has_no_quiet_hours() {
    let prefs = ChatNotificationPrefs::defaults(Uuid::now_v7());
    assert_eq!(prefs.quiet_window(), None);
}

/// Half a window is not a window: acting on one end and inventing the other is how
/// the old default got applied to people who never asked for it.
#[test]
fn one_end_of_a_window_on_its_own_is_not_quiet_hours() {
    let mut prefs = ChatNotificationPrefs::defaults(Uuid::now_v7());
    prefs.quiet_from = Some(t(22, 0));
    assert_eq!(prefs.quiet_window(), None);

    prefs.quiet_from = None;
    prefs.quiet_to = Some(t(7, 0));
    assert_eq!(prefs.quiet_window(), None);
}

#[test]
fn an_explicitly_set_window_is_honoured() {
    let mut prefs = ChatNotificationPrefs::defaults(Uuid::now_v7());
    prefs.quiet_from = Some(t(22, 0));
    prefs.quiet_to = Some(t(7, 0));
    let (from, to) = prefs.quiet_window().expect("explicit window");
    assert_eq!((from, to), (t(22, 0), t(7, 0)));
    assert!(in_quiet_hours(from, to, t(2, 0)));
    assert!(!in_quiet_hours(from, to, t(14, 0)));
}

#[test]
fn a_zero_width_window_means_no_quiet_hours() {
    let mut prefs = ChatNotificationPrefs::defaults(Uuid::now_v7());
    prefs.quiet_from = Some(Time::MIDNIGHT);
    prefs.quiet_to = Some(Time::MIDNIGHT);
    assert_eq!(prefs.quiet_window(), None);
}

// ============================================================================
// should_notify
// ============================================================================

#[test]
fn should_notify_table() {
    let now = at(2026, Month::August, 24, 12, 0);
    let user_id = Uuid::now_v7();

    // Takes quiet hours out of the way; they have their own table above.
    fn open(user_id: Uuid, f: impl FnOnce(&mut ChatNotificationPrefs)) -> ChatNotificationPrefs {
        let mut p = ChatNotificationPrefs::defaults(user_id);
        p.quiet_from = Some(Time::MIDNIGHT);
        p.quiet_to = Some(Time::MIDNIGHT);
        f(&mut p);
        p
    }

    let cases: Vec<(&str, ChatNotificationPrefs, NotificationTrigger, Time, bool)> = vec![
        (
            "default prefs pass on_import",
            open(user_id, |_| {}),
            NotificationTrigger::OnImport,
            t(12, 0),
            true,
        ),
        (
            "default prefs are silent on on_grab",
            open(user_id, |_| {}),
            NotificationTrigger::OnGrab,
            t(12, 0),
            false,
        ),
        (
            "default prefs are silent on on_failed",
            open(user_id, |_| {}),
            NotificationTrigger::OnFailed,
            t(12, 0),
            false,
        ),
        (
            "an opted-in trigger passes",
            open(user_id, |p| p.triggers = vec![NotificationTrigger::OnGrab]),
            NotificationTrigger::OnGrab,
            t(12, 0),
            true,
        ),
        (
            "disabled suppresses everything",
            open(user_id, |p| {
                p.enabled = false;
                p.triggers = NotificationTrigger::all().to_vec();
            }),
            NotificationTrigger::OnImport,
            t(12, 0),
            false,
        ),
        (
            "an empty trigger list suppresses everything",
            open(user_id, |p| p.triggers = vec![]),
            NotificationTrigger::OnImport,
            t(12, 0),
            false,
        ),
        (
            "muted_until in the future suppresses",
            open(user_id, |p| {
                p.muted_until = Some(at(2026, Month::August, 24, 14, 0))
            }),
            NotificationTrigger::OnImport,
            t(12, 0),
            false,
        ),
        (
            "muted_until in the past does not",
            open(user_id, |p| {
                p.muted_until = Some(at(2026, Month::August, 24, 10, 0))
            }),
            NotificationTrigger::OnImport,
            t(12, 0),
            true,
        ),
        (
            "muted_until exactly now does not",
            open(user_id, |p| p.muted_until = Some(now)),
            NotificationTrigger::OnImport,
            t(12, 0),
            true,
        ),
        (
            "2am with no quiet hours set still delivers",
            ChatNotificationPrefs::defaults(user_id),
            NotificationTrigger::OnImport,
            t(2, 0),
            true,
        ),
        (
            "2am inside an explicitly set quiet window suppresses",
            open(user_id, |p| {
                p.quiet_from = Some(t(22, 0));
                p.quiet_to = Some(t(7, 0));
            }),
            NotificationTrigger::OnImport,
            t(2, 0),
            false,
        ),
        (
            "2pm with no quiet hours set delivers",
            ChatNotificationPrefs::defaults(user_id),
            NotificationTrigger::OnImport,
            t(14, 0),
            true,
        ),
        (
            "07:00 is the moment a chosen window reopens",
            nightly(user_id),
            NotificationTrigger::OnImport,
            t(7, 0),
            true,
        ),
        (
            "22:00 is the moment a chosen window closes",
            nightly(user_id),
            NotificationTrigger::OnImport,
            t(22, 0),
            false,
        ),
    ];

    for (why, prefs, trigger, local_now, expected) in cases {
        assert_eq!(
            should_notify(&prefs, trigger, now, local_now),
            expected,
            "{why}"
        );
    }
}

// ============================================================================
// Relative time, resolved here rather than by the model
// ============================================================================

#[test]
fn relative_durations_resolve_to_timestamps() {
    let now = at(2026, Month::August, 24, 15, 30);
    let utc = &Zone::Iana(Tz::UTC);

    assert_eq!(
        resolve_mute_until(MuteDuration::AnHour, now, utc),
        Some(at(2026, Month::August, 24, 16, 30))
    );
    assert_eq!(
        resolve_mute_until(MuteDuration::ACoupleOfHours, now, utc),
        Some(at(2026, Month::August, 24, 17, 30))
    );
    assert_eq!(
        resolve_mute_until(MuteDuration::TheRestOfToday, now, utc),
        Some(at(2026, Month::August, 25, 0, 0))
    );
    // "until tomorrow" must not resume at one minute past midnight tonight.
    assert_eq!(
        resolve_mute_until(MuteDuration::UntilTomorrow, now, utc),
        Some(at(2026, Month::August, 25, 8, 0))
    );
    assert_eq!(
        resolve_mute_until(MuteDuration::AWeek, now, utc),
        Some(at(2026, Month::August, 31, 15, 30))
    );
    assert_eq!(
        resolve_mute_until(MuteDuration::Indefinitely, now, utc),
        None
    );
}

#[test]
fn relative_durations_use_the_local_day_not_the_utc_day() {
    // 23:30 UTC is already tomorrow in +02:00, so "the rest of today" must end
    // at the local midnight that is only 30 minutes away. Kaliningrad is +02:00
    // all year, so this case is about the local day and not about DST.
    let now = at(2026, Month::August, 24, 23, 30);
    let plus_two = &Zone::Iana(Tz::Europe__Kaliningrad);

    assert_eq!(
        resolve_mute_until(MuteDuration::TheRestOfToday, now, plus_two),
        Some(at(2026, Month::August, 25, 22, 0)),
        "local 2026-08-26 00:00 +02:00 is 2026-08-25 22:00 UTC"
    );
    assert_eq!(
        resolve_mute_until(MuteDuration::UntilTomorrow, now, plus_two),
        Some(at(2026, Month::August, 26, 6, 0)),
        "local 2026-08-26 08:00 +02:00 is 2026-08-26 06:00 UTC"
    );
}

// ============================================================================
// Whose clock — the zone a wall-clock preference is read against
// ============================================================================

/// The failure this whole mechanism exists to prevent: two people, one instant,
/// and only one of them is asleep.
#[test]
fn the_same_instant_is_quiet_for_one_user_and_not_another() {
    // 21:00 UTC on an August evening, against a chosen 22:00-07:00 window.
    let now = at(2026, Month::August, 24, 21, 0);
    let cases: &[(Tz, u8, bool, &str)] = &[
        (Tz::Europe__London, 22, false, "22:00 BST, just gone to bed"),
        (Tz::America__New_York, 17, true, "17:00 EDT, still teatime"),
        (
            Tz::Australia__Sydney,
            7,
            true,
            "07:00 AEST tomorrow, just up",
        ),
    ];

    for (tz, expect_hour, expect_notify, why) in cases {
        let zone = Zone::Iana(*tz);
        assert_eq!(zone.local_time(now).hour(), *expect_hour, "{why}");
        assert_eq!(
            should_notify(
                &nightly(Uuid::now_v7()),
                NotificationTrigger::OnImport,
                now,
                zone.local_time(now),
            ),
            *expect_notify,
            "{why}"
        );
    }
}

/// The test that justifies storing a zone name rather than an offset.
///
/// `22:00` has to mean 22:00 in summer and in winter. Europe/London is UTC+1 in
/// August and UTC+0 in December, so the *same UTC wall clock* falls on opposite
/// sides of the window either side of the change — which no stored offset can
/// express, because whichever one was captured is wrong for half the year.
#[test]
fn the_quiet_window_holds_either_side_of_a_dst_change() {
    let london = Zone::Iana(Tz::Europe__London);
    let prefs = nightly(Uuid::now_v7());

    // (instant, local hour, local minute, notify?, why)
    let cases: &[(OffsetDateTime, u8, u8, bool, &str)] = &[
        (
            at(2026, Month::August, 25, 6, 30),
            7,
            30,
            true,
            "06:30Z in BST is 07:30 local — the window has already reopened",
        ),
        (
            at(2026, Month::December, 25, 6, 30),
            6,
            30,
            false,
            "the identical 06:30Z in GMT is 06:30 local — still the middle of the night",
        ),
        (
            at(2026, Month::August, 24, 21, 30),
            22,
            30,
            false,
            "21:30Z in BST is 22:30 local — quiet hours have started",
        ),
        (
            at(2026, Month::December, 24, 21, 30),
            21,
            30,
            true,
            "the identical 21:30Z in GMT is 21:30 local — half an hour of evening left",
        ),
    ];

    for (instant, hour, minute, notify, why) in cases {
        let local = london.local_time(*instant);
        assert_eq!((local.hour(), local.minute()), (*hour, *minute), "{why}");
        assert_eq!(
            should_notify(&prefs, NotificationTrigger::OnImport, *instant, local),
            *notify,
            "{why}"
        );
    }

    // And the offsets really are different, which is the whole reason the pairs
    // above disagree.
    assert_eq!(
        london
            .offset_at(at(2026, Month::August, 25, 6, 30))
            .whole_hours(),
        1
    );
    assert_eq!(
        london
            .offset_at(at(2026, Month::December, 25, 6, 30))
            .whole_hours(),
        0
    );
}

/// A mute that spans the night the clocks change has to land on the wall clock,
/// not on a fixed number of hours from the offset that happened to be in force.
#[test]
fn relative_durations_cross_a_dst_transition_on_the_wall_clock() {
    let london = Zone::Iana(Tz::Europe__London);
    // 13:00 BST on the Saturday. "Tomorrow" is the Sunday the clocks go back at
    // 02:00, so 08:00 tomorrow is GMT: 08:00Z. Freezing today's +01:00 would
    // resume at 07:00Z, an hour early.
    let now = at(2026, Month::October, 24, 12, 0);
    assert_eq!(
        resolve_mute_until(MuteDuration::UntilTomorrow, now, &london),
        Some(at(2026, Month::October, 25, 8, 0)),
        "08:00 on the morning after the clocks go back is 08:00Z, not 07:00Z"
    );
    // Local midnight is still before the transition, so it is BST.
    assert_eq!(
        resolve_mute_until(MuteDuration::TheRestOfToday, now, &london),
        Some(at(2026, Month::October, 24, 23, 0)),
        "local midnight is still +01:00, half a night before the change"
    );

    // Havana moves its clocks at midnight, so the local midnight that
    // "the rest of today" aims at does not exist that night. Resuming at the
    // instant the clock jumps to is the only answer that is on a clock at all.
    let havana = Zone::Iana(Tz::America__Havana);
    let gap_night = at(2026, Month::March, 7, 18, 0);
    assert_eq!(
        resolve_mute_until(MuteDuration::TheRestOfToday, gap_night, &havana),
        Some(at(2026, Month::March, 8, 5, 0)),
        "00:00 CST and 01:00 CDT are the same instant; a midnight that never happens resolves to it"
    );
}

/// Nothing regresses for a user who has never said where they are.
#[test]
fn an_unset_zone_falls_back_to_the_server() {
    let cases: &[(Option<&str>, &str)] = &[
        (None, "no timezone column"),
        (Some(""), "an empty string"),
        (Some("   "), "whitespace"),
        (
            Some("Mars/Olympus_Mons"),
            "a name the tz database has never had",
        ),
    ];
    for (stored, why) in cases {
        assert_eq!(Zone::for_user(*stored), Zone::ServerLocal, "{why}");
    }

    let now = OffsetDateTime::now_utc();
    assert_eq!(
        Zone::ServerLocal.local_time(now),
        now.to_offset(api::notifications::prefs::server_local_offset())
            .time(),
        "the fallback is exactly the server clock the old code used"
    );
    assert_eq!(
        Zone::for_user(Some("Europe/London")),
        Zone::Iana(Tz::Europe__London)
    );
}

/// A misconfigured zone has to be visible in what the bot says, because the
/// alternative is a message at 2 am with nothing to point at.
#[test]
fn the_resolved_zone_is_said_out_loud() {
    let summer = at(2026, Month::August, 24, 12, 0);
    let winter = at(2026, Month::December, 24, 12, 0);

    let london = Zone::Iana(Tz::Europe__London);
    assert_eq!(london.label_at(summer), "Europe/London, UTC+01:00");
    assert_eq!(
        london.label_at(winter),
        "Europe/London, UTC",
        "the same zone, half a year later, is on a different clock"
    );
    assert_eq!(
        Zone::Iana(Tz::Asia__Kolkata).label_at(summer),
        "Asia/Kolkata, UTC+05:30",
        "a half-hour offset is not rounded away"
    );
    assert!(
        Zone::ServerLocal
            .label_at(summer)
            .starts_with("server time,"),
        "the fallback names itself as the fallback"
    );

    let prefs = nightly(Uuid::now_v7());
    assert!(
        api::notifications::prefs::describe(&prefs, &london, summer).contains("Europe/London"),
        "status reads the zone back"
    );
}

#[test]
fn an_unparseable_zone_is_refused() {
    for bad in ["", "London", "Europe/Nowhere", "UTC+1", "GMT+0100", "  "] {
        assert!(
            Zone::parse(bad).is_err(),
            "{bad:?} should not parse as an IANA zone"
        );
    }
    for good in ["Europe/London", "Asia/Tokyo", "America/New_York", "UTC"] {
        assert!(Zone::parse(good).is_ok(), "{good:?} should parse");
    }
}

/// The write path, not just the type: a typo must be a 400 rather than a row
/// that silently reads as the server's zone forever.
#[tokio::test]
async fn the_timezone_endpoint_refuses_a_name_that_is_not_a_zone() {
    let server = TestServer::start_api_only().await;
    let client = create_user_client(&server, "tzcheck@test.local", "viewer").await;

    client
        .put_json(
            "/api/v1/admin/me/timezone",
            &serde_json::json!({ "timezone": "London" }),
        )
        .await
        .assert_status(reqwest::StatusCode::BAD_REQUEST);

    client
        .put_json(
            "/api/v1/admin/me/timezone",
            &serde_json::json!({ "timezone": "Europe/London" }),
        )
        .await
        .assert_status(reqwest::StatusCode::NO_CONTENT);

    let me = client.get("/api/v1/admin/me").await;
    me.assert_ok();
    assert_eq!(
        me.json::<serde_json::Value>()["timezone"].as_str(),
        Some("Europe/London")
    );

    // Clearing it puts the user back on the server's clock.
    client
        .put_json(
            "/api/v1/admin/me/timezone",
            &serde_json::json!({ "timezone": "" }),
        )
        .await
        .assert_status(reqwest::StatusCode::NO_CONTENT);
    let me = client.get("/api/v1/admin/me").await;
    assert!(me.json::<serde_json::Value>()["timezone"].is_null());
}

/// End to end: the same preference row, judged against two different stored
/// zones, gives two different answers.
#[tokio::test]
async fn quiet_hours_are_read_against_the_users_stored_zone() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;
    let sleeper = user(pool, "london@test.local").await;
    let awake = user(pool, "newyork@test.local").await;

    for (id, tz) in [(sleeper, "Europe/London"), (awake, "America/New_York")] {
        api::repositories::users::update_timezone(pool, id, Some(tz))
            .await
            .expect("timezone stored");
    }

    let now = at(2026, Month::August, 24, 21, 0);
    let zones = api::repositories::users::get_timezones(pool, &[sleeper, awake])
        .await
        .expect("zones read");

    let verdict = |id: Uuid| {
        let zone = Zone::for_user(zones.get(&id).map(String::as_str));
        should_notify(
            &nightly(id),
            NotificationTrigger::OnImport,
            now,
            zone.local_time(now),
        )
    };

    assert!(!verdict(sleeper), "22:00 in London is inside quiet hours");
    assert!(verdict(awake), "17:00 in New York is not");
}

#[test]
fn wall_clock_parsing_accepts_what_the_model_emits() {
    let cases: &[(&str, Option<Time>)] = &[
        ("22:00", Some(t(22, 0))),
        ("07:00", Some(t(7, 0))),
        ("7:05", Some(t(7, 5))),
        ("22:00:00", Some(t(22, 0))),
        ("22", Some(t(22, 0))),
        (" 22:30 ", Some(t(22, 30))),
        ("10pm", Some(t(22, 0))),
        ("10 PM", Some(t(22, 0))),
        ("7am", Some(t(7, 0))),
        ("12am", Some(Time::MIDNIGHT)),
        ("12pm", Some(t(12, 0))),
        ("tonight", None),
        ("25:00", None),
        ("22:99", None),
        ("", None),
    ];

    for (raw, expected) in cases {
        match expected {
            Some(want) => assert_eq!(parse_wall_clock(raw).ok(), Some(*want), "parsing {raw:?}"),
            None => assert!(parse_wall_clock(raw).is_err(), "{raw:?} should be refused"),
        }
    }
}

// ============================================================================
// Storage
// ============================================================================

#[tokio::test]
async fn a_user_with_no_row_gets_the_defaults() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;
    let id = user(pool, "defaults@test.local").await;

    let prefs = chat_notification_prefs::get(pool, id).await.unwrap();
    assert_eq!(prefs, ChatNotificationPrefs::defaults(id));
    assert!(prefs.enabled);
    assert_eq!(prefs.triggers, vec![NotificationTrigger::OnImport]);
    assert_eq!(prefs.muted_until, None);

    let other = user(pool, "defaults-2@test.local").await;
    let many = chat_notification_prefs::get_many(pool, &[id, other])
        .await
        .unwrap();
    assert_eq!(many.len(), 2);
    assert_eq!(many[&other], ChatNotificationPrefs::defaults(other));
}

#[tokio::test]
async fn upsert_round_trips_every_column() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;
    let id = user(pool, "roundtrip@test.local").await;

    let mut prefs = ChatNotificationPrefs::defaults(id);
    prefs.enabled = false;
    prefs.triggers = vec![NotificationTrigger::OnGrab, NotificationTrigger::OnFailed];
    prefs.muted_until = Some(at(2026, Month::September, 1, 9, 0));
    prefs.quiet_from = Some(t(23, 15));
    prefs.quiet_to = Some(t(6, 45));

    let saved = chat_notification_prefs::upsert(pool, &prefs).await.unwrap();
    assert_eq!(saved, prefs);
    assert_eq!(chat_notification_prefs::get(pool, id).await.unwrap(), prefs);

    prefs.enabled = true;
    let updated = chat_notification_prefs::upsert(pool, &prefs).await.unwrap();
    assert!(updated.enabled);
    assert!(chat_notification_prefs::delete(pool, id).await.unwrap());
    assert_eq!(
        chat_notification_prefs::get(pool, id).await.unwrap(),
        ChatNotificationPrefs::defaults(id)
    );
}

// ============================================================================
// Setting preferences by talking
// ============================================================================

fn req(action: PrefAction) -> SetNotificationPrefs {
    SetNotificationPrefs {
        action: Some(action),
        ..Default::default()
    }
}

#[tokio::test]
async fn talking_changes_preferences() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;
    let id = user(pool, "talker@test.local").await;
    let now = at(2026, Month::August, 24, 15, 30);

    // "shut up for a couple of hours"
    let muted = apply_at(
        pool,
        id,
        &SetNotificationPrefs {
            action: Some(PrefAction::Mute),
            duration: Some(MuteDuration::ACoupleOfHours),
            ..Default::default()
        },
        now,
    )
    .await
    .expect("mute");
    assert_eq!(
        muted.prefs.muted_until,
        resolve_mute_until(MuteDuration::ACoupleOfHours, now, &Zone::ServerLocal)
    );
    assert!(!should_notify(
        &muted.prefs,
        NotificationTrigger::OnImport,
        now,
        t(14, 0)
    ));

    // "start again"
    let resumed = apply_at(pool, id, &req(PrefAction::Resume), now)
        .await
        .expect("resume");
    assert!(resumed.prefs.enabled);
    assert_eq!(resumed.prefs.muted_until, None);
    assert!(!resumed.summary.is_empty());

    // "stop telling me about downloads"
    let stopped = apply_at(
        pool,
        id,
        &SetNotificationPrefs {
            action: Some(PrefAction::DisableTrigger),
            trigger: Some(PrefTrigger::OnImport),
            ..Default::default()
        },
        now,
    )
    .await
    .expect("disable trigger");
    assert!(
        !stopped
            .prefs
            .triggers
            .contains(&NotificationTrigger::OnImport)
    );
    assert!(!should_notify(
        &stopped.prefs,
        NotificationTrigger::OnImport,
        now,
        t(14, 0)
    ));

    // "tell me when things finish again"
    let started = apply_at(
        pool,
        id,
        &SetNotificationPrefs {
            action: Some(PrefAction::EnableTrigger),
            trigger: Some(PrefTrigger::OnImport),
            ..Default::default()
        },
        now,
    )
    .await
    .expect("enable trigger");
    assert!(
        started
            .prefs
            .triggers
            .contains(&NotificationTrigger::OnImport)
    );

    // "nothing before 8am"
    let quiet = apply_at(
        pool,
        id,
        &SetNotificationPrefs {
            action: Some(PrefAction::SetQuietHours),
            quiet_from: Some("23:00".into()),
            quiet_to: Some("8am".into()),
            ..Default::default()
        },
        now,
    )
    .await
    .expect("set quiet hours");
    assert_eq!(quiet.prefs.quiet_window(), Some((t(23, 0), t(8, 0))));
    assert!(!should_notify(
        &quiet.prefs,
        NotificationTrigger::OnImport,
        now,
        t(2, 0)
    ));

    // "message me whenever"
    let loud = apply_at(pool, id, &req(PrefAction::ClearQuietHours), now)
        .await
        .expect("clear quiet hours");
    assert_eq!(loud.prefs.quiet_window(), None);
    assert!(should_notify(
        &loud.prefs,
        NotificationTrigger::OnImport,
        now,
        t(2, 0)
    ));

    // "what are my settings" changes nothing but says something.
    let before = chat_notification_prefs::get(pool, id).await.unwrap();
    let status = apply_at(pool, id, &req(PrefAction::Status), now)
        .await
        .expect("status");
    assert_eq!(status.prefs, before);
    assert!(!status.summary.is_empty());
}

#[tokio::test]
async fn muting_indefinitely_is_reversible() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;
    let id = user(pool, "indefinite@test.local").await;
    let now = at(2026, Month::August, 24, 15, 30);

    let muted = apply_at(
        pool,
        id,
        &SetNotificationPrefs {
            action: Some(PrefAction::Mute),
            duration: Some(MuteDuration::Indefinitely),
            ..Default::default()
        },
        now,
    )
    .await
    .expect("mute indefinitely");
    assert!(!muted.prefs.enabled);
    assert_eq!(muted.prefs.muted_until, None);
    assert!(!should_notify(
        &muted.prefs,
        NotificationTrigger::OnImport,
        now,
        t(14, 0)
    ));

    let resumed = apply_at(pool, id, &req(PrefAction::Resume), now)
        .await
        .expect("resume");
    assert!(resumed.prefs.enabled);
    assert!(!resumed.prefs.triggers.is_empty());
}

#[tokio::test]
async fn bad_quiet_hours_are_a_question_not_a_guess() {
    let server = TestServer::start_api_only().await;
    let pool = &server.pool;
    let id = user(pool, "badquiet@test.local").await;
    let now = at(2026, Month::August, 24, 15, 30);

    for (from, to) in [
        (Some("tonight"), Some("07:00")),
        (Some("22:00"), None),
        (None, Some("07:00")),
        (Some("22:00"), Some("22:00")),
    ] {
        let result = apply_at(
            pool,
            id,
            &SetNotificationPrefs {
                action: Some(PrefAction::SetQuietHours),
                quiet_from: from.map(str::to_string),
                quiet_to: to.map(str::to_string),
                ..Default::default()
            },
            now,
        )
        .await;
        assert!(result.is_err(), "{from:?}..{to:?} should be refused");
    }

    // Nothing was written by any of the refusals.
    assert_eq!(
        chat_notification_prefs::get(pool, id).await.unwrap(),
        ChatNotificationPrefs::defaults(id)
    );
}

// ============================================================================
// The settings-page endpoints — same logic, second front end
// ============================================================================

const PREFS_PATH: &str = "/api/v1/admin/me/notification-prefs";

async fn read_prefs(client: &TestClient) -> Value {
    let resp = client.get(PREFS_PATH).await;
    resp.assert_ok();
    resp.json()
}

async fn change_prefs(client: &TestClient, payload: &Value) -> Value {
    let resp = client.put_json(PREFS_PATH, payload).await;
    resp.assert_ok();
    resp.json()
}

fn triggers_of(view: &Value) -> Vec<String> {
    view["triggers"]
        .as_array()
        .expect("triggers is an array")
        .iter()
        .map(|t| t.as_str().expect("trigger is a string").to_string())
        .collect()
}

#[tokio::test]
async fn the_endpoint_reads_the_defaults_for_a_user_with_no_row() {
    let server = TestServer::start_api_only().await;
    let client = create_user_client(&server, "prefsdefaults@test.local", "viewer").await;

    let view = read_prefs(&client).await;

    assert_eq!(view["enabled"], json!(true));
    assert_eq!(triggers_of(&view), vec!["on_import".to_string()]);
    assert_eq!(view["muted"], json!(false));
    assert_eq!(view["muted_until"], Value::Null);
    // Quiet hours are opt-in, so a user who never chose a window has none. Coming
    // back filled in would make the settings page claim a window nobody picked.
    assert_eq!(view["quiet_from"], Value::Null);
    assert_eq!(view["quiet_to"], Value::Null);
    assert_eq!(view["timezone"], Value::Null);
    assert!(
        view["timezone_label"]
            .as_str()
            .is_some_and(|l| l.contains("server time")),
        "an unset zone must still name the clock it fell back to: {view:#?}"
    );
    assert!(!view["summary"].as_str().unwrap_or_default().is_empty());

    // Reading is not a write: the row still does not exist. Runtime query, not
    // the macro — a new macro here needs a `.sqlx` cache entry to exist first.
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_notification_prefs")
        .fetch_one(&server.pool)
        .await
        .expect("count rows");
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn a_viewer_can_change_their_own_preferences() {
    let server = TestServer::start_api_only().await;
    let client = create_user_client(&server, "prefsviewer@test.local", "viewer").await;

    // The whole point of the endpoint: muting yourself needs no privilege.
    let view = change_prefs(&client, &json!({ "action": "mute", "duration": "an_hour" })).await;
    assert_eq!(view["muted"], json!(true));
    assert!(view["muted_until"].as_str().is_some());
}

#[tokio::test]
async fn every_action_round_trips_through_the_endpoint() {
    let server = TestServer::start_api_only().await;
    let client = create_user_client(&server, "prefsactions@test.local", "viewer").await;

    let muted = change_prefs(
        &client,
        &json!({ "action": "mute", "duration": "a_couple_of_hours" }),
    )
    .await;
    assert_eq!(muted["muted"], json!(true));
    assert_eq!(muted["enabled"], json!(true));

    let off = change_prefs(
        &client,
        &json!({ "action": "mute", "duration": "indefinitely" }),
    )
    .await;
    assert_eq!(off["enabled"], json!(false));
    assert_eq!(off["muted_until"], Value::Null);

    let resumed = change_prefs(&client, &json!({ "action": "resume" })).await;
    assert_eq!(resumed["enabled"], json!(true));
    assert_eq!(resumed["muted"], json!(false));

    let everything = change_prefs(
        &client,
        &json!({ "action": "enable_trigger", "trigger": "everything" }),
    )
    .await;
    assert_eq!(
        triggers_of(&everything),
        vec![
            "on_grab".to_string(),
            "on_import".to_string(),
            "on_failed".to_string(),
            "on_media_corrupt".to_string()
        ]
    );

    let narrower = change_prefs(
        &client,
        &json!({ "action": "disable_trigger", "trigger": "on_grab" }),
    )
    .await;
    assert_eq!(
        triggers_of(&narrower),
        vec![
            "on_import".to_string(),
            "on_failed".to_string(),
            "on_media_corrupt".to_string()
        ]
    );

    let quiet = change_prefs(
        &client,
        &json!({
            "action": "set_quiet_hours",
            "quiet_from": "09:00",
            "quiet_to": "17:00"
        }),
    )
    .await;
    assert_eq!(quiet["quiet_from"], json!("09:00"));
    assert_eq!(quiet["quiet_to"], json!("17:00"));

    // Everything above survives a fresh read, not just the write's echo.
    let reread = read_prefs(&client).await;
    assert_eq!(reread["enabled"], json!(true));
    assert_eq!(
        triggers_of(&reread),
        vec![
            "on_import".to_string(),
            "on_failed".to_string(),
            "on_media_corrupt".to_string()
        ]
    );
    assert_eq!(reread["quiet_from"], json!("09:00"));
    assert_eq!(reread["quiet_to"], json!("17:00"));
}

#[tokio::test]
async fn a_window_that_crosses_midnight_survives_a_round_trip() {
    let server = TestServer::start_api_only().await;
    let client = create_user_client(&server, "prefswrap@test.local", "viewer").await;

    let saved = change_prefs(
        &client,
        &json!({
            "action": "set_quiet_hours",
            "quiet_from": "22:30",
            "quiet_to": "07:15"
        }),
    )
    .await;
    assert_eq!(saved["quiet_from"], json!("22:30"));
    assert_eq!(saved["quiet_to"], json!("07:15"));

    // Not reordered into 07:15-22:30, which would silence the whole day.
    let reread = read_prefs(&client).await;
    assert_eq!(reread["quiet_from"], json!("22:30"));
    assert_eq!(reread["quiet_to"], json!("07:15"));
}

#[tokio::test]
async fn a_zero_width_window_reads_back_as_no_quiet_hours() {
    let server = TestServer::start_api_only().await;
    let client = create_user_client(&server, "prefsclear@test.local", "viewer").await;

    let cleared = change_prefs(&client, &json!({ "action": "clear_quiet_hours" })).await;
    assert_eq!(cleared["quiet_from"], Value::Null);
    assert_eq!(cleared["quiet_to"], Value::Null);
    // Clearing stores a zero-width window rather than NULLing the columns. Now that
    // an unset window is also "off", the two are indistinguishable from out here —
    // the encoding still matters to `quiet_window`, but no longer to a caller.

    let reread = read_prefs(&client).await;
    assert_eq!(reread["quiet_from"], Value::Null);
    assert_eq!(reread["quiet_to"], Value::Null);

    // Turning them back on is reachable from there.
    let back = change_prefs(
        &client,
        &json!({
            "action": "set_quiet_hours",
            "quiet_from": "23:00",
            "quiet_to": "06:00"
        }),
    )
    .await;
    assert_eq!(back["quiet_from"], json!("23:00"));
}

#[tokio::test]
async fn the_endpoint_names_the_zone_the_window_is_judged_in() {
    let server = TestServer::start_api_only().await;
    let client = create_user_client(&server, "prefszone@test.local", "viewer").await;

    client
        .put_json(
            "/api/v1/admin/me/timezone",
            &json!({ "timezone": "Europe/London" }),
        )
        .await
        .assert_success();

    let view = read_prefs(&client).await;
    assert_eq!(view["timezone"], json!("Europe/London"));
    assert!(
        view["timezone_label"]
            .as_str()
            .is_some_and(|l| l.contains("Europe/London") && l.contains("UTC")),
        "the label must name the zone and its offset: {view:#?}"
    );
}

#[tokio::test]
async fn a_time_that_is_not_a_time_is_refused_rather_than_guessed() {
    let server = TestServer::start_api_only().await;
    let client = create_user_client(&server, "prefsbad@test.local", "viewer").await;

    for payload in [
        json!({ "action": "set_quiet_hours", "quiet_from": "tonight", "quiet_to": "07:00" }),
        json!({ "action": "set_quiet_hours", "quiet_from": "22:00" }),
        // A zero-width window is spelled clear_quiet_hours, not set_quiet_hours.
        json!({ "action": "set_quiet_hours", "quiet_from": "09:00", "quiet_to": "09:00" }),
    ] {
        client
            .put_json(PREFS_PATH, &payload)
            .await
            .assert_status(StatusCode::BAD_REQUEST);
    }

    // None of the refusals wrote anything.
    let view = read_prefs(&client).await;
    assert_eq!(view["quiet_from"], Value::Null);
    assert_eq!(view["quiet_to"], Value::Null);
}

#[tokio::test]
async fn the_endpoint_is_refused_without_a_session() {
    let server = TestServer::start_api_only().await;
    let anonymous = TestClient::new(&server.api_url);

    anonymous
        .get(PREFS_PATH)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    anonymous
        .put_json(PREFS_PATH, &json!({ "action": "resume" }))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn one_user_can_neither_read_nor_change_another_users_preferences() {
    let server = TestServer::start_api_only().await;
    let hers = create_user_client(&server, "prefshers@test.local", "viewer").await;
    let his = create_user_client(&server, "prefshis@test.local", "admin").await;

    change_prefs(
        &hers,
        &json!({
            "action": "set_quiet_hours",
            "quiet_from": "21:00",
            "quiet_to": "08:00"
        }),
    )
    .await;
    change_prefs(&hers, &json!({ "action": "mute", "duration": "a_week" })).await;

    // The endpoint has no user parameter at all, so an admin reading it sees
    // his own untouched defaults, not hers.
    let his_view = read_prefs(&his).await;
    assert_eq!(his_view["quiet_from"], Value::Null);
    assert_eq!(his_view["muted"], json!(false));

    change_prefs(&his, &json!({ "action": "clear_quiet_hours" })).await;

    let her_view = read_prefs(&hers).await;
    assert_eq!(her_view["quiet_from"], json!("21:00"));
    assert_eq!(her_view["quiet_to"], json!("08:00"));
    assert_eq!(her_view["muted"], json!(true));
}
