//! TV monitor job handler - checks monitored shows for new episodes.
//!
//! This scheduled job runs hourly to refresh season/episode data from TMDB
//! for monitored shows, and enqueues tv_want jobs based on each show's
//! auto_want_mode setting.

use crate::LibraryEvent;
use crate::images::ImageEntry;
use crate::jobs::handlers::auto_want;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::kobload::KobloadClient;
use crate::repositories::{tv_episodes, tv_seasons, tv_shows};
use crate::runtime_config::RuntimeConfigManager;
use crate::video_metadata::{
    SeasonDetails, TvShowDetails, VideoProviderRegistry, types::VideoCapability,
};
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::HashSet;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// Handler for monitoring TV shows and syncing new episodes from TMDB.
pub struct TvMonitorHandler {
    pool: PgPool,
    runtime_config: RuntimeConfigManager,
    nats_publisher: Publisher,
    library_events: broadcast::Sender<LibraryEvent>,
}

impl TvMonitorHandler {
    pub fn new(
        pool: PgPool,
        runtime_config: RuntimeConfigManager,
        nats_publisher: Publisher,
        library_events: broadcast::Sender<LibraryEvent>,
    ) -> Self {
        Self {
            pool,
            runtime_config,
            nats_publisher,
            library_events,
        }
    }
}

#[async_trait]
impl JobHandler for TvMonitorHandler {
    fn job_type(&self) -> &'static str {
        "tv_monitor"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        _payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let monitored: Vec<_> = tv_shows::list_monitored(&self.pool)
            .await
            .map_err(|e| {
                JobError::ExecutionFailed(format!("Failed to list monitored shows: {}", e))
            })?
            .into_iter()
            .filter(|s| s.content_category != "sport")
            .collect();

        if monitored.is_empty() {
            info!("No monitored TV shows, nothing to do");
            return Ok(json!({ "shows_checked": 0, "episodes_wanted": 0 }));
        }

        let registry = VideoProviderRegistry::new(self.pool.clone());
        let mut total_shows_checked = 0u32;
        let mut total_episodes_wanted = 0u32;
        let mut errors = Vec::new();

        for show in &monitored {
            nats_jobs::check_cancelled(&cancel)?;
            match self.process_show(show, &registry).await {
                Ok(wanted_count) => {
                    total_shows_checked += 1;
                    total_episodes_wanted += wanted_count;
                }
                Err(e) if e.contains("Rate limited") => {
                    // Abort the whole job — continuing will just hit more 429s.
                    // The scheduler will retry on its next run.
                    return Err(JobError::retryable(format!(
                        "Rate limited after processing {total_shows_checked} shows: {e}"
                    )));
                }
                Err(e) => {
                    warn!(show_id = %show.id, show_title = %show.title, error = %e, "Failed to process monitored show");
                    errors.push(format!("{}: {}", show.title, e));
                }
            }
        }

        // Reconcile against kobload: reset episodes whose pattern vanished, and
        // re-push seasons whose library state has drifted from what kobload holds.
        let (stale_reset, drift_refreshed) = self.reconcile_with_kobload().await;

        // Notify per-show so the frontend TV page picks it up via tv_show_updated
        if let Some(first) = monitored.first() {
            let _ = self
                .library_events
                .send(LibraryEvent::tv_show_updated(first.id));
        }

        info!(
            shows_checked = total_shows_checked,
            episodes_wanted = total_episodes_wanted,
            errors_count = errors.len(),
            "TV monitor job complete"
        );

        Ok(json!({
            "shows_checked": total_shows_checked,
            "episodes_wanted": total_episodes_wanted,
            "stale_episodes_reset": stale_reset,
            "seasons_refreshed": drift_refreshed,
            "errors": errors,
        }))
    }
}

/// Episode identifiers kobload believes are already in the library, taken from
/// the `episodes_needed` it is holding.
fn kobload_in_library_ids(media_want: &nats_jobs::MediaWant) -> Option<HashSet<String>> {
    let nats_jobs::MediaWant::TvSeason {
        episodes_needed: Some(eps),
        ..
    } = media_want
    else {
        return None;
    };
    Some(
        eps.iter()
            .filter(|er| er.in_library.unwrap_or(false))
            .map(|er| er.episode.clone())
            .collect(),
    )
}

/// Whether kobload's view of a season is out of date with kobbler's library.
///
/// kobload scores season packs and upgrades off the `in_library` flags in the
/// `episodes_needed` it was last sent. Nothing re-sends those once every episode
/// has moved to 'watching', so a season that keeps acquiring episodes leaves
/// kobload re-hunting files we already hold and computing pack ratios from a
/// stale picture.
fn library_state_drifted(
    media_want: &nats_jobs::MediaWant,
    episodes: &[tv_episodes::EpisodeRow],
) -> bool {
    let Some(kobload_ids) = kobload_in_library_ids(media_want) else {
        return false;
    };
    let kobbler_ids: HashSet<String> = episodes
        .iter()
        .filter(|ep| ep.is_in_library)
        .map(|ep| ep.episode_identifier.clone())
        .collect();
    kobbler_ids != kobload_ids
}

impl TvMonitorHandler {
    async fn process_show(
        &self,
        show: &tv_shows::MonitoredShow,
        registry: &VideoProviderRegistry,
    ) -> Result<u32, String> {
        // Resolve provider from show identifiers
        let (provider_type, provider_id) = registry
            .resolve_provider_for_identifiers(&show.identifiers, VideoCapability::TvDetails)
            .await
            .ok_or_else(|| "No enabled provider found for show identifiers".to_string())?;

        let show_details: TvShowDetails = registry
            .get_tv_show(&provider_type, &provider_id)
            .await
            .map_err(|e| format!("Failed to fetch show from {provider_type}: {e}"))?;

        let mut episodes_wanted = 0u32;

        // Process each season from TMDB
        for season_summary in &show_details.seasons {
            let season_number = season_summary.season_number;

            // Upsert season record
            let air_date = season_summary.air_date.as_deref().and_then(parse_date);

            let season_id = tv_seasons::upsert(
                &self.pool,
                tv_seasons::SeasonInput {
                    show_id: show.id,
                    season_identifier: season_number.to_string(),
                    season_sort_order: season_number as i32,
                    title: season_summary.title.clone(),
                    description: None,
                    images: season_summary.poster_url.as_ref().map(|url| {
                        let mut m = std::collections::HashMap::new();
                        m.insert(
                            "poster".to_string(),
                            ImageEntry {
                                source_url: Some(url.clone()),
                                cached_at: None,
                            },
                        );
                        m
                    }),
                    air_date,
                    episode_count: season_summary.episode_count.map(|c| c as i32),
                    is_monitored: true,
                    is_special: season_number == 0,
                    auto_want_mode: "inherit".to_string(),
                },
            )
            .await
            .map_err(|e| format!("Failed to upsert season {}: {}", season_number, e))?;

            // Fetch season details to get episodes
            let season_details: SeasonDetails = match registry
                .get_season_merged(
                    &provider_type,
                    &provider_id,
                    season_number,
                    &show.identifiers,
                )
                .await
            {
                Ok(d) => d,
                Err(crate::video_metadata::VideoProviderError::RateLimited) => {
                    return Err(format!(
                        "Rate limited while fetching season {season_number} for show {}",
                        show.id
                    ));
                }
                Err(e) => {
                    warn!(
                        show_id = %show.id,
                        season = season_number,
                        error = %e,
                        "Failed to fetch season details from {}, skipping",
                        provider_type
                    );
                    continue;
                }
            };

            // Map TMDB episodes to EpisodeInput for bulk upsert
            let episode_inputs: Vec<tv_episodes::EpisodeInput> = season_details
                .episodes
                .iter()
                .map(|ep| tv_episodes::EpisodeInput {
                    season_id,
                    episode_identifier: ep.episode_number.to_string(),
                    episode_sort_order: ep.episode_number as i32,
                    title: ep.title.clone(),
                    description: ep.description.clone(),
                    images: ep.still_url.as_ref().map(|url| {
                        let mut m = std::collections::HashMap::new();
                        m.insert(
                            "still".to_string(),
                            ImageEntry {
                                source_url: Some(url.clone()),
                                cached_at: None,
                            },
                        );
                        m
                    }),
                    air_date: ep.air_date.as_deref().and_then(parse_date),
                    air_time: ep
                        .air_time
                        .as_deref()
                        .and_then(crate::video_metadata::parse_air_time),
                    runtime: ep.runtime.map(|r| r as i32),
                    identifiers: match ep.provider_id.as_ref() {
                        Some(pid) => serde_json::json!({ format!("{provider_type}_id"): pid }),
                        None => serde_json::json!({}),
                    },
                    country: None,
                })
                .collect();

            if !episode_inputs.is_empty() {
                tv_episodes::bulk_upsert(&self.pool, season_id, episode_inputs)
                    .await
                    .map_err(|e| format!("Failed to bulk upsert episodes: {}", e))?;
            }

            // Resolve effective auto_want_mode: season overrides show if not 'inherit'
            let season_row = tv_seasons::get(&self.pool, season_id)
                .await
                .map_err(|e| format!("Failed to get season: {e}"))?;
            let effective_mode = match season_row.as_ref() {
                Some(s) => auto_want::resolve_effective_mode(s, &show.auto_want_mode),
                None => show.auto_want_mode.as_str(),
            };

            // Apply auto_want_mode using shared logic
            let wanted = auto_want::apply_auto_want(
                &self.pool,
                &self.nats_publisher,
                show.id,
                season_id,
                effective_mode,
            )
            .await
            .map_err(|e| format!("Failed to apply auto_want: {e}"))?;

            episodes_wanted += wanted;
        }

        Ok(episodes_wanted)
    }

    /// Reconcile kobbler against kobload's watch patterns, in one pattern fetch.
    ///
    /// Returns `(episodes reset to 'wanted', seasons re-pushed after library drift)`.
    async fn reconcile_with_kobload(&self) -> (u32, u32) {
        let config = self.runtime_config.get().await;
        let (Some(base_url), Some(api_key)) = (
            config.kobload_base_url.as_deref().filter(|s| !s.is_empty()),
            config.kobload_api_key.as_deref().filter(|s| !s.is_empty()),
        ) else {
            return (0, 0);
        };

        let client = KobloadClient::new(base_url.to_string(), api_key.to_string());

        // Fetch current watch patterns from kobload
        let patterns = match client.list_watch_patterns(Some("kobbler")).await {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, "Failed to list kobload watch patterns for reconciliation");
                return (0, 0);
            }
        };

        let drift_refreshed = self.refresh_drifted_seasons(&patterns).await;
        let stale_reset = self.reset_stale_watch_statuses(&patterns).await;

        (stale_reset, drift_refreshed)
    }

    /// Re-send `tv_want` for seasons whose in-library set no longer matches what
    /// kobload holds, so its pack ratios and upgrade decisions score against the
    /// real library rather than a snapshot from the last new episode.
    async fn refresh_drifted_seasons(&self, patterns: &[nats_jobs::WatchPatternResponse]) -> u32 {
        let mut refreshed = 0;

        for pattern in patterns {
            let Some(media_want) = pattern.media_want.as_ref() else {
                continue;
            };
            let Ok(season_id) = pattern.source_id.parse::<Uuid>() else {
                continue;
            };

            let episodes = match tv_episodes::list_for_season_pattern(&self.pool, season_id).await {
                Ok(eps) => eps,
                Err(e) => {
                    warn!(error = %e, season_id = %season_id, "Failed to list season episodes for drift check");
                    continue;
                }
            };

            // Drift alone is enough to justify a push. This deliberately does NOT
            // require anything outstanding or an open upgrade window: the most
            // important drift to correct is a season that has just filled up, which
            // has neither. Requiring them left Kobload's episodes_needed frozen at
            // the last pre-import state, and its backlog search re-grabbed episodes
            // already on disk. `refresh_only` below tells `tv_want` to push anyway
            // rather than take its own "nothing to do" early return.
            if episodes.is_empty() || !library_state_drifted(media_want, &episodes) {
                continue;
            }

            let Ok(Some(season)) = tv_seasons::get(&self.pool, season_id).await else {
                continue;
            };

            match auto_want::enqueue_tv_want_refresh(
                &self.nats_publisher,
                season.show_id,
                season_id,
            )
            .await
            {
                Ok(()) => {
                    refreshed += 1;
                    info!(
                        season_id = %season_id,
                        show_id = %season.show_id,
                        "Library state drifted from kobload's pattern, re-sent want"
                    );
                }
                Err(e) => {
                    warn!(error = %e, season_id = %season_id, "Failed to re-send want after library drift");
                }
            }
        }

        refreshed
    }

    /// Check episodes with 'watching'/'searching' status against kobload's
    /// actual watch patterns. If a pattern no longer exists (e.g. after a
    /// kobload data purge), reset the episode to 'wanted' so it gets
    /// re-enqueued on the next auto_want pass.
    async fn reset_stale_watch_statuses(
        &self,
        patterns: &[nats_jobs::WatchPatternResponse],
    ) -> u32 {
        // Get all episodes that think they have an active watch pattern
        let active = match tv_episodes::list_active_watch_statuses(&self.pool).await {
            Ok(eps) => eps,
            Err(e) => {
                warn!(error = %e, "Failed to list active watch status episodes for reconciliation");
                return 0;
            }
        };

        if active.is_empty() {
            return 0;
        }

        let active_source_ids: HashSet<String> =
            patterns.iter().map(|p| p.source_id.clone()).collect();

        // An episode is covered if a kobload pattern exists keyed on either its season_id
        // (TV-season patterns) or its own id (sport-event patterns use episode.id as
        // source_id). Without the per-episode check, every sport season looked "stale" —
        // its season_id never matches an episode-keyed pattern — and imported sport events
        // were repeatedly reset to 'wanted' and re-grabbed.
        let stale_episode_ids: Vec<Uuid> = active
            .iter()
            .filter(|e| {
                !active_source_ids.contains(&e.season_id.to_string())
                    && !active_source_ids.contains(&e.id.to_string())
            })
            .map(|e| e.id)
            .collect();

        if stale_episode_ids.is_empty() {
            return 0;
        }

        let count = stale_episode_ids.len() as u32;

        if let Err(e) =
            tv_episodes::set_want_status_batch(&self.pool, &stale_episode_ids, "wanted").await
        {
            warn!(error = %e, "Failed to reset stale episodes to wanted");
            return 0;
        }

        info!(
            episodes_reset = count,
            "Reset stale watching/searching episodes to wanted (kobload patterns missing)"
        );

        count
    }
}

/// Parse a date string (YYYY-MM-DD) into a time::Date.
fn parse_date(s: &str) -> Option<time::Date> {
    let format = time::format_description::well_known::Iso8601::DATE;
    time::Date::parse(s, &format).ok().or_else(|| {
        // Fallback: parse manually for YYYY-MM-DD format
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() == 3 {
            let year: i32 = parts[0].parse().ok()?;
            let month: u8 = parts[1].parse().ok()?;
            let day: u8 = parts[2].parse().ok()?;
            let month = time::Month::try_from(month).ok()?;
            time::Date::from_calendar_date(year, month, day).ok()
        } else {
            None
        }
    })
}

#[cfg(test)]
mod library_drift_tests {
    use super::*;

    fn episode(identifier: &str, in_library: bool) -> tv_episodes::EpisodeRow {
        tv_episodes::EpisodeRow {
            id: Uuid::nil(),
            season_id: Uuid::nil(),
            episode_identifier: identifier.to_string(),
            episode_sort_order: identifier.parse().unwrap_or(0),
            title: None,
            description: None,
            images: Default::default(),
            air_date: None,
            air_time: None,
            runtime: None,
            is_in_library: in_library,
            want_status: "watching".to_string(),
            identifiers: serde_json::json!({}),
            country: None,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn season_want(in_library: &[&str], all: &[&str]) -> nats_jobs::MediaWant {
        nats_jobs::MediaWant::TvSeason {
            show_name: "Million Dollar Nannies".to_string(),
            season: "1".to_string(),
            year: Some(2026),
            episodes_needed: Some(
                all.iter()
                    .map(|ep| nats_jobs::EpisodeRef {
                        season: "1".to_string(),
                        episode: ep.to_string(),
                        sort_order: ep.parse().ok(),
                        air_date: None,
                        in_library: Some(in_library.contains(ep)),
                        library_video_height: None,
                        title: None,
                    })
                    .collect(),
            ),
            total_episodes: Some(all.len() as u32),
            pack_preference: "auto".to_string(),
            last_air_date: None,
            identifiers: None,
            alternate_titles: None,
            quality_profile_id: None,
            quality_overrides: None,
        }
    }

    /// The prod case: kobload's pattern still says one episode is in the
    /// library while kobbler has since acquired three more. Nothing re-sent the
    /// want because every episode had already moved to 'watching'.
    #[test]
    fn detects_episodes_acquired_since_the_last_push() {
        let all = ["1", "2", "3", "4", "5", "6", "7", "8"];
        let media_want = season_want(&["3"], &all);
        let episodes: Vec<_> = all
            .iter()
            .map(|ep| episode(ep, ["1", "2", "3", "5"].contains(ep)))
            .collect();
        assert!(library_state_drifted(&media_want, &episodes));
    }

    #[test]
    fn agrees_when_both_sides_match() {
        let all = ["1", "2", "3"];
        let media_want = season_want(&["1", "2"], &all);
        let episodes: Vec<_> = all
            .iter()
            .map(|ep| episode(ep, ["1", "2"].contains(ep)))
            .collect();
        assert!(!library_state_drifted(&media_want, &episodes));
    }

    /// A file removed from the library is drift too — kobload should go back to
    /// wanting it rather than believing it is still held.
    #[test]
    fn detects_episodes_removed_since_the_last_push() {
        let all = ["1", "2", "3"];
        let media_want = season_want(&["1", "2"], &all);
        let episodes: Vec<_> = all.iter().map(|ep| episode(ep, *ep == "1")).collect();
        assert!(library_state_drifted(&media_want, &episodes));
    }

    /// The upgrade window is what `tv_want` consults to decide whether a season
    /// with nothing outstanding is still worth searching. The drift reconciler
    /// deliberately does not: it pushes `refresh_only` so a full season can still
    /// correct kobload's `episodes_needed`. That push is not a no-op and does not
    /// loop, because it makes `library_state_drifted` false.
    #[test]
    fn long_finished_season_has_a_closed_upgrade_window() {
        let old = time::OffsetDateTime::now_utc().date() - time::Duration::days(120);
        let episodes: Vec<_> = ["1", "2"]
            .iter()
            .map(|ep| {
                let mut e = episode(ep, false);
                e.air_date = Some(old);
                e.want_status = "watching".to_string();
                e
            })
            .collect();
        assert!(!auto_want::upgrade_window_open(&episodes));
    }

    #[test]
    fn recently_aired_season_has_an_open_upgrade_window() {
        let recent = time::OffsetDateTime::now_utc().date() - time::Duration::days(3);
        let mut ep = episode("1", true);
        ep.air_date = Some(recent);
        assert!(auto_want::upgrade_window_open(&[ep]));
    }

    #[test]
    fn ignores_wants_that_are_not_tv_seasons() {
        let media_want = nats_jobs::MediaWant::Movie {
            title: "Some Movie".to_string(),
            year: Some(2026),
            release_date: None,
            identifiers: None,
            alternate_titles: None,
            quality_profile_id: None,
            quality_overrides: None,
            in_library: None,
            library_video_height: None,
        };
        assert!(!library_state_drifted(&media_want, &[episode("1", true)]));
    }
}
