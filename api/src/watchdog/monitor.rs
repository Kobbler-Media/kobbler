use super::{Alert, LONG_POLL, Probe, Shared, StallEvent, StallMonitor, remote_wake_latency};
use chrono::Utc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

const TICK: Duration = Duration::from_secs(1);
const PROBE_EVERY_TICKS: u64 = 5;
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const STARTUP_GRACE: Duration = Duration::from_secs(60);

/// Watches for the stalls the health check is too coarse to see: a request that takes half a
/// second instead of five never fails it, and on 2026-11-12 that ran for nine hours unnoticed.
pub(super) fn run(shared: &Shared, alerts: &UnboundedSender<Alert>) {
    std::thread::sleep(STARTUP_GRACE);
    let mut monitor = StallMonitor::default();
    let mut tick = 0u64;
    loop {
        std::thread::sleep(TICK);
        tick += 1;
        let probe = if tick.is_multiple_of(PROBE_EVERY_TICKS) {
            remote_wake_latency(&shared.runtime, PROBE_TIMEOUT).map_or(Probe::TimedOut, Probe::Ran)
        } else {
            Probe::NotRun
        };
        let polls = shared.tracker.long_polls(LONG_POLL);
        for event in monitor.observe(Utc::now(), &polls, probe) {
            match event {
                StallEvent::Opened(i) => eprintln!("[watchdog] STALL started: {}", i.summary()),
                StallEvent::Ongoing(i) => {
                    eprintln!("[watchdog] STALL still going: {}", i.summary());
                    record(shared, &i);
                    let _ = alerts.send(Alert::Ongoing(i));
                }
                StallEvent::Closed(i) => {
                    eprintln!("[watchdog] STALL over: {}", i.summary());
                    record(shared, &i);
                    if i.notable() {
                        let _ = alerts.send(Alert::Recovered(i));
                    }
                }
            }
        }
        if let Ok(mut current) = shared.current.lock() {
            *current = monitor.current().cloned();
        }
    }
}

fn record(shared: &Shared, incident: &super::Incident) {
    if let Err(e) = shared.log.append(incident) {
        eprintln!("[watchdog] Could not record the incident: {e}");
    }
}
