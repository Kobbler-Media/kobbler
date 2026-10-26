use std::collections::HashSet;

use tokio_util::sync::CancellationToken;

use super::TitleMedia;
use crate::jobs::JobResult;
use crate::video_metadata::tmdb::{TmdbProvider, TmdbTitle};

/// 20 results a page: each sweep reaches the 5,000 most popular titles, so a full pass is
/// 4 sweeps × 250 = 1,000 requests a week, against the exports' 125,000 rows.
pub const PAGES_PER_SWEEP: u32 = 250;

/// Originals in these scripts cannot be typed on a Latin keyboard at all, so they get a
/// sweep of their own beyond the overall most popular.
pub const NON_LATIN_LANGUAGES: &str =
    "ko|ja|zh|cn|hi|ta|te|ml|kn|bn|mr|th|ru|uk|ar|fa|he|el|ka|hy|ur|pa|gu|my|km|bg|sr|mk|kk|mn";

pub async fn english_titles(
    tmdb: &TmdbProvider,
    media: TitleMedia,
    cancel: &CancellationToken,
) -> JobResult<(Vec<TmdbTitle>, Vec<String>)> {
    let mut seen = HashSet::new();
    let mut titles = Vec::new();
    let mut errors = Vec::new();
    for languages in [None, Some(NON_LATIN_LANGUAGES)] {
        let mut page = 1;
        loop {
            nats_jobs::check_cancelled(cancel)?;
            match tmdb.discover_titles(media.as_str(), page, languages).await {
                Ok(result) => {
                    titles.extend(
                        result
                            .results
                            .into_iter()
                            .filter(|t| !t.title.trim().is_empty() && seen.insert(t.id)),
                    );
                    if page >= result.total_pages.min(PAGES_PER_SWEEP) {
                        break;
                    }
                    page += 1;
                }
                Err(e) => {
                    errors.push(format!("TMDB discover {} page {page}: {e}", media.as_str()));
                    break;
                }
            }
        }
    }
    Ok((titles, errors))
}
