//! JWT-based sync token for device-carried sync state.
//!
//! Encodes sync cursor as a JWT in the `x-kobo-synctoken` header.
//! The device round-trips this token; decode failure triggers a full resync.

use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Static signing key. Doesn't need real security — just needs to produce
/// tokens that look like real Kobo sync tokens to the device firmware.
const JWT_SECRET: &[u8] = b"kobbler-sync-token-key";

/// Sync state carried by the device in `x-kobo-synctoken`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncToken {
    /// Schema version (for future-proofing).
    pub v: i32,
    /// Last synced unix timestamp (microseconds).
    pub ls: i64,
    /// Upstream Kobo sync token (for merge mode).
    pub kt: String,
}

impl Default for SyncToken {
    fn default() -> Self {
        Self {
            v: 1,
            ls: 0,
            kt: String::new(),
        }
    }
}

impl SyncToken {
    /// Try to decode a sync token from a header value.
    /// Returns `None` if the value is not a valid Kobbler JWT (e.g. a real Kobo token).
    pub fn try_decode(header: &str) -> Option<Self> {
        let header = header.trim();
        if header.is_empty() {
            return None;
        }

        let key = DecodingKey::from_secret(JWT_SECRET);
        let mut validation = Validation::default();
        validation.required_spec_claims.clear();
        validation.validate_exp = false;
        validation.validate_aud = false;

        decode::<SyncToken>(header, &key, &validation)
            .map(|data| data.claims)
            .ok()
    }

    /// Decode a sync token from a header value. Returns default on any error.
    pub fn decode(header: &str) -> Self {
        let header = header.trim();
        if header.is_empty() {
            return Self::default();
        }

        let key = DecodingKey::from_secret(JWT_SECRET);
        let mut validation = Validation::default();
        validation.required_spec_claims.clear();
        validation.validate_exp = false;
        validation.validate_aud = false;

        decode::<SyncToken>(header, &key, &validation)
            .map(|data| data.claims)
            .unwrap_or_default()
    }

    /// Encode this token as a JWT string.
    pub fn encode(&self) -> String {
        let key = EncodingKey::from_secret(JWT_SECRET);
        encode(&Header::default(), self, &key).unwrap_or_default()
    }

    /// Get the last sync time as an OffsetDateTime.
    pub fn last_sync_at(&self) -> OffsetDateTime {
        let secs = self.ls / 1_000_000;
        let micros = self.ls % 1_000_000;
        OffsetDateTime::from_unix_timestamp(secs).unwrap_or(OffsetDateTime::UNIX_EPOCH)
            + time::Duration::microseconds(micros)
    }

    /// Create a new token with the given last sync timestamp.
    pub fn with_last_sync(mut self, ts: OffsetDateTime) -> Self {
        self.ls = ts.unix_timestamp() * 1_000_000 + ts.microsecond() as i64;
        self
    }

    /// Create a new token with the given upstream Kobo token.
    pub fn with_kobo_token(mut self, kt: String) -> Self {
        self.kt = kt;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_roundtrip() {
        let token = SyncToken {
            v: 1,
            ls: 1_706_000_000_500_207, // microseconds
            kt: "upstream-token-abc".to_string(),
        };
        let encoded = token.encode();
        let decoded = SyncToken::decode(&encoded);
        assert_eq!(decoded.v, 1);
        assert_eq!(decoded.ls, 1_706_000_000_500_207);
        assert_eq!(decoded.kt, "upstream-token-abc");
    }

    #[test]
    fn test_decode_invalid_jwt() {
        let decoded = SyncToken::decode("not-a-jwt");
        assert_eq!(decoded.ls, 0);
        assert_eq!(decoded.kt, "");
    }

    #[test]
    fn test_decode_empty_string() {
        let decoded = SyncToken::decode("");
        assert_eq!(decoded.ls, 0);
    }

    #[test]
    fn test_default_token() {
        let token = SyncToken::default();
        assert_eq!(token.v, 1);
        assert_eq!(token.ls, 0);
        assert_eq!(token.kt, "");
        assert_eq!(token.last_sync_at(), OffsetDateTime::UNIX_EPOCH);
    }

    #[test]
    fn test_try_decode_returns_none_for_non_kobbler_token() {
        // A real Kobo-style token string won't decode as our JWT
        assert!(SyncToken::try_decode("some-real-kobo-sync-token-abc123").is_none());
        assert!(SyncToken::try_decode("").is_none());
    }

    #[test]
    fn test_try_decode_returns_some_for_kobbler_token() {
        let token = SyncToken::default().with_kobo_token("real-kobo-token".to_string());
        let encoded = token.encode();
        let decoded = SyncToken::try_decode(&encoded);
        assert!(decoded.is_some());
        assert_eq!(decoded.unwrap().kt, "real-kobo-token");
    }

    #[test]
    fn test_with_kobo_token() {
        let token = SyncToken::default().with_kobo_token("kobo-123".to_string());
        let encoded = token.encode();
        let decoded = SyncToken::decode(&encoded);
        assert_eq!(decoded.kt, "kobo-123");
    }
}
