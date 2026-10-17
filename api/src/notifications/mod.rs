pub mod types;

pub mod discord;
pub mod google_chat;
pub mod in_app;
mod plex;
pub mod prefs;
mod pushover;
pub mod relevance;
mod webpush;

use std::collections::HashMap;

use async_trait::async_trait;

pub use types::{
    ConfigFieldType, FileDetails, ItemDetails, NotificationAgentInfo, NotificationConfigField,
    NotificationPayload, NotificationTrigger,
};
use uuid::Uuid;
pub use webpush::generate_vapid_keys;

use crate::events::LibraryEvent;

// ============================================================================
// Agent Traits
// ============================================================================

/// Core trait for notification agents.
#[async_trait]
pub trait NotificationAgent: Send + Sync {
    fn info(&self) -> &'static NotificationAgentInfo;
    async fn send(&self, payload: &NotificationPayload) -> Result<(), NotificationError>;
}

/// Factory for creating notification agent instances.
pub trait NotificationAgentFactory: Send + Sync {
    fn info(&self) -> &'static NotificationAgentInfo;
    fn create(
        &self,
        client: reqwest::Client,
        config: &serde_json::Value,
        pool: &sqlx::PgPool,
        agent_id: Uuid,
    ) -> Box<dyn NotificationAgent>;
}

// ============================================================================
// Error Type
// ============================================================================

#[derive(Debug, thiserror::Error)]
pub enum NotificationError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Failed to parse response: {0}")]
    Parse(String),

    #[error("Missing required config field: {0}")]
    ConfigMissing(String),

    #[error("Service error: {0}")]
    ServiceError(String),
}

// ============================================================================
// Agent Registry
// ============================================================================

/// Registry of notification agents.
pub struct NotificationAgentRegistry {
    factories: HashMap<&'static str, Box<dyn NotificationAgentFactory>>,
}

impl Default for NotificationAgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationAgentRegistry {
    pub fn new() -> Self {
        let mut factories: HashMap<&'static str, Box<dyn NotificationAgentFactory>> =
            HashMap::new();

        factories.insert(pushover::INFO.id, Box::new(pushover::PushoverFactory));
        factories.insert(
            google_chat::INFO.id,
            Box::new(google_chat::GoogleChatFactory),
        );
        factories.insert(discord::INFO.id, Box::new(discord::DiscordFactory));
        factories.insert(plex::INFO.id, Box::new(plex::PlexFactory));
        factories.insert(webpush::INFO.id, Box::new(webpush::WebPushFactory));

        Self { factories }
    }

    /// Get information about all registered agent types.
    pub fn available_types(&self) -> Vec<&'static NotificationAgentInfo> {
        self.factories.values().map(|f| f.info()).collect()
    }

    /// Get agent info by type.
    pub fn get_type_info(&self, agent_type: &str) -> Option<&'static NotificationAgentInfo> {
        self.factories.get(agent_type).map(|f| f.info())
    }

    /// Create an agent instance with the given config.
    pub fn create_agent(
        &self,
        agent_type: &str,
        config: &serde_json::Value,
        pool: &sqlx::PgPool,
        agent_id: Uuid,
    ) -> Option<Box<dyn NotificationAgent>> {
        let factory = self.factories.get(agent_type)?;
        let client = crate::http::new_client();
        Some(factory.create(client, config, pool, agent_id))
    }
}

// ============================================================================
// Event Translation
// ============================================================================

/// Result of translating a library event into notification data.
pub struct TranslatedEvent {
    pub trigger: NotificationTrigger,
    pub title: String,
    pub message_template: &'static str,
    pub entity_id: Uuid,
    /// True when this acquisition replaced an older file (a quality upgrade).
    pub upgraded: bool,
}

/// Maps a library event to a notification trigger.
///
/// Returns `None` for events that don't map to any notification trigger.
/// The `message_template` contains `{}` as a placeholder for the entity name.
pub fn translate_event(event: &LibraryEvent) -> Option<TranslatedEvent> {
    match event {
        // `announce: false` is a season pack's episodes: the screens need each one,
        // but the pack has already spoken for all of them.
        LibraryEvent::WantedStatusUpdated {
            wanted_id,
            status,
            announce: true,
            upgraded,
        } => match status.as_str() {
            "downloading" => Some(TranslatedEvent {
                trigger: NotificationTrigger::OnGrab,
                title: "Download Started".to_string(),
                message_template: "{} grabbed",
                entity_id: *wanted_id,
                upgraded: *upgraded,
            }),
            "acquired" => Some(TranslatedEvent {
                trigger: NotificationTrigger::OnImport,
                title: "Import Complete".to_string(),
                message_template: "{} added",
                entity_id: *wanted_id,
                upgraded: *upgraded,
            }),
            "search_failed" => Some(TranslatedEvent {
                trigger: NotificationTrigger::OnFailed,
                title: "Search Failed".to_string(),
                message_template: "Search failed for {}",
                entity_id: *wanted_id,
                upgraded: *upgraded,
            }),
            _ => None,
        },
        _ => None,
    }
}

/// Entity info resolved from a UUID across books, movies, and TV episodes.
pub struct ResolvedEntity {
    pub entity_type: &'static str,
    pub name: String,
    pub url: String,
    /// The show this item belongs to (episodes and season packs). Lets an agent
    /// group a show's releases together — Discord posts them into one thread.
    pub show_id: Option<Uuid>,
    pub show_name: Option<String>,
    pub season_number: Option<i32>,
    /// The show's year, for disambiguating thread names ("Show (1990)").
    pub show_year: Option<i32>,
    /// Title, date, summary and artwork, for a single episode or movie.
    pub details: Option<types::ItemDetails>,
}

/// An episode title worth showing: not blank, and not a placeholder like
/// "Episode 3" that TMDB uses before the real title is known.
fn real_episode_title(title: Option<String>) -> Option<String> {
    let title = title?.trim().to_string();
    let placeholder = title
        .strip_prefix("Episode ")
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    (!title.is_empty() && !placeholder).then_some(title)
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// The library file's release name and quality, from a file row's columns.
fn file_details(
    path: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
    release_title: Option<String>,
    hdr_format: Option<String>,
    is_hdr: Option<bool>,
) -> Option<types::FileDetails> {
    let path = path?;
    let release_name = non_empty(release_title).unwrap_or_else(|| {
        std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(path)
    });
    Some(types::FileDetails {
        release_name,
        width,
        height,
        hdr: non_empty(hdr_format).or_else(|| is_hdr.unwrap_or(false).then(|| "HDR".to_string())),
    })
}

/// Look up entity info by UUID across books, movies, and TV episodes.
pub async fn resolve_entity(pool: &sqlx::PgPool, id: Uuid) -> Option<ResolvedEntity> {
    // Try books first (most common)
    if let Ok(Some(row)) = sqlx::query!(r#"SELECT title FROM books WHERE id = $1"#, id)
        .fetch_optional(pool)
        .await
    {
        return Some(ResolvedEntity {
            entity_type: "book",
            name: row.title,
            url: format!("/library/books/{id}"),
            show_id: None,
            show_name: None,
            season_number: None,
            show_year: None,
            details: None,
        });
    }

    // Try movies
    if let Ok(Some(row)) = sqlx::query!(
        r#"
        SELECT m.title, m.description, m.release_date,
               m.images->'poster'->>'source_url' AS "image_url?",
               f.file_path AS "file_path?",
               f.video_width AS "video_width?",
               f.video_height AS "video_height?",
               f.release_title AS "release_title?",
               f.hdr_format AS "hdr_format?",
               f.is_hdr AS "is_hdr?"
        FROM movies m
        LEFT JOIN LATERAL (
            SELECT file_path, video_width, video_height, hdr_format, is_hdr,
                   source_metadata->>'release_title' AS release_title
            FROM movie_files
            WHERE movie_id = m.id AND available
            ORDER BY created_at DESC
            LIMIT 1
        ) f ON TRUE
        WHERE m.id = $1
        "#,
        id
    )
    .fetch_optional(pool)
    .await
    {
        return Some(ResolvedEntity {
            entity_type: "movie",
            name: row.title,
            url: format!("/movies/{id}"),
            show_id: None,
            show_name: None,
            season_number: None,
            show_year: None,
            details: Some(ItemDetails {
                title: None,
                air_date: row.release_date,
                summary: non_empty(row.description),
                image_url: non_empty(row.image_url),
                file: file_details(
                    row.file_path,
                    row.video_width,
                    row.video_height,
                    row.release_title,
                    row.hdr_format,
                    row.is_hdr,
                ),
            }),
        });
    }

    // Try TV episodes (include show name + SxEE for context, or sport-specific format)
    if let Ok(Some(row)) = sqlx::query!(
        r#"
        SELECT ts.id AS show_id,
               ts.title AS show_title,
               ts.content_category,
               ts.year,
               s.season_sort_order,
               e.episode_sort_order,
               e.title AS "episode_title?",
               e.description AS "episode_description?",
               e.air_date AS "air_date?",
               e.images->'still'->>'source_url' AS "still_url?",
               f.file_path AS "file_path?",
               f.video_width AS "video_width?",
               f.video_height AS "video_height?",
               f.release_title AS "release_title?",
               f.hdr_format AS "hdr_format?",
               f.is_hdr AS "is_hdr?"
        FROM tv_episodes e
        JOIN tv_seasons s ON s.id = e.season_id
        JOIN tv_shows ts ON ts.id = s.show_id
        LEFT JOIN LATERAL (
            SELECT tf.file_path, tf.video_width, tf.video_height, tf.hdr_format,
                   tf.is_hdr, tf.source_metadata->>'release_title' AS release_title
            FROM tv_episode_file_episodes fe
            JOIN tv_episode_files tf ON tf.id = fe.episode_file_id
            WHERE fe.episode_id = e.id AND tf.available
            ORDER BY tf.created_at DESC
            LIMIT 1
        ) f ON TRUE
        WHERE e.id = $1
        "#,
        id
    )
    .fetch_optional(pool)
    .await
    {
        let is_sport = row.content_category == "sport";
        let name = if is_sport {
            // Format: "League EventTrimmed Season" e.g. "Moto2 Thailand Race 2026"
            let event_title = row
                .episode_title
                .as_deref()
                .map(|t| t.strip_prefix(&row.show_title).unwrap_or(t).trim())
                .unwrap_or("");
            if event_title.is_empty() {
                format!("{} {}", row.show_title, row.season_sort_order)
            } else {
                format!(
                    "{} {} {}",
                    row.show_title, event_title, row.season_sort_order
                )
            }
        } else {
            format!(
                "{} {}x{:02}",
                row.show_title, row.season_sort_order, row.episode_sort_order
            )
        };
        let details = ItemDetails {
            // A sport event's title is already in the name.
            title: match is_sport {
                true => None,
                false => real_episode_title(row.episode_title),
            },
            air_date: row.air_date,
            summary: non_empty(row.episode_description),
            image_url: non_empty(row.still_url),
            file: file_details(
                row.file_path,
                row.video_width,
                row.video_height,
                row.release_title,
                row.hdr_format,
                row.is_hdr,
            ),
        };
        return Some(ResolvedEntity {
            entity_type: "episode",
            name,
            url: format!("/tv/episodes/{id}"),
            show_id: Some(row.show_id),
            show_name: Some(row.show_title),
            season_number: Some(row.season_sort_order),
            show_year: row.year,
            details: Some(details),
        });
    }

    // Try TV seasons (season pack downloads broadcast the season_id)
    if let Ok(Some(row)) = sqlx::query!(
        r#"
        SELECT ts.title AS show_title,
               ts.id AS show_id,
               ts.content_category,
               ts.year,
               s.season_sort_order,
               (SELECT array_agg(e.episode_sort_order ORDER BY e.episode_sort_order)
                FROM tv_episodes e WHERE e.season_id = s.id
               ) AS "episodes?: Vec<i32>"
        FROM tv_seasons s
        JOIN tv_shows ts ON ts.id = s.show_id
        WHERE s.id = $1
        "#,
        id
    )
    .fetch_optional(pool)
    .await
    {
        let episodes = row.episodes.unwrap_or_default();
        let name = if row.content_category == "sport" {
            format!("{} Season {}", row.show_title, row.season_sort_order)
        } else if episodes.len() == 1 {
            // Single episode: "Show 1x05"
            format!(
                "{} {}x{:02}",
                row.show_title, row.season_sort_order, episodes[0]
            )
        } else if episodes.len() <= 6 {
            // Small batch: "Show 1x01, 1x02, 1x03"
            let ep_list: Vec<String> = episodes
                .iter()
                .map(|e| format!("{}x{:02}", row.season_sort_order, e))
                .collect();
            format!("{} {}", row.show_title, ep_list.join(", "))
        } else {
            // Big list: "Show Season 1 (20 episodes)"
            format!(
                "{} Season {} ({} episodes)",
                row.show_title,
                row.season_sort_order,
                episodes.len()
            )
        };
        let entity_type = if episodes.len() == 1 {
            "episode"
        } else {
            "season"
        };
        return Some(ResolvedEntity {
            entity_type,
            name,
            url: format!("/tv/{}", row.show_id),
            show_id: Some(row.show_id),
            show_name: Some(row.show_title),
            season_number: Some(row.season_sort_order),
            show_year: row.year,
            details: None,
        });
    }

    // Try TV shows
    if let Ok(Some(row)) = sqlx::query!(r#"SELECT title, year FROM tv_shows WHERE id = $1"#, id)
        .fetch_optional(pool)
        .await
    {
        return Some(ResolvedEntity {
            entity_type: "show",
            name: row.title.clone(),
            url: format!("/tv/{id}"),
            show_id: Some(id),
            show_name: Some(row.title),
            season_number: None,
            show_year: row.year,
            details: None,
        });
    }

    None
}

/// Raise a notification from somewhere that has no `LibraryEvent` to emit.
///
/// The dispatcher in `main.rs` fans library events out to agents; background
/// jobs have no such event and no access to that bus, so this does the same
/// fan-out directly: find enabled agents subscribed to the trigger, enqueue one
/// `notification_send` each. Best-effort by design — a notification that cannot
/// be delivered must never fail the work that raised it.
pub async fn notify_agents(
    pool: &sqlx::PgPool,
    publisher: &nats_jobs::Publisher,
    payload: &types::NotificationPayload,
) {
    let agents = match crate::repositories::notification_agents::list_enabled(pool).await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, "notify_agents: could not list agents");
            return;
        }
    };
    let trigger = match serde_json::to_value(payload.trigger) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, "notify_agents: trigger did not serialize");
            return;
        }
    };
    for agent in agents {
        let subscribed = agent
            .triggers
            .as_array()
            .is_some_and(|ts| ts.contains(&trigger));
        if !subscribed {
            continue;
        }
        let context = crate::jobs::JobContext::new(format!("Notifying {}", agent.name));
        if let Err(e) = crate::jobs::enqueue_routine(
            pool,
            publisher,
            "notification_send",
            serde_json::json!({ "agent_id": agent.id, "payload": payload }),
            context,
        )
        .await
        {
            tracing::warn!(error = %e, agent_id = %agent.id, "notify_agents: enqueue failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A season pack moves thirteen episodes at one instant. Every one of them is
    /// broadcast so the UI can draw it; none of them is news, because the pack
    /// has already said it once as the season.
    #[test]
    fn only_an_announcing_update_becomes_a_notification() {
        let id = Uuid::now_v7();
        assert!(
            translate_event(&LibraryEvent::wanted_status_updated(
                id,
                "acquired".to_string()
            ))
            .is_some()
        );
        assert!(
            translate_event(&LibraryEvent::wanted_status_updated_quietly(
                id,
                "acquired".to_string()
            ))
            .is_none()
        );
    }
}
