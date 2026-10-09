use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use jsonwebtoken::{Algorithm, EncodingKey, Header, get_current_timestamp};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

pub const CHAT_BOT_SCOPE: &str = "https://www.googleapis.com/auth/chat.bot";

const DEFAULT_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
const JWT_BEARER_GRANT: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";
const ASSERTION_LIFETIME_SECS: u64 = 3600;
const REFRESH_MARGIN_SECS: u64 = 60;
const FALLBACK_EXPIRES_IN_SECS: u64 = 3600;

#[derive(Debug, thiserror::Error)]
pub enum GoogleAuthError {
    #[error("service account key is not valid JSON: {0}")]
    InvalidKeyJson(String),

    #[error("service account key is missing `{0}`")]
    MissingField(&'static str),

    #[error("service account private key could not be read: {0}")]
    InvalidPrivateKey(String),

    #[error("failed to sign the JWT assertion: {0}")]
    Sign(String),

    #[error("could not reach Google: {0}")]
    Http(String),

    #[error("Google token endpoint returned {status}: {body}")]
    TokenEndpoint { status: u16, body: String },

    #[error("Google token endpoint response was not understood: {0}")]
    MalformedResponse(String),

    #[error("could not fetch Google signing keys: {0}")]
    Certs(String),

    #[error("token rejected: {0}")]
    InvalidToken(String),
}

#[derive(Clone)]
pub struct ServiceAccountKey {
    client_email: String,
    private_key: String,
    private_key_id: String,
    token_uri: String,
    project_id: Option<String>,
    signing_key: EncodingKey,
}

impl std::fmt::Debug for ServiceAccountKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceAccountKey")
            .field("client_email", &self.client_email)
            .field("private_key", &"[redacted]")
            .field("private_key_id", &self.private_key_id)
            .field("token_uri", &self.token_uri)
            .field("project_id", &self.project_id)
            .finish()
    }
}

#[derive(Deserialize)]
struct RawServiceAccountKey {
    #[serde(rename = "type")]
    key_type: Option<String>,
    client_email: Option<String>,
    private_key: Option<String>,
    private_key_id: Option<String>,
    token_uri: Option<String>,
    project_id: Option<String>,
}

impl ServiceAccountKey {
    pub fn from_json(raw: &str) -> Result<Self, GoogleAuthError> {
        let parsed: RawServiceAccountKey = serde_json::from_str(raw)
            .map_err(|e| GoogleAuthError::InvalidKeyJson(e.to_string()))?;

        if let Some(key_type) = parsed.key_type.as_deref()
            && key_type != "service_account"
        {
            return Err(GoogleAuthError::InvalidKeyJson(format!(
                "expected a service_account key, got `{key_type}`"
            )));
        }

        let client_email =
            non_empty(parsed.client_email).ok_or(GoogleAuthError::MissingField("client_email"))?;
        let private_key = normalise_pem(
            &non_empty(parsed.private_key).ok_or(GoogleAuthError::MissingField("private_key"))?,
        );
        let signing_key = EncodingKey::from_rsa_pem(private_key.as_bytes())
            .map_err(|e| GoogleAuthError::InvalidPrivateKey(e.to_string()))?;

        Ok(Self {
            client_email,
            private_key,
            private_key_id: non_empty(parsed.private_key_id).unwrap_or_default(),
            token_uri: non_empty(parsed.token_uri).unwrap_or_else(|| DEFAULT_TOKEN_URI.to_string()),
            project_id: non_empty(parsed.project_id),
            signing_key,
        })
    }

    pub fn client_email(&self) -> &str {
        &self.client_email
    }

    pub fn project_id(&self) -> Option<&str> {
        self.project_id.as_deref()
    }

    pub fn private_key_id(&self) -> &str {
        &self.private_key_id
    }

    pub fn token_uri(&self) -> &str {
        &self.token_uri
    }

    pub fn private_key_pem(&self) -> &str {
        &self.private_key
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

/// The settings UI renders secret fields as a single-line `<input type="password">`, and the
/// HTML value sanitiser strips newlines out of anything pasted into one. That leaves the PEM
/// inside the key file as one long line, which `from_rsa_pem` rejects with an opaque error.
fn normalise_pem(raw: &str) -> String {
    if raw.contains('\n') {
        return raw.to_string();
    }
    rewrap_pem(raw.trim()).unwrap_or_else(|| raw.to_string())
}

fn rewrap_pem(trimmed: &str) -> Option<String> {
    let header_end = trimmed.match_indices("-----").nth(1).map(|(i, _)| i + 5)?;
    let footer_start = trimmed.rfind("-----END")?;
    if footer_start <= header_end {
        return None;
    }
    let body: String = trimmed[header_end..footer_start]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if body.is_empty() {
        return None;
    }
    let wrapped = body
        .as_bytes()
        .chunks(64)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "{}\n{}\n{}\n",
        &trimmed[..header_end],
        wrapped,
        &trimmed[footer_start..]
    ))
}

#[derive(Serialize)]
struct Assertion<'a> {
    iss: &'a str,
    sub: &'a str,
    aud: &'a str,
    scope: &'a str,
    iat: u64,
    exp: u64,
}

fn build_assertion(
    key: &ServiceAccountKey,
    scope: &str,
    now: u64,
) -> Result<String, GoogleAuthError> {
    let mut header = Header::new(Algorithm::RS256);
    if !key.private_key_id.is_empty() {
        header.kid = Some(key.private_key_id.clone());
    }

    let claims = Assertion {
        iss: &key.client_email,
        sub: &key.client_email,
        aud: &key.token_uri,
        scope,
        iat: now,
        exp: now + ASSERTION_LIFETIME_SECS,
    };

    jsonwebtoken::encode(&header, &claims, &key.signing_key)
        .map_err(|e| GoogleAuthError::Sign(e.to_string()))
}

struct AccessToken {
    token: String,
    expires_in: u64,
}

struct CacheEntry {
    token: String,
    expires_at: Instant,
}

type ExchangeFuture = Pin<Box<dyn Future<Output = Result<AccessToken, GoogleAuthError>> + Send>>;
type ExchangeFn = Box<dyn Fn(ServiceAccountKey, String) -> ExchangeFuture + Send + Sync>;

/// Process-wide access-token cache keyed by `(client_email, scopes)`.
pub struct TokenCache {
    slots: DashMap<(String, String), Arc<Mutex<Option<CacheEntry>>>>,
    exchange: ExchangeFn,
}

impl TokenCache {
    pub fn global() -> &'static TokenCache {
        static GLOBAL: LazyLock<TokenCache> = LazyLock::new(|| TokenCache {
            slots: DashMap::new(),
            exchange: Box::new(|key, scope| Box::pin(exchange_assertion(key, scope))),
        });
        &GLOBAL
    }

    pub async fn access_token(
        &self,
        key: &ServiceAccountKey,
        scopes: &[&str],
    ) -> Result<String, GoogleAuthError> {
        let scope = scopes.join(" ");
        let slot = {
            let entry = self
                .slots
                .entry((key.client_email.clone(), scope.clone()))
                .or_insert_with(|| Arc::new(Mutex::new(None)));
            Arc::clone(&entry)
        };

        // Holding the per-key lock across the exchange is the single-flight: concurrent
        // callers for the same (account, scope) queue here and take the cached token.
        let mut guard = slot.lock().await;
        if let Some(entry) = guard.as_ref()
            && entry.expires_at > Instant::now()
        {
            return Ok(entry.token.clone());
        }

        let fresh = (self.exchange)(key.clone(), scope).await?;
        let ttl = fresh.expires_in.saturating_sub(REFRESH_MARGIN_SECS);
        *guard = Some(CacheEntry {
            token: fresh.token.clone(),
            expires_at: Instant::now() + Duration::from_secs(ttl),
        });
        Ok(fresh.token)
    }

    #[cfg(test)]
    fn with_exchange(exchange: ExchangeFn) -> Self {
        Self {
            slots: DashMap::new(),
            exchange,
        }
    }
}

/// reqwest is built without the `form` feature in this crate, so the body is hand-rolled.
fn exchange_body(assertion: &str) -> String {
    format!(
        "grant_type={}&assertion={}",
        urlencoding::encode(JWT_BEARER_GRANT),
        urlencoding::encode(assertion)
    )
}

async fn exchange_assertion(
    key: ServiceAccountKey,
    scope: String,
) -> Result<AccessToken, GoogleAuthError> {
    let assertion = build_assertion(&key, &scope, get_current_timestamp())?;
    let body = exchange_body(&assertion);

    let response = crate::http::new_client()
        .post(&key.token_uri)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| GoogleAuthError::Http(e.to_string()))?;

    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| GoogleAuthError::Http(e.to_string()))?;

    if !status.is_success() {
        return Err(GoogleAuthError::TokenEndpoint {
            status: status.as_u16(),
            body: truncate(&text, 500),
        });
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        expires_in: Option<u64>,
    }

    let parsed: TokenResponse = serde_json::from_str(&text)
        .map_err(|e| GoogleAuthError::MalformedResponse(e.to_string()))?;

    Ok(AccessToken {
        token: parsed.access_token,
        expires_in: parsed.expires_in.unwrap_or(FALLBACK_EXPIRES_IN_SECS),
    })
}

pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::google::test_keys;

    fn valid_key() -> ServiceAccountKey {
        ServiceAccountKey::from_json(&test_keys::service_account_json(
            "bot@kobbler-test.iam.gserviceaccount.com",
            "https://oauth2.googleapis.com/token",
        ))
        .expect("test key parses")
    }

    #[test]
    fn from_json_reads_the_documented_fields() {
        let key = valid_key();
        assert_eq!(
            key.client_email(),
            "bot@kobbler-test.iam.gserviceaccount.com"
        );
        assert_eq!(key.project_id(), Some("kobbler-test"));
        assert_eq!(key.private_key_id(), "test-key-id");
        assert_eq!(key.token_uri(), "https://oauth2.googleapis.com/token");
    }

    #[test]
    fn from_json_defaults_the_token_uri() {
        let raw = serde_json::json!({
            "type": "service_account",
            "client_email": "bot@example.iam.gserviceaccount.com",
            "private_key": test_keys::PRIVATE_KEY_PEM,
        })
        .to_string();
        let key = ServiceAccountKey::from_json(&raw).expect("parses");
        assert_eq!(key.token_uri(), DEFAULT_TOKEN_URI);
        assert_eq!(key.project_id(), None);
    }

    #[test]
    fn from_json_rejects_malformed_input() {
        let err = ServiceAccountKey::from_json("not json at all").unwrap_err();
        assert!(matches!(err, GoogleAuthError::InvalidKeyJson(_)));
        assert!(err.to_string().contains("not valid JSON"));
    }

    #[test]
    fn from_json_rejects_a_missing_client_email() {
        let raw = serde_json::json!({
            "type": "service_account",
            "private_key": test_keys::PRIVATE_KEY_PEM,
        })
        .to_string();
        let err = ServiceAccountKey::from_json(&raw).unwrap_err();
        assert!(matches!(err, GoogleAuthError::MissingField("client_email")));
    }

    #[test]
    fn from_json_rejects_a_missing_private_key() {
        let raw = serde_json::json!({
            "type": "service_account",
            "client_email": "bot@example.iam.gserviceaccount.com",
        })
        .to_string();
        let err = ServiceAccountKey::from_json(&raw).unwrap_err();
        assert!(matches!(err, GoogleAuthError::MissingField("private_key")));
    }

    #[test]
    fn from_json_rejects_a_key_that_is_not_a_service_account() {
        let raw = serde_json::json!({
            "type": "authorized_user",
            "client_email": "bot@example.iam.gserviceaccount.com",
            "private_key": test_keys::PRIVATE_KEY_PEM,
        })
        .to_string();
        let err = ServiceAccountKey::from_json(&raw).unwrap_err();
        assert!(err.to_string().contains("authorized_user"));
    }

    #[test]
    fn from_json_rejects_a_private_key_that_is_not_a_key() {
        let raw = serde_json::json!({
            "type": "service_account",
            "client_email": "bot@example.iam.gserviceaccount.com",
            "private_key": "-----BEGIN PRIVATE KEY-----\nbm90IGEga2V5\n-----END PRIVATE KEY-----\n",
        })
        .to_string();
        let err = ServiceAccountKey::from_json(&raw).unwrap_err();
        assert!(matches!(err, GoogleAuthError::InvalidPrivateKey(_)));
    }

    #[test]
    fn from_json_recovers_a_pem_whose_newlines_were_stripped() {
        let flattened = test_keys::PRIVATE_KEY_PEM.replace('\n', "");
        let raw = serde_json::json!({
            "type": "service_account",
            "client_email": "bot@example.iam.gserviceaccount.com",
            "private_key": flattened,
        })
        .to_string();
        let key = ServiceAccountKey::from_json(&raw).expect("flattened PEM is recovered");
        assert!(
            key.private_key_pem()
                .starts_with("-----BEGIN PRIVATE KEY-----\n")
        );
        assert!(
            key.private_key_pem()
                .ends_with("-----END PRIVATE KEY-----\n")
        );
    }

    #[test]
    fn debug_does_not_leak_the_private_key() {
        let rendered = format!("{:?}", valid_key());
        assert!(rendered.contains("[redacted]"));
        assert!(!rendered.contains("MIIEvQIBADANBgkqhkiG9w0"));
    }

    #[test]
    fn assertion_carries_the_documented_claims() {
        let key = valid_key();
        let now = 1_700_000_000;
        let jwt = build_assertion(
            &key,
            "https://www.googleapis.com/auth/chat.bot https://www.googleapis.com/auth/other",
            now,
        )
        .expect("signs");

        let header = jsonwebtoken::decode_header(&jwt).expect("header decodes");
        assert_eq!(header.alg, Algorithm::RS256);
        assert_eq!(header.kid.as_deref(), Some("test-key-id"));

        let data = jsonwebtoken::dangerous::insecure_decode::<serde_json::Value>(&jwt)
            .expect("claims decode");
        let claims = data.claims;
        assert_eq!(claims["iss"], "bot@kobbler-test.iam.gserviceaccount.com");
        assert_eq!(claims["sub"], claims["iss"]);
        assert_eq!(claims["aud"], "https://oauth2.googleapis.com/token");
        assert_eq!(
            claims["scope"],
            "https://www.googleapis.com/auth/chat.bot https://www.googleapis.com/auth/other"
        );
        assert_eq!(claims["iat"], now);
        assert_eq!(claims["exp"], now + 3600);
    }

    #[test]
    fn assertion_omits_kid_when_the_key_file_has_none() {
        let raw = serde_json::json!({
            "type": "service_account",
            "client_email": "bot@example.iam.gserviceaccount.com",
            "private_key": test_keys::PRIVATE_KEY_PEM,
        })
        .to_string();
        let key = ServiceAccountKey::from_json(&raw).expect("parses");
        let jwt = build_assertion(&key, CHAT_BOT_SCOPE, 1_700_000_000).expect("signs");
        assert_eq!(jsonwebtoken::decode_header(&jwt).unwrap().kid, None);
    }

    fn counting_cache(expires_in: u64, calls: &'static AtomicUsize) -> TokenCache {
        TokenCache::with_exchange(Box::new(move |_key, _scope| {
            Box::pin(async move {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                Ok(AccessToken {
                    token: format!("token-{n}"),
                    expires_in,
                })
            })
        }))
    }

    #[tokio::test]
    async fn cache_reuses_a_live_token() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        let cache = counting_cache(3600, &CALLS);
        let key = valid_key();

        let first = cache.access_token(&key, &[CHAT_BOT_SCOPE]).await.unwrap();
        let second = cache.access_token(&key, &[CHAT_BOT_SCOPE]).await.unwrap();

        assert_eq!(first, "token-0");
        assert_eq!(second, "token-0");
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cache_refreshes_early_enough_to_beat_expiry() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        // 30s of life is inside the 60s refresh margin, so the entry is born already stale.
        let cache = counting_cache(30, &CALLS);
        let key = valid_key();

        let first = cache.access_token(&key, &[CHAT_BOT_SCOPE]).await.unwrap();
        let second = cache.access_token(&key, &[CHAT_BOT_SCOPE]).await.unwrap();

        assert_eq!(first, "token-0");
        assert_eq!(second, "token-1");
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn cache_keys_on_the_scope_set() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        let cache = counting_cache(3600, &CALLS);
        let key = valid_key();

        cache.access_token(&key, &[CHAT_BOT_SCOPE]).await.unwrap();
        cache
            .access_token(&key, &[CHAT_BOT_SCOPE, "https://example.test/other"])
            .await
            .unwrap();

        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cache_mints_one_token_for_concurrent_callers() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        let cache = Arc::new(TokenCache::with_exchange(Box::new(|_key, _scope| {
            Box::pin(async {
                let n = CALLS.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(AccessToken {
                    token: format!("token-{n}"),
                    expires_in: 3600,
                })
            })
        })));
        let key = valid_key();

        let mut handles = Vec::new();
        for _ in 0..8 {
            let cache = Arc::clone(&cache);
            let key = key.clone();
            handles.push(tokio::spawn(async move {
                cache.access_token(&key, &[CHAT_BOT_SCOPE]).await.unwrap()
            }));
        }

        for handle in handles {
            assert_eq!(handle.await.unwrap(), "token-0");
        }
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn exchange_body_is_form_encoded() {
        let body = exchange_body("head.body.sig");
        assert_eq!(
            body,
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion=head.body.sig"
        );
    }

    #[tokio::test]
    async fn exchange_posts_a_signed_assertion_and_returns_the_token() {
        use wiremock::matchers::{body_string_contains, header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(header("Content-Type", "application/x-www-form-urlencoded"))
            .and(body_string_contains(
                "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "ya29.minted",
                "token_type": "Bearer",
                "expires_in": 3599,
            })))
            .mount(&server)
            .await;

        let key = ServiceAccountKey::from_json(&test_keys::service_account_json(
            "bot@kobbler-test.iam.gserviceaccount.com",
            &format!("{}/token", server.uri()),
        ))
        .expect("parses");

        let token = TokenCache::global()
            .access_token(&key, &[CHAT_BOT_SCOPE])
            .await
            .expect("exchanges");
        assert_eq!(token, "ya29.minted");

        let requests = server.received_requests().await.expect("recorded");
        assert_eq!(requests.len(), 1);
        let body = String::from_utf8(requests[0].body.clone()).expect("utf8");
        let assertion = body
            .split("assertion=")
            .nth(1)
            .expect("assertion present")
            .to_string();
        let claims = jsonwebtoken::dangerous::insecure_decode::<serde_json::Value>(&assertion)
            .expect("assertion is a JWT")
            .claims;
        assert_eq!(claims["scope"], CHAT_BOT_SCOPE);
        assert_eq!(claims["aud"], format!("{}/token", server.uri()));
    }

    #[tokio::test]
    async fn exchange_surfaces_a_rejection_from_google() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string(
                r#"{"error":"invalid_grant","error_description":"Invalid JWT Signature."}"#,
            ))
            .mount(&server)
            .await;

        let key = ServiceAccountKey::from_json(&test_keys::service_account_json(
            "rejected@kobbler-test.iam.gserviceaccount.com",
            &format!("{}/token", server.uri()),
        ))
        .expect("parses");

        let err = TokenCache::global()
            .access_token(&key, &[CHAT_BOT_SCOPE])
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            GoogleAuthError::TokenEndpoint { status: 400, .. }
        ));
        assert!(err.to_string().contains("invalid_grant"));
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("abc", 10), "abc");
        assert_eq!(truncate("aéb", 2), "a…");
    }
}
