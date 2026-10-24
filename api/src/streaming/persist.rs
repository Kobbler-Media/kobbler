//! Playback session progress kept in Postgres so a restart can rebuild a session.
//!
//! A session's processes and segment index die with the API, but its segments stay on
//! disk. What a rebuild needs besides the request that opened it is small — where its
//! playlist starts, which generations own which segments, how far the player got — and
//! is snapshotted here so a client holding the old session's URLs keeps playing from
//! them.

use crate::AppState;
use crate::repositories::playback_sessions;
use crate::streaming::decision::PlaybackDecision;
use crate::streaming::segment_cache::SegmentCache;
use crate::streaming::session::PlaybackSession;
use kob_ffmpeg::session::PriorRun;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// How long a session survives unused, in memory and in its row alike.
pub const VOD_IDLE_TTL: Duration = Duration::from_secs(4 * 3600);

const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(5);

/// The idle clock is written only this often while nothing else changes; the expiry
/// sweep it feeds works in hours.
const ACTIVITY_WRITE_EVERY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Run {
    pub generation: u32,
    pub run_start: u32,
    pub start_secs: f64,
    pub end_segment: Option<u32>,
}

impl From<PriorRun> for Run {
    fn from(r: PriorRun) -> Self {
        Self {
            generation: r.generation,
            run_start: r.run_start,
            start_secs: r.start_secs,
            end_segment: r.end_segment,
        }
    }
}

impl From<Run> for PriorRun {
    fn from(r: Run) -> Self {
        Self {
            generation: r.generation,
            run_start: r.run_start,
            start_secs: r.start_secs,
            end_segment: r.end_segment,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    pub decision: PlaybackDecision,
    pub force_video_transcode: bool,
    pub seek_generation: u32,
    pub generation: u32,
    pub playback_start_segment: u32,
    pub playback_start_secs: f64,
    pub highest_served_segment: Option<u32>,
    pub runs: Vec<Run>,
    pub variant_runs: BTreeMap<String, Vec<Run>>,
}

impl Progress {
    pub fn of(session: &PlaybackSession) -> Self {
        let runs = session
            .transcode
            .as_ref()
            .map(|t| t.runs().into_iter().map(Run::from).collect())
            .unwrap_or_default();
        let variant_runs = session
            .variants
            .iter()
            .flatten()
            .map(|(id, v)| {
                let runs = match v.transcode {
                    Some(ref t) => t.runs(),
                    None => v.prior_runs.clone(),
                };
                (id.clone(), runs.into_iter().map(Run::from).collect())
            })
            .collect();
        Self {
            decision: session.decision.clone(),
            force_video_transcode: session.force_video_transcode,
            seek_generation: session.seek_generation,
            generation: session
                .transcode
                .as_ref()
                .map_or(0, |t| t.current_generation()),
            playback_start_segment: session.segment_cache.playback_start_segment,
            playback_start_secs: session.segment_cache.playback_start_secs,
            highest_served_segment: session.highest_served_segment,
            runs,
            variant_runs,
        }
    }

    pub fn prior_runs(&self) -> Vec<PriorRun> {
        self.runs.iter().copied().map(PriorRun::from).collect()
    }

    pub fn variant_prior_runs(&self, variant_id: &str) -> Vec<PriorRun> {
        self.variant_runs
            .get(variant_id)
            .into_iter()
            .flatten()
            .copied()
            .map(PriorRun::from)
            .collect()
    }

    /// A rebuilt session's master playlist must not be mistaken for the one before it.
    pub fn next_seek_generation(&self) -> u32 {
        self.seek_generation + 1
    }

    /// The first generation the rebuilt transcode may write without touching one on disk.
    pub fn next_generation(&self) -> u32 {
        self.runs
            .iter()
            .map(|r| r.generation)
            .chain([self.generation, self.seek_generation])
            .max()
            .unwrap_or(0)
            + 1
    }

    /// Where the rebuilt transcode picks up: the last segment the player asked for, so the
    /// run is cut at the end of whatever is cached from there rather than from the top.
    pub fn resume_secs(&self, cache: &SegmentCache) -> f64 {
        let segment = self
            .highest_served_segment
            .unwrap_or(self.playback_start_segment)
            .max(self.playback_start_segment);
        if segment == self.playback_start_segment {
            self.playback_start_secs
        } else {
            cache.segment_to_time(segment)
        }
    }
}

struct Written {
    progress: serde_json::Value,
    activity: Instant,
}

/// Snapshot every persisted session whose progress moved, until the process exits.
pub async fn run_snapshots(state: AppState) {
    let mut written: HashMap<Uuid, Written> = HashMap::new();
    let mut tick = tokio::time::interval(SNAPSHOT_INTERVAL);
    loop {
        tick.tick().await;
        snapshot(&state, &mut written, false).await;
    }
}

/// Write every persisted session's progress now, changed or not.
pub async fn flush(state: &AppState) {
    snapshot(state, &mut HashMap::new(), true).await;
}

async fn snapshot(state: &AppState, written: &mut HashMap<Uuid, Written>, force: bool) {
    let current: Vec<(Uuid, serde_json::Value, Instant)> = state
        .playback_sessions
        .iter()
        .filter(|e| e.persisted)
        .filter_map(|e| {
            let progress = serde_json::to_value(Progress::of(e.value())).ok()?;
            Some((*e.key(), progress, e.last_activity))
        })
        .collect();
    written.retain(|id, _| current.iter().any(|(c, _, _)| c == id));

    for (id, progress, activity) in current {
        let due = force
            || written.get(&id).is_none_or(|w| {
                w.progress != progress
                    || activity.saturating_duration_since(w.activity) >= ACTIVITY_WRITE_EVERY
            });
        if !due {
            continue;
        }
        let idle = activity.elapsed().as_secs_f64();
        match playback_sessions::save_progress(&state.pool, id, &progress, idle).await {
            Ok(()) => {
                written.insert(id, Written { progress, activity });
            }
            Err(e) => {
                tracing::warn!(session_id = %id, error = %e, "failed to save playback progress")
            }
        }
    }
}
