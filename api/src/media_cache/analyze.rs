//! Path-based single-pass analysis primitive.
//!
//! Given an input media file and a target cache directory, runs
//! `kob_ffmpeg::single_pass::run_single_pass` once, writes every successful
//! branch's sidecar, and returns an in-memory `Manifest` plus per-branch
//! outcomes. No DB, no IO gate, no GPU lock, no `skip_detect` chain — the
//! caller composes those around this core.
//!
//! Consumers:
//! - `jobs::handlers::media_analyze` wraps this with file_id resolution,
//!   `media_cache_runs` tracking, vision embeddings, and the skip_detect
//!   enqueue.
//! - `jobs::handlers::debrid_process` calls this with `cache_dir` set to a
//!   sibling of the debrid cache file and `want_embed_src=false`.
//! - `jobs::handlers::iptv_record` calls this for recordings the same way.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kob_ffmpeg::single_pass::{
    AudioPeaksParams, BlackFramesParams, BranchId, ChromaprintParams, CorruptionParams,
    CustomSinks, EmbedSrcParams, SilenceParams, SinglePassRequest, SinglePassResult,
    TrickplayParams, VhashParams,
};
use kob_ffmpeg::thumbnails::JpegFrame;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::trickplay_sink::{IncrementalSpriteSink, shared as shared_trickplay_sink};
use super::writers::{
    ArtefactInfo, BlackGap, Loudness, Manifest, SilenceGap, new_manifest_at, write_manifest_at,
    write_sidecar_at,
};
use super::{Branch, PIPELINE_VERSION};
use crate::jobs::{JobError, JobResult};
use crate::skip_detect::primitives::vhash_align::{FrameDescriptor, quantize_descriptor};

/// embed_src JPEG width — must match the vision server's expected input.
pub const EMBED_WIDTH: i32 = 768;
/// Frames-per-second at which embed_src is grabbed.
pub const EMBED_FPS: f64 = 1.0;

/// All the knobs `run_analysis` needs. Caller computes paths + windows.
pub struct AnalyzeInputs {
    pub input_path: PathBuf,
    /// Where file-bound sidecars (audio_peaks) + the per-file manifest live.
    /// Trickplay sprites go to `trickplay_dir`. Content-bound sidecars
    /// (vhash, chromaprint, silence, loudness, black_frames, embeddings) go
    /// to `episode_cache_dir` when set; otherwise they're skipped.
    pub cache_dir: PathBuf,
    /// Where sprite JPEGs + thumbnails.vtt go. Per-file by design — sprites
    /// describe a specific video file, not an episode.
    pub trickplay_dir: PathBuf,
    /// If true, video branches are enabled (trickplay, vhash, black_frames,
    /// and optionally embed_src). If false, only audio branches run plus
    /// `audio_peaks` for the waveform scrubber.
    pub is_video: bool,
    /// Duration from the DB row, used to narrow vhash/embed_src to the V1-
    /// equivalent windows. `None` falls back to whole-file extraction.
    pub known_duration: Option<f64>,
    /// Capture embed_src JPEGs and return them in `AnalyzeOutcome.embed_src_frames`.
    /// Set by callers that plan to send the frames to the vision service.
    pub want_embed_src: bool,
    /// `"episode" | "movie" | "audiobook" | "recording" | "debrid" | ...`.
    /// Recorded for logging only — branch selection uses `is_video`.
    pub media_type: String,
    /// If true, write the manifest to disk before returning. The native
    /// kobbler handler sets this to false because it will update the manifest
    /// with embeddings info and write it once at the end. Path-based callers
    /// (debrid, iptv) that don't do embeddings set this to true.
    pub write_manifest_on_success: bool,
    /// Supplement-pass mode. When true, only the embed_src branch runs — all
    /// other branches (trickplay, vhash, chromaprint, silence, loudness,
    /// black_frames, audio_peaks) are skipped. Used by the warm-cache
    /// re-enqueue path that needs to fill in embeddings when vision was
    /// turned on after the first analyze. `is_video` + `want_embed_src` must
    /// both be true when this is set.
    pub only_embed_src: bool,
    /// Where content-bound artefacts go: `episode_cache/{namespace}/{id}/`.
    /// `None` means "this analyze run has no canonical episode/movie key" —
    /// content sidecars are skipped entirely (e.g. iptv recordings, which
    /// only need trickplay + audio_peaks). When `Some`, `analyzed_file_id`
    /// should usually also be `Some` so the manifest carries provenance.
    pub episode_cache_dir: Option<PathBuf>,
    /// Provenance UUID written into the episode_cache manifest's
    /// `analyzed_file_id` field. Native ingest passes its `tv_episode_files.id`
    /// or `movie_files.id`; debrid passes the canonical media_id. Ignored
    /// when `episode_cache_dir` is `None`.
    pub analyzed_file_id: Option<uuid::Uuid>,
}

/// Output of `run_analysis`. The manifest is in memory so the native-kobbler
/// caller can update it with embeddings before the final write.
pub struct AnalyzeOutcome {
    /// When `inputs.episode_cache_dir` was `Some`, this is the **content**
    /// (episode_cache) manifest with vhash/chromaprint/silence/loudness/
    /// black_frames info. The caller usually appends embeddings to it and
    /// writes it back to `inputs.episode_cache_dir`. When that input was
    /// `None`, this is the per-file manifest carrying only trickplay +
    /// audio_peaks info.
    pub manifest: Manifest,
    pub branch_results: Map<String, Value>,
    pub input_duration_secs: f64,
    pub per_branch_errors: Vec<(BranchId, String)>,
    pub branch_durations: HashMap<BranchId, Duration>,
    /// Populated only when `AnalyzeInputs.want_embed_src` was true. Caller
    /// sends these to vision. Empty otherwise.
    pub embed_src_frames: Vec<JpegFrame>,
    /// Time ranges where the SW decoder hit error-concealment / missing
    /// references / invalid bitstream. Populated when video branches ran;
    /// empty otherwise. Caller (media_analyze handler) persists these to the
    /// owning file row's `corrupt_ranges` column so the streaming layer can
    /// pre-emptively transcode the affected windows.
    pub corrupt_ranges: Vec<crate::corruption::CorruptRange>,
    pub single_pass_duration: Duration,
    pub post_decode_duration: Duration,
}

/// Run the single-pass decode, write sidecars + trickplay sprites, return
/// in-memory manifest. See module docs for composition.
pub async fn run_analysis(
    inputs: AnalyzeInputs,
    cancel: &CancellationToken,
) -> JobResult<AnalyzeOutcome> {
    nats_jobs::check_cancelled(cancel)?;

    std::fs::create_dir_all(&inputs.cache_dir).map_err(|e| {
        JobError::ExecutionFailed(format!(
            "create cache_dir {}: {e}",
            inputs.cache_dir.display()
        ))
    })?;

    // canonical vhash/embed_src windows when known_duration is set.
    let (vhash_windows, embed_windows) = match inputs.known_duration.filter(|d| *d > 0.0) {
        Some(duration) => {
            let intro_window = crate::skip_detect::extract_duration_secs(duration) as f64;
            let vhash_credits_start = duration * 0.80;
            let credits_window = crate::skip_detect::credits_window_secs(&inputs.media_type) as f64;
            let embed_scan_start = (duration - credits_window).max(0.0);
            (
                vec![(0.0, intro_window), (vhash_credits_start, duration)],
                vec![(embed_scan_start, duration)],
            )
        }
        None => {
            warn!(
                media_type = %inputs.media_type,
                "No known_duration — falling back to whole-file vhash + embed_src"
            );
            (Vec::new(), Vec::new())
        }
    };

    let mut req = SinglePassRequest::new(inputs.input_path.clone());
    if inputs.only_embed_src {
        // Supplement pass: video decode only, embed_src sink. `single_pass`
        // discards the audio stream when no audio branches are configured, so
        // we pay nothing for audio decode.
        req.embed_src = Some(EmbedSrcParams {
            width: EMBED_WIDTH,
            fps: EMBED_FPS,
            jpeg_quality: 3,
            windows: embed_windows,
        });
    } else if inputs.is_video {
        req.trickplay = Some(TrickplayParams {
            width: 320,
            fps: 0.2,
            jpeg_quality: 5,
        });
        req.vhash = Some(VhashParams {
            intermediate_width: 64,
            fps: 3.0,
            windows: vhash_windows,
        });
        if inputs.want_embed_src {
            req.embed_src = Some(EmbedSrcParams {
                width: EMBED_WIDTH,
                fps: EMBED_FPS,
                jpeg_quality: 3,
                windows: embed_windows,
            });
        }
        req.black_frames = Some(BlackFramesParams::default());
        req.corruption = Some(CorruptionParams::default());
        req.chromaprint = Some(ChromaprintParams::default());
        req.silence = Some(SilenceParams::default());
        req.loudness = true;
    } else {
        req.audio_peaks = Some(AudioPeaksParams::default());
        req.chromaprint = Some(ChromaprintParams::default());
        req.silence = Some(SilenceParams::default());
        req.loudness = true;
    }

    let req_corruption_enabled = req.corruption.is_some();

    // Progressive trickplay: stream sprite sheets + thumbnails.vtt to disk as
    // frames are decoded so the player can scrub against partial output well
    // before the analyze pass finishes. Held behind `Arc<Mutex<_>>` so we can
    // read its summary back after `run_single_pass_with_sinks` consumes the
    // boxed sink. Only constructed when the request actually wants trickplay.
    let (sink_handle_arc, custom_sinks) = if req.trickplay.is_some() {
        let sink = IncrementalSpriteSink::new(
            inputs.trickplay_dir.clone(),
            inputs.known_duration.unwrap_or(0.0),
        );
        let (arc, boxed) = shared_trickplay_sink(sink);
        (
            Some(arc),
            CustomSinks {
                trickplay: Some(boxed),
            },
        )
    } else {
        (None, CustomSinks::default())
    };

    let single_pass_started = Instant::now();
    let mut result: SinglePassResult = tokio::task::spawn_blocking(move || {
        kob_ffmpeg::single_pass::run_single_pass_with_sinks(req, None, custom_sinks)
    })
    .await
    .map_err(|e| JobError::ExecutionFailed(format!("single_pass task panicked: {e}")))?
    .map_err(|e| JobError::ExecutionFailed(format!("single_pass failed: {e}")))?;
    let single_pass_duration = single_pass_started.elapsed();

    let input_duration = result.input_duration_secs;
    let mut branch_results = Map::new();
    let per_branch_errors: Vec<(BranchId, String)> = result
        .per_branch_errors
        .iter()
        .map(|(id, e)| (*id, e.to_string()))
        .collect();
    for (id, err) in &per_branch_errors {
        branch_results.insert(
            format!("{id:?}").to_lowercase(),
            json!({"ok": false, "error": err}),
        );
    }

    let post_decode_started = Instant::now();
    // Per-file manifest records file-bound artefacts (trickplay, audio_peaks)
    // and lives at inputs.cache_dir. Content manifest records content-bound
    // artefacts (vhash, chromaprint, silence, loudness, black_frames,
    // embeddings) and lives at inputs.episode_cache_dir when set. When the
    // caller didn't supply a target (audiobooks, iptv recordings), the two
    // are merged and written as one per-file manifest at the end —
    // those media types have no cross-episode reuse so file-locality is
    // correct.
    let mut per_file_manifest = new_manifest_at(&inputs.trickplay_dir, input_duration);
    let mut content_manifest = new_manifest_at(&inputs.trickplay_dir, input_duration);
    content_manifest.analyzed_file_id = inputs.analyzed_file_id;
    content_manifest.analyzed_at = Some(chrono::Utc::now());

    // Where content sidecars actually land. Falls through to the per-file
    // cache_dir for callers without a target so audiobook loudness etc.
    // continues to work.
    let content_dir: &Path = inputs
        .episode_cache_dir
        .as_deref()
        .unwrap_or(inputs.cache_dir.as_path());
    if let Some(dir) = inputs.episode_cache_dir.as_deref() {
        std::fs::create_dir_all(dir).map_err(|e| {
            JobError::ExecutionFailed(format!("create episode_cache_dir {}: {e}", dir.display()))
        })?;
    }

    // Trickplay — sprites + thumbnails.vtt were already written progressively
    // during decode by IncrementalSpriteSink. Pull its summary back out of the
    // shared handle and record it in the per-file manifest.
    if let Some(handle) = sink_handle_arc {
        // After spawn_blocking returned, the boxed adapter has been dropped, so
        // strong_count is 1 and lock contention is impossible.
        let summary = handle.lock().expect("trickplay sink mutex").summary();
        let frame_count = summary.total_thumbs as usize;
        let trickplay_failed = result
            .per_branch_errors
            .iter()
            .any(|(id, _)| matches!(id, BranchId::Trickplay));
        if frame_count == 0 || trickplay_failed {
            warn!(
                frame_count,
                trickplay_failed, "trickplay produced no usable sprites"
            );
            branch_results.insert(
                "trickplay".into(),
                json!({"ok": false, "error": "no frames"}),
            );
        } else {
            let mut params = Map::new();
            params.insert("width".into(), json!(320));
            params.insert("fps".into(), json!(0.2));
            params.insert("jpeg_quality".into(), json!(5));
            params.insert("frame_count".into(), json!(frame_count));
            params.insert("sprite_count".into(), json!(summary.sprite_count));
            params.insert("tile_cols".into(), json!(summary.tile_cols));
            params.insert("tile_rows".into(), json!(summary.tile_rows));
            params.insert("thumb_width".into(), json!(summary.thumb_width));
            params.insert("thumb_height".into(), json!(summary.thumb_height));
            per_file_manifest.artefacts.set(
                Branch::Trickplay,
                ArtefactInfo {
                    present: true,
                    params,
                },
            );
            branch_results.insert(
                "trickplay".into(),
                json!({
                    "ok": true,
                    "frame_count": frame_count,
                    "sprite_count": summary.sprite_count,
                }),
            );
        }
    }

    // Vhash — content-bound (episode_cache when target is set, per-file
    // media_cache otherwise).
    if let Some(descriptors) = result.vhash {
        let frame_count = descriptors.len();
        let frame_descs: Vec<FrameDescriptor> = descriptors
            .into_iter()
            .map(|d| FrameDescriptor {
                timestamp: d.timestamp_secs,
                quantized: quantize_descriptor(&d.data),
                values: d.data,
            })
            .collect();
        let bytes = crate::skip_detect::vhash::serialize_descriptors(&frame_descs);
        match write_sidecar_at(content_dir, Branch::Vhash, &bytes) {
            Ok(()) => {
                let mut params = Map::new();
                params.insert("intermediate_width".into(), json!(64));
                params.insert("fps".into(), json!(3.0));
                params.insert("frame_count".into(), json!(frame_count));
                content_manifest.artefacts.set(
                    Branch::Vhash,
                    ArtefactInfo {
                        present: true,
                        params,
                    },
                );
                branch_results.insert(
                    "vhash".into(),
                    json!({"ok": true, "frame_count": frame_count}),
                );
            }
            Err(e) => {
                warn!(error = %e, "vhash write failed");
                branch_results.insert("vhash".into(), json!({"ok": false, "error": e.to_string()}));
            }
        }
    }

    // Chromaprint — content-bound.
    if let Some(cp) = result.chromaprint {
        let mut bytes = Vec::with_capacity(cp.raw.len() * 4);
        for v in &cp.raw {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        match write_sidecar_at(content_dir, Branch::Chromaprint, &bytes) {
            Ok(()) => {
                let mut params = Map::new();
                params.insert("len_u32".into(), json!(cp.raw.len()));
                params.insert("duration_secs".into(), json!(cp.duration_secs));
                content_manifest.artefacts.set(
                    Branch::Chromaprint,
                    ArtefactInfo {
                        present: true,
                        params,
                    },
                );
                branch_results.insert(
                    "chromaprint".into(),
                    json!({"ok": true, "len_u32": cp.raw.len()}),
                );
            }
            Err(e) => {
                warn!(error = %e, "chromaprint write failed");
                branch_results.insert(
                    "chromaprint".into(),
                    json!({"ok": false, "error": e.to_string()}),
                );
            }
        }
    }

    // Silence.
    if let Some(intervals) = result.silence {
        let gaps: Vec<SilenceGap> = intervals
            .iter()
            .map(|i| SilenceGap {
                start_secs: i.start_secs,
                end_secs: i.end_secs,
                duration_secs: i.duration_secs,
            })
            .collect();
        match serde_json::to_vec(&gaps) {
            Ok(bytes) => match write_sidecar_at(content_dir, Branch::Silence, &bytes) {
                Ok(()) => {
                    let mut params = Map::new();
                    params.insert("noise_db".into(), json!(-30.0));
                    params.insert("min_duration_secs".into(), json!(0.5));
                    params.insert("count".into(), json!(gaps.len()));
                    content_manifest.artefacts.set(
                        Branch::Silence,
                        ArtefactInfo {
                            present: true,
                            params,
                        },
                    );
                    branch_results
                        .insert("silence".into(), json!({"ok": true, "count": gaps.len()}));
                }
                Err(e) => {
                    warn!(error = %e, "silence write failed");
                    branch_results.insert(
                        "silence".into(),
                        json!({"ok": false, "error": e.to_string()}),
                    );
                }
            },
            Err(e) => {
                warn!(error = %e, "silence serialize failed");
                branch_results.insert(
                    "silence".into(),
                    json!({"ok": false, "error": e.to_string()}),
                );
            }
        }
    }

    // Black frames — content-bound.
    if let Some(intervals) = result.black_frames {
        let gaps: Vec<BlackGap> = intervals
            .iter()
            .map(|i| BlackGap {
                start_secs: i.start_secs,
                end_secs: i.end_secs,
                duration_secs: i.duration_secs,
            })
            .collect();
        match serde_json::to_vec(&gaps) {
            Ok(bytes) => match write_sidecar_at(content_dir, Branch::BlackFrames, &bytes) {
                Ok(()) => {
                    let mut params = Map::new();
                    params.insert("min_duration_secs".into(), json!(0.1));
                    params.insert("pixel_threshold".into(), json!(0.10));
                    params.insert("count".into(), json!(gaps.len()));
                    content_manifest.artefacts.set(
                        Branch::BlackFrames,
                        ArtefactInfo {
                            present: true,
                            params,
                        },
                    );
                    branch_results.insert(
                        "black_frames".into(),
                        json!({"ok": true, "count": gaps.len()}),
                    );
                }
                Err(e) => {
                    warn!(error = %e, "black_frames write failed");
                    branch_results.insert(
                        "black_frames".into(),
                        json!({"ok": false, "error": e.to_string()}),
                    );
                }
            },
            Err(e) => {
                warn!(error = %e, "black_frames serialize failed");
                branch_results.insert(
                    "black_frames".into(),
                    json!({"ok": false, "error": e.to_string()}),
                );
            }
        }
    }

    // Loudness — content-bound. Audiobook callers (target=None) get this
    // written into per-file media_cache which keeps the DB lufs-column
    // update at media_analyze.rs working without a separate code path.
    let loudness = Loudness {
        integrated_lufs: result.loudness,
    };
    match serde_json::to_vec(&loudness) {
        Ok(bytes) => match write_sidecar_at(content_dir, Branch::Loudness, &bytes) {
            Ok(()) => {
                let mut params = Map::new();
                params.insert("integrated_lufs".into(), json!(loudness.integrated_lufs));
                content_manifest.artefacts.set(
                    Branch::Loudness,
                    ArtefactInfo {
                        present: true,
                        params,
                    },
                );
                branch_results.insert(
                    "loudness".into(),
                    json!({"ok": true, "integrated_lufs": loudness.integrated_lufs}),
                );
            }
            Err(e) => {
                warn!(error = %e, "loudness write failed");
                branch_results.insert(
                    "loudness".into(),
                    json!({"ok": false, "error": e.to_string()}),
                );
            }
        },
        Err(e) => {
            warn!(error = %e, "loudness serialize failed");
            branch_results.insert(
                "loudness".into(),
                json!({"ok": false, "error": e.to_string()}),
            );
        }
    }

    // Audio peaks — file-bound (audiobook waveform scrubber). Stays in
    // per-file media_cache.
    if let Some(peaks) = result.audio_peaks {
        match write_sidecar_at(&inputs.cache_dir, Branch::AudioPeaks, &peaks.bytes) {
            Ok(()) => {
                let mut params = Map::new();
                params.insert("sample_rate".into(), json!(peaks.sample_rate));
                params.insert("byte_len".into(), json!(peaks.bytes.len()));
                per_file_manifest.artefacts.set(
                    Branch::AudioPeaks,
                    ArtefactInfo {
                        present: true,
                        params,
                    },
                );
                branch_results.insert(
                    "audio_peaks".into(),
                    json!({"ok": true, "byte_len": peaks.bytes.len()}),
                );
            }
            Err(e) => {
                warn!(error = %e, "audio_peaks write failed");
                branch_results.insert(
                    "audio_peaks".into(),
                    json!({"ok": false, "error": e.to_string()}),
                );
            }
        }
    }

    let embed_src_frames = result.embed_src.take().unwrap_or_default();
    let embed_src_ok = inputs.want_embed_src
        && !embed_src_frames.is_empty()
        && !result
            .per_branch_errors
            .iter()
            .any(|(id, _)| matches!(id, BranchId::EmbedSrc));
    if inputs.want_embed_src {
        branch_results.insert(
            "embed_src".into(),
            json!({"ok": embed_src_ok, "count": embed_src_frames.len()}),
        );
    }

    let corrupt_ranges: Vec<crate::corruption::CorruptRange> = result
        .corruption
        .take()
        .unwrap_or_default()
        .into_iter()
        .map(Into::into)
        .collect();
    if req_corruption_enabled {
        let corruption_failed = result
            .per_branch_errors
            .iter()
            .any(|(id, _)| matches!(id, BranchId::Corruption));
        branch_results.insert(
            "corruption".into(),
            json!({"ok": !corruption_failed, "ranges": corrupt_ranges.len()}),
        );
        if !corrupt_ranges.is_empty() {
            warn!(
                ranges = corrupt_ranges.len(),
                first_start = corrupt_ranges.first().map(|r| r.start_secs),
                "corruption detected — playback layer will transcode affected ranges",
            );
        }
    }

    // Manifest persistence:
    //   target Some → split: per-file manifest (trickplay+audio_peaks) at
    //                  inputs.cache_dir, content manifest at episode_cache_dir.
    //   target None → merge content branches into per_file_manifest, write
    //                  one combined manifest at inputs.cache_dir. This keeps
    //                  audiobook/recording behaviour identical to pre-refactor.
    let target_dir = inputs.episode_cache_dir.as_deref();
    if target_dir.is_none() {
        merge_content_into_per_file(&content_manifest, &mut per_file_manifest);
    }
    if inputs.write_manifest_on_success {
        if let Err(e) = write_manifest_at(&inputs.cache_dir, &per_file_manifest) {
            warn!(error = %e, "per-file manifest write failed");
        }
        if let Some(dir) = target_dir
            && let Err(e) = write_manifest_at(dir, &content_manifest)
        {
            warn!(error = %e, "episode_cache manifest write failed");
        }
    }

    let post_decode_duration = post_decode_started.elapsed();

    // Keep PIPELINE_VERSION reachable from the module (caller may want to log it).
    let _ = PIPELINE_VERSION;

    // Returned manifest is the one the caller appends embeddings to. When
    // a target was supplied that's the content manifest (which lands in
    // episode_cache); without a target it's the merged per-file manifest.
    let manifest = if target_dir.is_some() {
        content_manifest
    } else {
        per_file_manifest
    };

    Ok(AnalyzeOutcome {
        manifest,
        branch_results,
        input_duration_secs: input_duration,
        per_branch_errors,
        branch_durations: result.branch_durations,
        embed_src_frames,
        corrupt_ranges,
        single_pass_duration,
        post_decode_duration,
    })
}

fn merge_content_into_per_file(content: &Manifest, per_file: &mut Manifest) {
    let m = &mut per_file.artefacts;
    let c = &content.artefacts;
    if c.vhash.is_some() {
        m.vhash = c.vhash.clone();
    }
    if c.chromaprint.is_some() {
        m.chromaprint = c.chromaprint.clone();
    }
    if c.silence.is_some() {
        m.silence = c.silence.clone();
    }
    if c.loudness.is_some() {
        m.loudness = c.loudness.clone();
    }
    if c.black_frames.is_some() {
        m.black_frames = c.black_frames.clone();
    }
    if c.embeddings.is_some() {
        m.embeddings = c.embeddings.clone();
    }
}
