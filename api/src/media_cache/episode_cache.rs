//! Persistent content-bound artefact cache.
//!
//! `episode_cache/` holds the heavy detect inputs (vhash, chromaprint,
//! silence, loudness, black_frames, embeddings) at a stable key keyed off the
//! authoritative DB id, so they survive per-file evictions and re-downloads.
//! File-bound stuff (trickplay sprites, audio_peaks, source.tmp) stays in
//! `media_cache/{file_id}/` and dies with the video.
//!
//! Three namespaces, one per source-of-truth table:
//!
//! ```text
//! episode_cache/
//! ├── tv_file/{tv_episode_files.id}/      ← native TV. Multi-ep files share one dir.
//! ├── movie/{movies.id}/                  ← movies (native + debrid converge).
//! └── debrid_episode/{tv_episodes.id}/    ← debrid TV (always single-ep).
//! ```
//!
//! Reading for a TV episode: query `tv_episode_file_episodes` for the
//! episode_id → file_ids; try `tv_file/{file_id}/` for each, fall back to
//! `debrid_episode/{episode_id}/`. No symlinks, no duplication on disk —
//! the junction does the multi-ep deduplication transparently.
//!
//! Replacement policy is `pipeline_version`-gated: a write proceeds iff the
//! incoming manifest's version is `>=` the existing one. Older-version
//! writes are dropped silently so a regression can't overwrite a richer
//! prior cache.

use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::writers::{Manifest, write_atomic, write_manifest_at};
use super::{Branch, sidecar_filename};

/// Source-of-truth-keyed namespace for a persisted artefact set. Each variant
/// names the DB table whose `id` keys the directory; see module docs for the
/// on-disk hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaKind {
    /// `tv_episode_files.id`. One physical file may map to N episodes via
    /// `tv_episode_file_episodes` — they all share this cache.
    TvFile { file_id: Uuid },
    /// `movies.id`. Native and debrid movie analyses converge here.
    Movie { movie_id: Uuid },
    /// `tv_episodes.id`. Used when there's no local file row to anchor to —
    /// i.e. debrid streams of episodes the user doesn't own locally. Always
    /// keyed by a single episode (debrid streams target one episode).
    DebridEpisode { episode_id: Uuid },
}

impl MediaKind {
    fn dir_name(&self) -> &'static str {
        match self {
            MediaKind::TvFile { .. } => "tv_file",
            MediaKind::Movie { .. } => "movie",
            MediaKind::DebridEpisode { .. } => "debrid_episode",
        }
    }

    pub fn id(&self) -> Uuid {
        match self {
            MediaKind::TvFile { file_id } => *file_id,
            MediaKind::Movie { movie_id } => *movie_id,
            MediaKind::DebridEpisode { episode_id } => *episode_id,
        }
    }

    /// Resolve a debrid session's `(media_type, media_id)` into the right
    /// namespace. Returns `None` for media types that don't get an
    /// episode_cache copy (audiobook, recording, …).
    pub fn for_debrid(media_type: &str, media_id: Uuid) -> Option<Self> {
        match media_type {
            "tv_episode" | "episode" => Some(MediaKind::DebridEpisode {
                episode_id: media_id,
            }),
            "movie" => Some(MediaKind::Movie { movie_id: media_id }),
            _ => None,
        }
    }
}

/// Resolve the directory holding `kind`'s artefacts.
pub fn episode_cache_dir(storage_root: &str, kind: &MediaKind) -> PathBuf {
    PathBuf::from(storage_root)
        .join("episode_cache")
        .join(kind.dir_name())
        .join(kind.id().to_string())
}

/// Path to the manifest for `kind`.
pub fn manifest_path(storage_root: &str, kind: &MediaKind) -> PathBuf {
    episode_cache_dir(storage_root, kind).join("manifest.json")
}

/// Read and parse the persisted episode_cache manifest. Returns `None` if
/// missing, unreadable, or malformed.
pub fn read_manifest(storage_root: &str, kind: &MediaKind) -> Option<Manifest> {
    let bytes = fs::read(manifest_path(storage_root, kind)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Outcome of [`write_episode_cache`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    /// Wrote a fresh artefact set — no prior manifest existed.
    Wrote,
    /// Replaced an existing artefact set whose `pipeline_version` is `<=`
    /// the new one. Same-version writes also count as Replaced (last-good
    /// wins).
    Replaced,
    /// Existing manifest's `pipeline_version` is strictly greater; new
    /// write dropped silently and the existing artefacts remain untouched.
    Skipped,
}

/// Wire-format bytes for each branch the episode_cache stores. trickplay +
/// audio_peaks are intentionally absent — they live in the per-file
/// `media_cache/{file_id}/` and don't belong here. A `None` slot means
/// "branch was not produced" — its sidecar is not written.
pub struct EpisodeArtefacts<'a> {
    pub vhash: Option<&'a [u8]>,
    pub chromaprint: Option<&'a [u8]>,
    pub silence: Option<&'a [u8]>,
    pub loudness: Option<&'a [u8]>,
    pub black_frames: Option<&'a [u8]>,
    pub embeddings: Option<&'a [u8]>,
}

/// Write `manifest` + each `Some(_)` artefact into the namespace dir for
/// `kind`. Returns what happened relative to any existing copy.
pub fn write_episode_cache(
    storage_root: &str,
    kind: &MediaKind,
    manifest: &Manifest,
    artefacts: &EpisodeArtefacts<'_>,
) -> std::io::Result<WriteOutcome> {
    let dir = episode_cache_dir(storage_root, kind);
    let outcome = match read_manifest(storage_root, kind) {
        Some(existing) if existing.pipeline_version > manifest.pipeline_version => {
            return Ok(WriteOutcome::Skipped);
        }
        Some(_) => WriteOutcome::Replaced,
        None => WriteOutcome::Wrote,
    };

    fs::create_dir_all(&dir)?;
    write_artefact(&dir, Branch::Vhash, artefacts.vhash)?;
    write_artefact(&dir, Branch::Chromaprint, artefacts.chromaprint)?;
    write_artefact(&dir, Branch::Silence, artefacts.silence)?;
    write_artefact(&dir, Branch::Loudness, artefacts.loudness)?;
    write_artefact(&dir, Branch::BlackFrames, artefacts.black_frames)?;
    write_artefact(&dir, Branch::Embeddings, artefacts.embeddings)?;
    write_manifest_at(&dir, manifest)?;
    Ok(outcome)
}

/// Remove the entire cache directory for `kind`. Called from repository
/// delete paths (`tv_episode_files`, `tv_episodes`, `movies`) so disk drops
/// with the row. Best-effort — a missing directory is not an error.
pub fn remove(storage_root: &str, kind: &MediaKind) -> std::io::Result<()> {
    let dir = episode_cache_dir(storage_root, kind);
    if dir.is_dir() {
        fs::remove_dir_all(dir)?;
    }
    Ok(())
}

fn write_artefact(dir: &Path, branch: Branch, bytes: Option<&[u8]>) -> std::io::Result<()> {
    let Some(bytes) = bytes else {
        return Ok(());
    };
    let Some(name) = sidecar_filename(branch) else {
        return Ok(());
    };
    write_atomic(&dir.join(name), bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_cache::PIPELINE_VERSION;
    use crate::media_cache::writers::ArtefactMap;
    use chrono::Utc;
    use tempfile::TempDir;

    fn fresh_manifest(version: i32, file_id: Uuid) -> Manifest {
        Manifest {
            pipeline_version: version,
            generated_at: Utc::now(),
            input_duration_seconds: 1234.5,
            trickplay_dir: "/dev/null".into(),
            artefacts: ArtefactMap::default(),
            analyzed_file_id: Some(file_id),
            analyzed_at: Some(Utc::now()),
            input_size_bytes: None,
            input_mtime: None,
        }
    }

    fn arts_a() -> Vec<u8> {
        b"vhash-AAAA".to_vec()
    }
    fn arts_b() -> Vec<u8> {
        b"vhash-BBBB".to_vec()
    }
    fn empty() -> EpisodeArtefacts<'static> {
        EpisodeArtefacts {
            vhash: None,
            chromaprint: None,
            silence: None,
            loudness: None,
            black_frames: None,
            embeddings: None,
        }
    }

    #[test]
    fn three_namespaces_have_disjoint_directories() {
        let id = Uuid::now_v7();
        assert_eq!(
            episode_cache_dir("/x", &MediaKind::TvFile { file_id: id }),
            PathBuf::from(format!("/x/episode_cache/tv_file/{id}"))
        );
        assert_eq!(
            episode_cache_dir("/x", &MediaKind::Movie { movie_id: id }),
            PathBuf::from(format!("/x/episode_cache/movie/{id}"))
        );
        assert_eq!(
            episode_cache_dir("/x", &MediaKind::DebridEpisode { episode_id: id }),
            PathBuf::from(format!("/x/episode_cache/debrid_episode/{id}"))
        );
    }

    #[test]
    fn writes_when_no_existing_manifest() {
        let tmp = TempDir::new().unwrap();
        let storage = tmp.path().to_string_lossy().into_owned();
        let kind = MediaKind::TvFile {
            file_id: Uuid::now_v7(),
        };
        let manifest = fresh_manifest(PIPELINE_VERSION, Uuid::now_v7());
        let v = arts_a();
        let arts = EpisodeArtefacts {
            vhash: Some(&v),
            ..empty()
        };
        let outcome = write_episode_cache(&storage, &kind, &manifest, &arts).unwrap();
        assert_eq!(outcome, WriteOutcome::Wrote);
        let dir = episode_cache_dir(&storage, &kind);
        assert!(dir.join("manifest.json").is_file());
        assert_eq!(std::fs::read(dir.join("vhash.bin")).unwrap(), v);
    }

    #[test]
    fn overwrites_when_new_version_geq_existing() {
        let tmp = TempDir::new().unwrap();
        let storage = tmp.path().to_string_lossy().into_owned();
        let kind = MediaKind::TvFile {
            file_id: Uuid::now_v7(),
        };
        let manifest_v2 = fresh_manifest(2, Uuid::now_v7());
        let arts_old = arts_a();
        let arts_new = arts_b();
        write_episode_cache(
            &storage,
            &kind,
            &manifest_v2,
            &EpisodeArtefacts {
                vhash: Some(&arts_old),
                ..empty()
            },
        )
        .unwrap();
        let outcome = write_episode_cache(
            &storage,
            &kind,
            &manifest_v2,
            &EpisodeArtefacts {
                vhash: Some(&arts_new),
                ..empty()
            },
        )
        .unwrap();
        assert_eq!(outcome, WriteOutcome::Replaced);
        let dir = episode_cache_dir(&storage, &kind);
        assert_eq!(std::fs::read(dir.join("vhash.bin")).unwrap(), arts_new);
    }

    #[test]
    fn skips_when_new_version_lt_existing() {
        let tmp = TempDir::new().unwrap();
        let storage = tmp.path().to_string_lossy().into_owned();
        let kind = MediaKind::Movie {
            movie_id: Uuid::now_v7(),
        };
        let manifest_v3 = fresh_manifest(3, Uuid::now_v7());
        let manifest_v2 = fresh_manifest(2, Uuid::now_v7());
        let arts_old = arts_a();
        let arts_new = arts_b();
        write_episode_cache(
            &storage,
            &kind,
            &manifest_v3,
            &EpisodeArtefacts {
                vhash: Some(&arts_old),
                ..empty()
            },
        )
        .unwrap();
        let outcome = write_episode_cache(
            &storage,
            &kind,
            &manifest_v2,
            &EpisodeArtefacts {
                vhash: Some(&arts_new),
                ..empty()
            },
        )
        .unwrap();
        assert_eq!(outcome, WriteOutcome::Skipped);
        let dir = episode_cache_dir(&storage, &kind);
        assert_eq!(std::fs::read(dir.join("vhash.bin")).unwrap(), arts_old);
    }

    #[test]
    fn malformed_existing_manifest_treated_as_missing() {
        let tmp = TempDir::new().unwrap();
        let storage = tmp.path().to_string_lossy().into_owned();
        let kind = MediaKind::DebridEpisode {
            episode_id: Uuid::now_v7(),
        };
        let dir = episode_cache_dir(&storage, &kind);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), b"not json").unwrap();
        let v = arts_a();
        let outcome = write_episode_cache(
            &storage,
            &kind,
            &fresh_manifest(PIPELINE_VERSION, Uuid::now_v7()),
            &EpisodeArtefacts {
                vhash: Some(&v),
                ..empty()
            },
        )
        .unwrap();
        assert_eq!(outcome, WriteOutcome::Wrote);
        assert!(read_manifest(&storage, &kind).is_some());
    }

    #[test]
    fn for_debrid_routes_media_types() {
        let id = Uuid::now_v7();
        assert_eq!(
            MediaKind::for_debrid("tv_episode", id),
            Some(MediaKind::DebridEpisode { episode_id: id })
        );
        assert_eq!(
            MediaKind::for_debrid("movie", id),
            Some(MediaKind::Movie { movie_id: id })
        );
        assert_eq!(MediaKind::for_debrid("audiobook", id), None);
    }

    #[test]
    fn remove_drops_the_dir() {
        let tmp = TempDir::new().unwrap();
        let storage = tmp.path().to_string_lossy().into_owned();
        let kind = MediaKind::TvFile {
            file_id: Uuid::now_v7(),
        };
        let v = arts_a();
        write_episode_cache(
            &storage,
            &kind,
            &fresh_manifest(PIPELINE_VERSION, Uuid::now_v7()),
            &EpisodeArtefacts {
                vhash: Some(&v),
                ..empty()
            },
        )
        .unwrap();
        assert!(episode_cache_dir(&storage, &kind).is_dir());
        remove(&storage, &kind).unwrap();
        assert!(!episode_cache_dir(&storage, &kind).exists());
        // Idempotent — second remove is a no-op.
        remove(&storage, &kind).unwrap();
    }
}
