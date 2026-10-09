use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Events that can trigger notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationTrigger {
    OnGrab,
    OnImport,
    OnFailed,
    /// A file already in the library turned out not to be intact.
    OnMediaCorrupt,
    OnTest,
}

impl NotificationTrigger {
    pub fn all() -> &'static [NotificationTrigger] {
        &[
            NotificationTrigger::OnGrab,
            NotificationTrigger::OnImport,
            NotificationTrigger::OnFailed,
            NotificationTrigger::OnMediaCorrupt,
        ]
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::OnGrab => "On Grab",
            Self::OnImport => "On Import",
            Self::OnFailed => "On Failed",
            Self::OnMediaCorrupt => "On Corrupt Media",
            Self::OnTest => "Test",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::OnGrab => "When a download starts",
            Self::OnImport => "When an item is added to the library",
            Self::OnFailed => "When acquisition fails",
            Self::OnMediaCorrupt => "When a file in the library is found incomplete or unreadable",
            Self::OnTest => "Test notification",
        }
    }
}

/// Payload passed to agents when a notification fires.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationPayload {
    pub trigger: NotificationTrigger,
    pub title: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Externally reachable base for `url`. Kept separate because `url` stays
    /// relative for web push, which is served from the same origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    /// The show this item belongs to (episodes and season packs). Lets an agent
    /// group a show's releases together — Discord posts them into one thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season_number: Option<i32>,
    /// The show's year, for disambiguating thread names ("Show (1990)").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_year: Option<i32>,
    /// True when this acquisition replaced a file already in the library (an
    /// upgrade, a repack, a re-download).
    /// Agents that keep a per-entity message map edit the original post in place
    /// rather than announcing the same item twice.
    #[serde(default)]
    pub upgraded: bool,
    /// What the item is about — episode title, air date, summary, artwork — for
    /// agents that can show more than one line (Discord renders it as an embed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ItemDetails>,
}

/// Descriptive metadata for a single episode or movie.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ItemDetails {
    /// The episode's own title ("The Rains of Castamere"); `None` when it is only
    /// a placeholder ("Episode 3") or already part of the entity name (sport).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Air date for an episode, release date for a movie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub air_date: Option<time::Date>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// An absolute, publicly reachable image (the TMDB still or poster).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    /// The file now in the library for it, if there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<FileDetails>,
}

/// The library file behind an episode or movie: what was downloaded, and how good.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FileDetails {
    /// The release as downloaded ("Show.S01E01.1080p.WEB.h264-GRP") — the name the
    /// web file list shows — or the file's own name when that is not known.
    pub release_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
    /// "Dolby Vision", "HDR10", or "HDR" when only the flag is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdr: Option<String>,
}

impl FileDetails {
    /// "1080p", "2160p HDR10" — what a viewer cares about, not the encode.
    pub fn quality(&self) -> Option<String> {
        let res = resolution_label(self.width, self.height)?;
        Some(match &self.hdr {
            Some(hdr) => format!("{res} {hdr}"),
            None => res,
        })
    }
}

/// The marketing resolution, judged by width first: a scope film at 1920×800 is
/// still 1080p.
fn resolution_label(width: Option<i32>, height: Option<i32>) -> Option<String> {
    let (w, h) = (width.unwrap_or(0), height.unwrap_or(0));
    let label = if w >= 3200 || h >= 2000 {
        "2160p"
    } else if w >= 1800 || h >= 1000 {
        "1080p"
    } else if w >= 1200 || h >= 700 {
        "720p"
    } else if h > 0 {
        return Some(format!("{h}p"));
    } else {
        return None;
    };
    Some(label.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_is_resolution_and_hdr() {
        let f = FileDetails {
            release_name: "x".into(),
            width: Some(1920),
            height: Some(800),
            hdr: Some("HDR10".into()),
        };
        assert_eq!(f.quality().as_deref(), Some("1080p HDR10"));
    }

    #[test]
    fn nothing_known_means_no_quality() {
        let f = FileDetails {
            release_name: "x".into(),
            ..Default::default()
        };
        assert_eq!(f.quality(), None);
    }
}

impl NotificationPayload {
    /// The link to hand an agent that opens it outside the app, or None when
    /// no public URL is configured — a relative path is not openable there.
    pub fn external_url(&self) -> Option<String> {
        let url = self.url.as_deref().filter(|u| !u.is_empty())?;
        if url.starts_with("http://") || url.starts_with("https://") {
            return Some(url.to_string());
        }
        let base = self.public_url.as_deref().filter(|b| !b.is_empty())?;
        Some(format!(
            "{}/{}",
            base.trim_end_matches('/'),
            url.trim_start_matches('/')
        ))
    }
}

/// Configuration field types for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFieldType {
    String,
    Secret,
    Url,
    Number,
}

/// A configuration field that a notification agent accepts.
#[derive(Debug, Clone, Serialize)]
pub struct NotificationConfigField {
    pub key: &'static str,
    pub label: &'static str,
    pub field_type: ConfigFieldType,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help_url: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<&'static str>,
}

/// Static information about a notification agent type.
#[derive(Debug, Clone, Serialize)]
pub struct NotificationAgentInfo {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub supported_triggers: &'static [NotificationTrigger],
    pub config_fields: &'static [NotificationConfigField],
}
