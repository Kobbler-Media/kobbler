//! Shared HTTP client configuration and per-provider rate limiting.
//!
//! All outbound HTTP clients should use `new_client()` to get a clone of the
//! shared application-wide client. This ensures consistent headers and
//! connection pooling across the application.
//!
//! `reqwest::Client` uses `Arc` internally, so cloning is cheap — all clones
//! share the same connection pool.

use dashmap::DashMap;
use governor::{Quota, RateLimiter};
use reqwest::{Client, ClientBuilder};
use std::num::NonZeroU32;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use tracing::debug;

/// Provider ID constants. Use these instead of inline string literals to ensure
/// rate limit gate names always match registered names.
pub mod provider_ids {
    // Book metadata
    pub const HARDCOVER: &str = "hardcover";
    pub const OPENLIBRARY: &str = "openlibrary";
    pub const GOOGLEBOOKS: &str = "googlebooks";
    pub const AUDNEXUS: &str = "audnexus";
    // Video metadata
    pub const TMDB: &str = "tmdb";
    pub const TVDB: &str = "tvdb";
    pub const TVMAZE: &str = "tvmaze";
    pub const OMDB: &str = "omdb";
    pub const THESPORTSDB: &str = "thesportsdb";
    // Video sync
    pub const TRAKT: &str = "trakt";
}

/// Governor rate limiter type alias for readability.
type DirectLimiter = RateLimiter<
    governor::state::NotKeyed,
    governor::state::InMemoryState,
    governor::clock::DefaultClock,
    governor::middleware::NoOpMiddleware,
>;

/// Shared application-wide provider cooldown state.
///
/// All provider registries share this to coordinate rate limit backoff.
static PROVIDER_COOLDOWN: LazyLock<ProviderCooldown> = LazyLock::new(ProviderCooldown::new);

/// Get the shared provider cooldown instance.
pub fn provider_cooldown() -> &'static ProviderCooldown {
    &PROVIDER_COOLDOWN
}

/// A client per tokio runtime. A pooled connection is driven by a task on the
/// runtime that opened it, so another runtime handed that connection waits on
/// one it does not drive: the server has one runtime, but every test has its own.
pub struct RuntimeClient {
    build: fn() -> Client,
    clients: LazyLock<DashMap<Option<tokio::runtime::Id>, Client>>,
}

impl RuntimeClient {
    pub const fn new(build: fn() -> Client) -> Self {
        Self {
            build,
            clients: LazyLock::new(DashMap::new),
        }
    }

    pub fn client(&self) -> Client {
        let runtime = tokio::runtime::Handle::try_current().ok().map(|h| h.id());
        self.clients
            .entry(runtime)
            .or_insert_with(self.build)
            .clone()
    }
}

/// Shared application-wide HTTP client.
///
/// Single connection pool reused by all callers of `new_client()`.
static SHARED_CLIENT: RuntimeClient = RuntimeClient::new(|| {
    builder()
        .user_agent("Kobbler/1.0")
        .build()
        .expect("Failed to create HTTP client")
});

/// How long a streamed response may go without producing bytes before it is
/// treated as dead. Generous enough to cover a GPU service queueing behind
/// another job before its first chunk.
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Shared client for responses that are CONSUMED AS A STREAM.
///
/// `builder()`'s 30 s `timeout` is a deadline on the WHOLE request, body
/// included — not on idleness. Any streamed response still being read at 30 s
/// is killed mid-body, and reqwest reports that as `error decoding response
/// body`, which reads like a protocol fault rather than the timeout it is. A
/// read-along page is minutes of audio arriving over minutes of wall clock, so
/// it hit that ceiling routinely.
///
/// Here the total deadline is off and `read_timeout` bounds IDLE time between
/// chunks instead: a stalled upstream still fails fast, a slow-but-progressing
/// one runs as long as it needs. Same pattern as the debrid/kobload streams.
static STREAMING_CLIENT: RuntimeClient = RuntimeClient::new(|| {
    builder()
        .user_agent("Kobbler/1.0")
        .timeout(Duration::MAX)
        .read_timeout(STREAM_IDLE_TIMEOUT)
        .build()
        .expect("Failed to create streaming HTTP client")
});

/// Get a clone of the shared client for streamed responses.
///
/// Use this whenever the response body is read incrementally
/// (`bytes_stream()`) and may outlive `new_client()`'s 30 s total deadline.
/// For request/response calls that buffer the whole body, use `new_client()`.
pub fn new_streaming_client() -> Client {
    STREAMING_CLIENT.client()
}

/// Default headers applied to all outbound HTTP clients.
fn default_headers() -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::ACCEPT_LANGUAGE,
        "en-US,en;q=0.9".parse().unwrap(),
    );
    headers
}

/// Get a clone of the shared HTTP client.
///
/// All clones share the same connection pool, so this is both cheap and
/// efficient for connection reuse. Sets `Accept-Language: en-US,en;q=0.9`
/// and a 30s timeout.
pub fn new_client() -> Client {
    SHARED_CLIENT.client()
}

/// Get a pre-configured `ClientBuilder` with default headers and timeout.
///
/// Use this only when you need a separate client with different settings
/// (e.g., longer timeout for file downloads). The resulting client will have
/// its own connection pool.
pub fn builder() -> ClientBuilder {
    Client::builder()
        .default_headers(default_headers())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .tcp_keepalive(Duration::from_secs(60))
        .pool_max_idle_per_host(10)
}

/// Per-provider reactive cooldown state (429 exponential backoff).
struct CooldownState {
    /// When the cooldown expires.
    until: Instant,
    /// Number of consecutive 429s (drives exponential backoff).
    consecutive_429s: u32,
}

/// Maximum backoff duration (5 minutes).
const MAX_BACKOFF_SECS: u64 = 300;

/// Shared per-provider rate limit state.
///
/// Two layers of protection:
/// 1. **Proactive rate limiting** (governor): GCRA-based token bucket enforces
///    requests-per-minute per provider. Fully async, lock-free atomics
///    internally. Multiple requests can be in-flight concurrently.
/// 2. **Reactive 429 backoff** (DashMap + sleep): consecutive 429s increase
///    delay (10s, 20s, 40s, 80s, capped at 5 minutes). Successful requests
///    reset. Checked before governor so we don't burn a token during a
///    known cooldown.
#[derive(Clone)]
pub struct ProviderCooldown {
    limiters: Arc<DashMap<String, Arc<DirectLimiter>>>,
    cooldowns: Arc<DashMap<String, CooldownState>>,
}

impl Default for ProviderCooldown {
    fn default() -> Self {
        Self {
            limiters: Arc::new(DashMap::new()),
            cooldowns: Arc::new(DashMap::new()),
        }
    }
}

impl ProviderCooldown {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a provider's rate limit. Call during registry construction.
    /// `rpm` = requests per minute. Safe to call multiple times; replaces the
    /// limiter if RPM changed.
    pub fn register_rate_limit(&self, provider: &str, rpm: u32) {
        if rpm == 0 {
            return;
        }
        let quota = Quota::per_minute(NonZeroU32::new(rpm).expect("rpm must be > 0"));
        debug!(provider = %provider, rpm = rpm, "Registered provider rate limit");
        self.limiters
            .insert(provider.to_string(), Arc::new(RateLimiter::direct(quota)));
    }

    /// Record a 429 for this provider. Applies exponential backoff:
    /// base delay × 2^(n-1), capped at 5 minutes.
    pub(crate) fn set_cooldown(&self, provider: &str, retry_after_secs: u64) {
        let consecutive = self
            .cooldowns
            .get(provider)
            .map(|s| s.consecutive_429s + 1)
            .unwrap_or(1);

        let backoff =
            (retry_after_secs.saturating_mul(1 << (consecutive - 1).min(5))).min(MAX_BACKOFF_SECS);

        let until = Instant::now() + Duration::from_secs(backoff);
        debug!(
            provider = %provider,
            backoff_secs = backoff,
            consecutive_429s = consecutive,
            "Provider rate limited, backing off"
        );
        self.cooldowns.insert(
            provider.to_string(),
            CooldownState {
                until,
                consecutive_429s: consecutive,
            },
        );
    }

    pub fn is_cooling_down(&self, provider: &str) -> bool {
        self.cooldowns
            .get(provider)
            .is_some_and(|state| state.until > Instant::now())
    }

    /// Reset the consecutive 429 counter after a successful request.
    pub(crate) fn reset(&self, provider: &str) {
        self.cooldowns.remove(provider);
    }

    /// Wait for rate limit and cooldown before dispatching a request.
    pub(crate) async fn wait_if_needed(&self, provider: &str) {
        // 1. Reactive 429 backoff — sleep before hitting governor so we
        //    don't burn a token during a known cooldown period.
        if let Some(state) = self.cooldowns.get(provider) {
            let now = Instant::now();
            if state.until > now {
                let wait = state.until - now;
                debug!(provider = %provider, wait_secs = wait.as_secs(), "Waiting for 429 cooldown");
                drop(state);
                tokio::time::sleep(wait).await;
            }
        }

        // 2. Proactive rate limit (governor GCRA token bucket).
        let limiter = self.limiters.get(provider).map(|r| Arc::clone(&r));
        if let Some(limiter) = limiter {
            limiter.until_ready().await;
        }
    }
}

/// HTTP client wrapper that automatically manages per-provider rate limiting.
///
/// Handles `wait_if_needed` before sending, `set_cooldown` on 429, and `reset`
/// on success. Callers still receive the raw `Response` for their own error
/// handling and body consumption.
#[derive(Clone)]
pub struct ProviderClient {
    client: Client,
    provider: &'static str,
}

impl ProviderClient {
    pub fn new(client: Client, provider: &'static str) -> Self {
        Self { client, provider }
    }

    /// Build a GET request through the underlying client.
    pub fn get(&self, url: &str) -> reqwest::RequestBuilder {
        self.client.get(url)
    }

    /// Build a POST request through the underlying client.
    pub fn post(&self, url: &str) -> reqwest::RequestBuilder {
        self.client.post(url)
    }

    /// Execute a request with automatic rate limit management.
    ///
    /// 1. Waits for any active cooldown + proactive rate limit
    /// 2. Sends the request
    /// 3. On 429: records cooldown with Retry-After header (exponential backoff)
    /// 4. On success (2xx): resets the cooldown counter
    /// 5. Returns the raw Response — body is unconsumed and available to caller
    pub async fn execute(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let cooldown = provider_cooldown();
        cooldown.wait_if_needed(self.provider).await;

        let response = request.send().await?;

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            cooldown.set_cooldown(self.provider, parse_retry_after(response.headers()));
        } else if response.status().is_success() {
            cooldown.reset(self.provider);
        }

        Ok(response)
    }
}

/// Parse the `Retry-After` header from an HTTP response.
/// Returns the number of seconds to wait, defaulting to 10 if the header
/// is missing or unparseable.
pub fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> u64 {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(10)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // ── parse_retry_after ──────────────────────────────────────────────

    #[test]
    fn parse_retry_after_present() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "42".parse().unwrap());
        assert_eq!(parse_retry_after(&headers), 42);
    }

    #[test]
    fn parse_retry_after_missing_defaults_to_10() {
        let headers = reqwest::header::HeaderMap::new();
        assert_eq!(parse_retry_after(&headers), 10);
    }

    #[test]
    fn parse_retry_after_non_numeric_defaults_to_10() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "not-a-number".parse().unwrap());
        assert_eq!(parse_retry_after(&headers), 10);
    }

    // ── ProviderCooldown ───────────────────────────────────────────────

    #[tokio::test]
    async fn cooldown_wait_passes_without_registration() {
        let cd = ProviderCooldown::new();
        // Unregistered provider — should return immediately, not panic
        cd.wait_if_needed("unregistered").await;
    }

    #[tokio::test]
    async fn cooldown_reset_clears_state() {
        let cd = ProviderCooldown::new();
        cd.set_cooldown("test_reset", 60);
        assert!(cd.cooldowns.contains_key("test_reset"));
        cd.reset("test_reset");
        assert!(!cd.cooldowns.contains_key("test_reset"));
    }

    #[tokio::test]
    async fn cooldown_exponential_backoff() {
        let cd = ProviderCooldown::new();
        // First 429: base × 2^0 = 10s
        cd.set_cooldown("test_exp", 10);
        let first = cd.cooldowns.get("test_exp").unwrap();
        assert_eq!(first.consecutive_429s, 1);
        let first_remaining = first.until.duration_since(Instant::now());
        drop(first);

        // Second 429: base × 2^1 = 20s
        cd.set_cooldown("test_exp", 10);
        let second = cd.cooldowns.get("test_exp").unwrap();
        assert_eq!(second.consecutive_429s, 2);
        let second_remaining = second.until.duration_since(Instant::now());
        drop(second);

        assert!(second_remaining > first_remaining);
    }

    #[tokio::test]
    async fn cooldown_capped_at_max() {
        let cd = ProviderCooldown::new();
        // Simulate many consecutive 429s
        for _ in 0..20 {
            cd.set_cooldown("test_cap", 10);
        }
        let state = cd.cooldowns.get("test_cap").unwrap();
        let remaining = state.until.duration_since(Instant::now());
        assert!(remaining <= Duration::from_secs(MAX_BACKOFF_SECS + 1));
    }

    // ── ProviderClient::execute ────────────────────────────────────────

    #[tokio::test]
    async fn execute_200_resets_cooldown() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&mock)
            .await;

        let provider = "test_exec_200";
        let cooldown = provider_cooldown();
        cooldown.register_rate_limit(provider, 600);
        // Simulate a prior 429
        cooldown.set_cooldown(provider, 0);
        assert!(cooldown.cooldowns.contains_key(provider));

        let client = ProviderClient::new(new_client(), provider);
        let resp = client.execute(client.get(&mock.uri())).await.unwrap();
        assert_eq!(resp.status(), 200);
        // Cooldown should be cleared
        assert!(!cooldown.cooldowns.contains_key(provider));
    }

    #[tokio::test]
    async fn execute_429_sets_cooldown() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "5"))
            .mount(&mock)
            .await;

        let provider = "test_exec_429";
        let cooldown = provider_cooldown();
        cooldown.register_rate_limit(provider, 600);

        let client = ProviderClient::new(new_client(), provider);
        let resp = client.execute(client.get(&mock.uri())).await.unwrap();
        assert_eq!(resp.status(), 429);
        // Cooldown should be set
        let state = cooldown
            .cooldowns
            .get(provider)
            .expect("cooldown should be set");
        assert_eq!(state.consecutive_429s, 1);
    }

    #[tokio::test]
    async fn execute_non_429_error_leaves_cooldown_unchanged() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock)
            .await;

        let provider = "test_exec_500";
        let cooldown = provider_cooldown();
        cooldown.register_rate_limit(provider, 600);

        let client = ProviderClient::new(new_client(), provider);
        let resp = client.execute(client.get(&mock.uri())).await.unwrap();
        assert_eq!(resp.status(), 500);
        // No cooldown should be set (500 is not a rate limit)
        assert!(!cooldown.cooldowns.contains_key(provider));
    }

    #[tokio::test]
    async fn execute_consecutive_429s_increase_backoff() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "1"))
            .mount(&mock)
            .await;

        let provider = "test_exec_429_exp";
        let cooldown = provider_cooldown();
        cooldown.register_rate_limit(provider, 600);

        let client = ProviderClient::new(new_client(), provider);

        // First 429
        client.execute(client.get(&mock.uri())).await.unwrap();
        let first_count = cooldown.cooldowns.get(provider).unwrap().consecutive_429s;
        // Clear the cooldown timer (not the count) so second request doesn't sleep
        cooldown.cooldowns.remove(provider);
        cooldown.set_cooldown(provider, 1); // re-set as if it's a second 429
        assert_eq!(first_count, 1);

        // Two execute calls back-to-back (with 0 retry-after to avoid sleeping)
        cooldown.cooldowns.remove(provider);
        let mock2 = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
            .mount(&mock2)
            .await;

        let client2 = ProviderClient::new(new_client(), provider);
        client2.execute(client2.get(&mock2.uri())).await.unwrap();
        client2.execute(client2.get(&mock2.uri())).await.unwrap();
        let count = cooldown.cooldowns.get(provider).unwrap().consecutive_429s;
        assert_eq!(count, 2);
    }
}
