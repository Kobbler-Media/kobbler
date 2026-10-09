//! OAuth2 authentication flow for Amazon STK.

use crate::api;
use crate::client::Client;
use crate::error::{Error, Result};
use base64::Engine;
use sha2::{Digest, Sha256};

/// Amazon OAuth2 client ID for the STK service.
const CLIENT_ID: &str =
    "device:658490dfb190e494030082836775981fa23be0c2425441860352ba0f55915b43002d";

/// OAuth2 return URL.
const RETURN_TO: &str = "https://www.amazon.com/gp/sendtokindle";

/// OAuth2 authentication handler for Amazon STK.
///
/// Implements PKCE (Proof Key for Code Exchange) for secure authorization.
#[derive(Debug, Clone)]
pub struct OAuth2 {
    /// The PKCE code verifier (random string).
    verifier: String,
}

impl Default for OAuth2 {
    fn default() -> Self {
        Self::new()
    }
}

impl OAuth2 {
    /// Create a new OAuth2 instance with a random verifier.
    pub fn new() -> Self {
        let random_bytes: [u8; 32] = rand::random();
        let verifier = base64_url_encode(&random_bytes);
        Self { verifier }
    }

    /// Restore an OAuth2 instance from a previously saved verifier.
    ///
    /// Use this when completing the OAuth flow after the redirect.
    pub fn with_verifier(verifier: String) -> Self {
        Self { verifier }
    }

    /// Get the PKCE verifier for storage between OAuth steps.
    pub fn verifier(&self) -> &str {
        &self.verifier
    }

    /// Generate the Amazon sign-in URL.
    ///
    /// Open this URL in a browser to start the OAuth flow.
    pub fn get_signin_url(&self) -> String {
        let challenge = base64_url_encode(&sha256(self.verifier.as_bytes()));

        let params = [
            (
                "openid.claimed_id",
                "http://specs.openid.net/auth/2.0/identifier_select",
            ),
            ("openid.ns.oa2", "http://www.amazon.com/ap/ext/oauth/2"),
            ("openid.ns", "http://specs.openid.net/auth/2.0"),
            (
                "openid.identity",
                "http://specs.openid.net/auth/2.0/identifier_select",
            ),
            ("openid.oa2.client_id", CLIENT_ID),
            ("openid.mode", "checkid_setup"),
            ("openid.oa2.scope", "device_auth_access"),
            ("openid.oa2.response_type", "code"),
            ("openid.oa2.code_challenge", &challenge),
            ("openid.oa2.code_challenge_method", "S256"),
            ("openid.return_to", RETURN_TO),
            (
                "openid.ns.pape",
                "http://specs.openid.net/extensions/pape/1.0",
            ),
            ("openid.pape.max_auth_age", "0"),
            ("accountStatusPolicy", "P1"),
            ("openid.assoc_handle", "amzn_device_na"),
            ("pageId", "amzn_device_common_dark"),
            ("disableLoginPrepopulate", "1"),
        ];

        let query = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        format!("https://www.amazon.com/ap/signin?{}", query)
    }

    /// Complete the OAuth flow with the redirect URL.
    ///
    /// After the user signs in and authorizes, Amazon redirects to a URL
    /// containing the authorization code. Pass that full URL here.
    pub async fn create_client(&self, redirect_url: &str) -> Result<Client> {
        let code = parse_authorization_code(redirect_url)?;
        let access_token = api::token_exchange(&code, &self.verifier).await?;
        let device_info = api::register_device_with_token(&access_token).await?;
        Ok(Client::new(device_info))
    }
}

/// Parse the authorization code from an OAuth2 redirect URL.
fn parse_authorization_code(redirect_url: &str) -> Result<String> {
    let url = url::Url::parse(redirect_url)?;

    // Look for openid.oa2.authorization_code in query params
    for (key, value) in url.query_pairs() {
        if key == "openid.oa2.authorization_code" {
            return Ok(value.into_owned());
        }
    }

    Err(Error::OAuth(
        "Missing authorization code in redirect URL".to_string(),
    ))
}

/// Base64-URL encode bytes (no padding).
fn base64_url_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Compute SHA-256 hash.
fn sha256(data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

/// Re-export for use by urlencoding
mod urlencoding {
    pub fn encode(s: &str) -> String {
        url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_authorization_code() {
        let url = "https://www.amazon.com/gp/sendtokindle?openid.assoc_handle=amzn_device_na&openid.oa2.authorization_code=FAKEAUTHCODE0000000000000&other=param";
        let code = parse_authorization_code(url).unwrap();
        assert_eq!(code, "FAKEAUTHCODE0000000000000");
    }

    #[test]
    fn test_signin_url_generation() {
        let oauth = OAuth2::with_verifier("test_verifier".to_string());
        let url = oauth.get_signin_url();
        assert!(url.starts_with("https://www.amazon.com/ap/signin?"));
        assert!(url.contains("openid.oa2.code_challenge="));
        assert!(url.contains("openid.oa2.code_challenge_method=S256"));
    }
}
