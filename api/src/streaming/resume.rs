use std::time::{Duration, Instant};

const FIRST_DELAY: Duration = Duration::from_millis(250);
const MAX_DELAY: Duration = Duration::from_secs(2);
/// The usenet engine restarts inside kobbler's 10 s upstream read timeout, so a download
/// that has gone that long without a byte is not waiting on a restart.
const BUDGET: Duration = Duration::from_secs(10);

/// The engine's stream errors that are a verdict on the release, not on the moment
/// (`timeout`, `providers_unavailable`).
const DEAD_CODES: &[&str] = &["missing_articles", "corrupt_articles"];

pub fn dead_code(code: &str) -> bool {
    DEAD_CODES.contains(&code)
}

/// The `error` code of a JSON error body, as the usenet engine sends.
pub async fn error_code(resp: reqwest::Response) -> Option<String> {
    let body: serde_json::Value = resp.json().await.ok()?;
    body.get("error")?.as_str().map(str::to_string)
}

/// A signed URL refused because its token lapsed or was revoked.
pub fn is_lapsed(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 401 | 403 | 410)
}

/// Debrid CDN nodes return 429/5xx under load and recover within seconds; 4xx (other
/// than 429) means the link itself is dead and retrying just burns the session's startup
/// budget.
pub fn is_transient_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status.is_server_error()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interruption {
    /// The host could not be reached: says nothing about the file.
    Unreachable,
    /// The host answered with a status worth retrying.
    Busy,
    /// The host answered, then ended the response short.
    Aborted,
}

/// Retries for a download that keeps being interrupted at the same place: quick at first,
/// for as long as it goes on making no progress within the budget.
#[derive(Debug)]
pub struct Resume {
    attempts: u32,
    stalled_since: Option<Instant>,
    last: Option<Interruption>,
}

impl Default for Resume {
    fn default() -> Self {
        Self::new()
    }
}

impl Resume {
    pub fn new() -> Self {
        Self {
            attempts: 0,
            stalled_since: None,
            last: None,
        }
    }

    pub fn progressed(&mut self) {
        self.attempts = 0;
        self.stalled_since = None;
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    pub fn last(&self) -> Option<Interruption> {
        self.last
    }

    /// How long to wait before the next attempt, or `None` once the budget is spent.
    pub fn interrupted(&mut self, how: Interruption) -> Option<Duration> {
        let since = *self.stalled_since.get_or_insert_with(Instant::now);
        self.attempts += 1;
        self.last = Some(how);
        if since.elapsed() >= BUDGET {
            return None;
        }
        let delay = FIRST_DELAY
            .saturating_mul(1 << (self.attempts - 1).min(8))
            .min(MAX_DELAY);
        Some(delay.min(BUDGET.saturating_sub(since.elapsed())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backs_off_quickly_and_progress_starts_over() {
        let mut r = Resume::new();
        let delays: Vec<_> = (0..5)
            .map(|_| r.interrupted(Interruption::Aborted).unwrap())
            .collect();
        assert_eq!(delays[0], Duration::from_millis(250));
        assert_eq!(delays[1], Duration::from_millis(500));
        assert_eq!(delays[3], Duration::from_secs(2));
        assert_eq!(delays[4], Duration::from_secs(2));
        r.progressed();
        assert_eq!(r.attempts(), 0);
        assert_eq!(
            r.interrupted(Interruption::Unreachable),
            Some(Duration::from_millis(250))
        );
        assert_eq!(r.last(), Some(Interruption::Unreachable));
    }

    #[test]
    fn only_verdicts_on_the_release_are_dead() {
        assert!(dead_code("missing_articles"));
        assert!(dead_code("corrupt_articles"));
        assert!(!dead_code("providers_unavailable"));
        assert!(!dead_code("timeout"));
        assert!(!dead_code("forbidden"));
    }
}
