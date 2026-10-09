//! The logic behind the `set_notification_prefs` intent, and the gate the chat
//! fan-out asks before it says anything.
//!
//! Two rules shape this module:
//!
//! * **Relative time is resolved here, not by the model.** "a couple of hours"
//!   and "until tomorrow" arrive as [`MuteDuration`] variants; Gemma is bad at
//!   date arithmetic and this is a resolver problem, not an LLM one.
//! * **Quiet hours are wall-clock times in the owner's timezone.** Nobody says
//!   "mute me from 22:00Z", and a household is not one clock — a member in
//!   another zone woken at 2 am is the exact failure quiet hours exist to
//!   prevent, and nothing visibly breaks to reveal it. Each user's [`Zone`]
//!   comes from `users.timezone`, an IANA name resolved against the tz rules at
//!   evaluation time so 22:00 means 22:00 in summer and in winter. A user with
//!   no zone set falls back to the server's, which is why [`describe`] says the
//!   resolved zone out loud rather than letting a wrong one stay silent.
//!
//! `set_notification_prefs` writes only the caller's own row, so it is the one
//! intent that is safe for a `viewer` — see [`SAFE_FOR_VIEWER`].

use chrono::{Offset, TimeZone};
pub use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime, Time, UtcOffset};
use uuid::Uuid;

use super::NotificationTrigger;
use crate::repositories::{
    chat_notification_prefs::{self, ChatNotificationPrefs},
    users,
};

/// Muting yourself needs no privilege, and this intent cannot touch anyone
/// else's row — the dispatch entry for `set_notification_prefs` must therefore
/// be `Role::Any`, not `Role::Contributor`.
pub const SAFE_FOR_VIEWER: bool = true;

pub const TOOL_NAME: &str = "set_notification_prefs";

/// The hour "until tomorrow" resumes at. Local midnight would resume tonight,
/// which is the opposite of what anyone means by it.
const TOMORROW_RESUME_HOUR: u8 = 8;

// ============================================================================
// Evaluation — the gate the chat fan-out asks
// ============================================================================

pub fn server_local_offset() -> UtcOffset {
    let seconds = chrono::Local::now().offset().local_minus_utc();
    UtcOffset::from_whole_seconds(seconds).unwrap_or(UtcOffset::UTC)
}

/// The clock a user's wall-clock preferences are read against.
///
/// A stored UTC offset cannot express any of this: `22:00` means 22:00 in
/// summer *and* in winter, so the zone has to be resolved against the IANA rules
/// at the instant being judged, not frozen when the preference was written.
///
/// [`Zone::ServerLocal`] is the fallback for a user who has not said where they
/// are. It reads the process's offset (`TZ` in the container) at evaluation time
/// too, so it tracks the server's own DST — what it cannot do is get a *future*
/// wall clock right across a transition, which only an IANA name can.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Iana(Tz),
    ServerLocal,
}

impl Zone {
    /// Rejects an unparseable name rather than silently falling back, so a typo
    /// is caught at the API boundary instead of at 2 am.
    pub fn parse(name: &str) -> Result<Self, PrefsError> {
        name.trim().parse::<Tz>().map(Zone::Iana).map_err(|_| {
            PrefsError::Invalid(format!(
                "'{name}' is not a timezone name like Europe/London or America/New_York"
            ))
        })
    }

    /// A stored name that no longer parses (a tz database rename, a hand-edited
    /// row) falls back rather than failing the notification — the write path is
    /// what refuses bad names.
    pub fn for_user(stored: Option<&str>) -> Self {
        stored
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .and_then(|n| Zone::parse(n).ok())
            .unwrap_or(Zone::ServerLocal)
    }

    pub fn offset_at(&self, instant: OffsetDateTime) -> UtcOffset {
        match self {
            Zone::ServerLocal => server_local_offset(),
            Zone::Iana(tz) => {
                let seconds = to_chrono(instant)
                    .with_timezone(tz)
                    .offset()
                    .fix()
                    .local_minus_utc();
                UtcOffset::from_whole_seconds(seconds).unwrap_or(UtcOffset::UTC)
            }
        }
    }

    pub fn local_time(&self, instant: OffsetDateTime) -> Time {
        instant.to_offset(self.offset_at(instant)).time()
    }

    pub fn local_date(&self, instant: OffsetDateTime) -> time::Date {
        instant.to_offset(self.offset_at(instant)).date()
    }

    /// A wall-clock date and time in this zone, as the UTC instant it happens at.
    ///
    /// Two cases only an IANA zone knows about: the hour that occurs twice when
    /// clocks go back takes the earlier one, and the hour that never happens when
    /// they go forward resumes at the instant the clock jumps to, rather than
    /// resolving to a time that is not on any clock.
    pub fn wall_clock_instant(&self, date: time::Date, wall: Time) -> OffsetDateTime {
        let tz = match self {
            Zone::ServerLocal => {
                return date.with_time(wall).assume_offset(server_local_offset());
            }
            Zone::Iana(tz) => tz,
        };

        let Some(naive) = chrono::NaiveDate::from_ymd_opt(
            date.year(),
            u8::from(date.month()) as u32,
            date.day() as u32,
        )
        .and_then(|d| {
            d.and_hms_opt(
                wall.hour() as u32,
                wall.minute() as u32,
                wall.second() as u32,
            )
        }) else {
            return date.with_time(wall).assume_utc();
        };

        if let Some(resolved) = tz.from_local_datetime(&naive).earliest() {
            return from_chrono(resolved.to_utc());
        }

        let mut probe = naive;
        for _ in 0..4 {
            probe += chrono::TimeDelta::hours(1);
            if let Some(resolved) = tz.from_local_datetime(&probe).earliest() {
                return from_chrono(resolved.to_utc());
            }
        }
        from_chrono(chrono::Utc.from_utc_datetime(&naive))
    }

    /// Named out loud wherever a wall-clock preference is echoed back. A user in
    /// the wrong zone, or a container with no `TZ`, then reads as a wrong label
    /// rather than as messages arriving at the wrong hour for no visible reason.
    pub fn label_at(&self, instant: OffsetDateTime) -> String {
        let offset = self.offset_at(instant);
        let clock = if offset.is_utc() {
            "UTC".to_string()
        } else {
            let (h, m, _) = offset.as_hms();
            format!("UTC{h:+03}:{:02}", m.abs())
        };
        match self {
            Zone::Iana(tz) => format!("{}, {clock}", tz.name()),
            Zone::ServerLocal => format!("server time, {clock}"),
        }
    }
}

fn to_chrono(t: OffsetDateTime) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(t.unix_timestamp(), 0).unwrap_or_default()
}

fn from_chrono(t: chrono::DateTime<chrono::Utc>) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(t.timestamp()).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

/// The zone to judge one user's wall-clock preferences in, falling back to the
/// server's when they have not set one.
pub async fn zone_for_user(pool: &PgPool, user_id: Uuid) -> Zone {
    match users::get_timezone(pool, user_id).await {
        Ok(stored) => Zone::for_user(stored.as_deref()),
        Err(e) => {
            tracing::warn!(error = %e, %user_id, "Failed to read user timezone; using server time");
            Zone::ServerLocal
        }
    }
}

/// Inclusive at `from`, exclusive at `to`, and correct across midnight — a plain
/// `BETWEEN` gets a 22:00-07:00 window exactly backwards and would silence the
/// whole day instead of the night.
pub fn in_quiet_hours(from: Time, to: Time, now: Time) -> bool {
    if from == to {
        false
    } else if from < to {
        now >= from && now < to
    } else {
        now >= from || now < to
    }
}

pub fn should_notify(
    prefs: &ChatNotificationPrefs,
    trigger: NotificationTrigger,
    now: OffsetDateTime,
    local_now: Time,
) -> bool {
    if !prefs.enabled {
        return false;
    }
    if !prefs.triggers.contains(&trigger) {
        return false;
    }
    if let Some(until) = prefs.muted_until
        && until > now
    {
        return false;
    }
    if let Some((from, to)) = prefs.quiet_window()
        && in_quiet_hours(from, to, local_now)
    {
        return false;
    }
    true
}

// ============================================================================
// The argument surface — narrow, enum-shaped, five fields
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrefAction {
    Mute,
    Resume,
    EnableTrigger,
    DisableTrigger,
    SetQuietHours,
    ClearQuietHours,
    Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrefTrigger {
    OnGrab,
    OnImport,
    OnFailed,
    Everything,
}

impl PrefTrigger {
    fn expand(self) -> Vec<NotificationTrigger> {
        match self {
            PrefTrigger::OnGrab => vec![NotificationTrigger::OnGrab],
            PrefTrigger::OnImport => vec![NotificationTrigger::OnImport],
            PrefTrigger::OnFailed => vec![NotificationTrigger::OnFailed],
            PrefTrigger::Everything => NotificationTrigger::all().to_vec(),
        }
    }

    fn describe(self) -> &'static str {
        match self {
            PrefTrigger::OnGrab => "downloads starting",
            PrefTrigger::OnImport => "things finishing",
            PrefTrigger::OnFailed => "failures",
            PrefTrigger::Everything => "anything",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MuteDuration {
    AnHour,
    ACoupleOfHours,
    TheRestOfToday,
    UntilTomorrow,
    AWeek,
    Indefinitely,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetNotificationPrefs {
    pub action: Option<PrefAction>,
    pub trigger: Option<PrefTrigger>,
    pub duration: Option<MuteDuration>,
    pub quiet_from: Option<String>,
    pub quiet_to: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PrefUpdate {
    pub prefs: ChatNotificationPrefs,
    /// A plain sentence naming what changed, for the bot to echo back. Every
    /// action needs a terminal message; silence reads as broken.
    pub summary: String,
    /// The clock the window was judged against, so a caller rendering the times
    /// can name it without resolving the user's zone a second time.
    pub zone: Zone,
}

#[derive(Debug, thiserror::Error)]
pub enum PrefsError {
    #[error("{0}")]
    Invalid(String),

    #[error("database error: {0}")]
    Db(#[from] crate::repositories::RepositoryError),
}

// ============================================================================
// Relative time, resolved here
// ============================================================================

/// `None` is "indefinitely", which is stored as `enabled = false` rather than a
/// timestamp far in the future.
pub fn resolve_mute_until(
    duration: MuteDuration,
    now: OffsetDateTime,
    zone: &Zone,
) -> Option<OffsetDateTime> {
    let tomorrow = zone.local_date(now) + Duration::days(1);
    match duration {
        MuteDuration::AnHour => Some(now + Duration::hours(1)),
        MuteDuration::ACoupleOfHours => Some(now + Duration::hours(2)),
        MuteDuration::AWeek => Some(now + Duration::days(7)),
        MuteDuration::TheRestOfToday => Some(zone.wall_clock_instant(tomorrow, Time::MIDNIGHT)),
        MuteDuration::UntilTomorrow => Some(zone.wall_clock_instant(
            tomorrow,
            Time::from_hms(TOMORROW_RESUME_HOUR, 0, 0).expect("08:00 is a valid time"),
        )),
        MuteDuration::Indefinitely => None,
    }
}

/// Accepts what the model actually emits: `22:00`, `22:00:00`, `22`, `10pm`,
/// `7 am`. Anything else is a question back to the user, not a guess.
pub fn parse_wall_clock(raw: &str) -> Result<Time, PrefsError> {
    let text = raw.trim().to_ascii_lowercase();
    let (body, shift) = if let Some(rest) = text.strip_suffix("pm") {
        (rest.trim().to_string(), 12u8)
    } else if let Some(rest) = text.strip_suffix("am") {
        (rest.trim().to_string(), 0u8)
    } else {
        (text.clone(), 0u8)
    };

    let mut parts = body.split(':');
    let hour_raw: u8 = parts
        .next()
        .and_then(|h| h.trim().parse().ok())
        .ok_or_else(|| PrefsError::Invalid(format!("'{raw}' is not a time like 22:00")))?;
    let minute: u8 = match parts.next() {
        Some(m) => m
            .trim()
            .parse()
            .map_err(|_| PrefsError::Invalid(format!("'{raw}' is not a time like 22:00")))?,
        None => 0,
    };
    let second: u8 = match parts.next() {
        Some(s) => s
            .trim()
            .parse()
            .map_err(|_| PrefsError::Invalid(format!("'{raw}' is not a time like 22:00")))?,
        None => 0,
    };
    if parts.next().is_some() {
        return Err(PrefsError::Invalid(format!(
            "'{raw}' is not a time like 22:00"
        )));
    }

    let hour = if shift == 12 && hour_raw < 12 {
        hour_raw + 12
    } else if shift == 0 && text.ends_with("am") && hour_raw == 12 {
        0
    } else {
        hour_raw
    };

    Time::from_hms(hour, minute, second)
        .map_err(|_| PrefsError::Invalid(format!("'{raw}' is not a real time of day")))
}

// ============================================================================
// Applying a change
// ============================================================================

pub async fn apply(
    pool: &PgPool,
    user_id: Uuid,
    req: &SetNotificationPrefs,
) -> Result<PrefUpdate, PrefsError> {
    apply_at(pool, user_id, req, OffsetDateTime::now_utc()).await
}

/// The deterministic form: `now` is injected so relative durations are testable.
pub async fn apply_at(
    pool: &PgPool,
    user_id: Uuid,
    req: &SetNotificationPrefs,
    now: OffsetDateTime,
) -> Result<PrefUpdate, PrefsError> {
    let zone = zone_for_user(pool, user_id).await;
    let mut prefs = chat_notification_prefs::get(pool, user_id).await?;
    let action = req.action.unwrap_or(PrefAction::Status);

    let summary = match action {
        PrefAction::Status => describe(&prefs, &zone, now),

        PrefAction::Mute => {
            let duration = req.duration.unwrap_or(MuteDuration::ACoupleOfHours);
            match resolve_mute_until(duration, now, &zone) {
                Some(until) => {
                    prefs.enabled = true;
                    prefs.muted_until = Some(until);
                    format!("Muted until {}.", wall_clock_phrase(until, now, &zone))
                }
                None => {
                    prefs.enabled = false;
                    prefs.muted_until = None;
                    "Muted. Say \"start again\" whenever you want me back.".to_string()
                }
            }
        }

        PrefAction::Resume => {
            prefs.enabled = true;
            prefs.muted_until = None;
            if prefs.triggers.is_empty() {
                prefs.triggers = vec![NotificationTrigger::OnImport];
            }
            format!("Back on. {}", trigger_phrase(&prefs.triggers))
        }

        PrefAction::EnableTrigger => {
            let target = req.trigger.unwrap_or(PrefTrigger::Everything);
            for trigger in target.expand() {
                if !prefs.triggers.contains(&trigger) {
                    prefs.triggers.push(trigger);
                }
            }
            sort_triggers(&mut prefs.triggers);
            prefs.enabled = true;
            format!("I'll tell you about {}.", target.describe())
        }

        PrefAction::DisableTrigger => {
            let target = req.trigger.unwrap_or(PrefTrigger::Everything);
            let removed = target.expand();
            prefs.triggers.retain(|t| !removed.contains(t));
            format!("I'll stop telling you about {}.", target.describe())
        }

        PrefAction::SetQuietHours => {
            let from = req
                .quiet_from
                .as_deref()
                .ok_or_else(|| PrefsError::Invalid("Which time should quiet hours start?".into()))
                .and_then(parse_wall_clock)?;
            let to = req
                .quiet_to
                .as_deref()
                .ok_or_else(|| PrefsError::Invalid("Which time should quiet hours end?".into()))
                .and_then(parse_wall_clock)?;
            if from == to {
                return Err(PrefsError::Invalid(
                    "Quiet hours need a start and an end that differ.".into(),
                ));
            }
            prefs.quiet_from = Some(from);
            prefs.quiet_to = Some(to);
            format!(
                "Quiet from {} to {} ({}).",
                hhmm(from),
                hhmm(to),
                zone.label_at(now)
            )
        }

        // Unset and cleared both mean "no quiet hours" now, but clearing still writes
        // a zero-width window rather than NULLing: it records that someone decided,
        // which is what a later "put them back how they were" would need.
        PrefAction::ClearQuietHours => {
            prefs.quiet_from = Some(Time::MIDNIGHT);
            prefs.quiet_to = Some(Time::MIDNIGHT);
            "No quiet hours — I'll message at any time of day.".to_string()
        }
    };

    let saved = if action == PrefAction::Status {
        prefs
    } else {
        chat_notification_prefs::upsert(pool, &prefs).await?
    };

    Ok(PrefUpdate {
        prefs: saved,
        summary,
        zone,
    })
}

// ============================================================================
// Phrasing
// ============================================================================

fn sort_triggers(triggers: &mut [NotificationTrigger]) {
    let order = NotificationTrigger::all();
    triggers.sort_by_key(|t| order.iter().position(|o| o == t).unwrap_or(usize::MAX));
}

pub fn hhmm(t: Time) -> String {
    format!("{:02}:{:02}", t.hour(), t.minute())
}

fn wall_clock_phrase(until: OffsetDateTime, now: OffsetDateTime, zone: &Zone) -> String {
    let until_date = zone.local_date(until);
    let today = zone.local_date(now);
    let clock = hhmm(zone.local_time(until));
    if until_date == today {
        clock
    } else if until_date == today + Duration::days(1) {
        format!("{clock} tomorrow")
    } else {
        format!("{clock} on {until_date}")
    }
}

fn trigger_phrase(triggers: &[NotificationTrigger]) -> String {
    if triggers.is_empty() {
        return "Nothing is switched on, so you still won't hear from me.".to_string();
    }
    let names: Vec<&str> = triggers
        .iter()
        .map(|t| match t {
            NotificationTrigger::OnGrab => "downloads starting",
            NotificationTrigger::OnImport => "things finishing",
            NotificationTrigger::OnFailed => "failures",
            NotificationTrigger::OnMediaCorrupt => "corrupt files",
            NotificationTrigger::OnTest => "tests",
        })
        .collect();
    format!("You'll hear about {}.", names.join(" and "))
}

/// What the bot reads back for `status`, and the one place the resolved zone is
/// said out loud so a wrong one cannot stay silent.
pub fn describe(prefs: &ChatNotificationPrefs, zone: &Zone, now: OffsetDateTime) -> String {
    if !prefs.enabled {
        return "Notifications are off. Say \"start again\" to turn them back on.".to_string();
    }
    let mut parts = vec![trigger_phrase(&prefs.triggers)];
    if let Some(until) = prefs.muted_until
        && until > now
    {
        parts.push(format!("Muted until {}.", hhmm(zone.local_time(until))));
    }
    match prefs.quiet_window() {
        Some((from, to)) => parts.push(format!(
            "Quiet from {} to {} ({}).",
            hhmm(from),
            hhmm(to),
            zone.label_at(now)
        )),
        None => parts.push("No quiet hours.".to_string()),
    }
    parts.join(" ")
}

// ============================================================================
// Tool schema — the chat tool file wires this, it does not restate it
// ============================================================================

pub fn tool_description() -> &'static str {
    "Change how and when this person hears from you about downloads: mute for a while, \
     stop or start a kind of notification, set quiet hours, or read back the current \
     settings. Use it for 'stop telling me about downloads', 'shut up for a couple of \
     hours', 'not before 8am' and 'start again'. It only ever changes the settings of \
     the person you are talking to."
}

pub fn tool_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": [
                    "mute", "resume", "enable_trigger", "disable_trigger",
                    "set_quiet_hours", "clear_quiet_hours", "status"
                ],
                "description": "What they asked for. 'shut up for a bit' is mute; 'start again' is resume; 'stop telling me about downloads' is disable_trigger; 'not before 8am' is set_quiet_hours; 'what are my settings' is status.",
            },
            "trigger": {
                "type": "string",
                "enum": ["on_grab", "on_import", "on_failed", "everything"],
                "description": "Which kind of message, for enable_trigger and disable_trigger. on_grab is a download starting, on_import is something finishing and arriving, on_failed is a search that gave up. Use 'everything' when they did not narrow it down.",
            },
            "duration": {
                "type": "string",
                "enum": [
                    "an_hour", "a_couple_of_hours", "the_rest_of_today",
                    "until_tomorrow", "a_week", "indefinitely"
                ],
                "description": "How long to mute for, when the action is mute. Pick the closest phrase they used — do not work out a time or a date yourself. Leave it out and it means a couple of hours.",
            },
            "quiet_from": {
                "type": "string",
                "description": "When quiet hours start, as 24-hour HH:MM in their local time, e.g. '22:00'. Only for set_quiet_hours.",
            },
            "quiet_to": {
                "type": "string",
                "description": "When quiet hours end, as 24-hour HH:MM, e.g. '07:00'. A window that runs past midnight is fine. Only for set_quiet_hours.",
            },
        },
        "required": ["action"],
    })
}
