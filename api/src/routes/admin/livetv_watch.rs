//! Watch history surfaces (Decision 14).
//!
//! Not analytics for its own sake: the whole point is "this is on right now and
//! you always watch it", which is why the clash notice lives here rather than
//! with the leases.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::AppState;
use crate::livetv::{affinity, series_rules};
use crate::repositories::{
    livetv_affinity, livetv_channels, livetv_guide, livetv_recordings, livetv_tuners,
    livetv_watch_events,
};
use crate::routes::AuthUser;

use super::livetv::{
    AppError, StackedQuery, channel_response, guide_entry_response, now_next_map, stacks_for,
};

// =============================================================================
// POST /watch-events
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct WatchEventInput {
    pub channel_id: Uuid,
    #[serde(default)]
    pub guide_id: Option<Uuid>,
    pub started_at: String,
    #[serde(default)]
    pub ended_at: Option<String>,
    pub watched_secs: i32,
    #[serde(default)]
    pub programme_secs: Option<i32>,
    pub source: String,
}

fn parse_time(raw: &str) -> Result<OffsetDateTime, AppError> {
    OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339)
        .map_err(|_| AppError::BadRequest(format!("'{raw}' is not an RFC3339 timestamp")))
}

pub async fn create_watch_event(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<WatchEventInput>,
) -> Result<impl IntoResponse, AppError> {
    if !matches!(input.source.as_str(), "live" | "buffer" | "recording") {
        return Err(AppError::BadRequest(
            "source must be live, buffer or recording".to_string(),
        ));
    }
    if input.watched_secs < 0 {
        return Err(AppError::BadRequest(
            "watched_secs cannot be negative".to_string(),
        ));
    }
    if livetv_channels::get_by_id(&state.pool, input.channel_id)
        .await?
        .is_none()
    {
        return Err(AppError::NotFound("Channel not found".to_string()));
    }
    let started_at = parse_time(&input.started_at)?;
    let ended_at = match input.ended_at.as_deref() {
        Some(raw) => Some(parse_time(raw)?),
        None => None,
    };

    let entry = match input.guide_id {
        Some(id) => livetv_guide::get_by_id(&state.pool, id).await?,
        None => None,
    };

    livetv_watch_events::create(
        &state.pool,
        livetv_watch_events::CreateWatchEvent {
            user_id: Some(user.user_id),
            channel_id: input.channel_id,
            guide_id: entry.as_ref().map(|e| e.id),
            series_id: entry.as_ref().and_then(|e| e.series_id.clone()),
            title: entry.as_ref().map(|e| e.title.clone()),
            episode_key: entry.as_ref().and_then(|e| e.episode_key.clone()),
            started_at,
            ended_at,
            watched_secs: input.watched_secs,
            programme_secs: input.programme_secs.or_else(|| {
                entry
                    .as_ref()
                    .map(|e| (e.end_time - e.start_time).whole_seconds() as i32)
            }),
            source: input.source,
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// GET /on-now
// =============================================================================

#[derive(Debug, Serialize)]
pub struct OnNowItem {
    channel: serde_json::Value,
    guide: serde_json::Value,
    minutes_left: Option<i64>,
    starts_in_minutes: Option<i64>,
    reason: String,
    /// Null when there is not enough history — the UI hides the dots entirely
    /// rather than rendering a fabricated value.
    confidence: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct OnNowGroup {
    key: &'static str,
    title: String,
    subtitle: String,
    items: Vec<OnNowItem>,
}

pub async fn on_now(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<StackedQuery>,
) -> Result<impl IntoResponse, AppError> {
    let now = OffsetDateTime::now_utc();
    let stacks = stacks_for(&state, q.stacked).await?;
    let mut channels = livetv_channels::list_for_guide(&state.pool, None, true).await?;
    channels.retain(|c| !stacks.is_hidden(c.id));
    if channels.is_empty() {
        return Ok(Json(json!({
            "generated_at": now.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
            "tuner_notice": serde_json::Value::Null,
            "groups": Vec::<OnNowGroup>::new(),
        })));
    }

    let channel_ids: Vec<Uuid> = channels.iter().map(|c| c.id).collect();
    let map = now_next_map(&state, &channel_ids, now).await?;
    let live: Vec<(&livetv_channels::ChannelRow, livetv_guide::GuideRow)> = channels
        .iter()
        .filter_map(|c| {
            map.get(&c.id)
                .and_then(|(current, _)| current.clone())
                .map(|entry| (c, entry))
        })
        .collect();
    let soon: Vec<(&livetv_channels::ChannelRow, livetv_guide::GuideRow)> = channels
        .iter()
        .filter_map(|c| {
            map.get(&c.id)
                .and_then(|(_, next)| next.clone())
                .filter(|entry| entry.start_time <= now + USUAL_LOOKAHEAD)
                .map(|entry| (c, entry))
        })
        .collect();

    let zone = crate::notifications::prefs::zone_for_user(&state.pool, user.user_id).await;
    let local = now.to_offset(zone.offset_at(now));
    let weekday = affinity::weekday_index(local.weekday());
    let hour = local.hour() as i16;

    let slots = livetv_affinity::slot_totals(&state.pool, user.user_id).await?;
    let regularity = affinity::slot_regularity(
        &slots
            .iter()
            .flat_map(|(w, h, count)| std::iter::repeat_n((*w, *h), (*count).max(0) as usize))
            .collect::<Vec<_>>(),
    );
    let slot_now = slots
        .iter()
        .find(|(w, h, _)| *w == weekday && *h == hour)
        .map(|(_, _, c)| *c)
        .unwrap_or(0);

    let series_ids: Vec<String> = live
        .iter()
        .chain(soon.iter())
        .filter_map(|(_, e)| e.series_id.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let genres: Vec<String> = live
        .iter()
        .flat_map(|(_, e)| e.genres.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    let series_rows: HashMap<String, livetv_affinity::AffinityRow> =
        livetv_affinity::series_affinity(&state.pool, user.user_id, &series_ids)
            .await?
            .into_iter()
            .filter_map(|r| r.series_id.clone().map(|s| (s, r)))
            .collect();
    let channel_rows: HashMap<Uuid, livetv_affinity::AffinityRow> =
        livetv_affinity::channel_affinity(&state.pool, user.user_id, &channel_ids)
            .await?
            .into_iter()
            .filter_map(|r| r.channel_id.map(|c| (c, r)))
            .collect();
    let genre_rows: HashMap<String, livetv_affinity::AffinityRow> =
        livetv_affinity::genre_affinity(&state.pool, user.user_id, &genres)
            .await?
            .into_iter()
            .filter_map(|r| r.genre.clone().map(|g| (g, r)))
            .collect();

    let item = |channel: &livetv_channels::ChannelRow,
                entry: &livetv_guide::GuideRow,
                reason: String,
                confidence: Option<f64>| OnNowItem {
        channel: serde_json::to_value(channel_response(channel, 0).stacked(&stacks))
            .unwrap_or(json!({})),
        guide: serde_json::to_value(guide_entry_response(entry, now, 0, None)).unwrap_or(json!({})),
        minutes_left: (entry.start_time <= now)
            .then(|| (entry.end_time - now).whole_minutes().max(0)),
        starts_in_minutes: (entry.start_time > now)
            .then(|| (entry.start_time - now).whole_minutes().max(0)),
        reason,
        confidence,
    };

    let candidates: Vec<(&livetv_channels::ChannelRow, &livetv_guide::GuideRow)> = live
        .iter()
        .chain(soon.iter())
        .map(|(c, e)| (*c, e))
        .collect();
    let usual_hour = (u16::from(local.hour()) * 60 + u16::from(local.minute()) + 30) / 60 % 24;
    let usual_items: Vec<OnNowItem> =
        usual_airings(&state, user.user_id, &zone, local, now, &candidates)
            .await?
            .into_iter()
            .map(|(channel, entry, _)| {
                let confidence = entry
                    .series_id
                    .as_ref()
                    .and_then(|s| series_rows.get(s))
                    .and_then(|row| {
                        affinity::confidence(
                            row.watch_count,
                            row.mean_completion,
                            regularity,
                            row.mean_completion > 0.0,
                        )
                    });
                item(
                    channel,
                    entry,
                    format!("you usually watch this around {usual_hour:02}:00"),
                    confidence,
                )
            })
            .collect();

    let mut series_items: Vec<OnNowItem> = Vec::new();
    for (channel, entry) in &live {
        let Some(row) = entry.series_id.as_ref().and_then(|s| series_rows.get(s)) else {
            continue;
        };
        let known = row.mean_completion > 0.0;
        let confidence =
            affinity::confidence(row.watch_count, row.mean_completion, regularity, known);
        series_items.push(item(
            channel,
            entry,
            format!(
                "you have watched {} airing{} of this",
                row.watch_count,
                if row.watch_count == 1 { "" } else { "s" }
            ),
            confidence,
        ));
    }

    let mut channel_items: Vec<OnNowItem> = Vec::new();
    for (channel, entry) in &live {
        let Some(row) = channel_rows.get(&channel.id) else {
            continue;
        };
        let known = row.mean_completion > 0.0;
        channel_items.push(item(
            channel,
            entry,
            format!("you watch this channel often ({} times)", row.watch_count),
            affinity::confidence(row.watch_count, row.mean_completion, regularity, known),
        ));
    }

    let mut genre_items: Vec<OnNowItem> = Vec::new();
    for (channel, entry) in &live {
        let Some((genre, row)) = entry
            .genres
            .iter()
            .find_map(|g| genre_rows.get(g).map(|r| (g.clone(), r)))
        else {
            continue;
        };
        let known = row.mean_completion > 0.0;
        genre_items.push(item(
            channel,
            entry,
            format!("you watch a lot of {genre}"),
            affinity::confidence(row.watch_count, row.mean_completion, regularity, known),
        ));
    }

    let mut slot_items: Vec<OnNowItem> = Vec::new();
    if slot_now > 0 {
        for (channel, entry) in &live {
            let series = entry.series_id.as_ref().and_then(|s| series_rows.get(s));
            let channel_row = channel_rows.get(&channel.id);
            if series.is_none() && channel_row.is_none() {
                continue;
            }
            let row = series.or(channel_row).expect("one of the two is set");
            slot_items.push(item(
                channel,
                entry,
                "this is when you usually have something on".to_string(),
                affinity::confidence(
                    slot_now,
                    row.mean_completion,
                    regularity,
                    row.mean_completion > 0.0,
                ),
            ));
        }
    }

    for items in [
        &mut series_items,
        &mut slot_items,
        &mut channel_items,
        &mut genre_items,
    ] {
        items.sort_by(|a, b| {
            b.confidence
                .unwrap_or(0.0)
                .partial_cmp(&a.confidence.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        items.truncate(12);
    }

    let notice = clash_notice(&state, user.user_id, &live, &series_rows, now).await?;

    let groups: Vec<OnNowGroup> = [
        (
            "usual",
            "Your usual",
            "On around now — you usually watch these at this time",
            usual_items,
        ),
        (
            "series",
            "Because you watch it",
            "On now, and you keep coming back",
            series_items,
        ),
        (
            "timeofday",
            "Your usual slot",
            "What you normally have on around now",
            slot_items,
        ),
        (
            "channel",
            "Channels you watch",
            "On now on the channels you use most",
            channel_items,
        ),
        (
            "genre",
            "More of what you like",
            "On now, in genres you watch",
            genre_items,
        ),
    ]
    .into_iter()
    .filter(|(_, _, _, items)| !items.is_empty())
    .map(|(key, title, subtitle, items)| OnNowGroup {
        key,
        title: title.to_string(),
        subtitle: subtitle.to_string(),
        items,
    })
    .collect();

    Ok(Json(json!({
        "generated_at": now.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
        "tuner_notice": notice,
        "groups": groups,
    })))
}

const USUAL_LOOKAHEAD: time::Duration = time::Duration::minutes(30);
const USUAL_HISTORY: time::Duration = time::Duration::days(90);
const USUAL_WINDOW_MINUTES: u16 = 90;
const USUAL_MIN_WATCHES: usize = 2;
const USUAL_LIMIT: usize = 12;

/// Weekday is deliberately ignored: a weeknight show is watched at the same
/// time on different days, and requiring a weekday match would need weeks of
/// history before it ever fired.
async fn usual_airings<'a>(
    state: &AppState,
    user_id: Uuid,
    zone: &crate::notifications::prefs::Zone,
    local_now: OffsetDateTime,
    now: OffsetDateTime,
    candidates: &[(&'a livetv_channels::ChannelRow, &'a livetv_guide::GuideRow)],
) -> Result<
    Vec<(
        &'a livetv_channels::ChannelRow,
        &'a livetv_guide::GuideRow,
        usize,
    )>,
    AppError,
> {
    let series_ids: Vec<String> = candidates
        .iter()
        .filter_map(|(_, e)| e.series_id.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if series_ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut counts: HashMap<String, usize> = HashMap::new();
    for (series, at) in livetv_watch_events::series_watch_starts(
        &state.pool,
        user_id,
        &series_ids,
        now - USUAL_HISTORY,
    )
    .await?
    {
        let local = at.to_offset(zone.offset_at(at));
        if affinity::clock_minutes_apart(local.time(), local_now.time()) <= USUAL_WINDOW_MINUTES {
            *counts.entry(series).or_default() += 1;
        }
    }

    // Earliest start wins, which is what makes an airing that is already on
    // beat a later repeat of the same series.
    let mut best: HashMap<&str, (&livetv_channels::ChannelRow, &livetv_guide::GuideRow)> =
        HashMap::new();
    for &(channel, entry) in candidates {
        let Some(series) = entry.series_id.as_deref() else {
            continue;
        };
        if counts.get(series).is_none_or(|c| *c < USUAL_MIN_WATCHES) {
            continue;
        }
        best.entry(series)
            .and_modify(|b| {
                if entry.start_time < b.1.start_time {
                    *b = (channel, entry);
                }
            })
            .or_insert((channel, entry));
    }

    let mut out: Vec<_> = best
        .into_iter()
        .map(|(series, (channel, entry))| (channel, entry, counts[series]))
        .collect();
    out.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then(a.1.start_time.cmp(&b.1.start_time))
            .then(a.0.id.cmp(&b.0.id))
    });
    out.truncate(USUAL_LIMIT);
    Ok(out)
}

/// The notice only works because Decision 6 already computes repeat clusters:
/// watch history says the programme matters, the cluster data says when it airs
/// again, and the lease table says why now is impossible.
async fn clash_notice(
    state: &AppState,
    user_id: Uuid,
    live: &[(&livetv_channels::ChannelRow, livetv_guide::GuideRow)],
    series_rows: &HashMap<String, livetv_affinity::AffinityRow>,
    now: OffsetDateTime,
) -> Result<Option<serde_json::Value>, AppError> {
    let tuners = livetv_tuners::list_all(&state.pool).await?;
    if tuners.is_empty() {
        return Ok(None);
    }
    let free: i64 = tuners
        .iter()
        .map(|t| {
            let capacity = t.tuner_count.max(1) as i64;
            capacity - state.livetv.leases.in_use(t.id) as i64
        })
        .sum();
    if free > 0 {
        return Ok(None);
    }

    let mut best: Option<(&livetv_channels::ChannelRow, &livetv_guide::GuideRow, i32)> = None;
    for (channel, entry) in live {
        let Some(row) = entry.series_id.as_ref().and_then(|s| series_rows.get(s)) else {
            continue;
        };
        if best.is_none_or(|(_, _, count)| row.watch_count > count) {
            best = Some((channel, entry, row.watch_count));
        }
    }
    let Some((_, entry, watch_count)) = best else {
        return Ok(None);
    };
    if livetv_recordings::get_by_guide_id(&state.pool, entry.id)
        .await?
        .is_some()
    {
        return Ok(None);
    }

    let alternative = match (entry.series_id.as_deref(), entry.episode_key.as_deref()) {
        (Some(series_id), Some(key)) => {
            let airings: Vec<series_rules::Airing> = livetv_guide::list_airings_for_series(
                &state.pool,
                series_id,
                now,
                now + series_rules::GUIDE_HORIZON,
            )
            .await?
            .into_iter()
            .map(Into::into)
            .collect();
            series_rules::cluster_airings(&airings)
                .into_iter()
                .filter_map(|cluster| {
                    cluster
                        .into_iter()
                        .find(|a| a.episode_key.as_deref() == Some(key) && a.guide_id != entry.id)
                })
                .filter(|a| a.start_time > entry.start_time)
                .min_by_key(|a| a.start_time)
        }
        _ => None,
    };

    let alternative_entry = match alternative {
        Some(a) => livetv_guide::get_by_id(&state.pool, a.guide_id).await?,
        None => None,
    };

    // Decision 14's sentence, and the reason it is stated as a ratio: the point
    // is that this one matters, not that a counter went up.
    let recent = livetv_watch_events::recent_series_completions(
        &state.pool,
        user_id,
        entry.series_id.as_deref().unwrap_or_default(),
        16,
    )
    .await?;
    let finished = recent.iter().filter(|(c, _)| *c >= 0.8).count();
    let evidence = if recent.is_empty() {
        format!(
            "you have watched {watch_count} airing{} of this",
            if watch_count == 1 { "" } else { "s" }
        )
    } else {
        format!("you have watched {finished} of the last {}", recent.len())
    };

    Ok(Some(json!({
        "guide": guide_entry_response(entry, now, 0, None),
        "reason": format!("every tuner is busy and {evidence}"),
        "alternative": alternative_entry
            .as_ref()
            .map(|e| guide_entry_response(e, now, 0, None)),
    })))
}

// =============================================================================
// Ad breaks (Phase 10)
// =============================================================================

#[derive(Debug, Serialize)]
pub struct AdBreakResponse {
    pub id: Uuid,
    pub recording_id: Uuid,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub method: String,
    pub confidence: f64,
    pub ads_known: i32,
    pub ads_total: i32,
    #[serde(with = "time::serde::rfc3339::option")]
    pub corrected_at: Option<OffsetDateTime>,
}

pub fn ad_break_response(
    row: crate::repositories::livetv_ad_breaks::AdBreakRow,
) -> AdBreakResponse {
    AdBreakResponse {
        id: row.id,
        recording_id: row.recording_id,
        start_seconds: row.start_seconds,
        end_seconds: row.end_seconds,
        method: row.method,
        confidence: row.confidence,
        ads_known: row.ads_known,
        ads_total: row.ads_total,
        corrected_at: row.corrected_at,
    }
}

pub async fn list_ad_breaks(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    super::livetv::require_contributor(&user)?;
    if livetv_recordings::get_by_id(&state.pool, id)
        .await?
        .is_none()
    {
        return Err(AppError::NotFound("Recording not found".to_string()));
    }
    let items: Vec<AdBreakResponse> =
        crate::repositories::livetv_ad_breaks::list_for_recording(&state.pool, id)
            .await?
            .into_iter()
            .map(ad_break_response)
            .collect();
    Ok(Json(json!({ "items": items })))
}

#[derive(Debug, Deserialize)]
pub struct CorrectAdBreakInput {
    pub start_seconds: f64,
    pub end_seconds: f64,
}

pub async fn correct_ad_break(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, break_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<CorrectAdBreakInput>,
) -> Result<impl IntoResponse, AppError> {
    super::livetv::require_contributor(&user)?;
    if !input.start_seconds.is_finite()
        || !input.end_seconds.is_finite()
        || input.start_seconds < 0.0
        || input.end_seconds <= input.start_seconds
    {
        return Err(AppError::BadRequest(
            "end_seconds must be after start_seconds".to_string(),
        ));
    }
    let recording = livetv_recordings::get_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Recording not found".to_string()))?;
    let existing = crate::repositories::livetv_ad_breaks::get_by_id(&state.pool, break_id)
        .await?
        .filter(|b| b.recording_id == id)
        .ok_or_else(|| AppError::NotFound("Ad break not found".to_string()))?;

    let row = crate::repositories::livetv_ad_breaks::correct(
        &state.pool,
        existing.id,
        input.start_seconds,
        input.end_seconds,
        Some(user.user_id),
    )
    .await?
    .ok_or_else(|| AppError::NotFound("Ad break not found".to_string()))?;

    // The correction loop is the compounding asset: one fix lands every future
    // airing of the same ad, so the corrected span is re-fingerprinted here
    // rather than waiting for the next detection pass.
    let runtime = state.runtime_config.get().await;
    if runtime.livetv_ad_train_from_corrections {
        let _ = state
            .nats
            .publisher()
            .enqueue_job(
                "kobbler",
                "livetv_ad_detect",
                json!({
                    "recording_id": recording.id,
                    "reprint_break_id": row.id,
                    "detect": false,
                }),
            )
            .await;
    }

    Ok(Json(ad_break_response(row)))
}

pub async fn delete_ad_break(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, break_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, AppError> {
    super::livetv::require_contributor(&user)?;
    let existing = crate::repositories::livetv_ad_breaks::get_by_id(&state.pool, break_id)
        .await?
        .filter(|b| b.recording_id == id)
        .ok_or_else(|| AppError::NotFound("Ad break not found".to_string()))?;
    crate::repositories::livetv_ad_breaks::delete(&state.pool, existing.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// EDL is what every third-party player already understands; `3` is "commercial
/// break", which players skip or shade according to their own setting.
pub async fn ad_breaks_edl(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<axum::response::Response, AppError> {
    super::livetv::require_contributor(&user)?;
    if livetv_recordings::get_by_id(&state.pool, id)
        .await?
        .is_none()
    {
        return Err(AppError::NotFound("Recording not found".to_string()));
    }
    let body = crate::repositories::livetv_ad_breaks::list_for_recording(&state.pool, id)
        .await?
        .into_iter()
        .map(|b| format!("{:.3}\t{:.3}\t3", b.start_seconds, b.end_seconds))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )
        .body(axum::body::Body::from(body + "\n"))
        .expect("valid edl response"))
}
