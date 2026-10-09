use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;

use super::types::{NotificationAgentInfo, NotificationConfigField, NotificationTrigger};
use super::{NotificationAgent, NotificationAgentFactory, NotificationError, NotificationPayload};
use uuid::Uuid;

// ============================================================================
// Agent Info
// ============================================================================

static SUPPORTED_TRIGGERS: &[NotificationTrigger] = &[
    NotificationTrigger::OnGrab,
    NotificationTrigger::OnImport,
    NotificationTrigger::OnFailed,
    NotificationTrigger::OnMediaCorrupt,
];

pub static INFO: NotificationAgentInfo = NotificationAgentInfo {
    id: "webpush",
    display_name: "Web Push",
    description: "Browser push notifications",
    supported_triggers: SUPPORTED_TRIGGERS,
    config_fields: &[] as &[NotificationConfigField],
};

// ============================================================================
// Factory
// ============================================================================

pub struct WebPushFactory;

impl NotificationAgentFactory for WebPushFactory {
    fn info(&self) -> &'static NotificationAgentInfo {
        &INFO
    }

    fn create(
        &self,
        client: reqwest::Client,
        config: &serde_json::Value,
        _pool: &sqlx::PgPool,
        _agent_id: Uuid,
    ) -> Box<dyn NotificationAgent> {
        let vapid_private_key = config
            .get("vapid_private_key")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let vapid_public_key = config
            .get("vapid_public_key")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let subscriptions: Vec<PushSubscription> = config
            .get("subscriptions")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        Box::new(WebPushAgent {
            client,
            vapid_private_key,
            vapid_public_key,
            subscriptions,
        })
    }
}

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, Deserialize)]
struct PushSubscription {
    endpoint: String,
    p256dh: String,
    auth: String,
}

// ============================================================================
// Agent
// ============================================================================

struct WebPushAgent {
    client: reqwest::Client,
    vapid_private_key: String,
    vapid_public_key: String,
    subscriptions: Vec<PushSubscription>,
}

#[async_trait]
impl NotificationAgent for WebPushAgent {
    fn info(&self) -> &'static NotificationAgentInfo {
        &INFO
    }

    async fn send(&self, payload: &NotificationPayload) -> Result<(), NotificationError> {
        if self.vapid_private_key.is_empty() || self.vapid_public_key.is_empty() {
            return Err(NotificationError::ConfigMissing(
                "vapid_private_key and vapid_public_key".into(),
            ));
        }

        if self.subscriptions.is_empty() {
            tracing::debug!("No web push subscriptions to notify");
            return Ok(());
        }

        let plaintext = serde_json::to_vec(payload)
            .map_err(|e| NotificationError::Parse(format!("Failed to serialize payload: {e}")))?;

        for sub in &self.subscriptions {
            match self.send_to_subscription(sub, &plaintext).await {
                Ok(()) => {
                    tracing::debug!("Web push sent to {}", sub.endpoint);
                }
                Err(NotificationError::ServiceError(ref msg)) if msg.contains("410") => {
                    tracing::info!("Web push subscription expired (410 Gone): {}", sub.endpoint);
                }
                Err(e) => {
                    tracing::warn!("Web push to {} failed: {}", sub.endpoint, e);
                }
            }
        }

        Ok(())
    }
}

impl WebPushAgent {
    async fn send_to_subscription(
        &self,
        sub: &PushSubscription,
        plaintext: &[u8],
    ) -> Result<(), NotificationError> {
        let encrypted = encrypt_payload(sub, plaintext)?;
        let vapid_header = build_vapid_header(
            &sub.endpoint,
            &self.vapid_private_key,
            &self.vapid_public_key,
        )?;

        let resp = self
            .client
            .post(&sub.endpoint)
            .header("Content-Type", "application/octet-stream")
            .header("Content-Encoding", "aes128gcm")
            .header("TTL", "86400")
            .header("Authorization", &vapid_header)
            .body(encrypted)
            .send()
            .await?;

        let status = resp.status();
        if status == reqwest::StatusCode::GONE {
            return Err(NotificationError::ServiceError("410 Gone".into()));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NotificationError::ServiceError(format!(
                "Push service returned {status}: {body}"
            )));
        }

        Ok(())
    }
}

// ============================================================================
// RFC 8291 Content Encryption (aes128gcm)
// ============================================================================

/// Encrypt a web push payload per RFC 8291 using aes128gcm encoding.
fn encrypt_payload(sub: &PushSubscription, plaintext: &[u8]) -> Result<Vec<u8>, NotificationError> {
    use aes_gcm::{Aes128Gcm, KeyInit, Nonce, aead::Aead};
    use hkdf::Hkdf;
    use p256::PublicKey;
    use p256::ecdh::EphemeralSecret;
    use p256::elliptic_curve::Generate;
    use p256::elliptic_curve::sec1::ToSec1Point;
    use rand::Rng;
    use sha2::Sha256;

    // Decode subscriber's public key and auth secret
    let ua_public_bytes = URL_SAFE_NO_PAD
        .decode(&sub.p256dh)
        .map_err(|e| NotificationError::Parse(format!("Invalid p256dh: {e}")))?;
    let auth_secret = URL_SAFE_NO_PAD
        .decode(&sub.auth)
        .map_err(|e| NotificationError::Parse(format!("Invalid auth: {e}")))?;

    let ua_public = PublicKey::from_sec1_bytes(&ua_public_bytes)
        .map_err(|e| NotificationError::Parse(format!("Invalid subscriber public key: {e}")))?;

    // Generate ephemeral key pair for this message
    let eph_secret = EphemeralSecret::generate_from_rng(&mut rand::rng());
    let eph_public = eph_secret.public_key();
    let eph_public_bytes = eph_public.to_sec1_point(false);

    // ECDH shared secret
    let shared_secret = eph_secret.diffie_hellman(&ua_public);

    // RFC 8291 Section 3.3: IKM derivation
    // ikm_info = "WebPush: info\0" || ua_public || as_public
    let mut ikm_info = Vec::with_capacity(144);
    ikm_info.extend_from_slice(b"WebPush: info\0");
    ikm_info.extend_from_slice(&ua_public_bytes);
    ikm_info.extend_from_slice(eph_public_bytes.as_bytes());

    let hkdf_auth = Hkdf::<Sha256>::new(Some(&auth_secret), shared_secret.raw_secret_bytes());
    let mut ikm = [0u8; 32];
    hkdf_auth
        .expand(&ikm_info, &mut ikm)
        .map_err(|e| NotificationError::ServiceError(format!("HKDF auth expand failed: {e}")))?;

    // RFC 8291 Section 3.4: Content encryption key and nonce derivation
    // Generate random 16-byte salt
    let mut salt = [0u8; 16];
    rand::rng().fill_bytes(&mut salt);

    let hkdf_content = Hkdf::<Sha256>::new(Some(&salt), &ikm);

    let mut cek = [0u8; 16];
    hkdf_content
        .expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .map_err(|e| NotificationError::ServiceError(format!("HKDF CEK expand failed: {e}")))?;

    let mut nonce_bytes = [0u8; 12];
    hkdf_content
        .expand(b"Content-Encoding: nonce\0", &mut nonce_bytes)
        .map_err(|e| NotificationError::ServiceError(format!("HKDF nonce expand failed: {e}")))?;

    // Encrypt with AES-128-GCM
    // Plaintext gets a padding delimiter byte (0x02 for final record)
    let mut padded = Vec::with_capacity(plaintext.len() + 1);
    padded.extend_from_slice(plaintext);
    padded.push(0x02); // delimiter: final record

    let cipher = Aes128Gcm::new_from_slice(&cek)
        .map_err(|e| NotificationError::ServiceError(format!("AES key init failed: {e}")))?;
    #[allow(deprecated)]
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, padded.as_slice())
        .map_err(|e| NotificationError::ServiceError(format!("AES encryption failed: {e}")))?;

    // Build aes128gcm header: salt(16) || rs(4) || idlen(1) || keyid(65)
    let rs: u32 = 4096;
    let key_id = eph_public_bytes.as_bytes();

    let mut result = Vec::with_capacity(16 + 4 + 1 + key_id.len() + ciphertext.len());
    result.extend_from_slice(&salt);
    result.extend_from_slice(&rs.to_be_bytes());
    result.push(key_id.len() as u8);
    result.extend_from_slice(key_id);
    result.extend_from_slice(&ciphertext);

    Ok(result)
}

// ============================================================================
// VAPID Authentication
// ============================================================================

/// Build the VAPID Authorization header value.
fn build_vapid_header(
    endpoint: &str,
    private_key_b64: &str,
    public_key_b64: &str,
) -> Result<String, NotificationError> {
    let private_key_der = URL_SAFE_NO_PAD
        .decode(private_key_b64)
        .map_err(|e| NotificationError::Parse(format!("Invalid VAPID private key: {e}")))?;

    // Extract the audience (origin) from the endpoint URL
    let parsed = url::Url::parse(endpoint)
        .map_err(|e| NotificationError::Parse(format!("Invalid endpoint URL: {e}")))?;
    let audience = format!("{}://{}", parsed.scheme(), parsed.host_str().unwrap_or(""));

    let encoding_key = jsonwebtoken::EncodingKey::from_ec_der(&private_key_der);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let claims = serde_json::json!({
        "aud": audience,
        "exp": now + 12 * 3600,
        "sub": "mailto:admin@kobbler.local",
    });

    let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
    let jwt = jsonwebtoken::encode(&header, &claims, &encoding_key)
        .map_err(|e| NotificationError::ServiceError(format!("VAPID JWT signing failed: {e}")))?;

    Ok(format!("vapid t={jwt},k={public_key_b64}"))
}

// ============================================================================
// VAPID Key Generation
// ============================================================================

/// Generate a new VAPID key pair.
///
/// Returns `(public_key_base64url, private_key_base64url)` where the private key
/// is PKCS8 DER encoded and the public key is the uncompressed SEC1 point.
pub fn generate_vapid_keys() -> (String, String) {
    use p256::SecretKey;
    use p256::elliptic_curve::Generate;
    use p256::elliptic_curve::sec1::ToSec1Point;
    use p256::pkcs8::EncodePrivateKey;

    let secret_key = SecretKey::generate_from_rng(&mut rand::rng());

    let private_der = secret_key
        .to_pkcs8_der()
        .expect("Failed to encode VAPID private key as PKCS8 DER");
    let private_b64 = URL_SAFE_NO_PAD.encode(private_der.as_bytes());

    let public_key = secret_key.public_key();
    let public_point = public_key.to_sec1_point(false);
    let public_b64 = URL_SAFE_NO_PAD.encode(public_point.as_bytes());

    (public_b64, private_b64)
}
