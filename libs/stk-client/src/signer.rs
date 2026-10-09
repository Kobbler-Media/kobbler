//! RSA request signing for Amazon APIs.

use crate::error::Result;
use crate::models::DeviceInfo;
use base64::Engine;
use chrono::Utc;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::traits::{PrivateKeyParts, PublicKeyParts};
use rsa::{BigUint, RsaPrivateKey};
use sha2::{Digest, Sha256};

/// Request signer for Amazon STK API.
///
/// Generates the `X-ADP-Authentication-Token` and `X-ADP-Request-Digest` headers
/// required for authenticated API requests.
#[derive(Debug, Clone)]
pub struct Signer {
    /// RSA private key for signing.
    private_key: RsaPrivateKey,

    /// ADP token for authentication header.
    pub adp_token: String,
}

impl Signer {
    /// Create a signer from device info.
    pub fn from_device_info(device_info: &DeviceInfo) -> Result<Self> {
        let private_key = RsaPrivateKey::from_pkcs1_pem(&device_info.device_private_key)?;
        Ok(Self {
            private_key,
            adp_token: device_info.adp_token.clone(),
        })
    }

    /// Compute the X-ADP-Request-Digest header value.
    ///
    /// The format is: `{base64_signature}:{timestamp}`
    pub fn digest_header_for_request(
        &self,
        method: &str,
        path: &str,
        body: &str,
    ) -> Result<String> {
        let timestamp = get_signing_date();
        self.digest_header_with_date(method, path, body, &timestamp)
    }

    /// Compute digest header with a specific timestamp (for testing).
    pub fn digest_header_with_date(
        &self,
        method: &str,
        path: &str,
        body: &str,
        timestamp: &str,
    ) -> Result<String> {
        // Build signature data: method\npath\ntimestamp\nbody\nadp_token
        let sig_data = format!(
            "{}\n{}\n{}\n{}\n{}",
            method, path, timestamp, body, self.adp_token
        );

        // SHA-256 hash
        let digest = sha256(sig_data.as_bytes());

        // PKCS#1 v1.5 padding (RFC 8017)
        // The pad is 01 || FF...FF || 00 || DigestInfo || Hash
        // For SHA-256, the total padded message is 256 bytes (2048-bit key)
        //
        // The stkclient implementation uses a pre-computed pad that includes
        // the DigestInfo for SHA-256. We replicate that approach.
        let pad = hex::decode(
            "01ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff00"
        ).unwrap();

        // Padded message = pad || digest
        let mut padded = pad;
        padded.extend_from_slice(&digest);

        // Convert to BigUint for RSA operation
        let payload = BigUint::from_bytes_be(&padded);

        // RSA "encrypt" with private key (raw RSA operation)
        // encrypted = payload^d mod n
        let encrypted = payload.modpow(self.private_key.d(), self.private_key.n());

        // Convert to fixed-size bytes (256 bytes for 2048-bit key)
        let encrypted_bytes = to_fixed_bytes(&encrypted, 256);

        // Base64 encode
        let encoded = base64::engine::general_purpose::STANDARD.encode(&encrypted_bytes);

        Ok(format!("{}:{}", encoded, timestamp))
    }
}

/// Get current UTC timestamp in ISO 8601 format for signing.
fn get_signing_date() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Compute SHA-256 hash.
fn sha256(data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

/// Convert BigUint to fixed-size byte array (big-endian, zero-padded).
fn to_fixed_bytes(n: &BigUint, size: usize) -> Vec<u8> {
    let bytes = n.to_bytes_be();
    if bytes.len() >= size {
        bytes[bytes.len() - size..].to_vec()
    } else {
        let mut padded = vec![0u8; size - bytes.len()];
        padded.extend_from_slice(&bytes);
        padded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signing_date_format() {
        let date = get_signing_date();
        // Should match format like "2021-10-09T05:02:38Z"
        assert!(date.ends_with('Z'));
        assert_eq!(date.len(), 20);
    }
}
