//! Speech-to-text via any OpenAI-compatible transcription service.
//!
//! Sends the file path to `{service_url}/v1/audio/transcriptions` so the STT
//! service reads directly from the shared filesystem (zero-copy).

mod utterances;

use serde::Deserialize;
use std::path::Path;
use tracing::{debug, info};

/// A single transcribed segment with timestamps and confidence metadata.
#[derive(Debug, Clone)]
pub struct TranscribedSegment {
    pub index: i32,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub text: String,
    pub avg_logprob: Option<f64>,
    pub no_speech_prob: Option<f64>,
    pub compression_ratio: Option<f64>,
}

/// Result of transcribing an audio file.
#[derive(Debug)]
pub struct TranscribeResult {
    pub segments: Vec<TranscribedSegment>,
}

/// Errors from transcription.
#[derive(Debug, thiserror::Error)]
pub enum TranscribeError {
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("transcription failed (HTTP {code}): {body}")]
    ServiceError { code: u16, body: String },
    #[error("failed to parse transcription response: {0}")]
    ParseError(String),
    #[error("HTTP request error: {0}")]
    HttpError(String),
    #[error("STT service not configured")]
    NotConfigured,
}

impl TranscribeError {
    /// Whether this failure is likely transient — i.e. the audio is probably
    /// fine and the request should be retried once the service recovers.
    ///
    /// Network/timeout failures and upstream 5xx responses (including the
    /// HTTP 502 the koblem gateway returns when it can't reach the STT
    /// backend during a crash/restart window) are transient. A 4xx, a parse
    /// failure, or a missing config are not — retrying won't change anything.
    pub fn is_transient(&self) -> bool {
        match self {
            TranscribeError::HttpError(_) => true,
            TranscribeError::ServiceError { code, .. } => *code >= 500,
            TranscribeError::IoError(_)
            | TranscribeError::ParseError(_)
            | TranscribeError::NotConfigured => false,
        }
    }
}

/// OpenAI-format transcription response.
#[derive(Debug, Deserialize)]
struct SttResponse {
    text: String,
    #[serde(default)]
    segments: Option<Vec<SttSegment>>,
    #[serde(default)]
    words: Vec<SttWord>,
}

#[derive(Debug, Deserialize)]
struct SttWord {
    start: f64,
    end: f64,
    #[serde(alias = "word")]
    text: String,
}

#[derive(Debug, Deserialize)]
struct SttSegment {
    start: f64,
    end: f64,
    text: String,
    avg_logprob: Option<f64>,
    no_speech_prob: Option<f64>,
    compression_ratio: Option<f64>,
}

/// Transcribe an audio file using an OpenAI-compatible STT service.
///
/// Sends the file path to `{service_url}/v1/audio/transcriptions` so the STT
/// service can read directly from the shared filesystem (zero-copy).
pub async fn transcribe_audio(
    service_url: &str,
    audio_path: &Path,
) -> Result<TranscribeResult, TranscribeError> {
    let url = format!(
        "{}/v1/audio/transcriptions",
        service_url.trim_end_matches('/')
    );

    info!(
        path = %audio_path.display(),
        url = %url,
        "Transcribing audio via STT service",
    );

    let file_path_str = audio_path.to_string_lossy();

    let form = reqwest::multipart::Form::new()
        .text("file_path", file_path_str.into_owned())
        .text("model", "whisper-1")
        .text("response_format", "verbose_json")
        // Parakeet's spelling (koblem passes it through); a server that does not know it
        // answers with segments only, which are stored as they come.
        .text("timestamp_granularity", "word");

    let client = crate::http::builder()
        .timeout(std::time::Duration::from_secs(3600))
        .build()
        .map_err(|e| TranscribeError::HttpError(e.to_string()))?;

    let response = client
        .post(&url)
        .multipart(form)
        .send()
        .await
        .map_err(|e| {
            let kind = if e.is_timeout() {
                "timeout"
            } else if e.is_connect() {
                "connection"
            } else if e.is_request() {
                "request"
            } else {
                "unknown"
            };
            TranscribeError::HttpError(format!("{kind} error: {e}"))
        })?;

    if !response.status().is_success() {
        let code = response.status().as_u16();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<failed to read body>".to_string());
        return Err(TranscribeError::ServiceError { code, body });
    }

    let body = response
        .text()
        .await
        .map_err(|e| TranscribeError::HttpError(e.to_string()))?;

    debug!(response_len = body.len(), "STT service response received");

    let parsed: SttResponse = serde_json::from_str(&body).map_err(|e| {
        TranscribeError::ParseError(format!("{}: {}", e, &body[..200.min(body.len())]))
    })?;

    let segments: Vec<TranscribedSegment> = if !parsed.words.is_empty() {
        let words: Vec<utterances::TimedWord> = parsed
            .words
            .into_iter()
            .map(|w| utterances::TimedWord {
                start: w.start,
                end: w.end,
                text: w.text,
            })
            .collect();
        utterances::from_words(&words)
    } else if let Some(segs) = parsed.segments {
        segs.into_iter()
            .enumerate()
            .map(|(i, seg)| TranscribedSegment {
                index: i as i32,
                start_seconds: seg.start,
                end_seconds: seg.end,
                text: seg.text,
                avg_logprob: seg.avg_logprob,
                no_speech_prob: seg.no_speech_prob,
                compression_ratio: seg.compression_ratio,
            })
            .filter(|s| !s.text.is_empty())
            .collect()
    } else if !parsed.text.is_empty() {
        // Service returned text but no segments — wrap in a single segment
        vec![TranscribedSegment {
            index: 0,
            start_seconds: 0.0,
            end_seconds: 0.0,
            text: parsed.text,
            avg_logprob: None,
            no_speech_prob: None,
            compression_ratio: None,
        }]
    } else {
        Vec::new()
    };

    info!(
        segments = segments.len(),
        path = %audio_path.display(),
        "Transcription complete",
    );

    Ok(TranscribeResult { segments })
}
