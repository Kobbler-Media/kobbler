use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Serialize;
use uuid::Uuid;

use crate::AppState;
use crate::routes::AuthUser;
use crate::subtitle_providers::types::{SubtitleCapability, SubtitleConfigFieldType};
use crate::subtitle_providers::{SubtitleProviderRegistry, UpdateSubtitleProviderRequest};

// ============================================================================
// Provider Response Types
// ============================================================================

#[derive(Debug, Serialize)]
pub struct SubtitleConfigFieldResponse {
    pub key: String,
    pub label: String,
    pub field_type: SubtitleConfigFieldType,
    pub required: bool,
    pub description: Option<String>,
    pub help_url: Option<String>,
    pub default: Option<String>,
    pub current_value: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct SubtitleProviderResponse {
    pub id: Uuid,
    pub provider_type: String,
    pub enabled: bool,
    pub priority: i32,
    pub display_name: String,
    pub description: String,
    pub website: String,
    pub capabilities: Vec<SubtitleCapability>,
    pub config_fields: Vec<SubtitleConfigFieldResponse>,
}

// ============================================================================
// List Providers
// ============================================================================

/// List all subtitle providers with their capabilities and config schema.
/// GET /api/v1/admin/subtitle-providers/providers
pub async fn list_providers(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let registry = SubtitleProviderRegistry::new(state.pool.clone());
    let stored_providers = registry.get_all_providers().await;

    let responses: Vec<SubtitleProviderResponse> = stored_providers
        .into_iter()
        .filter_map(|stored| {
            let info = registry.get_provider_info(&stored.provider_type)?;

            let config_fields: Vec<SubtitleConfigFieldResponse> = info
                .config_fields
                .iter()
                .map(|field| SubtitleConfigFieldResponse {
                    key: field.key.to_string(),
                    label: field.label.to_string(),
                    field_type: field.field_type,
                    required: field.required,
                    description: field.description.map(String::from),
                    help_url: field.help_url.map(String::from),
                    default: field.default.map(String::from),
                    current_value: stored.config.get(field.key).cloned(),
                })
                .collect();

            Some(SubtitleProviderResponse {
                id: stored.id,
                provider_type: stored.provider_type,
                enabled: stored.enabled,
                priority: stored.priority,
                display_name: info.display_name.to_string(),
                description: info.description.to_string(),
                website: info.website.to_string(),
                capabilities: info.capabilities.to_vec(),
                config_fields,
            })
        })
        .collect();

    Json(responses).into_response()
}

// ============================================================================
// Update Provider
// ============================================================================

/// Update a subtitle provider's configuration.
/// PATCH /api/v1/admin/subtitle-providers/providers/:id
pub async fn update_provider(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(provider_id): Path<Uuid>,
    Json(update): Json<UpdateSubtitleProviderRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let registry = SubtitleProviderRegistry::new(state.pool.clone());
    match registry.update_provider(provider_id, &update).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!("Failed to update subtitle provider: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to update provider" })),
            )
                .into_response()
        }
    }
}
