//! Shared embedding packing + fp16 conversion for writing the `embeddings`
//! sidecar. The vision service returns f32 vectors; the on-disk sidecar format
//! is fp16 (little-endian) to halve the footprint. Readers live in
//! `media_cache::readers::read_embeddings` and do the reverse conversion.

use std::path::Path;

use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::embedding::{EMBED_DIM, EMBED_MODEL, EmbeddingClient, IMAGE_TOKENS};
use crate::gpu::GpuLock;
use crate::media_cache::Branch;
use crate::media_cache::analyze::{EMBED_FPS, EMBED_WIDTH};
use crate::media_cache::writers::{ArtefactInfo, Manifest, write_atomic};
use crate::runtime_config::RuntimeConfig;

/// IEEE-754 fp32 → fp16 conversion (round-to-nearest-even via truncation).
/// Mirror of `media_cache::readers::f16_to_f32`.
pub fn f32_to_f16(f: f32) -> u16 {
    let bits = f.to_bits();
    let sign = ((bits >> 31) & 0x1) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x7fffff;
    if exp == 0xff {
        return (sign << 15) | 0x7c00 | (mant >> 13) as u16;
    }
    let new_exp = exp - 127 + 15;
    if new_exp >= 0x1f {
        return (sign << 15) | 0x7c00;
    }
    if new_exp <= 0 {
        if new_exp < -10 {
            return sign << 15;
        }
        let mant = (mant | 0x800000) >> (1 - new_exp);
        return (sign << 15) | (mant >> 13) as u16;
    }
    (sign << 15) | ((new_exp as u16) << 10) | ((mant >> 13) as u16)
}

/// Pack a batch of f32 embedding vectors into the fp16 wire format used by the
/// `embeddings.bin` sidecar: each f32 scalar becomes a little-endian `u16`.
pub fn pack_fp16(embeddings: &[Vec<f32>]) -> Vec<u8> {
    let total = embeddings.iter().map(|v| v.len()).sum::<usize>();
    let mut packed = Vec::with_capacity(total * 2);
    for row in embeddings {
        for &f in row {
            packed.extend_from_slice(&f32_to_f16(f).to_le_bytes());
        }
    }
    packed
}

/// L2-normalise each row in place. The vision service is expected to return
/// normalised vectors already; this is a defensive re-normalisation so the
/// sidecar is always unit-length and cosine similarity works without further
/// pre-processing by the reader.
pub fn l2_normalise(embeddings: &mut [Vec<f32>]) {
    for row in embeddings {
        let norm = row.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in row.iter_mut() {
                *v /= norm;
            }
        }
    }
}

/// Point the GPU gate at the configured services before acquiring it for vision.
pub fn ready_vision_gpu(gpu_lock: &GpuLock, config: &RuntimeConfig) {
    gpu_lock.set_enabled(config.gpu_preempt_tts);
    gpu_lock.set_service_urls(
        config.stt_service_url.clone(),
        config.tts_service_url.clone(),
        config.vision_service_url.clone(),
    );
}

/// Send in-memory JPEG frames to the vision service, pack to fp16, and write
/// `embeddings.bin` + patch the manifest. Replaces the previous disk-spool +
/// `generate_embeddings` job hop.
///
/// On success: returns `Ok(count)` and the manifest carries `present: true`
/// with `start_time_secs` so `EmbeddingsView::window` can index correctly.
///
/// On failure (vision down, zero frames returned, etc.): returns `Err(reason)`
/// AND records `present: false` in the manifest with `attempted_at` +
/// `error`. The `attempted_at` field is what distinguishes "never ran"
/// (no manifest entry at all) from "ran and produced nothing" in the
/// supplementation gate — critical for avoiding endless reruns when vision
/// is off but skip_detect still enqueues normally.
pub async fn embed_frames(
    target_dir: &Path,
    vision_url: Option<&str>,
    frames: Vec<kob_ffmpeg::thumbnails::JpegFrame>,
    manifest: &mut Manifest,
    cancel: &CancellationToken,
) -> Result<usize, String> {
    let start_time_secs = frames.first().map(|f| f.timestamp_secs).unwrap_or(0.0);
    let result = do_embed_frames(target_dir, vision_url, frames, cancel).await;
    match &result {
        Ok(count) => {
            manifest.artefacts.set(
                Branch::Embeddings,
                ArtefactInfo {
                    present: true,
                    params: embeddings_success(*count, start_time_secs),
                },
            );
        }
        Err(reason) => {
            manifest.artefacts.set(
                Branch::Embeddings,
                ArtefactInfo {
                    present: false,
                    params: embeddings_failure(reason),
                },
            );
        }
    }
    result
}

async fn do_embed_frames(
    target_dir: &Path,
    vision_url: Option<&str>,
    frames: Vec<kob_ffmpeg::thumbnails::JpegFrame>,
    cancel: &CancellationToken,
) -> Result<usize, String> {
    let vision_url = vision_url
        .filter(|u| !u.is_empty())
        .ok_or_else(|| "vision_service_url not configured".to_string())?;

    let client = EmbeddingClient::for_url(vision_url);
    if !client.health().await {
        return Err("vision service not healthy".to_string());
    }

    // The client pipelines each batch as 4 requests of 4 images. We still chunk
    // here so a cancel mid-file aborts promptly between batches rather than only
    // between files.
    let mut jpegs: Vec<Vec<u8>> = frames.into_iter().map(|f| f.jpeg_data).collect();
    let total = jpegs.len();
    let mut all_embeddings: Vec<Vec<f32>> = Vec::with_capacity(total);
    let batch = 16;
    let mut start = 0;
    while start < jpegs.len() {
        nats_jobs::check_cancelled(cancel).map_err(|e| format!("cancelled: {e}"))?;
        let end = (start + batch).min(jpegs.len());
        let chunk: Vec<Vec<u8>> = jpegs[start..end].to_vec();
        let chunk_embeddings = client
            .embed_images(&chunk)
            .await
            .map_err(|e| format!("vision embed failed at batch {start}..{end}: {e}"))?;
        all_embeddings.extend(chunk_embeddings);
        start = end;
    }
    jpegs.clear();

    if all_embeddings.is_empty() {
        return Err("vision service returned no embeddings".to_string());
    }
    // The manifest records `EMBED_MODEL` and its width, and the reader decodes
    // rows at that width: vectors of any other width are another model's.
    if let Some(bad) = all_embeddings.iter().find(|v| v.len() != EMBED_DIM) {
        return Err(format!(
            "vision service returned {}-dimensional embeddings, expected {EMBED_DIM} ({EMBED_MODEL})",
            bad.len()
        ));
    }

    l2_normalise(&mut all_embeddings);
    let packed = pack_fp16(&all_embeddings);

    let target_dir = target_dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let sidecar = target_dir.join(
            crate::media_cache::sidecar_filename(Branch::Embeddings)
                .expect("embeddings is a file branch"),
        );
        write_atomic(&sidecar, &packed)
    })
    .await
    .map_err(|e| format!("write task panicked: {e}"))?
    .map_err(|e| format!("embeddings write failed: {e}"))?;

    Ok(all_embeddings.len())
}

/// Seconds-since-UNIX-epoch as f64; stored in the manifest so readers can
/// distinguish "never ran" (no attempted_at) from "ran but empty".
fn now_unix_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn embeddings_success(
    count: usize,
    start_time_secs: f64,
) -> serde_json::Map<String, serde_json::Value> {
    let mut params = serde_json::Map::new();
    params.insert("dim".into(), json!(EMBED_DIM));
    params.insert("count".into(), json!(count));
    params.insert("fps".into(), json!(EMBED_FPS));
    params.insert("dtype".into(), json!("f16"));
    params.insert("model".into(), json!(EMBED_MODEL));
    params.insert("image_tokens".into(), json!(IMAGE_TOKENS));
    params.insert("width".into(), json!(EMBED_WIDTH));
    params.insert("start_time_secs".into(), json!(start_time_secs));
    params.insert("attempted_at".into(), json!(now_unix_secs()));
    params
}

/// Carries the model too: an attempt that failed under the current model is not
/// stale, so skip_detect does not keep re-requesting it.
pub fn embeddings_failure(reason: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut params = serde_json::Map::new();
    params.insert("model".into(), json!(EMBED_MODEL));
    params.insert("attempted_at".into(), json!(now_unix_secs()));
    params.insert("error".into(), json!(reason));
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f16_to_f32(half: u16) -> f32 {
        let sign = ((half >> 15) & 0x1) as u32;
        let exp = ((half >> 10) & 0x1f) as u32;
        let mant = (half & 0x3ff) as u32;
        let f_bits = if exp == 0 {
            if mant == 0 {
                sign << 31
            } else {
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
            (sign << 31) | (0xff << 23) | (mant << 13)
        } else {
            (sign << 31) | ((exp + (127 - 15)) << 23) | (mant << 13)
        };
        f32::from_bits(f_bits)
    }

    #[test]
    fn roundtrip_fp16_known_values() {
        for &f in &[0.0_f32, 1.0, -1.0, 0.5, -0.25, 2.5] {
            let back = f16_to_f32(f32_to_f16(f));
            assert!((back - f).abs() < 1e-3, "f16 roundtrip {f} -> {back}");
        }
    }

    #[test]
    fn fp16_inf_overflows_to_inf() {
        let v = 1e30_f32;
        let h = f32_to_f16(v);
        assert_eq!(h & 0x7fff, 0x7c00);
    }

    #[test]
    fn pack_fp16_lengths_match() {
        let embeddings = vec![vec![0.5_f32; 4], vec![-0.5_f32; 4]];
        let packed = pack_fp16(&embeddings);
        assert_eq!(packed.len(), 2 * 4 * 2);
    }

    #[test]
    fn l2_normalise_is_unit_length() {
        let mut embeddings = vec![vec![3.0_f32, 4.0_f32]];
        l2_normalise(&mut embeddings);
        let norm = embeddings[0].iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[test]
    fn l2_normalise_zero_vector_is_zero() {
        let mut embeddings = vec![vec![0.0_f32, 0.0, 0.0]];
        l2_normalise(&mut embeddings);
        assert_eq!(embeddings[0], vec![0.0, 0.0, 0.0]);
    }
}
