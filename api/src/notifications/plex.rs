use async_trait::async_trait;

use super::types::{
    ConfigFieldType, NotificationAgentInfo, NotificationConfigField, NotificationTrigger,
};
use super::{NotificationAgent, NotificationAgentFactory, NotificationError, NotificationPayload};
use uuid::Uuid;

// ============================================================================
// Agent Info
// ============================================================================

static SUPPORTED_TRIGGERS: &[NotificationTrigger] = &[NotificationTrigger::OnImport];

static CONFIG_FIELDS: &[NotificationConfigField] = &[
    NotificationConfigField {
        key: "base_url",
        label: "Plex Server URL",
        field_type: ConfigFieldType::Url,
        required: true,
        description: Some("Plex server URL, e.g. http://192.168.1.100:32400"),
        help_url: None,
        default: None,
    },
    NotificationConfigField {
        key: "token",
        label: "Plex Token",
        field_type: ConfigFieldType::Secret,
        required: true,
        description: Some("X-Plex-Token for authentication"),
        help_url: Some(
            "https://support.plex.tv/articles/204059436-finding-an-authentication-token-x-plex-token/",
        ),
        default: None,
    },
    NotificationConfigField {
        key: "library_ids",
        label: "Library Section IDs",
        field_type: ConfigFieldType::String,
        required: false,
        description: Some("Comma-separated library section IDs to scan. Leave empty to scan all."),
        help_url: None,
        default: None,
    },
];

pub static INFO: NotificationAgentInfo = NotificationAgentInfo {
    id: "plex",
    display_name: "Plex",
    description: "Trigger Plex library scan when new content is imported",
    supported_triggers: SUPPORTED_TRIGGERS,
    config_fields: CONFIG_FIELDS,
};

// ============================================================================
// Factory
// ============================================================================

pub struct PlexFactory;

impl NotificationAgentFactory for PlexFactory {
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
        let base_url = config
            .get("base_url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();

        let token = config
            .get("token")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let library_ids: Vec<String> = config
            .get("library_ids")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.split(',').map(|id| id.trim().to_string()).collect())
            .unwrap_or_default();

        Box::new(PlexAgent {
            client,
            base_url,
            token,
            library_ids,
        })
    }
}

// ============================================================================
// Agent
// ============================================================================

struct PlexAgent {
    client: reqwest::Client,
    base_url: String,
    token: String,
    library_ids: Vec<String>,
}

#[async_trait]
impl NotificationAgent for PlexAgent {
    fn info(&self) -> &'static NotificationAgentInfo {
        &INFO
    }

    async fn send(&self, _payload: &NotificationPayload) -> Result<(), NotificationError> {
        if self.base_url.is_empty() {
            return Err(NotificationError::ConfigMissing("base_url".into()));
        }
        if self.token.is_empty() {
            return Err(NotificationError::ConfigMissing("token".into()));
        }

        if self.library_ids.is_empty() {
            // Scan all libraries
            let url = format!(
                "{}/library/sections/all/refresh?X-Plex-Token={}",
                self.base_url, self.token
            );
            let resp = self.client.get(&url).send().await?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(NotificationError::ServiceError(format!(
                    "Plex scan-all returned {status}: {body}"
                )));
            }
        } else {
            // Scan specific library sections
            for id in &self.library_ids {
                let url = format!(
                    "{}/library/sections/{}/refresh?X-Plex-Token={}",
                    self.base_url, id, self.token
                );
                let resp = self.client.put(&url).send().await?;
                if !resp.status().is_success() {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    tracing::warn!("Plex library section {id} scan returned {status}: {body}");
                }
            }
        }

        Ok(())
    }
}
