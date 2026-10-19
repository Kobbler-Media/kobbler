//! Where a Stremio library item has got to, read the way stremio-core's player writes it:
//! finishing an episode moves `video_id` on to the next one and sets `time_offset` to 1, and
//! when there is nothing to move on to — or a movie ends — `time_offset` drops to 0.

use time::OffsetDateTime;

use super::client::LibraryItem;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pointer {
    Movie,
    Resume { season: u32, episode: u32 },
    UpNext { season: u32, episode: u32 },
    CaughtUp { season: u32, episode: u32 },
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub imdb_id: String,
    pub pointer: Pointer,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub last_watched: OffsetDateTime,
}

pub fn classify(item: &LibraryItem) -> Option<Progress> {
    if !item.id.starts_with("tt") {
        return None;
    }
    let state = &item.state;
    let in_continue_watching = (!item.removed || item.temp) && state.time_offset > 0;
    let pointer = match item.item_type.as_str() {
        "movie" if in_continue_watching => Pointer::Movie,
        "series" => {
            let (season, episode) = parse_video_id(state.video_id.as_deref()?, &item.id)?;
            match state.time_offset {
                _ if !in_continue_watching && (item.removed || item.temp) => return None,
                0 => Pointer::CaughtUp { season, episode },
                1 => Pointer::UpNext { season, episode },
                _ => Pointer::Resume { season, episode },
            }
        }
        _ => return None,
    };
    Some(Progress {
        imdb_id: item.id.clone(),
        pointer,
        position_ms: if matches!(pointer, Pointer::Movie | Pointer::Resume { .. }) {
            state.time_offset
        } else {
            0
        },
        duration_ms: state.duration,
        last_watched: state.last_watched.or(item.mtime)?,
    })
}

pub(super) fn parse_video_id(video_id: &str, imdb_id: &str) -> Option<(u32, u32)> {
    let mut parts = video_id
        .strip_prefix(imdb_id)?
        .strip_prefix(':')?
        .split(':');
    let season = parts.next()?.parse().ok()?;
    let episode = parts.next()?.parse().ok()?;
    (season > 0 && parts.next().is_none()).then_some((season, episode))
}
