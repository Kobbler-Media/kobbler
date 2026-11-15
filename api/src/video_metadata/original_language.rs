//! The language a title was made in, as TMDB records it (ISO 639-1), kept on the title
//! and fetched the first time playback asks for one that predates the column.

use std::time::Duration;

use sqlx::PgPool;
use tracing::debug;
use uuid::Uuid;

use super::tmdb::TmdbProvider;
use crate::repositories::original_language::{self as stored, Known};
use crate::repositories::video_metadata_providers;

// Playback waits on this, so a slow TMDB costs the answer rather than the start.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub enum Title {
    Movie(Uuid),
    Episode(Uuid),
    EpisodeFile(Uuid),
    /// A Discover title streamed by TMDB id, which may have no library row yet.
    TmdbMovie(String),
    TmdbShow(String),
}

#[derive(Clone, Copy)]
enum Kind {
    Movie,
    Show,
}

pub async fn of(pool: &PgPool, title: &Title) -> Option<String> {
    match tokio::time::timeout(LOOKUP_TIMEOUT, lookup(pool, title)).await {
        Ok(Ok(language)) => language,
        Ok(Err(e)) => {
            debug!(error = %e, ?title, "original language lookup failed");
            None
        }
        Err(_) => {
            debug!(?title, "original language lookup timed out");
            None
        }
    }
}

async fn lookup(pool: &PgPool, title: &Title) -> anyhow::Result<Option<String>> {
    let (kind, known) = match title {
        Title::Movie(id) => (Kind::Movie, stored::movie(pool, *id).await?),
        Title::TmdbMovie(tmdb_id) => (Kind::Movie, stored::movie_by_tmdb(pool, tmdb_id).await?),
        Title::Episode(id) => (Kind::Show, stored::show_of_episode(pool, *id).await?),
        Title::EpisodeFile(id) => (Kind::Show, stored::show_of_episode_file(pool, *id).await?),
        Title::TmdbShow(tmdb_id) => (Kind::Show, stored::show_by_tmdb(pool, tmdb_id).await?),
    };
    if let Some(Known {
        original_language: Some(language),
        ..
    }) = known
    {
        return Ok(Some(language));
    }
    let tmdb_id = match (&known, title) {
        (Some(known), _) => known.tmdb_id.clone(),
        (None, Title::TmdbMovie(id) | Title::TmdbShow(id)) => Some(id.clone()),
        (None, _) => None,
    };
    // The id of a Discover title comes from the client and ends up in a URL path.
    let Some(tmdb_id) =
        tmdb_id.filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()))
    else {
        return Ok(None);
    };
    let Some(tmdb) = provider(pool).await? else {
        return Ok(None);
    };
    let path = match kind {
        Kind::Movie => "movie",
        Kind::Show => "tv",
    };
    let Some(language) = tmdb.original_language(path, &tmdb_id).await? else {
        return Ok(None);
    };
    if let Some(known) = known {
        match kind {
            Kind::Movie => stored::set_movie(pool, known.id, &language).await?,
            Kind::Show => stored::set_show(pool, known.id, &language).await?,
        }
    }
    Ok(Some(language))
}

async fn provider(pool: &PgPool) -> anyhow::Result<Option<TmdbProvider>> {
    Ok(video_metadata_providers::list_enabled(pool)
        .await?
        .into_iter()
        .find(|p| p.provider_type == super::tmdb::INFO.id)
        .map(|p| p.config)
        .filter(|config| {
            config
                .get("api_key")
                .and_then(|key| key.as_str())
                .is_some_and(|key| !key.is_empty())
        })
        .map(|config| TmdbProvider::from_config(crate::http::new_client(), &config)))
}
