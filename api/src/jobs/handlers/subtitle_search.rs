//! Subtitle search job handler.
//!
//! Searches enabled subtitle providers for a movie or episode, returning
//! results for UI display or automatic download.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{
    movie_files, movies, tv_episode_files, tv_episodes, tv_seasons, tv_shows,
};
use crate::subtitle_providers::{
    EpisodeSubtitleQuery, MovieSubtitleQuery, SubtitleProviderRegistry, SubtitleSearchResult,
    download::download_and_save_subtitle,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

/// Payload for subtitle_search jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleSearchPayload {
    /// Search for subtitles for this movie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub movie_id: Option<Uuid>,
    /// Search for subtitles for this episode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_id: Option<Uuid>,
    /// Languages to search for (ISO 639 codes). If empty, searches all.
    #[serde(default)]
    pub languages: Vec<String>,
    /// Only search for forced/foreign-parts subtitles.
    #[serde(default)]
    pub forced_only: bool,
    /// Automatically download the best result if found.
    #[serde(default)]
    pub auto_download: bool,
}

pub struct SubtitleSearchHandler {
    pool: PgPool,
}

impl SubtitleSearchHandler {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl JobHandler for SubtitleSearchHandler {
    fn job_type(&self) -> &'static str {
        "subtitle_search"
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: SubtitleSearchPayload = serde_json::from_value(payload)?;
        nats_jobs::check_cancelled(&cancel)?;

        let registry = SubtitleProviderRegistry::new(self.pool.clone());

        nats_jobs::check_cancelled(&cancel)?;

        if let Some(movie_id) = payload.movie_id {
            search_movie_subtitles(&self.pool, &registry, movie_id, &payload).await
        } else if let Some(episode_id) = payload.episode_id {
            search_episode_subtitles(&self.pool, &registry, episode_id, &payload).await
        } else {
            Err(JobError::ExecutionFailed(
                "Either movie_id or episode_id must be provided".to_string(),
            ))
        }
    }
}

async fn search_movie_subtitles(
    pool: &PgPool,
    registry: &SubtitleProviderRegistry,
    movie_id: Uuid,
    payload: &SubtitleSearchPayload,
) -> JobResult<Value> {
    let movie = movies::get(pool, movie_id)
        .await?
        .ok_or_else(|| JobError::ExecutionFailed(format!("Movie {movie_id} not found")))?;

    // Extract identifiers for more accurate search
    let imdb_id = movie
        .identifiers
        .get("imdb_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let tmdb_id = movie.identifiers.get("tmdb_id").and_then(|v| {
        v.as_str()
            .map(String::from)
            .or_else(|| v.as_i64().map(|n| n.to_string()))
    });

    let query = MovieSubtitleQuery {
        title: movie.title.clone(),
        year: movie.year.map(|y| y as u16),
        imdb_id,
        tmdb_id,
        languages: payload.languages.clone(),
        forced_only: payload.forced_only,
    };

    info!(movie_id = %movie_id, title = %movie.title, "Searching for movie subtitles");

    let results = registry.search_movie(&query, None).await;

    info!(
        movie_id = %movie_id,
        results_count = results.len(),
        "Movie subtitle search complete"
    );

    let downloaded = if payload.auto_download && !results.is_empty() {
        try_auto_download(pool, registry, &results, Some(movie_id), None).await
    } else {
        None
    };

    Ok(json!({
        "movie_id": movie_id,
        "title": movie.title,
        "results": results,
        "downloaded": downloaded,
    }))
}

async fn search_episode_subtitles(
    pool: &PgPool,
    registry: &SubtitleProviderRegistry,
    episode_id: Uuid,
    payload: &SubtitleSearchPayload,
) -> JobResult<Value> {
    let episode = tv_episodes::get(pool, episode_id)
        .await?
        .ok_or_else(|| JobError::ExecutionFailed(format!("Episode {episode_id} not found")))?;

    let season = tv_seasons::get(pool, episode.season_id)
        .await?
        .ok_or_else(|| {
            JobError::ExecutionFailed(format!("Season {} not found", episode.season_id))
        })?;

    let show = tv_shows::get(pool, season.show_id)
        .await?
        .ok_or_else(|| JobError::ExecutionFailed(format!("Show {} not found", season.show_id)))?;

    let show_imdb_id = show
        .identifiers
        .get("imdb_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let show_tmdb_id = show.identifiers.get("tmdb_id").and_then(|v| {
        v.as_str()
            .map(String::from)
            .or_else(|| v.as_i64().map(|n| n.to_string()))
    });

    let season_num = season.season_sort_order as u32;
    let episode_num = episode.episode_sort_order as u32;

    let query = EpisodeSubtitleQuery {
        show_title: show.title.clone(),
        show_imdb_id,
        show_tmdb_id,
        season: season_num,
        episode: episode_num,
        languages: payload.languages.clone(),
        forced_only: payload.forced_only,
    };

    info!(
        episode_id = %episode_id,
        show = %show.title,
        season = season_num,
        episode = episode_num,
        "Searching for episode subtitles"
    );

    let results = registry.search_episode(&query, None).await;

    info!(
        episode_id = %episode_id,
        results_count = results.len(),
        "Episode subtitle search complete"
    );

    let downloaded = if payload.auto_download && !results.is_empty() {
        try_auto_download(pool, registry, &results, None, Some(episode_id)).await
    } else {
        None
    };

    Ok(json!({
        "episode_id": episode_id,
        "show_title": show.title,
        "season": season_num,
        "episode": episode_num,
        "results": results,
        "downloaded": downloaded,
    }))
}

/// Pick the best subtitle result and download it.
///
/// Best = highest `download_count` (most popular, most reliable).
/// Returns a JSON summary of what was downloaded, or None on failure.
async fn try_auto_download(
    pool: &PgPool,
    registry: &SubtitleProviderRegistry,
    results: &[SubtitleSearchResult],
    movie_id: Option<Uuid>,
    episode_id: Option<Uuid>,
) -> Option<Value> {
    let best = results
        .iter()
        .max_by_key(|r| r.download_count.unwrap_or(0))?;

    // Look up the video file path
    let (video_file_path, movie_file_id, episode_file_id) = if let Some(mid) = movie_id {
        match movie_files::get_preferred_available(pool, mid).await {
            Ok(Some(f)) => (f.file_path, Some(f.id), None),
            Ok(None) => {
                tracing::warn!(movie_id = %mid, "No available movie file for auto-download");
                return None;
            }
            Err(e) => {
                tracing::warn!(movie_id = %mid, error = %e, "Failed to get movie file for auto-download");
                return None;
            }
        }
    } else {
        let eid = episode_id?;
        match tv_episode_files::get_preferred_available(pool, eid).await {
            Ok(Some(f)) => (f.file_path, None, Some(f.id)),
            Ok(None) => {
                tracing::warn!(episode_id = %eid, "No available episode file for auto-download");
                return None;
            }
            Err(e) => {
                tracing::warn!(episode_id = %eid, error = %e, "Failed to get episode file for auto-download");
                return None;
            }
        }
    };

    match download_and_save_subtitle(
        pool,
        registry,
        &best.provider,
        &best.id,
        &video_file_path,
        movie_file_id,
        episode_file_id,
        Some(&best.language),
        best.is_forced,
        best.is_hearing_impaired,
    )
    .await
    {
        Ok((sub_id, file_path)) => {
            info!(
                subtitle_id = %sub_id,
                file_path = %file_path,
                provider = %best.provider,
                "Auto-downloaded subtitle"
            );
            Some(json!({
                "subtitle_id": sub_id,
                "file_path": file_path,
                "provider": best.provider,
                "language": best.language,
            }))
        }
        Err(e) => {
            tracing::warn!(error = %e, "Auto-download failed");
            None
        }
    }
}
