//! Instant Stream: a debrid session warmed before anyone presses Play — searched,
//! resolved at kobload's `warm` priority, probed and holding the first few MB — that the
//! player claims instead of walking the options itself.

mod store;

pub use store::PrewarmStore;

use std::time::Instant;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::routes::PlaybackAuth;
use crate::routes::admin::debrid_open::{self, OpenMode, WarmGate, live_response, probe_json};
use crate::routes::admin::debrid_stream::{AutoCreateMedia, StreamResolveBody, kids_may_resolve};
use crate::routes::admin::stream_search_request::{self, SearchTarget};
use crate::routes::helpers::build_kobload_client;
use crate::streaming::cache_download;
use crate::streaming::decision::ClientCapabilities;
use store::{PER_USER, Phase, Prewarm, SAME_PREWARM_FOR};

#[derive(Debug, Deserialize)]
pub struct PrewarmBody {
    pub media_type: String,
    #[serde(default)]
    pub media_id: Option<Uuid>,
    #[serde(default)]
    pub auto_create: Option<AutoCreateMedia>,
    #[serde(default)]
    pub season: Option<u32>,
    #[serde(default)]
    pub episode: Option<u32>,
    #[serde(default)]
    pub capabilities: Option<ClientCapabilities>,
    #[serde(default)]
    pub start_time: Option<f64>,
    #[serde(default)]
    pub estimated_duration: Option<f64>,
    #[serde(default)]
    pub preferred_audio_language: Option<String>,
}

/// What to search for, the key a repeat call for the same title finds it under, and the
/// resolve body any picked option is completed into.
struct Title {
    target: SearchTarget,
    key: String,
    template: StreamResolveBody,
}

impl PrewarmBody {
    /// Clients send a Discover episode's numbers both at the top level and inside
    /// `auto_create`; the top level wins and both end up the same.
    fn title(self) -> Result<Title, Response> {
        let library_id = self.media_id.filter(|id| !id.is_nil());
        let season = self
            .season
            .or(self.auto_create.as_ref().and_then(|a| a.season_number));
        let episode = self
            .episode
            .or(self.auto_create.as_ref().and_then(|a| a.episode_number));
        let mut auto_create = self.auto_create;
        if let Some(ac) = auto_create.as_mut() {
            ac.season_number = season;
            ac.episode_number = episode;
        }
        let (target, key) = match (self.media_type.as_str(), library_id, &auto_create) {
            ("movie", Some(id), _) => (SearchTarget::Movie(id), format!("movie:{id}")),
            ("episode", Some(id), _) => (SearchTarget::Episode(id), format!("episode:{id}")),
            (media_type, None, Some(ac)) => {
                let ids = ac.identifiers.clone().unwrap_or(Value::Null);
                let id_text = |k: &str| {
                    ids.get(k).and_then(|v| {
                        v.as_str()
                            .map(String::from)
                            .or_else(|| v.as_i64().map(|n| n.to_string()))
                    })
                };
                let kind = if matches!(media_type, "tv" | "episode") {
                    "tv"
                } else {
                    "movie"
                };
                let tmdb_id = id_text("tmdb_id");
                let imdb_id = id_text("imdb_id");
                let who = tmdb_id
                    .clone()
                    .or_else(|| imdb_id.clone())
                    .unwrap_or_else(|| ac.title.to_lowercase());
                let key = format!(
                    "discover:{kind}:{who}:{}:{}",
                    season.map(|s| s.to_string()).unwrap_or_default(),
                    episode.map(|e| e.to_string()).unwrap_or_default()
                );
                (
                    SearchTarget::Discover {
                        media_type: kind.to_string(),
                        title: ac.title.clone(),
                        year: ac.year,
                        imdb_id,
                        tmdb_id,
                        season,
                        episode,
                    },
                    key,
                )
            }
            _ => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "media_id (movie or episode) or auto_create required"})),
                )
                    .into_response());
            }
        };
        let template = StreamResolveBody {
            debrid_provider_id: Uuid::nil(),
            info_hash: None,
            magnet_url: None,
            nzb_url: None,
            file_id: None,
            media_type: self.media_type,
            media_id: library_id,
            release_title: String::new(),
            debrid_provider: String::new(),
            quality: None,
            start_time: self.start_time,
            capabilities: self.capabilities,
            estimated_duration: self.estimated_duration,
            auto_create,
            preferred_audio_language: self.preferred_audio_language,
        };
        Ok(Title {
            target,
            key,
            template,
        })
    }
}

/// POST /api/v1/admin/stream/prewarm
pub async fn prewarm(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Json(body): Json<PrewarmBody>,
) -> Response {
    let title = match body.title() {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    let lock = state.stream_prewarms.title_lock(auth.user_id, &title.key);
    let _guard = lock.lock().await;

    if let Some(existing) = state.stream_prewarms.for_title(auth.user_id, &title.key) {
        if reusable(&state, &existing) {
            state.stream_prewarms.touch(existing.id);
            touch_session(&state, existing.session_id);
            return warm_reply(&state, &existing);
        }
        end(&state, existing.id, Phase::Released).await;
    }

    let Some((option, resolve_body)) = pick(&state, &auth, &title).await else {
        return StatusCode::NO_CONTENT.into_response();
    };
    if auth.is_kids && !kids_may_resolve(&state.pool, &resolve_body).await {
        return (StatusCode::NOT_FOUND, "not available on a Kids profile").into_response();
    }
    let release_key =
        debrid_open::release_key(resolve_body.info_hash.as_deref(), resolve_body.file_id);
    // One download per release file: a release already playing, or warming for someone
    // else, is left to the normal walk.
    if let Some(key) = &release_key
        && (state.debrid_session_index.contains_key(key)
            || state.stream_prewarms.warm_for_release(key).is_some())
    {
        return StatusCode::NO_CONTENT.into_response();
    }

    let warm = state.stream_prewarms.warm_for_user(auth.user_id);
    for oldest in warm.iter().take((warm.len() + 1).saturating_sub(PER_USER)) {
        end(&state, oldest.id, Phase::Released).await;
    }

    let gate = WarmGate::default();
    let opened = match debrid_open::open(
        &state,
        auth.user_id,
        &resolve_body,
        OpenMode::Warm(gate.clone()),
    )
    .await
    {
        Ok(o) => o,
        Err(resp) => {
            info!(status = %resp.status(), title = %title.key, "prewarm resolve did not produce a session");
            return StatusCode::NO_CONTENT.into_response();
        }
    };
    let now = Instant::now();
    let prewarm = Prewarm {
        id: Uuid::now_v7(),
        user_id: auth.user_id,
        title: title.key,
        session_id: opened.session_id,
        release_key,
        option,
        body: resolve_body,
        opened: opened.response,
        row: opened.row,
        remainder: opened.remainder,
        gate,
        created_at: now,
        touched_at: now,
        ended_at: None,
        phase: Phase::Warm,
    };
    info!(prewarm_id = %prewarm.id, session_id = %prewarm.session_id, title = %prewarm.title, "prewarmed stream");
    let reply = warm_reply(&state, &prewarm);
    state.stream_prewarms.insert(prewarm);
    reply
}

fn reusable(state: &AppState, prewarm: &Prewarm) -> bool {
    prewarm.is_warm()
        && !prewarm.untouched_too_long()
        && prewarm.created_at.elapsed() < SAME_PREWARM_FOR
        && state
            .playback_sessions
            .get(&prewarm.session_id)
            .is_some_and(|s| s.error.is_none())
}

fn touch_session(state: &AppState, session_id: Uuid) {
    if let Some(mut s) = state.playback_sessions.get_mut(&session_id) {
        s.touch();
    }
}

/// `ready` once the probe has landed and the head is on disk.
fn warm_reply(state: &AppState, prewarm: &Prewarm) -> Response {
    let ready = state
        .playback_sessions
        .get(&prewarm.session_id)
        .is_some_and(|s| {
            let probed = !s.file_path.as_os_str().is_empty();
            let headed = s.download.as_ref().is_none_or(|m| {
                m.is_complete()
                    || m.downloaded() >= debrid_open::WARM_HEAD_BYTES.min(m.total().max(1))
            });
            probed && headed && s.error.is_none()
        });
    Json(json!({
        "state": if ready { "ready" } else { "warming" },
        "prewarm_id": prewarm.id,
        "option": prewarm.option,
    }))
    .into_response()
}

/// The top pick kobload offers that is cached and has not failed, as kobload sent it and
/// as a resolve body.
async fn pick(
    state: &AppState,
    auth: &PlaybackAuth,
    title: &Title,
) -> Option<(Value, StreamResolveBody)> {
    let prepared = stream_search_request::prepare(
        state,
        auth.user_id,
        &title.target,
        title.template.preferred_audio_language.as_deref(),
    )
    .await
    .ok()?;
    let runtime = state.runtime_config.get().await;
    let client = build_kobload_client(&runtime).ok()?;
    let value = client
        .stream_search_value(&prepared.request)
        .await
        .map_err(|e| warn!(error = %e, "prewarm search failed"))
        .ok()?;
    let raw = value["options"].as_array()?;
    raw.iter().find_map(|raw| {
        let option: nats_jobs::StreamOption = serde_json::from_value(raw.clone()).ok()?;
        let available = option.cached
            || matches!(
                option.availability,
                Some(nats_jobs::StreamAvailability::Ready | nats_jobs::StreamAvailability::Likely)
            );
        let failed = option.playback == Some(nats_jobs::StreamPlayback::Failed);
        (available && !failed).then(|| {
            let mut body = title.template.clone();
            body.debrid_provider_id = option.debrid_provider_id;
            body.info_hash = option.info_hash.clone();
            body.magnet_url = option.magnet_url.clone();
            body.nzb_url = option.nzb_url.clone();
            body.file_id = option.files.first().map(|f| f.id);
            body.release_title = option.release_title.clone();
            body.debrid_provider = option.debrid_provider.clone();
            body.quality = option.quality.clone();
            (raw.clone(), body)
        })
    })
}

/// End a warm prewarm and its session. The cache file stays for whoever plays it next.
async fn end(state: &AppState, id: Uuid, phase: Phase) {
    let Some(prewarm) = state.stream_prewarms.get(id) else {
        return;
    };
    if !state.stream_prewarms.finish(id, phase) {
        return;
    }
    prewarm.gate.released.cancel();
    crate::routes::admin::streaming::end_session(state, prewarm.session_id).await;
    info!(prewarm_id = %id, session_id = %prewarm.session_id, "prewarm ended");
}

/// Expire every prewarm nobody touched in time, and forget long-ended ones.
pub async fn sweep(state: &AppState) {
    for id in state.stream_prewarms.untouched() {
        end(state, id, Phase::Expired).await;
    }
    state.stream_prewarms.forget_old();
}

fn owned(state: &AppState, auth: &PlaybackAuth, id: Uuid) -> Option<Prewarm> {
    state
        .stream_prewarms
        .get(id)
        .filter(|p| p.user_id == auth.user_id)
}

fn gone(phase: &Phase) -> Option<Response> {
    match phase {
        Phase::Expired => Some((StatusCode::GONE, "prewarm expired").into_response()),
        Phase::Released => Some((StatusCode::NOT_FOUND, "prewarm released").into_response()),
        _ => None,
    }
}

/// POST /api/v1/admin/stream/prewarm/{id}/claim
pub async fn claim(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Path(id): Path<Uuid>,
) -> Response {
    let Some(prewarm) = owned(&state, &auth, id) else {
        return (StatusCode::NOT_FOUND, "prewarm not found").into_response();
    };
    if prewarm.untouched_too_long() {
        end(&state, id, Phase::Expired).await;
        return (StatusCode::GONE, "prewarm expired").into_response();
    }
    claim_prewarm(&state, prewarm).await
}

async fn claim_prewarm(state: &AppState, prewarm: Prewarm) -> Response {
    if let Some(resp) = gone(&prewarm.phase) {
        return resp;
    }
    if let Phase::Claimed(response) = &prewarm.phase {
        return Json(response.as_ref().clone()).into_response();
    }
    let failed = match state.playback_sessions.get(&prewarm.session_id) {
        None => {
            end(state, prewarm.id, Phase::Expired).await;
            return (StatusCode::GONE, "prewarm expired").into_response();
        }
        Some(s) => s.error.is_some(),
    };
    if failed {
        end(state, prewarm.id, Phase::Released).await;
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": "the prewarmed stream failed"})),
        )
            .into_response();
    }

    let (media_id, media_type) = match prewarm.row.commit(state, &prewarm.body).await {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let session_id = prewarm.session_id;
    // The probe may have landed before the row existed; its own write then went nowhere.
    let probe = state
        .playback_sessions
        .get(&session_id)
        .filter(|s| !s.file_path.as_os_str().is_empty())
        .map(|s| probe_json(&s));
    if let Some(probe) = probe {
        let _ = sqlx::query(
            "UPDATE debrid_stream_sessions SET probe_data = $1, updated_at = NOW() WHERE id = $2",
        )
        .bind(probe)
        .bind(session_id)
        .execute(&state.pool)
        .await;
    }
    if let Some(key) = &prewarm.release_key {
        state.debrid_session_index.insert(key.clone(), session_id);
    }
    resume_download(state, &prewarm);
    prewarm.gate.claimed.cancel();

    let mut base = prewarm.opened.clone();
    base.media_id = Some(media_id.to_string());
    base.media_type = Some(media_type);
    let response = live_response(state, session_id, Some(&base)).unwrap_or(base);
    state
        .stream_prewarms
        .finish(prewarm.id, Phase::Claimed(Box::new(response.clone())));
    touch_session(state, session_id);
    info!(prewarm_id = %prewarm.id, %session_id, "prewarm claimed");
    Json(response).into_response()
}

fn resume_download(state: &AppState, prewarm: &Prewarm) {
    let Some(target) = prewarm.remainder.clone() else {
        return;
    };
    let Some(mut session) = state.playback_sessions.get_mut(&prewarm.session_id) else {
        return;
    };
    let Some(meter) = session.download.clone() else {
        return;
    };
    if meter.is_complete() {
        return;
    }
    if let Some(old) = &session.download_cancel {
        old.cancel();
    }
    let cancel = CancellationToken::new();
    session.download_cancel = Some(cancel.clone());
    drop(session);
    cache_download::spawn(state, target, cancel, meter);
}

/// A resolve of a release this viewer has warm claims that prewarm instead of opening a
/// second session; one warming for another viewer is let go so only one download writes
/// the file.
pub async fn claim_for_resolve(
    state: &AppState,
    user_id: Uuid,
    body: &StreamResolveBody,
) -> Option<Response> {
    let key = debrid_open::release_key(body.info_hash.as_deref(), body.file_id)?;
    let prewarm = state.stream_prewarms.warm_for_release(&key)?;
    if prewarm.user_id != user_id {
        end(state, prewarm.id, Phase::Released).await;
        return None;
    }
    Some(claim_prewarm(state, prewarm).await)
}

/// POST /api/v1/admin/stream/prewarm/{id}/touch
pub async fn touch(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Path(id): Path<Uuid>,
) -> Response {
    let Some(prewarm) = owned(&state, &auth, id) else {
        return (StatusCode::NOT_FOUND, "prewarm not found").into_response();
    };
    if prewarm.untouched_too_long() {
        end(&state, id, Phase::Expired).await;
        return (StatusCode::GONE, "prewarm expired").into_response();
    }
    if let Some(resp) = gone(&prewarm.phase) {
        return resp;
    }
    state.stream_prewarms.touch(id);
    touch_session(&state, prewarm.session_id);
    StatusCode::NO_CONTENT.into_response()
}

/// DELETE /api/v1/admin/stream/prewarm/{id}
pub async fn release(
    State(state): State<AppState>,
    auth: PlaybackAuth,
    Path(id): Path<Uuid>,
) -> Response {
    if owned(&state, &auth, id).is_none() {
        return (StatusCode::NOT_FOUND, "prewarm not found").into_response();
    }
    end(&state, id, Phase::Released).await;
    StatusCode::NO_CONTENT.into_response()
}
