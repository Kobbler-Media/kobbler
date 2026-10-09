//! Shared helper functions for routes.

use crate::kobload::KobloadClient;
use crate::runtime_config::RuntimeConfig;
use axum::http::StatusCode;

/// Build a Kobload client from runtime config.
/// Returns an error response tuple if Kobload is not configured.
pub fn build_kobload_client(
    runtime: &RuntimeConfig,
) -> Result<KobloadClient, (StatusCode, String)> {
    let base_url = runtime.kobload_base_url.clone();
    let api_key = runtime.kobload_api_key.clone();

    let base_url = match base_url {
        Some(val) if !val.is_empty() => val,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "Kobload Base URL not configured. Go to Settings > System > Kobload and set the Base URL.".to_string(),
            ));
        }
    };
    let api_key = match api_key {
        Some(val) if !val.is_empty() => val,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "Kobload API Key not configured. Go to Settings > System > Kobload and set the API Key.".to_string(),
            ));
        }
    };

    Ok(KobloadClient::new(base_url, api_key))
}
