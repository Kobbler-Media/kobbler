use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use time::{OffsetDateTime, Time};
use uuid::Uuid;

use crate::AppState;
use crate::livetv::series_rules::{self, Airing, RuleSpec};
use crate::repositories::{livetv_guide, livetv_recordings, livetv_series_rules};
use crate::routes::AuthUser;

use super::livetv::{AppError, require_contributor};

const KEEP_UNTIL: [&str; 4] = ["watched", "space", "deleted", "date"];
const ON_CLASH: [&str; 3] = ["move", "drop", "ask"];

#[derive(Debug, Serialize)]
pub struct SeriesRuleResponse {
    id: Uuid,
    series_id: String,
    title: String,
    enabled: bool,
    channel_ids: Vec<Uuid>,
    weekdays: Vec<i16>,
    window_start: Option<String>,
    window_end: Option<String>,
    new_only: bool,
    min_season: Option<i32>,
    keep_count: Option<i32>,
    keep_until: String,
    #[serde(with = "time::serde::rfc3339::option")]
    keep_until_date: Option<OffsetDateTime>,
    padding_before_secs: i32,
    padding_after_secs: i32,
    on_clash: String,
    priority: i32,
    queued_count: i64,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

fn format_time(t: Option<Time>) -> Option<String> {
    t.map(|t| format!("{:02}:{:02}", t.hour(), t.minute()))
}

/// The editor sends an `<input type="time">` value, which is `HH:MM` or
/// `HH:MM:SS` depending on the browser.
fn parse_clock(raw: &str) -> Result<Time, AppError> {
    let mut parts = raw.trim().split(':');
    let h: u8 = parts
        .next()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| AppError::BadRequest(format!("'{raw}' is not a HH:MM time")))?;
    let m: u8 = parts
        .next()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| AppError::BadRequest(format!("'{raw}' is not a HH:MM time")))?;
    let s: u8 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    Time::from_hms(h, m, s)
        .map_err(|_| AppError::BadRequest(format!("'{raw}' is not a valid time of day")))
}

fn rule_response(row: livetv_series_rules::SeriesRuleRow, queued_count: i64) -> SeriesRuleResponse {
    SeriesRuleResponse {
        id: row.id,
        series_id: row.series_id,
        title: row.title,
        enabled: row.enabled,
        channel_ids: row.channel_ids,
        weekdays: row.weekdays,
        window_start: format_time(row.window_start),
        window_end: format_time(row.window_end),
        new_only: row.new_only,
        min_season: row.min_season,
        keep_count: row.keep_count,
        keep_until: row.keep_until,
        keep_until_date: row.keep_until_date,
        padding_before_secs: row.padding_before_secs,
        padding_after_secs: row.padding_after_secs,
        on_clash: row.on_clash,
        priority: row.priority,
        queued_count,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

#[derive(Debug, Deserialize)]
pub struct SeriesRuleInput {
    pub series_id: String,
    pub title: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub channel_ids: Vec<Uuid>,
    #[serde(default)]
    pub weekdays: Vec<i16>,
    #[serde(default)]
    pub window_start: Option<String>,
    #[serde(default)]
    pub window_end: Option<String>,
    #[serde(default = "default_true")]
    pub new_only: bool,
    #[serde(default)]
    pub min_season: Option<i32>,
    #[serde(default)]
    pub keep_count: Option<i32>,
    #[serde(default)]
    pub keep_until: Option<String>,
    #[serde(default)]
    pub padding_before_secs: Option<i32>,
    #[serde(default)]
    pub padding_after_secs: Option<i32>,
    #[serde(default)]
    pub on_clash: Option<String>,
    #[serde(default)]
    pub priority: Option<i32>,
}

fn default_true() -> bool {
    true
}

struct Validated {
    window_start: Option<Time>,
    window_end: Option<Time>,
    keep_until: String,
    on_clash: String,
    weekdays: Vec<i16>,
}

fn validate(input: &SeriesRuleInput) -> Result<Validated, AppError> {
    if input.series_id.trim().is_empty() {
        return Err(AppError::BadRequest("series_id is required".to_string()));
    }
    if input.title.trim().is_empty() {
        return Err(AppError::BadRequest("title is required".to_string()));
    }
    let keep_until = input
        .keep_until
        .clone()
        .unwrap_or_else(|| "space".to_string());
    if !KEEP_UNTIL.contains(&keep_until.as_str()) {
        return Err(AppError::BadRequest(format!(
            "keep_until must be one of {}",
            KEEP_UNTIL.join(", ")
        )));
    }
    let on_clash = input.on_clash.clone().unwrap_or_else(|| "move".to_string());
    if !ON_CLASH.contains(&on_clash.as_str()) {
        return Err(AppError::BadRequest(format!(
            "on_clash must be one of {}",
            ON_CLASH.join(", ")
        )));
    }
    if input.weekdays.iter().any(|d| !(0..=6).contains(d)) {
        return Err(AppError::BadRequest(
            "weekdays must be 0 (Sunday) to 6 (Saturday)".to_string(),
        ));
    }
    let mut weekdays = input.weekdays.clone();
    weekdays.sort_unstable();
    weekdays.dedup();

    Ok(Validated {
        window_start: input
            .window_start
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(parse_clock)
            .transpose()?,
        window_end: input
            .window_end
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(parse_clock)
            .transpose()?,
        keep_until,
        on_clash,
        weekdays,
    })
}

async fn queued_counts(state: &AppState) -> Result<HashMap<Uuid, i64>, AppError> {
    Ok(livetv_recordings::queued_counts_by_rule(&state.pool)
        .await?
        .into_iter()
        .collect())
}

pub async fn list_series_rules(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;
    let counts = queued_counts(&state).await?;
    let items: Vec<SeriesRuleResponse> = livetv_series_rules::list_all(&state.pool, false)
        .await?
        .into_iter()
        .map(|r| {
            let queued = counts.get(&r.id).copied().unwrap_or(0);
            rule_response(r, queued)
        })
        .collect();
    Ok(Json(json!({ "items": items })))
}

pub async fn create_series_rule(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<SeriesRuleInput>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;
    let v = validate(&input)?;
    let row = livetv_series_rules::create(
        &state.pool,
        livetv_series_rules::CreateSeriesRule {
            series_id: input.series_id.trim().to_string(),
            title: input.title.trim().to_string(),
            enabled: input.enabled,
            channel_ids: input.channel_ids.clone(),
            weekdays: v.weekdays,
            window_start: v.window_start,
            window_end: v.window_end,
            new_only: input.new_only,
            min_season: input.min_season,
            keep_count: input.keep_count,
            keep_until: v.keep_until,
            keep_until_date: None,
            padding_before_secs: input.padding_before_secs.unwrap_or(120).clamp(0, 3600),
            padding_after_secs: input.padding_after_secs.unwrap_or(360).clamp(0, 7200),
            on_clash: v.on_clash,
            priority: input.priority.unwrap_or(0),
        },
    )
    .await?;

    // A new rule cannot wait for the next guide refresh six hours away.
    if let Err(e) = series_rules::expand_all(&state.pool).await {
        tracing::warn!(error = %e, "expanding series rules after a create failed");
    }
    let counts = queued_counts(&state).await?;
    let queued = counts.get(&row.id).copied().unwrap_or(0);
    Ok((StatusCode::CREATED, Json(rule_response(row, queued))))
}

pub async fn update_series_rule(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(input): Json<SeriesRuleInput>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;
    let v = validate(&input)?;
    let row = livetv_series_rules::update(
        &state.pool,
        id,
        livetv_series_rules::UpdateSeriesRule {
            title: Some(input.title.trim().to_string()),
            enabled: Some(input.enabled),
            channel_ids: Some(input.channel_ids.clone()),
            weekdays: Some(v.weekdays),
            window_start: Some(v.window_start),
            window_end: Some(v.window_end),
            new_only: Some(input.new_only),
            min_season: Some(input.min_season),
            keep_count: Some(input.keep_count),
            keep_until: Some(v.keep_until),
            keep_until_date: None,
            padding_before_secs: input.padding_before_secs.map(|p| p.clamp(0, 3600)),
            padding_after_secs: input.padding_after_secs.map(|p| p.clamp(0, 7200)),
            on_clash: Some(v.on_clash),
            priority: input.priority,
        },
    )
    .await?
    .ok_or_else(|| AppError::NotFound("Series rule not found".to_string()))?;

    if let Err(e) = series_rules::expand_all(&state.pool).await {
        tracing::warn!(error = %e, "expanding series rules after an update failed");
    }
    let counts = queued_counts(&state).await?;
    let queued = counts.get(&row.id).copied().unwrap_or(0);
    Ok(Json(rule_response(row, queued)))
}

pub async fn delete_series_rule(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;
    if !livetv_series_rules::delete(&state.pool, id).await? {
        return Err(AppError::NotFound("Series rule not found".to_string()));
    }
    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Preview — rendered live as the rule is edited, so it must not write anything
// =============================================================================

pub async fn preview_series_rule(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<SeriesRuleInput>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;
    let v = validate(&input)?;
    let spec = RuleSpec {
        series_id: input.series_id.trim().to_string(),
        channel_ids: input.channel_ids.clone(),
        weekdays: v.weekdays,
        window_start: v.window_start,
        window_end: v.window_end,
        new_only: input.new_only,
        min_season: input.min_season,
        on_clash: v.on_clash,
        padding_before_secs: input.padding_before_secs.unwrap_or(120),
        padding_after_secs: input.padding_after_secs.unwrap_or(360),
    };

    let now = OffsetDateTime::now_utc();
    let airings: Vec<Airing> = livetv_guide::list_airings_for_series(
        &state.pool,
        &spec.series_id,
        now,
        now + series_rules::GUIDE_HORIZON,
    )
    .await?
    .into_iter()
    .map(Airing::from)
    .collect();

    let keys: Vec<String> = airings
        .iter()
        .filter_map(|a| a.episode_key.clone())
        .collect();
    let claimed: HashSet<String> = livetv_recordings::claimed_episode_keys(&state.pool, &keys)
        .await?
        .into_iter()
        .collect();

    let clusters = series_rules::evaluate(&spec, &airings, &claimed);
    let mut would_record = 0usize;
    let mut suppressed = 0usize;
    let rendered: Vec<serde_json::Value> = clusters
        .iter()
        .map(|c| {
            let members: Vec<serde_json::Value> = c
                .members
                .iter()
                .map(|m| {
                    if m.action == series_rules::Action::Record {
                        would_record += 1;
                    } else {
                        suppressed += 1;
                    }
                    json!({
                        "guide_id": m.airing.guide_id,
                        "channel_id": m.airing.channel_id,
                        "channel_name": m.airing.channel_name,
                        "lcn": m.airing.lcn,
                        "hd": m.airing.hd,
                        "start_time": m.airing.start_time
                            .format(&time::format_description::well_known::Rfc3339)
                            .unwrap_or_default(),
                        "action": m.action.as_str(),
                        "reason": m.reason,
                    })
                })
                .collect();
            json!({
                "label": c.label,
                "start_time": c.start_time
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
                "members": members,
            })
        })
        .collect();

    Ok(Json(json!({
        "clusters": rendered,
        "would_record": would_record,
        "suppressed": suppressed,
    })))
}
