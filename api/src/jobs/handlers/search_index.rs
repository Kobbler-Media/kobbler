//! Search index job handler for indexing entities to search backends.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::search::{self, CommandDocument, PageDocument, SearchBackend, SettingDocument};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Payload for search_index jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct SearchIndexPayload {
    /// Type of entity to index: "book", "author", or "series"
    pub entity_type: String,
    /// ID of the entity to index
    pub entity_id: Uuid,
}

pub struct SearchIndexHandler {
    pool: PgPool,
    search: Arc<dyn SearchBackend>,
}

impl SearchIndexHandler {
    pub fn new(pool: PgPool, search: Arc<dyn SearchBackend>) -> Self {
        Self { pool, search }
    }
}

#[async_trait]
impl JobHandler for SearchIndexHandler {
    fn job_type(&self) -> &'static str {
        "search_index"
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: SearchIndexPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {}", e)))?;
        nats_jobs::check_cancelled(&cancel)?;

        info!(
            job_id = %job_id,
            entity_type = %payload.entity_type,
            entity_id = %payload.entity_id,
            "Indexing entity to search backend"
        );

        match payload.entity_type.as_str() {
            "book" => {
                search::index_book(self.search.as_ref(), &self.pool, payload.entity_id)
                    .await
                    .map_err(|e| {
                        error!(error = %e, "Failed to index book");
                        JobError::retryable(format!("Failed to index book: {}", e))
                    })?;
            }
            "author" => {
                search::index_author(self.search.as_ref(), &self.pool, payload.entity_id)
                    .await
                    .map_err(|e| {
                        error!(error = %e, "Failed to index author");
                        JobError::retryable(format!("Failed to index author: {}", e))
                    })?;
            }
            "series" => {
                search::index_series(self.search.as_ref(), &self.pool, payload.entity_id)
                    .await
                    .map_err(|e| {
                        error!(error = %e, "Failed to index series");
                        JobError::retryable(format!("Failed to index series: {}", e))
                    })?;
            }
            "movie" => {
                search::index_movie(self.search.as_ref(), &self.pool, payload.entity_id)
                    .await
                    .map_err(|e| {
                        error!(error = %e, "Failed to index movie");
                        JobError::retryable(format!("Failed to index movie: {}", e))
                    })?;
            }
            "tv_show" => {
                search::index_tv_show(self.search.as_ref(), &self.pool, payload.entity_id)
                    .await
                    .map_err(|e| {
                        error!(error = %e, "Failed to index TV show");
                        JobError::retryable(format!("Failed to index TV show: {}", e))
                    })?;
            }
            "tv_show_children" => {
                let (seasons, episodes) = search::index_tv_show_children(
                    self.search.as_ref(),
                    &self.pool,
                    payload.entity_id,
                )
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to index TV show seasons and episodes");
                    JobError::retryable(format!(
                        "Failed to index TV show seasons and episodes: {}",
                        e
                    ))
                })?;
                info!(seasons, episodes, "Indexed TV show seasons and episodes");
            }
            "tv_season" => {
                search::index_tv_season(self.search.as_ref(), &self.pool, payload.entity_id)
                    .await
                    .map_err(|e| {
                        error!(error = %e, "Failed to index TV season");
                        JobError::retryable(format!("Failed to index TV season: {}", e))
                    })?;
            }
            "tv_episode" => {
                search::index_tv_episode(self.search.as_ref(), &self.pool, payload.entity_id)
                    .await
                    .map_err(|e| {
                        error!(error = %e, "Failed to index TV episode");
                        JobError::retryable(format!("Failed to index TV episode: {}", e))
                    })?;
            }
            "person" => {
                search::index_person(self.search.as_ref(), &self.pool, payload.entity_id)
                    .await
                    .map_err(|e| {
                        error!(error = %e, "Failed to index person");
                        JobError::retryable(format!("Failed to index person: {}", e))
                    })?;
            }
            other => {
                return Err(JobError::ExecutionFailed(format!(
                    "Unknown entity type: {}",
                    other
                )));
            }
        }

        Ok(json!({
            "indexed": true,
            "entity_type": payload.entity_type,
            "entity_id": payload.entity_id
        }))
    }
}

/// Payload for search_reindex jobs (full rebuild of all indexes).
#[derive(Debug, Deserialize, Serialize, Default)]
pub struct SearchReindexPayload {
    /// Optional: only reindex specific types (default: all)
    #[serde(default)]
    pub entity_types: Option<Vec<String>>,
}

/// Static pages registry for search indexing.
/// Matches web/src/lib/components/CommandPalette/registry.ts
pub fn get_static_pages() -> Vec<PageDocument> {
    vec![
        PageDocument {
            id: "home".to_string(),
            label: "Home".to_string(),
            path: "/".to_string(),
            aliases: vec![
                "home".to_string(),
                "discover".to_string(),
                "find".to_string(),
                "search".to_string(),
            ],
            icon: "home".to_string(),
        },
        PageDocument {
            id: "dashboard".to_string(),
            label: "Dashboard".to_string(),
            path: "/dashboard".to_string(),
            aliases: vec![
                "dashboard".to_string(),
                "activity".to_string(),
                "overview".to_string(),
            ],
            icon: "dashboard".to_string(),
        },
        PageDocument {
            id: "library".to_string(),
            label: "Books".to_string(),
            path: "/library".to_string(),
            aliases: vec!["library".to_string(), "collection".to_string()],
            icon: "books".to_string(),
        },
        PageDocument {
            id: "movies".to_string(),
            label: "Movies".to_string(),
            path: "/movies".to_string(),
            aliases: vec!["film".to_string(), "cinema".to_string()],
            icon: "movies".to_string(),
        },
        PageDocument {
            id: "tv".to_string(),
            label: "TV Shows".to_string(),
            path: "/tv".to_string(),
            aliases: vec![
                "television".to_string(),
                "shows".to_string(),
                "series".to_string(),
            ],
            icon: "tv".to_string(),
        },
        PageDocument {
            id: "series".to_string(),
            label: "Series".to_string(),
            path: "/series".to_string(),
            aliases: vec![],
            icon: "series".to_string(),
        },
        PageDocument {
            id: "authors".to_string(),
            label: "Authors".to_string(),
            path: "/authors".to_string(),
            aliases: vec![],
            icon: "authors".to_string(),
        },
        PageDocument {
            id: "shelves".to_string(),
            label: "Shelves".to_string(),
            path: "/shelves".to_string(),
            aliases: vec!["collections".to_string(), "tags".to_string()],
            icon: "shelves".to_string(),
        },
        PageDocument {
            id: "wanted".to_string(),
            label: "Wanted Books".to_string(),
            path: "/library?ownership=wanted".to_string(),
            aliases: vec![
                "track".to_string(),
                "tracking".to_string(),
                "follow".to_string(),
                "wanted".to_string(),
                "queue".to_string(),
                "monitored".to_string(),
            ],
            icon: "monitored".to_string(),
        },
        PageDocument {
            id: "downloads".to_string(),
            label: "Downloads".to_string(),
            path: "/downloads".to_string(),
            aliases: vec![
                "download".to_string(),
                "active".to_string(),
                "transfer".to_string(),
            ],
            icon: "downloads".to_string(),
        },
        PageDocument {
            id: "ingest".to_string(),
            label: "Ingest Queue".to_string(),
            path: "/ingest-queue".to_string(),
            aliases: vec![
                "import".to_string(),
                "upload".to_string(),
                "add".to_string(),
            ],
            icon: "ingest".to_string(),
        },
        PageDocument {
            id: "activity".to_string(),
            label: "Activity Logs".to_string(),
            path: "/activity".to_string(),
            aliases: vec!["logs".to_string(), "history".to_string()],
            icon: "logs".to_string(),
        },
        PageDocument {
            id: "proxy".to_string(),
            label: "Proxy Logs".to_string(),
            path: "/proxy-logs".to_string(),
            aliases: vec!["kobo".to_string(), "sync".to_string()],
            icon: "proxy".to_string(),
        },
        PageDocument {
            id: "chat-logs".to_string(),
            label: "Chat Turn Logs".to_string(),
            path: "/chat-logs".to_string(),
            aliases: vec![
                "chat".to_string(),
                "bot".to_string(),
                "turn".to_string(),
                "llm".to_string(),
                "assistant".to_string(),
                "debug".to_string(),
            ],
            icon: "chatLogs".to_string(),
        },
        PageDocument {
            id: "devices".to_string(),
            label: "Devices".to_string(),
            path: "/settings/devices".to_string(),
            aliases: vec![
                "kobo".to_string(),
                "reader".to_string(),
                "ereader".to_string(),
            ],
            icon: "devices".to_string(),
        },
        PageDocument {
            id: "media-roots".to_string(),
            label: "Media Roots".to_string(),
            path: "/settings/media-roots".to_string(),
            aliases: vec![
                "root".to_string(),
                "folder".to_string(),
                "directory".to_string(),
                "scan".to_string(),
                "media".to_string(),
                "library".to_string(),
            ],
            icon: "media-roots".to_string(),
        },
        PageDocument {
            id: "users".to_string(),
            label: "Users".to_string(),
            path: "/settings/users".to_string(),
            aliases: vec![],
            icon: "users".to_string(),
        },
        PageDocument {
            id: "chat-identities".to_string(),
            label: "Chat Identities".to_string(),
            path: "/settings/chat-identities".to_string(),
            aliases: vec![
                "chat".to_string(),
                "bot".to_string(),
                "google".to_string(),
                "pairing".to_string(),
                "identity".to_string(),
            ],
            icon: "chatIdentities".to_string(),
        },
        PageDocument {
            id: "chat-memory".to_string(),
            label: "Chat Memory".to_string(),
            path: "/settings/chat-memory".to_string(),
            aliases: vec![
                "memory".to_string(),
                "facts".to_string(),
                "profile".to_string(),
                "remember".to_string(),
                "forget".to_string(),
                "privacy".to_string(),
                "assistant".to_string(),
            ],
            icon: "chatMemory".to_string(),
        },
        PageDocument {
            id: "sessions".to_string(),
            label: "Sessions".to_string(),
            path: "/settings/sessions".to_string(),
            aliases: vec![
                "device".to_string(),
                "login".to_string(),
                "revoke".to_string(),
                "signed in".to_string(),
                "logout".to_string(),
                "linked".to_string(),
            ],
            icon: "sessions".to_string(),
        },
        PageDocument {
            id: "tokens".to_string(),
            label: "API Tokens".to_string(),
            path: "/settings/tokens".to_string(),
            aliases: vec!["api".to_string(), "key".to_string(), "auth".to_string()],
            icon: "tokens".to_string(),
        },
        PageDocument {
            id: "general".to_string(),
            label: "General Settings".to_string(),
            path: "/settings/general".to_string(),
            aliases: vec![
                "config".to_string(),
                "configuration".to_string(),
                "system".to_string(),
                "watcher".to_string(),
                "cache".to_string(),
                "pagination".to_string(),
            ],
            icon: "general".to_string(),
        },
        PageDocument {
            id: "audio".to_string(),
            label: "Audio & Speech Settings".to_string(),
            path: "/settings/audio".to_string(),
            aliases: vec![
                "tts".to_string(),
                "text-to-speech".to_string(),
                "speech".to_string(),
                "transcription".to_string(),
                "whisper".to_string(),
                "stt".to_string(),
            ],
            icon: "audio".to_string(),
        },
        PageDocument {
            id: "voice".to_string(),
            label: "Voice Presets".to_string(),
            path: "/settings/voice".to_string(),
            aliases: vec![
                "tts".to_string(),
                "voice".to_string(),
                "preset".to_string(),
                "clone".to_string(),
                "emotion".to_string(),
                "speech".to_string(),
            ],
            icon: "voice".to_string(),
        },
        PageDocument {
            id: "obtain".to_string(),
            label: "Obtain Settings".to_string(),
            path: "/settings/obtain".to_string(),
            aliases: vec!["download".to_string(), "kobload".to_string()],
            icon: "obtain".to_string(),
        },
        PageDocument {
            id: "streaming".to_string(),
            label: "Streaming Settings".to_string(),
            path: "/settings/streaming".to_string(),
            aliases: vec![
                "debrid".to_string(),
                "stremio".to_string(),
                "real-debrid".to_string(),
                "torbox".to_string(),
                "cache".to_string(),
            ],
            icon: "streaming".to_string(),
        },
        PageDocument {
            id: "cast".to_string(),
            label: "Cast Settings".to_string(),
            path: "/settings/cast".to_string(),
            aliases: vec![
                "chromecast".to_string(),
                "google tv".to_string(),
                "casting".to_string(),
                "receiver".to_string(),
            ],
            icon: "cast".to_string(),
        },
        PageDocument {
            id: "metadata".to_string(),
            label: "Metadata Settings".to_string(),
            path: "/settings/metadata".to_string(),
            aliases: vec!["info".to_string(), "data".to_string()],
            icon: "metadata".to_string(),
        },
        PageDocument {
            id: "subtitles".to_string(),
            label: "Subtitle Settings".to_string(),
            path: "/settings/subtitles".to_string(),
            aliases: vec![
                "subtitle".to_string(),
                "opensubtitles".to_string(),
                "captions".to_string(),
                "srt".to_string(),
                "vtt".to_string(),
            ],
            icon: "subtitles".to_string(),
        },
        PageDocument {
            id: "search-settings".to_string(),
            label: "Search & AI Settings".to_string(),
            path: "/settings/search".to_string(),
            aliases: vec![
                "meili".to_string(),
                "meilisearch".to_string(),
                "index".to_string(),
                "indexer".to_string(),
                "koblem".to_string(),
                "llm".to_string(),
                "chat".to_string(),
            ],
            icon: "search".to_string(),
        },
        PageDocument {
            id: "integrations".to_string(),
            label: "Integrations".to_string(),
            path: "/settings/integrations".to_string(),
            aliases: vec!["connect".to_string(), "external".to_string()],
            icon: "integrations".to_string(),
        },
        PageDocument {
            id: "notification-inbox".to_string(),
            label: "Notification Inbox".to_string(),
            path: "/notifications".to_string(),
            aliases: vec![
                "inbox".to_string(),
                "alerts".to_string(),
                "bell".to_string(),
            ],
            icon: "notifications".to_string(),
        },
        PageDocument {
            id: "notification-settings".to_string(),
            label: "Notification Settings".to_string(),
            path: "/settings/notifications".to_string(),
            aliases: vec![
                "notify".to_string(),
                "push".to_string(),
                "pushover".to_string(),
                "plex".to_string(),
                "alert".to_string(),
            ],
            icon: "notifications".to_string(),
        },
        PageDocument {
            id: "jobs".to_string(),
            label: "Jobs".to_string(),
            path: "/jobs".to_string(),
            aliases: vec![
                "tasks".to_string(),
                "background".to_string(),
                "scheduled".to_string(),
                "queue".to_string(),
                "history".to_string(),
            ],
            icon: "jobs".to_string(),
        },
        PageDocument {
            id: "job-concurrency".to_string(),
            label: "Job Concurrency".to_string(),
            path: "/settings/jobs".to_string(),
            aliases: vec![
                "parallelism".to_string(),
                "workers".to_string(),
                "threads".to_string(),
            ],
            icon: "jobs".to_string(),
        },
        PageDocument {
            id: "playback_reports".to_string(),
            label: "Playback Reports".to_string(),
            path: "/settings/diagnostics".to_string(),
            aliases: vec![
                "errors".to_string(),
                "failures".to_string(),
                "diagnostics".to_string(),
                "crash".to_string(),
                "client reports".to_string(),
            ],
            icon: "diagnostics".to_string(),
        },
        PageDocument {
            id: "backup".to_string(),
            label: "Backup".to_string(),
            path: "/settings/backup".to_string(),
            aliases: vec![
                "export".to_string(),
                "restore".to_string(),
                "save".to_string(),
            ],
            icon: "backup".to_string(),
        },
        PageDocument {
            id: "people".to_string(),
            label: "People".to_string(),
            path: "/people".to_string(),
            aliases: vec![
                "person".to_string(),
                "actor".to_string(),
                "director".to_string(),
                "cast".to_string(),
                "crew".to_string(),
            ],
            icon: "people".to_string(),
        },
        PageDocument {
            id: "calendar".to_string(),
            label: "Calendar".to_string(),
            path: "/calendar".to_string(),
            aliases: vec![
                "upcoming".to_string(),
                "releases".to_string(),
                "schedule".to_string(),
            ],
            icon: "calendar".to_string(),
        },
        PageDocument {
            id: "sports".to_string(),
            label: "Sports".to_string(),
            path: "/sports".to_string(),
            aliases: vec![
                "league".to_string(),
                "motorsport".to_string(),
                "motogp".to_string(),
                "race".to_string(),
                "racing".to_string(),
                "formula".to_string(),
            ],
            icon: "sports".to_string(),
        },
        PageDocument {
            id: "discover-settings".to_string(),
            label: "Discover Settings".to_string(),
            path: "/settings/discover".to_string(),
            aliases: vec![
                "streaming services".to_string(),
                "netflix".to_string(),
                "region".to_string(),
            ],
            icon: "discover".to_string(),
        },
        PageDocument {
            id: "migration".to_string(),
            label: "Migration".to_string(),
            path: "/settings/migration".to_string(),
            aliases: vec![
                "sonarr".to_string(),
                "radarr".to_string(),
                "import".to_string(),
                "migrate".to_string(),
            ],
            icon: "migration".to_string(),
        },
        PageDocument {
            id: "profile".to_string(),
            label: "Profile".to_string(),
            path: "/profile".to_string(),
            aliases: vec![
                "account".to_string(),
                "avatar".to_string(),
                "password".to_string(),
                "user".to_string(),
            ],
            icon: "profile".to_string(),
        },
        PageDocument {
            id: "iptv".to_string(),
            label: "Live TV Home".to_string(),
            path: "/iptv".to_string(),
            aliases: vec![
                "live".to_string(),
                "tv".to_string(),
                "iptv".to_string(),
                "home".to_string(),
            ],
            icon: "iptv".to_string(),
        },
        PageDocument {
            id: "iptv-channels".to_string(),
            label: "Channels".to_string(),
            path: "/iptv/channels".to_string(),
            aliases: vec![
                "channels".to_string(),
                "live".to_string(),
                "stream".to_string(),
                "iptv".to_string(),
            ],
            icon: "iptv".to_string(),
        },
        PageDocument {
            id: "iptv-recordings".to_string(),
            label: "Recordings".to_string(),
            path: "/iptv/recordings".to_string(),
            aliases: vec![
                "recording".to_string(),
                "dvr".to_string(),
                "record".to_string(),
            ],
            icon: "iptv".to_string(),
        },
        PageDocument {
            id: "livetv".to_string(),
            label: "Live TV".to_string(),
            path: "/livetv".to_string(),
            aliases: vec![
                "livetv".to_string(),
                "antenna".to_string(),
                "broadcast".to_string(),
                "tuner".to_string(),
                "hdhomerun".to_string(),
            ],
            icon: "livetv".to_string(),
        },
        PageDocument {
            id: "livetv-guide".to_string(),
            label: "TV Guide".to_string(),
            path: "/livetv/guide".to_string(),
            aliases: vec![
                "guide".to_string(),
                "epg".to_string(),
                "listings".to_string(),
                "whats on".to_string(),
            ],
            icon: "livetv".to_string(),
        },
        PageDocument {
            id: "livetv-channels".to_string(),
            label: "Live TV Channels".to_string(),
            path: "/livetv/channels".to_string(),
            aliases: vec![
                "channels".to_string(),
                "lineup".to_string(),
                "scan".to_string(),
                "tuner".to_string(),
            ],
            icon: "livetv".to_string(),
        },
        PageDocument {
            id: "livetv-recordings".to_string(),
            label: "Live TV Recordings".to_string(),
            path: "/livetv/recordings".to_string(),
            aliases: vec![
                "dvr".to_string(),
                "recordings".to_string(),
                "record".to_string(),
            ],
            icon: "livetv".to_string(),
        },
        PageDocument {
            id: "kobo-proxy".to_string(),
            label: "Kobo Proxy Settings".to_string(),
            path: "/settings/kobo".to_string(),
            aliases: vec![
                "kobo".to_string(),
                "proxy".to_string(),
                "sync".to_string(),
                "entitlement".to_string(),
            ],
            icon: "kobo".to_string(),
        },
        PageDocument {
            id: "processing".to_string(),
            label: "Processing Settings".to_string(),
            path: "/settings/processing".to_string(),
            aliases: vec![
                "enrichment".to_string(),
                "thumbnails".to_string(),
                "skip detection".to_string(),
                "intro".to_string(),
                "credits".to_string(),
            ],
            icon: "processing".to_string(),
        },
    ]
}

/// Static commands registry for search indexing.
pub fn get_static_commands() -> Vec<CommandDocument> {
    vec![
        CommandDocument {
            id: "toggle-theme".to_string(),
            label: "Toggle theme".to_string(),
            action: "toggleTheme".to_string(),
            aliases: vec!["dark".to_string(), "light".to_string(), "mode".to_string()],
            icon: "theme".to_string(),
        },
        CommandDocument {
            id: "refresh-library".to_string(),
            label: "Refresh library".to_string(),
            action: "refreshLibrary".to_string(),
            aliases: vec!["reload".to_string(), "sync".to_string()],
            icon: "refresh".to_string(),
        },
        CommandDocument {
            id: "reindex-search".to_string(),
            label: "Rebuild search index".to_string(),
            action: "reindexSearch".to_string(),
            aliases: vec!["rebuild".to_string(), "index".to_string()],
            icon: "search".to_string(),
        },
        CommandDocument {
            id: "metadata-clear-cache".to_string(),
            label: "Metadata: Clear Cache".to_string(),
            action: "navigate:/settings/metadata".to_string(),
            aliases: vec!["cache".to_string(), "purge".to_string()],
            icon: "metadata".to_string(),
        },
        CommandDocument {
            id: "metadata-clear-unowned".to_string(),
            label: "Metadata: Clear Unowned Books".to_string(),
            action: "navigate:/settings/metadata".to_string(),
            aliases: vec!["unowned".to_string(), "cleanup".to_string()],
            icon: "metadata".to_string(),
        },
        CommandDocument {
            id: "metadata-write-all".to_string(),
            label: "Metadata: Write to All Files".to_string(),
            action: "navigate:/settings/metadata".to_string(),
            aliases: vec![
                "write".to_string(),
                "embed".to_string(),
                "ebook-meta".to_string(),
            ],
            icon: "metadata".to_string(),
        },
    ]
}

/// Static settings registry for search indexing.
/// Each entry maps a RuntimeConfig field to its settings page for field-level navigation.
pub fn get_static_settings() -> Vec<SettingDocument> {
    let s = |id: &str,
             field: &str,
             label: &str,
             desc: &str,
             page_path: &str,
             page_label: &str,
             aliases: &[&str]| SettingDocument {
        id: format!("setting-{id}"),
        field_name: field.to_string(),
        label: label.to_string(),
        description: desc.to_string(),
        page_path: page_path.to_string(),
        page_label: page_label.to_string(),
        aliases: aliases.iter().map(|a| a.to_string()).collect(),
    };

    vec![
        // === Kobo Proxy page ===
        s(
            "public_proxy_url",
            "public_proxy_url",
            "Public Proxy URL",
            "Full public URL that Kobo devices use to reach the proxy",
            "/settings/kobo",
            "Kobo Proxy",
            &["proxy url", "kobo url", "sync url", "device url"],
        ),
        s(
            "kobo_storeapi_base_url",
            "kobo_storeapi_base_url",
            "Kobo Store API URL",
            "Base URL for the upstream Kobo store API",
            "/settings/kobo",
            "Kobo Proxy",
            &["store api", "kobo api", "upstream"],
        ),
        s(
            "kobo_mode",
            "kobo_mode",
            "Kobo Mode",
            "Operating mode: Merge (local + upstream), Passthrough, or Local",
            "/settings/kobo",
            "Kobo Proxy",
            &["mode", "merge", "passthrough", "local", "proxy mode"],
        ),
        s(
            "proxy_logging_enabled",
            "proxy_logging_enabled",
            "Proxy Logging",
            "Enable request logging for the Kobo proxy",
            "/settings/kobo",
            "Kobo Proxy",
            &["proxy logging", "request logging"],
        ),
        s(
            "proxy_log_retention_days",
            "proxy_log_retention_days",
            "Proxy Log Retention",
            "Number of days to retain proxy request logs",
            "/settings/kobo",
            "Kobo Proxy",
            &["log retention", "proxy logs", "retention days"],
        ),
        s(
            "blob_capture_enabled",
            "blob_capture_enabled",
            "Blob Capture",
            "Enable capturing request/response blobs for debugging",
            "/settings/kobo",
            "Kobo Proxy",
            &["blob capture", "debug capture"],
        ),
        s(
            "rewrite_kobo_urls",
            "rewrite_kobo_urls",
            "Rewrite Kobo URLs",
            "Rewrite external Kobo URLs to go through the proxy",
            "/settings/kobo",
            "Kobo Proxy",
            &["rewrite urls", "url rewrite"],
        ),
        s(
            "kobo_sync_batch_size",
            "kobo_sync_batch_size",
            "Sync Batch Size",
            "Number of items per Kobo sync batch",
            "/settings/kobo",
            "Kobo Proxy",
            &["batch size", "sync batch"],
        ),
        s(
            "default_entitlement_mode",
            "default_entitlement_mode",
            "Default Entitlement Mode",
            "Default ebook entitlement for new books: all_devices or none",
            "/settings/kobo",
            "Kobo Proxy",
            &["entitlement", "device access", "ebook access"],
        ),
        // === General page ===
        s(
            "public_url",
            "public_url",
            "Public URL",
            "Externally reachable base URL for this Kobbler, used for links opened outside the app",
            "/settings/general",
            "General",
            &[
                "public url",
                "base url",
                "external url",
                "hostname",
                "link base",
            ],
        ),
        s(
            "default_page_size",
            "default_page_size",
            "Default Page Size",
            "Default number of items per page in API responses",
            "/settings/general",
            "General",
            &["page size", "pagination", "items per page"],
        ),
        s(
            "max_page_size",
            "max_page_size",
            "Max Page Size",
            "Maximum allowed page size for API responses",
            "/settings/general",
            "General",
            &["max page", "pagination limit"],
        ),
        s(
            "activity_limit",
            "activity_limit",
            "Activity Limit",
            "Maximum activity records per query",
            "/settings/general",
            "General",
            &["activity", "log limit"],
        ),
        s(
            "watcher_enabled",
            "watcher_enabled",
            "File Watcher",
            "Enable filesystem watching for new media files",
            "/settings/general",
            "General",
            &["watcher", "file watch", "inotify", "auto scan"],
        ),
        s(
            "watcher_scan_interval_seconds",
            "watcher_scan_interval_seconds",
            "Watcher Scan Interval",
            "Fallback scan interval in seconds for filesystems without inotify",
            "/settings/general",
            "General",
            &["scan interval", "watcher interval", "poll interval"],
        ),
        s(
            "video_storage_root",
            "video_storage_root",
            "Video Storage Root",
            "Root directory for video files",
            "/settings/general",
            "General",
            &["video root", "video directory", "video path"],
        ),
        s(
            "transcode_cache_dir",
            "transcode_cache_dir",
            "Transcode Cache Directory",
            "Directory for temporary transcode segments",
            "/settings/general",
            "General",
            &["transcode cache", "hls cache", "segment cache"],
        ),
        s(
            "cache_dir",
            "cache_dir",
            "Cache Directory",
            "General-purpose cache directory",
            "/settings/general",
            "General",
            &["cache", "cache path", "cache directory"],
        ),
        s(
            "ebook_convert_path",
            "ebook_convert_path",
            "ebook-convert Path",
            "Path to Calibre ebook-convert tool",
            "/settings/general",
            "General",
            &["ebook-convert", "calibre", "conversion tool"],
        ),
        s(
            "ebook_meta_path",
            "ebook_meta_path",
            "ebook-meta Path",
            "Path to Calibre ebook-meta tool",
            "/settings/general",
            "General",
            &["ebook-meta", "calibre meta"],
        ),
        // === Processing page ===
        s(
            "enrichment_enabled",
            "enrichment_enabled",
            "Media Enrichment",
            "Enable media enrichment (thumbnails, LUFS analysis)",
            "/settings/processing",
            "Processing",
            &["enrichment", "thumbnails", "lufs"],
        ),
        s(
            "skip_detect_mode",
            "skip_detect_mode",
            "Skip Detection Mode",
            "Intro/credits/recap skip detection: off, audio, or full",
            "/settings/processing",
            "Processing",
            &["skip detection", "intro skip", "credits skip", "recap"],
        ),
        s(
            "vision_service_url",
            "vision_service_url",
            "Vision Service URL",
            "URL for the vision service used in visual skip detection",
            "/settings/processing",
            "Processing",
            &["vision service", "visual analysis"],
        ),
        // === Audio page ===
        s(
            "transcript_mode",
            "transcript_mode",
            "Transcription Mode",
            "Transcription mode: off, auto, or manual",
            "/settings/audio",
            "Audio & Speech",
            &["transcription", "whisper", "stt mode"],
        ),
        s(
            "stt_service_url",
            "stt_service_url",
            "STT Service URL",
            "Speech-to-text service URL (OpenAI-compatible)",
            "/settings/audio",
            "Audio & Speech",
            &["stt", "speech to text", "whisper url"],
        ),
        s(
            "tts_enabled",
            "tts_enabled",
            "Text-to-Speech",
            "Enable TTS read-along feature",
            "/settings/audio",
            "Audio & Speech",
            &["tts", "text to speech", "read along"],
        ),
        s(
            "tts_service_url",
            "tts_service_url",
            "TTS Service URL",
            "OpenAI-compatible TTS service URL",
            "/settings/audio",
            "Audio & Speech",
            &["tts url", "kokoro url"],
        ),
        s(
            "tts_service_type",
            "tts_service_type",
            "TTS Service Type",
            "TTS service type: auto, kokoro, chatterbox, or openai",
            "/settings/audio",
            "Audio & Speech",
            &["tts type", "kokoro", "chatterbox"],
        ),
        s(
            "tts_default_voice",
            "tts_default_voice",
            "Default TTS Voice",
            "Default voice ID for text-to-speech",
            "/settings/audio",
            "Audio & Speech",
            &["tts voice", "voice id", "default voice"],
        ),
        s(
            "tts_default_temperature",
            "tts_default_temperature",
            "TTS Temperature",
            "Default temperature/variation for TTS (0.0-1.5)",
            "/settings/audio",
            "Audio & Speech",
            &["tts temperature", "voice variation"],
        ),
        s(
            "hw_encoder",
            "hw_encoder",
            "Hardware Encoder",
            "Hardware encoder preference: auto, vaapi, nvenc, or none",
            "/settings/audio",
            "Audio & Speech",
            &["hardware encoder", "vaapi", "nvenc", "gpu encoder"],
        ),
        s(
            "gpu_preempt_tts",
            "gpu_preempt_tts",
            "GPU Preempt TTS",
            "Whether TTS preempts STT when sharing a GPU",
            "/settings/audio",
            "Audio & Speech",
            &["gpu preempt", "gpu sharing", "tts priority"],
        ),
        // === Search & AI page ===
        s(
            "meili_url",
            "meili_url",
            "MeiliSearch URL",
            "MeiliSearch instance URL for full-text search",
            "/settings/search",
            "Search & AI",
            &["meilisearch", "search url", "meili"],
        ),
        s(
            "meili_master_key",
            "meili_master_key",
            "MeiliSearch Master Key",
            "MeiliSearch admin API key",
            "/settings/search",
            "Search & AI",
            &["meili key", "search key"],
        ),
        s(
            "chat_enabled",
            "chat_enabled",
            "Chat",
            "Enable conversational search feature",
            "/settings/search",
            "Search & AI",
            &["chat", "ai chat", "conversational search"],
        ),
        s(
            "chat_llm_provider",
            "chat_llm_provider",
            "Chat LLM Provider",
            "LLM provider: gemini, openai, or custom",
            "/settings/search",
            "Search & AI",
            &["llm provider", "gemini", "openai"],
        ),
        s(
            "chat_llm_base_url",
            "chat_llm_base_url",
            "Chat LLM Base URL",
            "Base URL for the LLM API",
            "/settings/search",
            "Search & AI",
            &["llm url", "chat url"],
        ),
        s(
            "chat_llm_api_key",
            "chat_llm_api_key",
            "Chat LLM API Key",
            "API key for the LLM provider",
            "/settings/search",
            "Search & AI",
            &["llm key", "chat key"],
        ),
        s(
            "chat_llm_model",
            "chat_llm_model",
            "Chat LLM Model",
            "Model name for chat (e.g. gemini-2.5-flash-lite)",
            "/settings/search",
            "Search & AI",
            &["llm model", "chat model"],
        ),
        s(
            "koblem_service_url",
            "koblem_service_url",
            "Koblem Service URL",
            "Local LLM service URL for Koblem features",
            "/settings/search",
            "Search & AI",
            &["koblem", "local llm", "llm service"],
        ),
        // === Obtain page ===
        s(
            "wanted_default_provider",
            "wanted_default_provider",
            "Default Wanted Provider",
            "Default metadata provider for Wanted searches",
            "/settings/obtain",
            "Obtain",
            &["wanted provider", "default provider", "search provider"],
        ),
        s(
            "wanted_min_score",
            "wanted_min_score",
            "Wanted Match Score",
            "Minimum fuzzy match score for watch patterns (0.0-1.0)",
            "/settings/obtain",
            "Obtain",
            &["match score", "fuzzy match", "min score"],
        ),
        s(
            "kobload_base_url",
            "kobload_base_url",
            "Kobload Base URL",
            "Kobload service URL for download orchestration",
            "/settings/obtain",
            "Obtain",
            &["kobload url", "download service"],
        ),
        s(
            "kobload_api_key",
            "kobload_api_key",
            "Kobload API Key",
            "API key for Kobload integration",
            "/settings/obtain",
            "Obtain",
            &["kobload key"],
        ),
        s(
            "kobload_ingest_url",
            "kobload_ingest_url",
            "Kobload Ingest URL",
            "Full ingest URL with token for Kobload callbacks",
            "/settings/obtain",
            "Obtain",
            &["ingest url", "callback url"],
        ),
        s(
            "kobload_communication_mode",
            "kobload_communication_mode",
            "Kobload Communication Mode",
            "Communication protocol: nats (default) or http",
            "/settings/obtain",
            "Obtain",
            &["communication mode", "nats", "http mode"],
        ),
        // === Streaming page ===
        s(
            "preferred_audio_language",
            "preferred_audio_language",
            "Preferred Audio Language",
            "Default audio language for playback",
            "/settings/streaming",
            "Streaming",
            &["audio language", "playback language"],
        ),
        s(
            "preferred_subtitle_language",
            "preferred_subtitle_language",
            "Preferred Subtitle Language",
            "Default subtitle language for playback",
            "/settings/streaming",
            "Streaming",
            &["subtitle language", "subtitles default"],
        ),
        s(
            "debrid_cache_dir",
            "debrid_cache_dir",
            "Debrid Cache Directory",
            "Directory for debrid download cache",
            "/settings/streaming",
            "Streaming",
            &["debrid cache", "cache directory"],
        ),
        s(
            "debrid_cache_ttl_secs",
            "debrid_cache_ttl_secs",
            "Debrid Cache TTL",
            "Time-to-live for debrid cache entries",
            "/settings/streaming",
            "Streaming",
            &["cache ttl", "cache expiry"],
        ),
        s(
            "debrid_cache_max_bytes",
            "debrid_cache_max_bytes",
            "Debrid Cache Max Size",
            "Maximum debrid cache size",
            "/settings/streaming",
            "Streaming",
            &["cache size", "max cache"],
        ),
        // === Cast page ===
        s(
            "cast_receiver_app_id",
            "cast_receiver_app_id",
            "Cast Receiver App ID",
            "Google Cast receiver application ID",
            "/settings/cast",
            "Cast",
            &["chromecast", "cast app", "receiver id", "google cast"],
        ),
        // === Metadata page ===
        s(
            "metadata_providers_enabled",
            "metadata_providers_enabled",
            "Metadata Providers",
            "Enable automatic metadata lookups from providers",
            "/settings/metadata",
            "Metadata",
            &["metadata enabled", "provider toggle", "auto metadata"],
        ),
        s(
            "metadata_cache_days",
            "metadata_cache_days",
            "Metadata Cache Duration",
            "Number of days to cache metadata lookups",
            "/settings/metadata",
            "Metadata",
            &["metadata cache", "cache days", "cache duration"],
        ),
        // === Discover page ===
        s(
            "discover_region",
            "discover_region",
            "Discover Region",
            "ISO region code for streaming service recommendations",
            "/settings/discover",
            "Discover",
            &["region", "country", "locale"],
        ),
        s(
            "discover_streaming_services",
            "discover_streaming_services",
            "Streaming Services",
            "Streaming services to include in discover recommendations",
            "/settings/discover",
            "Discover",
            &["netflix", "streaming", "services"],
        ),
        // === Migration page ===
        s(
            "sonarr_url",
            "sonarr_url",
            "Sonarr URL",
            "Sonarr API URL for library migration",
            "/settings/migration",
            "Migration",
            &["sonarr", "sonarr url"],
        ),
        s(
            "sonarr_api_key",
            "sonarr_api_key",
            "Sonarr API Key",
            "Sonarr API key for authentication",
            "/settings/migration",
            "Migration",
            &["sonarr key"],
        ),
        s(
            "radarr_url",
            "radarr_url",
            "Radarr URL",
            "Radarr API URL for library migration",
            "/settings/migration",
            "Migration",
            &["radarr", "radarr url"],
        ),
        s(
            "radarr_api_key",
            "radarr_api_key",
            "Radarr API Key",
            "Radarr API key for authentication",
            "/settings/migration",
            "Migration",
            &["radarr key"],
        ),
        // === IPTV page ===
        s(
            "iptv_language_filter",
            "iptv_language_filter",
            "IPTV Language Filter",
            "Language codes to show by default in IPTV channel listings",
            "/settings/iptv",
            "IPTV",
            &["iptv language", "channel language", "language filter"],
        ),
        s(
            "iptv_auto_create_channels",
            "iptv_auto_create_channels",
            "Auto-Create Channels",
            "Automatically create channels for unmatched streams during sync",
            "/settings/iptv",
            "IPTV",
            &["auto create", "channel creation"],
        ),
        s(
            "iptv_health_check_enabled",
            "iptv_health_check_enabled",
            "IPTV Health Checks",
            "Enable periodic IPTV source health checks",
            "/settings/iptv",
            "IPTV",
            &["health check", "source monitoring"],
        ),
        // === Live TV page ===
        s(
            "livetv_guide_source",
            "livetv_guide_source",
            "Live TV Guide Source",
            "Where the EPG comes from: the tuner's own feed, SiliconDust DVR or XMLTV",
            "/settings/livetv",
            "Live TV",
            &["guide source", "epg source", "listings"],
        ),
        s(
            "livetv_buffer_policy",
            "livetv_buffer_policy",
            "Live TV Buffer Policy",
            "How much of a live channel is retained for pause and rewind",
            "/settings/livetv",
            "Live TV",
            &["timeshift", "pause buffer", "rewind"],
        ),
        s(
            "livetv_buffer_cap_hours",
            "livetv_buffer_cap_hours",
            "Live TV Buffer Cap",
            "Hard ceiling on retained live buffer, in hours",
            "/settings/livetv",
            "Live TV",
            &["buffer size", "retention"],
        ),
        s(
            "livetv_clash_policy",
            "livetv_clash_policy",
            "Live TV Tuner Clash Policy",
            "What a viewer sees when a recording claims their tuner",
            "/settings/livetv",
            "Live TV",
            &["tuner clash", "conflict"],
        ),
        s(
            "livetv_ad_posture",
            "livetv_ad_posture",
            "Live TV Ad Marking",
            "How detected ad breaks are surfaced during playback",
            "/settings/livetv",
            "Live TV",
            &["ads", "commercials", "ad skip"],
        ),
        s(
            "livetv_ad_disable_genres",
            "livetv_ad_disable_genres",
            "Live TV Ad Detection Exclusions",
            "Genres where ad detection is suppressed outright",
            "/settings/livetv",
            "Live TV",
            &["ad genres", "news", "sports"],
        ),
        s(
            "livetv_ad_train_from_corrections",
            "livetv_ad_train_from_corrections",
            "Learn From Ad Corrections",
            "Seed the ad fingerprint bank when a user corrects a break boundary",
            "/settings/livetv",
            "Live TV",
            &["ad training", "fingerprints"],
        ),
        s(
            "livetv_recording_path",
            "livetv_recording_path",
            "Live TV Recording Path",
            "Where Live TV recordings are written",
            "/settings/livetv",
            "Live TV",
            &["dvr path", "recording directory"],
        ),
        s(
            "livetv_deinterlace_web",
            "livetv_deinterlace_web",
            "Deinterlace Live TV For Web",
            "Deinterlace 1080i DVB-T sources on the web transcode path",
            "/settings/livetv",
            "Live TV",
            &["deinterlace", "interlaced", "combing"],
        ),
        s(
            "livetv_transcode_height",
            "livetv_transcode_height",
            "Live TV Transcode Height",
            "Target height for the Live TV web transcode path",
            "/settings/livetv",
            "Live TV",
            &["resolution", "transcode height"],
        ),
        // === Jobs page ===
        s(
            "job_concurrency",
            "job_concurrency",
            "Job Concurrency",
            "Per-job-type concurrency limits (1-32 workers)",
            "/settings/jobs",
            "Job Concurrency",
            &["concurrency", "workers", "parallelism", "threads"],
        ),
    ]
}

pub struct SearchReindexHandler {
    pool: PgPool,
    search: Arc<dyn SearchBackend>,
}

impl SearchReindexHandler {
    pub fn new(pool: PgPool, search: Arc<dyn SearchBackend>) -> Self {
        Self { pool, search }
    }
}

#[async_trait]
impl JobHandler for SearchReindexHandler {
    fn job_type(&self) -> &'static str {
        "search_reindex"
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: SearchReindexPayload = serde_json::from_value(payload).unwrap_or_default();
        nats_jobs::check_cancelled(&cancel)?;

        info!(job_id = %job_id, "Starting full search reindex");

        let should_index_all = payload.entity_types.is_none();
        let types = payload.entity_types.unwrap_or_default();

        let mut books_count = 0usize;
        let mut authors_count = 0usize;
        let mut series_count = 0usize;
        let mut genres_count = 0usize;
        let mut movies_count = 0usize;
        let mut tv_shows_count = 0usize;
        let mut tv_seasons_count = 0usize;
        let mut tv_episodes_count = 0usize;
        let mut people_count = 0usize;
        let mut media_root_files_count = 0usize;

        // Reindex books
        if should_index_all || types.iter().any(|t| t == "book" || t == "books") {
            let docs = search::fetch_all_book_docs(&self.pool).await.map_err(|e| {
                error!(error = %e, "Failed to fetch books for reindex");
                JobError::retryable(format!("Failed to fetch books: {}", e))
            })?;

            books_count = self.search.reindex_all_books(docs).await.map_err(|e| {
                error!(error = %e, "Failed to reindex books");
                JobError::retryable(format!("Failed to reindex books: {}", e))
            })?;
            info!(count = books_count, "Reindexed books");
        }

        // Reindex authors
        if should_index_all || types.iter().any(|t| t == "author" || t == "authors") {
            let docs = search::fetch_all_author_docs(&self.pool)
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to fetch authors for reindex");
                    JobError::retryable(format!("Failed to fetch authors: {}", e))
                })?;

            authors_count = self.search.reindex_all_authors(docs).await.map_err(|e| {
                error!(error = %e, "Failed to reindex authors");
                JobError::retryable(format!("Failed to reindex authors: {}", e))
            })?;
            info!(count = authors_count, "Reindexed authors");
        }

        // Reindex series
        if should_index_all || types.iter().any(|t| t == "series") {
            let docs = search::fetch_all_series_docs(&self.pool)
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to fetch series for reindex");
                    JobError::retryable(format!("Failed to fetch series: {}", e))
                })?;

            series_count = self.search.reindex_all_series(docs).await.map_err(|e| {
                error!(error = %e, "Failed to reindex series");
                JobError::retryable(format!("Failed to reindex series: {}", e))
            })?;
            info!(count = series_count, "Reindexed series");
        }

        // Reindex genres
        if should_index_all || types.iter().any(|t| t == "genre" || t == "genres") {
            let docs = search::fetch_all_genre_docs(&self.pool)
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to fetch genres for reindex");
                    JobError::retryable(format!("Failed to fetch genres: {}", e))
                })?;

            genres_count = self.search.reindex_all_genres(docs).await.map_err(|e| {
                error!(error = %e, "Failed to reindex genres");
                JobError::retryable(format!("Failed to reindex genres: {}", e))
            })?;
            info!(count = genres_count, "Reindexed genres");
        }

        // Reindex movies
        if should_index_all || types.iter().any(|t| t == "movie" || t == "movies") {
            let docs = search::fetch_all_movie_docs(&self.pool)
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to fetch movies for reindex");
                    JobError::retryable(format!("Failed to fetch movies: {}", e))
                })?;

            movies_count = self.search.reindex_all_movies(docs).await.map_err(|e| {
                error!(error = %e, "Failed to reindex movies");
                JobError::retryable(format!("Failed to reindex movies: {}", e))
            })?;
            info!(count = movies_count, "Reindexed movies");
        }

        // Reindex TV shows
        if should_index_all || types.iter().any(|t| t == "tv_show" || t == "tv_shows") {
            let docs = search::fetch_all_tv_show_docs(&self.pool)
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to fetch TV shows for reindex");
                    JobError::retryable(format!("Failed to fetch TV shows: {}", e))
                })?;

            tv_shows_count = self.search.reindex_all_tv_shows(docs).await.map_err(|e| {
                error!(error = %e, "Failed to reindex TV shows");
                JobError::retryable(format!("Failed to reindex TV shows: {}", e))
            })?;
            info!(count = tv_shows_count, "Reindexed TV shows");
        }

        // Reindex TV seasons
        if should_index_all || types.iter().any(|t| t == "tv_season" || t == "tv_seasons") {
            let docs = search::fetch_all_tv_season_docs(&self.pool)
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to fetch TV seasons for reindex");
                    JobError::retryable(format!("Failed to fetch TV seasons: {}", e))
                })?;

            tv_seasons_count = self
                .search
                .reindex_all_tv_seasons(docs)
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to reindex TV seasons");
                    JobError::retryable(format!("Failed to reindex TV seasons: {}", e))
                })?;
            info!(count = tv_seasons_count, "Reindexed TV seasons");
        }

        // Reindex TV episodes
        if should_index_all
            || types
                .iter()
                .any(|t| t == "tv_episode" || t == "tv_episodes")
        {
            let docs = search::fetch_all_tv_episode_docs(&self.pool)
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to fetch TV episodes for reindex");
                    JobError::retryable(format!("Failed to fetch TV episodes: {}", e))
                })?;

            tv_episodes_count = self
                .search
                .reindex_all_tv_episodes(docs)
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to reindex TV episodes");
                    JobError::retryable(format!("Failed to reindex TV episodes: {}", e))
                })?;
            info!(count = tv_episodes_count, "Reindexed TV episodes");
        }

        // Reindex people
        if should_index_all || types.iter().any(|t| t == "person" || t == "people") {
            let docs = search::fetch_all_people_docs(&self.pool)
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to fetch people for reindex");
                    JobError::retryable(format!("Failed to fetch people: {}", e))
                })?;

            people_count = self.search.reindex_all_people(docs).await.map_err(|e| {
                error!(error = %e, "Failed to reindex people");
                JobError::retryable(format!("Failed to reindex people: {}", e))
            })?;
            info!(count = people_count, "Reindexed people");
        }

        // Reindex media root files
        if should_index_all
            || types
                .iter()
                .any(|t| t == "media_root_file" || t == "media_root_files")
        {
            let docs = search::fetch_all_media_root_file_docs(&self.pool)
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to fetch media root files for reindex");
                    JobError::retryable(format!("Failed to fetch media root files: {}", e))
                })?;

            media_root_files_count = self
                .search
                .reindex_all_media_root_files(docs)
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to reindex media root files");
                    JobError::retryable(format!("Failed to reindex media root files: {}", e))
                })?;
            info!(count = media_root_files_count, "Reindexed media root files");
        }

        // Reindex IPTV channels
        let mut iptv_channels_count = 0usize;
        if should_index_all
            || types
                .iter()
                .any(|t| t == "iptv_channel" || t == "iptv_channels")
        {
            let docs = search::fetch_all_iptv_channel_docs(&self.pool)
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to fetch IPTV channels for reindex");
                    JobError::retryable(format!("Failed to fetch IPTV channels: {}", e))
                })?;
            iptv_channels_count =
                self.search
                    .reindex_all_iptv_channels(docs)
                    .await
                    .map_err(|e| {
                        error!(error = %e, "Failed to reindex IPTV channels");
                        JobError::retryable(format!("Failed to reindex IPTV channels: {}", e))
                    })?;
        }

        // Reindex IPTV EPG
        let mut iptv_epg_count = 0usize;
        if should_index_all
            || types
                .iter()
                .any(|t| t == "iptv_epg" || t == "iptv_channels")
        {
            let docs = search::fetch_all_iptv_epg_docs(&self.pool)
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to fetch IPTV EPG for reindex");
                    JobError::retryable(format!("Failed to fetch IPTV EPG: {}", e))
                })?;
            iptv_epg_count = self.search.reindex_all_iptv_epg(docs).await.map_err(|e| {
                error!(error = %e, "Failed to reindex IPTV EPG");
                JobError::retryable(format!("Failed to reindex IPTV EPG: {}", e))
            })?;
        }

        // Reindex Live TV channels
        let mut livetv_channels_count = 0usize;
        if should_index_all
            || types
                .iter()
                .any(|t| t == "livetv_channel" || t == "livetv_channels")
        {
            let docs = search::fetch_all_livetv_channel_docs(&self.pool)
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to fetch Live TV channels for reindex");
                    JobError::retryable(format!("Failed to fetch Live TV channels: {}", e))
                })?;
            livetv_channels_count = self
                .search
                .reindex_all_livetv_channels(docs)
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to reindex Live TV channels");
                    JobError::retryable(format!("Failed to reindex Live TV channels: {}", e))
                })?;
        }

        // Reindex Live TV guide
        let mut livetv_guide_count = 0usize;
        if should_index_all
            || types
                .iter()
                .any(|t| t == "livetv_guide" || t == "livetv_channels")
        {
            let docs = search::fetch_all_livetv_guide_docs(&self.pool)
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to fetch Live TV guide for reindex");
                    JobError::retryable(format!("Failed to fetch Live TV guide: {}", e))
                })?;
            livetv_guide_count = self
                .search
                .reindex_all_livetv_guide(docs)
                .await
                .map_err(|e| {
                    error!(error = %e, "Failed to reindex Live TV guide");
                    JobError::retryable(format!("Failed to reindex Live TV guide: {}", e))
                })?;
        }

        // Reindex transcript segments (audio transcripts + ebook text)
        // Only when explicitly requested — transcripts are expensive and rarely change
        let mut transcript_books_count = 0usize;
        if types
            .iter()
            .any(|t| t == "transcript" || t == "transcripts")
        {
            // Find all books that have audio transcripts or ebook segments
            let book_ids: Vec<uuid::Uuid> = sqlx::query_scalar!(
                r#"
                SELECT DISTINCT af.book_id AS "book_id!"
                FROM audiobook_files af
                WHERE EXISTS (
                    SELECT 1 FROM audiobook_transcripts at WHERE at.audiobook_file_id = af.id
                )
                UNION
                SELECT DISTINCT bf.book_id
                FROM book_files bf
                WHERE EXISTS (
                    SELECT 1 FROM ebook_segments es WHERE es.book_file_id = bf.id
                )
                "#
            )
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to fetch books for transcript reindex");
                JobError::retryable(format!("Failed to fetch transcript books: {}", e))
            })?;

            for book_id in &book_ids {
                if let Err(e) = search::index_transcript_segments_for_book(
                    &self.pool,
                    self.search.as_ref(),
                    *book_id,
                )
                .await
                {
                    warn!(error = %e, book_id = %book_id, "Failed to reindex transcript segments for book");
                } else {
                    transcript_books_count += 1;
                }
            }
            info!(
                count = transcript_books_count,
                "Reindexed transcript segments"
            );
        }

        // Always reindex static content (pages, commands, and settings)
        let pages = get_static_pages();
        let commands = get_static_commands();
        let settings = get_static_settings();
        let pages_count = pages.len();
        let commands_count = commands.len();
        let settings_count = settings.len();

        self.search.reindex_all_pages(pages).await.map_err(|e| {
            error!(error = %e, "Failed to reindex pages");
            JobError::retryable(format!("Failed to reindex pages: {}", e))
        })?;
        info!(count = pages_count, "Reindexed pages");

        self.search
            .reindex_all_commands(commands)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to reindex commands");
                JobError::retryable(format!("Failed to reindex commands: {}", e))
            })?;
        info!(count = commands_count, "Reindexed commands");

        self.search
            .reindex_all_settings(settings)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to reindex settings");
                JobError::retryable(format!("Failed to reindex settings: {}", e))
            })?;
        info!(count = settings_count, "Reindexed settings");

        // Record successful reindex: update schema version and timestamp
        if let Err(e) = crate::repositories::search_meta::set_schema_version(
            &self.pool,
            search::SEARCH_SCHEMA_VERSION,
        )
        .await
        {
            error!(error = %e, "Failed to update search schema version");
        }
        if let Err(e) = crate::repositories::search_meta::record_reindex(&self.pool).await {
            error!(error = %e, "Failed to record reindex timestamp");
        }

        info!(
            books = books_count,
            authors = authors_count,
            series = series_count,
            genres = genres_count,
            movies = movies_count,
            tv_shows = tv_shows_count,
            tv_seasons = tv_seasons_count,
            tv_episodes = tv_episodes_count,
            people = people_count,
            media_root_files = media_root_files_count,
            iptv_channels = iptv_channels_count,
            iptv_epg = iptv_epg_count,
            livetv_channels = livetv_channels_count,
            livetv_guide = livetv_guide_count,
            transcript_books = transcript_books_count,
            pages = pages_count,
            commands = commands_count,
            settings = settings_count,
            schema_version = search::SEARCH_SCHEMA_VERSION,
            "Full search reindex completed"
        );

        Ok(json!({
            "reindexed": true,
            "books_count": books_count,
            "authors_count": authors_count,
            "series_count": series_count,
            "genres_count": genres_count,
            "movies_count": movies_count,
            "tv_shows_count": tv_shows_count,
            "tv_seasons_count": tv_seasons_count,
            "tv_episodes_count": tv_episodes_count,
            "people_count": people_count,
            "media_root_files_count": media_root_files_count,
            "iptv_channels_count": iptv_channels_count,
            "iptv_epg_count": iptv_epg_count,
            "livetv_channels_count": livetv_channels_count,
            "livetv_guide_count": livetv_guide_count,
            "transcript_books_count": transcript_books_count,
            "pages_count": pages_count,
            "commands_count": commands_count,
            "settings_count": settings_count
        }))
    }
}
