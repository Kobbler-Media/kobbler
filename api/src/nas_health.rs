//! CIFS NAS contention monitoring.
//!
//! Opt-in (config flag `nas_health_monitoring`). A background tick reads
//! `/proc/fs/cifs/Stats` every 60 seconds and tracks the "share reconnects"
//! delta over the last hour. Share reconnects happen when the SMB session
//! drops, which only happens when the NAS is so overwhelmed it stops
//! responding — a clean "things are too hot" signal that doesn't
//! false-positive on normal load.
//!
//! Threshold: >5 share reconnects in the last hour is considered degraded.
//! Zero bytes readable / missing procfs file is "unreachable" (the mount
//! might be dead, or the container just doesn't have /proc/fs/cifs mapped
//! through).

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

const POLL_INTERVAL: Duration = Duration::from_secs(60);
const WINDOW: Duration = Duration::from_secs(3600);
const DEGRADED_THRESHOLD: u64 = 5;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NasHealthStatus {
    Healthy,
    Degraded,
    Unreachable,
    Disabled,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct NasHealthSnapshot {
    pub status: NasHealthStatus,
    pub reconnects_last_hour: u64,
    pub note: Option<String>,
}

impl NasHealthSnapshot {
    pub fn disabled() -> Self {
        Self {
            status: NasHealthStatus::Disabled,
            reconnects_last_hour: 0,
            note: None,
        }
    }
}

#[derive(Clone, Default)]
pub struct NasHealthMonitor {
    state: Arc<RwLock<MonitorState>>,
}

#[derive(Default)]
struct MonitorState {
    enabled: bool,
    samples: VecDeque<(Instant, u64)>, // (time, cumulative share reconnects)
    latest: NasHealthSnapshot,
}

impl Default for NasHealthSnapshot {
    fn default() -> Self {
        Self::disabled()
    }
}

impl NasHealthMonitor {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn snapshot(&self) -> NasHealthSnapshot {
        self.state.read().await.latest.clone()
    }

    /// Spawn the background polling loop. Does nothing if monitoring is disabled.
    ///
    /// `stats_path` is where the monitor reads cumulative SMB share-reconnect
    /// counters from. Defaults to `/proc/fs/cifs/Stats` for bare-metal, but
    /// containerised deploys must override it: runc forbids bind-mounting any
    /// file inside `/proc` into the container, so the usual pattern is a
    /// host-side cron that copies `/proc/fs/cifs/Stats` to something like
    /// `/var/lib/kobbler/nas-stats` every minute and bind-mounts *that* file
    /// into the api container.
    pub fn spawn(self, enabled: bool, stats_path: String) {
        if !enabled {
            return;
        }
        tokio::spawn(async move {
            {
                let mut s = self.state.write().await;
                s.enabled = true;
                // Flip from disabled → healthy so the frontend knows the feature
                // is on and worth polling, even before the first real sample.
                s.latest = NasHealthSnapshot {
                    status: NasHealthStatus::Healthy,
                    reconnects_last_hour: 0,
                    note: None,
                };
            }
            let mut ticker = tokio::time::interval(POLL_INTERVAL);
            loop {
                ticker.tick().await;
                let reconnects = match read_share_reconnects(&stats_path).await {
                    Some(v) => v,
                    None => {
                        let mut s = self.state.write().await;
                        s.latest = NasHealthSnapshot {
                            status: NasHealthStatus::Unreachable,
                            reconnects_last_hour: 0,
                            note: Some(format!("cannot read {stats_path}")),
                        };
                        continue;
                    }
                };
                let now = Instant::now();
                let mut s = self.state.write().await;
                s.samples.push_back((now, reconnects));
                while let Some((t, _)) = s.samples.front()
                    && now.duration_since(*t) > WINDOW
                {
                    s.samples.pop_front();
                }
                let delta = if let (Some(first), Some(last)) = (s.samples.front(), s.samples.back())
                {
                    last.1.saturating_sub(first.1)
                } else {
                    0
                };
                let status = if delta > DEGRADED_THRESHOLD {
                    NasHealthStatus::Degraded
                } else {
                    NasHealthStatus::Healthy
                };
                s.latest = NasHealthSnapshot {
                    status,
                    reconnects_last_hour: delta,
                    note: None,
                };
            }
        });
    }
}

/// Read the sum of "share reconnects" counters from the configured stats file
/// (normally `/proc/fs/cifs/Stats` or a host-copied mirror of it).
/// Returns None if the file can't be read or parsed.
async fn read_share_reconnects(stats_path: &str) -> Option<u64> {
    let contents = tokio::fs::read_to_string(stats_path).await.ok()?;
    let mut total: u64 = 0;
    let mut any = false;
    for line in contents.lines() {
        // Lines look like: "N) \\host\share   <stats>"
        // followed by "Stats since ... share reconnects"
        // The actual counter is on lines with "share reconnects".
        if let Some(idx) = line.find("share reconnects") {
            // Count appears before "share reconnects" on the same line as a trailing int.
            let before = &line[..idx];
            if let Some(n) = before
                .split_whitespace()
                .last()
                .and_then(|s| s.parse::<u64>().ok())
            {
                total = total.saturating_add(n);
                any = true;
            }
        }
    }
    if any { Some(total) } else { None }
}
