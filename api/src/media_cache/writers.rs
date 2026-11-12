//! Atomic sidecar + manifest writers.
//!
//! Every write goes via `write_atomic`: write to a sibling `.tmp`, fsync, then
//! rename in one shot. A crashed analyze never leaves a partially-written
//! sidecar visible to consumers.

use std::fs;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    Branch, PIPELINE_VERSION, cache_dir, manifest_path, sidecar_filename, sidecar_path,
    trickplay_dir,
};

/// One silence interval as written to `silence.json`.
/// Mirrors `kob_ffmpeg::filter::detect::SilenceInterval` but serializable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SilenceGap {
    pub start_secs: f64,
    pub end_secs: f64,
    pub duration_secs: f64,
}

/// One black-frame interval as written to `black_frames.json`.
/// Mirrors `kob_ffmpeg::filter::detect::BlackInterval` but serializable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlackGap {
    pub start_secs: f64,
    pub end_secs: f64,
    pub duration_secs: f64,
}

/// Loudness sidecar shape (`loudness.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Loudness {
    pub integrated_lufs: Option<f64>,
    #[serde(default)]
    pub downmix_peak_db: Option<f64>,
}

/// One artefact's coverage info as it appears in `manifest.json`.
/// Fields are JSON values so each branch can carry whatever params it needs
/// without forcing a fat union enum here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtefactInfo {
    pub present: bool,
    /// Branch-specific params (e.g., `{"width": 320, "fps": 0.2}`). Free-form so
    /// the manifest survives schema additions without breaking older readers.
    #[serde(default, flatten)]
    pub params: serde_json::Map<String, serde_json::Value>,
}

impl ArtefactInfo {
    pub fn missing() -> Self {
        Self {
            present: false,
            params: Default::default(),
        }
    }
}

/// On-disk manifest. Bump `pipeline_version` when the format or branch
/// semantics change; the backfill walker uses it to find files to regenerate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub pipeline_version: i32,
    pub generated_at: chrono::DateTime<chrono::Utc>,
    pub input_duration_seconds: f64,
    /// Absolute path to the trickplay sprites directory.
    pub trickplay_dir: String,
    pub artefacts: ArtefactMap,
    /// Provenance for episode_cache copies: which file_id produced this set of
    /// artefacts. `None` for the per-file `media_cache/{file_id}/` layout.
    /// Carried by `media_cache::episode_cache::write_episode_cache` so a
    /// future cross-episode reader can identify the source stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analyzed_file_id: Option<uuid::Uuid>,
    /// When the persisted cache was written. `None` for the per-file layout
    /// where `generated_at` is already authoritative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analyzed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Size and mtime of the source media file as it was when this cache was
    /// produced, so a later check can tell whether the cache still describes
    /// the file on disk.
    ///
    /// Without it there is no way to notice that a file changed after being
    /// analyzed. A download that reports complete, gets ingested and analyzed,
    /// and is then re-created by a retry that never finishes leaves a full-size
    /// file of mostly zeros behind a cache and metadata describing the good
    /// copy — and nothing looks at it again. Observed on three files that sat
    /// undetected for two months.
    ///
    /// `None` on caches written before these fields existed; a check that finds
    /// `None` should treat the file as unverifiable rather than as changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_size_bytes: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_mtime: Option<chrono::DateTime<chrono::Utc>>,
}

/// Size + mtime of a file, the pair used to decide whether it still matches a
/// cache built from it. Cheap: one `stat`, no reads.
pub fn source_stamp(path: &Path) -> Option<(i64, chrono::DateTime<chrono::Utc>)> {
    let md = std::fs::metadata(path).ok()?;
    let mtime = md.modified().ok()?;
    Some((
        md.len() as i64,
        chrono::DateTime::<chrono::Utc>::from(mtime),
    ))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArtefactMap {
    #[serde(default)]
    pub trickplay: Option<ArtefactInfo>,
    #[serde(default)]
    pub vhash: Option<ArtefactInfo>,
    #[serde(default)]
    pub chromaprint: Option<ArtefactInfo>,
    #[serde(default)]
    pub silence: Option<ArtefactInfo>,
    #[serde(default)]
    pub loudness: Option<ArtefactInfo>,
    #[serde(default)]
    pub audio_peaks: Option<ArtefactInfo>,
    #[serde(default)]
    pub embeddings: Option<ArtefactInfo>,
    #[serde(default)]
    pub black_frames: Option<ArtefactInfo>,
}

impl ArtefactMap {
    pub fn set(&mut self, branch: Branch, info: ArtefactInfo) {
        match branch {
            Branch::Trickplay => self.trickplay = Some(info),
            Branch::Vhash => self.vhash = Some(info),
            Branch::Chromaprint => self.chromaprint = Some(info),
            Branch::Silence => self.silence = Some(info),
            Branch::Loudness => self.loudness = Some(info),
            Branch::AudioPeaks => self.audio_peaks = Some(info),
            Branch::Embeddings => self.embeddings = Some(info),
            Branch::BlackFrames => self.black_frames = Some(info),
        }
    }
}

/// Write `bytes` to `path` atomically (sibling .tmp + rename). Creates parent
/// dirs as needed. Errors propagate as io::Error.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(match path.extension() {
        Some(ext) => format!("{}.tmp", ext.to_string_lossy()),
        None => "tmp".to_string(),
    });
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Write a sidecar into an arbitrary cache directory. Fails if called with
/// `Branch::Trickplay` (trickplay is a directory of sprite JPEGs, not a file).
pub fn write_sidecar_at(cache_dir: &Path, branch: Branch, bytes: &[u8]) -> std::io::Result<()> {
    let name = sidecar_filename(branch).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "trickplay is a directory — write sprite JPEGs directly",
        )
    })?;
    write_atomic(&cache_dir.join(name), bytes)
}

/// Write the manifest into an arbitrary cache directory.
pub fn write_manifest_at(cache_dir: &Path, manifest: &Manifest) -> std::io::Result<()> {
    fs::create_dir_all(cache_dir)?;
    let bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    write_atomic(&cache_dir.join("manifest.json"), &bytes)
}

/// Fresh manifest stub with an explicit trickplay_dir. Caller fills in
/// `artefacts.set(...)` per branch and then calls `write_manifest_at`.
pub fn new_manifest_at(trickplay_dir: &Path, input_duration_seconds: f64) -> Manifest {
    Manifest {
        pipeline_version: PIPELINE_VERSION,
        generated_at: chrono::Utc::now(),
        input_duration_seconds,
        trickplay_dir: trickplay_dir.to_string_lossy().into_owned(),
        artefacts: ArtefactMap::default(),
        analyzed_file_id: None,
        analyzed_at: None,
        input_size_bytes: None,
        input_mtime: None,
    }
}

/// Convenience: write a sidecar by (storage_root, file_id, branch).
pub fn write_sidecar(
    storage_root: &str,
    file_id: Uuid,
    branch: Branch,
    bytes: &[u8],
) -> std::io::Result<()> {
    if branch == Branch::Trickplay {
        return write_atomic(&sidecar_path(storage_root, file_id, branch), bytes);
    }
    write_sidecar_at(&cache_dir(storage_root, file_id), branch, bytes)
}

/// Write the manifest atomically for (storage_root, file_id).
pub fn write_manifest(
    storage_root: &str,
    file_id: Uuid,
    manifest: &Manifest,
) -> std::io::Result<()> {
    fs::create_dir_all(cache_dir(storage_root, file_id))?;
    let path = manifest_path(storage_root, file_id);
    let bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    write_atomic(&path, &bytes)
}

/// Build a fresh manifest stub with pipeline_version + trickplay_dir derived
/// from (storage_root, file_id). Native-kobbler flow only.
pub fn new_manifest(storage_root: &str, file_id: Uuid, input_duration_seconds: f64) -> Manifest {
    new_manifest_at(
        &trickplay_dir(storage_root, file_id),
        input_duration_seconds,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_creates_parents_and_renames() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("subdir/sub2/file.bin");
        write_atomic(&target, b"hello").expect("write");
        assert!(target.is_file());
        assert_eq!(std::fs::read(&target).unwrap(), b"hello");
        // .tmp sibling should be gone
        let sib = target.with_extension("bin.tmp");
        assert!(!sib.exists());
    }

    #[test]
    fn manifest_roundtrip() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let storage = tmp.path().to_string_lossy().into_owned();
        let id = Uuid::now_v7();
        let mut m = new_manifest(&storage, id, 100.0);
        let mut params = serde_json::Map::new();
        params.insert("width".into(), 320.into());
        params.insert("fps".into(), 0.2.into());
        m.artefacts.set(
            Branch::Trickplay,
            ArtefactInfo {
                present: true,
                params,
            },
        );
        write_manifest(&storage, id, &m).expect("write");
        let back: Manifest =
            serde_json::from_slice(&std::fs::read(manifest_path(&storage, id)).expect("read"))
                .expect("parse");
        assert_eq!(back.pipeline_version, PIPELINE_VERSION);
        assert!(back.artefacts.trickplay.as_ref().unwrap().present);
        assert_eq!(
            back.artefacts.trickplay.as_ref().unwrap().params["width"],
            320
        );
    }
}
