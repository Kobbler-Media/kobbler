//! From a subject to the thing actually being asked for.
//!
//! "Season 24 of someShow" resolves a show and then descends to a season;
//! "the new one" resolves a show and then descends to an air date. Relative
//! time is resolved here, against `air_date_ts`, with the season and episode
//! sort orders as the fallback for shows whose dates are missing.

use std::collections::HashSet;

use time::OffsetDateTime;
use uuid::Uuid;

use crate::search::{TvEpisodeDocument, TvSeasonDocument, TvShowDocument};

use super::index::{IndexKind, IndexQuery, LibraryIndex};
use super::phrasing::Relative;
use super::types::ResolveError;

/// TMDB dates are wrong often enough to matter, and there is no air *time*
/// anywhere, so "last night" is a window rather than a date.
const LAST_NIGHT_BACK_HOURS: i64 = 48;
const LAST_NIGHT_FORWARD_HOURS: i64 = 6;

const SEASONS_PER_SHOW: usize = 60;
const EPISODES_PER_SEASON: usize = 200;
const EPISODES_PER_SHOW: usize = 500;

pub fn season_code(season: i32) -> String {
    format!("S{season:02}")
}

pub fn episode_code(season: i32, episode: i32) -> String {
    format!("S{season:02}E{episode:02}")
}

fn id_filter(field: &str, id: Uuid) -> String {
    format!("{field} = \"{id}\"")
}

pub async fn find_season(
    index: &dyn LibraryIndex,
    show_id: Uuid,
    season: i32,
) -> Result<Option<TvSeasonDocument>, ResolveError> {
    let hits = index
        .multi_search(
            &[IndexQuery::new(IndexKind::TvSeasons, "", SEASONS_PER_SHOW)
                .filtered(id_filter("show_id", show_id))],
        )
        .await?;
    let wanted = season_code(season);
    Ok(hits.seasons.into_iter().find(|s| s.season_code == wanted))
}

pub async fn episodes_of_season(
    index: &dyn LibraryIndex,
    season_id: Uuid,
) -> Result<Vec<TvEpisodeDocument>, ResolveError> {
    let hits = index
        .multi_search(&[
            IndexQuery::new(IndexKind::TvEpisodes, "", EPISODES_PER_SEASON)
                .filtered(id_filter("season_id", season_id))
                .sorted(&["episode_sort_order:asc"]),
        ])
        .await?;
    Ok(hits.episodes)
}

pub async fn find_episode(
    index: &dyn LibraryIndex,
    show_id: Uuid,
    season: i32,
    episode: i32,
) -> Result<Option<TvEpisodeDocument>, ResolveError> {
    let Some(season_doc) = find_season(index, show_id, season).await? else {
        return Ok(None);
    };
    let season_uuid =
        Uuid::parse_str(&season_doc.id).map_err(|e| ResolveError::Index(e.to_string()))?;
    let wanted = episode_code(season, episode);
    Ok(episodes_of_season(index, season_uuid)
        .await?
        .into_iter()
        .find(|e| e.episode_code == wanted))
}

pub async fn show_by_id(
    index: &dyn LibraryIndex,
    show_id: Uuid,
) -> Result<Option<TvShowDocument>, ResolveError> {
    let hits = index
        .multi_search(&[
            IndexQuery::new(IndexKind::TvShows, "", 1).filtered(id_filter("id", show_id))
        ])
        .await?;
    Ok(hits.shows.into_iter().next())
}

pub async fn episode_by_id(
    index: &dyn LibraryIndex,
    episode_id: Uuid,
) -> Result<Option<TvEpisodeDocument>, ResolveError> {
    let hits = index
        .multi_search(&[
            IndexQuery::new(IndexKind::TvEpisodes, "", 1).filtered(id_filter("id", episode_id))
        ])
        .await?;
    Ok(hits.episodes.into_iter().next())
}

pub async fn all_episodes_in_order(
    index: &dyn LibraryIndex,
    show_id: Uuid,
) -> Result<Vec<TvEpisodeDocument>, ResolveError> {
    let hits = index
        .multi_search(&[
            IndexQuery::new(IndexKind::TvEpisodes, "", EPISODES_PER_SHOW)
                .filtered(id_filter("show_id", show_id))
                .sorted(&["season_sort_order:asc", "episode_sort_order:asc"]),
        ])
        .await?;
    Ok(hits.episodes)
}

/// The episode after `after_id` in broadcast order.
pub async fn next_after(
    index: &dyn LibraryIndex,
    show_id: Uuid,
    after_id: Uuid,
) -> Result<Option<TvEpisodeDocument>, ResolveError> {
    let episodes = all_episodes_in_order(index, show_id).await?;
    let Some(pos) = episodes.iter().position(|e| e.id == after_id.to_string()) else {
        return Ok(None);
    };
    Ok(episodes.into_iter().nth(pos + 1))
}

pub struct RelativeOutcome {
    pub episode: Option<TvEpisodeDocument>,
    /// What the answer has to be hedged with.
    pub note: Option<String>,
}

/// Resolve `latest` / `next` / `last night's` / `first` for one show.
pub async fn relative_episode(
    index: &dyn LibraryIndex,
    show_id: Uuid,
    relative: Relative,
    now: OffsetDateTime,
    watched: &HashSet<Uuid>,
) -> Result<RelativeOutcome, ResolveError> {
    let now_ts = now.unix_timestamp();
    let show = id_filter("show_id", show_id);

    let (query, mut note) = match relative {
        Relative::Latest => (
            IndexQuery::new(IndexKind::TvEpisodes, "", 25)
                .filtered(format!("{show} AND air_date_ts <= {now_ts}"))
                .sorted(&["air_date_ts:desc"]),
            None,
        ),
        Relative::Next => (
            IndexQuery::new(IndexKind::TvEpisodes, "", 5)
                .filtered(format!("{show} AND air_date_ts > {now_ts}"))
                .sorted(&["air_date_ts:asc"]),
            Some("air dates come from the metadata provider, so treat them as ±a day".to_string()),
        ),
        Relative::LastNight => {
            let from = now_ts - LAST_NIGHT_BACK_HOURS * 3600;
            let to = now_ts + LAST_NIGHT_FORWARD_HOURS * 3600;
            (
                IndexQuery::new(IndexKind::TvEpisodes, "", 5)
                    .filtered(format!(
                        "{show} AND air_date_ts >= {from} AND air_date_ts <= {to}"
                    ))
                    .sorted(&["air_date_ts:desc"]),
                None,
            )
        }
        Relative::First => (
            IndexQuery::new(IndexKind::TvEpisodes, "", 25)
                .filtered(format!("{show} AND season_sort_order > 0"))
                .sorted(&["season_sort_order:asc", "episode_sort_order:asc"]),
            None,
        ),
    };

    let mut episodes = index.multi_search(&[query]).await?.episodes;

    // Shows with no air dates at all still have an order.
    if episodes.is_empty() && matches!(relative, Relative::Latest) {
        episodes = index
            .multi_search(&[IndexQuery::new(IndexKind::TvEpisodes, "", 25)
                .filtered(format!("{show} AND season_sort_order > 0"))
                .sorted(&["season_sort_order:desc", "episode_sort_order:desc"])])
            .await?
            .episodes;
        if !episodes.is_empty() {
            note = Some("no air dates on this show, so this is the last one by number".to_string());
        }
    }

    let episode = episodes.into_iter().find(|e| match Uuid::parse_str(&e.id) {
        Ok(id) => !watched.contains(&id),
        Err(_) => false,
    });

    Ok(RelativeOutcome { episode, note })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_padded_the_way_the_index_stores_them() {
        assert_eq!(season_code(2), "S02");
        assert_eq!(season_code(24), "S24");
        assert_eq!(episode_code(2, 5), "S02E05");
        assert_eq!(episode_code(24, 7), "S24E07");
        assert_eq!(episode_code(2026, 8), "S2026E08");
    }

    #[test]
    fn filters_quote_uuids() {
        let id = Uuid::nil();
        assert_eq!(
            id_filter("show_id", id),
            "show_id = \"00000000-0000-0000-0000-000000000000\""
        );
    }
}
