//! Discord as a notification agent — the unaddressed half.
//!
//! This is the firehose: grabs, imports and failures posted into one channel, for anyone
//! who wants to watch the library fill up. It is deliberately *not* the same thing as the
//! assistant answering someone. A download somebody asked for in a DM is followed up in
//! that DM by [`crate::chatbot::watches`], which knows who asked.
//!
//! It shares its stored config with [`crate::chatbot::transport::discord`] — one row,
//! one bot token, one credential to rotate. `channel_id` is the only field the transport
//! does not need, and `bot_token` is the only field this does not need on its own.
//!
//! An import is announced once, quietly kept current after that:
//! - **One message per item.** The item's name (linked), its resolution, an embed with
//!   the episode or movie's own details, and the release name in small grey text. The
//!   message id is kept in `discord_message_map`.
//! - **Replacements edit, never re-announce.** A file that replaces one already in the
//!   library (an upgrade, a repack, a re-download) edits the item's message in place —
//!   no new post, no bump. Only an item that had *no* file (never had one, or it was
//!   deleted as a bad release) is announced.
//! - **Per-show threads.** A show's releases are grouped into one public thread so its
//!   episodes stop scrolling past the announcements channel. The thread id is cached in
//!   `discord_show_threads`; creation falls back to a plain channel post when the bot
//!   lacks Create Public Threads.
//! - **A channel card per show.** Threads are easy to miss, so each show also has one
//!   message in the channel carrying a copy of its latest item and a link to its
//!   thread. The card is deleted and re-posted on each announcement — Discord cannot
//!   bump a message — so an active show sits at the bottom of the channel without
//!   adding to it. A replacement edits the card in place when it is showing that item.

use async_trait::async_trait;

use super::types::{
    ConfigFieldType, NotificationAgentInfo, NotificationConfigField, NotificationTrigger,
};
use super::{NotificationAgent, NotificationAgentFactory, NotificationError, NotificationPayload};
use crate::chatbot::transport::discord::API_BASE;
use crate::repositories::discord;
use sqlx::PgPool;
use uuid::Uuid;

/// Serialises show-card bumps across every Discord agent (see `bump_card`).
static CARD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

static CONFIG_FIELDS: &[NotificationConfigField] = &[
    NotificationConfigField {
        key: "bot_token",
        label: "Bot Token",
        field_type: ConfigFieldType::Secret,
        required: true,
        description: Some(
            "From the Discord developer portal, Bot → Token. The same token the chat assistant uses",
        ),
        help_url: Some("https://discord.com/developers/applications"),
        default: None,
    },
    NotificationConfigField {
        key: "channel_id",
        label: "Announcements Channel ID",
        field_type: ConfigFieldType::String,
        required: true,
        description: Some(
            "Where these go. Turn on Developer Mode in Discord, then right-click the channel and Copy Channel ID",
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
    NotificationTrigger::OnServerHealth,
];

pub static INFO: NotificationAgentInfo = NotificationAgentInfo {
    id: "discord",
    display_name: "Discord",
    description: "Post notifications into a Discord channel",
    supported_triggers: SUPPORTED_TRIGGERS,
    config_fields: CONFIG_FIELDS,
};

pub struct DiscordFactory;

impl NotificationAgentFactory for DiscordFactory {
    fn info(&self) -> &'static NotificationAgentInfo {
        &INFO
    }

    fn create(
        &self,
        client: reqwest::Client,
        config: &serde_json::Value,
        pool: &PgPool,
        agent_id: Uuid,
    ) -> Box<dyn NotificationAgent> {
        let field = |key: &str| {
            config
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or_default()
                .to_string()
        };
        Box::new(DiscordAgent {
            client,
            token: field("bot_token"),
            channel_id: field("channel_id"),
            pool: pool.clone(),
            agent_id,
        })
    }
}

struct DiscordAgent {
    client: reqwest::Client,
    token: String,
    channel_id: String,
    pool: PgPool,
    agent_id: Uuid,
}

#[async_trait]
impl NotificationAgent for DiscordAgent {
    fn info(&self) -> &'static NotificationAgentInfo {
        &INFO
    }

    async fn send(&self, payload: &NotificationPayload) -> Result<(), NotificationError> {
        if self.token.is_empty() {
            return Err(NotificationError::ConfigMissing("bot_token".into()));
        }
        if self.channel_id.is_empty() {
            return Err(NotificationError::ConfigMissing("channel_id".into()));
        }

        // An import is a release: one message per item, kept current. Anything else
        // (a failure, a corruption report, a test) is a one-off post.
        match payload.entity_id {
            Some(entity_id) if payload.trigger == NotificationTrigger::OnImport => {
                self.send_release(payload, entity_id).await
            }
            _ => {
                let embed = render_embed(payload, today());
                self.post_content(&self.channel_id, &render(payload), embed)
                    .await?;
                Ok(())
            }
        }
    }
}

impl DiscordAgent {
    /// Announce an item, or — when it replaced a file already in the library — bring
    /// its existing message up to date without a sound.
    async fn send_release(
        &self,
        payload: &NotificationPayload,
        entity_id: Uuid,
    ) -> Result<(), NotificationError> {
        let content = render_release(payload);
        let embed = render_embed(payload, today());

        if payload.upgraded {
            self.update_quietly(payload, entity_id, &content, embed)
                .await;
            return Ok(());
        }

        let (message_id, channel_id) = match payload.show_id {
            Some(show_id) => {
                let name = thread_name(payload);
                match self
                    .post_into_show_thread(show_id, &name, &content, embed.clone())
                    .await
                {
                    Some((message_id, thread_id)) => {
                        self.bump_card(show_id, entity_id, &name, &thread_id, &content, embed)
                            .await;
                        (message_id, thread_id)
                    }
                    None => self.post_to_channel(&content, embed).await?,
                }
            }
            None => self.post_to_channel(&content, embed).await?,
        };
        if let Err(e) = discord::upsert_message(
            &self.pool,
            self.agent_id,
            entity_id,
            &message_id,
            &channel_id,
        )
        .await
        {
            tracing::warn!(entity_id = %entity_id, error = %e, "Discord: failed to record message id");
        }
        Ok(())
    }

    /// A replacement file: edit the item's message, and the show's card if it is
    /// showing this item. Never posts — an item announced before this agent kept a
    /// message map, or whose message is gone, stays unannounced rather than loud.
    async fn update_quietly(
        &self,
        payload: &NotificationPayload,
        entity_id: Uuid,
        content: &str,
        embed: Option<serde_json::Value>,
    ) {
        match discord::get_message(&self.pool, self.agent_id, entity_id).await {
            Ok(Some((message_id, channel_id))) => {
                if let Err(e) = self
                    .edit_message(&channel_id, &message_id, content, embed.clone())
                    .await
                {
                    tracing::info!(entity_id = %entity_id, error = %e, "Discord: replacement could not edit the item's message");
                }
                // The card copies the message from the thread; keep that copy current.
                if let Some(show_id) = payload.show_id
                    && channel_id != self.channel_id
                    && let Ok(Some((card_id, Some(card_entity)))) =
                        discord::get_card(&self.pool, self.agent_id, show_id).await
                    && card_entity == entity_id
                {
                    let card = render_card(&thread_name(payload), &channel_id, content);
                    if let Err(e) = self
                        .edit_message(&self.channel_id, &card_id, &card, embed)
                        .await
                    {
                        tracing::info!(show_id = %show_id, error = %e, "Discord: replacement could not edit the show card");
                    }
                }
            }
            Ok(None) => tracing::debug!(
                entity_id = %entity_id,
                "Discord: replacement for an item never announced here, staying quiet"
            ),
            Err(e) => {
                tracing::warn!(entity_id = %entity_id, error = %e, "Discord: message lookup failed")
            }
        }
    }

    /// Post into the announcements channel. Returns `(message_id, channel_id)`; a
    /// forum channel puts the post in a thread of its own, which is where it lives.
    async fn post_to_channel(
        &self,
        content: &str,
        embed: Option<serde_json::Value>,
    ) -> Result<(String, String), NotificationError> {
        let (message_id, thread_id) = self.post_content(&self.channel_id, content, embed).await?;
        Ok((
            message_id,
            thread_id.unwrap_or_else(|| self.channel_id.clone()),
        ))
    }

    /// Post into the show's thread, creating it on first use. Returns
    /// `(message_id, thread_id)`. Text channels ignore `thread_name` on message
    /// creation (that parameter is forum-only), so the thread is created explicitly. A
    /// cached thread that has since auto-archived (4 weeks of quiet) or been deleted is
    /// replaced by a fresh one. `None` means threads are unavailable and the caller
    /// posts to the channel instead.
    async fn post_into_show_thread(
        &self,
        show_id: Uuid,
        thread_name: &str,
        content: &str,
        embed: Option<serde_json::Value>,
    ) -> Option<(String, String)> {
        let cached = match discord::get_thread(&self.pool, self.agent_id, show_id).await {
            Ok(cached) => cached,
            Err(e) => {
                tracing::warn!(
                    show_id = %show_id,
                    error = %e,
                    "Discord: thread lookup failed, falling back to plain channel post"
                );
                return None;
            }
        };

        if let Some(thread_id) = cached {
            match self.post_content(&thread_id, content, embed.clone()).await {
                Ok((message_id, _)) => return Some((message_id, thread_id)),
                Err(e) => tracing::info!(
                    show_id = %show_id,
                    error = %e,
                    "Discord: cached thread rejected the post, starting a fresh one"
                ),
            }
        }

        let thread_id = match self.create_thread(&self.channel_id, thread_name).await {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(
                    show_id = %show_id,
                    error = %e,
                    "Discord: thread creation failed, falling back to plain channel post"
                );
                return None;
            }
        };
        if let Err(e) = discord::upsert_thread(&self.pool, self.agent_id, show_id, &thread_id).await
        {
            tracing::warn!(
                show_id = %show_id,
                error = %e,
                "Discord: failed to cache new thread id"
            );
        }
        match self.post_content(&thread_id, content, embed).await {
            Ok((message_id, _)) => Some((message_id, thread_id)),
            Err(e) => {
                tracing::warn!(
                    show_id = %show_id,
                    error = %e,
                    "Discord: new thread rejected the post, falling back to plain channel post"
                );
                None
            }
        }
    }

    /// Replace the show's channel card with a fresh one at the bottom of the channel,
    /// carrying a copy of the item's message. The new card is posted before the old one
    /// is deleted, so a failure leaves a stale card rather than none. Best-effort: the
    /// item itself is already in the thread.
    async fn bump_card(
        &self,
        show_id: Uuid,
        entity_id: Uuid,
        thread_name: &str,
        thread_id: &str,
        item: &str,
        embed: Option<serde_json::Value>,
    ) {
        // Two releases landing together must not both delete the same old card and
        // leave two new ones behind.
        let _guard = CARD_LOCK.lock().await;

        let old = discord::get_card(&self.pool, self.agent_id, show_id)
            .await
            .ok()
            .flatten();
        let content = render_card(thread_name, thread_id, item);
        let new_id = match self.post_content(&self.channel_id, &content, embed).await {
            Ok((id, _)) => id,
            Err(e) => {
                tracing::warn!(show_id = %show_id, error = %e, "Discord: failed to post show card");
                return;
            }
        };
        if let Err(e) =
            discord::set_card(&self.pool, self.agent_id, show_id, &new_id, entity_id).await
        {
            tracing::warn!(show_id = %show_id, error = %e, "Discord: failed to record show card");
        }
        if let Some((old, _)) = old
            && let Err(e) = self.delete_message(&self.channel_id, &old).await
        {
            tracing::warn!(show_id = %show_id, error = %e, "Discord: failed to delete old show card");
        }
    }

    /// Delete a message. One that is already gone counts as deleted.
    async fn delete_message(
        &self,
        channel_id: &str,
        message_id: &str,
    ) -> Result<(), NotificationError> {
        let url = format!("{API_BASE}/channels/{channel_id}/messages/{message_id}");
        let response = self
            .client
            .delete(&url)
            .header("Authorization", format!("Bot {}", self.token))
            .send()
            .await?;
        let status = response.status();
        if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        Err(service_error(response).await)
    }

    /// Create a public thread in the channel and return its id.
    async fn create_thread(
        &self,
        channel_id: &str,
        name: &str,
    ) -> Result<String, NotificationError> {
        let url = format!("{API_BASE}/channels/{channel_id}/threads");
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bot {}", self.token))
            .json(&serde_json::json!({
                "name": name,
                // Without a starter message a thread defaults to PRIVATE (12), which
                // nobody but the bot and moderators can see. 11 is public.
                "type": 11,
                // 4 weeks: an active show's thread keeps getting boosted; a quiet
                // one archives, and the next release starts a fresh thread.
                "auto_archive_duration": 10080,
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(service_error(response).await);
        }

        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|e| NotificationError::Parse(e.to_string()))?;
        value
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| NotificationError::Parse("discord thread had no id".into()))
            .map(|s| s.to_string())
    }

    /// Replace a message's content and embed. Edits never notify anyone.
    async fn edit_message(
        &self,
        channel_id: &str,
        message_id: &str,
        content: &str,
        embed: Option<serde_json::Value>,
    ) -> Result<(), NotificationError> {
        let url = format!("{API_BASE}/channels/{channel_id}/messages/{message_id}");
        let response = self
            .client
            .patch(&url)
            .header("Authorization", format!("Bot {}", self.token))
            .json(&message_body(content, embed))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(service_error(response).await);
        }
        Ok(())
    }

    /// POST a message. Returns `(message_id, thread_id)`; `thread_id` is `Some` only
    /// in forum channels, where posting creates a thread automatically.
    async fn post_content(
        &self,
        channel_id: &str,
        content: &str,
        embed: Option<serde_json::Value>,
    ) -> Result<(String, Option<String>), NotificationError> {
        let url = format!("{API_BASE}/channels/{channel_id}/messages");
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bot {}", self.token))
            .json(&message_body(content, embed))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(service_error(response).await);
        }

        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|e| NotificationError::Parse(e.to_string()))?;
        let message_id = value
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| NotificationError::Parse("discord message had no id".into()))?
            .to_string();
        let thread_id = value
            .get("thread_id")
            .and_then(|v| v.as_str())
            .map(String::from);
        Ok((message_id, thread_id))
    }
}

/// The JSON body for posting or editing a message. `embeds` is always sent so an
/// edit that no longer has an embed clears the old one.
fn message_body(content: &str, embed: Option<serde_json::Value>) -> serde_json::Value {
    serde_json::json!({
        "content": content,
        "embeds": embed.into_iter().collect::<Vec<_>>(),
        // A notification naming a film called "@everyone" must not ping the server.
        "allowed_mentions": { "parse": [] },
    })
}

async fn service_error(response: reqwest::Response) -> NotificationError {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let snippet: String = text.chars().take(300).collect();
    NotificationError::ServiceError(format!("discord {status}: {snippet}"))
}

fn today() -> time::Date {
    time::OffsetDateTime::now_utc()
        .to_offset(super::prefs::server_local_offset())
        .date()
}

/// Thread name: "Show (YYYY)" — the year disambiguates remakes and reboots.
fn thread_name(payload: &NotificationPayload) -> String {
    let show_name = payload
        .show_name
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Releases".to_string());
    match payload.show_year {
        Some(year) => format!("{show_name} ({year})"),
        None => show_name,
    }
}

/// An item's message: its name (linked) and resolution, then the release name in
/// Discord's small grey subtext — there for whoever wants to check the file, easy
/// for everyone else to skip. The embed carries the rest.
fn render_release(payload: &NotificationPayload) -> String {
    let name = payload
        .entity_name
        .as_deref()
        .filter(|n| !n.is_empty())
        .unwrap_or(payload.title.as_str());
    let mut out = match payload.external_url() {
        Some(url) => format!("**{}**", masked_link(name, &url)),
        None => format!("**{name}**"),
    };
    let file = payload.details.as_ref().and_then(|d| d.file.as_ref());
    if let Some(quality) = file.and_then(|f| f.quality()) {
        out.push_str(&format!(" · {quality}"));
    }
    if let Some(file) = file {
        // Backticks stop a release name's underscores turning into italics.
        out.push_str(&format!("\n-# `{}`", file.release_name.replace('`', "'")));
    }
    clamp(out)
}

/// Discord rejects an over-long message rather than trimming it, and a notification
/// is not worth chunking into a thread of its own.
fn clamp(out: String) -> String {
    match out.chars().count() > 2000 {
        true => out.chars().take(1997).collect::<String>() + "...",
        false => out,
    }
}

/// The show's channel card: its name and a link to its thread (`<#id>` renders as a
/// clickable thread link, and pings nobody), then a copy of the item's message.
fn render_card(thread_name: &str, thread_id: &str, item: &str) -> String {
    let name: String = thread_name.chars().take(200).collect();
    clamp(format!("📺 **{name}** · <#{thread_id}>\n{item}"))
}

/// Discord renders markdown, so the title is bolded and the item's name, where the
/// message mentions it, is a link — not a bare URL dumped on its own line.
///
/// The link is [`NotificationPayload::external_url`], never `url` — a relative path is
/// what web push wants and is unopenable from a phone.
fn render(payload: &NotificationPayload) -> String {
    let mut out = format!("**{}**", payload.title);
    let url = payload.external_url();
    let name = payload.entity_name.as_deref().filter(|n| !n.is_empty());
    let mut linked = false;
    if !payload.message.is_empty() {
        out.push('\n');
        match (&url, name) {
            (Some(url), Some(name)) if payload.message.contains(name) => {
                out.push_str(&payload.message.replacen(name, &masked_link(name, url), 1));
                linked = true;
            }
            _ => out.push_str(&payload.message),
        }
    }
    if let Some(url) = &url
        && !linked
    {
        out.push('\n');
        out.push_str(&masked_link(name.unwrap_or("Open in Kobbler"), url));
    }
    clamp(out)
}

/// `[text](<url>)` — the angle brackets stop Discord unfurling a preview of our own
/// page, and brackets in the text would end the link early.
fn masked_link(text: &str, url: &str) -> String {
    let text = text.replace('[', "(").replace(']', ")");
    format!("[{text}](<{url}>)")
}

const AIR_DATE_FORMAT: &[time::format_description::BorrowedFormatItem<'_>] =
    time::macros::format_description!("[day padding:none] [month repr:short] [year]");

/// The embed under a notification: the episode's (or movie's) own title, its air
/// date, a summary and the TMDB artwork. `None` when there is nothing to add beyond
/// the message line — a season pack, a book, a test notification.
fn render_embed(payload: &NotificationPayload, today: time::Date) -> Option<serde_json::Value> {
    let details = payload.details.as_ref()?;
    let mut embed = serde_json::Map::new();

    if let Some(title) = &details.title {
        embed.insert("title".into(), truncate(title, 256).into());
        if let Some(url) = payload.external_url() {
            embed.insert("url".into(), url.into());
        }
    }
    if let Some(summary) = &details.summary {
        embed.insert("description".into(), truncate(summary, 400).into());
    }
    if let Some(date) = details.air_date {
        let verb = match (payload.entity_type.as_deref(), date > today) {
            (Some("movie"), true) => "Releases",
            (Some("movie"), false) => "Released",
            (_, true) => "Airs",
            (_, false) => "Aired",
        };
        embed.insert(
            "footer".into(),
            serde_json::json!({ "text": format!("{verb} {}", date.format(AIR_DATE_FORMAT).unwrap_or_default()) }),
        );
    }
    if let Some(image) = &details.image_url {
        embed.insert("thumbnail".into(), serde_json::json!({ "url": image }));
    }
    if embed.is_empty() {
        return None;
    }
    let color = match payload.trigger {
        NotificationTrigger::OnImport => 0x2ecc71,
        NotificationTrigger::OnFailed
        | NotificationTrigger::OnMediaCorrupt
        | NotificationTrigger::OnServerHealth => 0xe74c3c,
        _ => 0x3498db,
    };
    embed.insert("color".into(), color.into());
    Some(serde_json::Value::Object(embed))
}

/// Cut to `max` characters on a word boundary, with an ellipsis when anything went.
fn truncate(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max - 1).collect();
    let cut = match cut.rfind(char::is_whitespace) {
        Some(i) if i > max / 2 => &cut[..i],
        _ => cut.as_str(),
    };
    format!(
        "{}…",
        cut.trim_end_matches(|c: char| c.is_whitespace() || c == ',' || c == '.')
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::{FileDetails, ItemDetails};
    use uuid::Uuid;

    fn payload(url: Option<&str>, public: Option<&str>) -> NotificationPayload {
        NotificationPayload {
            trigger: NotificationTrigger::OnImport,
            title: "Added someFilm".to_string(),
            message: "1080p WEB-DL".to_string(),
            entity_type: Some("movie".to_string()),
            entity_id: Some(Uuid::nil()),
            entity_name: Some("someFilm".to_string()),
            url: url.map(String::from),
            public_url: public.map(String::from),
            show_id: None,
            show_name: None,
            season_number: None,
            show_year: None,
            upgraded: false,
            details: None,
        }
    }

    #[test]
    fn a_relative_url_becomes_absolute_against_the_public_url() {
        let text = render(&payload(Some("/movies/1"), Some("https://kob.example.com")));
        assert!(text.contains("https://kob.example.com/movies/1"), "{text}");
    }

    /// Without a public URL there is nothing openable, so the notification says its
    /// piece and omits the link rather than printing a path nobody can follow.
    #[test]
    fn a_relative_url_with_no_public_url_is_omitted() {
        let text = render(&payload(Some("/movies/1"), None));
        assert!(!text.contains("/movies/1"), "{text}");
        assert!(text.contains("Added someFilm"));
    }

    #[test]
    fn a_card_names_the_show_links_the_thread_and_carries_the_item() {
        let text = render_card("Ludwig (2024)", "42", "**Ludwig 2x03**\n✅ Added");
        assert_eq!(
            text,
            "📺 **Ludwig (2024)** · <#42>\n**Ludwig 2x03**\n✅ Added"
        );
    }

    /// Name and resolution on one line; the release name tiny underneath.
    #[test]
    fn a_release_is_its_name_quality_and_a_tiny_release_name() {
        let mut p = payload(Some("/tv/episodes/1"), Some("https://kob.example.com"));
        p.entity_name = Some("The Simpsons 38x01".to_string());
        p.details = Some(ItemDetails {
            file: Some(FileDetails {
                release_name: "The.Simpsons.S38E01.1080p.DSNP.WEB-DL.H.264-FLUX".to_string(),
                width: Some(1920),
                height: Some(1080),
                hdr: None,
            }),
            ..Default::default()
        });
        assert_eq!(
            render_release(&p),
            "**[The Simpsons 38x01](<https://kob.example.com/tv/episodes/1>)** · 1080p\n\
             -# `The.Simpsons.S38E01.1080p.DSNP.WEB-DL.H.264-FLUX`"
        );
    }

    #[test]
    fn a_release_without_a_file_is_just_its_name() {
        let mut p = payload(None, None);
        p.entity_name = Some("Some Season Pack".to_string());
        assert_eq!(render_release(&p), "**Some Season Pack**");
    }

    #[test]
    fn an_episode_embed_carries_title_summary_air_date_and_still() {
        let mut p = payload(Some("/tv/episodes/1"), Some("https://kob.example.com"));
        p.entity_type = Some("episode".to_string());
        p.details = Some(ItemDetails {
            title: Some("The Ceilidh".to_string()),
            air_date: Some(time::macros::date!(2026 - 09 - 03)),
            summary: Some("John is left in a spin.".to_string()),
            image_url: Some("https://image.tmdb.org/t/p/w300/x.jpg".to_string()),
            file: None,
        });
        let today = time::macros::date!(2026 - 09 - 25);
        let embed = render_embed(&p, today).expect("embed");
        assert_eq!(embed["title"], "The Ceilidh");
        assert_eq!(embed["url"], "https://kob.example.com/tv/episodes/1");
        assert_eq!(embed["description"], "John is left in a spin.");
        assert_eq!(embed["footer"]["text"], "Aired 3 Sep 2026");
        assert_eq!(
            embed["thumbnail"]["url"],
            "https://image.tmdb.org/t/p/w300/x.jpg"
        );
    }

    #[test]
    fn nothing_to_describe_means_no_embed() {
        let mut p = payload(None, None);
        assert!(render_embed(&p, time::Date::MIN).is_none());
        p.details = Some(ItemDetails::default());
        assert!(render_embed(&p, time::Date::MIN).is_none());
    }

    #[test]
    fn a_long_summary_is_cut_on_a_word() {
        let text = truncate(&"word ".repeat(200), 400);
        assert!(text.chars().count() <= 400);
        assert!(text.ends_with("word…"), "{text}");
    }

    /// Discord rejects the whole message over the limit, so a pathological title must
    /// not take the notification down with it.
    #[test]
    fn an_overlong_notification_is_trimmed_to_the_limit() {
        let mut p = payload(None, None);
        p.message = "x".repeat(5000);
        let text = render(&p);
        assert!(text.chars().count() <= 2000);
        assert!(text.ends_with("..."));
    }

    #[tokio::test]
    async fn missing_config_is_reported_rather_than_requested() {
        let agent = DiscordAgent {
            client: reqwest::Client::new(),
            token: String::new(),
            channel_id: "1".to_string(),
            pool: sqlx::Pool::<sqlx::Postgres>::connect_lazy("postgres://localhost/dummy")
                .expect("lazy pool"),
            agent_id: Uuid::nil(),
        };
        let err = agent
            .send(&payload(None, None))
            .await
            .expect_err("no token");
        assert!(matches!(err, NotificationError::ConfigMissing(_)));
    }
}
