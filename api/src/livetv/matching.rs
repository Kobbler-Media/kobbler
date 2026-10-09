//! Recording -> library title matching (Decision 16).
//!
//! Precision over recall: an unmatched recording is still watchable under its
//! channel and programme title, but a wrong match files someone's episode under
//! another show and poisons Up next.

use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::repositories::{RepositoryError, livetv_recordings};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Match {
    Episode(Uuid),
    Movie(Uuid),
    None,
}

#[derive(Debug, Clone, Default)]
pub struct MatchInput {
    pub title: String,
    pub episode_number: Option<String>,
    pub episode_title: Option<String>,
    pub original_airdate: Option<OffsetDateTime>,
}

/// Must stay identical to the `regexp_replace(lower(x), '[^a-z0-9]+', '', 'g')`
/// the repository applies, or the two halves of every comparison drift apart.
pub fn normalise(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// `S03E07` -> (3, 7). A guide feed that only carries a season is not enough to
/// identify an episode, so both halves are required.
fn parse_season_episode(raw: &str) -> Option<(i32, i32)> {
    let upper = raw.trim().to_ascii_uppercase();
    let rest = upper.strip_prefix('S')?;
    let season: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if season.is_empty() {
        return None;
    }
    let after = &rest[season.len()..];
    let episode: String = after
        .strip_prefix('E')?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if episode.is_empty() {
        return None;
    }
    Some((season.parse().ok()?, episode.parse().ok()?))
}

/// Guide titles carry the broadcaster's decoration; the library carries the
/// canonical title.
pub fn clean_title(raw: &str) -> String {
    let mut out = raw.trim();
    for suffix in [" (Repeat)", " (New)", " (Final)", " (Encore)"] {
        out = out.trim_end_matches(suffix).trim_end();
    }
    if let Some(idx) = out.find(" - Season ") {
        out = &out[..idx];
    }
    out.trim().to_string()
}

pub async fn resolve(pool: &sqlx::PgPool, input: &MatchInput) -> Result<Match, RepositoryError> {
    let title = clean_title(&input.title);
    let key = normalise(&title);
    if key.is_empty() {
        return Ok(Match::None);
    }

    let shows = livetv_recordings::find_shows_by_normalised_title(pool, &key).await?;
    if shows.len() == 1 {
        let show_id = shows[0];
        if let Some((season, episode)) = input
            .episode_number
            .as_deref()
            .and_then(parse_season_episode)
            && let Some(id) =
                livetv_recordings::find_episode_by_numbers(pool, show_id, season, episode).await?
        {
            return Ok(Match::Episode(id));
        }
        if let Some(date) = input.original_airdate.map(OffsetDateTime::date) {
            let hits = air_date_hits(pool, show_id, date).await?;
            if hits.len() == 1 {
                return Ok(Match::Episode(hits[0]));
            }
        }
        if let Some(episode_title) = input.episode_title.as_deref() {
            let ekey = normalise(episode_title);
            if !ekey.is_empty() {
                let hits =
                    livetv_recordings::find_episodes_by_normalised_title(pool, show_id, &ekey)
                        .await?;
                if hits.len() == 1 {
                    return Ok(Match::Episode(hits[0]));
                }
            }
        }
        return Ok(Match::None);
    }

    // A broadcast film has no episode identity at all, so the only safe key is a
    // title that resolves to exactly one library movie.
    if shows.is_empty() && input.episode_number.is_none() {
        let movies = livetv_recordings::find_movies_by_normalised_title(pool, &key).await?;
        if movies.len() == 1 {
            return Ok(Match::Movie(movies[0]));
        }
    }
    Ok(Match::None)
}

/// The broadcaster's `OriginalAirdate` is a date in the broadcaster's own zone,
/// so a one-day window either side is the honest comparison.
async fn air_date_hits(
    pool: &sqlx::PgPool,
    show_id: Uuid,
    date: Date,
) -> Result<Vec<Uuid>, RepositoryError> {
    let mut hits = livetv_recordings::find_episodes_by_air_date(pool, show_id, date).await?;
    if hits.is_empty()
        && let Some(prev) = date.previous_day()
    {
        hits = livetv_recordings::find_episodes_by_air_date(pool, show_id, prev).await?;
    }
    if hits.is_empty()
        && let Some(next) = date.next_day()
    {
        hits = livetv_recordings::find_episodes_by_air_date(pool, show_id, next).await?;
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_the_same_way_postgres_does() {
        assert_eq!(normalise("Hard Quiz!"), "hardquiz");
        assert_eq!(normalise("  The 7.30 Report "), "the730report");
        assert_eq!(normalise("---"), "");
    }

    #[test]
    fn parses_season_and_episode_but_never_half_of_one() {
        assert_eq!(parse_season_episode("S03E07"), Some((3, 7)));
        assert_eq!(parse_season_episode("s12e01"), Some((12, 1)));
        assert_eq!(parse_season_episode("S03"), None);
        assert_eq!(parse_season_episode("E07"), None);
        assert_eq!(parse_season_episode(""), None);
    }

    #[test]
    fn strips_broadcaster_decoration() {
        assert_eq!(clean_title("The Block (Repeat)"), "The Block");
        assert_eq!(clean_title("The Traitors - Season 3"), "The Traitors");
        assert_eq!(clean_title(" Hard Quiz "), "Hard Quiz");
    }
}
