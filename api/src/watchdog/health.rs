use super::{Shared, exit_now};

/// Sends a raw HTTP request to the health endpoint every 30 seconds. If 3 consecutive checks
/// fail, dumps runtime state, records the incident for the next boot to report, and kills the
/// process so Docker can restart it. Runs on a plain OS thread so it works even when the async
/// runtime is completely hung.
pub(super) fn run(addr: std::net::SocketAddr, shared: &Shared) {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    const CHECK_INTERVAL: Duration = Duration::from_secs(30);
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
    const READ_TIMEOUT: Duration = Duration::from_secs(5);
    const MAX_FAILURES: u32 = 3;
    const STARTUP_GRACE: Duration = Duration::from_secs(60);
    const DUMP_TIMEOUT: Duration = Duration::from_secs(10);

    std::thread::sleep(STARTUP_GRACE);

    let mut consecutive_failures: u32 = 0;

    loop {
        std::thread::sleep(CHECK_INTERVAL);

        let ok = (|| -> bool {
            let mut stream = match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                Ok(s) => s,
                Err(_) => return false,
            };
            if stream.set_read_timeout(Some(READ_TIMEOUT)).is_err() {
                return false;
            }
            if stream
                .write_all(b"GET /health HTTP/1.0\r\nHost: localhost\r\n\r\n")
                .is_err()
            {
                return false;
            }
            let mut buf = [0u8; 256];
            match stream.read(&mut buf) {
                Ok(n) if n > 0 => String::from_utf8_lossy(&buf[..n]).contains("200"),
                _ => false,
            }
        })();

        if ok {
            if consecutive_failures > 0 {
                eprintln!(
                    "[watchdog] Health check recovered after {} consecutive failure(s)",
                    consecutive_failures
                );
            }
            consecutive_failures = 0;
        } else {
            consecutive_failures += 1;
            eprintln!(
                "[watchdog] Health check FAILED ({}/{})",
                consecutive_failures, MAX_FAILURES
            );

            // Dump runtime metrics on the FIRST failure, not just at kill time.
            // A worker stuck in a long synchronous call looks identical to an
            // idle one from outside the process: load is low, queues are empty,
            // the DB pool is healthy. These counters are the only cheap signal
            // that says otherwise, and a stall that recovers before the third
            // check never reached the kill-time dump -- so the failure mode
            // stayed invisible for as long as it kept recovering. Metrics only:
            // the async task dump is expensive and stays at kill time.
            if consecutive_failures == 1 {
                eprintln!(
                    "[watchdog] First failure — dumping runtime metrics. A single busy \
                     worker with everything else parked means blocking work on the runtime."
                );
                dump_runtime_metrics(&shared.runtime);
                dump_long_polls(shared);
            }

            if consecutive_failures >= MAX_FAILURES {
                if let Err(e) = shared.log.write_pending_kill(&shared.kill_report()) {
                    eprintln!("[watchdog] Could not record the incident: {e}");
                }
                dump_runtime_metrics(&shared.runtime);
                dump_long_polls(shared);
                dump_db_pool_stats(&shared.pool);
                // Last, and only because the process is about to go: a dump that times out
                // leaves the workers it reached parked at its barrier for good.
                dump_async_task_traces(&shared.runtime, DUMP_TIMEOUT);

                let total_secs = MAX_FAILURES * CHECK_INTERVAL.as_secs() as u32
                    + CONNECT_TIMEOUT.as_secs() as u32 * MAX_FAILURES;

                eprintln!(
                    r#"
╔══════════════════════════════════════════════════════════════════════════╗
║                                                                          ║
║   ██╗    ██╗ █████╗ ████████╗ ██████╗██╗  ██╗██████╗  ██████╗  ██████╗  ║
║   ██║    ██║██╔══██╗╚══██╔══╝██╔════╝██║  ██║██╔══██╗██╔═══██╗██╔════╝  ║
║   ██║ █╗ ██║███████║   ██║   ██║     ███████║██║  ██║██║   ██║██║  ███╗  ║
║   ██║███╗██║██╔══██║   ██║   ██║     ██╔══██║██║  ██║██║   ██║██║   ██║  ║
║   ╚███╔███╔╝██║  ██║   ██║   ╚██████╗██║  ██║██████╔╝╚██████╔╝╚██████╔╝ ║
║    ╚══╝╚══╝ ╚═╝  ╚═╝   ╚═╝    ╚═════╝╚═╝  ╚═╝╚═════╝  ╚═════╝  ╚═════╝ ║
║                                                                          ║
║   KOBBLER API IS UNRESPONSIVE — KILLING PROCESS                          ║
║                                                                          ║
║   /health failed {} times in a row (~{}s with no response).               ║
║   Runtime metrics and async task traces have been dumped above.           ║
║                                                                          ║
║   Exiting with code 1 so Docker/systemd can restart the container.       ║
║                                                                          ║
╚══════════════════════════════════════════════════════════════════════════╝
"#,
                    MAX_FAILURES, total_secs,
                );
                exit_now(1);
            }
        }
    }
}

fn dump_long_polls(shared: &Shared) {
    for p in shared.tracker.long_polls(super::LONG_POLL) {
        eprintln!(
            "[watchdog]   Worker {} has been in one poll for {:.1}s: task spawned at {}",
            p.worker,
            p.running_for.as_secs_f64(),
            p.spawned_at
        );
    }
}

fn dump_runtime_metrics(handle: &tokio::runtime::Handle) {
    let m = handle.metrics();
    eprintln!("\n[watchdog] ═══ TOKIO RUNTIME STATE ═══");
    eprintln!("[watchdog]   Workers:           {}", m.num_workers());
    eprintln!(
        "[watchdog]   Blocking threads:  {}",
        m.num_blocking_threads()
    );
    eprintln!("[watchdog]   Active tasks:      {}", m.num_alive_tasks());
    eprintln!("[watchdog]   Global queue:      {}", m.global_queue_depth());
    eprintln!(
        "[watchdog]   Blocking queue:    {}",
        m.blocking_queue_depth()
    );
    for i in 0..m.num_workers() {
        eprintln!(
            "[watchdog]   Worker {}: local_queue={}, steal_count={}, poll_count={}, park_count={}",
            i,
            m.worker_local_queue_depth(i),
            m.worker_steal_count(i),
            m.worker_poll_count(i),
            m.worker_park_count(i),
        );
    }
}

fn dump_db_pool_stats(pool: &crate::DbPool) {
    eprintln!("\n[watchdog] ═══ DATABASE CONNECTION POOL ═══");
    eprintln!(
        "[watchdog]   Connections: {} total, {} idle (max: {})",
        pool.size(),
        pool.num_idle(),
        pool.options().get_max_connections(),
    );
    if pool.num_idle() == 0 && pool.size() >= pool.options().get_max_connections() {
        eprintln!(
            "[watchdog]   ⚠ POOL EXHAUSTED — all {} connections in use, new queries are blocking",
            pool.size()
        );
    }
}

fn dump_async_task_traces(handle: &tokio::runtime::Handle, timeout: std::time::Duration) {
    eprintln!("\n[watchdog] ═══ ASYNC TASK DUMP ═══");
    eprintln!(
        "[watchdog]   Capturing task traces ({}s timeout)...",
        timeout.as_secs()
    );

    // Use a throwaway single-threaded runtime to drive Handle::dump().
    // This works even if the main runtime is completely deadlocked because
    // dump() signals worker threads via OS-level mechanisms.
    let dump_rt = match tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("[watchdog]   ✗ Failed to create dump runtime: {e}");
            return;
        }
    };

    let result = dump_rt.block_on(async { tokio::time::timeout(timeout, handle.dump()).await });

    match result {
        Ok(dump) => {
            let tasks: Vec<_> = dump.tasks().iter().collect();
            eprintln!("[watchdog]   Captured {} task(s):\n", tasks.len());
            for (i, task) in tasks.iter().enumerate() {
                let trace = format!("{}", task.trace());
                if trace.trim().is_empty() {
                    eprintln!("[watchdog]   ─── Task {}: (no trace available) ───", i);
                } else {
                    eprintln!("[watchdog]   ─── Task {} ───", i);
                    for line in trace.lines() {
                        eprintln!("[watchdog]     {line}");
                    }
                }
                eprintln!();
            }
        }
        Err(_) => {
            eprintln!("[watchdog]   ✗ Task dump TIMED OUT — runtime is too stuck to respond");
            eprintln!("[watchdog]     This means workers cannot even pause to report their state.");
            eprintln!(
                "[watchdog]     Likely cause: blocking syscall on a worker thread (not spawn_blocking)."
            );
        }
    }
}
