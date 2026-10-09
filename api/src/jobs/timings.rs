//! Lightweight per-phase wall-clock recorder for jobs.
//!
//! Use `record(name, dur)` for already-measured spans, or wrap a future /
//! closure with `time_async` / `time_sync` to time it inline. Order is
//! preserved (it's a `Vec`), so the JSON / log output reads top-to-bottom
//! the way the work actually happened.
//!
//! Names are `&'static str` because every site is expected to be a literal —
//! lets us avoid allocating per-tick.

use serde_json::{Value, json};
use std::future::Future;
use std::time::{Duration, Instant};

#[derive(Debug, Default)]
pub struct PhaseTimings {
    spans: Vec<(&'static str, Duration)>,
}

impl PhaseTimings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, name: &'static str, duration: Duration) {
        self.spans.push((name, duration));
    }

    /// Time a synchronous block, record under `name`, return its result.
    pub fn time_sync<T, F: FnOnce() -> T>(&mut self, name: &'static str, f: F) -> T {
        let started = Instant::now();
        let out = f();
        self.record(name, started.elapsed());
        out
    }

    /// Time an awaited future, record under `name`, return its result.
    pub async fn time_async<T, F: Future<Output = T>>(&mut self, name: &'static str, f: F) -> T {
        let started = Instant::now();
        let out = f.await;
        self.record(name, started.elapsed());
        out
    }

    pub fn total(&self) -> Duration {
        self.spans.iter().map(|(_, d)| *d).sum()
    }

    pub fn total_ms(&self) -> u64 {
        self.total().as_millis().min(u64::MAX as u128) as u64
    }

    /// `{"phase_a_ms": 123, "phase_b_ms": 45, ...}`. Same order as recorded.
    pub fn to_json(&self) -> Value {
        let mut map = serde_json::Map::with_capacity(self.spans.len());
        for (name, dur) in &self.spans {
            let ms = dur.as_millis().min(u64::MAX as u128) as u64;
            map.insert(format!("{name}_ms"), json!(ms));
        }
        Value::Object(map)
    }
}
