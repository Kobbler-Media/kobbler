use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use tokio::sync::Mutex;

use super::service_account::GoogleAuthError;

/// Google Chat signs its webhook bearer tokens with this service account.
pub const CHAT_ISSUER: &str = "chat@system.gserviceaccount.com";

const JWK_BASE: &str = "https://www.googleapis.com/service_accounts/v1/jwk/";
const DEFAULT_CERT_TTL_SECS: u64 = 3600;
const MIN_CERT_TTL_SECS: u64 = 60;
const MAX_CERT_TTL_SECS: u64 = 86_400;
const UNKNOWN_KID_REFETCH_AFTER: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Deserialize)]
pub struct GoogleClaims {
    pub iss: String,
    pub aud: String,
    #[serde(default)]
    pub sub: String,
    #[serde(default)]
    pub email: Option<String>,
    pub exp: i64,
}

struct CachedKeys {
    keys: JwkSet,
    fetched_at: Instant,
    expires_at: Instant,
}

pub struct IdTokenVerifier {
    client: reqwest::Client,
    jwk_base: String,
    slots: DashMap<String, Arc<Mutex<Option<CachedKeys>>>>,
}

static VERIFIERS: LazyLock<DashMap<String, Arc<IdTokenVerifier>>> = LazyLock::new(DashMap::new);

impl IdTokenVerifier {
    pub fn new(jwk_base: impl Into<String>) -> Self {
        Self {
            client: crate::http::new_client(),
            jwk_base: jwk_base.into(),
            slots: DashMap::new(),
        }
    }

    /// One verifier per JWK base, because the signing-key cache must outlive the caller:
    /// the Chat transport is rebuilt on every webhook and would otherwise refetch
    /// Google's keys on each request.
    pub fn shared(jwk_base: &str) -> Arc<IdTokenVerifier> {
        if let Some(existing) = VERIFIERS.get(jwk_base) {
            return Arc::clone(&existing);
        }
        Arc::clone(
            &VERIFIERS
                .entry(jwk_base.to_string())
                .or_insert_with(|| Arc::new(IdTokenVerifier::new(jwk_base))),
        )
    }

    pub fn global() -> Arc<IdTokenVerifier> {
        Self::shared(JWK_BASE)
    }

    pub async fn verify(
        &self,
        jwt: &str,
        audience: &str,
        expected_issuer: &str,
    ) -> Result<GoogleClaims, GoogleAuthError> {
        let header = decode_header(jwt)
            .map_err(|e| GoogleAuthError::InvalidToken(format!("unreadable JWT header: {e}")))?;
        let kid = header
            .kid
            .ok_or_else(|| GoogleAuthError::InvalidToken("JWT header has no `kid`".into()))?;

        let slot = {
            let entry = self
                .slots
                .entry(expected_issuer.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(None)));
            Arc::clone(&entry)
        };
        let mut guard = slot.lock().await;

        let mut refreshed = false;
        if guard
            .as_ref()
            .is_none_or(|c| c.expires_at <= Instant::now())
        {
            *guard = Some(self.fetch_keys(expected_issuer).await?);
            refreshed = true;
        }

        // Google rotates signing keys, so an unrecognised kid is a refetch rather than a
        // rejection — rate-limited so a junk kid cannot turn into a request loop.
        let stale_kid = {
            let cached = guard.as_ref().expect("populated above");
            !refreshed
                && cached.keys.find(&kid).is_none()
                && cached.fetched_at.elapsed() > UNKNOWN_KID_REFETCH_AFTER
        };
        if stale_kid {
            *guard = Some(self.fetch_keys(expected_issuer).await?);
        }

        let cached = guard.as_ref().expect("populated above");
        let jwk = cached.keys.find(&kid).ok_or_else(|| {
            GoogleAuthError::InvalidToken(format!("no Google signing key for kid `{kid}`"))
        })?;

        verify_with_jwk(jwt, jwk, audience, expected_issuer)
    }

    async fn fetch_keys(&self, issuer: &str) -> Result<CachedKeys, GoogleAuthError> {
        let url = format!("{}{}", self.jwk_base, urlencoding::encode(issuer));
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| GoogleAuthError::Certs(e.to_string()))?;

        let status = response.status();
        let ttl = response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok())
            .and_then(max_age_secs)
            .unwrap_or(DEFAULT_CERT_TTL_SECS)
            .clamp(MIN_CERT_TTL_SECS, MAX_CERT_TTL_SECS);

        let text = response
            .text()
            .await
            .map_err(|e| GoogleAuthError::Certs(e.to_string()))?;

        if !status.is_success() {
            return Err(GoogleAuthError::Certs(format!(
                "{url} returned {}: {}",
                status.as_u16(),
                super::service_account::truncate(&text, 300)
            )));
        }

        let keys: JwkSet = serde_json::from_str(&text)
            .map_err(|e| GoogleAuthError::Certs(format!("{url} returned unusable JWKs: {e}")))?;

        let now = Instant::now();
        Ok(CachedKeys {
            keys,
            fetched_at: now,
            expires_at: now + Duration::from_secs(ttl),
        })
    }
}

fn verify_with_jwk(
    jwt: &str,
    jwk: &Jwk,
    audience: &str,
    expected_issuer: &str,
) -> Result<GoogleClaims, GoogleAuthError> {
    let key = DecodingKey::from_jwk(jwk)
        .map_err(|e| GoogleAuthError::Certs(format!("unusable Google signing key: {e}")))?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[audience]);
    validation.set_issuer(&[expected_issuer]);
    validation.set_required_spec_claims(&["exp", "aud", "iss"]);

    decode::<GoogleClaims>(jwt, &key, &validation)
        .map(|data| data.claims)
        .map_err(|e| GoogleAuthError::InvalidToken(e.to_string()))
}

fn max_age_secs(header: &str) -> Option<u64> {
    let lowered = header.to_ascii_lowercase();
    lowered.split(',').find_map(|directive| {
        directive
            .trim()
            .strip_prefix("max-age=")
            .and_then(|v| v.trim().trim_matches('"').parse().ok())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::google::test_keys;
    use jsonwebtoken::{EncodingKey, Header, encode};

    const KID: &str = "test-kid";
    const AUDIENCE: &str = "123456789012";

    fn jwk() -> Jwk {
        serde_json::from_value(serde_json::json!({
            "kty": "RSA",
            "alg": "RS256",
            "use": "sig",
            "kid": KID,
            "n": test_keys::MODULUS_B64,
            "e": test_keys::EXPONENT_B64,
        }))
        .expect("test JWK is well formed")
    }

    fn sign(claims: serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(KID.to_string());
        let key = EncodingKey::from_rsa_pem(test_keys::PRIVATE_KEY_PEM.as_bytes())
            .expect("test key parses");
        encode(&header, &claims, &key).expect("signs")
    }

    fn now() -> i64 {
        jsonwebtoken::get_current_timestamp() as i64
    }

    fn chat_claims() -> serde_json::Value {
        serde_json::json!({
            "iss": CHAT_ISSUER,
            "aud": AUDIENCE,
            "sub": "users/12345",
            "email": "her@example.com",
            "iat": now() - 10,
            "exp": now() + 600,
        })
    }

    #[test]
    fn accepts_a_well_formed_chat_token() {
        let jwt = sign(chat_claims());
        let claims = verify_with_jwk(&jwt, &jwk(), AUDIENCE, CHAT_ISSUER).expect("verifies");
        assert_eq!(claims.iss, CHAT_ISSUER);
        assert_eq!(claims.aud, AUDIENCE);
        assert_eq!(claims.sub, "users/12345");
        assert_eq!(claims.email.as_deref(), Some("her@example.com"));
    }

    #[test]
    fn accepts_a_token_without_an_email() {
        let mut claims = chat_claims();
        claims.as_object_mut().unwrap().remove("email");
        let jwt = sign(claims);
        let verified = verify_with_jwk(&jwt, &jwk(), AUDIENCE, CHAT_ISSUER).expect("verifies");
        assert_eq!(verified.email, None);
    }

    #[test]
    fn rejects_a_wrong_audience() {
        let jwt = sign(chat_claims());
        let err = verify_with_jwk(&jwt, &jwk(), "999999999999", CHAT_ISSUER).unwrap_err();
        assert!(matches!(err, GoogleAuthError::InvalidToken(_)));
    }

    #[test]
    fn rejects_a_wrong_issuer() {
        let mut claims = chat_claims();
        claims["iss"] = serde_json::json!("attacker@system.gserviceaccount.com");
        let jwt = sign(claims);
        let err = verify_with_jwk(&jwt, &jwk(), AUDIENCE, CHAT_ISSUER).unwrap_err();
        assert!(matches!(err, GoogleAuthError::InvalidToken(_)));
    }

    #[test]
    fn rejects_an_expired_token() {
        let mut claims = chat_claims();
        // Validation allows 60s of clock skew by default, so expire well past that.
        claims["exp"] = serde_json::json!(now() - 3600);
        claims["iat"] = serde_json::json!(now() - 7200);
        let jwt = sign(claims);
        let err = verify_with_jwk(&jwt, &jwk(), AUDIENCE, CHAT_ISSUER).unwrap_err();
        assert!(matches!(err, GoogleAuthError::InvalidToken(_)));
    }

    #[test]
    fn rejects_a_token_with_no_expiry() {
        let mut claims = chat_claims();
        claims.as_object_mut().unwrap().remove("exp");
        let jwt = sign(claims);
        let err = verify_with_jwk(&jwt, &jwk(), AUDIENCE, CHAT_ISSUER).unwrap_err();
        assert!(matches!(err, GoogleAuthError::InvalidToken(_)));
    }

    #[test]
    fn rejects_a_tampered_signature() {
        let jwt = sign(chat_claims());
        let (body, signature) = jwt.rsplit_once('.').expect("three segments");
        let flipped = if signature.starts_with('A') { 'B' } else { 'A' };
        let tampered = format!("{body}.{flipped}{}", &signature[1..]);
        let err = verify_with_jwk(&tampered, &jwk(), AUDIENCE, CHAT_ISSUER).unwrap_err();
        assert!(matches!(err, GoogleAuthError::InvalidToken(_)));
    }

    #[test]
    fn parses_cache_control_max_age() {
        assert_eq!(
            max_age_secs("public, max-age=24542, must-revalidate, no-transform"),
            Some(24542)
        );
        assert_eq!(max_age_secs("Max-Age=60"), Some(60));
        assert_eq!(max_age_secs("no-store"), None);
        assert_eq!(max_age_secs(""), None);
    }
}
