//! Where an acquired movie or episode file goes in the library and what registering it
//! there involves. Shared by every way a video file is acquired: kobload deliveries and
//! debrid streams kept in the library.

use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::kobload::{check_ingest_media_root, sanitize_filename};
use crate::AppState;
use crate::media_root_naming::{self, NamingContext};
use crate::repositories::media_roots::EnabledMediaRoot;
use crate::repositories::{
    media_root_files, media_roots, movies, tv_episodes, tv_seasons, tv_shows,
};
use crate::runtime_config::RuntimeConfig;

pub struct Placement {
    pub media_root: Option<EnabledMediaRoot>,
    pub dir: PathBuf,
    /// The naming template's file stem; `None` keeps the file's own (sanitized) name.
    pub stem: Option<String>,
}

impl Placement {
    pub fn file_path(&self, extension: &str, original_name: &str) -> PathBuf {
        match &self.stem {
            Some(stem) => self
                .dir
                .join(format!("{stem}.{}", extension.to_lowercase())),
            None => self.dir.join(sanitize_filename(original_name)),
        }
    }
}

/// The movie's media root (adopting the default one for movies if it has none), and the
/// path its naming template gives. The error is the reply when that root is unavailable.
pub async fn movie(
    state: &AppState,
    runtime: &RuntimeConfig,
    movie_id: Uuid,
    title: &str,
    year: Option<i32>,
) -> Result<Placement, axum::response::Response> {
    let movie_row = movies::get(&state.pool, movie_id).await.ok().flatten();
    let media_root = match movie_row.as_ref().and_then(|m| m.media_root_id) {
        Some(root_id) => media_roots::get_enabled_by_id(&state.pool, root_id)
            .await
            .ok()
            .flatten(),
        None => {
            let default = media_roots::get_default_for_type(&state.pool, "movie")
                .await
                .ok()
                .flatten();
            if let Some(ref root) = default {
                let _ = movies::update(
                    &state.pool,
                    movie_id,
                    movies::MovieUpdate {
                        media_root_id: Some(Some(root.id)),
                        ..Default::default()
                    },
                )
                .await;
            }
            default
        }
    };
    if let Some(ref root) = media_root
        && let Some(resp) = check_ingest_media_root(state, root).await
    {
        return Err(resp);
    }

    let Some(root) = media_root else {
        let video_root = runtime
            .video_storage_root
            .clone()
            .unwrap_or_else(|| format!("{}/video", state.config.storage_root));
        let dir_name = match year {
            Some(y) => format!("{} ({})", sanitize_filename(title), y),
            None => sanitize_filename(title),
        };
        return Ok(Placement {
            media_root: None,
            dir: PathBuf::from(&video_root).join("movies").join(dir_name),
            stem: None,
        });
    };
    let template = root
        .naming_template
        .as_deref()
        .unwrap_or_else(|| media_root_naming::default_movie_template());
    let ctx = NamingContext {
        title: Some(title.to_string()),
        year: year.map(|y| y as u32),
        // Placeholder: the real extension is applied per file.
        ext: Some("mkv".to_string()),
        ..Default::default()
    };
    let fresh = media_root_naming::compute_target_path(&root.path, template, &ctx);
    // An existing directory wins over a fresh one when the DB title differs from the one
    // on disk, so a second file does not start a duplicate folder.
    let full_path = if let Some(existing) =
        media_root_naming::find_existing_file_for_entity(&state.pool, root.id, "movie", movie_id)
            .await
        && let Some(existing_dir) =
            media_root_naming::extract_dir_from_existing(&root.path, &existing, 1)
    {
        media_root_naming::reuse_existing_dir(&root.path, &existing_dir, &fresh, 1)
    } else {
        fresh
    };
    Ok(placement_for(Some(root), &full_path))
}

/// What an episode's file is named after.
pub struct EpisodeNaming {
    pub show_id: Option<Uuid>,
    pub show_title: String,
    pub show_year: Option<u32>,
    pub season_identifier: String,
    pub episode_num: u32,
    pub episode_title: Option<String>,
    pub is_sport: bool,
    show_root_id: Option<Uuid>,
}

impl EpisodeNaming {
    pub async fn load(
        state: &AppState,
        episode_id: Uuid,
        show_id: Option<Uuid>,
        season_id: Option<Uuid>,
        fallback_show_title: Option<&str>,
    ) -> Self {
        let show_row = match show_id {
            Some(sid) => tv_shows::get(&state.pool, sid).await.ok().flatten(),
            None => None,
        };
        let season_identifier = match season_id {
            Some(sid) => tv_seasons::get(&state.pool, sid)
                .await
                .ok()
                .flatten()
                .map(|s| s.season_identifier),
            None => None,
        }
        .unwrap_or_else(|| "1".to_string());
        let episode_row = tv_episodes::get(&state.pool, episode_id)
            .await
            .ok()
            .flatten();
        Self {
            show_id,
            show_title: show_row
                .as_ref()
                .map(|s| s.title.clone())
                .or_else(|| fallback_show_title.map(String::from))
                .unwrap_or_else(|| "Unknown Show".to_string()),
            show_year: show_row.as_ref().and_then(|s| s.year).map(|y| y as u32),
            season_identifier,
            episode_num: episode_row
                .as_ref()
                .map(|e| e.episode_sort_order as u32)
                .unwrap_or(1),
            episode_title: episode_row.and_then(|e| e.title),
            is_sport: show_row
                .as_ref()
                .is_some_and(|s| s.content_category == "sport"),
            show_root_id: show_row.and_then(|s| s.media_root_id),
        }
    }
}

/// The show's media root (adopting the default one for TV if it has none), and the path
/// its naming template gives the episode.
pub async fn episode(
    state: &AppState,
    runtime: &RuntimeConfig,
    naming: &EpisodeNaming,
) -> Result<Placement, axum::response::Response> {
    let media_root = match naming.show_root_id {
        Some(root_id) => media_roots::get_enabled_by_id(&state.pool, root_id)
            .await
            .ok()
            .flatten(),
        None => {
            let default = media_roots::get_default_for_type(&state.pool, "tv")
                .await
                .ok()
                .flatten();
            if let Some(ref root) = default
                && let Some(sid) = naming.show_id
            {
                let _ = tv_shows::update(
                    &state.pool,
                    sid,
                    tv_shows::TvShowUpdate {
                        media_root_id: Some(Some(root.id)),
                        ..Default::default()
                    },
                )
                .await;
            }
            default
        }
    };
    if let Some(ref root) = media_root
        && let Some(resp) = check_ingest_media_root(state, root).await
    {
        return Err(resp);
    }

    let Some(root) = media_root else {
        let video_root = runtime
            .video_storage_root
            .clone()
            .unwrap_or_else(|| format!("{}/video", state.config.storage_root));
        return Ok(Placement {
            media_root: None,
            dir: PathBuf::from(&video_root)
                .join("tv")
                .join(sanitize_filename(&naming.show_title))
                .join(format!("Season {}", naming.season_identifier)),
            stem: None,
        });
    };
    let template =
        root.naming_template
            .as_deref()
            .unwrap_or_else(|| match root.media_type.as_str() {
                "sport" => media_root_naming::default_sport_template(),
                _ => media_root_naming::default_tv_template(),
            });
    let ctx = NamingContext {
        show_title: Some(naming.show_title.clone()),
        year: naming.show_year,
        season: Some(naming.season_identifier.parse().unwrap_or(1)),
        episode: Some(naming.episode_num),
        episode_title: naming.episode_title.clone(),
        round: naming.is_sport.then_some(naming.episode_num / 1000),
        // Placeholder: the real extension is applied per file.
        ext: Some("mkv".to_string()),
        ..Default::default()
    };
    let fresh = media_root_naming::compute_target_path(&root.path, template, &ctx);
    // An existing show directory wins over a fresh one when the DB title differs from the
    // one on disk.
    let full_path = if let Some(sid) = naming.show_id
        && let Some(existing) =
            media_root_naming::find_existing_file_for_entity(&state.pool, root.id, "tv_show", sid)
                .await
        && let Some(existing_dir) =
            media_root_naming::extract_dir_from_existing(&root.path, &existing, 1)
    {
        media_root_naming::reuse_existing_dir(&root.path, &existing_dir, &fresh, 1)
    } else {
        fresh
    };
    Ok(placement_for(Some(root), &full_path))
}

fn placement_for(media_root: Option<EnabledMediaRoot>, full_path: &Path) -> Placement {
    Placement {
        media_root,
        dir: full_path.parent().unwrap_or(full_path).to_path_buf(),
        stem: full_path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(String::from),
    }
}

/// Probe the new file — which chains extraction, analysis and skip detection — and list
/// it under its media root so it shows as added straight away.
pub async fn announce_file(
    state: &AppState,
    placement: &Placement,
    file_path: &Path,
    kind: FileKind,
    file_id: Uuid,
    entity_id: Uuid,
) {
    let (entity_type, media_type, root_kind) = match kind {
        FileKind::Movie => ("movie_file", "movie", "movie"),
        FileKind::Episode => ("tv_episode_file", "episode", "tv_episode"),
    };
    let display = crate::jobs::fetch_job_display(&state.pool, entity_type, file_id).await;
    let context =
        crate::jobs::JobContext::for_entity(entity_type, file_id, "Probing media file".to_string())
            .with_display_opt(display);
    let _ = crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "media_probe",
        serde_json::json!({"file_id": file_id, "media_type": media_type}),
        context,
    )
    .await;

    if let Some(ref root) = placement.media_root {
        let name = file_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let size = tokio::fs::metadata(file_path)
            .await
            .ok()
            .map(|m| m.len() as i64);
        if let Err(e) = media_root_files::upsert_added(
            &state.pool,
            root.id,
            &file_path.to_string_lossy(),
            &name,
            size,
            root_kind,
            entity_id,
        )
        .await
        {
            tracing::warn!(error = %e, "Failed to register file in media_root_files");
        }
    }
}

#[derive(Clone, Copy)]
pub enum FileKind {
    Movie,
    Episode,
}

/// The library side of a finished acquisition: the item is imported, recorded as such,
/// and its sidecars and images are written next to the file.
pub async fn complete_movie(state: &AppState, movie_id: Uuid) {
    let _ = movies::clear_rejected_releases(&state.pool, movie_id).await;
    let _ = movies::set_want_status(&state.pool, movie_id, "imported").await;
    crate::routes::library::helpers::record_activity_for_entity(
        &state.pool,
        "imported",
        movie_id,
        crate::routes::library::helpers::requester_of(&state.pool, movie_id).await,
    )
    .await;
    crate::sidecar::write_back::write_movie_sidecars(&state.pool, movie_id).await;
    crate::sidecar::write_back::write_movie_images(
        &state.pool,
        &state.config.storage_root,
        movie_id,
    )
    .await;
}

pub async fn complete_episode(state: &AppState, episode_id: Uuid, show_id: Option<Uuid>) {
    let _ = tv_episodes::clear_rejected_releases(&state.pool, episode_id).await;
    let _ = tv_episodes::set_want_status(&state.pool, episode_id, "imported").await;
    crate::routes::library::helpers::record_activity_for_entity(
        &state.pool,
        "imported",
        episode_id,
        crate::routes::library::helpers::requester_of(&state.pool, episode_id).await,
    )
    .await;
    crate::sidecar::write_back::write_episode_sidecar(&state.pool, episode_id).await;
    if let Some(sid) = show_id {
        crate::sidecar::write_back::write_tvshow_images(
            &state.pool,
            &state.config.storage_root,
            sid,
        )
        .await;
    }
}
