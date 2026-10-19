use serde_json::json;
use sqlx::PgPool;
use time::{Date, OffsetDateTime};

use super::progress::{Pointer, Progress};
use crate::repositories::metadata_cache;
use crate::repositories::stremio_continue::ContinueRow;
use crate::video_metadata::{SeasonDetails, VideoProviderError, VideoProviderRegistry};

const CACHE_PROVIDER: &str = "stremio";
const IMDB_MAP_TTL: time::Duration = time::Duration::days(30);

pub struct Identity {
    pub tmdb_id: String,
    pub title: String,
    pub year: Option<i32>,
    pub poster_url: Option<String>,
    pub content_rating: Option<String>,
}

pub struct Resolver {
    pool: PgPool,
    registry: VideoProviderRegistry,
}

impl Resolver {
    pub fn new(pool: PgPool) -> Self {
        Self {
            registry: VideoProviderRegistry::new(pool.clone()),
            pool,
        }
    }

    /// `Ok(None)` is a title with nothing to show right now: an ended show the user has
    /// finished, a TMDB miss, an episode TMDB has no air date for.
    pub async fn resolve(&self, p: &Progress) -> Result<Option<ContinueRow>, VideoProviderError> {
        let is_movie = p.pointer == Pointer::Movie;
        let Some(tmdb_id) = self.tmdb_id(&p.imdb_id, is_movie).await? else {
            return Ok(None);
        };
        if is_movie {
            let m = self.registry.get_movie("tmdb", &tmdb_id).await?;
            return Ok(Some(ContinueRow {
                stremio_id: p.imdb_id.clone(),
                media_type: "movie".into(),
                kind: "resume".into(),
                tmdb_id,
                title: m.title,
                year: m.year.map(i32::from),
                description: m.description,
                poster_url: m.poster_url,
                backdrop_url: m.backdrop_url,
                genres: m.genres,
                rating: m.rating,
                content_rating: m.content_rating,
                season: None,
                episode: None,
                episode_title: None,
                air_date: None,
                season_premiere: false,
                position_ms: p.position_ms as i64,
                duration_ms: p.duration_ms as i64,
                last_watched: p.last_watched,
            }));
        }

        let show = self.registry.get_tv_show("tmdb", &tmdb_id).await?;
        let (kind, season, episode) = match p.pointer {
            Pointer::Resume { season, episode } => ("resume", season, episode),
            Pointer::UpNext { season, episode } => ("up_next", season, episode),
            Pointer::CaughtUp { season, episode } => {
                if matches!(show.status.as_deref(), Some("Ended" | "Canceled")) {
                    return Ok(None);
                }
                let has_next_season = show.seasons.iter().any(|s| s.season_number == season + 1);
                match self
                    .episode_after(&tmdb_id, season, episode, has_next_season)
                    .await?
                {
                    Some((s, e)) => ("new_episode", s, e),
                    None => return Ok(None),
                }
            }
            Pointer::Movie => unreachable!(),
        };
        let found = self
            .season(&tmdb_id, season)
            .await?
            .and_then(|s| s.episodes.into_iter().find(|e| e.episode_number == episode));
        let air_date = found
            .as_ref()
            .and_then(|e| e.air_date.as_deref())
            .and_then(parse_date);
        if kind == "new_episode" && air_date.is_none() {
            return Ok(None);
        }
        Ok(Some(ContinueRow {
            stremio_id: p.imdb_id.clone(),
            media_type: "tv".into(),
            kind: kind.into(),
            tmdb_id,
            title: show.title,
            year: show.year.map(i32::from),
            description: show.description,
            poster_url: show.poster_url,
            backdrop_url: show.backdrop_url,
            genres: show.genres,
            rating: show.rating,
            content_rating: show.content_rating,
            season: Some(season as i32),
            episode: Some(episode as i32),
            episode_title: found.and_then(|e| e.title),
            air_date,
            season_premiere: season > 1 && episode == 1,
            position_ms: p.position_ms as i64,
            duration_ms: p.duration_ms as i64,
            last_watched: p.last_watched,
        }))
    }

    pub async fn identify(
        &self,
        imdb_id: &str,
        movie: bool,
    ) -> Result<Option<Identity>, VideoProviderError> {
        let Some(tmdb_id) = self.tmdb_id(imdb_id, movie).await? else {
            return Ok(None);
        };
        let (title, year, poster_url, content_rating) = if movie {
            let m = self.registry.get_movie("tmdb", &tmdb_id).await?;
            (m.title, m.year, m.poster_url, m.content_rating)
        } else {
            let s = self.registry.get_tv_show("tmdb", &tmdb_id).await?;
            (s.title, s.year, s.poster_url, s.content_rating)
        };
        Ok(Some(Identity {
            tmdb_id,
            title,
            year: year.map(i32::from),
            poster_url,
            content_rating,
        }))
    }

    async fn episode_after(
        &self,
        tmdb_id: &str,
        season: u32,
        episode: u32,
        has_next_season: bool,
    ) -> Result<Option<(u32, u32)>, VideoProviderError> {
        let in_season = self
            .season(tmdb_id, season)
            .await?
            .is_some_and(|s| s.episodes.iter().any(|e| e.episode_number == episode + 1));
        Ok(if in_season {
            Some((season, episode + 1))
        } else if has_next_season {
            Some((season + 1, 1))
        } else {
            None
        })
    }

    async fn season(
        &self,
        tmdb_id: &str,
        season: u32,
    ) -> Result<Option<SeasonDetails>, VideoProviderError> {
        match self.registry.get_season("tmdb", tmdb_id, season).await {
            Ok(s) => Ok(Some(s)),
            Err(VideoProviderError::NotFound) => Ok(None),
            Err(VideoProviderError::Http(e))
                if e.status() == Some(reqwest::StatusCode::NOT_FOUND) =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    async fn tmdb_id(
        &self,
        imdb_id: &str,
        movie: bool,
    ) -> Result<Option<String>, VideoProviderError> {
        let key = format!("imdb:{imdb_id}");
        let (movies, shows): (Vec<String>, Vec<String>) =
            match metadata_cache::get(&self.pool, CACHE_PROVIDER, &key).await {
                Ok(Some(cached)) => serde_json::from_value(cached).unwrap_or_default(),
                _ => {
                    let found = self.registry.find_by_imdb_id(imdb_id).await?;
                    let _ = metadata_cache::set(
                        &self.pool,
                        CACHE_PROVIDER,
                        &key,
                        &json!(found),
                        OffsetDateTime::now_utc() + IMDB_MAP_TTL,
                    )
                    .await;
                    found
                }
            };
        Ok(if movie { movies } else { shows }.into_iter().next())
    }
}

fn parse_date(s: &str) -> Option<Date> {
    Date::parse(s, time::macros::format_description!("[year]-[month]-[day]")).ok()
}
