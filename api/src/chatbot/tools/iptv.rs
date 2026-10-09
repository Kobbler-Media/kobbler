//! Live TV: what is on, and record it.
//!
//! IPTV is the one media type with no acquisition step. There is nothing to
//! search for, nothing to queue and no import to wait on — the programme either
//! goes out at 21:00 or it does not — so these two tools deliberately do **not**
//! wear the `want_it` shape: no `MediaRef`, no `chat_watches`, no progress.
//!
//! Three rules carry over unchanged from the acquisition half:
//!
//! * **the model never does date arithmetic.** "tonight" arrives as an enum and
//!   the timestamps are worked out here, in the server's local timezone, the
//!   same one quiet hours already use;
//! * **ambiguity is a question.** Two channels carrying the same match, or a
//!   channel name matching three channels, returns candidates and touches
//!   nothing;
//! * **an empty guide is not an empty schedule.** A channel with no EPG rows
//!   says so in as many words, because "nothing's on" is a lie the model would
//!   otherwise tell confidently.

use serde_json::{Value, json};
use time::{Duration, OffsetDateTime, UtcOffset};
use uuid::Uuid;

use crate::AppState;
use crate::notifications::prefs::server_local_offset;
use crate::repositories::iptv_epg;
use crate::repositories::iptv_recordings::{self, CreateRecording};

use super::act::gate;
use super::dispatch::Ctx;
use super::read::ToolOutcome;

/// A wide guide drowns a 12B model, so the list is cut here rather than in the
/// prompt. `limit` can only ever narrow it.
const MAX_PROGRAMMES: usize = 12;
const DEFAULT_PROGRAMMES: usize = 8;

/// How many channels a channel-less "what's on" sweeps. Favourites first, so on
/// a household box this is her own row of channels, not the first twelve
/// alphabetically.
const MAX_CHANNELS: usize = 12;

/// Two per channel is enough to answer "and after that?" without turning a
/// twelve-channel sweep into twenty-four lines.
const PER_CHANNEL: usize = 2;

/// Candidates offered when a name is ambiguous. Past this the answer is "be more
/// specific", not a longer list.
const MAX_CANDIDATES: usize = 6;

/// How far ahead `record_this` will look for a programme by name. The guide
/// rarely runs further out than this and a fortnight covers "next Saturday".
const RECORD_HORIZON: Duration = Duration::days(14);

// =============================================================================
// Arguments
// =============================================================================

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn arg_limit(args: &Value) -> usize {
    args.get("limit")
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        })
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_PROGRAMMES)
        .clamp(1, MAX_PROGRAMMES)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Window {
    Now,
    Tonight,
    Later,
}

impl Window {
    fn parse(args: &Value) -> Self {
        match arg_str(args, "when")
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("tonight" | "this_evening" | "evening") => Window::Tonight,
            Some("later" | "next" | "coming_up" | "soon") => Window::Later,
            _ => Window::Now,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Window::Now => "now",
            Window::Tonight => "tonight",
            Window::Later => "later",
        }
    }
}

/// Resolve the enum to real timestamps. This is the whole reason `when` is an
/// enum: Gemma is bad at date arithmetic and the answer has to be right.
fn bounds(window: Window, now: OffsetDateTime) -> (OffsetDateTime, OffsetDateTime) {
    match window {
        Window::Now => (now, now + Duration::seconds(1)),
        Window::Later => (now, now + Duration::hours(4)),
        Window::Tonight => {
            let offset = server_local_offset();
            let local = now.to_offset(offset);
            let Ok(six) = local.date().with_hms(18, 0, 0) else {
                return (now, now + Duration::hours(6));
            };
            let start = six.assume_offset(offset);
            let end = start + Duration::hours(6);
            (start.max(now), end.max(now))
        }
    }
}

// =============================================================================
// Saying a time out loud
// =============================================================================

fn weekday(dt: OffsetDateTime) -> &'static str {
    use time::Weekday::*;
    match dt.weekday() {
        Monday => "Mon",
        Tuesday => "Tue",
        Wednesday => "Wed",
        Thursday => "Thu",
        Friday => "Fri",
        Saturday => "Sat",
        Sunday => "Sun",
    }
}

fn month(dt: OffsetDateTime) -> &'static str {
    use time::Month::*;
    match dt.month() {
        January => "Jan",
        February => "Feb",
        March => "Mar",
        April => "Apr",
        May => "May",
        June => "Jun",
        July => "Jul",
        August => "Aug",
        September => "Sep",
        October => "Oct",
        November => "Nov",
        December => "Dec",
    }
}

/// A clock time a human would say, in the server's local timezone, dated only
/// when it is not today.
pub(super) fn spoken(at: OffsetDateTime, now: OffsetDateTime, offset: UtcOffset) -> String {
    let local = at.to_offset(offset);
    let today = now.to_offset(offset).date();
    let clock = format!("{:02}:{:02}", local.hour(), local.minute());
    if local.date() == today {
        return clock;
    }
    if today.next_day() == Some(local.date()) {
        return format!("tomorrow {clock}");
    }
    format!(
        "{} {} {} {clock}",
        weekday(local),
        local.day(),
        month(local)
    )
}

// =============================================================================
// Channels
// =============================================================================

struct Channel {
    id: Uuid,
    name: String,
}

/// Channels whose name the user's words match, best first.
///
/// Every tie is broken down to the id, deliberately: an IPTV install has several
/// channels with near-identical names and "whichever row came back" is how the
/// bot answers about the wrong one.
async fn channels_named(state: &AppState, query: &str) -> Result<Vec<Channel>, String> {
    let like = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
    let rows = sqlx::query!(
        r#"
        SELECT id, name
        FROM iptv_channels
        WHERE enabled
          AND (name ILIKE $1 OR normalized_name ILIKE $1)
        ORDER BY (LOWER(name) = LOWER($2)) DESC,
                 LENGTH(name) ASC,
                 display_order ASC,
                 name ASC,
                 id ASC
        LIMIT $3
        "#,
        like,
        query,
        MAX_CANDIDATES as i64,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| format!("could not look that channel up: {e}"))?;

    Ok(rows
        .into_iter()
        .map(|r| Channel {
            id: r.id,
            name: r.name,
        })
        .collect())
}

/// The channels a channel-less question is about: hers first, everyone's after.
async fn default_channels(state: &AppState, user_id: Uuid) -> Result<Vec<Channel>, String> {
    let favourites = sqlx::query!(
        r#"
        SELECT c.id, c.name
        FROM iptv_favourites f
        JOIN iptv_channels c ON c.id = f.channel_id
        WHERE f.user_id = $1 AND c.enabled
        ORDER BY f.display_order ASC, c.name ASC, c.id ASC
        LIMIT $2
        "#,
        user_id,
        MAX_CHANNELS as i64,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| format!("could not read your channels: {e}"))?;

    if !favourites.is_empty() {
        return Ok(favourites
            .into_iter()
            .map(|r| Channel {
                id: r.id,
                name: r.name,
            })
            .collect());
    }

    let rows = sqlx::query!(
        r#"
        SELECT id, name
        FROM iptv_channels
        WHERE enabled
        ORDER BY display_order ASC, name ASC, id ASC
        LIMIT $1
        "#,
        MAX_CHANNELS as i64,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| format!("could not list channels: {e}"))?;

    Ok(rows
        .into_iter()
        .map(|r| Channel {
            id: r.id,
            name: r.name,
        })
        .collect())
}

fn ask_which_channel(query: &str, candidates: &[Channel]) -> ToolOutcome {
    let options: Vec<Value> = candidates
        .iter()
        .map(|c| json!({"channel": c.name}))
        .collect();
    ToolOutcome::ok(
        json!({
            "ok": true,
            "found": false,
            "ambiguous": true,
            "channels": options,
            "note": "Several channels match that name. Ask which one they mean; do not pick one.",
        }),
        format!("{} channels match {query}", candidates.len()),
    )
}

// =============================================================================
// whats_on
// =============================================================================

pub async fn whats_on(ctx: &Ctx<'_>, args: &Value) -> ToolOutcome {
    let state = ctx.state;
    let window = Window::parse(args);
    let wanted = arg_limit(args);
    let now = OffsetDateTime::now_utc();
    let offset = server_local_offset();
    let (from, to) = bounds(window, now);

    let (channels, named) = match arg_str(args, "channel") {
        Some(query) => match channels_named(state, query).await {
            Ok(found) if found.is_empty() => {
                return ToolOutcome::ok(
                    json!({
                        "ok": true,
                        "found": false,
                        "unknown_channel": query,
                        "note": format!(
                            "There is no channel called '{query}' here. Say that, and offer to \
                             check what is on generally rather than guessing which channel they meant."
                        ),
                    }),
                    format!("no channel called {query}"),
                );
            }
            Ok(found) if found.len() > 1 && !found[0].name.eq_ignore_ascii_case(query) => {
                return ask_which_channel(query, &found);
            }
            Ok(mut found) => {
                found.truncate(1);
                (found, true)
            }
            Err(e) => return ToolOutcome::failed(e),
        },
        None => match default_channels(state, ctx.auth.user_id).await {
            Ok(found) if found.is_empty() => {
                return ToolOutcome::ok(
                    json!({
                        "ok": true,
                        "found": false,
                        "no_channels": true,
                        "note": "There are no live TV channels set up at all, so there is no guide to read.",
                    }),
                    "no iptv channels".to_string(),
                );
            }
            Ok(found) => (found, false),
            Err(e) => return ToolOutcome::failed(e),
        },
    };

    let ids: Vec<Uuid> = channels.iter().map(|c| c.id).collect();
    let entries = match iptv_epg::list_by_channel_ids_time_range(&state.pool, &ids, from, to).await
    {
        Ok(e) => e,
        Err(e) => return ToolOutcome::failed(format!("could not read the guide: {e}")),
    };

    if entries.is_empty() {
        return empty_guide(state, &channels, window, named).await;
    }

    let mut programmes: Vec<Value> = Vec::new();
    let mut total = 0usize;
    for channel in &channels {
        let mut taken = 0usize;
        let per_channel = match named {
            true => MAX_PROGRAMMES,
            false => PER_CHANNEL,
        };
        let mut for_channel: Vec<_> = entries
            .iter()
            .filter(|e| e.channel_id == channel.id)
            .collect();
        for_channel.sort_by_key(|e| (e.start_time, e.id));
        for entry in for_channel {
            total += 1;
            if taken >= per_channel || programmes.len() >= wanted {
                continue;
            }
            taken += 1;
            programmes.push(json!({
                "programme_id": entry.id,
                "channel": channel.name,
                "title": entry.title.clone().unwrap_or_else(|| "Untitled".to_string()),
                "starts": spoken(entry.start_time, now, offset),
                "ends": spoken(entry.stop_time, now, offset),
                "on_now": entry.start_time <= now && entry.stop_time > now,
                "starts_in_minutes": ((entry.start_time - now).whole_minutes()).max(0),
            }));
        }
    }

    let showing = programmes.len();
    ToolOutcome::ok(
        json!({
            "ok": true,
            "found": showing > 0,
            "window": window.name(),
            "window_local": window_description(window, from, to, now, offset),
            "showing": showing,
            "truncated": total > showing,
            "channels_checked": channels.len(),
            "programmes": programmes,
            "note": "Times are local. Use a programme_id from this list if they ask to record one — \
                     never make one up.",
        }),
        match named {
            true => format!("{showing} on {}", channels[0].name),
            false => format!("{showing} programmes {}", window.name()),
        },
    )
}

fn window_description(
    window: Window,
    from: OffsetDateTime,
    to: OffsetDateTime,
    now: OffsetDateTime,
    offset: UtcOffset,
) -> String {
    match window {
        Window::Now => format!("right now, {}", spoken(now, now, offset)),
        Window::Later => format!(
            "{} to {}",
            spoken(from, now, offset),
            spoken(to, now, offset)
        ),
        Window::Tonight => format!(
            "this evening, {} to {}",
            spoken(from, now, offset),
            spoken(to, now, offset)
        ),
    }
}

/// The honest answer when the window came back empty.
///
/// "Nothing's on" and "I have no guide for this channel" are completely
/// different answers and the second one is the true one far more often, so the
/// difference is established with a count rather than assumed.
async fn empty_guide(
    state: &AppState,
    channels: &[Channel],
    window: Window,
    named: bool,
) -> ToolOutcome {
    let ids: Vec<Uuid> = channels.iter().map(|c| c.id).collect();
    let any = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!" FROM iptv_epg WHERE channel_id = ANY($1)"#,
        &ids,
    )
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0);

    let names: Vec<&str> = channels.iter().map(|c| c.name.as_str()).collect();
    let which = match named {
        true => names.first().copied().unwrap_or("that channel").to_string(),
        false => "those channels".to_string(),
    };
    let (guide, note) = match any {
        0 => (
            "none",
            format!(
                "There is no guide data for {which} at all, so I cannot tell what is on. Say that \
                 it is the listings that are missing — do not say nothing is on."
            ),
        ),
        _ => (
            "outside_window",
            format!(
                "The guide has listings for {which} but nothing covering {}. Say what the guide \
                 does not cover rather than implying the screen is blank.",
                window.name()
            ),
        ),
    };
    ToolOutcome::ok(
        json!({
            "ok": true,
            "found": false,
            "window": window.name(),
            "guide_data": guide,
            "channels": names,
            "note": note,
        }),
        format!("no listings {} for {which}", window.name()),
    )
}

// =============================================================================
// record_this
// =============================================================================

struct Programme {
    id: Uuid,
    channel_id: Uuid,
    channel: String,
    title: String,
    start_time: OffsetDateTime,
    stop_time: OffsetDateTime,
}

async fn programme_by_id(state: &AppState, id: Uuid) -> Result<Option<Programme>, String> {
    let row = sqlx::query!(
        r#"
        SELECT e.id, e.channel_id, e.title, e.start_time, e.stop_time, c.name AS channel
        FROM iptv_epg e
        JOIN iptv_channels c ON c.id = e.channel_id
        WHERE e.id = $1
        "#,
        id,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| format!("could not read the guide: {e}"))?;

    Ok(row.map(|r| Programme {
        id: r.id,
        channel_id: r.channel_id,
        channel: r.channel,
        title: r.title.unwrap_or_else(|| "Untitled".to_string()),
        start_time: r.start_time,
        stop_time: r.stop_time,
    }))
}

async fn programmes_by_title(
    state: &AppState,
    title: &str,
    channel_id: Option<Uuid>,
    now: OffsetDateTime,
) -> Result<Vec<Programme>, String> {
    let like = format!("%{}%", title.replace('%', "\\%").replace('_', "\\_"));
    let rows = sqlx::query!(
        r#"
        SELECT e.id, e.channel_id, e.title, e.start_time, e.stop_time, c.name AS channel
        FROM iptv_epg e
        JOIN iptv_channels c ON c.id = e.channel_id
        WHERE c.enabled
          AND e.stop_time > $1
          AND e.start_time < $2
          AND e.title ILIKE $3
          AND ($4::uuid IS NULL OR e.channel_id = $4)
        ORDER BY e.start_time ASC, c.display_order ASC, c.name ASC, e.id ASC
        LIMIT $5
        "#,
        now,
        now + RECORD_HORIZON,
        like,
        channel_id,
        MAX_CANDIDATES as i64,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| format!("could not search the guide: {e}"))?;

    Ok(rows
        .into_iter()
        .map(|r| Programme {
            id: r.id,
            channel_id: r.channel_id,
            channel: r.channel,
            title: r.title.unwrap_or_else(|| "Untitled".to_string()),
            start_time: r.start_time,
            stop_time: r.stop_time,
        })
        .collect())
}

/// Work out what to record, or ask.
///
/// Returning `Err` here always means the caller must return that outcome
/// untouched — nothing has been scheduled and nothing may be.
async fn subject(
    ctx: &Ctx<'_>,
    args: &Value,
    now: OffsetDateTime,
) -> Result<Programme, ToolOutcome> {
    let offset = server_local_offset();
    if let Some(raw) = arg_str(args, "programme_id") {
        let Ok(id) = raw.parse::<Uuid>() else {
            return Err(ToolOutcome::failed(
                "that is not a programme_id from a whats_on result — call whats_on and use the \
                 programme_id it gives you",
            ));
        };
        return match programme_by_id(ctx.state, id).await {
            Err(e) => Err(ToolOutcome::failed(e)),
            Ok(None) => Err(ToolOutcome::failed(
                "that programme is no longer in the guide — call whats_on again and use a fresh \
                 programme_id",
            )),
            Ok(Some(p)) => Ok(p),
        };
    }

    let Some(title) = arg_str(args, "title") else {
        return Err(ToolOutcome::failed(
            "record_this needs either a programme_id from whats_on or the name of the programme",
        ));
    };

    let channel_id = match arg_str(args, "channel") {
        None => None,
        Some(query) => match channels_named(ctx.state, query).await {
            Err(e) => return Err(ToolOutcome::failed(e)),
            Ok(found) if found.is_empty() => {
                return Err(ToolOutcome::ok(
                    json!({
                        "ok": true,
                        "scheduled": false,
                        "unknown_channel": query,
                        "note": format!("There is no channel called '{query}' here. Ask which channel they meant; nothing has been recorded."),
                    }),
                    format!("no channel called {query}"),
                ));
            }
            Ok(found) if found.len() > 1 && !found[0].name.eq_ignore_ascii_case(query) => {
                return Err(ask_which_channel(query, &found));
            }
            Ok(found) => Some(found[0].id),
        },
    };

    let found = programmes_by_title(ctx.state, title, channel_id, now)
        .await
        .map_err(ToolOutcome::failed)?;

    match found.len() {
        0 => Err(ToolOutcome::ok(
            json!({
                "ok": true,
                "scheduled": false,
                "found": false,
                "note": format!(
                    "Nothing called '{title}' is in the guide between now and a fortnight's time. \
                     Say the listings do not have it rather than promising to record it."
                ),
            }),
            format!("no listing for {title}"),
        )),
        1 => Ok(found.into_iter().next().expect("one")),
        _ => {
            let options: Vec<Value> = found
                .iter()
                .map(|p| {
                    json!({
                        "programme_id": p.id,
                        "channel": p.channel,
                        "title": p.title,
                        "starts": spoken(p.start_time, now, offset),
                    })
                })
                .collect();
            Err(ToolOutcome::ok(
                json!({
                    "ok": true,
                    "scheduled": false,
                    "ambiguous": true,
                    "candidates": options,
                    "note": "More than one listing matches. Ask which one they mean and call this \
                             again with its programme_id. Nothing has been recorded.",
                }),
                format!("{} listings match {title}", options.len()),
            ))
        }
    }
}

pub async fn record_this(ctx: &Ctx<'_>, args: &Value) -> ToolOutcome {
    let now = OffsetDateTime::now_utc();
    let offset = server_local_offset();

    let programme = match subject(ctx, args, now).await {
        Ok(p) => p,
        Err(outcome) => return outcome,
    };

    if programme.stop_time <= now {
        return ToolOutcome::ok(
            json!({
                "ok": true,
                "scheduled": false,
                "already_over": true,
                "title": programme.title,
                "channel": programme.channel,
                "note": format!(
                    "'{}' finished at {}. Live TV cannot be recorded after the fact — say so, and \
                     offer to record the next showing if they want.",
                    programme.title,
                    spoken(programme.stop_time, now, offset)
                ),
            }),
            format!("{} already finished", programme.title),
        );
    }

    let target = format!(
        "{} on {} at {}",
        programme.title,
        programme.channel,
        spoken(programme.start_time, now, offset)
    );
    // The offer/consume pair is `act`'s, on purpose: one confirmation mechanism,
    // one TTL, one single-use guarantee.
    let confirmed = gate(ctx, "record", &target, args);
    // Re-resolve from the merged arguments, not the ones the confirming call
    // happened to carry: a "yes" that dropped `programme_id` and kept only a
    // title must still record the programme the sentence named.
    let programme = match subject(ctx, &confirmed, now).await {
        Ok(p) => p,
        Err(outcome) => return outcome,
    };

    match existing_recording(ctx.state, &programme).await {
        Err(e) => return ToolOutcome::failed(e),
        Ok(Some(id)) => {
            return ToolOutcome::ok(
                json!({
                    "ok": true,
                    "scheduled": true,
                    "already_scheduled": true,
                    "recording_id": id,
                    "title": programme.title,
                    "channel": programme.channel,
                    "starts": spoken(programme.start_time, now, offset),
                    "say": format!(
                        "That one's already set to record — {} on {} at {}.",
                        programme.title,
                        programme.channel,
                        spoken(programme.start_time, now, offset)
                    ),
                }),
                format!("{} already scheduled", programme.title),
            );
        }
        Ok(None) => {}
    }

    let created = iptv_recordings::create(
        &ctx.state.pool,
        CreateRecording {
            channel_id: programme.channel_id,
            epg_id: Some(programme.id),
            title: programme.title.clone(),
            start_time: programme.start_time,
            end_time: programme.stop_time,
            pre_padding_secs: None,
            post_padding_secs: None,
        },
    )
    .await;

    let row = match created {
        Ok(row) => row,
        Err(e) => return ToolOutcome::failed(format!("the recording could not be set up: {e}")),
    };

    let starts = spoken(row.start_time, now, offset);
    let ends = spoken(row.end_time, now, offset);
    let on_now = row.start_time <= now;
    // A recording that fires in six hours is exactly the case where saying
    // nothing reads as broken, so the terminal message is the point of this
    // branch, not a decoration on it.
    let say = match on_now {
        true => format!(
            "Recording {} on {} now — it runs until {ends}.",
            row.title, programme.channel
        ),
        false => format!(
            "That's set — I'll record {} on {} from {starts} to {ends}.",
            row.title, programme.channel
        ),
    };
    ToolOutcome::ok(
        json!({
            "ok": true,
            "scheduled": true,
            "recording_id": row.id,
            "title": row.title,
            "channel": programme.channel,
            "starts": starts,
            "ends": ends,
            "in_progress": on_now,
            "say": say,
            "note": "Say this to the user. The recording is scheduled and will run on its own; there \
                     is no download to wait for and nothing else to do.",
        }),
        format!("recording {} at {starts}", row.title),
    )
}

/// Whether this programme is already set to record.
///
/// Asking twice is the normal shape of a conversation ("did you get that?"), and
/// a second row would mean two ffmpeg captures of one programme fighting over
/// the same provider connection.
async fn existing_recording(
    state: &AppState,
    programme: &Programme,
) -> Result<Option<Uuid>, String> {
    sqlx::query_scalar!(
        r#"
        SELECT id
        FROM iptv_recordings
        WHERE channel_id = $1
          AND status IN ('scheduled', 'recording')
          AND (epg_id = $2 OR (start_time = $3 AND end_time = $4))
        ORDER BY created_at ASC, id ASC
        LIMIT 1
        "#,
        programme.channel_id,
        programme.id,
        programme.start_time,
        programme.stop_time,
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| format!("could not check what is already recording: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_window_enum_is_the_only_date_arithmetic_anyone_does() {
        assert_eq!(Window::parse(&json!({})), Window::Now);
        assert_eq!(Window::parse(&json!({"when": "now"})), Window::Now);
        assert_eq!(Window::parse(&json!({"when": "Tonight"})), Window::Tonight);
        assert_eq!(Window::parse(&json!({"when": "later"})), Window::Later);
        assert_eq!(Window::parse(&json!({"when": "gibberish"})), Window::Now);
    }

    #[test]
    fn tonight_is_this_evening_local_and_never_starts_in_the_past() {
        let now = OffsetDateTime::now_utc();
        let (from, to) = bounds(Window::Tonight, now);
        assert!(from >= now, "tonight cannot begin before now");
        assert!(to > from);
        let local = from.to_offset(server_local_offset());
        assert!(
            local.hour() >= 18 || from == now,
            "evening window began at {}",
            local.hour()
        );
    }

    #[test]
    fn now_is_an_instant_and_later_is_the_next_few_hours() {
        let now = OffsetDateTime::now_utc();
        let (from, to) = bounds(Window::Now, now);
        assert_eq!(from, now);
        assert!(to - from < Duration::minutes(1));
        let (from, to) = bounds(Window::Later, now);
        assert_eq!(from, now);
        assert_eq!(to - from, Duration::hours(4));
    }

    #[test]
    fn a_limit_can_only_ever_narrow_the_list() {
        assert_eq!(arg_limit(&json!({})), DEFAULT_PROGRAMMES);
        assert_eq!(arg_limit(&json!({"limit": 3})), 3);
        assert_eq!(arg_limit(&json!({"limit": 500})), MAX_PROGRAMMES);
        assert_eq!(arg_limit(&json!({"limit": 0})), 1);
    }

    #[test]
    fn a_time_today_is_a_clock_and_a_time_next_week_is_dated() {
        let offset = UtcOffset::UTC;
        let now = time::Date::from_calendar_date(2026, time::Month::August, 24)
            .unwrap()
            .with_hms(9, 0, 0)
            .unwrap()
            .assume_offset(offset);
        assert_eq!(spoken(now + Duration::hours(2), now, offset), "11:00");
        assert_eq!(
            spoken(now + Duration::hours(24), now, offset),
            "tomorrow 09:00"
        );
        assert_eq!(
            spoken(now + Duration::days(5), now, offset),
            "Sat 29 Aug 09:00"
        );
    }
}
