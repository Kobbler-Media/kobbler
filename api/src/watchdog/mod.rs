mod alerts;
mod health;
mod incident;
mod monitor;
mod poll_tracker;

pub use alerts::{Alert, send_alerts};
pub use incident::{Culprit, Incident, IncidentLog, Probe, StallEvent, StallMonitor};
pub use poll_tracker::{LongPoll, PollTracker};

use crate::DbPool;
use chrono::Utc;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::runtime::{Handle, Runtime};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

const LONG_POLL: Duration = Duration::from_secs(1);

pub struct Watchdog {
    pub health_addr: SocketAddr,
    pub runtime: Handle,
    pub pool: DbPool,
    pub tracker: Arc<PollTracker>,
    pub incident_dir: PathBuf,
}

struct Shared {
    runtime: Handle,
    pool: DbPool,
    tracker: Arc<PollTracker>,
    log: IncidentLog,
    current: Mutex<Option<Incident>>,
}

impl Shared {
    fn kill_report(&self) -> Incident {
        let open = self.current.lock().ok().and_then(|c| c.clone());
        let mut incident = open.unwrap_or_else(|| Incident::new(Utc::now()));
        for poll in self.tracker.long_polls(LONG_POLL) {
            incident.record_poll(&poll);
        }
        incident
    }
}

/// Starts the health-check and stall-monitor threads. The returned receiver yields what is
/// worth telling someone about; [`send_alerts`] forwards it to the notification agents.
pub fn start(watchdog: Watchdog) -> UnboundedReceiver<Alert> {
    let (tx, rx) = unbounded_channel();
    let shared = Arc::new(Shared {
        runtime: watchdog.runtime,
        pool: watchdog.pool,
        tracker: watchdog.tracker,
        log: IncidentLog::new(watchdog.incident_dir),
        current: Mutex::new(None),
    });
    let health = shared.clone();
    std::thread::Builder::new()
        .name("watchdog".into())
        .spawn(move || health::run(watchdog.health_addr, &health))
        .expect("Failed to spawn watchdog thread");
    std::thread::Builder::new()
        .name("stall-monitor".into())
        .spawn(move || monitor::run(&shared, &tx))
        .expect("Failed to spawn stall monitor thread");
    rx
}

/// Ends the process without running exit handlers. libnvcuvid registers one that deadlocks
/// once the runtime is wedged, which turned a watchdog kill into a hang Docker never restarted.
pub fn exit_now(code: i32) -> ! {
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    unsafe { libc::_exit(code) }
}

pub fn build_runtime(workers: usize, tracker: Arc<PollTracker>) -> std::io::Result<Runtime> {
    let after = tracker.clone();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .on_before_task_poll(move |meta| tracker.before_poll(meta))
        .on_after_task_poll(move |_| after.after_poll())
        .build()
}

/// How long a task spawned from outside the runtime waits for a worker to run it — the path
/// every accepted connection and every finished `spawn_blocking` read takes. None if it
/// did not run within `timeout`.
pub fn remote_wake_latency(handle: &Handle, timeout: Duration) -> Option<Duration> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let sent = Instant::now();
    handle.spawn(async move {
        let _ = tx.send(sent.elapsed());
    });
    rx.recv_timeout(timeout).ok()
}
