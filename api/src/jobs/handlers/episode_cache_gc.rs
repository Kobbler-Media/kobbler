//! Janitor for `{storage_root}/episode_cache/`.
//!
//! Walks each of the three namespaces and removes directories whose
//! authoritative DB row no longer exists. Also drops `debrid_episode/{id}/`
//! entries when the episode now has a `tv_episode_files` row — the local
//! file's `tv_file/{file_id}/` cache supersedes.
//!
//! Independent of `debrid_cache_cleanup`, which only touches the per-file
//! `media_cache/` tree adjacent to debrid-streamed video files. The two
//! caches are decoupled by design: per-file dies with the video, episode_cache
//! survives.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::media_cache::MIN_SUPPORTED_PIPELINE_VERSION;
use crate::media_cache::episode_cache::{MediaKind, read_manifest};
use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

pub struct EpisodeCacheGcHandler {
    pool: PgPool,
    storage_root: String,
}

impl EpisodeCacheGcHandler {
    pub fn new(pool: PgPool, storage_root: String) -> Self {
        Self { pool, storage_root }
    }
}

#[async_trait]
impl JobHandler for EpisodeCacheGcHandler {
    fn job_type(&self) -> &'static str {
        "episode_cache_gc"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        _payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let live_files: HashSet<Uuid> = sqlx::query_scalar!("SELECT id FROM tv_episode_files")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| JobError::Retryable(e.to_string()))?
            .into_iter()
            .collect();
        let live_episodes: HashSet<Uuid> = sqlx::query_scalar!("SELECT id FROM tv_episodes")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| JobError::Retryable(e.to_string()))?
            .into_iter()
            .collect();
        let live_movies: HashSet<Uuid> = sqlx::query_scalar!("SELECT id FROM movies")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| JobError::Retryable(e.to_string()))?
            .into_iter()
            .collect();
        // Episodes that now have a local file row — their debrid_episode/
        // copy is superseded and can be dropped.
        let episodes_with_files: HashSet<Uuid> =
            sqlx::query_scalar!("SELECT DISTINCT episode_id FROM tv_episode_file_episodes")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| JobError::Retryable(e.to_string()))?
                .into_iter()
                .collect();

        // All three sweeps are synchronous filesystem work over the episode_cache
        // tree -- a read_dir across every namespace plus a recursive remove per
        // dead entry. On a runtime worker that starves tokio's I/O driver and
        // the whole API stops answering until the GC finishes, so run them
        // together on the blocking pool.
        let (removed_orphans, removed_stale, removed_superseded) = {
            let storage_root = self.storage_root.clone();
            let cancel = cancel.clone();
            tokio::task::spawn_blocking(move || {
                let mut removed_orphans = 0usize;
                let mut removed_stale = 0usize;
                let mut removed_superseded = 0usize;

                removed_orphans += sweep(
                    &storage_root,
                    Namespace::TvFile,
                    &live_files,
                    &cancel,
                    &mut removed_stale,
                    &HashSet::new(),
                    &mut removed_superseded,
                );
                removed_orphans += sweep(
                    &storage_root,
                    Namespace::Movie,
                    &live_movies,
                    &cancel,
                    &mut removed_stale,
                    &HashSet::new(),
                    &mut removed_superseded,
                );
                removed_orphans += sweep(
                    &storage_root,
                    Namespace::DebridEpisode,
                    &live_episodes,
                    &cancel,
                    &mut removed_stale,
                    &episodes_with_files,
                    &mut removed_superseded,
                );
                (removed_orphans, removed_stale, removed_superseded)
            })
            .await
            .expect("episode_cache_gc sweep task panicked")
        };

        info!(
            removed_orphans,
            removed_stale, removed_superseded, "episode_cache_gc complete"
        );
        Ok(json!({
            "removed_orphans": removed_orphans,
            "removed_stale": removed_stale,
            "removed_superseded": removed_superseded,
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Namespace {
    TvFile,
    Movie,
    DebridEpisode,
}

impl Namespace {
    fn dir_name(self) -> &'static str {
        match self {
            Namespace::TvFile => "tv_file",
            Namespace::Movie => "movie",
            Namespace::DebridEpisode => "debrid_episode",
        }
    }

    fn make_kind(self, id: Uuid) -> MediaKind {
        match self {
            Namespace::TvFile => MediaKind::TvFile { file_id: id },
            Namespace::Movie => MediaKind::Movie { movie_id: id },
            Namespace::DebridEpisode => MediaKind::DebridEpisode { episode_id: id },
        }
    }
}

fn namespace_dir(storage_root: &str, ns: Namespace) -> PathBuf {
    PathBuf::from(storage_root)
        .join("episode_cache")
        .join(ns.dir_name())
}

#[allow(clippy::too_many_arguments)]
fn sweep(
    storage_root: &str,
    ns: Namespace,
    live_ids: &HashSet<Uuid>,
    cancel: &CancellationToken,
    removed_stale: &mut usize,
    superseded_ids: &HashSet<Uuid>,
    removed_superseded: &mut usize,
) -> usize {
    let dir = namespace_dir(storage_root, ns);
    if !dir.is_dir() {
        return 0;
    }

    let read = match std::fs::read_dir(&dir) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, dir = %dir.display(), "episode_cache_gc: read_dir failed");
            return 0;
        }
    };

    let mut removed = 0;
    for entry in read.flatten() {
        if cancel.is_cancelled() {
            break;
        }
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Ok(id) = Uuid::parse_str(name) else {
            continue;
        };

        if !live_ids.contains(&id) {
            // Orphan: id no longer in DB.
            try_rm(&path, "orphan");
            removed += 1;
            continue;
        }

        if ns == Namespace::DebridEpisode && superseded_ids.contains(&id) {
            // Local copy now exists; debrid_episode/ is superseded.
            try_rm(&path, "superseded");
            *removed_superseded += 1;
            continue;
        }

        // Live id — check manifest version. Stale = remove so next analyze regenerates.
        let kind = ns.make_kind(id);
        let manifest = read_manifest(storage_root, &kind);
        if manifest
            .as_ref()
            .is_some_and(|m| m.pipeline_version < MIN_SUPPORTED_PIPELINE_VERSION)
        {
            try_rm(&path, "stale");
            *removed_stale += 1;
        }
    }
    removed
}

fn try_rm(path: &Path, reason: &str) {
    if let Err(e) = std::fs::remove_dir_all(path) {
        warn!(
            error = %e,
            path = %path.display(),
            reason,
            "episode_cache_gc: failed to remove dir"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_cache::PIPELINE_VERSION;
    use crate::media_cache::episode_cache::{
        EpisodeArtefacts, episode_cache_dir, write_episode_cache,
    };
    use crate::media_cache::writers::{ArtefactMap, Manifest};
    use chrono::Utc;
    use tempfile::TempDir;

    fn fresh_manifest(version: i32) -> Manifest {
        Manifest {
            pipeline_version: version,
            generated_at: Utc::now(),
            input_duration_seconds: 100.0,
            trickplay_dir: "/dev/null".into(),
            artefacts: ArtefactMap::default(),
            analyzed_file_id: Some(Uuid::now_v7()),
            analyzed_at: Some(Utc::now()),
            input_size_bytes: None,
            input_mtime: None,
        }
    }

    fn no_artefacts() -> EpisodeArtefacts<'static> {
        EpisodeArtefacts {
            vhash: None,
            chromaprint: None,
            silence: None,
            loudness: None,
            black_frames: None,
            embeddings: None,
        }
    }

    #[tokio::test]
    async fn sweep_removes_orphan_dirs_only() {
        let tmp = TempDir::new().unwrap();
        let storage = tmp.path().to_string_lossy().into_owned();
        let live_id = Uuid::now_v7();
        let orphan_id = Uuid::now_v7();

        write_episode_cache(
            &storage,
            &MediaKind::TvFile { file_id: live_id },
            &fresh_manifest(PIPELINE_VERSION),
            &no_artefacts(),
        )
        .unwrap();
        write_episode_cache(
            &storage,
            &MediaKind::TvFile { file_id: orphan_id },
            &fresh_manifest(PIPELINE_VERSION),
            &no_artefacts(),
        )
        .unwrap();

        let live: HashSet<Uuid> = std::iter::once(live_id).collect();
        let mut removed_stale = 0usize;
        let mut removed_superseded = 0usize;
        let removed = sweep(
            &storage,
            Namespace::TvFile,
            &live,
            &CancellationToken::new(),
            &mut removed_stale,
            &HashSet::new(),
            &mut removed_superseded,
        );
        assert_eq!(removed, 1);
        assert_eq!(removed_stale, 0);
        assert!(episode_cache_dir(&storage, &MediaKind::TvFile { file_id: live_id }).is_dir());
        assert!(!episode_cache_dir(&storage, &MediaKind::TvFile { file_id: orphan_id }).exists());
    }

    #[tokio::test]
    async fn sweep_removes_stale_pipeline_version_for_live_id() {
        let tmp = TempDir::new().unwrap();
        let storage = tmp.path().to_string_lossy().into_owned();
        let live_id = Uuid::now_v7();

        write_episode_cache(
            &storage,
            &MediaKind::Movie { movie_id: live_id },
            &fresh_manifest(MIN_SUPPORTED_PIPELINE_VERSION - 1),
            &no_artefacts(),
        )
        .unwrap();
        let live: HashSet<Uuid> = std::iter::once(live_id).collect();
        let mut removed_stale = 0usize;
        let mut removed_superseded = 0usize;
        let removed = sweep(
            &storage,
            Namespace::Movie,
            &live,
            &CancellationToken::new(),
            &mut removed_stale,
            &HashSet::new(),
            &mut removed_superseded,
        );
        assert_eq!(removed, 0);
        assert_eq!(removed_stale, 1);
        assert!(!episode_cache_dir(&storage, &MediaKind::Movie { movie_id: live_id }).exists());
    }

    #[tokio::test]
    async fn sweep_removes_superseded_debrid_episode_when_local_exists() {
        let tmp = TempDir::new().unwrap();
        let storage = tmp.path().to_string_lossy().into_owned();
        let ep_id = Uuid::now_v7();

        write_episode_cache(
            &storage,
            &MediaKind::DebridEpisode { episode_id: ep_id },
            &fresh_manifest(PIPELINE_VERSION),
            &no_artefacts(),
        )
        .unwrap();
        let live_episodes: HashSet<Uuid> = std::iter::once(ep_id).collect();
        let superseded: HashSet<Uuid> = std::iter::once(ep_id).collect();
        let mut removed_stale = 0usize;
        let mut removed_superseded = 0usize;
        let removed = sweep(
            &storage,
            Namespace::DebridEpisode,
            &live_episodes,
            &CancellationToken::new(),
            &mut removed_stale,
            &superseded,
            &mut removed_superseded,
        );
        assert_eq!(removed, 0, "live episode, not orphan");
        assert_eq!(removed_superseded, 1);
        assert!(
            !episode_cache_dir(&storage, &MediaKind::DebridEpisode { episode_id: ep_id }).exists()
        );
    }

    #[tokio::test]
    async fn sweep_skips_non_uuid_dir_names() {
        let tmp = TempDir::new().unwrap();
        let storage = tmp.path().to_string_lossy().into_owned();
        let dir = namespace_dir(&storage, Namespace::TvFile);
        std::fs::create_dir_all(dir.join("not-a-uuid")).unwrap();

        let mut removed_stale = 0usize;
        let mut removed_superseded = 0usize;
        let removed = sweep(
            &storage,
            Namespace::TvFile,
            &HashSet::new(),
            &CancellationToken::new(),
            &mut removed_stale,
            &HashSet::new(),
            &mut removed_superseded,
        );
        assert_eq!(removed, 0);
        assert_eq!(removed_stale, 0);
        assert!(dir.join("not-a-uuid").is_dir());
    }
}
