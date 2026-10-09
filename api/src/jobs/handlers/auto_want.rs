//! Shared auto-want logic for TV episodes.
//!
//! Free functions that apply auto_want_mode for a season, used by both
//! `tv_monitor` (hourly scheduled) and `migration_episode_reconcile`
//! (after Sonarr import). Uses DB air dates populated by enrichment.

use crate::repositories::{tv_episodes, tv_seasons, tv_shows};
use nats_jobs::Publisher;
use serde_json::json;
use sqlx::PgPool;
use tracing::warn;
use uuid::Uuid;

/// Apply a show's standing want across every season it has right now.
///
/// The want is recorded on the show; the episodes it refers to can arrive long
/// afterwards, because creating a show only enqueues its enrichment. Applying the want
/// once, at the moment it was set, therefore reached exactly the seasons that existed
/// then — for a show added from Discover, none of them — and the hourly `tv_monitor`
/// sweep was the first thing to notice. So it is applied again whenever the episodes
/// turn up, which is what makes "want this show" mean it at the time of asking.
///
/// Returns the number of episodes newly wanted.
pub async fn apply_show_auto_want(
    pool: &PgPool,
    publisher: &Publisher,
    show_id: Uuid,
) -> Result<u32, String> {
    let show = tv_shows::get(pool, show_id)
        .await
        .map_err(|e| format!("Failed to get show: {e}"))?
        .ok_or_else(|| format!("Show {show_id} not found"))?;

    if !show.is_monitored || show.auto_want_mode == "none" {
        return Ok(0);
    }

    let seasons = tv_seasons::list_by_show(pool, show_id)
        .await
        .map_err(|e| format!("Failed to list seasons: {e}"))?;

    let mut wanted = 0;
    for season in &seasons {
        let effective_mode = resolve_effective_mode(season, &show.auto_want_mode);
        wanted += apply_auto_want(pool, publisher, show_id, season.id, effective_mode).await?;
    }
    Ok(wanted)
}

/// Apply the effective auto_want_mode for a single season.
///
/// Returns the number of episodes newly wanted.
pub async fn apply_auto_want(
    pool: &PgPool,
    publisher: &Publisher,
    show_id: Uuid,
    season_id: Uuid,
    effective_mode: &str,
) -> Result<u32, String> {
    match effective_mode {
        "none" => Ok(0),
        "new_only" => want_newly_aired(pool, publisher, show_id, season_id).await,
        "all" => want_all_missing(pool, publisher, show_id, season_id).await,
        "fill_gaps" => want_fill_gaps(pool, publisher, show_id, season_id).await,
        other => {
            warn!(
                show_id = %show_id,
                auto_want_mode = other,
                "Unknown auto_want_mode, treating as 'none'"
            );
            Ok(0)
        }
    }
}

/// How long after a season's last airing kobload is still asked to look for
/// quality upgrades.
const UPGRADE_WINDOW_DAYS: i64 = 30;

/// Whether the season aired recently enough that kobload should still be
/// looking for quality upgrades. A season with no dated episodes counts as open.
pub fn upgrade_window_open(episodes: &[tv_episodes::EpisodeRow]) -> bool {
    episodes
        .iter()
        .filter_map(|ep| ep.air_date)
        .max()
        .is_none_or(|d| {
            d >= time::OffsetDateTime::now_utc().date() - time::Duration::days(UPGRADE_WINDOW_DAYS)
        })
}

/// Auto-want modes that would re-want an episode a user has just unwanted, so a
/// bulk unwant has to switch them off to stick. 'new_only' is not one of them:
/// it only reaches episodes that have not aired yet. `sports_monitor` applies
/// the same vocabulary through its own copy of `apply_auto_want`.
pub const REFILLING_MODES: &[&str] = &["all", "fill_gaps"];

/// Whether `mode` would put the wants straight back. See [`REFILLING_MODES`].
pub fn mode_refills(mode: &str) -> bool {
    REFILLING_MODES.contains(&mode)
}

/// Resolve the effective auto_want_mode for a season, considering the
/// season's own setting and the show-level default.
pub fn resolve_effective_mode<'a>(
    season: &'a tv_seasons::SeasonRow,
    show_auto_want_mode: &'a str,
) -> &'a str {
    match season.auto_want_mode.as_str() {
        "inherit" => show_auto_want_mode,
        mode => mode,
    }
}

/// Want episodes within a ±5 day window around today and have want_status='none'.
///
/// Uses air dates stored in the DB (populated by enrichment), not live TMDB data.
/// The +5 day lookahead sends near-future episodes to kobload early so it can
/// grab them as soon as they drop (kobload has its own air_date logic to prevent
/// downloading too early). Also re-triggers for stuck 'wanted' episodes in the
/// window that were never sent to kobload.
pub async fn want_newly_aired(
    pool: &PgPool,
    publisher: &Publisher,
    show_id: Uuid,
    season_id: Uuid,
) -> Result<u32, String> {
    let now = time::OffsetDateTime::now_utc().date();
    let past_cutoff = now - time::Duration::days(5);
    let future_cutoff = now + time::Duration::days(5);

    let all_episodes = tv_episodes::list_by_season(pool, season_id)
        .await
        .map_err(|e| format!("Failed to list episodes: {e}"))?;

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
        tv_episodes::set_want_status_batch(pool, &to_want, "wanted")
            .await
            .map_err(|e| format!("Failed to set want_status: {e}"))?;

        enqueue_tv_want(publisher, show_id, season_id, Some(to_want)).await?;
    }

    // Re-trigger for episodes stuck in 'wanted' within the window that were
    // never sent to kobload. Only targets 'wanted' — 'searching' episodes are
    // already in kobload.
    let stuck: Vec<Uuid> = all_episodes
        .iter()
        .filter(|ep| {
            ep.want_status == "wanted"
                && !ep.is_in_library
                && ep
                    .air_date
                    .is_some_and(|d| d >= past_cutoff && d <= future_cutoff)
        })
        .map(|ep| ep.id)
        .collect();
    if !stuck.is_empty() {
        enqueue_tv_want(publisher, show_id, season_id, Some(stuck)).await?;
    }

    Ok(count)
}

/// Want all missing episodes for the season.
///
/// Also re-triggers tv_want for episodes stuck in 'wanted' status that were
/// never sent to kobload.
pub async fn want_all_missing(
    pool: &PgPool,
    publisher: &Publisher,
    show_id: Uuid,
    season_id: Uuid,
) -> Result<u32, String> {
    let missing = tv_episodes::list_missing(pool, season_id)
        .await
        .map_err(|e| format!("Failed to list missing episodes: {e}"))?;

    let episode_ids: Vec<Uuid> = missing.iter().map(|ep| ep.id).collect();
    let count = episode_ids.len() as u32;

    if !episode_ids.is_empty() {
        tv_episodes::set_want_status_batch(pool, &episode_ids, "wanted")
            .await
            .map_err(|e| format!("Failed to set want_status: {e}"))?;

        enqueue_tv_want(publisher, show_id, season_id, None).await?;
        return Ok(count);
    }

    // No new episodes to want, but check for episodes stuck in 'wanted' or
    // 'searching' that may not be in kobload's pattern (e.g. status mismatch
    // between kobbler and kobload). Re-sending is idempotent — kobload upserts
    // by season_id. This is safe for "all" mode since we want everything anyway.
    let all_episodes = tv_episodes::list_by_season(pool, season_id)
        .await
        .map_err(|e| format!("Failed to list episodes: {e}"))?;
    let has_unsatisfied = all_episodes.iter().any(|ep| {
        (ep.want_status == "wanted" || ep.want_status == "searching") && !ep.is_in_library
    });
    if has_unsatisfied {
        enqueue_tv_want(publisher, show_id, season_id, None).await?;
    }

    Ok(count)
}

/// Want missing episodes within the range between first and last in-library episode.
///
/// Also re-triggers tv_want for episodes stuck in 'wanted' status that were
/// never sent to kobload.
pub async fn want_fill_gaps(
    pool: &PgPool,
    publisher: &Publisher,
    show_id: Uuid,
    season_id: Uuid,
) -> Result<u32, String> {
    let all_episodes = tv_episodes::list_by_season(pool, season_id)
        .await
        .map_err(|e| format!("Failed to list episodes: {e}"))?;

    let in_library_orders: Vec<i32> = all_episodes
        .iter()
        .filter(|ep| ep.is_in_library)
        .map(|ep| ep.episode_sort_order)
        .collect();

    // Re-trigger for stuck 'wanted' episodes even if gap detection can't run
    let stuck: Vec<Uuid> = all_episodes
        .iter()
        .filter(|ep| ep.want_status == "wanted" && !ep.is_in_library)
        .map(|ep| ep.id)
        .collect();

    if in_library_orders.len() < 2 {
        if !stuck.is_empty() {
            enqueue_tv_want(publisher, show_id, season_id, Some(stuck)).await?;
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
        tv_episodes::set_want_status_batch(pool, &to_want, "wanted")
            .await
            .map_err(|e| format!("Failed to set want_status: {e}"))?;

        enqueue_tv_want(publisher, show_id, season_id, Some(to_want)).await?;
    } else if !stuck.is_empty() {
        // No new gaps but stuck episodes exist
        enqueue_tv_want(publisher, show_id, season_id, Some(stuck)).await?;
    }

    Ok(count)
}

/// Enqueue a tv_want job via NATS publisher.
pub async fn enqueue_tv_want(
    publisher: &Publisher,
    show_id: Uuid,
    season_id: Uuid,
    episode_ids: Option<Vec<Uuid>>,
) -> Result<(), String> {
    enqueue_tv_want_inner(publisher, show_id, season_id, episode_ids, false).await
}

/// Re-send a season's want purely to correct Kobload's view of the library.
///
/// Sets `refresh_only`, which bypasses `tv_want`'s "everything acquired and the
/// upgrade window is closed" skip: the whole point is to push a season that has
/// nothing left to acquire, so Kobload stops treating already-held episodes as
/// missing.
pub async fn enqueue_tv_want_refresh(
    publisher: &Publisher,
    show_id: Uuid,
    season_id: Uuid,
) -> Result<(), String> {
    enqueue_tv_want_inner(publisher, show_id, season_id, None, true).await
}

async fn enqueue_tv_want_inner(
    publisher: &Publisher,
    show_id: Uuid,
    season_id: Uuid,
    episode_ids: Option<Vec<Uuid>>,
    refresh_only: bool,
) -> Result<(), String> {
    publisher
        .enqueue_job(
            "kobbler",
            "tv_want",
            json!({
                "show_id": show_id,
                "season_id": season_id,
                "episode_ids": episode_ids,
                "refresh_only": refresh_only,
            }),
        )
        .await
        .map_err(|e| format!("Failed to enqueue tv_want job: {e}"))?;

    Ok(())
}
