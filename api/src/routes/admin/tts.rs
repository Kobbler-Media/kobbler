//! TTS (text-to-speech) proxy routes.
//!
//! Proxies requests to an OpenAI-compatible TTS service, adding authentication.
//! The TTS service itself is internal-only and has no auth.

use axum::Json;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use tokio_stream::StreamExt;
use tracing::{error, warn};
use uuid::Uuid;

use crate::AppState;
use crate::http;
use crate::repositories::{audiobook_files, user_voice_presets};
use crate::routes::AuthUser;
use crate::tts::{self, TtsBackend, TtsParams, timeline};

/// Default TTS service URL when none is configured (Docker compose service name).
const DEFAULT_TTS_URL: &str = "http://tts:8880";

/// Voice to use when neither the request, the active preset, nor the admin
/// default names one.
///
/// Breeze has no built-in voice ids at all — not even `"default"`; a voice is a
/// clone the operator created. Naming one that does not exist is worse than
/// naming none, because omitting `voice` is meaningful there: the engine picks
/// a speaker and holds it for the whole request. The other backends do ship
/// built-ins, and `af_heart` is kokoro's.
fn fallback_voice(backend: TtsBackend) -> String {
    match backend {
        TtsBackend::Breeze => String::new(),
        _ => "af_heart".to_string(),
    }
}

/// Maximum voice clip upload size (10 MB).
const MAX_CLIP_SIZE: usize = 10 * 1024 * 1024;

/// Maximum presets per user.
const MAX_PRESETS_PER_USER: usize = 10;

/// Allowed audio extensions for voice clips.
const ALLOWED_CLIP_EXTENSIONS: &[&str] = &["wav", "mp3", "ogg", "m4a", "flac", "opus", "webm"];

/// Map an `UpdatePresetRequest`'s tristate fields against an existing preset
/// row. Caller passes the result to `user_voice_presets::update_preset` so
/// the SQL UPDATE writes resolved values directly (one round-trip).
fn resolve_nullable_update(
    req: &UpdatePresetRequest,
    existing: &user_voice_presets::UserVoicePreset,
) -> user_voice_presets::ResolvedNullable {
    use user_voice_presets::{ResolvedNullable, SetOrClear};
    let temperature: SetOrClear<f64> = req.temperature.into();
    let ref_text: SetOrClear<String> = req.ref_text.clone().into();
    let instructions: SetOrClear<String> = req.instructions.clone().into();
    let speaker: SetOrClear<String> = req.speaker.clone().into();
    let seed: SetOrClear<i64> = req.seed.into();
    let top_p: SetOrClear<f64> = req.top_p.into();
    let top_k: SetOrClear<i32> = req.top_k.into();
    let repetition_penalty: SetOrClear<f64> = req.repetition_penalty.into();
    let max_new_frames: SetOrClear<i32> = req.max_new_frames.into();

    ResolvedNullable {
        temperature: temperature.resolve(existing.temperature),
        ref_text: ref_text.resolve_clone(existing.ref_text.as_ref()),
        instructions: instructions.resolve_clone(existing.instructions.as_ref()),
        speaker: speaker.resolve_clone(existing.speaker.as_ref()),
        seed: seed.resolve(existing.seed),
        top_p: top_p.resolve(existing.top_p),
        top_k: top_k.resolve(existing.top_k),
        repetition_penalty: repetition_penalty.resolve(existing.repetition_penalty),
        max_new_frames: max_new_frames.resolve(existing.max_new_frames),
    }
}

// =============================================================================
// TTS Status
// =============================================================================

/// GET /api/v1/admin/tts/status
///
/// Returns TTS availability and voice list.
pub async fn get_tts_status(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    let config = state.runtime_config.get().await;

    let backend = TtsBackend::from_config(&config.tts_service_type);

    if !config.tts_enabled {
        return Json(tts::TtsStatusResponse {
            enabled: false,
            service_url: None,
            voices: Vec::new(),
            default_voice: None,
            engine: backend.engine_label().to_string(),
            supports_emotion: false,
            supports_alignment: backend.supports_alignment(),
        })
        .into_response();
    }

    // Route voice listing through koblem when proxy mode is on so the engine
    // header is honoured. Falls back to the direct service URL otherwise.
    let service_url_owned = config
        .effective_tts_url()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_TTS_URL.to_string());
    let service_url = service_url_owned.as_str();

    let mut voices = tts::get_voices(service_url).await;

    // Voice clips are registered with chatterbox under the synthetic id
    // `preset-<uuid>`, with no friendly name. Substitute the user's preset name
    // so voice-selection UIs render the label the user actually picked.
    match user_voice_presets::list_presets(&state.pool, auth.user_id).await {
        Ok(presets) => {
            let mut renamed = false;
            for v in &mut voices {
                if let Some(uuid_str) = v.id.strip_prefix("preset-")
                    && let Ok(preset_id) = Uuid::parse_str(uuid_str)
                    && let Some(p) = presets.iter().find(|p| p.id == preset_id)
                {
                    v.name = p.name.clone();
                    renamed = true;
                }
            }
            // get_voices already sorted by name; re-sort so renamed entries land
            // in the right alphabetical slot.
            if renamed {
                voices.sort_by(|a, b| a.name.cmp(&b.name));
            }
        }
        Err(e) => {
            error!(error = %e, "Failed to load voice presets for status enrichment");
        }
    }

    // Determine if the TTS service supports emotion control and voice cloning.
    // Chatterbox (temperature + WAV cloning) and breeze (temperature + WAV
    // cloning + voice-design) both qualify; kokoro/openai don't. For "auto"
    // we probe the chatterbox-specific `/voices` endpoint.
    let supports_emotion = match backend {
        TtsBackend::Chatterbox | TtsBackend::Breeze => true,
        TtsBackend::Kokoro | TtsBackend::OpenAi => false,
        TtsBackend::Auto => {
            let probe_url = format!("{}/voices", service_url.trim_end_matches('/'));
            matches!(
                http::new_client().head(&probe_url).send().await,
                Ok(r) if r.status().is_success() || r.status().as_u16() == 405
            )
        }
    };

    let enabled = !voices.is_empty();
    Json(tts::TtsStatusResponse {
        enabled,
        service_url: Some(service_url.to_string()),
        voices,
        default_voice: config.tts_default_voice.clone(),
        engine: backend.engine_label().to_string(),
        supports_emotion,
        supports_alignment: backend.supports_alignment(),
    })
    .into_response()
}

// =============================================================================
// Synthesize
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct SynthesizeRequest {
    pub text: String,
    pub voice: Option<String>,
    pub temperature: Option<f64>,
    /// Render rate. Falls back to the active preset's stored speed. A rate the
    /// engine refuses degrades to 1.0 rather than failing — the applied rate is
    /// reported back on the stream, never assumed.
    pub speed: Option<f64>,
}

/// The read a synthesize or cast request asks for, with voice and parameters
/// settled. Parameter fallback chain: request -> active preset -> admin default ->
/// hardcoded default.
pub(super) async fn plan_synthesis(
    state: &AppState,
    user_id: Uuid,
    req: SynthesizeRequest,
) -> Result<timeline::Plan, Response> {
    let config = state.runtime_config.get().await;

    if !config.tts_enabled {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "TTS is not enabled"})),
        )
            .into_response());
    }

    // effective_tts_url redirects to koblem when routing is enabled.
    let service_url = config
        .effective_tts_url()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_TTS_URL.to_string());

    if req.text.is_empty() || req.text.len() > 50000 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Text must be 1-50000 characters"})),
        )
            .into_response());
    }

    // Reader text arrives carrying the EPUB source file's own line wrapping —
    // the model performs those breaks as pauses and the narration comes out
    // laboured. Normalised here rather than in the reader so every caller gets
    // it, and so the text echoed back on `paragraph_start` (which the client
    // maps word indices onto) is the same string the engine actually read.
    let text = tts::normalize_for_synthesis(&req.text);

    // Load user's active preset for fallback values
    let active_preset = match user_voice_presets::get_active_preset(&state.pool, user_id).await {
        Ok(p) => p,
        Err(e) => {
            error!(error = %e, "Failed to load active voice preset");
            None
        }
    };

    let backend = TtsBackend::from_config(&config.tts_service_type);

    // Owned so the spawned sentence loop can rebuild the endpoint per iteration
    // — `SpeechEndpoint` borrows the key, and that task outlives `config`.
    let via_koblem = config.tts_routes_via_koblem();
    let koblem_api_key = config.koblem_api_key.clone();

    // Resolve voice: request -> active preset (clip or named) -> admin default
    // -> the backend's own fallback.
    let voice = if let Some(v) = &req.voice
        && !v.trim().is_empty()
    {
        v.clone()
    } else if let Some(ref preset) = active_preset
        && preset.clip_filename.is_some()
    {
        format!("preset-{}", preset.id)
    } else if let Some(ref preset) = active_preset
        && let Some(ref v) = preset.voice
    {
        v.clone()
    } else if let Some(ref v) = config.tts_default_voice {
        v.clone()
    } else {
        fallback_voice(backend)
    };

    // Resolve TTS params: request -> active preset -> admin default -> None.
    // Engine-native fields (top_p/top_k/etc) come from the preset; chatterbox
    // ignores them via the wire-format branch.
    let params = TtsParams {
        speed: req
            .speed
            .or_else(|| active_preset.as_ref().map(|p| p.speed)),
        temperature: req
            .temperature
            .or_else(|| active_preset.as_ref().and_then(|p| p.temperature))
            .or(config.tts_default_temperature),
        top_p: active_preset.as_ref().and_then(|p| p.top_p),
        top_k: active_preset
            .as_ref()
            .and_then(|p| p.top_k)
            .map(|v| v as u32),
        repetition_penalty: active_preset.as_ref().and_then(|p| p.repetition_penalty),
        seed: active_preset.as_ref().and_then(|p| p.seed),
        instructions: active_preset.as_ref().and_then(|p| p.instructions.clone()),
        speaker: active_preset.as_ref().and_then(|p| p.speaker.clone()),
        max_new_frames: active_preset
            .as_ref()
            .and_then(|p| p.max_new_frames)
            .map(|v| v as u32),
        response_format: None,
    };

    // Resolve the preset whose clip backs the chosen voice (if any) and make
    // sure it's registered with the upstream TTS service. The pre-existing
    // active-preset branch covered the read-along flow that doesn't pass a
    // voice; the explicit-voice branch covers Test Voice / preset audition
    // sending `req.voice = "preset-<uuid>"` directly. Without the latter,
    // engine switches (chatterbox → breeze) leave the clip registered only
    // on the old engine and the new one synths silence for an unknown voice.
    let voice_preset = if let Some(uuid_str) = voice.strip_prefix("preset-")
        && let Ok(preset_id) = Uuid::parse_str(uuid_str)
    {
        match user_voice_presets::get_preset(&state.pool, preset_id, user_id).await {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, preset_id = %preset_id, "Failed to load voice preset");
                None
            }
        }
    } else {
        None
    };

    if let Some(ref preset) = voice_preset
        && preset.clip_filename.is_some()
    {
        let clip_path = std::path::PathBuf::from(&*state.config.storage_root)
            .join("voices")
            .join(user_id.to_string())
            .join(format!("{}.wav", preset.id));

        let voice_name = format!("preset-{}", preset.id);
        if let Err(e) = tts::ensure_voice_registered(
            &service_url,
            backend,
            &voice_name,
            &clip_path,
            preset.ref_text.as_deref(),
        )
        .await
        {
            error!(error = %e, "Failed to ensure voice is registered");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to register voice clip with TTS service"})),
            )
                .into_response());
        }
    }

    // Settle the narrator against what the engine actually has.
    //
    // Two failures collapse into one check here, both specific to the aligner
    // engine. First: an unnamed voice means the engine samples a speaker from
    // the text and keeps it for THIS request only — long-form rolls each
    // chunk's audio forward as the reference for the next, so one request holds
    // one reader and the next starts over. The read-along issues a request per
    // page, so unnamed means a different reader after every page turn. Second:
    // a voice the engine does not have is a hard 400, and a stale id is easy to
    // be holding — the reader used to default to a kokoro voice name and keeps
    // its choice in localStorage, which no engine switch ever cleared.
    //
    // Both answers are the same: fall back to one minted narrator that persists
    // in the engine's voice archive. Any voice the engine really has (including
    // a preset clip, registered just above) is left alone.
    //
    // Best-effort throughout: a mint that fails leaves the request exactly as it
    // was. Losing the narrator is not worth losing the page.
    let voice = if backend.supports_alignment() {
        let known = tts::get_voices(&service_url).await;
        let has = |id: &str| known.iter().any(|v| v.id == id);

        if !tts::voice_is_unpinned(&voice) && has(&voice) {
            voice
        } else {
            if !tts::voice_is_unpinned(&voice) {
                warn!(
                    voice = %voice,
                    "requested voice is not registered on the TTS engine; using the narrator"
                );
            }
            let minted = if has(tts::NARRATOR_VOICE_ID) {
                Ok(())
            } else {
                let endpoint = tts::SpeechEndpoint::resolve(via_koblem, koblem_api_key.as_deref());
                tts::mint_narrator_voice(&service_url, backend, endpoint).await
            };
            match minted {
                Ok(()) => tts::NARRATOR_VOICE_ID.to_string(),
                Err(e) => {
                    error!(error = %e, "Failed to mint the read-along narrator voice");
                    // Unpinned: a fresh reader per page, but a reader.
                    String::new()
                }
            }
        }
    } else {
        voice
    };

    // Alignment-capable engine (breeze) + koblem-with-api-key → paragraph
    // streaming with word-level forced alignment, routed to that engine's
    // aligned-generate endpoint. Any other backend (chatterbox/kokoro/openai), or
    // koblem unconfigured, renders per sentence: kobbler then talks to
    // `tts_service_url` directly so turning koblem off doesn't break TTS — it
    // just loses streaming + FA.
    let use_paragraph_stream = backend.supports_alignment()
        && config
            .koblem_service_url
            .as_deref()
            .is_some_and(|s| !s.is_empty())
        && config
            .koblem_api_key
            .as_deref()
            .is_some_and(|s| !s.is_empty());

    let route = if use_paragraph_stream {
        if text.is_empty() {
            return Err(bad_request("No text to synthesize"));
        }
        // Clients send whatever they have — a page, a chapter — and the
        // chunking policy lives here, in one place, rather than being guessed
        // at differently by each of them. Splitting was once done by the
        // caller, per sentence, which is what made a truncated paragraph
        // desync every later highlight; these passages are sentence-aligned
        // and carry their own offsets, so a failure costs one passage.
        timeline::Route::Passages {
            passages: tts::split_passages(&text),
            koblem_url: config.koblem_service_url.clone().unwrap_or_default(),
            koblem_api_key: koblem_api_key.unwrap_or_default(),
        }
    } else {
        let sentences = tts::split_sentences(&text);
        if sentences.is_empty() {
            return Err(bad_request("No sentences found in text"));
        }
        let gpu_lock = state.gpu_lock.clone();
        gpu_lock.set_enabled(config.gpu_preempt_tts);
        gpu_lock.set_service_urls(
            config.stt_service_url.clone(),
            config.tts_service_url.clone(),
            config.vision_service_url.clone(),
        );
        timeline::Route::Sentences {
            sentences,
            service_url,
            backend,
            via_koblem,
            koblem_api_key,
            gpu_lock,
        }
    };

    Ok(timeline::Plan {
        voice,
        params,
        route,
    })
}

fn bad_request(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": message}))).into_response()
}

/// POST /api/v1/admin/tts/synthesize
///
/// Streams the read back as NDJSON with its audio inline — see
/// [`timeline`] for the envelope.
pub async fn synthesize(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<SynthesizeRequest>,
) -> Response {
    let plan = match plan_synthesis(&state, auth.user_id, req).await {
        Ok(plan) => plan,
        Err(response) => return response,
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(32);
    tokio::spawn(async move { timeline::run(plan, &mut NdjsonSink(tx)).await });
    ndjson(tokio_stream::wrappers::ReceiverStream::new(rx).map(Ok::<_, std::convert::Infallible>))
}

struct NdjsonSink(tokio::sync::mpsc::Sender<String>);

impl timeline::Sink for NdjsonSink {
    fn alive(&self) -> bool {
        !self.0.is_closed()
    }

    async fn ready(&mut self) -> bool {
        self.alive()
    }

    async fn emit(&mut self, item: timeline::Item) -> bool {
        let line = serde_json::to_string(&item.into_wire()).unwrap_or_default();
        self.0.send(format!("{line}\n")).await.is_ok()
    }
}

pub(super) fn ndjson<S, B>(stream: S) -> Response
where
    S: futures_util::Stream<Item = Result<B, std::convert::Infallible>> + Send + 'static,
    B: Into<axum::body::Bytes> + 'static,
{
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/x-ndjson")
        .header("x-content-type-options", "nosniff")
        .header("x-accel-buffering", "no")
        .body(axum::body::Body::from_stream(stream))
        .unwrap()
}

// =============================================================================
// Preset CRUD
// =============================================================================

/// GET /api/v1/admin/tts/presets
pub async fn list_presets(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    match user_voice_presets::list_presets(&state.pool, auth.user_id).await {
        Ok(presets) => Json(json!(presets)).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to list voice presets");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to list presets"})),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreatePresetRequest {
    pub name: String,
    pub voice: Option<String>,
    pub speed: Option<f64>,
    pub temperature: Option<f64>,
    /// Transcript of the reference clip — raises clone fidelity.
    pub ref_text: Option<String>,
    /// Voice-design description ("a warm friendly young woman …").
    pub instructions: Option<String>,
    /// Built-in narrator token "S0".."S9".
    pub speaker: Option<String>,
    pub seed: Option<i64>,
    pub top_p: Option<f64>,
    pub top_k: Option<i32>,
    pub repetition_penalty: Option<f64>,
    /// 12.5 Hz FRAMES, not audio tokens.
    pub max_new_frames: Option<i32>,
}

/// POST /api/v1/admin/tts/presets
pub async fn create_preset(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreatePresetRequest>,
) -> impl IntoResponse {
    if req.name.is_empty() || req.name.len() > 100 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Name must be 1-100 characters"})),
        )
            .into_response();
    }

    // Check preset count limit
    match user_voice_presets::list_presets(&state.pool, auth.user_id).await {
        Ok(presets) if presets.len() >= MAX_PRESETS_PER_USER => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("Maximum of {} presets allowed", MAX_PRESETS_PER_USER)})),
            )
                .into_response();
        }
        Err(e) => {
            error!(error = %e, "Failed to check preset count");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Database error"})),
            )
                .into_response();
        }
        _ => {}
    }

    let input = user_voice_presets::CreatePreset {
        name: req.name,
        voice: req.voice,
        clip_filename: None,
        speed: req.speed,
        temperature: req.temperature,
        ref_text: req.ref_text,
        instructions: req.instructions,
        speaker: req.speaker,
        seed: req.seed,
        top_p: req.top_p,
        top_k: req.top_k,
        repetition_penalty: req.repetition_penalty,
        max_new_frames: req.max_new_frames,
    };

    match user_voice_presets::create_preset(&state.pool, auth.user_id, &input).await {
        Ok(preset) => (StatusCode::CREATED, Json(json!(preset))).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to create voice preset");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to create preset"})),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdatePresetRequest {
    pub name: Option<String>,
    pub voice: Option<String>,
    pub speed: Option<f64>,
    /// Tristate fields: absent = keep existing, null = clear to NULL, value = set.
    #[serde(default)]
    pub temperature: Option<Option<f64>>,
    #[serde(default)]
    pub ref_text: Option<Option<String>>,
    #[serde(default)]
    pub instructions: Option<Option<String>>,
    #[serde(default)]
    pub speaker: Option<Option<String>>,
    #[serde(default)]
    pub seed: Option<Option<i64>>,
    #[serde(default)]
    pub top_p: Option<Option<f64>>,
    #[serde(default)]
    pub top_k: Option<Option<i32>>,
    #[serde(default)]
    pub repetition_penalty: Option<Option<f64>>,
    #[serde(default)]
    pub max_new_frames: Option<Option<i32>>,
}

/// PUT /api/v1/admin/tts/presets/{preset_id}
pub async fn update_preset(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(preset_id): Path<Uuid>,
    Json(req): Json<UpdatePresetRequest>,
) -> impl IntoResponse {
    if let Some(ref name) = req.name
        && (name.is_empty() || name.len() > 100)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Name must be 1-100 characters"})),
        )
            .into_response();
    }

    // Fetch existing preset to resolve SetOrClear fields
    let existing = match user_voice_presets::get_preset(&state.pool, preset_id, auth.user_id).await
    {
        Ok(Some(p)) => p,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "Preset not found"})),
            )
                .into_response();
        }
        Err(e) => {
            error!(error = %e, "Failed to get preset for update");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Database error"})),
            )
                .into_response();
        }
    };

    let resolved = resolve_nullable_update(&req, &existing);

    let input = user_voice_presets::UpdatePreset {
        name: req.name,
        voice: req.voice,
        clip_filename: None,
        speed: req.speed,
    };

    match user_voice_presets::update_preset(&state.pool, preset_id, auth.user_id, &input, &resolved)
        .await
    {
        Ok(Some(preset)) => Json(json!(preset)).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Preset not found"})),
        )
            .into_response(),
        Err(e) => {
            error!(error = %e, "Failed to update voice preset");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to update preset"})),
            )
                .into_response()
        }
    }
}

/// DELETE /api/v1/admin/tts/presets/{preset_id}
pub async fn delete_preset(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(preset_id): Path<Uuid>,
) -> impl IntoResponse {
    // Delete clip file if it exists
    let clip_path = std::path::PathBuf::from(&*state.config.storage_root)
        .join("voices")
        .join(auth.user_id.to_string())
        .join(format!("{preset_id}.wav"));
    if clip_path.exists()
        && let Err(e) = tokio::fs::remove_file(&clip_path).await
    {
        error!(error = %e, "Failed to delete voice clip file");
    }

    match user_voice_presets::delete_preset(&state.pool, preset_id, auth.user_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Preset not found"})),
        )
            .into_response(),
        Err(e) => {
            error!(error = %e, "Failed to delete voice preset");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to delete preset"})),
            )
                .into_response()
        }
    }
}

// =============================================================================
// Voice Clip Management
// =============================================================================

/// POST /api/v1/admin/tts/presets/{preset_id}/clip
///
/// Upload a voice reference clip (WAV/MP3/OGG/M4A, max 10MB).
pub async fn upload_preset_clip(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(preset_id): Path<Uuid>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    // Verify preset exists and belongs to user
    match user_voice_presets::get_preset(&state.pool, preset_id, auth.user_id).await {
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "Preset not found"})),
            )
                .into_response();
        }
        Err(e) => {
            error!(error = %e, "Failed to check preset");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Database error"})),
            )
                .into_response();
        }
        Ok(Some(_)) => {}
    }

    // Extract clip from multipart
    let mut clip_bytes: Option<Vec<u8>> = None;
    let mut original_filename: Option<String> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("clip") {
            original_filename = field.file_name().map(|s| s.to_string());
            match field.bytes().await {
                Ok(bytes) => {
                    if bytes.len() > MAX_CLIP_SIZE {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({"error": "File too large (max 10MB)"})),
                        )
                            .into_response();
                    }
                    clip_bytes = Some(bytes.to_vec());
                }
                Err(e) => {
                    error!(error = %e, "Failed to read clip bytes");
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"error": "Failed to read upload"})),
                    )
                        .into_response();
                }
            }
            break;
        }
    }

    let clip_bytes = match clip_bytes {
        Some(bytes) => bytes,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "No clip file provided (field name: 'clip')"})),
            )
                .into_response();
        }
    };

    // Validate file extension
    if let Some(ref filename) = original_filename {
        let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
        if !ALLOWED_CLIP_EXTENSIONS.contains(&ext.as_str()) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "Unsupported format. Allowed: WAV, MP3, OGG, M4A, FLAC, OPUS, WEBM"})),
            )
                .into_response();
        }
    }

    // Save raw upload to a temp file, then convert to WAV
    let voice_dir = std::path::PathBuf::from(&*state.config.storage_root)
        .join("voices")
        .join(auth.user_id.to_string());
    if let Err(e) = tokio::fs::create_dir_all(&voice_dir).await {
        error!(error = %e, "Failed to create voices directory");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Failed to save clip"})),
        )
            .into_response();
    }

    let clip_path = voice_dir.join(format!("{preset_id}.wav"));

    // Determine if we need to convert (anything that isn't already WAV)
    let is_wav = original_filename
        .as_deref()
        .and_then(|f| f.rsplit('.').next())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"));

    if is_wav {
        // Already WAV — save directly
        if let Err(e) = tokio::fs::write(&clip_path, &clip_bytes).await {
            error!(error = %e, "Failed to write clip file");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to save clip"})),
            )
                .into_response();
        }
    } else {
        // Convert to WAV via ffmpeg
        let ext = original_filename
            .as_deref()
            .and_then(|f| f.rsplit('.').next())
            .unwrap_or("mp3");
        let tmp_input = voice_dir.join(format!("{preset_id}_upload.{ext}"));
        if let Err(e) = tokio::fs::write(&tmp_input, &clip_bytes).await {
            error!(error = %e, "Failed to write temp clip file");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to save clip"})),
            )
                .into_response();
        }

        if let Err(e) = crate::audio::convert::convert_to_wav(&tmp_input, &clip_path).await {
            let _ = tokio::fs::remove_file(&tmp_input).await;
            error!(error = %e, "Failed to convert clip to WAV");
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("Failed to convert audio to WAV: {e}")})),
            )
                .into_response();
        }
        let _ = tokio::fs::remove_file(&tmp_input).await;
    }

    let clip_filename = original_filename.unwrap_or_else(|| format!("{preset_id}.wav"));
    finalize_preset_clip(
        &state,
        auth.user_id,
        preset_id,
        &clip_path,
        clip_filename,
        None,
    )
    .await
}

/// Read a WAV at `clip_path`, register it with the TTS service, and persist the
/// `clip_filename` on the preset row. Caller is responsible for verifying preset
/// ownership and writing the WAV file beforehand. `ref_text` is engine-native
/// ICL transcript text; ignored by chatterbox.
async fn finalize_preset_clip(
    state: &AppState,
    user_id: Uuid,
    preset_id: Uuid,
    clip_path: &std::path::Path,
    clip_filename: String,
    ref_text: Option<&str>,
) -> axum::response::Response {
    let mut wav_bytes = match tokio::fs::read(clip_path).await {
        Ok(b) => b,
        Err(e) => {
            error!(error = %e, "Failed to read WAV clip");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to read clip"})),
            )
                .into_response();
        }
    };

    // Peak-normalise to -1 dBFS so quiet phone-mic recordings don't produce
    // quiet TTS output (breeze and chatterbox both clone the reference
    // envelope). Persist back to disk so subsequent re-registrations via
    // ensure_voice_registered upload the normalised version.
    match kob_audio_norm::peak_normalize_wav_in_place(
        &mut wav_bytes,
        kob_audio_norm::DEFAULT_TARGET_PEAK_DBFS,
    ) {
        Ok(report) if report.scale > 1.0 => {
            tracing::info!(
                scale = report.scale,
                peak_before = report.peak_before,
                peak_after = report.peak_after,
                "peak-normalised voice clip"
            );
            if let Err(e) = tokio::fs::write(clip_path, &wav_bytes).await {
                error!(error = %e, "Failed to persist normalised clip; using in-memory copy for upload");
            }
        }
        Ok(_) => {
            // Already at or above target peak — leave the file untouched.
        }
        Err(e) => {
            tracing::warn!(error = %e, "skipping peak normalisation; uploading raw clip");
        }
    }

    let config = state.runtime_config.get().await;
    let service_url_owned = config
        .effective_tts_url()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_TTS_URL.to_string());
    let backend = TtsBackend::from_config(&config.tts_service_type);
    let voice_name = format!("preset-{preset_id}");

    if let Err(e) = tts::upload_voice(
        service_url_owned.as_str(),
        backend,
        &voice_name,
        &wav_bytes,
        ref_text,
    )
    .await
    {
        error!(error = %e, "Failed to register voice with TTS service");
        // Don't fail — the file is saved and can be re-registered later
    }

    // Preserve every nullable column on a clip-only update.
    let existing = user_voice_presets::get_preset(&state.pool, preset_id, user_id).await;
    let resolved = match &existing {
        Ok(Some(p)) => user_voice_presets::ResolvedNullable::from_preset(p),
        _ => user_voice_presets::ResolvedNullable::default(),
    };
    let update = user_voice_presets::UpdatePreset {
        name: None,
        voice: None,
        clip_filename: Some(clip_filename),
        speed: None,
    };

    match user_voice_presets::update_preset(&state.pool, preset_id, user_id, &update, &resolved)
        .await
    {
        Ok(Some(preset)) => Json(json!(preset)).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Preset not found"})),
        )
            .into_response(),
        Err(e) => {
            error!(error = %e, "Failed to update preset clip_filename");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Clip saved but failed to update database"})),
            )
                .into_response()
        }
    }
}

/// POST /api/v1/admin/tts/presets/{preset_id}/transcribe-clip
///
/// Run the saved voice clip through the configured STT service and return
/// the transcript. Used by the voice page to autofill `ref_text` before
/// the user saves it. No persistence — the caller decides whether to PUT
/// the result via update_preset.
pub async fn transcribe_preset_clip(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(preset_id): Path<Uuid>,
) -> Response {
    match user_voice_presets::get_preset(&state.pool, preset_id, auth.user_id).await {
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "Preset not found"})),
            )
                .into_response();
        }
        Err(e) => {
            error!(error = %e, "Failed to check preset");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Database error"})),
            )
                .into_response();
        }
        Ok(Some(_)) => {}
    }

    let clip_path = std::path::PathBuf::from(&*state.config.storage_root)
        .join("voices")
        .join(auth.user_id.to_string())
        .join(format!("{preset_id}.wav"));

    if !clip_path.is_file() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "no clip saved for this preset"})),
        )
            .into_response();
    }

    let config = state.runtime_config.get().await;
    let Some(stt_url) = config.effective_stt_url().filter(|s| !s.is_empty()) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "STT service not configured"})),
        )
            .into_response();
    };

    match crate::audio::stt::transcribe_audio(&stt_url, &clip_path).await {
        Ok(result) => {
            let text: String = result
                .segments
                .into_iter()
                .map(|s| s.text)
                .collect::<Vec<_>>()
                .join(" ");
            Json(json!({ "text": text.trim() })).into_response()
        }
        Err(e) => {
            error!(error = %e, "STT transcribe_audio failed for preset clip");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("STT failed: {e}")})),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct PreviewSampleRequest {
    pub text: String,
    pub voice: Option<String>,
    /// All TtsParams shape fields are optional; a missing seed = service-random.
    pub speed: Option<f64>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub top_k: Option<u32>,
    pub repetition_penalty: Option<f64>,
    pub seed: Option<i64>,
    pub instructions: Option<String>,
    pub speaker: Option<String>,
    pub max_new_frames: Option<u32>,
    /// "wav" | "mp3" — defaults to mp3 for the audition flow.
    pub response_format: Option<String>,
}

/// POST /api/v1/admin/tts/preview-sample
///
/// Synthesise a single one-shot sample with the supplied voice/params and
/// return the audio blob. Drives the voice page's pre-save seed audition
/// (call N times in parallel with different `seed` values, browser plays each).
/// No persistence and no preset wiring — entirely transient.
pub async fn preview_sample(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(req): Json<PreviewSampleRequest>,
) -> Response {
    let config = state.runtime_config.get().await;
    if !config.tts_enabled {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "TTS is not enabled"})),
        )
            .into_response();
    }

    if req.text.is_empty() || req.text.len() > 4_000 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Text must be 1-4000 characters"})),
        )
            .into_response();
    }

    let service_url = config
        .effective_tts_url()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_TTS_URL.to_string());
    let backend = TtsBackend::from_config(&config.tts_service_type);

    let voice = req.voice.unwrap_or_else(|| {
        config
            .tts_default_voice
            .clone()
            .unwrap_or_else(|| fallback_voice(backend))
    });

    let speech_endpoint = tts::SpeechEndpoint::resolve(
        config.tts_routes_via_koblem(),
        config.koblem_api_key.as_deref(),
    );
    // Reported back as the content type, so it must be what was actually
    // rendered — the keyless koblem fallback cannot encode mp3.
    let response_format = speech_endpoint
        .container_for(req.response_format.as_deref().unwrap_or("mp3"))
        .to_string();
    let params = TtsParams {
        speed: req.speed,
        temperature: req.temperature,
        top_p: req.top_p,
        top_k: req.top_k,
        repetition_penalty: req.repetition_penalty,
        seed: req.seed,
        instructions: req.instructions,
        speaker: req.speaker,
        max_new_frames: req.max_new_frames,
        response_format: Some(response_format.clone()),
    };

    let gpu_lock = state.gpu_lock.clone();
    gpu_lock.set_enabled(config.gpu_preempt_tts);
    gpu_lock.set_service_urls(
        config.stt_service_url.clone(),
        config.tts_service_url.clone(),
        config.vision_service_url.clone(),
    );
    let _gpu_guard = gpu_lock.acquire_for_tts().await;

    match tts::synthesize_sentence(
        &service_url,
        backend,
        speech_endpoint,
        &req.text,
        &voice,
        &params,
    )
    .await
    {
        Ok(audio) => {
            let content_type = match response_format.as_str() {
                "wav" => "audio/wav",
                "ogg" => "audio/ogg",
                _ => "audio/mpeg",
            };
            axum::response::Response::builder()
                .status(StatusCode::OK)
                .header("content-type", content_type)
                .header("cache-control", "no-store")
                // What the render actually ran at, which is not necessarily the
                // rate that was asked for.
                .header("x-speed-applied", audio.speed_applied.to_string())
                .body(axum::body::Body::from(audio.bytes))
                .unwrap()
        }
        Err(e) => {
            error!(error = %e, "preview_sample synth failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("synth failed: {e}")})),
            )
                .into_response()
        }
    }
}

/// DELETE /api/v1/admin/tts/presets/{preset_id}/clip
pub async fn delete_preset_clip(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(preset_id): Path<Uuid>,
) -> impl IntoResponse {
    // Verify preset exists and belongs to user
    match user_voice_presets::get_preset(&state.pool, preset_id, auth.user_id).await {
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "Preset not found"})),
            )
                .into_response();
        }
        Err(e) => {
            error!(error = %e, "Failed to check preset");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Database error"})),
            )
                .into_response();
        }
        Ok(Some(_)) => {}
    }

    // Delete clip file
    let clip_path = std::path::PathBuf::from(&*state.config.storage_root)
        .join("voices")
        .join(auth.user_id.to_string())
        .join(format!("{preset_id}.wav"));
    if clip_path.exists()
        && let Err(e) = tokio::fs::remove_file(&clip_path).await
    {
        error!(error = %e, "Failed to delete voice clip file");
    }

    // Clear clip_filename — COALESCE-based update won't clear it, so use a direct query.
    let result = sqlx::query(
        "UPDATE user_voice_presets SET clip_filename = NULL, updated_at = NOW() WHERE id = $1 AND user_id = $2",
    )
    .bind(preset_id)
    .bind(auth.user_id)
    .execute(&state.pool)
    .await;

    match result {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            error!(error = %e, "Failed to clear preset clip_filename");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to clear clip"})),
            )
                .into_response()
        }
    }
}

/// GET /api/v1/admin/tts/presets/{preset_id}/clip
///
/// Serve the voice clip WAV file.
pub async fn get_preset_clip(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(preset_id): Path<Uuid>,
) -> impl IntoResponse {
    // Verify preset exists and belongs to user
    match user_voice_presets::get_preset(&state.pool, preset_id, auth.user_id).await {
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "Preset not found"})),
            )
                .into_response();
        }
        Err(e) => {
            error!(error = %e, "Failed to check preset");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Database error"})),
            )
                .into_response();
        }
        Ok(Some(_)) => {}
    }

    let clip_path = std::path::PathBuf::from(&*state.config.storage_root)
        .join("voices")
        .join(auth.user_id.to_string())
        .join(format!("{preset_id}.wav"));

    match tokio::fs::read(&clip_path).await {
        Ok(bytes) => axum::response::Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "audio/wav")
            .header(
                "content-disposition",
                format!("inline; filename=\"{preset_id}.wav\""),
            )
            .body(axum::body::Body::from(bytes))
            .unwrap()
            .into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "No clip file found"})),
        )
            .into_response(),
    }
}

// =============================================================================
// Active Preset
// =============================================================================

/// GET /api/v1/admin/tts/presets/active
pub async fn get_active_preset(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    match user_voice_presets::get_active_preset(&state.pool, auth.user_id).await {
        Ok(preset) => Json(json!(preset)).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to get active voice preset");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to get active preset"})),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SetActivePresetRequest {
    pub preset_id: Option<Uuid>,
}

/// PUT /api/v1/admin/tts/presets/active
pub async fn set_active_preset(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<SetActivePresetRequest>,
) -> impl IntoResponse {
    // If setting (not clearing), verify the preset exists and belongs to user
    if let Some(preset_id) = req.preset_id {
        match user_voice_presets::get_preset(&state.pool, preset_id, auth.user_id).await {
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error": "Preset not found"})),
                )
                    .into_response();
            }
            Err(e) => {
                error!(error = %e, "Failed to check preset");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "Database error"})),
                )
                    .into_response();
            }
            Ok(Some(_)) => {}
        }
    }

    match user_voice_presets::set_active_preset(&state.pool, auth.user_id, req.preset_id).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            error!(error = %e, "Failed to set active voice preset");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to set active preset"})),
            )
                .into_response()
        }
    }
}

// =============================================================================
// Voice clip from library audiobook
// =============================================================================

use crate::audio::clip::{self as clip_helpers, MAX_CLIP_SECS as MAX_AUDIOBOOK_CLIP_SECS};

struct ResolvedChapter {
    abs_path: std::path::PathBuf,
    book_title: String,
}

async fn resolve_audiobook_chapter(
    state: &AppState,
    audiobook_id: &str,
    chapter_index: u32,
) -> Result<ResolvedChapter, Response> {
    let book_uuid = Uuid::parse_str(audiobook_id).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid audiobook_id"})),
        )
            .into_response()
    })?;

    let title: String = sqlx::query_scalar!("SELECT title FROM books WHERE id = $1", book_uuid)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| {
            error!(error = %e, "audiobook title lookup failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "db error"})),
            )
                .into_response()
        })?
        .ok_or_else(|| -> Response {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "audiobook not found"})),
            )
                .into_response()
        })?;

    let files = audiobook_files::list_by_book(&state.pool, book_uuid)
        .await
        .map_err(|e| -> Response {
            error!(error = %e, "audiobook_files list failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "db error"})),
            )
                .into_response()
        })?;

    let file = files
        .into_iter()
        .nth(chapter_index as usize)
        .ok_or_else(|| -> Response {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "chapter index out of range"})),
            )
                .into_response()
        })?;

    let abs_path =
        crate::audio::path::resolve_file_path(&state.config.storage_root, &file.file_path);
    if !abs_path.is_file() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "audiobook file missing on disk"})),
        )
            .into_response());
    }

    Ok(ResolvedChapter {
        abs_path,
        book_title: title,
    })
}

fn default_chapter_index(chapter_count: usize) -> u32 {
    if chapter_count >= 3 { 1 } else { 0 }
}

#[derive(Debug, Deserialize)]
pub struct AudiobookSuggestQuery {
    pub audiobook_id: String,
    #[serde(default)]
    pub chapter_index: Option<u32>,
    #[serde(default)]
    pub duration_seconds: Option<f64>,
}

/// GET /api/v1/admin/tts/audiobook-clip/suggest
///
/// Pick a peaky window in the chapter and return its offset.
pub async fn suggest_audiobook_clip(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(q): Query<AudiobookSuggestQuery>,
) -> Response {
    let duration = q.duration_seconds.unwrap_or(30.0);
    if !(0.0..=MAX_AUDIOBOOK_CLIP_SECS).contains(&duration) || duration <= 0.0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "duration_seconds out of range"})),
        )
            .into_response();
    }

    let book_uuid = match Uuid::parse_str(&q.audiobook_id) {
        Ok(u) => u,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid audiobook_id"})),
            )
                .into_response();
        }
    };

    let files = match audiobook_files::list_by_book(&state.pool, book_uuid).await {
        Ok(f) => f,
        Err(e) => {
            error!(error = %e, "audiobook_files list failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "db error"})),
            )
                .into_response();
        }
    };
    if files.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "audiobook has no files"})),
        )
            .into_response();
    }
    let chapter_index = q
        .chapter_index
        .unwrap_or_else(|| default_chapter_index(files.len()));
    let file = match files.into_iter().nth(chapter_index as usize) {
        Some(f) => f,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "chapter index out of range"})),
            )
                .into_response();
        }
    };
    let abs_path =
        crate::audio::path::resolve_file_path(&state.config.storage_root, &file.file_path);
    if !abs_path.is_file() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "audiobook file missing on disk"})),
        )
            .into_response();
    }
    let peaks_path = std::path::PathBuf::from(&*state.config.storage_root)
        .join("audio_peaks")
        .join(format!("{}.peaks", file.id));

    match crate::audio::peaky::pick_peaky_window_with_peaks(&abs_path, Some(&peaks_path), duration)
        .await
    {
        Ok(res) => Json(json!({
            "audiobook_id": q.audiobook_id,
            "chapter_index": chapter_index,
            "offset_seconds": res.offset_seconds,
            "duration_seconds": res.duration_seconds,
            "chapter_total_seconds": res.total_seconds,
        }))
        .into_response(),
        Err(e) => {
            error!(error = %e, "peaky pick failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("peaky pick failed: {e}")})),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AudiobookClipQuery {
    pub audiobook_id: String,
    pub chapter_index: u32,
    pub offset_seconds: f64,
    pub duration_seconds: f64,
}

fn validate_clip_range(offset_seconds: f64, duration_seconds: f64) -> Option<Response> {
    if !offset_seconds.is_finite() || offset_seconds < 0.0 {
        return Some(
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "offset_seconds must be a finite, non-negative number"})),
            )
                .into_response(),
        );
    }
    if !duration_seconds.is_finite()
        || duration_seconds <= 0.0
        || duration_seconds > MAX_AUDIOBOOK_CLIP_SECS
    {
        return Some(
            (
                StatusCode::BAD_REQUEST,
                Json(
                    json!({"error": format!("duration_seconds must be in (0, {}]", MAX_AUDIOBOOK_CLIP_SECS)}),
                ),
            )
                .into_response(),
        );
    }
    None
}

/// Adapter wrapping `clip_helpers::cut_audiobook_segment` with this
/// route-layer's `Response` error shape.
async fn cut_audiobook_segment(
    abs_path: std::path::PathBuf,
    output_path: std::path::PathBuf,
    offset_seconds: f64,
    duration_seconds: f64,
) -> Result<(), Response> {
    clip_helpers::cut_audiobook_segment(abs_path, output_path, offset_seconds, duration_seconds)
        .await
        .map_err(|e| {
            error!(error = %e, "audiobook segment cut failed");
            match e {
                clip_helpers::ClipError::Ffmpeg(_) => (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({"error": format!("ffmpeg cut failed: {e}")})),
                )
                    .into_response(),
                _ => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "internal error"})),
                )
                    .into_response(),
            }
        })
}

/// GET /api/v1/admin/tts/audiobook-clip
///
/// Stream a WAV preview of the requested chapter window. Used by the voice-preset
/// UI to let the user audition a candidate clip before committing it.
pub async fn get_audiobook_clip(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(q): Query<AudiobookClipQuery>,
) -> Response {
    if let Some(resp) = validate_clip_range(q.offset_seconds, q.duration_seconds) {
        return resp;
    }

    let resolved = match resolve_audiobook_chapter(&state, &q.audiobook_id, q.chapter_index).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => {
            error!(error = %e, "tempdir create failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "tempdir create failed"})),
            )
                .into_response();
        }
    };
    let tmp_path = dir.path().join("clip.wav");

    if let Err(resp) = cut_audiobook_segment(
        resolved.abs_path.clone(),
        tmp_path.clone(),
        q.offset_seconds,
        q.duration_seconds,
    )
    .await
    {
        return resp;
    }

    let bytes = match tokio::fs::read(&tmp_path).await {
        Ok(b) => b,
        Err(e) => {
            error!(error = %e, "read tempfile failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "read failed"})),
            )
                .into_response();
        }
    };

    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "audio/wav")
        .header("cache-control", "no-store")
        .body(axum::body::Body::from(bytes))
        .unwrap()
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct AudiobookClipCommitRequest {
    pub audiobook_id: String,
    pub chapter_index: u32,
    pub offset_seconds: f64,
    pub duration_seconds: f64,
}

/// POST /api/v1/admin/tts/presets/{preset_id}/clip-from-audiobook
///
/// Cut a window from a library audiobook chapter and save it as the preset's
/// reference clip. Mirrors the disk + DB tail of [`upload_preset_clip`].
pub async fn create_preset_clip_from_audiobook(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(preset_id): Path<Uuid>,
    Json(req): Json<AudiobookClipCommitRequest>,
) -> Response {
    if let Some(resp) = validate_clip_range(req.offset_seconds, req.duration_seconds) {
        return resp;
    }

    match user_voice_presets::get_preset(&state.pool, preset_id, auth.user_id).await {
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "Preset not found"})),
            )
                .into_response();
        }
        Err(e) => {
            error!(error = %e, "Failed to check preset");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Database error"})),
            )
                .into_response();
        }
        Ok(Some(_)) => {}
    }

    let resolved =
        match resolve_audiobook_chapter(&state, &req.audiobook_id, req.chapter_index).await {
            Ok(r) => r,
            Err(resp) => return resp,
        };

    let voice_dir = std::path::PathBuf::from(&*state.config.storage_root)
        .join("voices")
        .join(auth.user_id.to_string());
    if let Err(e) = tokio::fs::create_dir_all(&voice_dir).await {
        error!(error = %e, "Failed to create voices directory");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Failed to save clip"})),
        )
            .into_response();
    }
    let clip_path = voice_dir.join(format!("{preset_id}.wav"));

    if let Err(resp) = cut_audiobook_segment(
        resolved.abs_path.clone(),
        clip_path.clone(),
        req.offset_seconds,
        req.duration_seconds,
    )
    .await
    {
        return resp;
    }

    let clip_filename = format!(
        "{} ch{}.wav",
        sanitize_for_filename(&resolved.book_title),
        req.chapter_index + 1
    );
    finalize_preset_clip(
        &state,
        auth.user_id,
        preset_id,
        &clip_path,
        clip_filename,
        None,
    )
    .await
}

fn sanitize_for_filename(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect()
}
