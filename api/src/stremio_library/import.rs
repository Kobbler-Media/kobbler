use std::collections::HashMap;

use sqlx::PgPool;
use tracing::warn;
use uuid::Uuid;

use crate::images::{ImageEntry, ImageMap};
use crate::jobs::handlers::movie_enrich::enrich_movie;
use crate::jobs::handlers::tv_show_enrich::enrich_tv_show;
use crate::repositories::stremio_history::{self, PendingImport};
use crate::repositories::{media_roots, movies, tv_shows};

/// Applies every selected title Stremio has played since its last import. A failure stays on
/// that selection and is retried on the next sync.
pub async fn apply_pending(pool: &PgPool, account_id: Uuid) -> Vec<String> {
    let pending = match stremio_history::list_pending(pool, account_id).await {
        Ok(p) => p,
        Err(e) => return vec![format!("Failed to list Stremio history imports: {e}")],
    };
    let mut errors = Vec::new();
    for p in pending {
        let result = apply(pool, &p).await;
        let error = result.as_ref().err().map(|e| format!("{}: {e}", p.title));
        if let Some(e) = &error {
            warn!(stremio_id = %p.stremio_id, "Stremio history import: {e}");
            errors.push(e.clone());
        }
        let applied = result.is_ok().then_some(p.last_watched);
        if let Err(e) = stremio_history::record_import(
            pool,
            p.user_id,
            account_id,
            &p.stremio_id,
            applied,
            error.as_deref(),
        )
        .await
        {
            errors.push(format!("Failed to record Stremio history import: {e}"));
        }
    }
    errors
}

async fn apply(pool: &PgPool, p: &PendingImport) -> Result<(), String> {
    if p.media_type == "movie" {
        let movie_id = ensure_movie(pool, p).await?;
        return stremio_history::mark_movie(pool, movie_id, p)
            .await
            .map_err(|e| e.to_string());
    }
    let (Some(season), Some(episode)) = (p.season, p.episode) else {
        return Ok(());
    };
    let show_id = ensure_show(pool, p).await?;
    if !stremio_history::episode_exists(pool, show_id, season, episode)
        .await
        .map_err(|e| e.to_string())?
    {
        enrich_tv_show(pool, show_id, false)
            .await
            .map_err(|e| e.to_string())?;
    }
    stremio_history::mark_episodes(pool, show_id, season, episode, p)
        .await
        .map_err(|e| e.to_string())
}

async fn ensure_movie(pool: &PgPool, p: &PendingImport) -> Result<Uuid, String> {
    if let Some(m) = movies::get_by_identifier(pool, "tmdb_id", &p.tmdb_id)
        .await
        .map_err(|e| e.to_string())?
    {
        return Ok(m.id);
    }
    let input = movies::MovieInput {
        title: p.title.clone(),
        year: p.year,
        images: poster(p),
        media_root_id: default_root(pool, "movie").await,
        ..Default::default()
    };
    let id = movies::upsert_by_provider(pool, "tmdb", &p.tmdb_id, input)
        .await
        .map_err(|e| e.to_string())?;
    if let Err(e) = enrich_movie(pool, id, false).await {
        warn!(movie_id = %id, "Enriching a movie imported from Stremio: {e}");
    }
    Ok(id)
}

async fn ensure_show(pool: &PgPool, p: &PendingImport) -> Result<Uuid, String> {
    if let Some(s) = tv_shows::get_by_identifier(pool, "tmdb_id", &p.tmdb_id)
        .await
        .map_err(|e| e.to_string())?
    {
        return Ok(s.id);
    }
    let input = tv_shows::TvShowInput {
        title: p.title.clone(),
        year: p.year,
        images: poster(p),
        is_monitored: Some(true),
        auto_want_mode: Some("none".to_string()),
        media_root_id: default_root(pool, "tv").await,
        ..Default::default()
    };
    tv_shows::upsert_by_provider(pool, "tmdb", &p.tmdb_id, input)
        .await
        .map_err(|e| e.to_string())
}

fn poster(p: &PendingImport) -> Option<ImageMap> {
    p.poster_url.as_ref().map(|url| {
        HashMap::from([(
            "poster".to_string(),
            ImageEntry {
                source_url: Some(url.clone()),
                cached_at: None,
            },
        )])
    })
}

async fn default_root(pool: &PgPool, media_type: &str) -> Option<Uuid> {
    media_roots::get_default_for_type(pool, media_type)
        .await
        .ok()
        .flatten()
        .map(|r| r.id)
}
