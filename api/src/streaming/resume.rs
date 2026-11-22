use std::time::{Duration, Instant};

const FIRST_DELAY: Duration = Duration::from_millis(250);
const MAX_DELAY: Duration = Duration::from_secs(2);
const OUTAGE_MAX_DELAY: Duration = Duration::from_secs(5);
/// The usenet engine restarts inside kobbler's 10 s upstream read timeout, so a host that
/// keeps answering without progress for that long is not waiting on a restart.
const BUDGET: Duration = Duration::from_secs(10);
const OUTAGE_BUDGET: Duration = Duration::from_secs(5 * 60);

/// The engine's stream errors that are a verdict on the release, not on the moment
/// (`timeout`, `providers_unavailable`).
const DEAD_CODES: &[&str] = &["missing_articles", "corrupt_articles"];

/// The engine no longer knows the release its URL names: kobload imports it again on the
/// next resolve.
pub const FORGOTTEN_CODE: &str = "unknown_release";

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

/// The engine has no provider free, or none answering in time. A debrid CDN's plain
/// 503 is not this: it is often a dead link, and the player is waiting on the verdict.
const BUSY_CODES: &[&str] = &["providers_unavailable", "timeout"];

pub fn busy_code(code: &str) -> bool {
    BUSY_CODES.contains(&code)
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
    /// The host has nothing free to serve with right now.
    Unavailable,
    /// The host answered with another status worth retrying.
    Busy,
    /// The host answered, then ended the response short or at the wrong place.
    Aborted,
}

impl Interruption {
    fn is_outage(self) -> bool {
        matches!(self, Self::Unreachable | Self::Unavailable)
    }

    fn budget(self) -> Duration {
        if self.is_outage() {
            OUTAGE_BUDGET
        } else {
            BUDGET
        }
    }

    fn max_delay(self) -> Duration {
        if self.is_outage() {
            OUTAGE_MAX_DELAY
        } else {
            MAX_DELAY
        }
    }
}

/// Retries for a download that keeps being interrupted at the same place: quick at first,
/// for as long as it goes on making no progress within the budget of the interruption.
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
        self.interrupted_at(how, Instant::now())
    }

    fn interrupted_at(&mut self, how: Interruption, now: Instant) -> Option<Duration> {
        // An outage that ends in a reset starts the short budget afresh, and the reverse.
        if self.last.is_some_and(|l| l.is_outage() != how.is_outage()) {
            self.stalled_since = None;
        }
        let since = *self.stalled_since.get_or_insert(now);
        self.attempts += 1;
        self.last = Some(how);
        let stalled = now.saturating_duration_since(since);
        if stalled >= how.budget() {
            return None;
        }
        let delay = FIRST_DELAY
            .saturating_mul(1 << (self.attempts - 1).min(8))
            .min(how.max_delay());
        Some(delay.min(how.budget() - stalled))
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

    /// Retries for `lasting`, every interruption `how`, and whether it gave up.
    fn gives_up(how: Interruption, lasting: Duration) -> bool {
        let start = Instant::now();
        let mut r = Resume::new();
        let mut at = start;
        while at < start + lasting {
            let Some(delay) = r.interrupted_at(how, at) else {
                return true;
            };
            assert!(delay <= how.max_delay(), "{how:?} waited {delay:?}");
            at += delay.max(Duration::from_millis(1));
        }
        false
    }

    #[test]
    fn an_outage_is_waited_out_for_minutes_and_resets_for_seconds() {
        for how in [Interruption::Unreachable, Interruption::Unavailable] {
            assert!(!gives_up(how, Duration::from_secs(60)), "{how:?}");
            assert!(gives_up(how, Duration::from_secs(6 * 60)), "{how:?}");
        }
        for how in [Interruption::Aborted, Interruption::Busy] {
            assert!(!gives_up(how, Duration::from_secs(9)), "{how:?}");
            assert!(gives_up(how, Duration::from_secs(11)), "{how:?}");
        }
    }

    #[test]
    fn a_reset_after_an_outage_gets_the_short_budget_afresh() {
        let start = Instant::now();
        let mut r = Resume::new();
        r.interrupted_at(Interruption::Unreachable, start);
        let after = start + Duration::from_secs(60);
        assert!(r.interrupted_at(Interruption::Unreachable, after).is_some());
        assert!(r.interrupted_at(Interruption::Aborted, after).is_some());
        assert!(
            r.interrupted_at(Interruption::Aborted, after + Duration::from_secs(11))
                .is_none()
        );
    }

    #[test]
    fn only_verdicts_on_the_release_are_dead() {
        assert!(dead_code("missing_articles"));
        assert!(dead_code("corrupt_articles"));
        assert!(!dead_code("providers_unavailable"));
        assert!(!dead_code("timeout"));
        assert!(!dead_code("forbidden"));
        assert!(!dead_code(FORGOTTEN_CODE));
        assert!(busy_code("providers_unavailable"));
        assert!(busy_code("timeout"));
        assert!(!busy_code("missing_articles"));
    }
}
