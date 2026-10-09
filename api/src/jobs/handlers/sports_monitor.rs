//! Sports monitor job handler - checks monitored leagues for new events.
//!
//! Scheduled job that fetches latest season data from TheSportsDB for
//! monitored sport leagues, upserts new events, and applies auto_want_mode.

use crate::LibraryEvent;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{tv_episodes, tv_seasons, tv_shows};
use crate::video_metadata::VideoProviderRegistry;
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// Handler for monitoring sport leagues and syncing new events from TheSportsDB.
pub struct SportsMonitorHandler {
    pool: PgPool,
    nats_publisher: Publisher,
    library_events: broadcast::Sender<LibraryEvent>,
}

impl SportsMonitorHandler {
    pub fn new(
        pool: PgPool,
        nats_publisher: Publisher,
        library_events: broadcast::Sender<LibraryEvent>,
    ) -> Self {
        Self {
            pool,
            nats_publisher,
            library_events,
        }
    }
}

#[async_trait]
impl JobHandler for SportsMonitorHandler {
    fn job_type(&self) -> &'static str {
        "sports_monitor"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        job_id: Uuid,
        _payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        info!(job_id = %job_id, "Running sports monitor job");

        let monitored = tv_shows::list_monitored(&self.pool).await.map_err(|e| {
            JobError::ExecutionFailed(format!("Failed to list monitored shows: {}", e))
        })?;

        // Only process sport leagues
        let sport_leagues: Vec<_> = monitored
            .into_iter()
            .filter(|s| s.content_category == "sport")
            .collect();

        if sport_leagues.is_empty() {
            info!("No monitored sport leagues, nothing to do");
            return Ok(json!({ "leagues_checked": 0, "events_wanted": 0 }));
        }

        let registry = VideoProviderRegistry::new(self.pool.clone());
        let mut total_checked = 0u32;
        let mut total_wanted = 0u32;
        let mut errors = Vec::new();

        for league in &sport_leagues {
            nats_jobs::check_cancelled(&cancel)?;
            match self.process_league(league, &registry).await {
                Ok(wanted_count) => {
                    total_checked += 1;
                    total_wanted += wanted_count;
                }
                Err(e) if e.contains("Rate limited") => {
                    return Err(JobError::retryable(format!(
                        "Rate limited after processing {total_checked} leagues: {e}"
                    )));
                }
                Err(e) => {
                    warn!(
                        league_id = %league.id,
                        league_title = %league.title,
                        error = %e,
                        "Failed to process monitored league"
                    );
                    errors.push(format!("{}: {}", league.title, e));
                }
            }
        }

        if let Some(first) = sport_leagues.first() {
            let _ = self
                .library_events
                .send(LibraryEvent::tv_show_updated(first.id));
        }

        info!(
            leagues_checked = total_checked,
            events_wanted = total_wanted,
            errors_count = errors.len(),
            "Sports monitor job complete"
        );

        Ok(json!({
            "leagues_checked": total_checked,
            "events_wanted": total_wanted,
            "errors": errors,
        }))
    }
}

impl SportsMonitorHandler {
    async fn process_league(
        &self,
        league: &tv_shows::MonitoredShow,
        registry: &VideoProviderRegistry,
    ) -> Result<u32, String> {
        let provider_id = league
            .identifiers
            .get("thesportsdb")
            .and_then(|v| {
                v.as_str()
                    .map(String::from)
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
            })
            .ok_or_else(|| "League has no thesportsdb in identifiers".to_string())?;

        let show_details = registry
            .get_tv_show("thesportsdb", &provider_id)
            .await
            .map_err(|e| format!("Failed to fetch from TheSportsDB: {}", e))?;

        let mut events_wanted = 0u32;

        for season_summary in &show_details.seasons {
            let season_id_str = season_summary
                .title
                .clone()
                .unwrap_or_else(|| season_summary.season_number.to_string());

            // Upsert season
            let season_id = tv_seasons::upsert(
                &self.pool,
                tv_seasons::SeasonInput {
                    show_id: league.id,
                    season_identifier: season_id_str.clone(),
                    season_sort_order: season_summary.season_number as i32,
                    title: season_summary.title.clone(),
                    description: None,
                    images: None,
                    air_date: None,
                    episode_count: season_summary.episode_count.map(|c| c as i32),
                    is_monitored: true,
                    is_special: false,
                    auto_want_mode: "inherit".to_string(),
                },
            )
            .await
            .map_err(|e| format!("Failed to upsert season {}: {}", season_id_str, e))?;

            // Fetch events for this season
            let season_details = match registry
                .get_season("thesportsdb", &provider_id, season_summary.season_number)
                .await
            {
                Ok(d) => d,
                Err(crate::video_metadata::VideoProviderError::RateLimited) => {
                    // Abort — continuing will just hit more 429s. Return error
                    // so the monitor can move on to the next league or retry.
                    return Err(format!(
                        "Rate limited while fetching season {season_id_str} events"
                    ));
                }
                Err(e) => {
                    warn!(
                        league_id = %league.id,
                        season = %season_id_str,
                        error = %e,
                        "Failed to fetch season details, skipping"
                    );
                    continue;
                }
            };

            // Upsert events
            let existing_episodes = tv_episodes::list_by_season(&self.pool, season_id)
                .await
                .map_err(|e| format!("Failed to list events: {}", e))?;

            for ep in &season_details.episodes {
                // Natural-key identifier (shared with sports_enrich) so two upstream
                // event IDs for one race collapse to a single row.
                let identifier = crate::video_metadata::thesportsdb::sport_episode_identifier(
                    ep.episode_number as i32,
                    ep.title.as_deref(),
                    ep.provider_id.as_deref(),
                );

                // Skip if already exists
                if existing_episodes
                    .iter()
                    .any(|e| e.episode_identifier == identifier)
                {
                    continue;
                }

                let air_date = ep.air_date.as_deref().and_then(parse_date);
                let ep_input = tv_episodes::EpisodeInput {
                    season_id,
                    episode_identifier: identifier,
                    episode_sort_order: ep.episode_number as i32,
                    title: ep.title.clone(),
                    description: ep.description.clone(),
                    images: None,
                    air_date,
                    air_time: None,
                    runtime: None,
                    identifiers: match ep.provider_id.as_ref() {
                        Some(pid) => serde_json::json!({ "thesportsdb_id": pid }),
                        None => serde_json::json!({}),
                    },
                    country: ep.country.clone(),
                };
                let _ = tv_episodes::create(&self.pool, ep_input).await;
            }

            // Apply auto_want_mode
            let season_row = tv_seasons::get(&self.pool, season_id)
                .await
                .map_err(|e| format!("Failed to get season: {}", e))?;
            let effective_mode = match season_row.as_ref().map(|s| s.auto_want_mode.as_str()) {
                Some("inherit") | None => league.auto_want_mode.as_str(),
                Some(mode) => mode,
            };

            let wanted = self
                .apply_auto_want(league, season_id, effective_mode)
                .await
                .map_err(|e| format!("Failed to apply auto_want: {}", e))?;

            events_wanted += wanted;
        }

        Ok(events_wanted)
    }

    async fn apply_auto_want(
        &self,
        league: &tv_shows::MonitoredShow,
        season_id: Uuid,
        effective_mode: &str,
    ) -> Result<u32, String> {
        match effective_mode {
            "none" => Ok(0),
            "new_only" => self.want_newly_aired(league, season_id).await,
            "all" => self.want_all_missing(league, season_id).await,
            "fill_gaps" => self.want_fill_gaps(league, season_id).await,
            other => {
                warn!(
                    league_id = %league.id,
                    auto_want_mode = other,
                    "Unknown auto_want_mode, treating as 'none'"
                );
                Ok(0)
            }
        }
    }

    async fn want_newly_aired(
        &self,
        league: &tv_shows::MonitoredShow,
        season_id: Uuid,
    ) -> Result<u32, String> {
        let now = time::OffsetDateTime::now_utc().date();
        let past_cutoff = now - time::Duration::days(5);
        let future_cutoff = now + time::Duration::days(5);

        let all_episodes = tv_episodes::list_by_season(&self.pool, season_id)
            .await
            .map_err(|e| format!("Failed to list events: {}", e))?;

        let to_want: Vec<Uuid> = all_episodes
            .iter()
            .filter(|ep| {
                ep.want_status == "none"
                    && !ep.is_in_library
                    && ep
                        .air_date
                        .map(|d| d >= past_cutoff && d <= future_cutoff)
                        .unwrap_or(false)
            })
            .map(|ep| ep.id)
            .collect();

        let count = to_want.len() as u32;

        if !to_want.is_empty() {
            tv_episodes::set_want_status_batch(&self.pool, &to_want, "wanted")
                .await
                .map_err(|e| format!("Failed to set want_status: {}", e))?;

            self.enqueue_sports_want(league.id, season_id, Some(to_want))
                .await?;
        }

        // Re-trigger for stuck episodes within the window. 'searching' counts as
        // stuck, not as in-flight: Kobload holds a watch pattern for it, so
        // nothing else re-pushes it, and if that pattern was built from a bad
        // search term (a country code where a place name belongs) it will never
        // match anything and the event stays 'searching' forever. Re-sending is
        // cheap — Kobload upserts the pattern by source_id — and it is the only
        // route by which a corrected want reaches an event already in flight.
        let stuck: Vec<Uuid> = all_episodes
            .iter()
            .filter(|ep| {
                matches!(ep.want_status.as_str(), "wanted" | "searching")
                    && !ep.is_in_library
                    && ep
                        .air_date
                        .is_some_and(|d| d >= past_cutoff && d <= future_cutoff)
            })
            .map(|ep| ep.id)
            .collect();
        if !stuck.is_empty() {
            self.enqueue_sports_want(league.id, season_id, Some(stuck))
                .await?;
        }

        Ok(count)
    }

    async fn want_all_missing(
        &self,
        league: &tv_shows::MonitoredShow,
        season_id: Uuid,
    ) -> Result<u32, String> {
        let missing = tv_episodes::list_missing(&self.pool, season_id)
            .await
            .map_err(|e| format!("Failed to list missing events: {}", e))?;

        let episode_ids: Vec<Uuid> = missing.iter().map(|ep| ep.id).collect();
        let count = episode_ids.len() as u32;

        if !episode_ids.is_empty() {
            tv_episodes::set_want_status_batch(&self.pool, &episode_ids, "wanted")
                .await
                .map_err(|e| format!("Failed to set want_status: {}", e))?;

            self.enqueue_sports_want(league.id, season_id, None).await?;
            return Ok(count);
        }

        // No new episodes but check for stuck 'wanted'/'searching' episodes
        // that may not be in kobload's pattern (status mismatch recovery)
        let all_episodes = tv_episodes::list_by_season(&self.pool, season_id)
            .await
            .map_err(|e| format!("Failed to list events: {}", e))?;
        let has_unsatisfied = all_episodes.iter().any(|ep| {
            (ep.want_status == "wanted" || ep.want_status == "searching") && !ep.is_in_library
        });
        if has_unsatisfied {
            self.enqueue_sports_want(league.id, season_id, None).await?;
        }

        Ok(count)
    }

    async fn want_fill_gaps(
        &self,
        league: &tv_shows::MonitoredShow,
        season_id: Uuid,
    ) -> Result<u32, String> {
        let all_episodes = tv_episodes::list_by_season(&self.pool, season_id)
            .await
            .map_err(|e| format!("Failed to list events: {}", e))?;

        let in_library_orders: Vec<i32> = all_episodes
            .iter()
            .filter(|ep| ep.is_in_library)
            .map(|ep| ep.episode_sort_order)
            .collect();

        // 'searching' is stuck too, not in-flight — see want_newly_aired.
        let stuck: Vec<Uuid> = all_episodes
            .iter()
            .filter(|ep| {
                matches!(ep.want_status.as_str(), "wanted" | "searching") && !ep.is_in_library
            })
            .map(|ep| ep.id)
            .collect();

        if in_library_orders.len() < 2 {
            if !stuck.is_empty() {
                self.enqueue_sports_want(league.id, season_id, Some(stuck))
                    .await?;
            }
            return Ok(0);
        }

        let min_order = *in_library_orders.iter().min().unwrap();
        let max_order = *in_library_orders.iter().max().unwrap();

        let to_want: Vec<Uuid> = all_episodes
            .iter()
            .filter(|ep| {
                ep.episode_sort_order >= min_order
                    && ep.episode_sort_order <= max_order
                    && ep.want_status == "none"
                    && !ep.is_in_library
            })
            .map(|ep| ep.id)
            .collect();

        let count = to_want.len() as u32;

        if !to_want.is_empty() {
            tv_episodes::set_want_status_batch(&self.pool, &to_want, "wanted")
                .await
                .map_err(|e| format!("Failed to set want_status: {}", e))?;

            self.enqueue_sports_want(league.id, season_id, Some(to_want))
                .await?;
        } else if !stuck.is_empty() {
            self.enqueue_sports_want(league.id, season_id, Some(stuck))
                .await?;
        }

        Ok(count)
    }

    async fn enqueue_sports_want(
        &self,
        show_id: Uuid,
        season_id: Uuid,
        episode_ids: Option<Vec<Uuid>>,
    ) -> Result<(), String> {
        let payload = json!({
            "show_id": show_id,
            "season_id": season_id,
            "episode_ids": episode_ids,
        });

        self.nats_publisher
            .enqueue_job("kobbler", "sports_want", payload)
            .await
            .map_err(|e| format!("Failed to enqueue sports_want job: {}", e))?;

        Ok(())
    }
}

fn parse_date(s: &str) -> Option<time::Date> {
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
}
