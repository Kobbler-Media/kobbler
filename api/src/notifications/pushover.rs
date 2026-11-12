use async_trait::async_trait;

use super::types::{
    ConfigFieldType, NotificationAgentInfo, NotificationConfigField, NotificationTrigger,
};
use super::{NotificationAgent, NotificationAgentFactory, NotificationError, NotificationPayload};
use uuid::Uuid;

// ============================================================================
// Agent Info
// ============================================================================

static CONFIG_FIELDS: &[NotificationConfigField] = &[
    NotificationConfigField {
        key: "user_key",
        label: "User Key",
        field_type: ConfigFieldType::Secret,
        required: true,
        description: Some("Your Pushover user key"),
        help_url: None,
        default: None,
    },
    NotificationConfigField {
        key: "api_token",
        label: "API Token",
        field_type: ConfigFieldType::Secret,
        required: true,
        description: Some("Application API token from Pushover"),
        help_url: None,
        default: None,
    },
    NotificationConfigField {
        key: "device",
        label: "Device",
        field_type: ConfigFieldType::String,
        required: false,
        description: Some("Target device name (leave empty for all devices)"),
        help_url: None,
        default: None,
    },
    NotificationConfigField {
        key: "priority",
        label: "Priority",
        field_type: ConfigFieldType::Number,
        required: false,
        description: Some("Message priority (-2 to 2)"),
        help_url: None,
        default: Some("0"),
    },
    NotificationConfigField {
        key: "sound",
        label: "Sound",
        field_type: ConfigFieldType::String,
        required: false,
        description: Some("Notification sound name"),
        help_url: None,
        default: Some("pushover"),
    },
];

static SUPPORTED_TRIGGERS: &[NotificationTrigger] = &[
    NotificationTrigger::OnGrab,
    NotificationTrigger::OnImport,
    NotificationTrigger::OnFailed,
    NotificationTrigger::OnMediaCorrupt,
    NotificationTrigger::OnServerHealth,
];

pub static INFO: NotificationAgentInfo = NotificationAgentInfo {
    id: "pushover",
    display_name: "Pushover",
    description: "Send push notifications via Pushover",
    supported_triggers: SUPPORTED_TRIGGERS,
    config_fields: CONFIG_FIELDS,
};

// ============================================================================
// Factory
// ============================================================================

pub struct PushoverFactory;

impl NotificationAgentFactory for PushoverFactory {
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
        let user_key = config
            .get("user_key")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let api_token = config
            .get("api_token")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let device = config
            .get("device")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);

        let priority = config
            .get("priority")
            .map(|v| match v {
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::String(s) => s.clone(),
                _ => "0".to_string(),
            })
            .unwrap_or_else(|| "0".to_string());

        let sound = config
            .get("sound")
            .and_then(|v| v.as_str())
            .unwrap_or("pushover")
            .to_string();

        Box::new(PushoverAgent {
            client,
            user_key,
            api_token,
            device,
            priority,
            sound,
        })
    }
}

// ============================================================================
// Agent
// ============================================================================

struct PushoverAgent {
    client: reqwest::Client,
    user_key: String,
    api_token: String,
    device: Option<String>,
    priority: String,
    sound: String,
}

#[async_trait]
impl NotificationAgent for PushoverAgent {
    fn info(&self) -> &'static NotificationAgentInfo {
        &INFO
    }

    async fn send(&self, payload: &NotificationPayload) -> Result<(), NotificationError> {
        if self.user_key.is_empty() {
            return Err(NotificationError::ConfigMissing("user_key".into()));
        }
        if self.api_token.is_empty() {
            return Err(NotificationError::ConfigMissing("api_token".into()));
        }

        // Build application/x-www-form-urlencoded body manually since reqwest
        // is compiled without the `form` feature in this project.
        let mut parts = vec![
            format!("token={}", urlencoding::encode(&self.api_token)),
            format!("user={}", urlencoding::encode(&self.user_key)),
            format!("message={}", urlencoding::encode(&payload.message)),
            format!("title={}", urlencoding::encode(&payload.title)),
            format!("priority={}", urlencoding::encode(&self.priority)),
            format!("sound={}", urlencoding::encode(&self.sound)),
        ];

        if let Some(ref device) = self.device {
            parts.push(format!("device={}", urlencoding::encode(device)));
        }

        if let Some(url) = payload.external_url() {
            parts.push(format!("url={}", urlencoding::encode(&url)));
        }

        let body = parts.join("&");

        let resp = self
            .client
            .post("https://api.pushover.net/1/messages.json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(NotificationError::ServiceError(format!(
                "Pushover returned {status}: {body}"
            )));
        }

        Ok(())
    }
}
