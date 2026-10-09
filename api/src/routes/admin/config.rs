//! System configuration handlers.

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use tracing::{error, info, warn};

use crate::routes::AuthUser;
use crate::{AppState, runtime_config::RuntimeConfigUpdate};

/// Koblem is wired for one specific hardware setup, down to GPU gating with no
/// non-DB way to edit it, so its settings fields stay hidden in the UI unless
/// explicitly opted into.
fn is_koblem_ui_enabled() -> bool {
    std::env::var("KOBLEM_UI_ENABLED")
        .ok()
        .filter(|s| s == "1")
        .is_some()
}

/// Get the current runtime configuration.
pub async fn get_config(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let config = state.runtime_config.get().await;
    let mut value = serde_json::to_value(&config).unwrap_or_default();
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "koblem_ui_enabled".to_string(),
            serde_json::Value::Bool(is_koblem_ui_enabled()),
        );
    }
    Json(value).into_response()
}

/// Update the runtime configuration.
pub async fn update_config(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<RuntimeConfigUpdate>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    // Capture old meili settings to detect changes
    let old_config = state.runtime_config.get().await;
    let old_meili_url = old_config.meili_url.clone();
    let old_meili_master_key = old_config.meili_master_key.clone();

    // Check if meili settings are being updated
    let meili_url_changed = payload.meili_url.is_some();
    let meili_key_changed = payload.meili_master_key.is_some();

    // Check if chat config changed (before payload is consumed by update)
    let chat_changed = payload.chat_enabled.is_some()
        || payload.chat_llm_provider.is_some()
        || payload.chat_llm_base_url.is_some()
        || payload.chat_llm_api_key.is_some()
        || payload.chat_llm_model.is_some()
        || payload.koblem_service_url.is_some();

    match state.runtime_config.update(payload).await {
        Ok(()) => {
            let config = state.runtime_config.get().await;

            // If meili settings changed, reconfigure the search backend
            if meili_url_changed || meili_key_changed {
                let new_url = config.meili_url.as_deref();
                let new_key = config.meili_master_key.as_deref();

                // Only reconfigure if we have both URL and key
                if let (Some(url), Some(key)) = (new_url, new_key) {
                    // Check if values actually changed
                    let url_changed = old_meili_url.as_deref() != Some(url);
                    let key_changed = old_meili_master_key.as_deref() != Some(key);

                    if url_changed || key_changed {
                        info!(url = %url, "Reconfiguring search backend");
                        if let Err(e) = state.search.reconfigure(url, key).await {
                            warn!(error = %e, "Failed to reconfigure search backend");
                            // Don't fail the config update - search will use old settings
                        }
                    }
                }
            }

            // If chat config changed, reconfigure chat workspace
            if chat_changed && config.chat_enabled {
                let meili_url = config
                    .meili_url
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&state.config.meili_url);
                let meili_key = config
                    .meili_master_key
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&state.config.meili_master_key);
                match meilisearch_sdk::client::Client::new(meili_url, Some(meili_key)) {
                    Ok(client) => {
                        if let Err(e) =
                            crate::search::chat::setup_chat(&client, meili_url, meili_key, &config)
                                .await
                        {
                            warn!(error = %e, "Failed to configure chat workspace");
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to create MeiliSearch client for chat setup");
                    }
                }
            }

            Json(config).into_response()
        }
        Err(crate::runtime_config::RuntimeConfigError::Validation(message)) => {
            (StatusCode::BAD_REQUEST, message).into_response()
        }
        Err(e) => {
            error!(error = %e, "Failed to update config");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to update config").into_response()
        }
    }
}
