//! In-memory debrid stream session management.
//!
//! Tracks active debrid streaming sessions with concurrent access via [`DashMap`]
//! and per-session SSE event broadcasting for progressive feature loading.

use dashmap::DashMap;
use dashmap::mapref::one::{Ref, RefMut};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use time::OffsetDateTime;
use tokio::sync::broadcast;
use uuid::Uuid;

/// Broadcast channel capacity per session.
const EVENT_CHANNEL_CAPACITY: usize = 64;

/// An active debrid stream session tracked in memory.
pub struct DebridStreamSession {
    pub id: Uuid,
    pub user_id: Uuid,
    pub media_type: String,
    pub media_id: Uuid,
    /// Current debrid direct URL (ephemeral, may need re-resolving).
    pub debrid_url: String,
    /// Debrid provider ID for re-resolving expired URLs.
    pub debrid_provider_id: Uuid,
    pub info_hash: Option<String>,
    pub release_title: String,
    pub filename: String,
    pub filesize: i64,

    // Progressive download state
    /// Local cache path if the file is being downloaded for processing.
    pub cache_path: Option<PathBuf>,
    /// Download progress from 0.0 to 1.0.
    pub download_progress: f64,
    /// Whether the file is fully cached locally.
    pub fully_cached: bool,

    // Processing state
    pub probe_complete: bool,
    pub subtitles_ready: bool,
    pub keyframes_ready: bool,
    pub thumbnails_ready: bool,
    pub skip_segments_ready: bool,

    // Playback state
    pub progress_seconds: f64,
    pub last_activity: Instant,
    pub created_at: OffsetDateTime,
}

impl DebridStreamSession {
    /// Update the last activity timestamp.
    pub fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Whether this session has been inactive longer than the given duration.
    pub fn is_expired(&self, max_idle: std::time::Duration) -> bool {
        self.last_activity.elapsed() > max_idle
    }
}

/// SSE events emitted as debrid stream features become available.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DebridSessionEvent {
    Connected,
    DownloadProgress { progress: f64 },
    ProbeComplete { duration: f64, codecs: String },
    SubtitlesReady { tracks: Vec<SubtitleTrackInfo> },
    KeyframesReady,
    ThumbnailsReady { vtt_url: String },
    SkipSegmentsReady { segments: Vec<SkipSegmentInfo> },
    Error { message: String },
}

/// Metadata for a subtitle track discovered during probing.
#[derive(Debug, Clone, Serialize)]
pub struct SubtitleTrackInfo {
    pub index: i32,
    pub language: Option<String>,
    pub codec: String,
}

/// A skip segment (intro, credits, etc.) with time boundaries.
#[derive(Debug, Clone, Serialize)]
pub struct SkipSegmentInfo {
    pub segment_type: String,
    pub start_seconds: f64,
    pub end_seconds: f64,
}

/// Concurrent store for active debrid stream sessions with per-session SSE broadcasting.
pub struct DebridSessionStore {
    sessions: DashMap<Uuid, DebridStreamSession>,
    /// Per-session broadcast senders for SSE events.
    channels: DashMap<Uuid, broadcast::Sender<DebridSessionEvent>>,
}

impl DebridSessionStore {
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
            channels: DashMap::new(),
        }
    }

    pub fn get(&self, id: &Uuid) -> Option<Ref<'_, Uuid, DebridStreamSession>> {
        self.sessions.get(id)
    }

    pub fn get_mut(&self, id: &Uuid) -> Option<RefMut<'_, Uuid, DebridStreamSession>> {
        self.sessions.get_mut(id)
    }

    pub fn insert(&self, session: DebridStreamSession) {
        let id = session.id;
        self.sessions.insert(id, session);
        self.channels
            .entry(id)
            .or_insert_with(|| broadcast::channel(EVENT_CHANNEL_CAPACITY).0);
    }

    pub fn remove(&self, id: &Uuid) -> Option<DebridStreamSession> {
        self.channels.remove(id);
        self.sessions.remove(id).map(|(_, s)| s)
    }

    pub fn active_count(&self) -> usize {
        self.sessions.len()
    }

    /// Subscribe to SSE events for a session. Creates the channel if it doesn't exist yet.
    pub fn subscribe(&self, session_id: Uuid) -> broadcast::Receiver<DebridSessionEvent> {
        self.channels
            .entry(session_id)
            .or_insert_with(|| broadcast::channel(EVENT_CHANNEL_CAPACITY).0)
            .subscribe()
    }

    /// Emit an SSE event to all subscribers of a session.
    /// Silently drops the event if there are no active subscribers.
    pub fn emit(&self, session_id: Uuid, event: DebridSessionEvent) {
        if let Some(tx) = self.channels.get(&session_id) {
            let _ = tx.send(event);
        }
    }
}

impl Default for DebridSessionStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared handle to the debrid session store.
pub type SharedDebridSessionStore = Arc<DebridSessionStore>;
