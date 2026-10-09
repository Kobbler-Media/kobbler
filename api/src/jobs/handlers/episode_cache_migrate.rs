//! One-shot migration that moves content-bound sidecars out of the per-file
//! `media_cache/{file_id}/` layer and into the new
//! `episode_cache/{namespace}/{id}/` layout.
//!
//! Walks every `media_cache/{file_id}/` directory present on disk, looks the
//! file up in `tv_episode_files` (→ `tv_file/{file_id}/`) or `movie_files`
//! (→ `movie/{movie_id}/`), and renames each content sidecar across the
//! filesystem. Trickplay + audio_peaks + source.tmp stay put — those are
//! file-bound by design. The per-file manifest's content fields are pruned
//! so future reads describe only what's still next to it.
//!
//! Idempotent — files already at the destination are left alone, files
//! already absent at the source are skipped.
//!
//! Wired to fire on server startup via the scheduler's `run_immediately`.
//! After a clean run with zero errors, writes
//! `<storage_root>/episode_cache/.migrated_at` so subsequent startups
//! short-circuit without an N-stat filesystem walk. Delete that marker file
//! to re-arm the walk (e.g. after a deploy rollback that re-introduces
//! legacy sidecars).

use std::path::{Path, PathBuf};

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::media_cache::episode_cache::{MediaKind, episode_cache_dir};
use crate::media_cache::writers::{Manifest, write_manifest_at};
use crate::media_cache::{Branch, sidecar_filename};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// Sidecars that must move out of media_cache/. Trickplay (a directory) and
/// audio_peaks (file-bound) stay put.
const CONTENT_BRANCHES: &[Branch] = &[
    Branch::Vhash,
    Branch::Chromaprint,
    Branch::Silence,
    Branch::Loudness,
    Branch::BlackFrames,
    Branch::Embeddings,
];

/// Sentinel file written under `episode_cache/` after a clean migration run.
/// Its presence on a subsequent invocation skips the per-file_id stat walk.
fn marker_path(storage_root: &str) -> PathBuf {
    PathBuf::from(storage_root)
        .join("episode_cache")
        .join(".migrated_at")
}

pub struct EpisodeCacheMigrateHandler {
    pool: PgPool,
    storage_root: String,
}

impl EpisodeCacheMigrateHandler {
    pub fn new(pool: PgPool, storage_root: String) -> Self {
        Self { pool, storage_root }
    }
}

#[async_trait]
impl JobHandler for EpisodeCacheMigrateHandler {
    fn job_type(&self) -> &'static str {
        "episode_cache_migrate"
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
        // Marker short-circuit: a successful prior run wrote
        // `episode_cache/.migrated_at`. Skip the FS walk entirely until an
        // operator deletes the marker (e.g. after a rollback).
        let marker = marker_path(&self.storage_root);
        if marker.is_file() {
            info!(
                marker = %marker.display(),
                "episode_cache_migrate: marker present — skipping walk"
            );
            return Ok(
                json!({ "status": "marker_present", "marker": marker.display().to_string() }),
            );
        }

        let media_cache_root = PathBuf::from(&self.storage_root).join("media_cache");
        if !media_cache_root.is_dir() {
            // Fresh install (no media_cache yet) — write the marker so
            // future startups skip immediately.
            write_marker(&marker);
            return Ok(json!({ "status": "no_media_cache_dir" }));
        }

        // Build (file_id → MediaKind) lookups from the DB once.
        let tv_files: HashMap<Uuid, MediaKind> =
            sqlx::query_scalar!("SELECT id FROM tv_episode_files")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| JobError::Retryable(e.to_string()))?
                .into_iter()
                .map(|file_id| (file_id, MediaKind::TvFile { file_id }))
                .collect();
        let movie_files: HashMap<Uuid, MediaKind> =
            sqlx::query!(r#"SELECT id AS "id!", movie_id AS "movie_id!" FROM movie_files"#)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| JobError::Retryable(e.to_string()))?
                .into_iter()
                .map(|r| {
                    (
                        r.id,
                        MediaKind::Movie {
                            movie_id: r.movie_id,
                        },
                    )
                })
                .collect();

        // The walk below is entirely synchronous filesystem work over the whole
        // media_cache tree -- ~15k directories here, with a rename per sidecar.
        // On a runtime worker that starves tokio's I/O driver and the API stops
        // answering for as long as the migration runs, so it goes to the
        // blocking pool. The marker write goes with it to keep the whole
        // filesystem phase in one place.
        let walk = {
            let storage_root = self.storage_root.clone();
            let cancel = cancel.clone();
            tokio::task::spawn_blocking(move || {
                let mut migrated = 0usize;
                let mut skipped = 0usize;
                let mut errors = 0usize;

                let read = match std::fs::read_dir(&media_cache_root) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(error = %e, "failed to read media_cache root");
                        return Err(e.to_string());
                    }
                };

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
                    let Ok(file_id) = Uuid::parse_str(name) else {
                        continue;
                    };

                    let target = if let Some(k) = tv_files.get(&file_id) {
                        *k
                    } else if let Some(k) = movie_files.get(&file_id) {
                        *k
                    } else {
                        // No DB row claims this file_id. Leave it to gc / orphan_cleanup.
                        skipped += 1;
                        continue;
                    };

                    match migrate_one(&storage_root, &path, file_id, &target) {
                        Ok(true) => migrated += 1,
                        Ok(false) => skipped += 1,
                        Err(e) => {
                            errors += 1;
                            warn!(
                                error = %e,
                                path = %path.display(),
                                "episode_cache_migrate: failed to move sidecars"
                            );
                        }
                    }
                }

                // Only short-circuit future runs if the walk was complete and
                // clean. A cancelled or partially-failed walk leaves the marker
                // absent so the next startup retries.
                let marker_written = if errors == 0 && !cancel.is_cancelled() {
                    write_marker(&marker);
                    true
                } else {
                    false
                };
                Ok((migrated, skipped, errors, marker_written))
            })
            .await
            .expect("episode_cache_migrate walk task panicked")
        };

        let (migrated, skipped, errors, marker_written) = match walk {
            Ok(v) => v,
            Err(e) => return Ok(json!({"status": "read_dir_failed", "error": e})),
        };

        info!(migrated, skipped, errors, "episode_cache_migrate complete");
        Ok(json!({
            "migrated": migrated,
            "skipped": skipped,
            "errors": errors,
            "marker_written": marker_written,
        }))
    }
}

fn write_marker(path: &Path) {
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        warn!(error = %e, "episode_cache_migrate: marker parent create failed");
        return;
    }
    let stamp = Utc::now().to_rfc3339();
    if let Err(e) = std::fs::write(path, stamp.as_bytes()) {
        warn!(error = %e, path = %path.display(), "episode_cache_migrate: marker write failed");
    }
}

/// Migrate one `media_cache/{file_id}/` directory. Returns `Ok(true)` if any
/// content sidecar moved, `Ok(false)` if there was nothing to move (idempotent
/// path — re-run on already-migrated data).
fn migrate_one(
    storage_root: &str,
    file_dir: &Path,
    file_id: Uuid,
    target: &MediaKind,
) -> std::io::Result<bool> {
    let dest_dir = episode_cache_dir(storage_root, target);
    let mut moved_any = false;

    for &branch in CONTENT_BRANCHES {
        let Some(name) = sidecar_filename(branch) else {
            continue;
        };
        let src = file_dir.join(name);
        if !src.is_file() {
            continue;
        }
        let dest = dest_dir.join(name);
        if dest.is_file() {
            // Already migrated — leave the source and let it be reconciled
            // by the per-file manifest pruning below.
            continue;
        }
        std::fs::create_dir_all(&dest_dir)?;
        // rename is atomic when src + dest live on the same filesystem.
        // /data/media_cache and /data/episode_cache live under the same root
        // so this is the cheap path.
        std::fs::rename(&src, &dest)?;
        moved_any = true;
    }

    // Update the per-file manifest: drop the content branches whose data
    // moved out. Build a fresh content manifest at the destination too so
    // readers see provenance.
    let per_file_manifest_path = file_dir.join("manifest.json");
    let mut per_file_manifest: Option<Manifest> = std::fs::read(&per_file_manifest_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());

    if let Some(ref mut m) = per_file_manifest {
        // Move the content artefact info into a fresh content manifest, then
        // strip those entries from the per-file copy.
        let content_artefacts = m.artefacts.clone();
        m.artefacts.vhash = None;
        m.artefacts.chromaprint = None;
        m.artefacts.silence = None;
        m.artefacts.loudness = None;
        m.artefacts.black_frames = None;
        m.artefacts.embeddings = None;

        let mut content_manifest = m.clone();
        content_manifest.artefacts = content_artefacts;
        content_manifest.artefacts.trickplay = None;
        content_manifest.artefacts.audio_peaks = None;
        content_manifest.analyzed_file_id = Some(file_id);
        content_manifest.analyzed_at = Some(Utc::now());
        if let Err(e) = write_manifest_at(&dest_dir, &content_manifest) {
            warn!(
                error = %e,
                path = %dest_dir.display(),
                "episode_cache_migrate: content manifest write failed"
            );
        }

        if let Err(e) = write_manifest_at(file_dir, m) {
            warn!(
                error = %e,
                path = %file_dir.display(),
                "episode_cache_migrate: per-file manifest pruning failed"
            );
        }
    }

    Ok(moved_any)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_cache::PIPELINE_VERSION;
    use crate::media_cache::writers::{ArtefactInfo, ArtefactMap, Manifest, write_sidecar};
    use serde_json::Map as JsonMap;
    use tempfile::TempDir;

    fn legacy_manifest() -> Manifest {
        let artefacts = ArtefactMap {
            trickplay: Some(ArtefactInfo {
                present: true,
                params: JsonMap::new(),
            }),
            vhash: Some(ArtefactInfo {
                present: true,
                params: JsonMap::new(),
            }),
            audio_peaks: Some(ArtefactInfo {
                present: true,
                params: JsonMap::new(),
            }),
            ..ArtefactMap::default()
        };
        Manifest {
            pipeline_version: PIPELINE_VERSION,
            generated_at: Utc::now(),
            input_duration_seconds: 100.0,
            trickplay_dir: "/dev/null".into(),
            artefacts,
            analyzed_file_id: None,
            analyzed_at: None,
            input_size_bytes: None,
            input_mtime: None,
        }
    }

    #[test]
    fn migrate_one_moves_content_sidecars_keeps_trickplay() {
        let tmp = TempDir::new().unwrap();
        let storage = tmp.path().to_string_lossy().into_owned();
        let file_id = Uuid::now_v7();

        // Write a legacy media_cache layout: vhash + audio_peaks + manifest.
        let media_cache_dir = crate::media_cache::cache_dir(&storage, file_id);
        std::fs::create_dir_all(&media_cache_dir).unwrap();
        write_sidecar(&storage, file_id, Branch::Vhash, b"VHASH-BYTES").unwrap();
        write_sidecar(&storage, file_id, Branch::AudioPeaks, b"PEAKS-BYTES").unwrap();
        let manifest_path = media_cache_dir.join("manifest.json");
        std::fs::write(
            &manifest_path,
            serde_json::to_vec(&legacy_manifest()).unwrap(),
        )
        .unwrap();

        let target = MediaKind::TvFile { file_id };
        let moved = migrate_one(&storage, &media_cache_dir, file_id, &target).unwrap();
        assert!(moved);

        let dest_dir = episode_cache_dir(&storage, &target);
        // Vhash moved out.
        assert!(dest_dir.join("vhash.bin").is_file());
        assert!(!media_cache_dir.join("vhash.bin").exists());
        // Audio_peaks stays put.
        assert!(media_cache_dir.join("audio_peaks.bin").is_file());
        // Per-file manifest exists, content branches stripped.
        let pruned: Manifest =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        assert!(pruned.artefacts.vhash.is_none());
        assert!(pruned.artefacts.trickplay.is_some());
        assert!(pruned.artefacts.audio_peaks.is_some());
        // Content manifest at destination exists.
        let content: Manifest =
            serde_json::from_slice(&std::fs::read(dest_dir.join("manifest.json")).unwrap())
                .unwrap();
        assert!(content.artefacts.vhash.is_some());
        assert!(content.artefacts.trickplay.is_none());
    }

    #[test]
    fn marker_path_lives_under_episode_cache() {
        let p = marker_path("/data");
        assert_eq!(p, PathBuf::from("/data/episode_cache/.migrated_at"));
    }

    #[test]
    fn write_marker_creates_parent_and_records_timestamp() {
        let tmp = TempDir::new().unwrap();
        let storage = tmp.path().to_string_lossy().into_owned();
        let path = marker_path(&storage);
        // Parent doesn't exist yet — write_marker creates it.
        assert!(!path.parent().unwrap().exists());
        write_marker(&path);
        assert!(path.is_file(), "marker file written");
        let body = std::fs::read_to_string(&path).unwrap();
        // RFC3339 starts with the year.
        assert!(body.starts_with("20"), "rfc3339 timestamp: {body}");
        // Idempotent — second call overwrites without error.
        write_marker(&path);
        assert!(path.is_file());
    }

    #[test]
    fn migrate_one_idempotent_on_rerun() {
        let tmp = TempDir::new().unwrap();
        let storage = tmp.path().to_string_lossy().into_owned();
        let file_id = Uuid::now_v7();
        let media_cache_dir = crate::media_cache::cache_dir(&storage, file_id);
        std::fs::create_dir_all(&media_cache_dir).unwrap();
        write_sidecar(&storage, file_id, Branch::Vhash, b"VHASH").unwrap();
        let manifest_path = media_cache_dir.join("manifest.json");
        std::fs::write(
            &manifest_path,
            serde_json::to_vec(&legacy_manifest()).unwrap(),
        )
        .unwrap();
        let target = MediaKind::TvFile { file_id };
        assert!(migrate_one(&storage, &media_cache_dir, file_id, &target).unwrap());
        // Second run finds nothing to move.
        assert!(!migrate_one(&storage, &media_cache_dir, file_id, &target).unwrap());
    }
}
