//! Generic OpenAI-compatible TTS client.
//!
//! Talks to whatever TTS service is at the configured URL via the standard
//! OpenAI `/v1/audio/speech` and `/v1/audio/voices` endpoints. Two backend
//! shapes diverge enough to warrant per-backend branches:
//!
//! - Chatterbox: `POST /voices` with `voice_name`/`voice_file`; voice list as
//!   `{"voices": [{"id":"...","name":"..."}, ...]}`.
//! - Breeze (`tts-breeze`, the engine koblem serves): `POST /v1/audio/voices`
//!   with `name`/`audio_sample` + `ref_text`; voice list carries both a
//!   `{"<model_id>": [...]}` map and a richer `{"voices":[{id,...}]}` array.
//!
//! Two engine behaviours shape every request built here: `long` defaults true
//! server-side, so text is NEVER pre-chunked, and `speed` is a real SERVER-SIDE
//! render parameter, so the player must not re-apply it — audio and word
//! timings both arrive in the rendered timebase. An engine that cannot do the
//! requested rate rejects the request outright, so every speed-carrying request
//! here falls back to a 1.0 re-render rather than failing, and reports what it
//! actually rendered at.

use std::fmt;
use std::sync::LazyLock;

use base64::Engine;
use bytes::Bytes;
use futures::Stream;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_stream::StreamExt;
use tracing::{error, warn};

use crate::http;

/// Which TTS backend the service URL points at. Selected by `tts_service_type`
/// in the runtime config and threaded into every per-request helper so the
/// helper picks the right wire shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsBackend {
    Chatterbox,
    /// BreezeTTS 2 (`tts-breeze`) — the engine koblem serves, and the only one
    /// with a forced aligner (so the only one that drives the read-along).
    Breeze,
    Kokoro,
    OpenAi,
    /// Probe the service to detect; falls back to chatterbox-compatible shapes.
    Auto,
}

impl TtsBackend {
    pub fn from_config(s: &str) -> Self {
        match s {
            "breeze" => TtsBackend::Breeze,
            "chatterbox" => TtsBackend::Chatterbox,
            "kokoro" => TtsBackend::Kokoro,
            "openai" => TtsBackend::OpenAi,
            _ => TtsBackend::Auto,
        }
    }

    /// Engine label surfaced to the frontend so it can gate engine-specific UI
    /// without re-probing.
    pub fn engine_label(self) -> &'static str {
        match self {
            TtsBackend::Chatterbox => "chatterbox",
            TtsBackend::Breeze => "breeze",
            TtsBackend::Kokoro => "kokoro",
            TtsBackend::OpenAi => "openai",
            TtsBackend::Auto => "auto",
        }
    }

    /// True for engines exposing the native voice endpoints
    /// (`/v1/audio/voices`, DELETE, multipart name/audio_sample clone).
    fn uses_native_voice_api(self) -> bool {
        matches!(self, TtsBackend::Breeze)
    }

    /// True for engines that support word-level forced alignment (the
    /// read-along highlight). Only `tts-breeze` runs an aligner and emits the
    /// `speech.audio.alignment.{partial,final}` SSE events the reader's
    /// streaming path consumes. Other backends fall back to the per-sentence
    /// path (audio, no highlight).
    pub fn supports_alignment(self) -> bool {
        matches!(self, TtsBackend::Breeze)
    }
}

/// Generation parameters. `temperature` is shared across backends; the rest
/// are engine-native and ignored by the chatterbox path.
///
/// Deliberately absent: `language` (the engine is en/zh and infers from the
/// text).
#[derive(Debug, Clone, Default)]
pub struct TtsParams {
    /// Render rate. `None` or `1.0` renders at natural rate. Anything else is a
    /// real re-render at that rate — changing it means synthesising again — and
    /// an engine that cannot do it falls back to 1.0 rather than failing, so
    /// callers must report the APPLIED rate, never the requested one.
    pub speed: Option<f64>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub top_k: Option<u32>,
    pub repetition_penalty: Option<f64>,
    pub seed: Option<i64>,
    pub instructions: Option<String>,
    /// Built-in speaker token `"S0"`..`"S9"`.
    pub speaker: Option<String>,
    /// Cap in 12.5 Hz FRAMES, not audio tokens. A stored pre-breeze value of
    /// 2048 means 2048 frames here and overflows the context window, so this is
    /// clamped on the way in — see `user_voice_presets`.
    pub max_new_frames: Option<u32>,
    /// Override default `mp3` for the read-aloud sentence path.
    pub response_format: Option<String>,
}

/// A voice available from the TTS service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsVoice {
    pub id: String,
    pub name: String,
}

/// Status response for the TTS subsystem.
#[derive(Debug, Serialize)]
pub struct TtsStatusResponse {
    pub enabled: bool,
    pub service_url: Option<String>,
    pub voices: Vec<TtsVoice>,
    pub default_voice: Option<String>,
    /// Backend label so the frontend can gate engine-specific UI sections
    /// without re-probing. One of: "chatterbox", "breeze", "kokoro", "openai",
    /// "auto".
    pub engine: String,
    /// Whether the TTS service supports voice-cloning params (temperature,
    /// voice clips). True for chatterbox AND breeze; false for kokoro/openai.
    pub supports_emotion: bool,
    /// Whether the engine supports word-level forced alignment (the read-along
    /// highlight). True for breeze only. Lets the reader UI
    /// gate the highlight without re-probing.
    pub supports_alignment: bool,
}

/// Errors from TTS operations.
#[derive(Debug)]
pub enum TtsError {
    /// HTTP/network error communicating with the TTS service.
    Http(reqwest::Error),
    /// The TTS service returned a non-success status.
    ServiceError { status: u16, body: String },
    /// The TTS service is not reachable or not configured.
    Unavailable(String),
}

impl fmt::Display for TtsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // The cause chain, not just the top line. reqwest reports a severed
            // body as the bare "error decoding response body" whatever actually
            // went wrong, so a timeout, a reset and genuinely corrupt framing
            // all surface as the same sentence — and the one that reaches the
            // reader reads like a parser bug. The source says which it was.
            TtsError::Http(e) => {
                write!(f, "TTS HTTP error: {e}")?;
                let mut src = std::error::Error::source(e);
                while let Some(cause) = src {
                    write!(f, ": {cause}")?;
                    src = cause.source();
                }
                Ok(())
            }
            TtsError::ServiceError { status, body } => {
                write!(f, "TTS service error ({status}): {body}")
            }
            TtsError::Unavailable(msg) => write!(f, "TTS unavailable: {msg}"),
        }
    }
}

impl std::error::Error for TtsError {}

impl From<reqwest::Error> for TtsError {
    fn from(e: reqwest::Error) -> Self {
        TtsError::Http(e)
    }
}

/// Single word's timing from the forced aligner. `t0_ms` / `t1_ms` are
/// offsets from the start of the synthesized audio for the paragraph being
/// streamed; 80 ms is the underlying resolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordTiming {
    pub word_index: u32,
    pub text: String,
    pub t0_ms: u32,
    pub t1_ms: u32,
}

/// Typed event from a streaming paragraph synth — what
/// `synthesize_paragraph_stream` yields to the caller.
#[derive(Debug, Clone)]
pub enum TtsStreamEvent {
    /// Always first, emitted once the upstream response headers land and before
    /// any audio. Carries the rate the render actually ran at, which is the
    /// requested one unless the engine refused it and fell back to 1.0.
    Started { speed_applied: f64 },
    /// A chunk of 16-bit-LE PCM audio at 24 kHz, decoded from a
    /// `speech.audio.delta` SSE event.
    Audio { pcm: Bytes },
    /// Interleaved forced-aligner update. `words` re-emits the full word
    /// list for the paragraph with timings refreshed against audio
    /// produced so far; clients should overwrite by `word_index`.
    AlignmentPartial {
        audio_seen_ms: u32,
        words: Vec<WordTiming>,
    },
    /// Final, locked alignment for the paragraph. Arrives after the last
    /// `Audio` event, before `Done`.
    AlignmentFinal {
        audio_total_ms: u32,
        words: Vec<WordTiming>,
    },
    /// A long-form chunk boundary: the engine is about to speak `text`, which
    /// starts `word_offset` words into this request's word list and
    /// `audio_offset_ms` into its audio.
    ///
    /// Independent of the forced aligner — the engine already knew where its
    /// own sentence boundaries were. It is the only position signal a reader
    /// gets when alignment is off or still catching up, and being sentence-
    /// aligned it never lands mid-word.
    Chunk {
        index: u32,
        chunks_total: u32,
        word_offset: u32,
        word_count: u32,
        audio_offset_ms: u32,
        text: String,
    },
    /// `speech.audio.done` — stream complete. `truncated` is the engine's own
    /// flag: generation stopped at `max_new_frames` mid-sentence and the audio
    /// is a valid but SHORT clip. It cannot happen while `long` is on, which is
    /// always, so it is surfaced rather than silently played.
    ///
    /// `cancelled` is the engine abandoning the render part way — it stops on
    /// the chunk it had reached and returns what it has, which looks exactly
    /// like a finished read: clean stream close, real audio, no error. Only
    /// this flag separates half a chapter from a whole one, so it is carried
    /// out rather than folded into `truncated`: the two mean different things
    /// and the aligner treats them differently (a cancelled render publishes no
    /// final alignment).
    Done { truncated: bool, cancelled: bool },
}

/// The engine's supported render-rate range. Outside it is a 400, and a 400
/// costs the caller their rate entirely (the fallback re-renders at 1x), so a
/// request is clamped into range instead: someone who asked for 6x is better
/// served by the fastest rate that exists than by being dropped back to 1x.
const SPEED_RANGE: std::ops::RangeInclusive<f64> = 0.25..=4.0;

/// The rate to put on the wire, or `None` for natural rate.
///
/// Natural rate is omitted entirely rather than sent as an explicit `1.0`: an
/// unconditional field is one engine-validation change away from rejecting
/// every render, and this is the only place that decision is made.
fn wire_speed(params: &TtsParams) -> Option<f64> {
    let v = params
        .speed
        .unwrap_or(1.0)
        .clamp(*SPEED_RANGE.start(), *SPEED_RANGE.end());
    ((v - 1.0).abs() > f64::EPSILON).then_some(v)
}

/// Whether a failed synth should be retried with `speed` dropped.
///
/// A rate the engine cannot render is rejected with 400, which would turn "I
/// wanted 1.5x" into no audio at all. Only a 400 on a request that actually
/// asked for a non-natural rate qualifies: every other status is a real failure,
/// and a 400 on a request with nothing to drop would just repeat itself. Mirrors
/// the same rule in koblem's `tts_breeze`, which owns the fallback whenever the
/// request is routed through it.
fn retry_at_natural_speed(status: u16, requested_speed: f64) -> bool {
    status == 400 && (requested_speed - 1.0).abs() > f64::EPSILON
}

/// Read the rate a response says it rendered at.
///
/// koblem sets `x-speed-applied` because it owns the fallback upstream; a
/// direct-to-engine response has no such header, in which case a 2xx means the
/// engine accepted the rate we asked for.
fn speed_applied_of(response: &reqwest::Response, requested_speed: f64) -> f64 {
    response
        .headers()
        .get("x-speed-applied")
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(requested_speed)
}

/// Fetch available voices from the TTS service.
///
/// Tries `GET {service_url}/v1/audio/voices` first, falling back to
/// `GET {service_url}/voices` for older chatterbox builds. Handles three
/// response shapes (parsed in this order):
/// - Legacy map: `{"<model_id>": ["v1", "v2", ...]}` — single top-level key whose
///   value is a string array. The string is used as both id and name.
/// - Object list: `{"voices": [{"id": "...", "name": "..."}, ...]}`
/// - String list: `{"voices": ["id1", "id2", ...]}` (e.g. Kokoro). For string
///   IDs the ID is used as both `id` and `name`.
pub async fn get_voices(service_url: &str) -> Vec<TtsVoice> {
    let client = http::new_client();
    let base = service_url.trim_end_matches('/');

    // Try /v1/audio/voices first, fall back to /voices
    let resp = match client.get(format!("{base}/v1/audio/voices")).send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(_) | Err(_) => match client.get(format!("{base}/voices")).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                error!(status = %r.status(), "TTS voices endpoint returned error");
                return Vec::new();
            }
            Err(e) => {
                error!(error = %e, "Failed to connect to TTS service");
                return Vec::new();
            }
        },
    };

    let body = match resp.text().await {
        Ok(b) => b,
        Err(e) => {
            error!(error = %e, "Failed to read TTS voices response body");
            return Vec::new();
        }
    };

    parse_voices_body(&body)
}

/// Pure parser for the three voice-list response shapes. Split out from
/// `get_voices` so unit tests can exercise it without HTTP.
pub(crate) fn parse_voices_body(body: &str) -> Vec<TtsVoice> {
    #[derive(Deserialize)]
    struct VoiceObject {
        id: String,
        name: Option<String>,
    }

    #[derive(Deserialize)]
    struct ObjectWrapper {
        voices: Vec<VoiceObject>,
    }

    #[derive(Deserialize)]
    struct StringWrapper {
        voices: Vec<String>,
    }

    // Legacy map shape first — most distinctive (no top-level `voices` key).
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(body)
        && map.len() == 1
        && let Some((_model_id, val)) = map.iter().next()
        && let Some(arr) = val.as_array()
        && arr.iter().all(|v| v.is_string())
    {
        let mut voices: Vec<TtsVoice> = arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| TtsVoice {
                id: s.to_string(),
                name: s.to_string(),
            })
            .collect();
        voices.sort_by(|a, b| a.name.cmp(&b.name));
        return voices;
    }

    if let Ok(parsed) = serde_json::from_str::<ObjectWrapper>(body) {
        let mut voices: Vec<TtsVoice> = parsed
            .voices
            .into_iter()
            .map(|v| TtsVoice {
                name: v.name.unwrap_or_else(|| v.id.clone()),
                id: v.id,
            })
            .collect();
        voices.sort_by(|a, b| a.name.cmp(&b.name));
        return voices;
    }

    if let Ok(parsed) = serde_json::from_str::<StringWrapper>(body) {
        let mut voices: Vec<TtsVoice> = parsed
            .voices
            .into_iter()
            .map(|id| TtsVoice {
                name: id.clone(),
                id,
            })
            .collect();
        voices.sort_by(|a, b| a.name.cmp(&b.name));
        return voices;
    }

    warn!("Could not parse TTS voices response: {body}");
    Vec::new()
}

/// One sentence of rendered audio, plus the rate it was rendered at.
///
/// The rate is carried out rather than assumed because a refused `speed` is the
/// one failure that looks exactly like success: real audio comes back, just at
/// a different rate than the caller asked for.
#[derive(Debug, Clone)]
pub struct SentenceAudio {
    pub bytes: bytes::Bytes,
    pub speed_applied: f64,
}

/// Which buffered-synth endpoint a request goes to, and what that endpoint can
/// encode.
///
/// Both speak the same body shape, so this only picks a path, an auth header,
/// and a container ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechEndpoint<'a> {
    /// The engine's own `/v1/audio/speech`. Encodes every container it
    /// supports, needs no key.
    Engine,
    /// koblem's mirror of that path — same shape, NARROWER: it renders over SSE
    /// upstream (peak VRAM flat in output length) and hands back what it
    /// collected, so it serves `wav`/`pcm` only. Not an oversight to route
    /// around: breeze cannot stream a compressed container at all ("mp3 needs a
    /// streaming encoder we do not have"), so mp3 exists only on a buffered
    /// render. Reached only when koblem has no API key configured.
    KoblemTier1,
    /// koblem tier-2 `/api/v1/tts/generate`, which DOES buffer upstream and so
    /// serves mp3/ogg/aac. Auth is a property of the `/api/v1/` prefix, not of
    /// the tier: nginx exposes `/api/` publicly, so everything under it takes a
    /// key while the LAN-only `/v1/` twins do not. Tier-1 publishes both
    /// spellings; tier-2 has no open twin, so this path needs the key.
    KoblemTier2 { api_key: &'a str },
}

impl<'a> SpeechEndpoint<'a> {
    /// Pick the endpoint for a kobbler that may be routing TTS through koblem.
    /// Without a koblem key there is no tier-2 to reach, so such a caller falls
    /// back to the tier-1 mirror and loses compressed containers with it.
    pub fn resolve(via_koblem: bool, koblem_api_key: Option<&'a str>) -> Self {
        match (via_koblem, koblem_api_key.filter(|k| !k.is_empty())) {
            (false, _) => SpeechEndpoint::Engine,
            (true, Some(api_key)) => SpeechEndpoint::KoblemTier2 { api_key },
            (true, None) => SpeechEndpoint::KoblemTier1,
        }
    }

    fn path(self) -> &'static str {
        match self {
            SpeechEndpoint::Engine | SpeechEndpoint::KoblemTier1 => "/v1/audio/speech",
            SpeechEndpoint::KoblemTier2 { .. } => "/api/v1/tts/generate",
        }
    }

    /// Clamp a requested container to what this endpoint can encode. Only the
    /// tier-1 mirror refuses anything; the other two encode what they are asked
    /// for, so a caller can keep asking for mp3 and read back what it got.
    pub fn container_for(self, requested: &str) -> &str {
        match self {
            SpeechEndpoint::KoblemTier1 if !matches!(requested, "wav" | "pcm") => "wav",
            _ => requested,
        }
    }
}

/// Synthesize a single sentence via the TTS service, returning audio bytes
/// (MP3 by default; override via `params.response_format`).
///
/// `params.speed` is a real render parameter. An engine that cannot do the
/// requested rate rejects the request with 400, so a rejected rate is retried
/// once at natural rate rather than surfaced as a failure — the caller gets
/// audio and `speed_applied` tells it what it actually got.
pub async fn synthesize_sentence(
    service_url: &str,
    backend: TtsBackend,
    endpoint: SpeechEndpoint<'_>,
    text: &str,
    voice: &str,
    params: &TtsParams,
) -> Result<SentenceAudio, TtsError> {
    // A whole passage buffered through koblem tier-2 routinely renders for
    // longer than the shared client's 30 s total deadline; only idle time is
    // bounded here.
    let client = http::new_streaming_client();
    let url = format!("{}{}", service_url.trim_end_matches('/'), endpoint.path());

    let response_format =
        endpoint.container_for(params.response_format.as_deref().unwrap_or("mp3"));
    let mut body = json!({
        "input": text,
        "response_format": response_format,
    });
    if let SpeechEndpoint::KoblemTier2 { .. } = endpoint {
        // koblem's own wire word for "one buffered blob". It does NOT reach the
        // engine, which has no such transport — koblem translates it into the
        // absence of `stream_format`. Omitting it here would instead get the
        // tier-2 default, which is a streamed SSE envelope.
        body["stream_format"] = json!("none");
    }
    // An empty voice means "let the engine pick one and hold it" — there is no
    // "default" clone to name, so the field is omitted rather than defaulted.
    if !voice.is_empty() {
        body["voice"] = json!(voice);
    }

    if let Some(v) = params.temperature {
        body["temperature"] = json!(v);
    }

    if backend.uses_native_voice_api() {
        if let Some(v) = params.top_p {
            body["top_p"] = json!(v);
        }
        if let Some(v) = params.top_k {
            body["top_k"] = json!(v);
        }
        if let Some(v) = params.repetition_penalty {
            body["repetition_penalty"] = json!(v);
        }
        if let Some(v) = params.seed {
            body["seed"] = json!(v);
        }
        if let Some(ref v) = params.instructions
            && !v.is_empty()
        {
            body["instructions"] = json!(v);
        }
        if let Some(ref v) = params.speaker
            && !v.is_empty()
        {
            body["speaker"] = json!(v);
        }
        if let Some(v) = params.max_new_frames {
            body["max_new_frames"] = json!(v);
        }
    }

    let requested_speed = wire_speed(params)
        .inspect(|v| body["speed"] = json!(v))
        .unwrap_or(1.0);

    let post = |body: &serde_json::Value| {
        let req = client.post(&url).json(body);
        match endpoint {
            SpeechEndpoint::KoblemTier2 { api_key } => req.bearer_auth(api_key),
            _ => req,
        }
    };

    let mut resp = post(&body).send().await?;
    let mut speed_applied = speed_applied_of(&resp, requested_speed);

    if retry_at_natural_speed(resp.status().as_u16(), requested_speed) {
        warn!(
            requested_speed,
            "TTS engine refused the speed; retrying at 1.0"
        );
        if let Some(obj) = body.as_object_mut() {
            obj.remove("speed");
        }
        speed_applied = 1.0;
        resp = post(&body).send().await?;
    }

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(TtsError::ServiceError { status, body });
    }

    Ok(SentenceAudio {
        bytes: resp.bytes().await?,
        speed_applied,
    })
}

/// Synthesize a whole passage via koblem tier-2 with streaming PCM +
/// interleaved word-level forced alignment. Returns a stream of
/// `TtsStreamEvent`s that fire as bytes arrive.
///
/// Wire shape: POSTs to `{koblem_url}/api/v1/tts/generate` with
/// `stream_format="sse"`, `response_format="pcm"`, `align=true`,
/// `align_stream="partial"`, `long=true`. The TTS server emits a
/// `text/event-stream` response which koblem tier-2 forwards byte-for-byte
/// (mpsc + bytes_stream passthrough), and this parses it into typed events.
///
/// The caller passes a whole PASSAGE, not sentences: `long` makes the engine
/// chunk internally with rolling context, which holds one voice across the
/// passage and keeps `word_index` absolute over all of it — so nothing inside
/// one of these calls needs globalising, and one chunk truncating does not
/// cascade into every later highlight. How big a passage should be is
/// [`split_passages`]' business, not this function's.
///
/// There is exactly one aligner-capable engine; callers should only route here
/// when `backend.supports_alignment()`.
///
/// Requires `koblem_api_key` (tier-2 endpoints enforce auth).
pub fn synthesize_paragraph_stream(
    koblem_url: &str,
    koblem_api_key: &str,
    text: &str,
    voice: &str,
    params: &TtsParams,
) -> impl Stream<Item = Result<TtsStreamEvent, TtsError>> + Send + 'static {
    let url = format!("{}/api/v1/tts/generate", koblem_url.trim_end_matches('/'));
    let api_key = koblem_api_key.to_string();
    let voice = voice.to_string();
    let text = text.to_string();
    let params = params.clone();

    async_stream::try_stream! {
        // Minutes of audio arrive over minutes of wall clock; the shared
        // client's 30 s total deadline would sever the body mid-passage.
        let client = http::new_streaming_client();

        let mut body = json!({
            "input": text,
            "response_format": "pcm",
            "stream_format": "sse",
            "align": true,
            "align_stream": "partial",
            "stream_first_batch_size": 1,
            // The engine's own default, stated explicitly: this path always
            // sends a whole passage and relies on internal chunking.
            "long": true,
        });
        // Omitted rather than defaulted — there is no "default" clone.
        if !voice.is_empty() {
            body["voice"] = json!(voice);
        }
        if let Some(v) = params.temperature {
            body["temperature"] = json!(v);
        }
        if let Some(v) = params.top_p {
            body["top_p"] = json!(v);
        }
        if let Some(v) = params.top_k {
            body["top_k"] = json!(v);
        }
        if let Some(v) = params.repetition_penalty {
            body["repetition_penalty"] = json!(v);
        }
        if let Some(v) = params.seed {
            body["seed"] = json!(v);
        }
        if let Some(ref v) = params.instructions
            && !v.is_empty()
        {
            body["instructions"] = json!(v);
        }
        if let Some(ref v) = params.speaker
            && !v.is_empty()
        {
            body["speaker"] = json!(v);
        }
        if let Some(v) = params.max_new_frames {
            body["max_new_frames"] = json!(v);
        }
        // koblem owns the fallback for this path and reports the outcome in
        // `x-speed-applied`.
        let requested_speed = wire_speed(&params).inspect(|v| body["speed"] = json!(v)).unwrap_or(1.0);

        let resp = client
            .post(&url)
            .bearer_auth(&api_key)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Err(TtsError::ServiceError { status, body })?;
            return;
        }

        // Before any audio: the caller has to know what rate it is about to
        // receive in order to label it, and the answer is only knowable once
        // the response headers land.
        yield TtsStreamEvent::Started {
            speed_applied: speed_applied_of(&resp, requested_speed),
        };

        let mut byte_stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
        let mut got_done = false;
        let mut truncated_out = false;
        let mut cancelled_out = false;

        while let Some(chunk) = byte_stream.next().await {
            let chunk = chunk?;
            buf.extend_from_slice(&chunk);

            // SSE events are separated by a blank line (either "\n\n" or
            // "\r\n\r\n"). Drain complete events from the head of `buf` and
            // dispatch each one.
            while let Some(end) = find_sse_event_end(&buf) {
                let raw = buf.drain(..end.consumed).collect::<Vec<u8>>();
                let body = std::str::from_utf8(&raw[..end.body_len]).unwrap_or("");
                match parse_sse_event(body) {
                    Some(SseEvent::AudioDelta { audio_b64 }) => {
                        let pcm = base64::engine::general_purpose::STANDARD
                            .decode(audio_b64.as_bytes())
                            .map_err(|e| TtsError::Unavailable(format!(
                                "TTS streamed invalid base64 audio: {e}"
                            )))?;
                        yield TtsStreamEvent::Audio { pcm: Bytes::from(pcm) };
                    }
                    Some(SseEvent::AlignmentPartial { audio_seen_ms, words }) => {
                        yield TtsStreamEvent::AlignmentPartial { audio_seen_ms, words };
                    }
                    Some(SseEvent::AlignmentFinal { audio_total_ms, words }) => {
                        yield TtsStreamEvent::AlignmentFinal { audio_total_ms, words };
                    }
                    Some(SseEvent::Chunk { index, chunks_total, word_offset, word_count, audio_offset_ms, text }) => {
                        yield TtsStreamEvent::Chunk {
                            index, chunks_total, word_offset, word_count, audio_offset_ms, text,
                        };
                    }
                    Some(SseEvent::Done { truncated, cancelled }) => {
                        got_done = true;
                        truncated_out = truncated;
                        cancelled_out = cancelled;
                    }
                    Some(SseEvent::AlignmentError { message }) => {
                        // Aligner failed but synth still produced audio — surface
                        // as a soft warning. Don't bail; the caller still has
                        // audio events to play.
                        warn!(message, "TTS forced alignment failed");
                    }
                    None => {
                        // Unknown event type — ignore. SSE allows arbitrary
                        // server-defined events.
                    }
                }
            }
        }

        if !got_done {
            Err(TtsError::ServiceError {
                status: 502,
                body: "TTS SSE stream ended without speech.audio.done".to_string(),
            })?;
            return;
        }

        yield TtsStreamEvent::Done { truncated: truncated_out, cancelled: cancelled_out };
    }
}

/// Result of locating the next complete SSE event in `buf`. `consumed` is
/// the byte count to drain (event body + blank-line terminator);
/// `body_len` is the byte count of the event body alone (excluding the
/// blank-line terminator) so the caller can slice the body for parsing.
struct SseEventBoundary {
    consumed: usize,
    body_len: usize,
}

fn find_sse_event_end(buf: &[u8]) -> Option<SseEventBoundary> {
    // Prefer "\n\n" — the canonical SSE separator and the only form the engine
    // emits today. Fall back to "\r\n\r\n" for spec-conformance with any
    // intermediary that rewrites line endings.
    if let Some(i) = find_subseq(buf, b"\n\n") {
        return Some(SseEventBoundary {
            consumed: i + 2,
            body_len: i,
        });
    }
    if let Some(i) = find_subseq(buf, b"\r\n\r\n") {
        return Some(SseEventBoundary {
            consumed: i + 4,
            body_len: i,
        });
    }
    None
}

fn find_subseq(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[derive(Debug)]
enum SseEvent {
    AudioDelta {
        audio_b64: String,
    },
    AlignmentPartial {
        audio_seen_ms: u32,
        words: Vec<WordTiming>,
    },
    AlignmentFinal {
        audio_total_ms: u32,
        words: Vec<WordTiming>,
    },
    Done {
        truncated: bool,
        cancelled: bool,
    },
    AlignmentError {
        message: String,
    },
    Chunk {
        index: u32,
        chunks_total: u32,
        word_offset: u32,
        word_count: u32,
        audio_offset_ms: u32,
        text: String,
    },
}

/// Parse one SSE event body (header lines + data line). We only look at
/// `data: ` lines; the canonical engine envelope JSON includes a `type`
/// field that we dispatch on (`event:` lines are redundant). Multi-line
/// `data:` payloads are concatenated per the SSE spec.
fn parse_sse_event(body: &str) -> Option<SseEvent> {
    let mut data = String::new();
    for line in body.split('\n') {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("data:") {
            // Spec: a single leading space after the colon is stripped.
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest);
        }
    }
    if data.is_empty() {
        return None;
    }

    let json: serde_json::Value = serde_json::from_str(&data).ok()?;
    let kind = json.get("type")?.as_str()?;
    match kind {
        "speech.audio.delta" => {
            let audio_b64 = json.get("audio")?.as_str()?.to_string();
            Some(SseEvent::AudioDelta { audio_b64 })
        }
        "speech.audio.alignment.partial" => {
            let audio_seen_ms = json
                .get("audio_seen_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let words = parse_word_array(json.get("words")?)?;
            Some(SseEvent::AlignmentPartial {
                audio_seen_ms,
                words,
            })
        }
        "speech.audio.alignment.final" => {
            let audio_total_ms = json
                .get("audio_total_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let words = parse_word_array(json.get("words")?)?;
            Some(SseEvent::AlignmentFinal {
                audio_total_ms,
                words,
            })
        }
        "speech.audio.chunk" => Some(SseEvent::Chunk {
            index: json.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            chunks_total: json
                .get("chunks_total")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            word_offset: json
                .get("word_offset")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            word_count: json.get("word_count").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            audio_offset_ms: json
                .get("audio_offset_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            text: json
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        }),
        "speech.audio.alignment.error" => {
            let message = json
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("(no message)")
                .to_string();
            Some(SseEvent::AlignmentError { message })
        }
        "speech.audio.done" => Some(SseEvent::Done {
            truncated: json
                .get("truncated")
                .and_then(|t| t.as_bool())
                .unwrap_or(false),
            // Absent on an engine that predates the flag, which is the same
            // answer as a render that ran to the end.
            cancelled: json
                .get("cancelled")
                .and_then(|t| t.as_bool())
                .unwrap_or(false),
        }),
        _ => None,
    }
}

fn parse_word_array(v: &serde_json::Value) -> Option<Vec<WordTiming>> {
    let arr = v.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for w in arr {
        let word_index = w.get("word_index")?.as_u64()? as u32;
        let text = w.get("text")?.as_str()?.to_string();
        let t0_ms = w.get("t0_ms")?.as_u64()? as u32;
        let t1_ms = w.get("t1_ms")?.as_u64()? as u32;
        out.push(WordTiming {
            word_index,
            text,
            t0_ms,
            t1_ms,
        });
    }
    Some(out)
}

/// The voice kobbler mints for itself when nothing else names one.
///
/// Lives in the engine's own voice archive alongside operator-created clones,
/// so it survives restarts and is visible (and deletable) in the voice list
/// like any other.
pub const NARRATOR_VOICE_ID: &str = "kobbler-narrator";

/// The passage the narrator is minted from.
///
/// Its exact wording matters more than it looks. The engine samples a speaker
/// from the TEXT when no reference is given, so this string IS the narrator —
/// change it and the reader's voice changes with it. It is deliberately plain,
/// declarative, unpunctuated by dialogue and about fifteen seconds long: long
/// enough to give the cloner a usable reference (the engine's own long-form
/// carry uses ~250 frames / 20 s), short enough to stay well inside the
/// prompt's share of a 2048-token context.
const NARRATOR_CALIBRATION_TEXT: &str = "This is the narrator's voice for the reader. \
     It should be clear and even, with a steady pace and no hurry, the way a good \
     audiobook is read aloud. Every chapter from here on will be spoken in this same \
     voice and this same manner.";

/// Make sure a stable narrator exists, minting it on first use.
///
/// The problem this solves: given no `voice`, the engine samples a speaker from
/// the text and holds it **for that request only** — long-form rolls each
/// chunk's own generated audio forward as the reference for the next, so one
/// request keeps one narrator, and the next request starts over. The read-along
/// issues a request per page, so an unnamed voice means a different reader on
/// every page turn, carrying different pacing and intonation with it.
///
/// There is no request field that seeds a synth from a previous one — the whole
/// engine knob set has no such thing. Registering a clone is the mechanism it
/// does expose, and it produces the same prompt shape the internal carry uses
/// (`[S0]{ref_text}<audio ref>[S0]{text}`), so the reference supplies delivery
/// as well as timbre. Measured over six passages, naming this voice took mean
/// silence from 23.8% to 19.8% of the audio; the pacing spread was unchanged,
/// because pacing was never the problem.
///
/// The mint is deterministic: `seed` defaults to a fixed constant server-side
/// and identical text renders bit-identically (verified over six runs), so a
/// wiped voice archive re-mints the SAME narrator rather than a new one. It is
/// rendered at natural rate on purpose — cloning WSOLA-stretched audio would
/// bake the stretch into the voice and then stretch it again on every use.
///
/// Cheap after the first call: the caller already holds the voice list, and the
/// engine's own `voice.warmup` cache makes a cloned synth ~50-70 ms versus
/// ~35-47 ms unconditioned.
///
/// The caller passes the voice list it already fetched so this costs no extra
/// round trip on the common path where the narrator is already there.
pub async fn mint_narrator_voice(
    service_url: &str,
    backend: TtsBackend,
    endpoint: SpeechEndpoint<'_>,
) -> Result<(), TtsError> {
    let params = TtsParams {
        // Natural rate, explicitly: see above.
        speed: None,
        response_format: Some("wav".to_string()),
        ..Default::default()
    };
    let audio = synthesize_sentence(
        service_url,
        backend,
        endpoint,
        NARRATOR_CALIBRATION_TEXT,
        "",
        &params,
    )
    .await?;

    upload_voice(
        service_url,
        backend,
        NARRATOR_VOICE_ID,
        &audio.bytes,
        Some(NARRATOR_CALIBRATION_TEXT),
    )
    .await
}

/// Whether `voice` leaves the engine to pick a speaker for itself.
///
/// `""` and `"default"` are the same instruction to this engine — `"default"`
/// is not a clone, it is the spelling for "no reference" — and both mean a
/// narrator resampled per request.
pub fn voice_is_unpinned(voice: &str) -> bool {
    voice.is_empty() || voice == "default"
}

/// Check if a voice is registered on the TTS service, and re-upload if missing.
///
/// Used to handle service container restarts — voice clips need to be
/// re-registered. `ref_text` is the reference transcript; ignored by other
/// backends.
pub async fn ensure_voice_registered(
    service_url: &str,
    backend: TtsBackend,
    voice_name: &str,
    wav_path: &std::path::Path,
    ref_text: Option<&str>,
) -> Result<(), TtsError> {
    let voices = get_voices(service_url).await;
    if voices.iter().any(|v| v.id == voice_name) {
        return Ok(());
    }

    // Voice not found — re-upload
    let wav_bytes = std::fs::read(wav_path).map_err(|e| {
        TtsError::Unavailable(format!(
            "Failed to read voice clip {}: {e}",
            wav_path.display()
        ))
    })?;
    upload_voice(service_url, backend, voice_name, &wav_bytes, ref_text).await
}

/// Upload a voice reference clip to the TTS service.
///
/// Branches on backend wire shape:
/// - Chatterbox: `POST /voices` multipart fields `voice_name` + `voice_file`.
/// - Breeze: `POST /v1/audio/voices` multipart fields `name` + `audio_sample`
///   + optional `ref_text` (turns on ICL ref-text for higher-fidelity clones).
/// - Kokoro/OpenAI/Auto: chatterbox shape (no upstream upload support; auto
///   probes work against chatterbox-style services).
pub async fn upload_voice(
    service_url: &str,
    backend: TtsBackend,
    voice_name: &str,
    wav_bytes: &[u8],
    ref_text: Option<&str>,
) -> Result<(), TtsError> {
    let client = http::new_client();
    let base = service_url.trim_end_matches('/');

    let part = reqwest::multipart::Part::bytes(wav_bytes.to_vec())
        .file_name(format!("{voice_name}.wav"))
        .mime_str("audio/wav")
        .unwrap();

    let (url, form) = match backend {
        b if b.uses_native_voice_api() => {
            let mut form = reqwest::multipart::Form::new()
                .text("name", voice_name.to_string())
                .part("audio_sample", part);
            if let Some(t) = ref_text
                && !t.is_empty()
            {
                form = form.text("ref_text", t.to_string());
            }
            (format!("{base}/v1/audio/voices"), form)
        }
        _ => {
            let form = reqwest::multipart::Form::new()
                .text("voice_name", voice_name.to_string())
                .part("voice_file", part);
            (format!("{base}/voices"), form)
        }
    };

    let resp = client.post(&url).multipart(form).send().await?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(TtsError::ServiceError { status, body });
    }

    Ok(())
}

/// Delete a registered voice. Breeze supports `DELETE /v1/audio/voices/{name}`;
/// chatterbox has no delete endpoint and this returns Ok without contacting the
/// service (caller must remove the on-disk WAV separately).
pub async fn delete_voice(
    service_url: &str,
    backend: TtsBackend,
    voice_name: &str,
) -> Result<(), TtsError> {
    if !backend.uses_native_voice_api() {
        return Ok(());
    }
    let client = http::new_client();
    let encoded =
        percent_encoding::utf8_percent_encode(voice_name, percent_encoding::NON_ALPHANUMERIC)
            .to_string();
    let url = format!(
        "{}/v1/audio/voices/{}",
        service_url.trim_end_matches('/'),
        encoded
    );
    let resp = client.delete(&url).send().await?;
    if !resp.status().is_success() && resp.status().as_u16() != 404 {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(TtsError::ServiceError { status, body });
    }
    Ok(())
}

/// Runs of horizontal whitespace, and single newlines with whatever surrounds
/// them. Both collapse to one space.
static INTRA_PARAGRAPH_WS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^\S\n]*\n[^\S\n]*|[^\S\n]+").expect("valid regex"));

/// Two or more newlines with any whitespace between them — a paragraph break.
static PARAGRAPH_BREAK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\n[^\S\n]*\n\s*").expect("valid regex"));

/// Tidy the whitespace of text on its way to the engine, and put the sentences
/// back together.
///
/// Text scraped out of an EPUB carries the source file's own line wrapping:
/// every text node keeps the newlines and indentation the producer pretty-
/// printed into the XHTML, and a fixed-layout book goes further — each VISUAL
/// LINE is its own absolutely-positioned block, so one sentence arrives as five
/// separate paragraphs and a lone "I" arrives as a paragraph of its own. The
/// model reads that layout as prosody: it pauses at every break and delivers
/// each fragment as a complete utterance, which is what makes the narration
/// crawl for reasons that have nothing to do with the prose.
///
/// Two passes. First whitespace inside a paragraph collapses to single spaces.
/// Then a paragraph break is KEPT only where a speaker would actually stop —
/// [`is_soft_wrap`] decides — and otherwise the two chunks are rejoined with a
/// space.
///
/// Measured on one page of the fixed-layout book that prompted this (158 words,
/// this engine, `speed` unset — the page arrives as 31 chunks, two of which are
/// the bare word "I"):
///
/// | text                            | rate      | silence |
/// |---------------------------------|-----------|---------|
/// | as scraped                      | 142.2 wpm | 31.6%   |
/// | whitespace collapsed only       | 127.0 wpm | 30.5%   |
/// | everything on one line          | 130.5 wpm | 23.0%   |
/// | this pass (16 chunks)           | 150.0 wpm | 24.1%   |
///
/// Collapsing whitespace WITHOUT rejoining is worse than doing nothing at all,
/// which is why this is one function and not two: flattening the indentation
/// leaves every wrap looking like a deliberate paragraph. Flattening everything
/// instead loses the real breaks and the model invents its own.
///
/// The isolated fragments are the expensive part, not the pauses: "I" on its
/// own line costs 0.48 s over the same six words joined (2.80 s vs 2.32 s), and
/// synthesised entirely alone it is 0.72 s of which 43% is silence.
///
/// Token-preserving by construction — every substitution maps whitespace to
/// whitespace, so the whitespace-split word sequence is untouched and the
/// aligner's `word_index` still refers to the same words. That matters: the
/// client maps those indices onto the text this returns.
pub fn normalize_for_synthesis(text: &str) -> String {
    // NBSP and friends first: they are not `\s` to the regex engine's ASCII
    // classes in every position, and the engine treats them as glyphs.
    let text = text.replace(['\u{00A0}', '\u{2007}', '\u{202F}'], " ");
    // Paragraph breaks are protected with a placeholder that contains no
    // whitespace, so the intra-paragraph pass cannot eat them.
    const PARA: &str = "\u{0000}";
    let text = PARAGRAPH_BREAK.replace_all(&text, PARA);
    let text = INTRA_PARAGRAPH_WS.replace_all(&text, " ");

    let mut out = String::with_capacity(text.len());
    for chunk in text.split(PARA) {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        if out.is_empty() {
            out.push_str(chunk);
        } else if is_soft_wrap(&out, chunk) {
            out.push(' ');
            out.push_str(chunk);
        } else {
            out.push_str("\n\n");
            out.push_str(chunk);
        }
    }
    out
}

/// Closing punctuation that rides AFTER the sentence-ending mark, so
/// `endure.\u{201d}` still ends a sentence.
const CLOSING_MARKS: [char; 9] = [
    '"', '\'', '\u{2019}', '\u{201d}', '\u{2018}', '\u{201c}', ')', ']', '\u{bb}',
];

/// Closed-class words. A line that ends on one of these is mid-sentence — no
/// heading ends "and", "the" or "he". Deliberately conservative: "one", "all",
/// "there" and friends are NOT here, because "Chapter One" is a heading and
/// joining it to the prose that follows is a worse failure than leaving a
/// break in the middle of a sentence.
const FUNCTION_WORDS: &[&str] = &[
    "a", "an", "the", "and", "but", "or", "nor", "so", "yet", "of", "to", "in", "on", "at", "for",
    "with", "as", "by", "from", "into", "than", "that", "this", "these", "those", "it", "its",
    "he", "she", "his", "her", "him", "they", "them", "their", "we", "us", "our", "you", "your",
    "i", "my", "me", "is", "was", "were", "are", "be", "been", "being", "am", "had", "has", "have",
    "do", "does", "did", "if", "when", "while", "which", "who", "whom", "whose", "about", "after",
    "before", "between", "through", "during", "upon",
];

/// Does this text end where a speaker would stop?
fn ends_a_sentence(text: &str) -> bool {
    let t = text.trim_end().trim_end_matches(CLOSING_MARKS).trim_end();
    t.ends_with(['.', '!', '?', '\u{2026}'])
}

/// Was the break between `prev` and `next` a line wrap rather than a paragraph?
///
/// The producer's intent is not recoverable, so this reads the text the way a
/// human skimming the page would. Any ONE of these settles it:
///
/// - `prev` ends mid-sentence and `next` starts lowercase — the common case;
/// - `prev` ends on a comma, colon, dash or an opening quote;
/// - `prev`'s last word is closed-class ("...world domination," he | says...).
///
/// Everything else keeps its break. That asymmetry is deliberate: a break kept
/// by mistake costs a pause, a break removed by mistake runs a heading into the
/// prose under it.
fn is_soft_wrap(prev: &str, next: &str) -> bool {
    if ends_a_sentence(prev) {
        return false;
    }
    let trimmed = prev.trim_end().trim_end_matches(CLOSING_MARKS).trim_end();
    let (Some(last_char), Some(first_next)) = (trimmed.chars().next_back(), next.chars().next())
    else {
        return true;
    };
    if first_next.is_lowercase() {
        return true;
    }
    if matches!(
        last_char,
        ',' | ';' | ':' | '\u{2014}' | '\u{2013}' | '-' | '(' | '\u{201c}' | '\u{2018}' | '"'
    ) {
        return true;
    }
    let last_word: String = trimmed
        .split_whitespace()
        .next_back()
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_alphabetic() || *c == '\u{2019}' || *c == '\'')
        .collect::<String>()
        .to_lowercase();
    FUNCTION_WORDS.contains(&last_word.as_str())
}

static SENTENCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([.!?;])\s+").expect("valid regex"));

/// The engine's own idea of a word: a run of bytes with no ASCII whitespace in
/// it.
///
/// The aligner indexes the list this produces, so anything counting words in
/// order to line up with an alignment has to count them the same way — notably
/// NOT `split_whitespace`, which also breaks on the Unicode separators the
/// engine treats as ordinary glyphs. One extra break here and every word index
/// after it points at the wrong word.
pub fn word_count(text: &str) -> usize {
    text.split_ascii_whitespace().count()
}

/// Words in the FIRST request of a passage.
///
/// Deliberately short. Nothing on screen can follow the voice until the first
/// alignment lands, and the aligner's cost is superlinear in the word list —
/// so the opening request is sized for time-to-first-highlight, not throughput.
/// Every request after it is long, and by the time the listener reaches one it
/// has had a whole short passage's worth of playback to be aligned in.
pub const FIRST_PASSAGE_WORDS: usize = 120;

/// Words in every request after the first.
///
/// Long enough that the per-request overhead and the seam between passages are
/// rare, short enough that the aligner stays well inside its VRAM envelope and
/// a re-render after a failure loses a paragraph rather than a chapter.
pub const PASSAGE_WORDS: usize = 400;

/// A tail shorter than this fraction of the budget is absorbed by the passage
/// before it rather than being sent as a request of its own.
const TAIL_MERGE_DIVISOR: usize = 4;

/// Cut `text` into request-sized passages at sentence boundaries.
///
/// The engine chunks internally too, but a single request holds the whole thing
/// in one aligner pass, and that pass is the expensive part: cost grows with
/// the square of the word list and its VRAM grows with the audio it has to hold
/// on to. Cutting here bounds both, and the sizes RAMP — see
/// [`FIRST_PASSAGE_WORDS`].
///
/// Token-preserving: cuts land on whitespace between words, so concatenating
/// the whitespace-split words of every passage reproduces the whitespace-split
/// words of `text`. That is what lets a client add a passage's `word_offset` to
/// a word index and get a position in the original.
///
/// Boundaries prefer sentence ends, then paragraph breaks, then — only when a
/// single sentence outruns the whole budget — the word budget itself.
pub fn split_passages(text: &str) -> Vec<String> {
    // (start, end) of each whitespace-delimited word, plus whether a passage
    // may end after it.
    let mut words: Vec<(usize, usize, bool)> = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i > start {
            // A paragraph break is as good a place to stop as a full stop, and
            // in verse or dialogue it is often the only one there is.
            let gap_has_newline = text[i..]
                .bytes()
                .take_while(u8::is_ascii_whitespace)
                .any(|b| b == b'\n');
            words.push((start, i, ends_sentence(&text[start..i]) || gap_has_newline));
        }
    }

    if words.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut w = 0;
    let mut budget = FIRST_PASSAGE_WORDS;
    while w < words.len() {
        let hard = (w + budget).min(words.len());
        // Absorb a tail too short to be worth its own request.
        let hard = if words.len() - hard < budget / TAIL_MERGE_DIVISOR {
            words.len()
        } else {
            hard
        };
        // Walk back to the last sentence end. `w + 1` at minimum: a passage of
        // one word beats an empty one.
        let mut cut = hard;
        while cut > w + 1 && !words[cut - 1].2 {
            cut -= 1;
        }
        if cut == w + 1 && !words[w].2 {
            cut = hard; // no boundary anywhere in range — take the budget
        }
        out.push(text[words[w].0..words[cut - 1].1].to_string());
        w = cut;
        budget = PASSAGE_WORDS;
    }
    out
}

/// Whether a word ends a sentence — terminal punctuation, possibly behind the
/// closing quote or bracket that a line of dialogue ends with.
fn ends_sentence(word: &str) -> bool {
    let trimmed = word.trim_end_matches([
        '"', '\'', ')', ']', '}', '\u{201D}', '\u{2019}', '\u{00BB}', '*', '_',
    ]);
    trimmed.ends_with(['.', '!', '?', '\u{2026}'])
}

/// Split text into sentences for per-sentence TTS generation.
///
/// First splits on paragraph boundaries (double newlines) so that headings
/// and short lines separated by blank lines become their own TTS sentences
/// with natural pauses. Then applies sentence-level splitting within each
/// paragraph chunk.
pub fn split_sentences(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let mut sentences = Vec::new();

    // Split on paragraph boundaries first (double newlines) so headings and
    // lines separated by blank lines become their own TTS sentences.
    for paragraph in trimmed.split("\n\n") {
        let para = paragraph.trim();
        if para.is_empty() {
            continue;
        }
        // Each paragraph starts fresh — no cross-paragraph merging of short
        // fragments, so "Chapter One" won't get glued to the previous paragraph.
        let mut para_sentences = Vec::new();
        split_paragraph_sentences(para, &mut para_sentences);
        sentences.extend(para_sentences);
    }

    sentences
}

/// Split a single paragraph into sentences using punctuation boundaries,
/// merging short fragments (<10 chars without sentence-ending punctuation)
/// into the previous sentence.
fn split_paragraph_sentences(text: &str, sentences: &mut Vec<String>) {
    let re = &*SENTENCE_RE;
    let mut last_end = 0;

    for cap in re.captures_iter(text) {
        let m = cap.get(0).unwrap();
        let punct = cap.get(1).unwrap();
        let sentence_end = punct.end();
        let chunk = text[last_end..sentence_end].trim();
        if !chunk.is_empty() {
            push_or_merge(sentences, chunk);
        }
        last_end = m.end();
    }

    let remainder = text[last_end..].trim();
    if !remainder.is_empty() {
        push_or_merge(sentences, remainder);
    }
}

/// Push a chunk as a new sentence, or merge it into the previous one if it's
/// a short fragment (<10 chars) without sentence-ending punctuation.
fn push_or_merge(sentences: &mut Vec<String>, chunk: &str) {
    if let Some(last) = sentences.last_mut()
        && chunk.len() < 10
        && !chunk.ends_with(['.', '!', '?'])
    {
        last.push(' ');
        last.push_str(chunk);
    } else {
        sentences.push(chunk.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── split_passages ─────────────────────────────────────────────────

    /// The whole contract in one assertion: a client adds a passage's
    /// `word_offset` to a word index and lands on the same word it would have
    /// in the original. That only holds if nothing is added, dropped or
    /// reordered.
    #[test]
    fn passages_reassemble_into_the_original_word_sequence() {
        let text = (0..40)
            .map(|i| format!("Sentence number {i} runs on for a while and then stops."))
            .collect::<Vec<_>>()
            .join(" ");
        let passages = split_passages(&text);
        assert!(
            passages.len() > 1,
            "a 480-word read was sent as one request"
        );
        let rejoined: Vec<&str> = passages.iter().flat_map(|p| p.split_whitespace()).collect();
        let original: Vec<&str> = text.split_whitespace().collect();
        assert_eq!(rejoined, original);
    }

    /// Time-to-first-highlight is what the ramp buys, so the opening request
    /// has to actually be the short one.
    #[test]
    fn the_first_passage_is_the_short_one() {
        let text = "Word ".repeat(2000) + "end.";
        let passages = split_passages(&text);
        let lens: Vec<usize> = passages
            .iter()
            .map(|p| p.split_whitespace().count())
            .collect();
        assert!(
            lens[0] <= FIRST_PASSAGE_WORDS,
            "first passage was {} words",
            lens[0]
        );
        assert!(lens[1] > FIRST_PASSAGE_WORDS, "the ramp never ramped");
    }

    #[test]
    fn a_passage_ends_on_a_sentence() {
        let text = (0..60)
            .map(|i| format!("This is sentence {i} and it ends here."))
            .collect::<Vec<_>>()
            .join(" ");
        for p in split_passages(&text) {
            assert!(
                p.trim_end().ends_with('.'),
                "a passage was cut mid-sentence: {:?}",
                &p[p.len().saturating_sub(40)..]
            );
        }
    }

    /// No punctuation anywhere — there is no boundary to find, and the split
    /// still has to terminate and still has to preserve every word.
    #[test]
    fn text_with_no_sentence_ends_still_splits() {
        let text = "word ".repeat(1500);
        let passages = split_passages(&text);
        assert!(passages.len() > 1);
        assert_eq!(
            passages
                .iter()
                .map(|p| p.split_whitespace().count())
                .sum::<usize>(),
            1500
        );
    }

    #[test]
    fn a_short_text_is_one_passage() {
        let text = "Short enough to say in one breath. Twice, even.";
        assert_eq!(split_passages(text), vec![text.to_string()]);
    }

    #[test]
    fn empty_text_is_no_passages() {
        assert!(split_passages("   \n\n  ").is_empty());
    }

    /// Verse and dialogue can run for pages without a full stop; the line break
    /// is the only boundary there is, and it is a real one.
    #[test]
    fn a_paragraph_break_is_a_boundary() {
        let text = (0..200)
            .map(|i| format!("line {i} of the poem"))
            .collect::<Vec<_>>()
            .join("\n\n");
        for p in split_passages(&text) {
            assert!(p.trim_end().ends_with("poem"), "cut mid-line: {p:?}");
        }
    }

    // ── normalize_for_synthesis ────────────────────────────────────────

    /// The defect this exists for: an EPUB's own source line wrapping arrives
    /// inside a sentence and the model performs it as a pause.
    #[test]
    fn source_line_wrapping_inside_a_sentence_becomes_one_space() {
        let scraped = "former detective and current has-been who \n   there is absolutely no reason to talk about.";
        assert_eq!(
            normalize_for_synthesis(scraped),
            "former detective and current has-been who there is absolutely no reason to talk about."
        );
    }

    /// Real paragraph breaks survive. Flattening these too measured WORSE than
    /// leaving the text alone — the model needs them to place real breaks.
    #[test]
    fn paragraph_breaks_are_kept() {
        assert_eq!(
            normalize_for_synthesis("First para.\n\nSecond para."),
            "First para.\n\nSecond para."
        );
        // However many blank lines the scrape produced, one break comes out.
        assert_eq!(
            normalize_for_synthesis("Done.\n \n\n  \nNext."),
            "Done.\n\nNext."
        );
    }

    /// The fixed-layout case: each visual line is its own block, so a sentence
    /// arrives as several "paragraphs" and pronouns arrive alone.
    #[test]
    fn wrapped_visual_lines_are_rejoined() {
        assert_eq!(
            normalize_for_synthesis(
                "I\n\nwatch as Scrimshaw backs farther\n\naway, waving the pool cue."
            ),
            "I watch as Scrimshaw backs farther away, waving the pool cue."
        );
        // Uppercase next line, but the previous one ends on a closed-class
        // word — still a wrap.
        assert_eq!(
            normalize_for_synthesis("Failure. So\n\nI had to crush you."),
            "Failure. So I had to crush you."
        );
        // Dialogue that wraps after the attribution comma.
        assert_eq!(
            normalize_for_synthesis(
                "\u{201c}All we wanted,\u{201d} he\n\nsays, \u{201c}but you stood in the way.\u{201d}"
            ),
            "\u{201c}All we wanted,\u{201d} he says, \u{201c}but you stood in the way.\u{201d}"
        );
    }

    /// The guard that keeps the rejoin from eating headings: no terminal
    /// punctuation, but nothing else says "mid-sentence" either.
    #[test]
    fn headings_keep_their_break() {
        for input in [
            "Chapter One\n\nThe morning was cold.",
            "CHAPTER 3\n\nIt was raining.",
            "The End\n\nAbout the Author",
        ] {
            assert_eq!(normalize_for_synthesis(input), input, "joined a heading");
        }
    }

    /// The whole pass has to be token-preserving: the aligner reports
    /// `word_index` against a whitespace split of exactly this string, and the
    /// client maps those indices back onto it.
    #[test]
    fn the_whitespace_split_is_unchanged() {
        for input in [
            "one\n\ntwo   three\tfour\n five",
            "  leading and trailing  ",
            "a\u{00A0}b\u{202F}c",
            "\n\n\nonly paragraph breaks\n\n\n",
        ] {
            let before: Vec<&str> = input.split_whitespace().collect();
            let after = normalize_for_synthesis(input);
            let after: Vec<&str> = after.split_whitespace().collect();
            assert_eq!(before, after, "token sequence changed for {input:?}");
        }
    }

    /// Non-breaking spaces are glyphs to the engine, not separators.
    #[test]
    fn nbsp_becomes_a_plain_space() {
        assert_eq!(normalize_for_synthesis("Mr.\u{00A0}Jenkins"), "Mr. Jenkins");
    }

    #[test]
    fn already_clean_text_is_untouched() {
        let clean = "A tidy sentence.\n\nAnd another one.";
        assert_eq!(normalize_for_synthesis(clean), clean);
    }

    /// Routing through koblem must not cost the caller its container. mp3 only
    /// exists on a buffered render (breeze cannot stream a compressed format at
    /// all), and tier-2 is the only koblem endpoint that buffers — so a keyed
    /// kobbler goes there and keeps mp3, while a keyless one falls back to the
    /// tier-1 mirror and is clamped to wav rather than 400'd.
    #[test]
    fn koblem_keeps_mp3_on_tier2_and_clamps_only_the_keyless_fallback() {
        let direct = SpeechEndpoint::resolve(false, Some("key"));
        let keyed = SpeechEndpoint::resolve(true, Some("key"));
        let keyless = SpeechEndpoint::resolve(true, None);
        assert_eq!(
            direct,
            SpeechEndpoint::Engine,
            "the key is irrelevant off-koblem"
        );
        assert_eq!(keyed, SpeechEndpoint::KoblemTier2 { api_key: "key" });
        assert_eq!(keyless, SpeechEndpoint::KoblemTier1);
        // An empty key is not a key.
        assert_eq!(
            SpeechEndpoint::resolve(true, Some("")),
            SpeechEndpoint::KoblemTier1
        );

        assert_eq!(direct.path(), "/v1/audio/speech");
        assert_eq!(keyless.path(), "/v1/audio/speech");
        assert_eq!(keyed.path(), "/api/v1/tts/generate");

        for want in ["mp3", "ogg", "aac"] {
            assert_eq!(direct.container_for(want), want, "{want} direct");
            assert_eq!(keyed.container_for(want), want, "{want} tier-2");
            assert_eq!(keyless.container_for(want), "wav", "{want} tier-1");
        }
        for want in ["wav", "pcm"] {
            for ep in [direct, keyed, keyless] {
                assert_eq!(ep.container_for(want), want, "{want} on {ep:?}");
            }
        }
    }

    #[test]
    fn parse_voices_chatterbox_object_shape() {
        let body = r#"{"voices": [
            {"id": "default", "name": "Default"},
            {"id": "alice", "name": "Alice"}
        ]}"#;
        let voices = parse_voices_body(body);
        assert_eq!(voices.len(), 2);
        // Sorted by name; Alice < Default lexicographically (uppercase sort).
        assert_eq!(voices[0].id, "alice");
        assert_eq!(voices[0].name, "Alice");
        assert_eq!(voices[1].id, "default");
    }

    #[test]
    fn parse_voices_kokoro_string_list() {
        let body = r#"{"voices": ["af_heart", "af_alloy"]}"#;
        let voices = parse_voices_body(body);
        assert_eq!(voices.len(), 2);
        // String IDs use the same value for id and name; sorted alphabetically.
        assert_eq!(voices[0].id, "af_alloy");
        assert_eq!(voices[0].name, "af_alloy");
        assert_eq!(voices[1].id, "af_heart");
    }

    #[test]
    fn parse_voices_model_id_keyed() {
        // Legacy map shape: a single top-level key (the model id) whose value
        // is a flat array of voice-name strings.
        let body = r#"{"breeze-tts-2": ["default", "alice", "bob"]}"#;
        let voices = parse_voices_body(body);
        assert_eq!(voices.len(), 3);
        assert_eq!(voices[0].id, "alice");
        assert_eq!(voices[0].name, "alice");
        assert_eq!(voices[1].id, "bob");
        assert_eq!(voices[2].id, "default");
    }

    /// The live engine answers with BOTH shapes in one body, listing the same
    /// ids. The map has 3 keys so it never matches the single-key branch, and
    /// the `voices` array wins. `default` is a real selectable voice — the
    /// model's own narrator — so it must survive rather than be filtered.
    #[test]
    fn parse_voices_prefers_the_rich_array_when_both_shapes_are_present() {
        let body = r#"{"breeze-tts-2":["default","gaiman"],"model_id":"breeze-tts-2",
                       "voices":[{"id":"default","builtin":true},
                                 {"id":"gaiman","frames":150,"has_ref_text":true}]}"#;
        let voices = parse_voices_body(body);
        assert_eq!(
            voices.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
            ["default", "gaiman"],
            "the rich array is authoritative"
        );
    }

    #[test]
    fn parse_voices_unknown_returns_empty() {
        let body = r#"{"unexpected": "shape"}"#;
        let voices = parse_voices_body(body);
        assert!(voices.is_empty());
    }

    #[test]
    fn tts_backend_from_config_maps_known_strings() {
        assert_eq!(TtsBackend::from_config("breeze"), TtsBackend::Breeze);
        assert_eq!(
            TtsBackend::from_config("chatterbox"),
            TtsBackend::Chatterbox
        );
        assert_eq!(TtsBackend::from_config("kokoro"), TtsBackend::Kokoro);
        assert_eq!(TtsBackend::from_config("openai"), TtsBackend::OpenAi);
        // Unknown / "auto" / empty all collapse to Auto.
        assert_eq!(TtsBackend::from_config("auto"), TtsBackend::Auto);
        assert_eq!(TtsBackend::from_config(""), TtsBackend::Auto);
        assert_eq!(TtsBackend::from_config("garbage"), TtsBackend::Auto);
    }

    #[test]
    fn split_simple_sentences() {
        let result = split_sentences("Hello world. How are you? I am fine!");
        assert_eq!(result, vec!["Hello world.", "How are you?", "I am fine!"]);
    }

    #[test]
    fn split_single_sentence() {
        let result = split_sentences("Just one sentence.");
        assert_eq!(result, vec!["Just one sentence."]);
    }

    #[test]
    fn split_no_punctuation() {
        let result = split_sentences("No ending punctuation here");
        assert_eq!(result, vec!["No ending punctuation here"]);
    }

    #[test]
    fn split_empty_input() {
        let result = split_sentences("");
        assert!(result.is_empty());
    }

    #[test]
    fn split_whitespace_only() {
        let result = split_sentences("   ");
        assert!(result.is_empty());
    }

    #[test]
    fn split_merges_short_fragments() {
        // Fragments under 10 chars without sentence-ending punct get merged
        let result = split_sentences("Hello world. OK; Next sentence.");
        // "OK" is < 10 chars and ends with ';' not '.!?' so merges with previous
        assert_eq!(result.len(), 2);
        assert!(result[0].contains("OK"));
    }

    #[test]
    fn split_semicolons() {
        let result = split_sentences("First part; second part. Third part.");
        assert!(!result.is_empty());
        // Semicolons are treated as sentence boundaries
    }

    #[test]
    fn split_preserves_trailing_text() {
        let result = split_sentences("First sentence. And some trailing text without punctuation");
        assert_eq!(result.len(), 2);
        assert_eq!(result[1], "And some trailing text without punctuation");
    }

    #[test]
    fn split_trims_whitespace() {
        let result = split_sentences("  Hello world.   How are you?  ");
        assert_eq!(result, vec!["Hello world.", "How are you?"]);
    }

    #[test]
    fn split_multiple_spaces_between() {
        let result = split_sentences("First.   Second.   Third.");
        assert_eq!(result, vec!["First.", "Second.", "Third."]);
    }

    #[test]
    fn split_paragraph_boundaries() {
        let result = split_sentences("Chapter One\n\nThe story begins with a hero.");
        assert_eq!(result, vec!["Chapter One", "The story begins with a hero."]);
    }

    #[test]
    fn split_multiple_paragraphs() {
        let result = split_sentences(
            "Chapter One\n\nThe story begins. It was dark.\n\nChapter Two\n\nThe hero arrived.",
        );
        assert_eq!(
            result,
            vec![
                "Chapter One",
                "The story begins.",
                "It was dark.",
                "Chapter Two",
                "The hero arrived.",
            ]
        );
    }

    #[test]
    fn split_no_cross_paragraph_merge() {
        // "OK" is <10 chars but should NOT merge into the previous paragraph
        let result = split_sentences("First sentence.\n\nOK\n\nThird paragraph.");
        assert_eq!(result, vec!["First sentence.", "OK", "Third paragraph."]);
    }

    #[test]
    fn split_single_newline_not_boundary() {
        // Single newlines should NOT split — only double newlines
        let result = split_sentences("Line one.\nLine two.");
        assert_eq!(result, vec!["Line one.", "Line two."]);
    }

    #[test]
    fn split_heading_with_sentences() {
        let result = split_sentences(
            "Part I: The Beginning\n\nIt was a dark and stormy night. The wind howled.",
        );
        assert_eq!(
            result,
            vec![
                "Part I: The Beginning",
                "It was a dark and stormy night.",
                "The wind howled.",
            ]
        );
    }
}

#[cfg(test)]
mod breeze_request_tests {
    use super::*;

    /// The fallback is what keeps a refused rate from becoming no audio at
    /// all, and it must not fire on a 400 that has nothing to do with speed —
    /// that would swallow the engine's real error behind a duplicate failure.
    #[test]
    fn only_a_speed_carrying_400_is_retried() {
        assert!(retry_at_natural_speed(400, 1.5));
        assert!(retry_at_natural_speed(400, 0.75));
        assert!(!retry_at_natural_speed(400, 1.0));
        assert!(!retry_at_natural_speed(503, 1.5));
        assert!(!retry_at_natural_speed(200, 1.5));
    }

    /// Natural rate stays OFF the wire. An unconditional `speed: 1.0` is one
    /// engine-validation change away from 400-ing every render.
    #[test]
    fn natural_speed_is_not_put_on_the_wire() {
        let speed_of = |speed| {
            wire_speed(&TtsParams {
                speed,
                ..Default::default()
            })
        };
        assert_eq!(speed_of(None), None);
        assert_eq!(speed_of(Some(1.0)), None);
        assert_eq!(speed_of(Some(1.5)), Some(1.5));
        assert_eq!(speed_of(Some(0.75)), Some(0.75));
    }

    /// Out of range is a 400 upstream, and a 400 costs the caller their rate
    /// entirely — the fallback re-renders at 1x. Clamping keeps the intent.
    #[test]
    fn out_of_range_speed_is_clamped_rather_than_sent() {
        let speed_of = |speed| {
            wire_speed(&TtsParams {
                speed,
                ..Default::default()
            })
        };
        assert_eq!(speed_of(Some(6.0)), Some(4.0));
        assert_eq!(speed_of(Some(0.1)), Some(0.25));
        // The bounds themselves are valid and pass through untouched.
        assert_eq!(speed_of(Some(4.0)), Some(4.0));
        assert_eq!(speed_of(Some(0.25)), Some(0.25));
    }

    /// The chunking helpers are gone on purpose — `long` chunks server-side
    /// with rolling context. `split_sentences` survives for the non-aligned
    /// per-sentence fallback path only.
    #[test]
    fn sentence_splitting_survives_for_the_fallback_path() {
        assert_eq!(
            split_sentences("One. Two."),
            vec!["One.".to_string(), "Two.".to_string()]
        );
    }
}
