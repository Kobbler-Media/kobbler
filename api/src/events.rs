use serde::Serialize;
use uuid::Uuid;

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LibraryEvent {
    LibraryUpdated {
        #[serde(skip_serializing_if = "Option::is_none")]
        book_id: Option<Uuid>,
    },
    /// An author was updated (for search index)
    AuthorUpdated {
        author_id: Uuid,
    },
    /// A series was updated (for search index)
    SeriesUpdated {
        series_id: Uuid,
    },
    ProxyLogCreated {
        log_id: Uuid,
    },
    /// Full snapshot of Kobload downloads (sent on SSE connect)
    KobloadDownloadSnapshot {
        downloads: Vec<serde_json::Value>,
    },
    /// A Kobload download was created
    KobloadDownloadCreated {
        download: serde_json::Value,
    },
    /// A Kobload download was updated
    KobloadDownloadUpdated {
        download: serde_json::Value,
    },
    /// A movie was updated (enrichment complete)
    MovieUpdated {
        movie_id: Uuid,
    },
    /// A TV show was updated (enrichment complete)
    TvShowUpdated {
        show_id: Uuid,
    },
    /// A TV season was updated
    TvSeasonUpdated {
        season_id: Uuid,
    },
    /// A TV episode was updated
    TvEpisodeUpdated {
        episode_id: Uuid,
    },
    /// A wanted item's status was updated
    WantedStatusUpdated {
        wanted_id: Uuid,
        status: String,
        /// Whether this is news, or only a screen keeping up.
        ///
        /// A season pack moves every episode it carries at once. The episodes are
        /// what the UI draws, so every one of them still has to be broadcast — but
        /// one download is one piece of news, not thirteen, and the notification
        /// dispatcher is the consumer that has to know the difference. The pack
        /// announces itself once, as the season.
        announce: bool,
        /// True when this acquisition replaced an older file (a quality upgrade).
        ///
        /// The entity was already announced when it first arrived; an agent that
        /// keeps a per-entity message map edits that post in place ("… (Upgraded)")
        /// instead of announcing the same item twice.
        upgraded: bool,
    },
    /// An in-app notification was created
    NotificationCreated,
    /// Activity log was updated (new entry added)
    ActivityLogUpdated,
    /// Skip detection finished for a file and persisted its segments.
    ///
    /// Carries the segments rather than just the id: a player is typically
    /// already mid-playback when this lands (detection runs minutes after a
    /// file is ingested, and people press play the moment it appears), and it
    /// only fetched skip segments once, at session creation. Sending them here
    /// lets an open player adopt them with no refetch and no reload.
    SkipSegmentsReady {
        file_id: Uuid,
        /// `"episode"` or `"movie"`.
        media_type: String,
        segments: serde_json::Value,
    },
    /// A tuner lease was taken or released.
    LivetvLeaseChanged {
        channel_id: Uuid,
        tuner_id: Uuid,
        in_use: u32,
        capacity: u32,
    },
    LivetvRecordingUpdated {
        recording_id: Uuid,
        status: String,
    },
    LivetvScanProgress {
        tuner_id: Uuid,
        found: i64,
        done: bool,
    },
    LivetvGuideSynced {
        tuner_id: Uuid,
        entries: i64,
    },
    /// A stream option finished debrid cache warming and is ready for instant playback.
    StreamWarmReady {
        info_hash: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        file_id: Option<i32>,
        debrid_provider_id: String,
        debrid_provider: String,
    },
    /// One of this user's devices saved a playback position. Addressed to one user —
    /// the SSE stream drops it for everyone else. See `crate::watch_handoff`.
    WatchProgress {
        #[serde(skip)]
        user_id: Uuid,
        #[serde(flatten)]
        progress: Box<crate::watch_handoff::WatchProgress>,
    },
}

impl LibraryEvent {
    /// Returns the snake_case event kind name (matches serde tag).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::LibraryUpdated { .. } => "library_updated",
            Self::AuthorUpdated { .. } => "author_updated",
            Self::SeriesUpdated { .. } => "series_updated",
            Self::ProxyLogCreated { .. } => "proxy_log_created",
            Self::KobloadDownloadSnapshot { .. } => "kobload_download_snapshot",
            Self::KobloadDownloadCreated { .. } => "kobload_download_created",
            Self::KobloadDownloadUpdated { .. } => "kobload_download_updated",
            Self::MovieUpdated { .. } => "movie_updated",
            Self::TvShowUpdated { .. } => "tv_show_updated",
            Self::TvSeasonUpdated { .. } => "tv_season_updated",
            Self::TvEpisodeUpdated { .. } => "tv_episode_updated",
            Self::WantedStatusUpdated { .. } => "wanted_status_updated",
            Self::NotificationCreated => "notification_created",
            Self::ActivityLogUpdated => "activity_log_updated",
            Self::SkipSegmentsReady { .. } => "skip_segments_ready",
            Self::LivetvLeaseChanged { .. } => "livetv_lease_changed",
            Self::LivetvRecordingUpdated { .. } => "livetv_recording_updated",
            Self::LivetvScanProgress { .. } => "livetv_scan_progress",
            Self::LivetvGuideSynced { .. } => "livetv_guide_synced",
            Self::StreamWarmReady { .. } => "stream_warm_ready",
            Self::WatchProgress { .. } => "watch_progress",
        }
    }

    /// The one user this event is for, or None for an event every user may see.
    pub fn addressed_to(&self) -> Option<Uuid> {
        match self {
            Self::WatchProgress { user_id, .. } => Some(*user_id),
            _ => None,
        }
    }

    pub fn library_updated(book_id: Option<Uuid>) -> Self {
        Self::LibraryUpdated { book_id }
    }

    pub fn author_updated(author_id: Uuid) -> Self {
        Self::AuthorUpdated { author_id }
    }

    pub fn series_updated(series_id: Uuid) -> Self {
        Self::SeriesUpdated { series_id }
    }

    pub fn proxy_log_created(log_id: Uuid) -> Self {
        Self::ProxyLogCreated { log_id }
    }

    pub fn kobload_download_snapshot(downloads: Vec<serde_json::Value>) -> Self {
        Self::KobloadDownloadSnapshot { downloads }
    }

    pub fn kobload_download_created(download: serde_json::Value) -> Self {
        Self::KobloadDownloadCreated { download }
    }

    pub fn kobload_download_updated(download: serde_json::Value) -> Self {
        Self::KobloadDownloadUpdated { download }
    }

    pub fn movie_updated(movie_id: Uuid) -> Self {
        Self::MovieUpdated { movie_id }
    }

    pub fn tv_show_updated(show_id: Uuid) -> Self {
        Self::TvShowUpdated { show_id }
    }

    pub fn tv_season_updated(season_id: Uuid) -> Self {
        Self::TvSeasonUpdated { season_id }
    }

    pub fn tv_episode_updated(episode_id: Uuid) -> Self {
        Self::TvEpisodeUpdated { episode_id }
    }

    pub fn wanted_status_updated(wanted_id: Uuid, status: String) -> Self {
        Self::WantedStatusUpdated {
            wanted_id,
            status,
            announce: true,
            upgraded: false,
        }
    }

    /// The same update, for screens only: something larger is speaking for it.
    /// See the `announce` field above.
    pub fn wanted_status_updated_quietly(wanted_id: Uuid, status: String) -> Self {
        Self::WantedStatusUpdated {
            wanted_id,
            status,
            announce: false,
            upgraded: false,
        }
    }

    pub fn skip_segments_ready(
        file_id: Uuid,
        media_type: String,
        segments: serde_json::Value,
    ) -> Self {
        Self::SkipSegmentsReady {
            file_id,
            media_type,
            segments,
        }
    }

    pub fn stream_warm_ready(
        info_hash: String,
        file_id: Option<i32>,
        debrid_provider_id: String,
        debrid_provider: String,
    ) -> Self {
        Self::StreamWarmReady {
            info_hash,
            file_id,
            debrid_provider_id,
            debrid_provider,
        }
    }

    /// Whether this event should be buffered for batching in the SSE stream.
    /// Returns `true` for high-volume event types that benefit from deduplication.
    /// Low-volume events return `false` and are passed through immediately.
    pub fn is_batchable(&self) -> bool {
        matches!(
            self,
            Self::LibraryUpdated { .. }
                | Self::AuthorUpdated { .. }
                | Self::SeriesUpdated { .. }
                | Self::MovieUpdated { .. }
                | Self::TvShowUpdated { .. }
                | Self::TvSeasonUpdated { .. }
                | Self::TvEpisodeUpdated { .. }
                | Self::ActivityLogUpdated
        )
    }
}

/// Long-lived subscriber over the `LibraryEvent` broadcast bus.
///
/// `Lagged` is recoverable — a burst (a large import, a media-root scan) outruns the
/// receiver, `n` events are dropped and the next `recv` succeeds. Ending the task there
/// would silently kill the subscriber for the life of the process; only a closed bus,
/// which is shutdown, ends it.
pub async fn run_subscriber<F>(
    name: &'static str,
    mut rx: tokio::sync::broadcast::Receiver<LibraryEvent>,
    mut handler: F,
) where
    F: AsyncFnMut(LibraryEvent),
{
    loop {
        match rx.recv().await {
            Ok(event) => handler(event).await,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                tracing::warn!(
                    subscriber = name,
                    skipped,
                    "Event subscriber fell behind the library event bus; events dropped"
                );
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                tracing::info!(
                    subscriber = name,
                    "Library event bus closed, subscriber stopping"
                );
                return;
            }
        }
    }
}
