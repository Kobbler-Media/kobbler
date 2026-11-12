//! Notification system route handlers.

use axum::{
    extract::{Json, Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use tracing::error;
use uuid::Uuid;

use crate::notifications::{
    ConfigFieldType, NotificationAgentInfo, NotificationAgentRegistry, NotificationConfigField,
    NotificationPayload, NotificationTrigger, generate_vapid_keys,
};
use crate::repositories::{notification_agents, webpush_subscriptions};
use crate::routes::AuthUser;
use crate::{AppState, jobs};

// =============================================================================
// Response Types
// =============================================================================

#[derive(Serialize)]
struct ConfigFieldResponse {
    key: &'static str,
    label: &'static str,
    field_type: ConfigFieldType,
    required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    help_url: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<&'static str>,
}

#[derive(Serialize)]
struct NotificationTypeResponse {
    id: &'static str,
    display_name: &'static str,
    description: &'static str,
    supported_triggers: &'static [NotificationTrigger],
    config_fields: Vec<ConfigFieldResponse>,
}

#[derive(Serialize)]
struct NotificationAgentResponse {
    id: Uuid,
    agent_type: String,
    name: String,
    enabled: bool,
    config: serde_json::Value,
    triggers: serde_json::Value,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
    display_name: &'static str,
    description: &'static str,
}

// =============================================================================
// Request Types
// =============================================================================

#[derive(Deserialize)]
pub struct CreateAgentRequest {
    agent_type: String,
    name: String,
    enabled: Option<bool>,
    config: serde_json::Value,
    triggers: serde_json::Value,
}

#[derive(Deserialize)]
pub struct UpdateAgentRequest {
    name: Option<String>,
    enabled: Option<bool>,
    config: Option<std::collections::HashMap<String, serde_json::Value>>,
    triggers: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct SubscribeRequest {
    endpoint: String,
    p256dh: String,
    auth: String,
    user_agent: Option<String>,
}

#[derive(Deserialize)]
pub struct UnsubscribeRequest {
    endpoint: String,
}

// =============================================================================
// Helpers
// =============================================================================

fn config_field_response(field: &NotificationConfigField) -> ConfigFieldResponse {
    ConfigFieldResponse {
        key: field.key,
        label: field.label,
        field_type: field.field_type,
        required: field.required,
        description: field.description,
        help_url: field.help_url,
        default: field.default,
    }
}

fn type_response(info: &'static NotificationAgentInfo) -> NotificationTypeResponse {
    NotificationTypeResponse {
        id: info.id,
        display_name: info.display_name,
        description: info.description,
        supported_triggers: info.supported_triggers,
        config_fields: info
            .config_fields
            .iter()
            .map(config_field_response)
            .collect(),
    }
}

/// Redact secret fields in an agent config, replacing non-empty values with "••••••".
fn redact_config(config: &serde_json::Value, info: &NotificationAgentInfo) -> serde_json::Value {
    let Some(obj) = config.as_object() else {
        return config.clone();
    };

    let secret_keys: std::collections::HashSet<&str> = info
        .config_fields
        .iter()
        .filter(|f| f.field_type == ConfigFieldType::Secret)
        .map(|f| f.key)
        .collect();

    let mut redacted = obj.clone();
    for (key, value) in &mut redacted {
        if secret_keys.contains(key.as_str())
            && let Some(s) = value.as_str()
            && !s.is_empty()
        {
            *value = json!("••••••");
        }
    }

    serde_json::Value::Object(redacted)
}

fn agent_response(
    row: notification_agents::NotificationAgentRow,
    registry: &NotificationAgentRegistry,
) -> NotificationAgentResponse {
    let info = registry.get_type_info(&row.agent_type);
    let (display_name, description) = match info {
        Some(i) => (i.display_name, i.description),
        None => ("Unknown", "Unknown agent type"),
    };
    let config = match info {
        Some(i) => redact_config(&row.config, i),
        None => row.config,
    };

    NotificationAgentResponse {
        id: row.id,
        agent_type: row.agent_type,
        name: row.name,
        enabled: row.enabled,
        config,
        triggers: row.triggers,
        created_at: row.created_at,
        updated_at: row.updated_at,
        display_name,
        description,
    }
}

// =============================================================================
// Handlers
// =============================================================================

/// GET /api/v1/admin/notifications/types
pub async fn list_types(auth: AuthUser) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let registry = NotificationAgentRegistry::new();
    let types: Vec<NotificationTypeResponse> = registry
        .available_types()
        .into_iter()
        .map(type_response)
        .collect();
    Json(types).into_response()
}

/// GET /api/v1/admin/notifications/agents
pub async fn list_agents(State(state): State<AppState>, auth: AuthUser) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let rows = match notification_agents::list_all(&state.pool).await {
        Ok(rows) => rows,
        Err(e) => {
            error!(error = %e, "Failed to list notification agents");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to list notification agents" })),
            )
                .into_response();
        }
    };

    let registry = NotificationAgentRegistry::new();
    let agents: Vec<NotificationAgentResponse> = rows
        .into_iter()
        .map(|r| agent_response(r, &registry))
        .collect();
    Json(agents).into_response()
}

/// POST /api/v1/admin/notifications/agents
pub async fn create_agent(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreateAgentRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let registry = NotificationAgentRegistry::new();

    if registry.get_type_info(&req.agent_type).is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("Unknown agent type: {}", req.agent_type) })),
        )
            .into_response();
    }

    let input = notification_agents::CreateNotificationAgent {
        agent_type: req.agent_type,
        name: req.name,
        enabled: req.enabled.unwrap_or(true),
        config: req.config,
        triggers: req.triggers,
    };

    match notification_agents::create(&state.pool, input).await {
        Ok(row) => (
            StatusCode::CREATED,
            Json(json!(agent_response(row, &registry))),
        )
            .into_response(),
        Err(e) => {
            error!(error = %e, "Failed to create notification agent");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to create notification agent" })),
            )
                .into_response()
        }
    }
}

/// PATCH /api/v1/admin/notifications/agents/{id}
pub async fn update_agent(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateAgentRequest>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    let input = notification_agents::UpdateNotificationAgent {
        name: req.name,
        enabled: req.enabled,
        config: req.config,
        triggers: req.triggers,
    };

    match notification_agents::update(&state.pool, id, &input).await {
        Ok(Some(row)) => {
            let registry = NotificationAgentRegistry::new();
            Json(json!(agent_response(row, &registry))).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Agent not found" })),
        )
            .into_response(),
        Err(e) => {
            error!(error = %e, agent_id = %id, "Failed to update notification agent");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to update notification agent" })),
            )
                .into_response()
        }
    }
}

/// DELETE /api/v1/admin/notifications/agents/{id}
pub async fn delete_agent(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    match notification_agents::delete(&state.pool, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Agent not found" })),
        )
            .into_response(),
        Err(e) => {
            error!(error = %e, agent_id = %id, "Failed to delete notification agent");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to delete notification agent" })),
            )
                .into_response()
        }
    }
}

/// POST /api/v1/admin/notifications/agents/{id}/test
pub async fn test_agent(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = auth.require_admin() {
        return e.into_response();
    }
    // Verify the agent exists
    let agent = match notification_agents::get_by_id(&state.pool, id).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Agent not found" })),
            )
                .into_response();
        }
        Err(e) => {
            error!(error = %e, agent_id = %id, "Failed to load agent for test");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to load agent" })),
            )
                .into_response();
        }
    };

    let payload = NotificationPayload {
        trigger: NotificationTrigger::OnTest,
        title: "Kobbler Test".to_string(),
        message: "This is a test notification from Kobbler.".to_string(),
        entity_type: None,
        entity_id: None,
        entity_name: None,
        url: None,
        public_url: state.runtime_config.get().await.public_url.clone(),
        show_id: None,
        show_name: None,
        season_number: None,
        show_year: None,
        upgraded: false,
        details: None,
    };

    let job_payload = json!({
        "agent_id": id,
        "payload": payload,
    });

    let display = crate::jobs::fetch_job_display(&state.pool, "notification_agent", id).await;
    let context = jobs::JobContext::for_entity(
        "notification_agent",
        id,
        format!("Test notification via {}", agent.name),
    )
    .with_display_opt(display);

    match jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "notification_send",
        job_payload,
        context,
    )
    .await
    {
        Ok(_) => Json(json!({ "status": "test_sent" })).into_response(),
        Err(e) => {
            error!(error = %e, agent_id = %id, "Failed to enqueue test notification");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to enqueue test notification" })),
            )
                .into_response()
        }
    }
}

/// GET /api/v1/admin/notifications/webpush/vapid-key
pub async fn get_vapid_key(State(state): State<AppState>, _auth: AuthUser) -> impl IntoResponse {
    // Try to fetch existing VAPID keys
    let row = match sqlx::query!(
        "SELECT vapid_public_key, vapid_private_key FROM system_config WHERE id = 1"
    )
    .fetch_one(&state.pool)
    .await
    {
        Ok(row) => row,
        Err(e) => {
            error!(error = %e, "Failed to query system_config for VAPID keys");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to query VAPID keys" })),
            )
                .into_response();
        }
    };

    // If we already have a public key, return it
    if let Some(ref public_key) = row.vapid_public_key
        && !public_key.is_empty()
    {
        return Json(json!({ "public_key": public_key })).into_response();
    }

    // Generate new VAPID keys
    let (public_key, private_key) = generate_vapid_keys();

    // Store them
    if let Err(e) = sqlx::query!(
        "UPDATE system_config SET vapid_public_key = $1, vapid_private_key = $2 WHERE id = 1",
        public_key,
        private_key,
    )
    .execute(&state.pool)
    .await
    {
        error!(error = %e, "Failed to store VAPID keys");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "Failed to store VAPID keys" })),
        )
            .into_response();
    }

    Json(json!({ "public_key": public_key })).into_response()
}

/// POST /api/v1/admin/notifications/webpush/subscribe
pub async fn webpush_subscribe(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<SubscribeRequest>,
) -> impl IntoResponse {
    let input = webpush_subscriptions::CreateWebPushSubscription {
        user_id: auth.user_id,
        endpoint: req.endpoint,
        p256dh: req.p256dh,
        auth: req.auth,
        user_agent: req.user_agent,
    };

    if let Err(e) = webpush_subscriptions::create(&state.pool, input).await {
        error!(error = %e, "Failed to create webpush subscription");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "Failed to create subscription" })),
        )
            .into_response();
    }

    // Auto-create a webpush notification agent if none exists
    let has_agent: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM notification_agents WHERE agent_type = 'webpush')",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap_or(false);

    if !has_agent {
        let agent_input = notification_agents::CreateNotificationAgent {
            agent_type: "webpush".to_string(),
            name: "Web Push".to_string(),
            enabled: true,
            config: json!({}),
            triggers: json!([
                "on_grab",
                "on_import",
                "on_failed",
                "on_media_corrupt",
                "on_server_health"
            ]),
        };
        if let Err(e) = notification_agents::create(&state.pool, agent_input).await {
            error!(error = %e, "Failed to auto-create webpush agent");
        }
    }

    StatusCode::CREATED.into_response()
}

/// DELETE /api/v1/admin/notifications/webpush/subscribe
pub async fn webpush_unsubscribe(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<UnsubscribeRequest>,
) -> impl IntoResponse {
    match webpush_subscriptions::delete_by_endpoint(&state.pool, auth.user_id, &req.endpoint).await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            error!(error = %e, "Failed to delete webpush subscription");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to delete subscription" })),
            )
                .into_response()
        }
    }
}
