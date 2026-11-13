use api::watchdog::{
    IncidentLog, LongPoll, PollTracker, Probe, StallEvent, StallMonitor, build_runtime, exit_now,
    remote_wake_latency,
};
use chrono::{DateTime, Utc};
use std::ffi::CString;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::panic::Location;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CHILD_ENV: &str = "KOB_WATCHDOG_EXIT_CHILD";

extern "C" fn hang_forever() {
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

/// A native library's exit handler that never returns (libnvcuvid's did) kept the
/// watchdog's kill from ever finishing, leaving a wedged process Docker never restarted.
#[test]
fn exit_now_ignores_exit_handlers_that_hang() {
    if std::env::var_os(CHILD_ENV).is_some() {
        unsafe { libc::atexit(hang_forever) };
        exit_now(3);
    }

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "watchdog_test::exit_now_ignores_exit_handlers_that_hang",
            "--exact",
            "--nocapture",
        ])
        .env(CHILD_ENV, "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            panic!("process still alive 10s after exit_now");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(3));
}

#[test]
fn a_worker_stuck_in_one_poll_is_reported_with_where_its_task_was_spawned() {
    let tracker = PollTracker::new(2);
    let rt = build_runtime(2, tracker.clone()).unwrap();
    let spawned_on = line!() + 1;
    rt.spawn(async { std::thread::sleep(Duration::from_millis(1500)) });
    std::thread::sleep(Duration::from_millis(1100));

    let long = tracker.long_polls(Duration::from_secs(1));
    assert_eq!(long.len(), 1, "{long:?}");
    assert!(long[0].running_for >= Duration::from_secs(1));
    assert!(long[0].spawned_at.file().ends_with("watchdog_test.rs"));
    assert_eq!(long[0].spawned_at.line(), spawned_on);

    std::thread::sleep(Duration::from_millis(600));
    assert!(tracker.long_polls(Duration::from_secs(1)).is_empty());
    rt.shutdown_background();
}

fn first_long_poll(tracker: &PollTracker) -> LongPoll {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(poll) = tracker.long_polls(Duration::from_secs(1)).pop() {
            return poll;
        }
        assert!(Instant::now() < deadline, "no worker was ever stuck");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A NAS that stops answering parks a worker in a filesystem call. Naming the call and the
/// file is what tells that apart from a task hogging the CPU.
#[test]
fn a_worker_blocked_in_a_syscall_is_reported_with_the_call_and_the_file() {
    type Block = fn(&std::path::Path);
    let cases: [(&str, Block); 2] = [
        ("openat", |fifo| {
            let _ = std::fs::File::open(fifo);
        }),
        ("read", |fifo| {
            let mut f = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(fifo)
                .unwrap();
            let _ = f.read(&mut [0u8; 1]);
        }),
    ];
    for (call, block) in cases {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("nas-share-file");
        let c_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

        let tracker = PollTracker::new(2);
        let rt = build_runtime(2, tracker.clone()).unwrap();
        let path = fifo.clone();
        rt.spawn(async move { block(&path) });

        let blocked = first_long_poll(&tracker).blocked_in.expect("blocked_in");
        assert!(blocked.starts_with(call), "{call}: {blocked}");
        assert!(
            blocked.contains(&*fifo.to_string_lossy()),
            "{call}: {blocked}"
        );

        let _unblock = std::fs::OpenOptions::new().write(true).open(&fifo);
        std::io::Write::write_all(&mut _unblock.unwrap(), b"x").unwrap();
        rt.shutdown_background();
    }
}

#[test]
fn a_worker_burning_cpu_in_one_poll_is_reported_as_running() {
    let tracker = PollTracker::new(2);
    let rt = build_runtime(2, tracker.clone()).unwrap();
    rt.spawn(async {
        let until = Instant::now() + Duration::from_millis(1600);
        while Instant::now() < until {
            std::hint::spin_loop();
        }
    });
    assert_eq!(
        first_long_poll(&tracker).blocked_in.as_deref(),
        Some("running on CPU")
    );
    rt.shutdown_background();
}

#[test]
fn a_worker_stuck_in_a_job_names_the_job() {
    let tracker = PollTracker::new(2);
    let rt = build_runtime(2, tracker.clone()).unwrap();
    let job_id = uuid::Uuid::now_v7();
    rt.spawn(async move {
        let _running = nats_jobs::running::track_running("kobload_ingest", job_id);
        std::thread::sleep(Duration::from_millis(1500));
    });
    assert_eq!(
        first_long_poll(&tracker).job,
        Some(format!("kobload_ingest {job_id}"))
    );
    rt.shutdown_background();
}

#[test]
fn remote_wake_latency_is_prompt_on_a_healthy_runtime() {
    let rt = build_runtime(2, PollTracker::new(2)).unwrap();
    let latency = remote_wake_latency(rt.handle(), Duration::from_secs(2)).expect("task ran");
    assert!(latency < Duration::from_millis(100), "{latency:?}");
    rt.shutdown_background();
}

#[test]
fn remote_wake_latency_times_out_when_no_worker_is_free() {
    let rt = build_runtime(1, PollTracker::new(1)).unwrap();
    rt.spawn(async { std::thread::sleep(Duration::from_millis(800)) });
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        remote_wake_latency(rt.handle(), Duration::from_millis(200)),
        None
    );
    rt.shutdown_background();
}

fn at(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000 + secs, 0).unwrap()
}

fn kinds(events: &[StallEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|e| match e {
            StallEvent::Opened(_) => "opened",
            StallEvent::Ongoing(_) => "ongoing",
            StallEvent::Closed(_) => "closed",
        })
        .collect()
}

#[test]
fn slow_wakes_open_an_incident_only_when_they_repeat_and_close_after_a_quiet_minute() {
    let mut m = StallMonitor::default();
    let slow = Probe::Ran(Duration::from_millis(384));
    assert!(m.observe(at(0), &[], slow).is_empty());
    assert!(
        m.observe(at(5), &[], Probe::Ran(Duration::from_millis(3)))
            .is_empty()
    );
    assert!(m.observe(at(10), &[], slow).is_empty());
    assert_eq!(kinds(&m.observe(at(15), &[], Probe::TimedOut)), ["opened"]);
    assert!(m.observe(at(20), &[], slow).is_empty());
    assert!(m.observe(at(79), &[], Probe::NotRun).is_empty());

    let events = m.observe(at(81), &[], Probe::Ran(Duration::from_millis(2)));
    assert_eq!(kinds(&events), ["closed"]);
    let StallEvent::Closed(incident) = &events[0] else {
        unreachable!()
    };
    assert_eq!(incident.started_at, at(10));
    assert_eq!(incident.ended_at, Some(at(20)));
    assert_eq!(incident.slow_probes, 3);
    assert_eq!(incident.timed_out_probes, 1);
    assert!(!incident.notable());
}

#[test]
fn a_long_poll_opens_an_incident_at_once_and_names_where_the_task_was_spawned() {
    let mut m = StallMonitor::default();
    let here = Location::caller();
    let poll = |secs| LongPoll {
        worker: 3,
        running_for: Duration::from_secs(secs),
        spawned_at: here,
        job: Some("kobload_ingest 01a1d41c".into()),
        blocked_in: Some(format!(
            "newfstatat(\"/mnt/nasdaq/x.nfo\") in cifs_wait {secs}"
        )),
    };
    assert_eq!(
        kinds(&m.observe(at(0), &[poll(1)], Probe::NotRun)),
        ["opened"]
    );
    m.observe(at(6), &[poll(7)], Probe::NotRun);
    let events = m.observe(at(70), &[], Probe::NotRun);
    let StallEvent::Closed(incident) = &events[0] else {
        panic!("{:?}", kinds(&events))
    };
    assert_eq!(incident.culprits.len(), 1);
    assert_eq!(incident.culprits[0].longest_ms, 7_000);
    assert!(incident.culprits[0].spawned_at.contains("watchdog_test.rs"));
    assert!(incident.notable());
    let summary = incident.summary();
    assert!(
        summary.contains("kobload_ingest 01a1d41c")
            && summary.contains("newfstatat(\"/mnt/nasdaq/x.nfo\") in cifs_wait 7"),
        "{summary}"
    );
}

/// The 2026-11-12 wedge: wakes stayed slow for nine hours and nothing ever said so.
#[test]
fn a_stall_that_never_recovers_reports_itself_once_while_still_going() {
    let mut m = StallMonitor::default();
    let mut seen = Vec::new();
    for t in (0..=1_800).step_by(5) {
        seen.extend(kinds(&m.observe(
            at(t),
            &[],
            Probe::Ran(Duration::from_millis(577)),
        )));
    }
    assert_eq!(seen, ["opened", "ongoing"]);
}

#[test]
fn incident_log_keeps_history_and_hands_a_pending_kill_report_over_once() {
    let dir = tempfile::tempdir().unwrap();
    let log = IncidentLog::new(dir.path());
    let mut m = StallMonitor::default();
    m.observe(at(0), &[], Probe::TimedOut);
    let StallEvent::Opened(incident) = &m.observe(at(5), &[], Probe::TimedOut)[0] else {
        unreachable!()
    };

    log.append(incident).unwrap();
    log.append(incident).unwrap();
    assert_eq!(
        std::fs::read_to_string(log.history_path())
            .unwrap()
            .lines()
            .count(),
        2
    );

    log.write_pending_kill(incident).unwrap();
    let handed = log.take_pending_kill().expect("pending report");
    assert!(handed.killed);
    assert!(log.take_pending_kill().is_none());
}
