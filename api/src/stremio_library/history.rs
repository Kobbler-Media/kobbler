//! What a Stremio library item says has been watched. Stremio keeps a position marker per series
//! rather than a list of episodes, so everything before the marker counts as watched.

use time::OffsetDateTime;

use super::client::LibraryItem;
use super::progress::parse_video_id;

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub stremio_id: String,
    pub media_type: &'static str,
    pub name: String,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub marker_watched: bool,
    pub watched: bool,
    pub position_ms: i64,
    pub last_watched: OffsetDateTime,
}

pub fn entry(item: &LibraryItem) -> Option<HistoryEntry> {
    if !item.id.starts_with("tt") {
        return None;
    }
    let state = &item.state;
    let last_watched = state.last_watched?;
    let finished = state.times_watched > 0 || state.flagged_watched > 0;
    let position_ms = i64::try_from(state.time_offset).unwrap_or(0);
    let (media_type, season, episode, marker_watched, watched) = match item.item_type.as_str() {
        "movie" if finished || position_ms > 0 => ("movie", None, None, false, finished),
        "series" => {
            let (season, episode) = parse_video_id(state.video_id.as_deref()?, &item.id)?;
            (
                "tv",
                Some(season as i32),
                Some(episode as i32),
                state.time_offset == 0,
                false,
            )
        }
        _ => return None,
    };
    Some(HistoryEntry {
        stremio_id: item.id.clone(),
        media_type,
        name: item.name.clone(),
        season,
        episode,
        marker_watched,
        watched,
        // stremio-core writes 1 to mean "moved on to this episode", not a resume point.
        position_ms: if watched || position_ms <= 1 {
            0
        } else {
            position_ms
        },
        last_watched,
    })
}
