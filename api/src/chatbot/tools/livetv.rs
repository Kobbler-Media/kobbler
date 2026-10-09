//! Recording broadcast TV from chat (Decision 17).
//!
//! The tools are the easy half. The hard half is that "record The Block" is
//! ambiguous in four separate ways at once — which airing, this one or every
//! episode, which channel, and new episodes only — and the model must resolve
//! all four *before* anything is scheduled.
//!
//! So the search tool does not return a flat list of guide rows. It returns
//! candidates already clustered and already deduplicated by
//! [`crate::livetv::series_rules`], with that module's own per-airing reason
//! attached, plus an `ask_about` list naming the fields that are genuinely
//! ambiguous in *this* result. The clustering is server-side and authoritative;
//! the model's job is to ask, not to guess and not to cluster.
//!
//! Nothing here is series-specific. A rule that says "9Life is the back
//! catalogue" would be a prompt that only works until the next network
//! reshuffle — the same conclusion falls out of "this series airs on two
//! channels in the window, ask which".

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};
use time::{Duration, OffsetDateTime, Time, UtcOffset};
use uuid::Uuid;

use crate::AppState;
use crate::events::LibraryEvent;
use crate::livetv::classify;
use crate::livetv::series_rules::{self, Action, Airing, Cluster, Member, RuleSpec};
use crate::notifications::prefs::server_local_offset;
use crate::repositories::{livetv_channels, livetv_guide, livetv_recordings, livetv_series_rules};

use super::act::gate;
use super::dispatch::Ctx;
use super::iptv::spoken;
use super::read::ToolOutcome;

/// A 48 h guide across forty channels is thousands of rows. These caps are what
/// keeps a search answerable by a 12B model rather than a wall of listings.
const MAX_MATCHES: usize = 4;
const MAX_EPISODES: usize = 4;
const MAX_CHANNELS: usize = 6;
const MAX_REPEATS: usize = 3;
const MAX_RECORDINGS: usize = 20;
const DEFAULT_RECORDINGS: usize = 8;

/// The same defaults `POST /recordings` uses, so a recording set from chat and
/// one set from the web app have identical padding.
const PADDING_BEFORE_SECS: i32 = 120;
const PADDING_AFTER_SECS: i32 = 360;

/// How wide a "same slot" rule window is around the airing it was built from.
/// Decision 6.5: the weekday/time window is the only thing separating a first
/// run from a sister channel's back catalogue, and broadcast slots drift.
const SLOT_TOLERANCE: Duration = Duration::minutes(90);

// =============================================================================
// Arguments
// =============================================================================

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn arg_bool(args: &Value, key: &str) -> Option<bool> {
    match args.get(key) {
        Some(Value::Bool(b)) => Some(*b),
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "y" => Some(true),
            "false" | "no" | "n" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn arg_uuid(args: &Value, key: &str) -> Option<Uuid> {
    arg_str(args, key).and_then(|s| s.parse().ok())
}

fn arg_limit(args: &Value, default: usize, max: usize) -> usize {
    args.get("limit")
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        })
        .map(|n| n as usize)
        .unwrap_or(default)
        .clamp(1, max)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum When {
    NextFewHours,
    Tonight,
    Tomorrow,
    Anytime,
}

impl When {
    fn parse(args: &Value) -> Self {
        match arg_str(args, "when")
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("next_few_hours" | "now" | "soon" | "later") => When::NextFewHours,
            Some("tonight" | "this_evening" | "evening") => When::Tonight,
            Some("tomorrow") => When::Tomorrow,
            _ => When::Anytime,
        }
    }

    fn name(self) -> &'static str {
        match self {
            When::NextFewHours => "next_few_hours",
            When::Tonight => "tonight",
            When::Tomorrow => "tomorrow",
            When::Anytime => "next_48_hours",
        }
    }
}

/// The model never does date arithmetic: `when` arrives as an enum and the
/// timestamps are worked out here, in the server's local timezone.
///
/// Everything is clamped to the guide's own 48 h ceiling, so "tomorrow night"
/// can never ask for a window the feed does not cover.
fn bounds(when: When, now: OffsetDateTime, offset: UtcOffset) -> (OffsetDateTime, OffsetDateTime) {
    let horizon = now + series_rules::GUIDE_HORIZON;
    let local = now.to_offset(offset);
    let (from, to) = match when {
        When::NextFewHours => (now, now + Duration::hours(4)),
        When::Anytime => (now, horizon),
        When::Tonight => match local.date().with_hms(18, 0, 0) {
            Ok(six) => {
                let start = six.assume_offset(offset);
                (start, start + Duration::hours(6))
            }
            Err(_) => (now, horizon),
        },
        When::Tomorrow => match local
            .date()
            .next_day()
            .and_then(|d| d.with_hms(0, 0, 0).ok())
        {
            Some(midnight) => {
                let start = midnight.assume_offset(offset);
                (start, start + Duration::hours(24))
            }
            None => (now, horizon),
        },
    };
    (from.max(now).min(horizon), to.max(now).min(horizon))
}

// =============================================================================
// Tuner contention
// =============================================================================

/// What the four tuner slots are already committed to across a window.
struct Load {
    capacity: u32,
    booked: Vec<(OffsetDateTime, OffsetDateTime, Uuid)>,
}

impl Load {
    async fn read(
        state: &AppState,
        from: OffsetDateTime,
        to: OffsetDateTime,
    ) -> Result<Self, String> {
        let capacity = series_rules::tuner_capacity(&state.pool)
            .await
            .map_err(|e| format!("could not count the tuners: {e}"))?;
        let booked = livetv_recordings::list_overlapping(&state.pool, from, to)
            .await
            .map_err(|e| format!("could not read what is already recording: {e}"))?
            .into_iter()
            .map(|r| (r.scheduled_start, r.scheduled_end, r.channel_id))
            .collect();
        Ok(Self { capacity, booked })
    }

    fn clashes(&self, start: OffsetDateTime, end: OffsetDateTime, channel_id: Uuid) -> bool {
        series_rules::concurrent_tunes(&self.booked, start, end, channel_id) >= self.capacity
    }

    fn clashes_with_padding(&self, a: &Airing) -> bool {
        self.clashes(
            a.start_time - Duration::seconds(PADDING_BEFORE_SECS as i64),
            a.end_time + Duration::seconds(PADDING_AFTER_SECS as i64),
            a.channel_id,
        )
    }
}

// =============================================================================
// Loading airings
// =============================================================================

/// A rule that constrains nothing, so [`series_rules::evaluate`] answers the
/// only question a search has: of these airings, which one would you keep, and
/// why are the rest redundant.
fn open_spec(series_id: &str) -> RuleSpec {
    RuleSpec {
        series_id: series_id.to_string(),
        channel_ids: Vec::new(),
        weekdays: Vec::new(),
        window_start: None,
        window_end: None,
        new_only: false,
        min_season: None,
        on_clash: "move".to_string(),
        padding_before_secs: PADDING_BEFORE_SECS,
        padding_after_secs: PADDING_AFTER_SECS,
    }
}

fn airing_of(guide: livetv_guide::GuideRow, channel: &livetv_channels::ChannelRow) -> Airing {
    Airing {
        guide_id: guide.id,
        channel_id: guide.channel_id,
        channel_name: classify::display_name(
            channel.display_name.as_deref(),
            channel.scan_name.as_deref(),
            channel.callsign.as_deref(),
        ),
        lcn: channel.lcn,
        hd: channel.hd,
        signal_quality: channel.signal_quality,
        channel_enabled: channel.enabled,
        start_time: guide.start_time,
        end_time: guide.end_time,
        title: guide.title,
        episode_number: guide.episode_number,
        episode_title: guide.episode_title,
        episode_key: guide.episode_key,
        episode_key_kind: guide.episode_key_kind,
        series_id: guide.series_id,
        synopsis: guide.synopsis,
        original_airdate: guide.original_airdate,
        image_url: guide.image_url.or(guide.poster_url),
    }
}

async fn channels_by_id(
    state: &AppState,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, livetv_channels::ChannelRow>, String> {
    let rows = livetv_channels::list_for_guide(&state.pool, Some(ids.to_vec()), false)
        .await
        .map_err(|e| format!("could not read the channels: {e}"))?;
    Ok(rows.into_iter().map(|c| (c.id, c)).collect())
}

/// Every airing of one guide entry's series across the whole guide window, not
/// just the slice the user asked about: a later repeat outside "tonight" is
/// exactly what a clash needs to be offered.
async fn siblings(
    state: &AppState,
    entry: &livetv_guide::GuideRow,
    now: OffsetDateTime,
) -> Result<Vec<Airing>, String> {
    let Some(series_id) = entry.series_id.as_deref() else {
        let channels = channels_by_id(state, &[entry.channel_id]).await?;
        let Some(channel) = channels.get(&entry.channel_id) else {
            return Err("that programme's channel is gone from the lineup".to_string());
        };
        return Ok(vec![airing_of(entry.clone(), channel)]);
    };
    let rows = livetv_guide::list_airings_for_series(
        &state.pool,
        series_id,
        now,
        now + series_rules::GUIDE_HORIZON,
    )
    .await
    .map_err(|e| format!("could not read the guide: {e}"))?;
    Ok(rows.into_iter().map(Airing::from).collect())
}

async fn claimed_keys(state: &AppState, airings: &[Airing]) -> Result<HashSet<String>, String> {
    let keys: Vec<String> = airings
        .iter()
        .filter_map(|a| a.episode_key.clone())
        .collect();
    let claimed = livetv_recordings::claimed_episode_keys(&state.pool, &keys)
        .await
        .map_err(|e| format!("could not check what is already recorded: {e}"))?;
    Ok(claimed.into_iter().collect())
}

// =============================================================================
// Proposals — one episode, one airing, and the count of what was dropped
// =============================================================================

/// The one airing to record, plus everything the model needs to explain the
/// eight it is not recording.
struct Proposal<'a> {
    label: String,
    winner: Option<&'a Member>,
    /// Every other airing of the same episode, simulcast or later repeat.
    suppressed: usize,
    reasons: Vec<String>,
    /// One per later cluster of the same episode: the clash fallback.
    repeats: Vec<&'a Member>,
}

fn cluster_key(cluster: &Cluster) -> String {
    cluster
        .members
        .first()
        .and_then(|m| m.airing.episode_key.clone())
        .unwrap_or_else(|| cluster.label.clone())
}

fn preferred(members: &[Member]) -> Option<&Member> {
    members
        .iter()
        .find(|m| m.airing.hd)
        .or_else(|| members.first())
}

/// Collapse the clusters of one series into one proposal per episode.
///
/// [`series_rules::evaluate`] already picked a winner per episode and marked
/// every other airing skipped with a reason; this only regroups its output so
/// that nine airings across three clusters read as *one* thing to record and a
/// count of eight, rather than three things to think about.
fn propose(clusters: &[Cluster]) -> Vec<Proposal<'_>> {
    let mut order: Vec<String> = Vec::new();
    let mut by_key: HashMap<String, Vec<&Cluster>> = HashMap::new();
    for cluster in clusters {
        let key = cluster_key(cluster);
        if !by_key.contains_key(&key) {
            order.push(key.clone());
        }
        by_key.entry(key).or_default().push(cluster);
    }

    order
        .into_iter()
        .filter_map(|key| {
            let group = by_key.remove(&key)?;
            let first = group.first()?;
            let total: usize = group.iter().map(|c| c.members.len()).sum();
            let winner = group
                .iter()
                .flat_map(|c| c.members.iter())
                .find(|m| m.action == Action::Record);
            let mut reasons: Vec<String> = Vec::new();
            for member in group.iter().flat_map(|c| c.members.iter()) {
                if member.action == Action::Skip && !reasons.contains(&member.reason) {
                    reasons.push(member.reason.clone());
                }
            }
            let repeats: Vec<&Member> = group
                .iter()
                .skip(1)
                .filter_map(|c| preferred(&c.members))
                .take(MAX_REPEATS)
                .collect();
            Some(Proposal {
                label: first.label.clone(),
                winner,
                suppressed: total - usize::from(winner.is_some()),
                reasons,
                repeats,
            })
        })
        .collect()
}

fn airing_json(a: &Airing, now: OffsetDateTime, offset: UtcOffset) -> Value {
    json!({
        "guide_id": a.guide_id,
        "channel": a.channel_name,
        "lcn": a.lcn,
        "hd": a.hd,
        "starts": spoken(a.start_time, now, offset),
        "ends": spoken(a.end_time, now, offset),
    })
}

fn proposal_json(p: &Proposal<'_>, load: &Load, now: OffsetDateTime, offset: UtcOffset) -> Value {
    let clash = p
        .winner
        .is_some_and(|m| load.clashes_with_padding(&m.airing));
    json!({
        "episode": p.label,
        "record": p.winner.map(|m| airing_json(&m.airing, now, offset)),
        "cannot_record": match p.winner {
            Some(_) => Value::Null,
            None => json!(p.reasons.first().cloned().unwrap_or_else(|| "no airing of this episode can be recorded".to_string())),
        },
        "duplicate_airings_suppressed": p.suppressed,
        "why_suppressed": p.reasons,
        "tuner_clash": clash,
        "later_repeats": p.repeats.iter().map(|m| airing_json(&m.airing, now, offset)).collect::<Vec<_>>(),
    })
}

// =============================================================================
// livetv_search_guide
// =============================================================================

/// Postgres compares `genres` exactly, and a user says "drama" where the feed
/// says "Drama".
fn normalise_genre(raw: &str) -> String {
    raw.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

struct Group {
    series_id: Option<String>,
    title: String,
    airings: Vec<Airing>,
}

pub async fn search_guide(ctx: &Ctx<'_>, args: &Value) -> ToolOutcome {
    let state = ctx.state;
    let now = OffsetDateTime::now_utc();
    let offset = server_local_offset();
    let when = When::parse(args);
    let (from, to) = bounds(when, now, offset);
    let title = arg_str(args, "title");
    let genre = arg_str(args, "genre").map(normalise_genre);

    if title.is_none() && genre.is_none() && arg_str(args, "channel").is_none() {
        return ToolOutcome::failed(
            "livetv_search_guide needs at least a title, a genre or a channel — the whole \
             48 hour guide is thousands of programmes",
        );
    }

    let channel_ids = match arg_str(args, "channel") {
        None => None,
        Some(query) => match channels_named(state, query).await {
            Err(e) => return ToolOutcome::failed(e),
            Ok(found) if found.is_empty() => {
                return ToolOutcome::ok(
                    json!({
                        "ok": true,
                        "found": false,
                        "unknown_channel": query,
                        "note": format!(
                            "There is no channel called '{query}' on this tuner. Say so and offer \
                             to search the whole guide instead of guessing which channel they meant."
                        ),
                    }),
                    format!("no channel called {query}"),
                );
            }
            Ok(found) => Some(found),
        },
    };

    let rows = match livetv_guide::list_window(
        &state.pool,
        from,
        to,
        channel_ids.as_deref(),
        title,
        genre.as_deref(),
    )
    .await
    {
        Ok(rows) => rows,
        Err(e) => return ToolOutcome::failed(format!("could not search the guide: {e}")),
    };

    if rows.is_empty() {
        return empty(state, when, title, genre.as_deref()).await;
    }

    let groups = match assemble(state, rows, now).await {
        Ok(groups) => groups,
        Err(e) => return ToolOutcome::failed(e),
    };
    let load = match Load::read(state, now, now + series_rules::GUIDE_HORIZON).await {
        Ok(load) => load,
        Err(e) => return ToolOutcome::failed(e),
    };

    let mut matches: Vec<Value> = Vec::new();
    let mut ask: Vec<&'static str> = Vec::new();
    for group in groups.iter().take(MAX_MATCHES) {
        let claimed = match claimed_keys(state, &group.airings).await {
            Ok(c) => c,
            Err(e) => return ToolOutcome::failed(e),
        };
        let spec = open_spec(group.series_id.as_deref().unwrap_or_default());
        let clusters = series_rules::evaluate(&spec, &group.airings, &claimed);
        let proposals = propose(&clusters);

        let mut channels: Vec<(String, usize, OffsetDateTime)> = Vec::new();
        for a in &group.airings {
            match channels
                .iter_mut()
                .find(|(name, _, _)| *name == a.channel_name)
            {
                Some(entry) => {
                    entry.1 += 1;
                    entry.2 = entry.2.min(a.start_time);
                }
                None => channels.push((a.channel_name.clone(), 1, a.start_time)),
            }
        }
        channels.sort_by_key(|(_, _, first)| *first);

        if channels.len() > 1 && !ask.contains(&"which_channel") {
            ask.push("which_channel");
        }
        if proposals.len() > 1 && !ask.contains(&"which_episode") {
            ask.push("which_episode");
        }
        if group.airings.len() > 1 && !ask.contains(&"one_off_or_series") {
            ask.push("one_off_or_series");
        }
        let rendered: Vec<Value> = proposals
            .iter()
            .take(MAX_EPISODES)
            .map(|p| proposal_json(p, &load, now, offset))
            .collect();
        if rendered.iter().any(|p| p["tuner_clash"] == json!(true)) && !ask.contains(&"tuner_clash")
        {
            ask.push("tuner_clash");
        }

        matches.push(json!({
            "series_id": group.series_id,
            "title": group.title,
            "total_airings": group.airings.len(),
            "on_channels": channels
                .iter()
                .take(MAX_CHANNELS)
                .map(|(name, count, first)| json!({
                    "channel": name,
                    "airings": count,
                    "first": spoken(*first, now, offset),
                }))
                .collect::<Vec<_>>(),
            "episodes": rendered,
            "more_episodes": proposals.len().saturating_sub(MAX_EPISODES),
        }));
    }

    let found = !matches.is_empty();
    let count = matches.len();
    ToolOutcome::ok(
        json!({
            "ok": true,
            "found": found,
            "window": when.name(),
            "window_local": format!("{} to {}", spoken(from, now, offset), spoken(to, now, offset)),
            "matches": matches,
            "more_matches": groups.len().saturating_sub(MAX_MATCHES),
            "ask_about": ask,
            "tuners": load.capacity,
            "note": "Each episode here is already deduplicated: `record` is the single airing to \
                     schedule and `duplicate_airings_suppressed` counts the airings of that same \
                     episode that would be wasted. Never schedule more than one airing of an \
                     episode. Ask the user about everything in `ask_about` and wait for an answer \
                     before calling livetv_schedule_recording or livetv_create_series_rule — a \
                     series on two channels, several showings, and 'this one or every week' are \
                     different requests. Use a guide_id from this result; never invent one.",
        }),
        match found {
            true => format!("{count} match(es) in the guide"),
            false => "nothing in the guide".to_string(),
        },
    )
}

/// Channels whose name, callsign or network the user's words match.
async fn channels_named(state: &AppState, query: &str) -> Result<Vec<Uuid>, String> {
    let filters = livetv_channels::ChannelFilters {
        search: Some(query.to_string()),
        enabled: Some(true),
        ..Default::default()
    };
    let (rows, _) = livetv_channels::list_filtered(&state.pool, &filters, Some("lcn"), 12, 0)
        .await
        .map_err(|e| format!("could not look that channel up: {e}"))?;
    Ok(rows.into_iter().map(|r| r.channel.id).collect())
}

/// Group the matched rows by series, then re-read each series in full.
///
/// The second read matters: a search for "the block" narrowed to tonight finds
/// two rows, but the repeat suppression is only correct over every airing of
/// that series in the window.
async fn assemble(
    state: &AppState,
    rows: Vec<livetv_guide::GuideRow>,
    now: OffsetDateTime,
) -> Result<Vec<Group>, String> {
    let channel_ids: Vec<Uuid> = rows
        .iter()
        .map(|r| r.channel_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let channels = channels_by_id(state, &channel_ids).await?;

    let mut order: Vec<String> = Vec::new();
    let mut grouped: HashMap<String, Vec<livetv_guide::GuideRow>> = HashMap::new();
    for row in rows {
        let key = match row.series_id.as_deref() {
            Some(sid) => format!("series:{sid}"),
            None => format!("title:{}", row.title.to_lowercase()),
        };
        if !grouped.contains_key(&key) {
            order.push(key.clone());
        }
        grouped.entry(key).or_default().push(row);
    }

    let mut out: Vec<Group> = Vec::new();
    for key in order {
        let Some(mut rows) = grouped.remove(&key) else {
            continue;
        };
        rows.sort_by_key(|r| (r.start_time, r.id));
        let title = rows[0].title.clone();
        let series_id = rows[0].series_id.clone();
        let airings: Vec<Airing> = match series_id.as_deref() {
            Some(sid) => livetv_guide::list_airings_for_series(
                &state.pool,
                sid,
                now,
                now + series_rules::GUIDE_HORIZON,
            )
            .await
            .map_err(|e| format!("could not read the guide: {e}"))?
            .into_iter()
            .map(Airing::from)
            .collect(),
            None => rows
                .into_iter()
                .filter_map(|r| channels.get(&r.channel_id).map(|c| airing_of(r, c)))
                .collect(),
        };
        if airings.is_empty() {
            continue;
        }
        out.push(Group {
            series_id,
            title,
            airings,
        });
    }
    out.sort_by_key(|g| g.airings.first().map(|a| a.start_time));
    Ok(out)
}

/// "Nothing is on" and "the guide does not go that far" are different answers,
/// and the second is the true one far more often.
async fn empty(
    state: &AppState,
    when: When,
    title: Option<&str>,
    genre: Option<&str>,
) -> ToolOutcome {
    let total = livetv_guide::count_all(&state.pool).await.unwrap_or(0);
    let what = title
        .map(str::to_string)
        .or_else(|| genre.map(str::to_string))
        .unwrap_or_else(|| "that".to_string());
    let note = match total {
        0 => "There is no guide data at all yet, so I cannot tell what is on. Say the listings \
              are missing — do not say nothing is on."
            .to_string(),
        _ => format!(
            "The guide has listings but nothing matching '{what}' in the {} window. The guide only \
             runs 48 hours ahead, so say what it does not cover rather than saying it is not on.",
            when.name()
        ),
    };
    ToolOutcome::ok(
        json!({
            "ok": true,
            "found": false,
            "window": when.name(),
            "guide_entries": total,
            "note": note,
        }),
        format!("nothing matching {what}"),
    )
}

// =============================================================================
// livetv_schedule_recording
// =============================================================================

struct Plan {
    entry: livetv_guide::GuideRow,
    airing: Airing,
    suppressed: usize,
    reasons: Vec<String>,
    repeats: Vec<Airing>,
    clash: bool,
    existing: Option<Uuid>,
    claimed_elsewhere: bool,
    capacity: u32,
}

impl Plan {
    fn target(&self, now: OffsetDateTime, offset: UtcOffset) -> String {
        format!(
            "{} on {} at {}",
            self.airing.title,
            self.airing.channel_name,
            spoken(self.airing.start_time, now, offset)
        )
    }
}

async fn plan(ctx: &Ctx<'_>, args: &Value, now: OffsetDateTime) -> Result<Plan, ToolOutcome> {
    let Some(guide_id) = arg_uuid(args, "guide_id") else {
        return Err(ToolOutcome::failed(
            "livetv_schedule_recording needs a guide_id from a livetv_search_guide result — call \
             that first and use the guide_id it gives you",
        ));
    };
    let entry = match livetv_guide::get_by_id(&ctx.state.pool, guide_id).await {
        Err(e) => {
            return Err(ToolOutcome::failed(format!(
                "could not read the guide: {e}"
            )));
        }
        Ok(None) => {
            return Err(ToolOutcome::failed(
                "that programme is no longer in the guide — search again and use a fresh guide_id",
            ));
        }
        Ok(Some(entry)) => entry,
    };

    let airings = siblings(ctx.state, &entry, now)
        .await
        .map_err(ToolOutcome::failed)?;
    let Some(airing) = airings.iter().find(|a| a.guide_id == entry.id).cloned() else {
        return Err(ToolOutcome::failed(
            "that programme has already dropped out of the guide window — search again",
        ));
    };

    let clusters = series_rules::cluster_airings(&airings);
    let same_key: Vec<&Airing> = airings
        .iter()
        .filter(|a| match (&a.episode_key, &airing.episode_key) {
            (Some(l), Some(r)) => l == r,
            _ => a.guide_id == airing.guide_id,
        })
        .collect();
    let repeats: Vec<Airing> = clusters
        .iter()
        .filter(|c| {
            c[0].start_time > airing.start_time
                && c.iter()
                    .any(|a| a.episode_key.is_some() && a.episode_key == airing.episode_key)
        })
        .filter_map(|c| c.iter().find(|a| a.hd).or_else(|| c.first()).cloned())
        .take(MAX_REPEATS)
        .collect();

    let mut reasons: Vec<String> = Vec::new();
    if same_key.iter().any(|a| {
        (a.start_time - airing.start_time).abs() <= series_rules::CLUSTER_TOLERANCE
            && a.guide_id != airing.guide_id
    }) {
        reasons.push("simultaneous duplicate of the airing being recorded".to_string());
    }
    if !repeats.is_empty() {
        reasons.push("later repeat of the same episode".to_string());
    }

    let existing = livetv_recordings::get_by_guide_id(&ctx.state.pool, entry.id)
        .await
        .map_err(|e| ToolOutcome::failed(format!("could not check the recordings: {e}")))?
        .filter(|r| r.status != "cancelled" && r.status != "moved")
        .map(|r| r.id);
    let claimed = claimed_keys(ctx.state, std::slice::from_ref(&airing))
        .await
        .map_err(ToolOutcome::failed)?;
    let claimed_elsewhere = airing
        .episode_key
        .as_ref()
        .is_some_and(|k| claimed.contains(k));

    let load = Load::read(ctx.state, now, now + series_rules::GUIDE_HORIZON)
        .await
        .map_err(ToolOutcome::failed)?;

    Ok(Plan {
        clash: load.clashes_with_padding(&airing),
        capacity: load.capacity,
        entry,
        suppressed: same_key.len().saturating_sub(1),
        reasons,
        repeats,
        airing,
        existing,
        claimed_elsewhere,
    })
}

/// Decision 6.6, said out loud: the guide usually airs it again, so a full
/// tuner bank is an alternative to offer, never a silent failure.
fn clash_outcome(p: &Plan, now: OffsetDateTime, offset: UtcOffset) -> ToolOutcome {
    let alternative = p.repeats.first();
    let note = match alternative {
        Some(alt) => format!(
            "All {} tuners are already committed for that slot, so nothing has been scheduled. \
             The same episode airs again on {} at {} and that slot is free — offer that instead, \
             and call this again with its guide_id if they agree.",
            p.capacity,
            alt.channel_name,
            spoken(alt.start_time, now, offset)
        ),
        None => format!(
            "All {} tuners are already committed for that slot and the guide has no later airing \
             of this episode, so nothing has been scheduled. Say that plainly and offer to cancel \
             one of the other recordings instead.",
            p.capacity
        ),
    };
    ToolOutcome::ok(
        json!({
            "ok": true,
            "scheduled": false,
            "tuner_clash": true,
            "tuners": p.capacity,
            "wanted": airing_json(&p.airing, now, offset),
            "alternative": alternative.map(|a| airing_json(a, now, offset)),
            "note": note,
        }),
        format!("tuner clash for {}", p.airing.title),
    )
}

pub async fn schedule_recording(ctx: &Ctx<'_>, args: &Value) -> ToolOutcome {
    let now = OffsetDateTime::now_utc();
    let offset = server_local_offset();

    let proposed = match plan(ctx, args, now).await {
        Ok(p) => p,
        Err(outcome) => return outcome,
    };
    if proposed.airing.end_time <= now {
        return ToolOutcome::ok(
            json!({
                "ok": true,
                "scheduled": false,
                "already_over": true,
                "note": format!(
                    "'{}' finished at {}. Broadcast TV cannot be recorded after the fact — say so, \
                     and offer the next showing if the guide has one.",
                    proposed.airing.title,
                    spoken(proposed.airing.end_time, now, offset)
                ),
                "later_repeats": proposed.repeats.iter().map(|a| airing_json(a, now, offset)).collect::<Vec<_>>(),
            }),
            format!("{} already finished", proposed.airing.title),
        );
    }
    if proposed.clash {
        return clash_outcome(&proposed, now, offset);
    }

    let target = proposed.target(now, offset);
    let confirmed = gate(ctx, "record", &target, args);
    // Re-plan from the merged arguments rather than whatever the confirming call
    // carried: a "yes" that dropped guide_id must still record what was offered,
    // and the tuner bank may have filled up while she was answering.
    let plan = match plan(ctx, &confirmed, now).await {
        Ok(p) => p,
        Err(outcome) => return outcome,
    };
    if plan.clash {
        return clash_outcome(&plan, now, offset);
    }

    if let Some(id) = plan.existing {
        return ToolOutcome::ok(
            json!({
                "ok": true,
                "scheduled": true,
                "already_scheduled": true,
                "recording_id": id,
                "say": format!(
                    "That one's already set to record — {} on {} at {}.",
                    plan.airing.title,
                    plan.airing.channel_name,
                    spoken(plan.airing.start_time, now, offset)
                ),
            }),
            format!("{} already scheduled", plan.airing.title),
        );
    }
    if plan.claimed_elsewhere {
        return ToolOutcome::ok(
            json!({
                "ok": true,
                "scheduled": false,
                "already_have_this_episode": true,
                "episode": plan.airing.episode_number,
                "say": format!(
                    "This episode of {} is already recorded or scheduled from another airing, so I \
                     haven't set a second copy going.",
                    plan.airing.title
                ),
                "note": "Nothing was scheduled. Say this, and only record it again if they \
                         explicitly want a duplicate.",
            }),
            format!("{} already claimed", plan.airing.title),
        );
    }

    let created = livetv_recordings::create(
        &ctx.state.pool,
        livetv_recordings::CreateRecording {
            channel_id: plan.entry.channel_id,
            guide_id: Some(plan.entry.id),
            series_rule_id: None,
            title: plan.entry.title.clone(),
            episode_number: plan.entry.episode_number.clone(),
            episode_title: plan.entry.episode_title.clone(),
            episode_key: plan.entry.episode_key.clone(),
            series_id: plan.entry.series_id.clone(),
            synopsis: plan.entry.synopsis.clone(),
            scheduled_start: plan.entry.start_time,
            scheduled_end: plan.entry.end_time,
            actual_start: None,
            status: "scheduled".to_string(),
            output_dir: None,
            container: Some("hls".to_string()),
            video_codec: None,
            audio_codec: None,
            padding_before_secs: PADDING_BEFORE_SECS,
            padding_after_secs: PADDING_AFTER_SECS,
            image_url: plan
                .entry
                .image_url
                .clone()
                .or_else(|| plan.entry.poster_url.clone()),
        },
    )
    .await;
    let row = match created {
        Ok(row) => row,
        Err(e) => return ToolOutcome::failed(format!("the recording could not be set up: {e}")),
    };
    let _ = ctx
        .state
        .library_events
        .send(LibraryEvent::LivetvRecordingUpdated {
            recording_id: row.id,
            status: row.status.clone(),
        });

    let starts = spoken(row.scheduled_start, now, offset);
    let dropped = match plan.suppressed {
        0 => String::new(),
        1 => " I've skipped the one duplicate airing.".to_string(),
        n => format!(" I've skipped the other {n} airings of the same episode."),
    };
    let say = format!(
        "That's set — {} on {} at {}.{dropped}",
        row.title, plan.airing.channel_name, starts
    );
    ToolOutcome::ok(
        json!({
            "ok": true,
            "scheduled": true,
            "recording_id": row.id,
            "title": row.title,
            "channel": plan.airing.channel_name,
            "starts": starts,
            "ends": spoken(row.scheduled_end, now, offset),
            "duplicate_airings_suppressed": plan.suppressed,
            "why_suppressed": plan.reasons,
            "say": say,
            "note": "Say this. Exactly one airing was scheduled; the duplicates were deliberately \
                     left alone. There is nothing to wait for.",
        }),
        format!("recording {} at {starts}", row.title),
    )
}

// =============================================================================
// livetv_create_series_rule
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    ThisChannelAndSlot,
    ThisChannel,
    AnyChannel,
}

impl Scope {
    fn parse(args: &Value) -> Option<Self> {
        match arg_str(args, "scope")
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("this_channel_and_slot" | "this_slot") => Some(Scope::ThisChannelAndSlot),
            Some("this_channel") => Some(Scope::ThisChannel),
            Some("any_channel" | "anywhere") => Some(Scope::AnyChannel),
            _ => None,
        }
    }

    fn describe(self, channel: &str) -> String {
        match self {
            Scope::ThisChannelAndSlot => format!("on {channel}, in this timeslot"),
            Scope::ThisChannel => format!("on {channel}"),
            Scope::AnyChannel => "on any channel".to_string(),
        }
    }
}

fn spec_for(
    scope: Scope,
    series_id: &str,
    a: &Airing,
    offset: UtcOffset,
    new_only: bool,
) -> RuleSpec {
    let local = a.start_time.to_offset(offset);
    let (channel_ids, weekdays, window) = match scope {
        Scope::AnyChannel => (Vec::new(), Vec::new(), None),
        Scope::ThisChannel => (vec![a.channel_id], Vec::new(), None),
        Scope::ThisChannelAndSlot => (
            vec![a.channel_id],
            vec![local.weekday().number_days_from_sunday() as i16],
            Some((local.time() - SLOT_TOLERANCE, local.time() + SLOT_TOLERANCE)),
        ),
    };
    RuleSpec {
        series_id: series_id.to_string(),
        channel_ids,
        weekdays,
        window_start: window.map(|(from, _)| from),
        window_end: window.map(|(_, to)| to),
        new_only,
        min_season: None,
        on_clash: "move".to_string(),
        padding_before_secs: PADDING_BEFORE_SECS,
        padding_after_secs: PADDING_AFTER_SECS,
    }
}

fn format_clock(t: Option<Time>) -> Option<String> {
    t.map(|t| format!("{:02}:{:02}", t.hour(), t.minute()))
}

pub async fn create_series_rule(ctx: &Ctx<'_>, args: &Value) -> ToolOutcome {
    let now = OffsetDateTime::now_utc();
    let offset = server_local_offset();

    let Some(guide_id) = arg_uuid(args, "guide_id") else {
        return ToolOutcome::failed(
            "livetv_create_series_rule needs a guide_id from a livetv_search_guide result — one \
             airing of the series the rule is about",
        );
    };
    let entry = match livetv_guide::get_by_id(&ctx.state.pool, guide_id).await {
        Err(e) => return ToolOutcome::failed(format!("could not read the guide: {e}")),
        Ok(None) => {
            return ToolOutcome::failed(
                "that programme is no longer in the guide — search again and use a fresh guide_id",
            );
        }
        Ok(Some(entry)) => entry,
    };
    let Some(series_id) = entry.series_id.clone() else {
        return ToolOutcome::ok(
            json!({
                "ok": true,
                "created": false,
                "no_series_identity": true,
                "note": format!(
                    "'{}' has no series id in the guide, so there is nothing for a standing rule to \
                     follow. Offer to record this one showing with livetv_schedule_recording instead.",
                    entry.title
                ),
            }),
            format!("{} has no series identity", entry.title),
        );
    };

    let airings = match siblings(ctx.state, &entry, now).await {
        Ok(a) => a,
        Err(e) => return ToolOutcome::failed(e),
    };
    let Some(airing) = airings.iter().find(|a| a.guide_id == entry.id).cloned() else {
        return ToolOutcome::failed(
            "that airing has dropped out of the guide window — search again and use a fresh guide_id",
        );
    };

    let mut channels: Vec<&Airing> = Vec::new();
    for a in &airings {
        if !channels.iter().any(|c| c.channel_id == a.channel_id) {
            channels.push(a);
        }
    }

    // Decision 6.5: the same SeriesID covers a network's first run and a sister
    // channel's back catalogue, so "which channel" is never ours to assume.
    let scope = match Scope::parse(args) {
        Some(scope) => scope,
        None if channels.len() > 1 => {
            return ToolOutcome::ok(
                json!({
                    "ok": true,
                    "created": false,
                    "ambiguous": true,
                    "ask": "which_channel",
                    "title": entry.title,
                    "channels": channels
                        .iter()
                        .take(MAX_CHANNELS)
                        .map(|a| json!({
                            "channel": a.channel_name,
                            "example_airing": spoken(a.start_time, now, offset),
                            "hd": a.hd,
                        }))
                        .collect::<Vec<_>>(),
                    "note": "This series airs on more than one channel in the guide, and the same \
                             series id covers a first run and another channel's back catalogue. Ask \
                             which channel they mean, and whether the rule should be pinned to this \
                             weekday and timeslot, then call this again with scope. Nothing has been \
                             created.",
                }),
                format!("{} airs on {} channels", entry.title, channels.len()),
            );
        }
        None => Scope::ThisChannel,
    };
    // Decision 17: new episodes only is the default for a standing rule.
    let new_only = arg_bool(args, "new_only").unwrap_or(true);
    let spec = spec_for(scope, &series_id, &airing, offset, new_only);

    let claimed = match claimed_keys(ctx.state, &airings).await {
        Ok(c) => c,
        Err(e) => return ToolOutcome::failed(e),
    };
    let clusters = series_rules::evaluate(&spec, &airings, &claimed);
    let would_record = clusters
        .iter()
        .flat_map(|c| c.members.iter())
        .filter(|m| m.action == Action::Record)
        .count();
    let suppressed = clusters
        .iter()
        .flat_map(|c| c.members.iter())
        .filter(|m| m.action == Action::Skip)
        .count();

    let target = format!(
        "{} {} — {} in the next 48 hours, skipping {suppressed} duplicate airing(s)",
        match new_only {
            true => format!("every new episode of {}", entry.title),
            false => format!("every episode of {}", entry.title),
        },
        scope.describe(&airing.channel_name),
        match would_record {
            1 => "1 episode".to_string(),
            n => format!("{n} episodes"),
        },
    );
    let confirmed = gate(ctx, "set up a series recording for", &target, args);
    let scope = Scope::parse(&confirmed).unwrap_or(scope);
    let new_only = arg_bool(&confirmed, "new_only").unwrap_or(new_only);
    let spec = spec_for(scope, &series_id, &airing, offset, new_only);

    let created = livetv_series_rules::create(
        &ctx.state.pool,
        livetv_series_rules::CreateSeriesRule {
            series_id: series_id.clone(),
            title: entry.title.clone(),
            enabled: true,
            channel_ids: spec.channel_ids.clone(),
            weekdays: spec.weekdays.clone(),
            window_start: spec.window_start,
            window_end: spec.window_end,
            new_only,
            min_season: None,
            keep_count: None,
            keep_until: "space".to_string(),
            keep_until_date: None,
            padding_before_secs: PADDING_BEFORE_SECS,
            padding_after_secs: PADDING_AFTER_SECS,
            on_clash: "move".to_string(),
            priority: 0,
        },
    )
    .await;
    let rule = match created {
        Ok(rule) => rule,
        Err(e) => return ToolOutcome::failed(format!("the series rule could not be set up: {e}")),
    };

    // A new rule cannot wait for the next guide refresh six hours away.
    if let Err(e) = series_rules::expand_all(&ctx.state.pool).await {
        tracing::warn!(error = %e, "expanding series rules after a chat create failed");
    }
    let queued = livetv_recordings::queued_counts_by_rule(&ctx.state.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|(id, _)| *id == rule.id)
        .map(|(_, n)| n)
        .unwrap_or(0);

    ToolOutcome::ok(
        json!({
            "ok": true,
            "created": true,
            "rule_id": rule.id,
            "title": rule.title,
            "scope": scope.describe(&airing.channel_name),
            "channel": match scope {
                Scope::AnyChannel => Value::Null,
                _ => json!(airing.channel_name),
            },
            "weekdays": rule.weekdays,
            "window_start": format_clock(rule.window_start),
            "window_end": format_clock(rule.window_end),
            "new_only": rule.new_only,
            "queued_now": queued,
            "duplicate_airings_suppressed": suppressed,
            "say": format!(
                "Done — I'll record {} {}, and I've queued {queued} of them from the next 48 hours.",
                match new_only {
                    true => format!("every new episode of {}", rule.title),
                    false => format!("every episode of {}", rule.title),
                },
                scope.describe(&airing.channel_name),
            ),
            "note": "Say this. The rule re-runs on every guide refresh, so episodes beyond the 48 \
                     hour guide are queued as the listings arrive.",
        }),
        format!("series rule for {}", rule.title),
    )
}

// =============================================================================
// livetv_list_recordings
// =============================================================================

fn status_filter(args: &Value) -> Option<&'static str> {
    match arg_str(args, "status")
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("recording" | "now" | "in_progress") => Some("recording"),
        Some("scheduled" | "queued" | "upcoming") => Some("scheduled"),
        Some("done" | "completed" | "finished") => Some("completed"),
        Some("failed" | "broken") => Some("failed"),
        _ => None,
    }
}

pub async fn list_recordings(ctx: &Ctx<'_>, args: &Value) -> ToolOutcome {
    let now = OffsetDateTime::now_utc();
    let offset = server_local_offset();
    let wanted = arg_limit(args, DEFAULT_RECORDINGS, MAX_RECORDINGS);
    let status = status_filter(args);

    let filters = livetv_recordings::RecordingFilters {
        status: status.map(str::to_string),
        ..Default::default()
    };
    let listed =
        livetv_recordings::list_filtered(&ctx.state.pool, &filters, MAX_RECORDINGS as i64 * 2, 0)
            .await;
    let (rows, total) = match listed {
        Ok(v) => v,
        Err(e) => return ToolOutcome::failed(format!("could not read the recordings: {e}")),
    };
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|r| status.is_some() || !matches!(r.status.as_str(), "cancelled" | "moved"))
        .take(wanted)
        .collect();

    let channel_ids: Vec<Uuid> = rows.iter().map(|r| r.channel_id).collect();
    let labels: HashMap<Uuid, String> = livetv_channels::labels_for(&ctx.state.pool, &channel_ids)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(id, name, _)| (id, name))
        .collect();

    let items: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "recording_id": r.id,
                "title": r.title,
                "episode": r.episode_number,
                "channel": labels.get(&r.channel_id).cloned().unwrap_or_else(|| "unknown channel".to_string()),
                "status": r.status,
                "starts": spoken(r.scheduled_start, now, offset),
                "ends": spoken(r.scheduled_end, now, offset),
                "from_series_rule": r.series_rule_id.is_some(),
            })
        })
        .collect();

    let rules = livetv_series_rules::list_all(&ctx.state.pool, false)
        .await
        .unwrap_or_default();
    let queued: HashMap<Uuid, i64> = livetv_recordings::queued_counts_by_rule(&ctx.state.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
    let rules: Vec<Value> = rules
        .iter()
        .map(|r| {
            json!({
                "rule_id": r.id,
                "title": r.title,
                "enabled": r.enabled,
                "new_only": r.new_only,
                "queued": queued.get(&r.id).copied().unwrap_or(0),
            })
        })
        .collect();

    let showing = items.len();
    ToolOutcome::ok(
        json!({
            "ok": true,
            "found": showing > 0 || !rules.is_empty(),
            "status": status.unwrap_or("any"),
            "showing": showing,
            "total": total,
            "recordings": items,
            "series_rules": rules,
            "note": "recording_id and rule_id here are the only ids livetv_cancel accepts. A \
                     recording marked 'recording' is being captured right now and is already \
                     watchable from the start.",
        }),
        format!("{showing} recordings"),
    )
}

// =============================================================================
// livetv_cancel
// =============================================================================

pub async fn cancel(ctx: &Ctx<'_>, args: &Value) -> ToolOutcome {
    let recording_id = arg_uuid(args, "recording_id");
    let rule_id = arg_uuid(args, "rule_id");

    if let Some(id) = recording_id {
        return cancel_recording(ctx, id).await;
    }
    if let Some(id) = rule_id {
        return disable_rule(ctx, id).await;
    }
    ToolOutcome::failed(
        "livetv_cancel needs a recording_id or a rule_id from a livetv_list_recordings result — \
         call that first and use an id it gives you",
    )
}

async fn cancel_recording(ctx: &Ctx<'_>, id: Uuid) -> ToolOutcome {
    let row = match livetv_recordings::get_by_id(&ctx.state.pool, id).await {
        Err(e) => return ToolOutcome::failed(format!("could not read that recording: {e}")),
        Ok(None) => {
            return ToolOutcome::failed(
                "there is no recording with that id — call livetv_list_recordings and use an id \
                 from it",
            );
        }
        Ok(Some(row)) => row,
    };
    // Deleting a captured programme is a different, irreversible act, and it is
    // not one that happens because a sentence was misheard.
    if !matches!(row.status.as_str(), "scheduled" | "recording") {
        return ToolOutcome::ok(
            json!({
                "ok": true,
                "cancelled": false,
                "status": row.status,
                "say": format!(
                    "'{}' is already {} — there's nothing queued to cancel. Deleting a finished \
                     recording isn't something I do from here.",
                    row.title, row.status
                ),
            }),
            format!("{} is {}", row.title, row.status),
        );
    }

    let was_recording = row.status == "recording";
    match livetv_recordings::cancel(&ctx.state.pool, row.id).await {
        Err(e) => return ToolOutcome::failed(format!("that recording could not be stopped: {e}")),
        Ok(false) => {
            return ToolOutcome::ok(
                json!({
                    "ok": true,
                    "cancelled": false,
                    "say": format!("'{}' had already finished by the time I got to it.", row.title),
                }),
                format!("{} already finished", row.title),
            );
        }
        Ok(true) => {}
    }
    let _ = ctx
        .state
        .library_events
        .send(LibraryEvent::LivetvRecordingUpdated {
            recording_id: row.id,
            status: match was_recording {
                true => "completed".to_string(),
                false => "cancelled".to_string(),
            },
        });

    ToolOutcome::ok(
        json!({
            "ok": true,
            "cancelled": true,
            "recording_id": row.id,
            "title": row.title,
            "was_in_progress": was_recording,
            "say": match was_recording {
                true => format!("Stopped recording {} — what it captured so far is kept.", row.title),
                false => format!("Dropped the recording of {}.", row.title),
            },
        }),
        format!("cancelled {}", row.title),
    )
}

async fn disable_rule(ctx: &Ctx<'_>, id: Uuid) -> ToolOutcome {
    let updated = livetv_series_rules::update(
        &ctx.state.pool,
        id,
        livetv_series_rules::UpdateSeriesRule {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .await;
    let rule = match updated {
        Err(e) => return ToolOutcome::failed(format!("that rule could not be turned off: {e}")),
        Ok(None) => {
            return ToolOutcome::failed(
                "there is no series rule with that id — call livetv_list_recordings and use a \
                 rule_id from it",
            );
        }
        Ok(Some(rule)) => rule,
    };
    let queued = livetv_recordings::queued_counts_by_rule(&ctx.state.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|(rid, _)| *rid == rule.id)
        .map(|(_, n)| n)
        .unwrap_or(0);

    ToolOutcome::ok(
        json!({
            "ok": true,
            "disabled": true,
            "rule_id": rule.id,
            "title": rule.title,
            "still_queued": queued,
            "say": match queued {
                0 => format!("Turned off the series recording for {}.", rule.title),
                1 => format!(
                    "Turned off the series recording for {} — one episode is still queued from \
                     before, say the word and I'll drop that too.",
                    rule.title
                ),
                n => format!(
                    "Turned off the series recording for {} — {n} episodes are still queued from \
                     before, say the word and I'll drop those too.",
                    rule.title
                ),
            },
            "note": "The rule is off, so nothing new will be queued. Already-queued episodes are \
                     untouched; cancel them with livetv_cancel and their recording_id if she wants.",
        }),
        format!("disabled the rule for {}", rule.title),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_when_enum_is_the_only_date_arithmetic_anyone_does() {
        assert_eq!(When::parse(&json!({})), When::Anytime);
        assert_eq!(When::parse(&json!({"when": "Tonight"})), When::Tonight);
        assert_eq!(When::parse(&json!({"when": "tomorrow"})), When::Tomorrow);
        assert_eq!(
            When::parse(&json!({"when": "next_few_hours"})),
            When::NextFewHours
        );
        assert_eq!(When::parse(&json!({"when": "gibberish"})), When::Anytime);
    }

    /// The feed only runs 48 h ahead, so no window may ask for more than that.
    #[test]
    fn every_window_is_clamped_to_the_guides_own_ceiling() {
        let now = OffsetDateTime::now_utc();
        let offset = server_local_offset();
        for when in [
            When::NextFewHours,
            When::Tonight,
            When::Tomorrow,
            When::Anytime,
        ] {
            let (from, to) = bounds(when, now, offset);
            assert!(from >= now, "{when:?} began in the past");
            assert!(
                to <= now + series_rules::GUIDE_HORIZON,
                "{when:?} ran past the guide"
            );
            assert!(to >= from, "{when:?} is inverted");
        }
    }

    #[test]
    fn a_genre_is_matched_the_way_the_feed_spells_it() {
        assert_eq!(normalise_genre("drama"), "Drama");
        assert_eq!(normalise_genre("REALITY show"), "Reality Show");
        assert_eq!(normalise_genre("News"), "News");
    }

    #[test]
    fn a_scope_the_model_did_not_send_is_not_guessed() {
        assert_eq!(Scope::parse(&json!({})), None);
        assert_eq!(
            Scope::parse(&json!({"scope": "this_channel"})),
            Some(Scope::ThisChannel)
        );
        assert_eq!(
            Scope::parse(&json!({"scope": "ANY_CHANNEL"})),
            Some(Scope::AnyChannel)
        );
    }

    #[test]
    fn a_recording_and_a_viewer_do_not_contend_on_the_same_channel() {
        let now = OffsetDateTime::now_utc();
        let channel = Uuid::now_v7();
        let other = Uuid::now_v7();
        let load = Load {
            capacity: 1,
            booked: vec![(now, now + Duration::hours(1), channel)],
        };
        assert!(!load.clashes(now, now + Duration::minutes(30), channel));
        assert!(load.clashes(now, now + Duration::minutes(30), other));
        assert!(!load.clashes(now + Duration::hours(2), now + Duration::hours(3), other));
    }
}
