//! Typed loaders for cached sidecars + manifest.
//!
//! Branch readers return `Option<T>` so a caller can probe coverage without
//! propagating "not found" errors. Use `read_manifest` for the authoritative
//! per-file picture (params + presence flags); use the typed loaders to
//! actually consume bytes.

use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::{Branch, cache_dir, manifest_path, sidecar_filename, sidecar_path};
use crate::skip_detect::primitives::vhash_align::FrameDescriptor;

pub use super::writers::{ArtefactInfo, ArtefactMap, BlackGap, Loudness, Manifest, SilenceGap};

/// Read and parse a manifest living directly at `<cache_dir>/manifest.json`.
/// Used by path-based callers (debrid, iptv) that don't have a kobbler file_id.
pub fn read_manifest_at(cache_dir: &Path) -> Option<Manifest> {
    let bytes = std::fs::read(cache_dir.join("manifest.json")).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Read and parse the manifest for a file. Returns `None` if the manifest
/// doesn't exist (file hasn't been analyzed yet) or is unreadable.
pub fn read_manifest(storage_root: &str, file_id: Uuid) -> Option<Manifest> {
    let path = manifest_path(storage_root, file_id);
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Returns the artefact info for a single branch from the manifest, if both
/// the manifest exists and that branch is recorded as present.
pub fn artefact_info_at(cache_dir: &Path, branch: Branch) -> Option<ArtefactInfo> {
    let m = read_manifest_at(cache_dir)?;
    let info = match branch {
        Branch::Trickplay => m.artefacts.trickplay,
        Branch::Vhash => m.artefacts.vhash,
        Branch::Chromaprint => m.artefacts.chromaprint,
        Branch::Silence => m.artefacts.silence,
        Branch::Loudness => m.artefacts.loudness,
        Branch::AudioPeaks => m.artefacts.audio_peaks,
        Branch::Embeddings => m.artefacts.embeddings,
        Branch::BlackFrames => m.artefacts.black_frames,
    }?;
    if info.present { Some(info) } else { None }
}

/// Returns the artefact info for a single branch from the manifest, if both
/// the manifest exists and that branch is recorded as present.
pub fn artefact_info(storage_root: &str, file_id: Uuid, branch: Branch) -> Option<ArtefactInfo> {
    artefact_info_at(&cache_dir(storage_root, file_id), branch)
}

fn sidecar_path_at(cache_dir: &Path, branch: Branch) -> Option<PathBuf> {
    sidecar_filename(branch).map(|name| cache_dir.join(name))
}

/// Path-based: read a chromaprint sidecar from `<cache_dir>/chromaprint.bin`.
pub fn read_chromaprint_at(cache_dir: &Path) -> Option<Vec<u32>> {
    let path = sidecar_path_at(cache_dir, Branch::Chromaprint)?;
    read_u32_le_vec(&path)
}

/// Read a chromaprint sidecar as raw `Vec<u32>` little-endian.
/// Wire format mirrors `kob_ffmpeg::chromaprint::Fingerprint::raw`.
pub fn read_chromaprint(storage_root: &str, file_id: Uuid) -> Option<Vec<u32>> {
    let path = sidecar_path(storage_root, file_id, Branch::Chromaprint);
    read_u32_le_vec(&path)
}

/// Path-based: read silence intervals from `<cache_dir>/silence.json`.
pub fn read_silence_at(cache_dir: &Path) -> Option<Vec<SilenceGap>> {
    let path = sidecar_path_at(cache_dir, Branch::Silence)?;
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Read silence intervals from `silence.json`.
pub fn read_silence(storage_root: &str, file_id: Uuid) -> Option<Vec<SilenceGap>> {
    let path = sidecar_path(storage_root, file_id, Branch::Silence);
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Path-based: read black-frame intervals from `<cache_dir>/black_frames.json`.
pub fn read_black_frames_at(cache_dir: &Path) -> Option<Vec<BlackGap>> {
    let path = sidecar_path_at(cache_dir, Branch::BlackFrames)?;
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Read black-frame intervals from `black_frames.json`.
pub fn read_black_frames(storage_root: &str, file_id: Uuid) -> Option<Vec<BlackGap>> {
    let path = sidecar_path(storage_root, file_id, Branch::BlackFrames);
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Read integrated LUFS from `loudness.json`.
pub fn read_loudness(storage_root: &str, file_id: Uuid) -> Option<f64> {
    #[derive(serde::Deserialize)]
    struct LoudnessFile {
        integrated_lufs: Option<f64>,
    }
    let path = sidecar_path(storage_root, file_id, Branch::Loudness);
    let bytes = std::fs::read(path).ok()?;
    let parsed: LoudnessFile = serde_json::from_slice(&bytes).ok()?;
    parsed.integrated_lufs
}

/// Path-based: integrated LUFS from `<cache_dir>/loudness.json`.
pub fn read_loudness_at(cache_dir: &Path) -> Option<f64> {
    #[derive(serde::Deserialize)]
    struct LoudnessFile {
        integrated_lufs: Option<f64>,
    }
    let bytes = std::fs::read(sidecar_path_at(cache_dir, Branch::Loudness)?).ok()?;
    serde_json::from_slice::<LoudnessFile>(&bytes)
        .ok()?
        .integrated_lufs
}

/// Read raw audio peaks bytes (already in wire format).
pub fn read_audio_peaks(storage_root: &str, file_id: Uuid) -> Option<Vec<u8>> {
    let path = sidecar_path(storage_root, file_id, Branch::AudioPeaks);
    std::fs::read(path).ok()
}

/// Path-based: read vhash descriptors from `<cache_dir>/vhash.bin`.
pub fn read_vhash_at(cache_dir: &Path) -> Option<Vec<FrameDescriptor>> {
    let path = sidecar_path_at(cache_dir, Branch::Vhash)?;
    let bytes = std::fs::read(path).ok()?;
    crate::skip_detect::vhash::deserialize_descriptors(&bytes)
}

/// Read vhash descriptors from `vhash.bin`. Wire format mirrors
/// `kobbler::skip_detect::vhash::serialize_descriptors`:
/// `[descriptor_size: u16 LE, frame_count: u32 LE, ((timestamp: f64 LE) (data: descriptor_size bytes))*]`.
pub fn read_vhash(storage_root: &str, file_id: Uuid) -> Option<Vec<FrameDescriptor>> {
    let path = sidecar_path(storage_root, file_id, Branch::Vhash);
    let bytes = std::fs::read(path).ok()?;
    crate::skip_detect::vhash::deserialize_descriptors(&bytes)
}

/// Path-based: read the embeddings sidecar from `<cache_dir>/embeddings.bin`.
pub fn read_embeddings_at(cache_dir: &Path) -> Option<EmbeddingsView> {
    let info = artefact_info_at(cache_dir, Branch::Embeddings)?;
    decode_embeddings(cache_dir, &info)
}

/// Read the embeddings sidecar as a flat `Vec<f32>` interpreted as
/// `count` rows of `dim` columns. Returns `(rows, dim, fps)` from the manifest.
/// Caller is responsible for slicing into per-frame rows.
pub fn read_embeddings(storage_root: &str, file_id: Uuid) -> Option<EmbeddingsView> {
    let info = artefact_info(storage_root, file_id, Branch::Embeddings)?;
    decode_embeddings(&cache_dir(storage_root, file_id), &info)
}

/// The model an embeddings artefact was computed with (recorded on failed
/// attempts too). Caches from before the model was recorded have none.
pub fn embeddings_model(info: &ArtefactInfo) -> Option<&str> {
    info.params.get("model").and_then(|v| v.as_str())
}

/// Present and from [`EMBED_MODEL`](crate::embedding::EMBED_MODEL). Vectors from
/// any other model live in a different space: comparing them with today's prompt
/// vectors gives meaningless scores, so they read as missing.
pub fn embeddings_are_current(info: &ArtefactInfo) -> bool {
    info.present && embeddings_model(info) == Some(crate::embedding::EMBED_MODEL)
}

fn decode_embeddings(dir: &Path, info: &ArtefactInfo) -> Option<EmbeddingsView> {
    if !embeddings_are_current(info) {
        return None;
    }
    let dim = info.params.get("dim")?.as_u64()? as usize;
    let count = info.params.get("count")?.as_u64()? as usize;
    let fps = info.params.get("fps")?.as_f64()?;
    // Optional — pre-windowing caches didn't set this. Default 0.0 means
    // "row 0 is at t=0", which is correct for the old whole-file layout.
    let start_time_secs = info
        .params
        .get("start_time_secs")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let path = dir.join(sidecar_filename(Branch::Embeddings)?);
    let bytes = std::fs::read(path).ok()?;
    // fp16 → 2 bytes per element.
    if bytes.len() < count * dim * 2 {
        return None;
    }
    let mut floats: Vec<f32> = Vec::with_capacity(count * dim);
    for chunk in bytes.as_chunks::<2>().0.iter().take(count * dim) {
        floats.push(f16_to_f32(u16::from_le_bytes(*chunk)));
    }
    Some(EmbeddingsView {
        rows: floats,
        count,
        dim,
        fps,
        start_time_secs,
    })
}

/// View into a flat embeddings buffer. Rows are stored contiguously; row `i`
/// corresponds to timestamp `start_time_secs + i / fps`. Existing whole-file
/// caches set `start_time_secs = 0.0`; windowed caches (from the V1-equivalent
/// credit-tail extraction) carry the window's start time so `window()` indexes
/// into the buffer correctly.
pub struct EmbeddingsView {
    pub rows: Vec<f32>,
    pub count: usize,
    pub dim: usize,
    pub fps: f64,
    pub start_time_secs: f64,
}

impl EmbeddingsView {
    /// Slice the embeddings to the rows whose timestamps fall in [t_start, t_end).
    /// Timestamps are `start_time_secs + row_idx / fps`.
    pub fn window(&self, t_start: f64, t_end: f64) -> Vec<Vec<f32>> {
        if self.fps <= 0.0 || self.count == 0 {
            return Vec::new();
        }
        let lo = (((t_start - self.start_time_secs) * self.fps).floor() as isize).max(0) as usize;
        let hi_raw = ((t_end - self.start_time_secs) * self.fps).ceil() as isize;
        let hi = (hi_raw.max(0) as usize).min(self.count);
        if lo >= hi {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(hi - lo);
        for i in lo..hi {
            let start = i * self.dim;
            let end = start + self.dim;
            out.push(self.rows[start..end].to_vec());
        }
        out
    }
}

// ── Internal helpers ────────────────────────────────────────────────────────

fn read_u32_le_vec(path: &Path) -> Option<Vec<u32>> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() % 4 != 0 {
        return None;
    }
    Some(
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect(),
    )
}

/// IEEE-754 fp16 → fp32 conversion. Branchless except for special-case sign/exp combos.
/// Mirrors the conversion the vision service uses when writing fp16 embeddings.
fn f16_to_f32(half: u16) -> f32 {
    let sign = ((half >> 15) & 0x1) as u32;
    let exp = ((half >> 10) & 0x1f) as u32;
    let mant = (half & 0x3ff) as u32;

    let f_bits = if exp == 0 {
        if mant == 0 {
            sign << 31
        } else {
            // Subnormal — normalise.
            let mut m = mant;
            let mut e = 1u32;
            while (m & 0x400) == 0 {
                m <<= 1;
                e = e.wrapping_sub(1);
            }
            let m = m & 0x3ff;
            (sign << 31) | ((127u32.wrapping_add(e).wrapping_sub(15)) << 23) | (m << 13)
        }
    } else if exp == 0x1f {
        // Inf / NaN.
        (sign << 31) | (0xff << 23) | (mant << 13)
    } else {
        (sign << 31) | ((exp + (127 - 15)) << 23) | (mant << 13)
    };
    f32::from_bits(f_bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_to_f32_known_values() {
        // 0.0
        assert_eq!(f16_to_f32(0x0000), 0.0);
        // -0.0
        assert_eq!(f16_to_f32(0x8000).to_bits(), (-0.0_f32).to_bits());
        // 1.0
        assert!((f16_to_f32(0x3c00) - 1.0).abs() < 1e-6);
        // -1.0
        assert!((f16_to_f32(0xbc00) + 1.0).abs() < 1e-6);
        // 0.5
        assert!((f16_to_f32(0x3800) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn embeddings_window_slices_correctly() {
        // 4 rows of 2 dims at 1 fps → t = [0, 1, 2, 3]
        let view = EmbeddingsView {
            rows: vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0],
            count: 4,
            dim: 2,
            fps: 1.0,
            start_time_secs: 0.0,
        };
        let win = view.window(1.0, 3.0);
        // Should include rows 1, 2 (t=1.0 → t=3.0, exclusive of last)
        assert_eq!(win.len(), 2);
        assert_eq!(win[0], vec![3.0, 4.0]);
        assert_eq!(win[1], vec![5.0, 6.0]);
    }

    #[test]
    fn empty_window_when_out_of_range() {
        let view = EmbeddingsView {
            rows: vec![1.0, 2.0],
            count: 1,
            dim: 2,
            fps: 1.0,
            start_time_secs: 0.0,
        };
        let win = view.window(10.0, 20.0);
        assert!(win.is_empty());
    }

    #[test]
    fn embeddings_window_respects_nonzero_start_offset() {
        // Windowed extraction: 4 rows at 1 fps starting at t=3300
        // → timestamps [3300, 3301, 3302, 3303].
        let view = EmbeddingsView {
            rows: vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0],
            count: 4,
            dim: 2,
            fps: 1.0,
            start_time_secs: 3300.0,
        };
        // Ask for [3301, 3303) — expect rows 1, 2 (t=3301, t=3302).
        let win = view.window(3301.0, 3303.0);
        assert_eq!(win.len(), 2);
        assert_eq!(win[0], vec![3.0, 4.0]);
        assert_eq!(win[1], vec![5.0, 6.0]);

        // Ask for [0, 3300) — entirely before the window → empty.
        let win = view.window(0.0, 3300.0);
        assert!(win.is_empty(), "pre-window query should be empty");

        // Ask for the whole window — should return all 4 rows.
        let win = view.window(3300.0, 3304.0);
        assert_eq!(win.len(), 4);
    }

    #[test]
    fn embeddings_window_before_offset_returns_empty() {
        // Regression guard for the original bug: with start_time_secs > 0,
        // asking for timestamps earlier than the window must not return
        // the first rows of the buffer (which is what the old math did).
        let view = EmbeddingsView {
            rows: vec![1.0, 2.0, 3.0, 4.0],
            count: 2,
            dim: 2,
            fps: 1.0,
            start_time_secs: 100.0,
        };
        let win = view.window(0.0, 50.0);
        assert!(win.is_empty());
    }

    /// A model switch must read as "no embeddings", never as vectors from a
    /// different space scored against today's prompts.
    #[test]
    fn embeddings_from_another_model_read_as_missing() {
        use crate::media_cache::writers::{new_manifest_at, write_manifest_at, write_sidecar_at};

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write_sidecar_at(dir, Branch::Embeddings, &[0u8; 2 * 2 * 3]).unwrap();
        let write = |model: &str| {
            let mut m = new_manifest_at(dir, 60.0);
            let mut params = serde_json::Map::new();
            for (k, v) in [
                ("dim", serde_json::json!(3)),
                ("count", serde_json::json!(2)),
                ("fps", serde_json::json!(1.0)),
                ("model", serde_json::json!(model)),
            ] {
                params.insert(k.into(), v);
            }
            m.artefacts.set(
                Branch::Embeddings,
                ArtefactInfo {
                    present: true,
                    params,
                },
            );
            write_manifest_at(dir, &m).unwrap();
        };

        write("google/siglip2-so400m-patch16-naflex");
        assert!(read_embeddings_at(dir).is_none());

        write(crate::embedding::EMBED_MODEL);
        let view = read_embeddings_at(dir).expect("current model reads");
        assert_eq!((view.count, view.dim), (2, 3));
    }
}
