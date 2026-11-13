use super::LongPoll;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const SLOW_WAKE: Duration = Duration::from_millis(250);
const SLOW_PROBES_TO_OPEN: u32 = 2;
const QUIET_TO_CLOSE: Duration = Duration::from_secs(60);
const ONGOING_AFTER: Duration = Duration::from_secs(120);
const NOTABLE_DURATION: Duration = Duration::from_secs(30);
const NOTABLE_POLL_MS: u64 = 5_000;

#[derive(Debug, Clone, Copy)]
pub enum Probe {
    NotRun,
    Ran(Duration),
    TimedOut,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Culprit {
    pub spawned_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    pub longest_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_in: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Incident {
    pub started_at: DateTime<Utc>,
    pub last_bad_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub slow_probes: u32,
    pub timed_out_probes: u32,
    pub worst_wake_ms: Option<u64>,
    pub culprits: Vec<Culprit>,
    pub killed: bool,
}

impl Incident {
    pub(super) fn new(started_at: DateTime<Utc>) -> Self {
        Self {
            started_at,
            last_bad_at: started_at,
            ended_at: None,
            slow_probes: 0,
            timed_out_probes: 0,
            worst_wake_ms: None,
            culprits: Vec::new(),
            killed: false,
        }
    }

    pub fn duration(&self) -> Duration {
        (self.ended_at.unwrap_or(self.last_bad_at) - self.started_at)
            .to_std()
            .unwrap_or_default()
    }

    pub fn notable(&self) -> bool {
        self.killed
            || self.duration() >= NOTABLE_DURATION
            || self
                .culprits
                .iter()
                .any(|c| c.longest_ms >= NOTABLE_POLL_MS)
    }

    pub fn summary(&self) -> String {
        let mut parts = vec![format!(
            "for {}s from {}",
            self.duration().as_secs(),
            self.started_at.format("%Y-%m-%d %H:%M:%S UTC")
        )];
        if self.slow_probes > 0 {
            let worst = match self.worst_wake_ms {
                Some(ms) if self.timed_out_probes == 0 => format!("worst {ms} ms"),
                _ => format!("{} never ran", self.timed_out_probes),
            };
            parts.push(format!("{} slow task wake-ups ({worst})", self.slow_probes));
        }
        for c in &self.culprits {
            let secs = c.longest_ms as f64 / 1000.0;
            let mut part = match &c.job {
                Some(job) => format!(
                    "a worker spent {secs:.1}s in one poll of job {job} (spawned at {})",
                    c.spawned_at
                ),
                None => format!(
                    "a worker spent {secs:.1}s in one poll of a task spawned at {}",
                    c.spawned_at
                ),
            };
            if let Some(blocked) = &c.blocked_in {
                part.push_str(&format!(", last seen in {blocked}"));
            }
            parts.push(part);
        }
        if self.killed {
            parts.push("the watchdog killed the process".into());
        }
        parts.join("; ")
    }

    fn record_slow(&mut self, probe: Probe) {
        self.slow_probes += 1;
        match probe {
            Probe::TimedOut => self.timed_out_probes += 1,
            Probe::Ran(d) => {
                let ms = d.as_millis() as u64;
                self.worst_wake_ms = Some(self.worst_wake_ms.map_or(ms, |w| w.max(ms)));
            }
            Probe::NotRun => {}
        }
    }

    pub(super) fn record_poll(&mut self, poll: &LongPoll) {
        let at = poll.spawned_at.to_string();
        let ms = poll.running_for.as_millis() as u64;
        match self
            .culprits
            .iter_mut()
            .find(|c| c.spawned_at == at && c.job == poll.job)
        {
            Some(c) if ms >= c.longest_ms => {
                c.longest_ms = ms;
                if poll.blocked_in.is_some() {
                    c.blocked_in.clone_from(&poll.blocked_in);
                }
            }
            Some(_) => {}
            None => self.culprits.push(Culprit {
                spawned_at: at,
                job: poll.job.clone(),
                longest_ms: ms,
                blocked_in: poll.blocked_in.clone(),
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub enum StallEvent {
    Opened(Incident),
    Ongoing(Incident),
    Closed(Incident),
}

/// Turns once-a-tick observations into incidents. A single slow wake-up is noise; two in a
/// row, or any worker stuck in one poll, is a stall worth naming.
#[derive(Default)]
pub struct StallMonitor {
    open: Option<Incident>,
    ongoing_sent: bool,
    slow_streak: Vec<Probe>,
    streak_started: Option<DateTime<Utc>>,
}

impl StallMonitor {
    pub fn observe(
        &mut self,
        now: DateTime<Utc>,
        long_polls: &[LongPoll],
        probe: Probe,
    ) -> Vec<StallEvent> {
        let slow = match probe {
            Probe::Ran(d) => d >= SLOW_WAKE,
            Probe::TimedOut => true,
            Probe::NotRun => false,
        };
        if matches!(probe, Probe::Ran(d) if d < SLOW_WAKE) {
            self.slow_streak.clear();
            self.streak_started = None;
        }
        if slow {
            self.streak_started.get_or_insert(now);
            self.slow_streak.push(probe);
        }

        let wakes_stalled = self.slow_streak.len() as u32 >= SLOW_PROBES_TO_OPEN;
        let mut events = Vec::new();

        if self.open.is_none() && (wakes_stalled || !long_polls.is_empty()) {
            let start = if wakes_stalled {
                self.streak_started.unwrap_or(now)
            } else {
                now
            };
            let mut incident = Incident::new(start);
            for p in self.slow_streak.drain(..) {
                incident.record_slow(p);
            }
            incident.last_bad_at = now;
            long_polls.iter().for_each(|p| incident.record_poll(p));
            self.ongoing_sent = false;
            events.push(StallEvent::Opened(incident.clone()));
            self.open = Some(incident);
            return events;
        }

        let Some(incident) = self.open.as_mut() else {
            return events;
        };
        if slow {
            incident.record_slow(probe);
            self.slow_streak.clear();
        }
        long_polls.iter().for_each(|p| incident.record_poll(p));
        if slow || !long_polls.is_empty() {
            incident.last_bad_at = now;
        }

        let since = |t: DateTime<Utc>| (now - t).to_std().unwrap_or_default();
        if since(incident.last_bad_at) >= QUIET_TO_CLOSE {
            incident.ended_at = Some(incident.last_bad_at);
            events.push(StallEvent::Closed(incident.clone()));
            self.open = None;
        } else if !self.ongoing_sent && since(incident.started_at) >= ONGOING_AFTER {
            self.ongoing_sent = true;
            events.push(StallEvent::Ongoing(incident.clone()));
        }
        events
    }

    pub fn current(&self) -> Option<&Incident> {
        self.open.as_ref()
    }
}

/// `incidents.jsonl` is the history an operator or agent reads; `pending-kill.json` carries
/// the last incident across a watchdog kill so the next boot can say what happened.
pub struct IncidentLog {
    dir: PathBuf,
}

impl IncidentLog {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn history_path(&self) -> PathBuf {
        self.dir.join("incidents.jsonl")
    }

    fn pending_kill_path(&self) -> PathBuf {
        self.dir.join("pending-kill.json")
    }

    pub fn append(&self, incident: &Incident) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let mut line = serde_json::to_vec(incident)?;
        line.push(b'\n');
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.history_path())?
            .write_all(&line)
    }

    pub fn write_pending_kill(&self, incident: &Incident) -> std::io::Result<()> {
        let mut killed = incident.clone();
        killed.killed = true;
        killed.ended_at.get_or_insert_with(Utc::now);
        self.append(&killed)?;
        fs::write(self.pending_kill_path(), serde_json::to_vec(&killed)?)
    }

    pub fn take_pending_kill(&self) -> Option<Incident> {
        let path = self.pending_kill_path();
        let bytes = fs::read(&path).ok()?;
        let _ = fs::remove_file(&path);
        serde_json::from_slice(&bytes).ok()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}
