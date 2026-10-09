use async_trait::async_trait;

use super::types::{
    ConfigFieldType, NotificationAgentInfo, NotificationConfigField, NotificationTrigger,
};
use super::{NotificationAgent, NotificationAgentFactory, NotificationError, NotificationPayload};
use crate::google::service_account::{CHAT_BOT_SCOPE, ServiceAccountKey, TokenCache};
use crate::notifications::relevance::ChatRecipient;
use uuid::Uuid;

const CHAT_API_BASE: &str = "https://chat.googleapis.com/v1";

// ============================================================================
// Agent Info
// ============================================================================

static CONFIG_FIELDS: &[NotificationConfigField] = &[
    NotificationConfigField {
        key: "service_account_json",
        label: "Service Account Key (JSON)",
        field_type: ConfigFieldType::Secret,
        required: true,
        description: Some(
            "Paste the whole downloaded service-account key file, with the Chat API enabled on its project",
        ),
        help_url: Some(
            "https://developers.google.com/workspace/chat/authenticate-authorize-chat-app",
        ),
        default: None,
    },
    NotificationConfigField {
        key: "space",
        label: "Space",
        field_type: ConfigFieldType::String,
        required: true,
        description: Some(
            "The space to post into, e.g. spaces/AAAA1234 — take it from the space's URL",
        ),
        help_url: None,
        default: None,
    },
    NotificationConfigField {
        key: "project_number",
        label: "Project Number",
        field_type: ConfigFieldType::String,
        required: false,
        description: Some(
            "Only needed for the chat bot: the numeric Cloud project number Google signs inbound webhooks with. Notifications work without it",
        ),
        help_url: None,
        default: None,
    },
    NotificationConfigField {
        key: "thread_key",
        label: "Thread Key",
        field_type: ConfigFieldType::String,
        required: false,
        description: Some(
            "Optional. Keeps every notification in one thread instead of one thread each",
        ),
        help_url: None,
        default: None,
    },
];

static SUPPORTED_TRIGGERS: &[NotificationTrigger] = &[
    NotificationTrigger::OnGrab,
    NotificationTrigger::OnImport,
    NotificationTrigger::OnFailed,
    NotificationTrigger::OnMediaCorrupt,
];

pub static INFO: NotificationAgentInfo = NotificationAgentInfo {
    id: "google_chat",
    display_name: "Google Chat",
    description: "Post notifications into a Google Chat space as a Chat app",
    supported_triggers: SUPPORTED_TRIGGERS,
    config_fields: CONFIG_FIELDS,
};

// ============================================================================
// Factory
// ============================================================================

pub struct GoogleChatFactory;

impl NotificationAgentFactory for GoogleChatFactory {
    fn info(&self) -> &'static NotificationAgentInfo {
        &INFO
    }

    fn create(
        &self,
        _client: reqwest::Client,
        config: &serde_json::Value,
        _pool: &sqlx::PgPool,
        _agent_id: Uuid,
    ) -> Box<dyn NotificationAgent> {
        let service_account_json = config
            .get("service_account_json")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let space = config
            .get("space")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let thread_key = config
            .get("thread_key")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);

        Box::new(GoogleChatAgent {
            service_account_json,
            space,
            thread_key,
        })
    }
}

// ============================================================================
// Agent
// ============================================================================

struct GoogleChatAgent {
    service_account_json: String,
    space: String,
    thread_key: Option<String>,
}

#[async_trait]
impl NotificationAgent for GoogleChatAgent {
    fn info(&self) -> &'static NotificationAgentInfo {
        &INFO
    }

    async fn send(&self, payload: &NotificationPayload) -> Result<(), NotificationError> {
        if self.service_account_json.trim().is_empty() {
            return Err(NotificationError::ConfigMissing(
                "service_account_json".into(),
            ));
        }
        if self.space.trim().is_empty() {
            return Err(NotificationError::ConfigMissing("space".into()));
        }

        let key = ServiceAccountKey::from_json(&self.service_account_json)
            .map_err(|e| NotificationError::ConfigMissing(format!("service_account_json: {e}")))?;

        let body = serde_json::json!({ "text": format_message(payload) });

        post_message(&key, &self.space, self.thread_key.as_deref(), &body)
            .await
            .map(|_| ())
    }
}

fn format_message(payload: &NotificationPayload) -> String {
    let mut text = format!("*{}*\n{}", payload.title, payload.message);
    if let Some(url) = payload.external_url() {
        text.push('\n');
        text.push_str(&url);
    }
    text
}

// ============================================================================
// Outbound Chat REST client
// ============================================================================

/// Normalise whatever the admin typed into the `spaces/{id}` resource name the API wants.
pub fn normalise_space(space: &str) -> String {
    let trimmed = space.trim().trim_matches('/');
    match trimmed.strip_prefix("spaces/") {
        Some(id) => format!("spaces/{id}"),
        None => format!("spaces/{trimmed}"),
    }
}

/// Normalise a stored `chat_identities.external_id` into the `users/{id}` resource
/// name the Chat API wants. The inbound webhook already stores it in that form;
/// this is belt and braces for an identity an admin typed by hand.
pub fn normalise_user(user: &str) -> String {
    let trimmed = user.trim().trim_matches('/');
    match trimmed.strip_prefix("users/") {
        Some(id) => format!("users/{id}"),
        None => format!("users/{trimmed}"),
    }
}

/// The app's DM space with one person. There is no way to derive this from the
/// user id offline, and `chat_identities` has no space column, so it is looked
/// up per delivery; the token behind it is cached by `TokenCache`.
pub async fn find_direct_message_space(
    key: &ServiceAccountKey,
    user: &str,
) -> Result<String, NotificationError> {
    let token = access_token(key).await?;
    let url = format!(
        "{CHAT_API_BASE}/spaces:findDirectMessage?name={}",
        urlencoding::encode(&normalise_user(user))
    );

    let response = crate::http::new_client()
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await?;

    let text = check_status(response, "spaces.findDirectMessage").await?;

    #[derive(serde::Deserialize)]
    struct FoundSpace {
        name: String,
    }

    let found: FoundSpace = serde_json::from_str(&text).map_err(|e| {
        NotificationError::Parse(format!("Google Chat findDirectMessage response: {e}"))
    })?;
    Ok(found.name)
}

/// One person this notification can actually reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatTarget {
    pub user_id: uuid::Uuid,
    /// `users/{id}` — the Chat resource name the DM lookup takes.
    pub user_ref: String,
}

/// Split relevance-scoped recipients into the ones we can address and a count of
/// the ones we cannot. An identity bound by email alone has no Chat user id, so
/// there is no DM space to find; that is a pairing gap, not an error worth
/// retrying the whole job over.
pub fn delivery_targets(recipients: &[ChatRecipient]) -> (Vec<ChatTarget>, usize) {
    let mut targets: Vec<ChatTarget> = Vec::new();
    let mut unreachable = 0usize;

    for recipient in recipients {
        match recipient
            .identity
            .external_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            Some(external_id) => {
                let user_ref = normalise_user(external_id);
                if !targets.iter().any(|t| t.user_ref == user_ref) {
                    targets.push(ChatTarget {
                        user_id: recipient.user_id,
                        user_ref,
                    });
                }
            }
            None => unreachable += 1,
        }
    }

    (targets, unreachable)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScopedDelivery {
    pub delivered: usize,
    pub unreachable: usize,
    pub failed: usize,
}

/// Deliver one notification to each interested person's own DM space.
///
/// Never touches the configured space: reaching this function at all means
/// identities are bound, and the whole point is that the shared space stops
/// being the destination once anyone has paired.
pub async fn send_scoped(
    config: &serde_json::Value,
    recipients: &[ChatRecipient],
    payload: &NotificationPayload,
) -> Result<ScopedDelivery, NotificationError> {
    let (targets, unreachable) = delivery_targets(recipients);
    let mut outcome = ScopedDelivery {
        unreachable,
        ..Default::default()
    };
    if targets.is_empty() {
        return Ok(outcome);
    }

    let service_account_json = config
        .get("service_account_json")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if service_account_json.trim().is_empty() {
        return Err(NotificationError::ConfigMissing(
            "service_account_json".into(),
        ));
    }
    let key = ServiceAccountKey::from_json(service_account_json)
        .map_err(|e| NotificationError::ConfigMissing(format!("service_account_json: {e}")))?;

    let body = serde_json::json!({ "text": format_message(payload) });

    for target in &targets {
        // One person being unreachable must not silence the rest, so a failure
        // here is counted and logged rather than returned.
        match find_direct_message_space(&key, &target.user_ref).await {
            Ok(space) => match post_message(&key, &space, None, &body).await {
                Ok(_) => outcome.delivered += 1,
                Err(e) => {
                    tracing::warn!(user_id = %target.user_id, error = %e, "Google Chat scoped send failed");
                    outcome.failed += 1;
                }
            },
            Err(e) => {
                tracing::warn!(user_id = %target.user_id, error = %e, "No Google Chat DM space for user");
                outcome.failed += 1;
            }
        }
    }

    Ok(outcome)
}

fn create_url(space: &str, thread: Option<&str>) -> String {
    let mut url = format!("{CHAT_API_BASE}/{}/messages", normalise_space(space));
    if let Some(thread) = thread.map(str::trim).filter(|t| !t.is_empty()) {
        // A `spaces/…/threads/…` resource name is `thread.name` and belongs in the body;
        // sending it as a threadKey too is a 400. Anything else is a client-chosen key.
        if is_thread_resource_name(thread) {
            url.push_str("?messageReplyOption=REPLY_MESSAGE_FALLBACK_TO_NEW_THREAD");
        } else {
            url.push_str(&format!(
                "?threadKey={}&messageReplyOption=REPLY_MESSAGE_FALLBACK_TO_NEW_THREAD",
                urlencoding::encode(thread)
            ));
        }
    }
    url
}

fn is_thread_resource_name(thread: &str) -> bool {
    thread.contains("/threads/")
}

fn create_body(body: &serde_json::Value, thread: Option<&str>) -> serde_json::Value {
    let Some(thread) = thread
        .map(str::trim)
        .filter(|t| !t.is_empty() && is_thread_resource_name(t))
    else {
        return body.clone();
    };
    let mut body = body.clone();
    if let Some(obj) = body.as_object_mut() {
        obj.insert("thread".to_string(), serde_json::json!({ "name": thread }));
    }
    body
}

fn patch_url(message_name: &str) -> String {
    format!(
        "{CHAT_API_BASE}/{}?updateMask=text,cardsV2",
        message_name.trim().trim_matches('/')
    )
}

/// Post a message into a space. Returns the message resource name
/// (`spaces/{space}/messages/{message}`), which `patch_message` takes.
pub async fn post_message(
    key: &ServiceAccountKey,
    space: &str,
    thread: Option<&str>,
    body: &serde_json::Value,
) -> Result<String, NotificationError> {
    let token = access_token(key).await?;
    let url = create_url(space, thread);
    let body = create_body(body, thread);

    let response = crate::http::new_client()
        .post(&url)
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await?;

    let text = check_status(response, "spaces.messages.create").await?;

    #[derive(serde::Deserialize)]
    struct CreatedMessage {
        name: String,
    }

    let created: CreatedMessage = serde_json::from_str(&text)
        .map_err(|e| NotificationError::Parse(format!("Google Chat create response: {e}")))?;
    Ok(created.name)
}

/// Edit a message in place. `message_name` is the value `post_message` returned.
pub async fn patch_message(
    key: &ServiceAccountKey,
    message_name: &str,
    body: &serde_json::Value,
) -> Result<(), NotificationError> {
    let token = access_token(key).await?;
    let url = patch_url(message_name);

    let response = crate::http::new_client()
        .patch(&url)
        .bearer_auth(&token)
        .json(body)
        .send()
        .await?;

    check_status(response, "spaces.messages.patch")
        .await
        .map(|_| ())
}

async fn access_token(key: &ServiceAccountKey) -> Result<String, NotificationError> {
    TokenCache::global()
        .access_token(key, &[CHAT_BOT_SCOPE])
        .await
        .map_err(|e| NotificationError::ServiceError(format!("Google Chat auth failed: {e}")))
}

async fn check_status(
    response: reqwest::Response,
    what: &str,
) -> Result<String, NotificationError> {
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(NotificationError::ServiceError(format!(
            "Google Chat {what} returned {}: {}",
            status.as_u16(),
            text
        )));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::types::NotificationTrigger;

    fn payload(url: Option<&str>) -> NotificationPayload {
        with_base(url, None)
    }

    fn with_base(url: Option<&str>, public_url: Option<&str>) -> NotificationPayload {
        NotificationPayload {
            trigger: NotificationTrigger::OnImport,
            title: "Import Complete".to_string(),
            message: "someShow 24x07 added".to_string(),
            entity_type: Some("episode".to_string()),
            entity_id: None,
            entity_name: Some("someShow 24x07".to_string()),
            url: url.map(String::from),
            public_url: public_url.map(String::from),
            show_id: None,
            show_name: None,
            season_number: None,
            show_year: None,
            upgraded: false,
            details: None,
        }
    }

    /// A bare "/tv/123" in a phone notification is not openable, so without a
    /// configured base the link is omitted rather than shipped broken.
    #[test]
    fn a_relative_path_is_omitted_until_a_public_url_exists() {
        let text = format_message(&with_base(Some("/tv/123"), None));
        assert!(!text.contains("/tv/123"), "got: {text}");
        assert!(text.contains("someShow 24x07 added"));
    }

    #[test]
    fn a_public_url_turns_the_path_into_something_tappable() {
        let text = format_message(&with_base(Some("/tv/123"), Some("https://kob.example.com")));
        assert!(
            text.contains("https://kob.example.com/tv/123"),
            "got: {text}"
        );
    }

    #[test]
    fn exactly_one_slash_joins_the_base_and_the_path() {
        let p = with_base(Some("/tv/123"), Some("https://kob.example.com/"));
        assert_eq!(
            p.external_url().as_deref(),
            Some("https://kob.example.com/tv/123")
        );
        assert_eq!(
            with_base(None, Some("https://k.example")).external_url(),
            None
        );
        assert_eq!(with_base(Some("/tv/1"), Some("")).external_url(), None);
        // An already-absolute url needs no base and must not be rewritten.
        assert_eq!(
            with_base(Some("https://kob.example/tv/1"), None)
                .external_url()
                .as_deref(),
            Some("https://kob.example/tv/1")
        );
    }

    #[test]
    fn normalises_whatever_the_admin_typed() {
        assert_eq!(normalise_space("spaces/AAAA1234"), "spaces/AAAA1234");
        assert_eq!(normalise_space("  AAAA1234 "), "spaces/AAAA1234");
        assert_eq!(normalise_space("/spaces/AAAA1234/"), "spaces/AAAA1234");
    }

    #[test]
    fn builds_the_create_url() {
        assert_eq!(
            create_url("spaces/AAAA1234", None),
            "https://chat.googleapis.com/v1/spaces/AAAA1234/messages"
        );
        assert_eq!(
            create_url("AAAA1234", Some(" kob bler ")),
            "https://chat.googleapis.com/v1/spaces/AAAA1234/messages\
?threadKey=kob%20bler&messageReplyOption=REPLY_MESSAGE_FALLBACK_TO_NEW_THREAD"
        );
        assert_eq!(
            create_url("spaces/AAAA1234", Some("   ")),
            "https://chat.googleapis.com/v1/spaces/AAAA1234/messages"
        );
    }

    #[test]
    fn an_existing_thread_rides_in_the_body_not_the_query() {
        let thread = "spaces/AAAA1234/threads/TTTT";
        assert_eq!(
            create_url("spaces/AAAA1234", Some(thread)),
            "https://chat.googleapis.com/v1/spaces/AAAA1234/messages\
?messageReplyOption=REPLY_MESSAGE_FALLBACK_TO_NEW_THREAD",
            "threadKey and thread.name together are a 400"
        );
        let body = create_body(&serde_json::json!({ "text": "hi" }), Some(thread));
        assert_eq!(body["thread"]["name"], thread);
        assert_eq!(body["text"], "hi");

        let keyed = create_body(&serde_json::json!({ "text": "hi" }), Some("kobbler"));
        assert!(keyed.get("thread").is_none());
    }

    #[test]
    fn builds_the_patch_url() {
        assert_eq!(
            patch_url("spaces/AAAA1234/messages/BBBB5678.BBBB5678"),
            "https://chat.googleapis.com/v1/spaces/AAAA1234/messages/BBBB5678.BBBB5678\
?updateMask=text,cardsV2"
        );
    }

    #[test]
    fn formats_title_message_and_url() {
        assert_eq!(
            format_message(&payload(Some("https://kob.example/tv/1"))),
            "*Import Complete*\nsomeShow 24x07 added\nhttps://kob.example/tv/1"
        );
        assert_eq!(
            format_message(&payload(None)),
            "*Import Complete*\nsomeShow 24x07 added"
        );
    }

    fn agent(config: serde_json::Value) -> Box<dyn NotificationAgent> {
        let pool = sqlx::Pool::<sqlx::Postgres>::connect_lazy("postgres://localhost/dummy")
            .expect("lazy pool");
        GoogleChatFactory.create(crate::http::new_client(), &config, &pool, uuid::Uuid::nil())
    }

    #[tokio::test]
    async fn refuses_to_send_without_a_key() {
        let err = agent(serde_json::json!({ "space": "spaces/AAAA1234" }))
            .send(&payload(None))
            .await
            .unwrap_err();
        assert!(matches!(err, NotificationError::ConfigMissing(_)));
        assert!(err.to_string().contains("service_account_json"));
    }

    #[tokio::test]
    async fn refuses_to_send_without_a_space() {
        let config = serde_json::json!({
            "service_account_json": crate::google::test_keys::service_account_json(
                "bot@kobbler-test.iam.gserviceaccount.com",
                "https://oauth2.googleapis.com/token",
            ),
        });
        let err = agent(config).send(&payload(None)).await.unwrap_err();
        assert!(matches!(err, NotificationError::ConfigMissing(_)));
        assert!(err.to_string().contains("space"));
    }

    #[tokio::test]
    async fn reports_an_unusable_key_file_before_any_network_call() {
        let config = serde_json::json!({
            "service_account_json": "{ not json }",
            "space": "spaces/AAAA1234",
        });
        let err = agent(config).send(&payload(None)).await.unwrap_err();
        assert!(err.to_string().contains("service_account_json"));
    }

    #[test]
    fn advertises_itself_to_the_settings_ui() {
        let info = GoogleChatFactory.info();
        assert_eq!(info.id, "google_chat");
        let keys: Vec<&str> = info.config_fields.iter().map(|f| f.key).collect();
        assert_eq!(
            keys,
            [
                "service_account_json",
                "space",
                "project_number",
                "thread_key"
            ]
        );
        assert!(
            info.config_fields
                .iter()
                .any(|f| f.key == "service_account_json"
                    && f.required
                    && f.field_type == ConfigFieldType::Secret)
        );
    }
}
