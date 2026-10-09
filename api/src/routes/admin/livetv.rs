use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use time::OffsetDateTime;
use tracing::warn;
use uuid::Uuid;

use crate::AppState;
use crate::livetv::{classify, hdhomerun};
use crate::repositories::{livetv_channels, livetv_guide, livetv_recordings, livetv_tuners};
use crate::routes::AuthUser;

pub enum AppError {
    Unauthorized,
    Forbidden,
    NotFound(String),
    BadRequest(String),
    Conflict(String),
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_string()),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::Conflict(msg) => (StatusCode::CONFLICT, msg),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

impl From<crate::repositories::RepositoryError> for AppError {
    fn from(e: crate::repositories::RepositoryError) -> Self {
        AppError::Internal(e.to_string())
    }
}

pub(crate) fn require_admin(user: &AuthUser) -> Result<(), AppError> {
    user.require_admin().map_err(|_| AppError::Forbidden)
}

pub(crate) fn require_contributor(user: &AuthUser) -> Result<(), AppError> {
    user.require_contributor().map_err(|_| AppError::Forbidden)
}

// =============================================================================
// Response types
// =============================================================================

#[derive(Debug, Serialize)]
pub struct TunerResponse {
    id: Uuid,
    device_id: String,
    friendly_name: String,
    model_number: Option<String>,
    firmware_version: Option<String>,
    base_url: String,
    tuner_count: i32,
    #[serde(with = "time::serde::rfc3339::option")]
    last_scan_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    last_guide_sync_at: Option<OffsetDateTime>,
    channel_count: i64,
    enabled_channel_count: i64,
    leases_in_use: i64,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct DiscoveredTuner {
    device_id: String,
    local_ip: Option<String>,
    base_url: String,
    already_added: bool,
}

#[derive(Debug, Serialize)]
pub struct ChannelResponse {
    id: Uuid,
    tuner_id: Uuid,
    lcn: Option<i32>,
    scan_name: Option<String>,
    callsign: Option<String>,
    frequency: i64,
    program_number: i32,
    onid: Option<i32>,
    tsid: Option<i32>,
    modulation: Option<String>,
    video_codec: Option<String>,
    audio_codec: Option<String>,
    hd: bool,
    signal_strength: Option<i32>,
    signal_quality: Option<i32>,
    #[serde(with = "time::serde::rfc3339::option")]
    last_seen_at: Option<OffsetDateTime>,
    enabled: bool,
    favourite: bool,
    sort_order: i32,
    display_name: Option<String>,
    logo_url: Option<String>,
    kind: String,
    region: Option<String>,
    network: Option<String>,
    name: String,
    mux_label: String,
    type_label: String,
    has_guide: bool,
    guide_entry_count: i64,
    now: Option<Box<GuideNowResponse>>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct RecordingRef {
    id: Uuid,
    status: String,
}

#[derive(Debug, Serialize)]
pub struct GuideEntryResponse {
    id: Uuid,
    channel_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    start_time: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    end_time: OffsetDateTime,
    title: String,
    episode_number: Option<String>,
    episode_title: Option<String>,
    synopsis: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    original_airdate: Option<OffsetDateTime>,
    series_id: Option<String>,
    genres: Vec<String>,
    image_url: Option<String>,
    poster_url: Option<String>,
    is_first: bool,
    episode_key: Option<String>,
    episode_key_kind: Option<String>,
    duration_seconds: i64,
    is_now: bool,
    progress_pct: f64,
    recording: Option<RecordingRef>,
    repeat_count: i64,
}

#[derive(Debug, Serialize)]
pub struct GuideChannelResponse {
    channel: ChannelResponse,
    entries: Vec<GuideEntryResponse>,
}

#[derive(Debug, Serialize)]
pub struct GuideNowResponse {
    channel: ChannelResponse,
    now: Option<GuideEntryResponse>,
    next: Option<GuideEntryResponse>,
    minutes_left: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct Facet {
    value: String,
    label: String,
    count: usize,
}

#[derive(Debug, Serialize)]
pub struct Facets {
    network: Vec<Facet>,
    region: Vec<Facet>,
    #[serde(rename = "type")]
    type_: Vec<Facet>,
    guide: Vec<Facet>,
    status: Vec<Facet>,
    mux: Vec<Facet>,
}

// =============================================================================
// Mapping
// =============================================================================

pub(crate) fn channel_response(
    row: &livetv_channels::ChannelRow,
    guide_entry_count: i64,
) -> ChannelResponse {
    let kind = classify::Kind::parse(&row.kind).unwrap_or(classify::Kind::Tv);
    ChannelResponse {
        id: row.id,
        tuner_id: row.tuner_id,
        lcn: row.lcn,
        scan_name: row.scan_name.clone(),
        callsign: row.callsign.clone(),
        frequency: row.frequency,
        program_number: row.program_number,
        onid: row.onid,
        tsid: row.tsid,
        modulation: row.modulation.clone(),
        video_codec: row.video_codec.clone(),
        audio_codec: row.audio_codec.clone(),
        hd: row.hd,
        signal_strength: row.signal_strength,
        signal_quality: row.signal_quality,
        last_seen_at: row.last_seen_at,
        enabled: row.enabled,
        favourite: row.favourite,
        sort_order: row.sort_order,
        display_name: row.display_name.clone(),
        logo_url: row.logo_url.clone(),
        kind: row.kind.clone(),
        region: row.region.clone(),
        network: row.network.clone(),
        name: classify::display_name(
            row.display_name.as_deref(),
            row.scan_name.as_deref(),
            row.callsign.as_deref(),
        ),
        mux_label: classify::mux_label(row.frequency),
        type_label: classify::type_label(kind, row.video_codec.as_deref(), row.hd),
        has_guide: guide_entry_count > 0,
        guide_entry_count,
        now: None,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

pub(crate) fn guide_entry_response(
    row: &livetv_guide::GuideRow,
    now: OffsetDateTime,
    repeat_count: i64,
    recording: Option<RecordingRef>,
) -> GuideEntryResponse {
    let duration_seconds = (row.end_time - row.start_time).whole_seconds().max(0);
    let is_now = row.start_time <= now && row.end_time > now;
    let progress_pct = if duration_seconds > 0 && now > row.start_time {
        let elapsed = (now - row.start_time).whole_seconds().max(0) as f64;
        ((elapsed / duration_seconds as f64) * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    GuideEntryResponse {
        id: row.id,
        channel_id: row.channel_id,
        start_time: row.start_time,
        end_time: row.end_time,
        title: row.title.clone(),
        episode_number: row.episode_number.clone(),
        episode_title: row.episode_title.clone(),
        synopsis: row.synopsis.clone(),
        original_airdate: row.original_airdate,
        series_id: row.series_id.clone(),
        genres: row.genres.clone(),
        image_url: row.image_url.clone(),
        poster_url: row.poster_url.clone(),
        is_first: row.is_first,
        episode_key: row.episode_key.clone(),
        episode_key_kind: row.episode_key_kind.clone(),
        duration_seconds,
        is_now,
        progress_pct,
        recording,
        repeat_count,
    }
}

// =============================================================================
// Tuners
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct AddTunerInput {
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub discover: bool,
}

async fn tuner_responses(state: &AppState) -> Result<Vec<TunerResponse>, AppError> {
    let rows = livetv_tuners::list_with_counts(&state.pool).await?;
    let leases: HashMap<Uuid, i64> = livetv_tuners::active_lease_counts(&state.pool)
        .await?
        .into_iter()
        .collect();
    Ok(rows
        .into_iter()
        .map(|r| TunerResponse {
            leases_in_use: leases.get(&r.tuner.id).copied().unwrap_or(0),
            id: r.tuner.id,
            device_id: r.tuner.device_id,
            friendly_name: r.tuner.friendly_name,
            model_number: r.tuner.model_number,
            firmware_version: r.tuner.firmware_version,
            base_url: r.tuner.base_url,
            tuner_count: r.tuner.tuner_count,
            last_scan_at: r.tuner.last_scan_at,
            last_guide_sync_at: r.tuner.last_guide_sync_at,
            channel_count: r.channel_count,
            enabled_channel_count: r.enabled_channel_count,
            created_at: r.tuner.created_at,
            updated_at: r.tuner.updated_at,
        })
        .collect())
}

pub async fn list_tuners(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;
    Ok(Json(json!({ "items": tuner_responses(&state).await? })))
}

async fn add_from_base_url(state: &AppState, base_url: &str) -> Result<TunerResponse, AppError> {
    let info = hdhomerun::discover(base_url)
        .await
        .map_err(|e| AppError::BadRequest(format!("Could not reach tuner: {e}")))?;

    let tuner = livetv_tuners::upsert(
        &state.pool,
        livetv_tuners::CreateTuner {
            device_id: info.device_id.clone(),
            friendly_name: info
                .friendly_name
                .clone()
                .unwrap_or_else(|| format!("HDHomeRun {}", info.device_id)),
            model_number: info.model_number.clone(),
            firmware_version: info.firmware_version.clone(),
            base_url: base_url.trim().trim_end_matches('/').to_string(),
            device_auth: info.device_auth.clone(),
            tuner_count: info.tuner_count.unwrap_or(1),
        },
    )
    .await?;

    enqueue_scan(state, tuner.id).await?;

    Ok(TunerResponse {
        id: tuner.id,
        device_id: tuner.device_id,
        friendly_name: tuner.friendly_name,
        model_number: tuner.model_number,
        firmware_version: tuner.firmware_version,
        base_url: tuner.base_url,
        tuner_count: tuner.tuner_count,
        last_scan_at: tuner.last_scan_at,
        last_guide_sync_at: tuner.last_guide_sync_at,
        channel_count: 0,
        enabled_channel_count: 0,
        leases_in_use: 0,
        created_at: tuner.created_at,
        updated_at: tuner.updated_at,
    })
}

pub async fn add_tuner(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<AddTunerInput>,
) -> Result<axum::response::Response, AppError> {
    require_admin(&user)?;

    if let Some(base_url) = input
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let tuner = add_from_base_url(&state, base_url).await?;
        return Ok((StatusCode::CREATED, Json(tuner)).into_response());
    }

    let found = hdhomerun::discover_network()
        .await
        .map_err(|e| AppError::BadRequest(format!("Discovery failed: {e}")))?;

    let existing = livetv_tuners::list_all(&state.pool).await?;
    let known: HashSet<String> = existing.iter().map(|t| t.device_id.clone()).collect();

    let candidates: Vec<DiscoveredTuner> = found
        .iter()
        .filter_map(|d| {
            let base_url = d
                .base_url
                .clone()
                .or_else(|| d.local_ip.as_ref().map(|ip| format!("http://{ip}")))?;
            Some(DiscoveredTuner {
                already_added: known.contains(&d.device_id),
                device_id: d.device_id.clone(),
                local_ip: d.local_ip.clone(),
                base_url,
            })
        })
        .collect();

    let next = candidates.iter().find(|c| !c.already_added);
    match next {
        Some(c) if !input.discover => {
            let tuner = add_from_base_url(&state, &c.base_url).await?;
            Ok((StatusCode::CREATED, Json(tuner)).into_response())
        }
        _ => Ok(Json(json!({ "candidates": candidates })).into_response()),
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct DeleteTunerQuery {
    #[serde(default)]
    pub force: bool,
}

pub async fn delete_tuner(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Query(query): Query<DeleteTunerQuery>,
) -> Result<impl IntoResponse, AppError> {
    require_admin(&user)?;

    // `livetv_recordings` cascades from the channel, so an unguarded delete drops
    // the rows while the captured video stays on disk — and a capture still in
    // flight loses the row it is writing to mid-job.
    let recordings = livetv_recordings::list_for_tuner(&state.pool, id).await?;
    let doomed: Vec<_> = recordings
        .into_iter()
        .filter(|r| r.status != "cancelled" && r.status != "moved")
        .collect();
    if !doomed.is_empty() {
        if !query.force {
            let in_flight = doomed.iter().filter(|r| r.status == "recording").count();
            let detail = if in_flight > 0 {
                format!(
                    "{} recording(s) on this tuner, {in_flight} still capturing.                      Stop them or delete the tuner with ?force=true to remove them too.",
                    doomed.len()
                )
            } else {
                format!(
                    "{} recording(s) on this tuner would be deleted along with their files.                      Delete the tuner with ?force=true to confirm.",
                    doomed.len()
                )
            };
            return Err(AppError::Conflict(detail));
        }
        for row in &doomed {
            livetv_recordings::cancel(&state.pool, row.id).await?;
            // A promoted live buffer is a session directory the sweeper owns; only
            // a scheduled capture's own directory is ours to remove.
            if let Some(dir) = row.output_dir.as_deref()
                && std::path::Path::new(dir)
                    .file_name()
                    .is_some_and(|n| n == row.id.to_string().as_str())
            {
                let _ = tokio::fs::remove_dir_all(dir).await;
            }
            livetv_recordings::delete(&state.pool, row.id).await?;
        }
    }

    // The channel rows cascade away with the tuner, so their search documents
    // have to go with them or the index keeps serving dead channels.
    let orphaned = livetv_channels::list_for_guide(&state.pool, None, false)
        .await?
        .into_iter()
        .filter(|c| c.tuner_id == id)
        .map(|c| c.id)
        .collect::<Vec<_>>();

    if !livetv_tuners::delete(&state.pool, id).await? {
        return Err(AppError::NotFound("Tuner not found".to_string()));
    }

    for channel_id in orphaned {
        if let Err(e) = state
            .search
            .delete_livetv_channel(&channel_id.to_string())
            .await
        {
            warn!(error = %e, "Failed to drop Live TV channel from the search index");
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

async fn enqueue_scan(state: &AppState, tuner_id: Uuid) -> Result<Uuid, AppError> {
    crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "livetv_scan",
        json!({ "tuner_id": tuner_id }),
        crate::jobs::JobContext::for_entity("livetv_tuner", tuner_id, "Scan Live TV tuner lineup"),
    )
    .await
    .map_err(|e| AppError::Internal(format!("Failed to enqueue scan: {e}")))
}

pub async fn scan_tuner(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    require_admin(&user)?;
    if livetv_tuners::get_by_id(&state.pool, id).await?.is_none() {
        return Err(AppError::NotFound("Tuner not found".to_string()));
    }
    let job_id = enqueue_scan(&state, id).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "job_id": job_id }))))
}

pub async fn guide_sync_tuner(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    require_admin(&user)?;
    if livetv_tuners::get_by_id(&state.pool, id).await?.is_none() {
        return Err(AppError::NotFound("Tuner not found".to_string()));
    }
    let job_id = crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "livetv_guide_sync",
        json!({ "tuner_id": id }),
        crate::jobs::JobContext::for_entity("livetv_tuner", id, "Sync Live TV guide"),
    )
    .await
    .map_err(|e| AppError::Internal(format!("Failed to enqueue guide sync: {e}")))?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "job_id": job_id }))))
}

// =============================================================================
// Channels
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ChannelQuery {
    pub tuner_id: Option<Uuid>,
    pub search: Option<String>,
    pub network: Option<String>,
    pub region: Option<String>,
    pub kind: Option<String>,
    pub hd: Option<bool>,
    pub enabled: Option<bool>,
    pub favourite: Option<bool>,
    pub has_guide: Option<bool>,
    pub mux: Option<i64>,
    pub sort: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    #[serde(default)]
    pub with_now: bool,
}

fn tally(values: impl Iterator<Item = (String, String)>) -> Vec<Facet> {
    let mut counts: HashMap<(String, String), usize> = HashMap::new();
    for (value, label) in values {
        *counts.entry((value, label)).or_default() += 1;
    }
    let mut out: Vec<Facet> = counts
        .into_iter()
        .map(|((value, label), count)| Facet {
            value,
            label,
            count,
        })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.label.cmp(&b.label)));
    out
}

fn build_facets(rows: &[livetv_channels::FacetSource]) -> Facets {
    Facets {
        network: tally(
            rows.iter()
                .filter_map(|r| r.network.clone())
                .map(|n| (n.clone(), n)),
        ),
        region: tally(
            rows.iter()
                .filter_map(|r| r.region.clone())
                .map(|n| (n.clone(), n)),
        ),
        type_: tally(rows.iter().map(|r| {
            let kind = classify::Kind::parse(&r.kind).unwrap_or(classify::Kind::Tv);
            let label = classify::type_label(kind, r.video_codec.as_deref(), r.hd);
            (label.clone(), label)
        })),
        guide: tally(rows.iter().map(|r| {
            if r.has_guide {
                ("true".to_string(), "With guide".to_string())
            } else {
                ("false".to_string(), "No guide".to_string())
            }
        })),
        status: tally(rows.iter().flat_map(|r| {
            let mut v = vec![if r.enabled {
                ("enabled".to_string(), "Enabled".to_string())
            } else {
                ("disabled".to_string(), "Hidden".to_string())
            }];
            if r.favourite {
                v.push(("favourite".to_string(), "Favourite".to_string()));
            }
            v
        })),
        mux: tally(
            rows.iter()
                .map(|r| (r.frequency.to_string(), classify::mux_label(r.frequency))),
        ),
    }
}

pub async fn list_channels(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<ChannelQuery>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let filters = livetv_channels::ChannelFilters {
        tuner_id: q.tuner_id,
        search: q.search.clone(),
        network: q.network.clone(),
        region: q.region.clone(),
        kind: q.kind.clone(),
        hd: q.hd,
        enabled: q.enabled,
        favourite: q.favourite,
        has_guide: q.has_guide,
        mux: q.mux,
    };
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let offset = q.offset.unwrap_or(0).max(0);

    let (rows, total) =
        livetv_channels::list_filtered(&state.pool, &filters, q.sort.as_deref(), limit, offset)
            .await?;
    let facet_rows = livetv_channels::facet_source(&state.pool, &filters).await?;

    let mut items: Vec<ChannelResponse> = rows
        .iter()
        .map(|r| channel_response(&r.channel, r.guide_entry_count))
        .collect();

    if q.with_now && !items.is_empty() {
        let ids: Vec<Uuid> = items.iter().map(|c| c.id).collect();
        let now = OffsetDateTime::now_utc();
        let by_channel = now_next_map(&state, &ids, now).await?;
        for (item, row) in items.iter_mut().zip(rows.iter()) {
            let Some((current, next)) = by_channel.get(&item.id) else {
                continue;
            };
            item.now = Some(Box::new(GuideNowResponse {
                channel: channel_response(&row.channel, row.guide_entry_count),
                minutes_left: current
                    .as_ref()
                    .map(|e| (e.end_time - now).whole_minutes().max(0)),
                now: current
                    .as_ref()
                    .map(|e| guide_entry_response(e, now, 0, None)),
                next: next.as_ref().map(|e| guide_entry_response(e, now, 0, None)),
            }));
        }
    }

    Ok(Json(json!({
        "items": items,
        "total": total,
        "facets": build_facets(&facet_rows),
    })))
}

/// Curated columns only. Anything scan-derived is rejected outright rather than
/// silently dropped — the split between the two is the whole point of the schema.
#[derive(Debug, Deserialize)]
pub struct CuratedPatch {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub favourite: Option<bool>,
    #[serde(default)]
    pub sort_order: Option<i32>,
    #[serde(default, deserialize_with = "double_option")]
    pub display_name: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub logo_url: Option<Option<String>>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    pub region: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub network: Option<Option<String>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn double_option<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::deserialize(de).map(Some)
}

const SCAN_DERIVED_KEYS: &[&str] = &[
    "lcn",
    "scan_name",
    "callsign",
    "frequency",
    "program_number",
    "onid",
    "tsid",
    "modulation",
    "video_codec",
    "audio_codec",
    "hd",
    "signal_strength",
    "signal_quality",
    "last_seen_at",
    "tuner_id",
    "name",
    "mux_label",
    "type_label",
    "has_guide",
    "guide_entry_count",
];

pub async fn update_channel(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(patch): Json<CuratedPatch>,
) -> Result<impl IntoResponse, AppError> {
    require_admin(&user)?;

    if let Some(key) = patch.extra.keys().next() {
        return Err(AppError::BadRequest(
            if SCAN_DERIVED_KEYS.contains(&key.as_str()) {
                format!("`{key}` is scan-derived and cannot be edited")
            } else {
                format!("unknown field `{key}`")
            },
        ));
    }

    if let Some(kind) = patch.kind.as_deref()
        && classify::Kind::parse(kind).is_none()
    {
        return Err(AppError::BadRequest(format!(
            "`kind` must be one of tv, radio, dead (got `{kind}`)"
        )));
    }

    let updated = livetv_channels::update_curated(
        &state.pool,
        id,
        livetv_channels::UpdateCuratedChannel {
            enabled: patch.enabled,
            favourite: patch.favourite,
            sort_order: patch.sort_order,
            display_name: patch.display_name,
            logo_url: patch.logo_url,
            kind: patch.kind,
            region: patch.region,
            network: patch.network,
        },
    )
    .await?
    .ok_or_else(|| AppError::NotFound("Channel not found".to_string()))?;

    if let Err(e) =
        crate::search::index_livetv_channel(state.search.as_ref(), &state.pool, id).await
    {
        warn!(error = %e, "Failed to reindex Live TV channel");
    }

    let guide_entry_count = livetv_channels::guide_entry_count(&state.pool, id).await?;
    Ok(Json(channel_response(&updated, guide_entry_count)))
}

#[derive(Debug, Deserialize)]
pub struct BulkChannelInput {
    pub ids: Vec<Uuid>,
    pub action: String,
}

pub async fn bulk_channels(
    State(state): State<AppState>,
    user: AuthUser,
    Json(input): Json<BulkChannelInput>,
) -> Result<impl IntoResponse, AppError> {
    require_admin(&user)?;

    if input.ids.is_empty() {
        return Err(AppError::BadRequest("`ids` must not be empty".to_string()));
    }

    let (enabled, favourite) = match input.action.as_str() {
        "enable" => (Some(true), None),
        "hide" => (Some(false), None),
        "favourite" => (None, Some(true)),
        "unfavourite" => (None, Some(false)),
        other => {
            return Err(AppError::BadRequest(format!(
                "`action` must be enable, hide, favourite or unfavourite (got `{other}`)"
            )));
        }
    };

    let updated = livetv_channels::bulk_set(&state.pool, &input.ids, enabled, favourite).await?;

    match crate::search::fetch_all_livetv_channel_docs(&state.pool).await {
        Ok(docs) => {
            if let Err(e) = state.search.reindex_all_livetv_channels(docs).await {
                warn!(error = %e, "Failed to reindex Live TV channels");
            }
        }
        Err(e) => warn!(error = %e, "Failed to fetch Live TV channel docs"),
    }

    Ok(Json(json!({ "updated": updated })))
}

// =============================================================================
// Guide
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct GuideQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    pub channel_ids: Option<String>,
    pub search: Option<String>,
    pub genre: Option<String>,
    #[serde(default)]
    pub include_hidden: bool,
}

fn parse_time(raw: Option<&String>, fallback: OffsetDateTime) -> Result<OffsetDateTime, AppError> {
    match raw {
        None => Ok(fallback),
        Some(s) => OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|_| AppError::BadRequest(format!("`{s}` is not an RFC3339 timestamp"))),
    }
}

fn parse_ids(raw: Option<&String>) -> Result<Option<Vec<Uuid>>, AppError> {
    let Some(raw) = raw.map(|s| s.trim()).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    raw.split(',')
        .map(|s| {
            Uuid::parse_str(s.trim())
                .map_err(|_| AppError::BadRequest(format!("`{s}` is not a uuid")))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn repeat_counts(rows: &[livetv_guide::GuideRow]) -> HashMap<String, i64> {
    let mut counts: HashMap<String, i64> = HashMap::new();
    for r in rows {
        if let Some(key) = r.episode_key.as_ref() {
            *counts.entry(key.clone()).or_default() += 1;
        }
    }
    counts
}

async fn recording_refs(
    state: &AppState,
    rows: &[livetv_guide::GuideRow],
) -> Result<HashMap<Uuid, RecordingRef>, AppError> {
    if rows.is_empty() {
        return Ok(HashMap::new());
    }
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    Ok(livetv_guide::recordings_for_entries(&state.pool, &ids)
        .await?
        .into_iter()
        .map(|(guide_id, id, status)| (guide_id, RecordingRef { id, status }))
        .collect())
}

pub async fn get_guide(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<GuideQuery>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let now = OffsetDateTime::now_utc();
    let from = parse_time(q.from.as_ref(), now)?;
    let to = parse_time(q.to.as_ref(), from + time::Duration::hours(24))?;
    if to <= from {
        return Err(AppError::BadRequest(
            "`to` must be after `from`".to_string(),
        ));
    }
    let channel_ids = parse_ids(q.channel_ids.as_ref())?;

    let channels =
        livetv_channels::list_for_guide(&state.pool, channel_ids.clone(), !q.include_hidden)
            .await?;
    let wanted: Vec<Uuid> = channels.iter().map(|c| c.id).collect();

    let search = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let entries = match search {
        None => {
            livetv_guide::list_window(
                &state.pool,
                from,
                to,
                Some(&wanted),
                None,
                q.genre.as_deref(),
            )
            .await?
        }
        Some(query) => search_guide(&state, query, from, to, &wanted, q.genre.as_deref()).await?,
    };

    let repeats = repeat_counts(&entries);
    let recordings = recording_refs(&state, &entries).await?;

    let mut by_channel: HashMap<Uuid, Vec<GuideEntryResponse>> = HashMap::new();
    let mut counts: HashMap<Uuid, i64> = HashMap::new();
    for row in &entries {
        *counts.entry(row.channel_id).or_default() += 1;
    }
    for row in &entries {
        let repeat = row
            .episode_key
            .as_ref()
            .and_then(|k| repeats.get(k))
            .copied()
            .unwrap_or(1)
            - 1;
        by_channel
            .entry(row.channel_id)
            .or_default()
            .push(guide_entry_response(
                row,
                now,
                repeat.max(0),
                recordings.get(&row.id).map(|r| RecordingRef {
                    id: r.id,
                    status: r.status.clone(),
                }),
            ));
    }

    // A query is answered across every channel at once, so the ones that matched nothing
    // are pure payload — and a result list is what the clients render from it, not a grid
    // that needs a row per channel. The unfiltered guide still carries them all.
    let items: Vec<GuideChannelResponse> = channels
        .iter()
        .filter_map(|c| {
            let entries = by_channel.remove(&c.id).unwrap_or_default();
            if search.is_some() && entries.is_empty() {
                return None;
            }
            Some(GuideChannelResponse {
                channel: channel_response(c, counts.get(&c.id).copied().unwrap_or(0)),
                entries,
            })
        })
        .collect();

    Ok(Json(json!({ "channels": items })))
}

/// Bounded well above what a 48 h guide can match so the index path answers with as much
/// as the scan would, and low enough that a one-letter query cannot pull the whole index.
const GUIDE_SEARCH_LIMIT: usize = 500;

/// Guide search, index first.
///
/// Meilisearch brings typo tolerance and word-level matching that `ILIKE '%q%'` cannot,
/// but it only ever yields ids: [`crate::search::LivetvGuideDocument`] is a projection
/// with no episode number, duration, recording state or repeat count, so the rows are
/// hydrated from Postgres and the response shape is the same either way.
///
/// The scan stays as the fallback rather than an alternative. An index that is
/// unreachable, unconfigured, or simply not populated yet all answer "no hits", which is
/// indistinguishable from a genuine miss — falling through costs one query on a real miss
/// and is the difference between degraded and broken everywhere else.
///
/// Relevance order is dropped on purpose: the response groups by channel and every client
/// sorts by airtime, so what the index is here for is matching, not ranking.
async fn search_guide(
    state: &AppState,
    query: &str,
    from: OffsetDateTime,
    to: OffsetDateTime,
    channel_ids: &[Uuid],
    genre: Option<&str>,
) -> Result<Vec<livetv_guide::GuideRow>, AppError> {
    let filters = crate::search::LivetvGuideSearchFilters {
        genres: genre.map(|g| vec![g.to_string()]),
        ..Default::default()
    };
    let hits = match state
        .search
        .search_livetv_guide(query, &filters, GUIDE_SEARCH_LIMIT, 0)
        .await
    {
        Ok(hits) => hits,
        Err(e) => {
            warn!(error = %e, "guide index search failed, falling back to scan");
            Vec::new()
        }
    };

    let ids: Vec<Uuid> = hits
        .iter()
        .filter_map(|d| Uuid::parse_str(&d.id).ok())
        .collect();
    if !ids.is_empty() {
        let rows = livetv_guide::list_by_ids(&state.pool, &ids, from, to, Some(channel_ids), genre)
            .await?;
        if !rows.is_empty() {
            return Ok(rows);
        }
    }

    Ok(
        livetv_guide::list_window(&state.pool, from, to, Some(channel_ids), Some(query), genre)
            .await?,
    )
}

pub(crate) type NowNext = (
    Option<livetv_guide::GuideRow>,
    Option<livetv_guide::GuideRow>,
);

pub(crate) async fn now_next_map(
    state: &AppState,
    channel_ids: &[Uuid],
    now: OffsetDateTime,
) -> Result<HashMap<Uuid, NowNext>, AppError> {
    let rows = livetv_guide::list_window(
        &state.pool,
        now - time::Duration::hours(6),
        now + time::Duration::hours(12),
        Some(channel_ids),
        None,
        None,
    )
    .await?;

    let mut out: HashMap<Uuid, NowNext> = HashMap::new();
    for row in rows {
        let slot = out.entry(row.channel_id).or_insert((None, None));
        if row.start_time <= now && row.end_time > now {
            slot.0 = Some(row);
        } else if row.start_time > now {
            let better = slot
                .1
                .as_ref()
                .map(|n| row.start_time < n.start_time)
                .unwrap_or(true);
            if better {
                slot.1 = Some(row);
            }
        }
    }
    Ok(out)
}

#[derive(Debug, Deserialize)]
pub struct GuideNowQuery {
    pub channel_ids: Option<String>,
    pub limit: Option<i64>,
}

pub async fn get_guide_now(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<GuideNowQuery>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let channel_ids = parse_ids(q.channel_ids.as_ref())?;
    let mut channels = livetv_channels::list_for_guide(&state.pool, channel_ids, true).await?;
    if let Some(limit) = q.limit {
        channels.truncate(limit.clamp(1, 500) as usize);
    }
    if channels.is_empty() {
        return Ok(Json(json!({ "items": [] })));
    }

    let now = OffsetDateTime::now_utc();
    let ids: Vec<Uuid> = channels.iter().map(|c| c.id).collect();
    let map = now_next_map(&state, &ids, now).await?;

    let items: Vec<GuideNowResponse> = channels
        .iter()
        .map(|c| {
            let (current, next) = map.get(&c.id).cloned().unwrap_or((None, None));
            GuideNowResponse {
                channel: channel_response(c, 0),
                minutes_left: current
                    .as_ref()
                    .map(|e| (e.end_time - now).whole_minutes().max(0)),
                now: current
                    .as_ref()
                    .map(|e| guide_entry_response(e, now, 0, None)),
                next: next.as_ref().map(|e| guide_entry_response(e, now, 0, None)),
            }
        })
        .collect();

    Ok(Json(json!({ "items": items })))
}

pub async fn get_guide_entry(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    require_contributor(&user)?;

    let row = livetv_guide::get_by_id(&state.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Guide entry not found".to_string()))?;

    let now = OffsetDateTime::now_utc();
    let recordings = recording_refs(&state, std::slice::from_ref(&row)).await?;
    let repeat = match row.episode_key.as_ref() {
        Some(key) => {
            let window = livetv_guide::list_window(
                &state.pool,
                now - time::Duration::hours(24),
                now + time::Duration::hours(48),
                None,
                None,
                None,
            )
            .await?;
            window
                .iter()
                .filter(|r| r.episode_key.as_deref() == Some(key.as_str()) && r.id != row.id)
                .count() as i64
        }
        None => 0,
    };

    Ok(Json(guide_entry_response(
        &row,
        now,
        repeat,
        recordings.get(&row.id).map(|r| RecordingRef {
            id: r.id,
            status: r.status.clone(),
        }),
    )))
}
