//! Opening a tuner stream, falling back across a stack's members.
//!
//! An HDHomeRun on a mux it cannot lock answers a tune with `503` only after ~10 s,
//! and never sends a byte at all on a marginal one. Both are the same failure to a
//! viewer, so a candidate gets a deadline for its first byte rather than for its
//! headers, and a stacked channel moves on to its next member when it misses it.
//!
//! No signal is a property of the mux, not the service: every member on the same
//! frequency fails the same way, so once one does the rest of that mux is skipped.

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use uuid::Uuid;

use super::live_session::ByteStream;

pub const FIRST_BYTE_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct SourceCandidate {
    pub channel_id: Uuid,
    pub name: String,
    pub url: String,
    pub frequency: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    Status {
        status: u16,
        tuner_error: Option<String>,
    },
    Unreachable(String),
    NoFirstByte,
    Ended,
}

impl OpenError {
    /// `807 No Video Data` is the device saying it found no lock, and a tuner that
    /// answers but never sends is the same thing without the courtesy of saying so.
    pub fn is_no_signal(&self) -> bool {
        match self {
            OpenError::Status {
                tuner_error: Some(t),
                ..
            } => t.starts_with("807"),
            OpenError::NoFirstByte => true,
            _ => false,
        }
    }
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Status {
                status,
                tuner_error: Some(e),
            } => write!(f, "HTTP {status} ({e})"),
            OpenError::Status { status, .. } => write!(f, "HTTP {status}"),
            OpenError::Unreachable(e) => write!(f, "unreachable: {e}"),
            OpenError::NoFirstByte => write!(f, "no data before the first-byte deadline"),
            OpenError::Ended => write!(f, "stream ended before any data"),
        }
    }
}

pub struct Opened {
    pub index: usize,
    pub first: Bytes,
    pub body: ByteStream,
    pub content_type: String,
}

async fn open_one(
    http: &reqwest::Client,
    url: &str,
    deadline: Duration,
) -> Result<(Bytes, ByteStream, String), OpenError> {
    let attempt = async {
        let resp = http
            .get(url)
            .timeout(Duration::from_secs(86400))
            .send()
            .await
            .map_err(|e| OpenError::Unreachable(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(OpenError::Status {
                status: resp.status().as_u16(),
                tuner_error: resp
                    .headers()
                    .get("x-hdhomerun-error")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string),
            });
        }
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("video/mp2t")
            .to_string();
        let mut body: ByteStream = Box::pin(resp.bytes_stream());
        loop {
            match body.next().await {
                Some(Ok(chunk)) if chunk.is_empty() => continue,
                Some(Ok(chunk)) => return Ok((chunk, body, content_type)),
                Some(Err(e)) => return Err(OpenError::Unreachable(e.to_string())),
                None => return Err(OpenError::Ended),
            }
        }
    };
    tokio::time::timeout(deadline, attempt)
        .await
        .unwrap_or(Err(OpenError::NoFirstByte))
}

/// The first candidate, in order, to deliver a byte. Every failure is logged as it
/// happens and handed back when they all fail.
pub async fn open_first(
    http: &reqwest::Client,
    candidates: &[SourceCandidate],
    deadline: Duration,
    cancel: &CancellationToken,
) -> Result<Opened, Vec<OpenError>> {
    let mut errors = Vec::new();
    let mut dead_muxes: HashSet<i64> = HashSet::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if dead_muxes.contains(&candidate.frequency) {
            continue;
        }
        let outcome = tokio::select! {
            _ = cancel.cancelled() => return Err(errors),
            o = open_one(http, &candidate.url, deadline) => o,
        };
        match outcome {
            Ok((first, body, content_type)) => {
                return Ok(Opened {
                    index,
                    first,
                    body,
                    content_type,
                });
            }
            Err(e) => {
                warn!(
                    channel = %candidate.name,
                    channel_id = %candidate.channel_id,
                    url = %candidate.url,
                    error = %e,
                    "live TV source connect failed"
                );
                if e.is_no_signal() {
                    dead_muxes.insert(candidate.frequency);
                }
                errors.push(e);
            }
        }
    }
    Err(errors)
}

/// What a viewer is told when no candidate tuned, named for the channel they picked.
pub fn failure_message(name: &str, errors: &[OpenError]) -> String {
    let tuner_error = errors.iter().find_map(|e| match e {
        OpenError::Status {
            tuner_error: Some(t),
            ..
        } => Some(t.as_str()),
        _ => None,
    });
    if let Some(t) = tuner_error
        && t.starts_with("805")
    {
        return format!("All tuners are in use, so {name} couldn't be tuned");
    }
    if let Some(t) = tuner_error
        && (t.starts_with("801") || t.starts_with("803") || t.starts_with("804"))
    {
        return format!("The tuner refused {name} ({t})");
    }
    if errors
        .iter()
        .all(|e| matches!(e, OpenError::Unreachable(_)))
    {
        return format!("The tuner didn't answer when tuning {name}");
    }
    format!("{name} has no signal right now (the tuner couldn't lock onto it)")
}

/// A tune's progress as the viewer-facing routes see it.
#[derive(Debug, Default)]
pub struct TuneHealth {
    delivered: AtomicBool,
    failure: Mutex<Option<String>>,
    dead: AtomicBool,
    died: tokio::sync::Notify,
    changed: tokio::sync::Notify,
}

impl TuneHealth {
    pub fn delivered(&self) -> bool {
        self.delivered.load(Ordering::Acquire)
    }

    pub fn mark_delivered(&self) {
        self.delivered.store(true, Ordering::Release);
        if let Ok(mut f) = self.failure.lock() {
            *f = None;
        }
        self.changed.notify_waiters();
    }

    pub fn fail(&self, message: String) {
        if let Ok(mut f) = self.failure.lock() {
            *f = Some(message);
        }
        self.changed.notify_waiters();
    }

    /// Returns as the first byte arrives, or with why the tune has produced none. Running
    /// out of `wait` is not a failure: the stream is handed over and may yet start.
    pub async fn first_byte(&self, wait: Duration) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let changed = self.changed.notified();
            if self.delivered() {
                return Ok(());
            }
            if let Some(reason) = self.failure() {
                return Err(reason);
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return Ok(());
            }
        }
    }

    /// The tune is over; the first reason given is the one kept.
    pub fn die(&self, reason: String) {
        if self.dead.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Ok(mut f) = self.failure.lock() {
            *f = Some(reason);
        }
        self.died.notify_waiters();
        self.changed.notify_waiters();
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Acquire)
    }

    pub async fn wait_dead(&self) {
        loop {
            let notified = self.died.notified();
            if self.is_dead() {
                return;
            }
            notified.await;
        }
    }

    /// The last failure, delivered or not.
    pub fn reason(&self) -> Option<String> {
        self.failure.lock().ok().and_then(|f| f.clone())
    }

    /// Only a tune that never produced a byte has failed; one that dropped later is
    /// reconnecting and still has a buffer to serve.
    pub fn failure(&self) -> Option<String> {
        if self.delivered() {
            return None;
        }
        self.failure.lock().ok().and_then(|f| f.clone())
    }
}
