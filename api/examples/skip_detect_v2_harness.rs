//! Standalone skip_detect harness — runs the production pipeline against
//! TV seasons on disk, without touching kobbler's database or job queue.
//!
//! With `--vision-url` this mirrors the full prod vision path:
//!   1. `kob_ffmpeg::single_pass` with the exact parameters `media_analyze.rs`
//!      uses for video files (chromaprint + silence + black_frames + vhash +
//!      loudness + embed_src).
//!   2. JPEG frames from the embed_src branch are sent to the vision service
//!      via `api::skip_detect::vision::VisionClient::embed_images` — the same
//!      client prod uses, same batching (VISION_BATCH_SIZE = 16).
//!   3. Embeddings are l2-normalised and fp16-packed via
//!      `api::media_cache::embeddings::{l2_normalise, pack_fp16}` — identical
//!      to `media_analyze::embed_frames_inline` — then written to the
//!      `Branch::Embeddings` sidecar. Manifest params (dim, count, fps, dtype,
//!      model, width, start_time_secs, attempted_at) match prod byte for byte.
//!   4. `api::skip_detect::cache_detect::detect_season_from_cache_with_chapters`
//!      runs over the cache — the same function the prod handler wraps — so
//!      `cache.embeddings` is populated and vision-driven signals
//!      (`credit_text_score`, `visual_score`) land on every candidate.
//!   5. Diagnostics print what prod would have written to
//!      `skip_detect_candidates`: the full `CandidateRow` shape per file, so
//!      you can eyeball the rows without a live Postgres.
//!
//! What's *not* mirrored: trickplay sprite tiling (irrelevant to vision +
//! skip_detect), LUFS DB writeback, `media_cache_runs` tracking, and the
//! `episode_skip_segments` upsert. None of those change what vision produces
//! or which candidates land. If you care about those, add `--dump-rows` is
//! enough audit for the candidate persistence shape.
//!
//! Usage:
//!   cargo run --release --example skip_detect_v2_harness -- \
//!     --storage /tmp/kob-harness \
//!     --vision-url http://192.0.2.40:8890 \
//!     "/path/to/Season 01" \
//!     "/path/to/Season 02"
//!
//!   --vision-url URL          Enables embed_src + vision.embed_images call.
//!                             Omit for CPU-only (audio + vhash only).
//!   --force                   Rebuild cache even if manifest already present.
//!   --jobs N                  Parallel decode workers (default 1).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use api::media_cache::embeddings::{l2_normalise, pack_fp16};
use api::media_cache::episode_cache::{
    MediaKind, episode_cache_dir, read_manifest as read_episode_manifest,
};
use api::media_cache::writers::{
    ArtefactInfo, BlackGap, Loudness, Manifest, SilenceGap, new_manifest_at, write_manifest_at,
    write_sidecar_at,
};
use api::media_cache::{self, Branch, PIPELINE_VERSION};
use api::repositories::skip_detect_candidates::CandidateRow;
use api::skip_detect::cache_detect::{
    EpisodeDiagnostics, MatchDiag, detect_season_from_cache_with_chapters,
};
use api::skip_detect::tv::EpisodeFile;
use api::skip_detect::vision::VisionClient;
use api::skip_detect::{
    AudioProfile, Candidate, DetectedSegment, SegmentType, extract_duration_secs,
};
use kob_ffmpeg::single_pass::{
    BlackFramesParams, ChromaprintParams, EmbedSrcParams, SilenceParams, SinglePassRequest,
    SinglePassResult, VhashParams, run_single_pass,
};
use kob_ffmpeg::thumbnails::JpegFrame;
use serde_json::json;
use tokio::sync::Semaphore;
use uuid::Uuid;

#[tokio::main]
async fn main() {
    let args = Args::parse();
    setup_tracing();

    fs::create_dir_all(&args.storage_root).expect("create storage_root");

    let mut seasons: Vec<SeasonPlan> = Vec::new();
    for dir in &args.season_dirs {
        match SeasonPlan::discover(dir, &args.storage_root) {
            Ok(plan) => seasons.push(plan),
            Err(e) => eprintln!("! Skipping {dir:?}: {e}"),
        }
    }
    if seasons.is_empty() {
        eprintln!("No seasons with video files found.");
        std::process::exit(1);
    }

    // Probe durations + chapters (cheap, serial). We need duration to compute
    // vhash/embed windows identically to media_analyze.
    for plan in &mut seasons {
        plan.probe_episodes();
    }

    // Run single_pass across every episode that needs (re)extraction.
    let decode_sem = Arc::new(Semaphore::new(args.jobs.max(1)));
    let mut handles = Vec::new();
    for plan in &seasons {
        for ep in &plan.episodes {
            if !ep.should_decode(args.force) {
                continue;
            }
            let sem = decode_sem.clone();
            let ep = ep.clone();
            let storage = args.storage_root.clone();
            let vision_url = args.vision_url.clone();
            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire_owned().await.expect("semaphore");
                run_and_persist(&ep, &storage, vision_url.as_deref()).await
            }));
        }
    }
    for h in handles {
        if let Err(e) = h.await {
            eprintln!("! decode task panicked: {e}");
        }
    }

    // Run detection per-season.
    for plan in &seasons {
        let episodes: Vec<EpisodeFile> = plan
            .episodes
            .iter()
            .map(|ep| EpisodeFile {
                file_id: ep.file_id,
                file_path: ep.source_path.to_string_lossy().into_owned(),
                duration_seconds: ep.duration_seconds,
                episode_id: None,
            })
            .collect();
        let mut chapters_by_file: HashMap<Uuid, Vec<(f64, Option<String>)>> = HashMap::new();
        for ep in &plan.episodes {
            chapters_by_file.insert(ep.file_id, ep.chapters.clone());
        }

        println!("\n════════════════════════════════════════════════════════");
        println!("SEASON: {}", plan.label);
        println!(
            "  {} episode(s), storage_root={}",
            episodes.len(),
            args.storage_root.display()
        );
        println!("════════════════════════════════════════════════════════");

        let start = Instant::now();
        let results = detect_season_from_cache_with_chapters(
            args.vision_url.as_deref(),
            &episodes,
            &args.storage_root.to_string_lossy(),
            &chapters_by_file,
        )
        .await;
        let detect_elapsed = start.elapsed();
        println!(
            "  detection: {:.2}s wall across {} episode(s)",
            detect_elapsed.as_secs_f64(),
            episodes.len()
        );

        // Build a lookup of episode label by file_id so we print pretty names.
        let label_by_id: HashMap<Uuid, String> = plan
            .episodes
            .iter()
            .map(|ep| (ep.file_id, ep.label.clone()))
            .collect();

        for (fid, segments, diag) in &results {
            let label = label_by_id
                .get(fid)
                .cloned()
                .unwrap_or_else(|| fid.to_string());
            print_episode_report(&label, segments, diag);
            print_candidate_rows(&label, *fid, diag);
        }

        print_season_summary(&plan.label, &results, &label_by_id);
        print_vision_coverage(&plan.label, &results, &label_by_id);

        if let Some(out_root) = args.extract_clips_dir.as_deref() {
            let source_by_id: HashMap<Uuid, &Path> = plan
                .episodes
                .iter()
                .map(|ep| (ep.file_id, ep.source_path.as_path()))
                .collect();
            extract_clips_for_season(out_root, &plan.label, &results, &source_by_id, &label_by_id);
        }
    }
}

/// For every picked DetectedSegment in `results`, stream-copy the bytes
/// between (start, end) out of the source mkv. Output naming embeds the
/// segment type and timestamps so the user can eyeball whether the picker
/// is landing on real intro/credits frames.
fn extract_clips_for_season(
    out_root: &Path,
    season_label: &str,
    results: &[(Uuid, Vec<DetectedSegment>, EpisodeDiagnostics)],
    source_by_id: &HashMap<Uuid, &Path>,
    label_by_id: &HashMap<Uuid, String>,
) {
    let safe_season = sanitize_path_component(season_label);
    let season_out = out_root.join(safe_season);
    if let Err(e) = fs::create_dir_all(&season_out) {
        eprintln!("! extract: create {season_out:?} failed: {e}");
        return;
    }
    let mut written = 0usize;
    for (fid, segments, _diag) in results {
        let Some(src) = source_by_id.get(fid) else {
            continue;
        };
        let ep_label = label_by_id
            .get(fid)
            .cloned()
            .unwrap_or_else(|| fid.to_string());
        let safe_ep = sanitize_path_component(&ep_label);
        for seg in segments {
            let start = seg.start_seconds.max(0.0);
            let dur = (seg.end_seconds - seg.start_seconds).max(0.0);
            if dur < 0.5 {
                continue;
            }
            let name = format!(
                "{safe_ep}__{seg}__{start:07.1}-{end:07.1}.mkv",
                seg = seg.segment_type.as_str(),
                start = start,
                end = seg.end_seconds,
            );
            let out_path = season_out.join(&name);
            let status = std::process::Command::new("ffmpeg")
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-ss",
                    &format!("{start}"),
                    "-t",
                    &format!("{dur}"),
                    "-i",
                ])
                .arg(src)
                .args(["-c", "copy", "-map", "0:v:0", "-map", "0:a:0?"])
                .arg(&out_path)
                .status();
            match status {
                Ok(s) if s.success() => {
                    written += 1;
                    println!("  ✔ extract: {}", out_path.display());
                }
                Ok(s) => eprintln!("  ! extract: ffmpeg exit {s}: {}", out_path.display()),
                Err(e) => eprintln!("  ! extract: spawn ffmpeg failed: {e}"),
            }
        }
    }
    println!(
        "─── extracted {written} clip(s) for {season_label} into {}",
        season_out.display()
    );
}

fn sanitize_path_component(s: &str) -> String {
    s.chars()
        .map(|c| if matches!(c, '/' | '\\') { '_' } else { c })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// CLI
// ─────────────────────────────────────────────────────────────────────────────

struct Args {
    storage_root: PathBuf,
    vision_url: Option<String>,
    force: bool,
    jobs: usize,
    extract_clips_dir: Option<PathBuf>,
    season_dirs: Vec<PathBuf>,
}

impl Args {
    fn parse() -> Self {
        let mut storage_root = PathBuf::from("/tmp/kob-skip-detect-harness");
        let mut vision_url: Option<String> = None;
        let mut force = false;
        let mut jobs: usize = 1;
        let mut extract_clips_dir: Option<PathBuf> = None;
        let mut season_dirs: Vec<PathBuf> = Vec::new();

        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "--storage" => storage_root = PathBuf::from(it.next().expect("--storage <path>")),
                "--vision-url" => vision_url = Some(it.next().expect("--vision-url <url>")),
                "--force" => force = true,
                "--jobs" => jobs = it.next().expect("--jobs N").parse().expect("jobs int"),
                "--extract-clips" => {
                    extract_clips_dir =
                        Some(PathBuf::from(it.next().expect("--extract-clips <dir>")))
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                _ if a.starts_with("--") => {
                    eprintln!("unknown flag: {a}");
                    std::process::exit(2);
                }
                _ => season_dirs.push(PathBuf::from(a)),
            }
        }

        if season_dirs.is_empty() {
            print_help();
            std::process::exit(2);
        }
        Self {
            storage_root,
            vision_url,
            force,
            jobs,
            extract_clips_dir,
            season_dirs,
        }
    }
}

fn print_help() {
    eprintln!(
        "Usage: skip_detect_v2_harness [--storage PATH] [--vision-url URL] \
         [--force] [--jobs N] [--extract-clips DIR] SEASON_DIR ...\n\
         \n\
         --extract-clips DIR  After detection, stream-copy each picked Intro / \
         Credits / PostCreditsScene\n\
                              segment to DIR/<season>/<episode>__<seg>__<start>-<end>.mkv \
         using ffmpeg.\n\
                              Lets you eyeball-verify the picks land on real \
         intro/credits frames."
    );
}

fn setup_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,api::skip_detect=info"));
    fmt().with_env_filter(filter).with_target(false).init();
}

// ─────────────────────────────────────────────────────────────────────────────
// Season + episode discovery
// ─────────────────────────────────────────────────────────────────────────────

struct SeasonPlan {
    label: String,
    episodes: Vec<EpisodePlan>,
}

impl SeasonPlan {
    fn discover(dir: &Path, storage_root: &Path) -> Result<Self, String> {
        let label = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| dir.to_string_lossy().into_owned());
        let parent = dir
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let pretty = if parent.is_empty() {
            label.clone()
        } else {
            format!("{parent} / {label}")
        };

        let mut entries: Vec<PathBuf> = fs::read_dir(dir)
            .map_err(|e| format!("read_dir: {e}"))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "mkv" | "mp4" | "m4v" | "avi" | "mov" | "ts" | "webm"
                )
            })
            .collect();
        entries.sort();
        if entries.is_empty() {
            return Err("no video files found".into());
        }

        let episodes = entries
            .into_iter()
            .map(|source_path| {
                // Deterministic file_id per absolute path so re-runs re-use the
                // cache under storage_root/media_cache/<file_id>/.
                let file_id = Uuid::new_v5(
                    &Uuid::NAMESPACE_URL,
                    source_path.to_string_lossy().as_bytes(),
                );
                let label = source_path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| source_path.to_string_lossy().into_owned());
                EpisodePlan {
                    file_id,
                    label,
                    source_path,
                    storage_root: storage_root.to_path_buf(),
                    duration_seconds: 0.0,
                    chapters: Vec::new(),
                }
            })
            .collect();

        Ok(Self {
            label: pretty,
            episodes,
        })
    }

    fn probe_episodes(&mut self) {
        for ep in &mut self.episodes {
            match kob_ffmpeg::probe::probe(&ep.source_path) {
                Ok(p) => {
                    ep.duration_seconds = p.format.duration_seconds.unwrap_or(0.0);
                    ep.chapters = p
                        .chapters
                        .into_iter()
                        .map(|c| (c.start_seconds, c.title))
                        .collect();
                }
                Err(e) => eprintln!("! probe failed {:?}: {e}", ep.source_path),
            }
        }
    }
}

#[derive(Clone)]
struct EpisodePlan {
    file_id: Uuid,
    label: String,
    source_path: PathBuf,
    storage_root: PathBuf,
    duration_seconds: f64,
    chapters: Vec<(f64, Option<String>)>,
}

impl EpisodePlan {
    fn should_decode(&self, force: bool) -> bool {
        if force {
            return true;
        }
        let kind = MediaKind::TvFile {
            file_id: self.file_id,
        };
        let m = read_episode_manifest(&self.storage_root.to_string_lossy(), &kind);
        match m {
            Some(m) => {
                if m.pipeline_version < PIPELINE_VERSION {
                    return true;
                }
                let present =
                    |slot: &Option<ArtefactInfo>| slot.as_ref().is_some_and(|i| i.present);
                !(present(&m.artefacts.chromaprint)
                    && present(&m.artefacts.vhash)
                    && present(&m.artefacts.silence)
                    && present(&m.artefacts.black_frames))
            }
            None => true,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// single_pass decode + sidecar writes
// ─────────────────────────────────────────────────────────────────────────────

/// SigLIP2 model metadata recorded in the manifest. Mirrors the private
/// constants in `kobbler/api/src/jobs/handlers/media_analyze.rs` so the
/// on-disk embeddings sidecar is byte-for-byte what prod writes.
const EMBED_DIM: usize = 1152;
const EMBED_FPS: f64 = 1.0;
const EMBED_WIDTH: i32 = 768;
const EMBED_MODEL: &str = "google/siglip2-so400m-patch16-naflex";

/// Runs single_pass for one episode, writes all sidecars, and — if a vision
/// URL is configured — sends the embed_src JPEGs to the vision service and
/// writes the embeddings sidecar. The final manifest is written once, at the
/// end, carrying every branch's artefact info (mirrors
/// `media_analyze.rs`'s `write_manifest_on_success: false` flow).
async fn run_and_persist(ep: &EpisodePlan, storage_root: &Path, vision_url: Option<&str>) {
    let started = Instant::now();
    let want_embed_src = vision_url.is_some();
    eprintln!("▶ decode: {}", ep.label);

    // Build the same SinglePassRequest media_analyze uses for video files.
    let duration = ep.duration_seconds;
    let (vhash_windows, embed_windows) = if duration > 0.0 {
        let intro_window = extract_duration_secs(duration) as f64;
        let vhash_credits_start = duration * 0.80;
        let embed_scan_start = (duration - 300.0).max(duration * 0.85);
        (
            vec![(0.0, intro_window), (vhash_credits_start, duration)],
            vec![(embed_scan_start, duration)],
        )
    } else {
        (Vec::new(), Vec::new())
    };

    let mut req = SinglePassRequest::new(ep.source_path.clone());
    req.chromaprint = Some(ChromaprintParams::default());
    req.silence = Some(SilenceParams::default());
    req.black_frames = Some(BlackFramesParams::default());
    req.vhash = Some(VhashParams {
        intermediate_width: 64,
        fps: 3.0,
        windows: vhash_windows,
    });
    if want_embed_src {
        req.embed_src = Some(EmbedSrcParams {
            width: EMBED_WIDTH,
            fps: EMBED_FPS,
            jpeg_quality: 3,
            windows: embed_windows,
        });
    }
    req.loudness = true;

    // single_pass is blocking; run it off the tokio thread so we don't stall
    // other episodes' vision calls.
    let source_label = ep.label.clone();
    let mut result = match tokio::task::spawn_blocking(move || run_single_pass(req, None)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            eprintln!("! single_pass failed {source_label}: {e}");
            return;
        }
        Err(e) => {
            eprintln!("! single_pass task panicked {source_label}: {e}");
            return;
        }
    };

    // Lift the embed_src frames out before persist_sidecars (which consumes
    // the rest of the result). Frames are held in-memory only until we hand
    // them to the vision client below — same pattern as prod.
    let embed_src_frames: Vec<JpegFrame> = result.embed_src.take().unwrap_or_default();

    let cache_dir = episode_cache_dir(
        &storage_root.to_string_lossy(),
        &MediaKind::TvFile {
            file_id: ep.file_id,
        },
    );
    let mut manifest = match persist_sidecars(ep, &cache_dir, storage_root, result) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("! persist failed {}: {e}", ep.label);
            return;
        }
    };

    // Vision pass — only when --vision-url is set. Mirrors
    // `media_analyze::embed_frames_inline` exactly: health check, chunked
    // embed_images, l2_normalise, pack_fp16, write Branch::Embeddings sidecar,
    // set manifest params (dim, count, fps, dtype, model, width,
    // start_time_secs, attempted_at) or record `present: false` with error on
    // failure.
    if let Some(url) = vision_url {
        embed_and_record(&cache_dir, ep, url, embed_src_frames, &mut manifest).await;
    }

    // Final manifest write — single point of truth for what's on disk.
    if let Err(e) = write_manifest_at(&cache_dir, &manifest) {
        eprintln!("! manifest write failed {}: {e}", ep.label);
        return;
    }

    eprintln!(
        "✔ decode: {} in {:.1}s",
        ep.label,
        started.elapsed().as_secs_f64()
    );
}

/// Call the vision service and persist the embeddings sidecar + manifest
/// artefact entry. Matches `media_analyze::embed_frames_inline` field-for-field
/// (same defaults, same error recording) so the on-disk cache is
/// indistinguishable from what a prod `media_analyze` run would produce.
async fn embed_and_record(
    cache_dir: &Path,
    ep: &EpisodePlan,
    vision_url: &str,
    frames: Vec<JpegFrame>,
    manifest: &mut Manifest,
) {
    let start_time_secs = frames.first().map(|f| f.timestamp_secs).unwrap_or(0.0);

    if frames.is_empty() {
        let reason = "embed_src produced no frames";
        eprintln!("  vision: {}: {reason}", ep.label);
        manifest.artefacts.set(
            Branch::Embeddings,
            ArtefactInfo {
                present: false,
                params: embeddings_params_failure(reason),
            },
        );
        return;
    }

    let vision_started = Instant::now();
    eprintln!(
        "  vision: {}: sending {} frames to {vision_url}",
        ep.label,
        frames.len()
    );

    let client = VisionClient::new(vision_url);
    if !client.health().await {
        let reason = "vision service not healthy";
        eprintln!("  ! vision: {}: {reason}", ep.label);
        manifest.artefacts.set(
            Branch::Embeddings,
            ArtefactInfo {
                present: false,
                params: embeddings_params_failure(reason),
            },
        );
        return;
    }

    let jpegs: Vec<Vec<u8>> = frames.into_iter().map(|f| f.jpeg_data).collect();
    let total_frames = jpegs.len();
    let mut all_embeddings = match client.embed_images(&jpegs).await {
        Ok(v) => v,
        Err(e) => {
            let reason = format!("vision embed failed: {e}");
            eprintln!("  ! vision: {}: {reason}", ep.label);
            manifest.artefacts.set(
                Branch::Embeddings,
                ArtefactInfo {
                    present: false,
                    params: embeddings_params_failure(&reason),
                },
            );
            return;
        }
    };

    if all_embeddings.is_empty() {
        let reason = "vision service returned no embeddings";
        eprintln!("  ! vision: {}: {reason}", ep.label);
        manifest.artefacts.set(
            Branch::Embeddings,
            ArtefactInfo {
                present: false,
                params: embeddings_params_failure(reason),
            },
        );
        return;
    }

    let dim = all_embeddings[0].len();
    if dim != EMBED_DIM {
        eprintln!(
            "  ! vision: {}: dim={dim} != expected {EMBED_DIM} — recording actual value",
            ep.label
        );
    }

    l2_normalise(&mut all_embeddings);
    let packed = pack_fp16(&all_embeddings);
    let packed_len = packed.len();

    if let Err(e) = write_sidecar_at(cache_dir, Branch::Embeddings, &packed) {
        let reason = format!("embeddings write failed: {e}");
        eprintln!("  ! vision: {}: {reason}", ep.label);
        manifest.artefacts.set(
            Branch::Embeddings,
            ArtefactInfo {
                present: false,
                params: embeddings_params_failure(&reason),
            },
        );
        return;
    }

    manifest.artefacts.set(
        Branch::Embeddings,
        ArtefactInfo {
            present: true,
            params: embeddings_params_success(all_embeddings.len(), start_time_secs, dim),
        },
    );

    eprintln!(
        "  ✔ vision: {}: {total_frames} frames → {} embeddings (dim={dim}, {packed_len} bytes fp16) in {:.1}s",
        ep.label,
        all_embeddings.len(),
        vision_started.elapsed().as_secs_f64(),
    );
}

fn now_unix_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn embeddings_params_success(
    count: usize,
    start_time_secs: f64,
    dim: usize,
) -> serde_json::Map<String, serde_json::Value> {
    let mut params = serde_json::Map::new();
    params.insert("dim".into(), json!(dim));
    params.insert("count".into(), json!(count));
    params.insert("fps".into(), json!(EMBED_FPS));
    params.insert("dtype".into(), json!("f16"));
    params.insert("model".into(), json!(EMBED_MODEL));
    params.insert("width".into(), json!(EMBED_WIDTH));
    params.insert("start_time_secs".into(), json!(start_time_secs));
    params.insert("attempted_at".into(), json!(now_unix_secs()));
    params
}

fn embeddings_params_failure(reason: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut params = serde_json::Map::new();
    params.insert("attempted_at".into(), json!(now_unix_secs()));
    params.insert("error".into(), json!(reason));
    params
}

fn persist_sidecars(
    ep: &EpisodePlan,
    cache_dir: &Path,
    storage_root: &Path,
    mut result: SinglePassResult,
) -> std::io::Result<Manifest> {
    let file_id = ep.file_id;
    let input_duration = if result.input_duration_secs > 0.0 {
        result.input_duration_secs
    } else {
        ep.duration_seconds
    };
    let trickplay = media_cache::trickplay_dir(&storage_root.to_string_lossy(), file_id);
    let mut manifest = new_manifest_at(&trickplay, input_duration);
    manifest.analyzed_file_id = Some(file_id);
    manifest.analyzed_at = Some(chrono::Utc::now());

    // Chromaprint — Vec<u32> → LE bytes.
    if let Some(cp) = result.chromaprint.take() {
        let mut bytes = Vec::with_capacity(cp.raw.len() * 4);
        for v in &cp.raw {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        write_sidecar_at(cache_dir, Branch::Chromaprint, &bytes)?;
        let mut params = serde_json::Map::new();
        params.insert("len_u32".into(), cp.raw.len().into());
        params.insert(
            "duration_secs".into(),
            serde_json::Value::from(cp.duration_secs),
        );
        manifest.artefacts.set(
            Branch::Chromaprint,
            ArtefactInfo {
                present: true,
                params,
            },
        );
    }

    // Vhash — serialize via the canonical serializer.
    if let Some(descriptors) = result.vhash.take() {
        let frame_count = descriptors.len();
        let frame_descs: Vec<api::skip_detect::primitives::vhash_align::FrameDescriptor> =
            descriptors
                .into_iter()
                .map(|d| {
                    let quantized =
                        api::skip_detect::primitives::vhash_align::quantize_descriptor(&d.data);
                    api::skip_detect::primitives::vhash_align::FrameDescriptor {
                        timestamp: d.timestamp_secs,
                        values: d.data,
                        quantized,
                    }
                })
                .collect();
        let bytes = api::skip_detect::vhash::serialize_descriptors(&frame_descs);
        write_sidecar_at(cache_dir, Branch::Vhash, &bytes)?;
        let mut params = serde_json::Map::new();
        params.insert("intermediate_width".into(), 64.into());
        params.insert("fps".into(), serde_json::Value::from(3.0));
        params.insert("frame_count".into(), frame_count.into());
        manifest.artefacts.set(
            Branch::Vhash,
            ArtefactInfo {
                present: true,
                params,
            },
        );
    }

    // Silence.
    if let Some(intervals) = result.silence.take() {
        let gaps: Vec<SilenceGap> = intervals
            .iter()
            .map(|i| SilenceGap {
                start_secs: i.start_secs,
                end_secs: i.end_secs,
                duration_secs: i.duration_secs,
            })
            .collect();
        let bytes = serde_json::to_vec(&gaps).expect("silence serialise");
        write_sidecar_at(cache_dir, Branch::Silence, &bytes)?;
        let mut params = serde_json::Map::new();
        params.insert("noise_db".into(), serde_json::Value::from(-30.0));
        params.insert("min_duration_secs".into(), serde_json::Value::from(0.5));
        params.insert("count".into(), gaps.len().into());
        manifest.artefacts.set(
            Branch::Silence,
            ArtefactInfo {
                present: true,
                params,
            },
        );
    }

    // Black frames.
    if let Some(intervals) = result.black_frames.take() {
        let gaps: Vec<BlackGap> = intervals
            .iter()
            .map(|i| BlackGap {
                start_secs: i.start_secs,
                end_secs: i.end_secs,
                duration_secs: i.duration_secs,
            })
            .collect();
        let bytes = serde_json::to_vec(&gaps).expect("black_frames serialise");
        write_sidecar_at(cache_dir, Branch::BlackFrames, &bytes)?;
        let mut params = serde_json::Map::new();
        params.insert("min_duration_secs".into(), serde_json::Value::from(0.1));
        params.insert("pixel_threshold".into(), serde_json::Value::from(0.10));
        params.insert("count".into(), gaps.len().into());
        manifest.artefacts.set(
            Branch::BlackFrames,
            ArtefactInfo {
                present: true,
                params,
            },
        );
    }

    // Loudness (optional; detection doesn't use it but write it for parity).
    let loudness = Loudness {
        integrated_lufs: result.loudness,
    };
    if let Ok(bytes) = serde_json::to_vec(&loudness) {
        let _ = write_sidecar_at(cache_dir, Branch::Loudness, &bytes);
        let mut params = serde_json::Map::new();
        params.insert(
            "integrated_lufs".into(),
            serde_json::json!(loudness.integrated_lufs),
        );
        manifest.artefacts.set(
            Branch::Loudness,
            ArtefactInfo {
                present: true,
                params,
            },
        );
    }

    // Manifest is written by the caller after the (optional) vision pass so
    // the embeddings artefact lands in the same file as everything else.
    Ok(manifest)
}

// ─────────────────────────────────────────────────────────────────────────────
// Reporting
// ─────────────────────────────────────────────────────────────────────────────

fn print_episode_report(
    label: &str,
    final_segments: &[DetectedSegment],
    diag: &EpisodeDiagnostics,
) {
    println!("\n─── {label}  ({:.0}s)", diag.duration);
    println!(
        "    cache: chromaprint={} vhash={} silence={} black={} chapters={}",
        diag.chromaprint_samples,
        diag.vhash_frames,
        diag.silence_count,
        diag.black_frames_count,
        diag.chapter_count,
    );

    println!("    passes:");
    print_pass("intro  chromaprint", diag.intro_chromaprint.as_ref());
    print_pass("intro  vhash      ", diag.intro_vhash.as_ref());
    print_pass("credits chromaprint", diag.credits_chromaprint.as_ref());
    print_pass("credits vhash      ", diag.credits_vhash.as_ref());
    print_vision_raw(diag);

    if !diag.candidates_pre_fusion.is_empty() {
        println!("    pre-fusion candidates:");
        for c in &diag.candidates_pre_fusion {
            println!(
                "      {:<18} {:7.1}..{:7.1}s  (dur {:5.1}s)  chroma={}  vhash={}  text={}  bound={}  audio={}",
                c.segment_type.as_str(),
                c.start_seconds,
                c.end_seconds,
                c.end_seconds - c.start_seconds,
                fmt_opt(c.signals.chromaprint_score),
                fmt_opt(c.signals.dhash_score),
                fmt_opt(c.signals.credit_text_score),
                c.signals
                    .boundary_match
                    .as_ref()
                    .map(|b| format!("black={}|silence={}", b.has_black_frame, b.has_silence))
                    .unwrap_or_else(|| "-".into()),
                c.signals
                    .audio_profile
                    .map(|p| format!("{p:?}"))
                    .unwrap_or_else(|| "-".into()),
            );
        }
    }

    println!("    picked:");
    if final_segments.is_empty() {
        println!("      (none)");
    } else {
        for s in final_segments {
            println!(
                "      {:<18} {:7.1}..{:7.1}s  conf={:.2}  veto={:.2}  source={}",
                s.segment_type.as_str(),
                s.start_seconds,
                s.end_seconds,
                s.confidence,
                s.veto_score,
                s.source,
            );
        }
    }

    if !diag.notes.is_empty() {
        println!("    notes:");
        for n in &diag.notes {
            println!("      • {n}");
        }
    }
}

fn print_pass(label: &str, m: Option<&MatchDiag>) {
    match m {
        Some(m) => {
            let mean_sim = m
                .mean_similarity
                .map(|v| format!("  mean_sim={v:.3}"))
                .unwrap_or_default();
            println!(
                "      {label}: {:7.1}..{:7.1}s  score={:.3}  pairs={}{mean_sim}",
                m.start_seconds, m.end_seconds, m.score, m.pair_count
            );
            if !m.pairs.is_empty() {
                let mut starts: Vec<f64> = m.pairs.iter().map(|p| p.lhs_start).collect();
                let mut ends: Vec<f64> = m.pairs.iter().map(|p| p.lhs_end).collect();
                starts.sort_by(|a, b| a.partial_cmp(b).unwrap());
                ends.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let median = |v: &[f64]| {
                    let n = v.len();
                    if n == 0 {
                        0.0
                    } else if n % 2 == 1 {
                        v[n / 2]
                    } else {
                        (v[n / 2 - 1] + v[n / 2]) / 2.0
                    }
                };
                println!(
                    "        per-pair starts: min={:.1} median={:.1} max={:.1}   ends: min={:.1} median={:.1} max={:.1}",
                    starts[0],
                    median(&starts),
                    starts[starts.len() - 1],
                    ends[0],
                    median(&ends),
                    ends[ends.len() - 1],
                );
                // List each pair densely so outliers are visible.
                let line: Vec<String> = m
                    .pairs
                    .iter()
                    .map(|p| {
                        let ms = p
                            .mean_similarity
                            .map(|s| format!("@{s:.2}"))
                            .unwrap_or_default();
                        format!(
                            "ep{}:{:.1}..{:.1}/{:.2}{ms}",
                            p.vs_ep, p.lhs_start, p.lhs_end, p.density
                        )
                    })
                    .collect();
                println!("        pairs: {}", line.join("  "));
            }
        }
        None => println!("      {label}: — no match"),
    }
}

fn fmt_opt(v: Option<f64>) -> String {
    v.map(|x| format!("{x:.2}")).unwrap_or_else(|| "-".into())
}

/// Print the raw credit blocks returned by `classify_credit_text` and compare
/// the roll against the Credits candidate that
/// `apply_vision_credit_text` ended up attaching the score to. When those
/// differ by more than 60s the merge is almost certainly bogus (the 2x01
/// pattern: a weak 2-pair chromaprint false-positive mid-episode gets
/// decorated with vision's real end-credits score).
fn print_vision_raw(diag: &EpisodeDiagnostics) {
    let Some(text) = &diag.vision_credit_text_raw else {
        println!("      vision credit-text  : — not called (no embeddings or vision off)");
        return;
    };
    for &(start, end) in &text.blocks {
        println!(
            "      vision credit-text  : {:7.1}..{:7.1}s  confidence={:.3}",
            start, end, text.confidence
        );
    }
    let ts = text.roll().0;
    let credits_cand = diag
        .candidates_pre_fusion
        .iter()
        .find(|c| matches!(c.segment_type, SegmentType::Credits));
    match credits_cand {
        Some(c) => {
            let delta = (c.start_seconds - ts).abs();
            let marker = if delta > 60.0 {
                " ⚠ LIKELY BOGUS MERGE"
            } else {
                ""
            };
            println!(
                "        applied onto cand   : {:7.1}..{:7.1}s  Δstart={:+.1}s{marker}",
                c.start_seconds,
                c.end_seconds,
                c.start_seconds - ts
            );
        }
        None => println!("        applied onto cand   : (no Credits candidate to attach to)"),
    }
}

fn print_season_summary(
    label: &str,
    results: &[(Uuid, Vec<DetectedSegment>, EpisodeDiagnostics)],
    label_by_id: &HashMap<Uuid, String>,
) {
    println!("\n─── summary: {label}");
    let mut with_intro = 0;
    let mut with_credits = 0;
    let mut with_post = 0;
    let mut cross_intro_chromaprint = 0;
    let mut cross_intro_vhash = 0;
    let mut cross_credits_chromaprint = 0;
    let mut cross_credits_vhash = 0;
    let mut intro_spans: Vec<(f64, f64)> = Vec::new();
    let mut credit_starts: Vec<f64> = Vec::new();
    let mut no_intro: Vec<String> = Vec::new();
    let mut no_credits: Vec<String> = Vec::new();
    for (fid, segs, diag) in results {
        let name = label_by_id.get(fid).cloned().unwrap_or_default();
        let has_intro = segs
            .iter()
            .any(|s| matches!(s.segment_type, SegmentType::Intro));
        let has_credits = segs
            .iter()
            .any(|s| matches!(s.segment_type, SegmentType::Credits));
        let has_post = segs
            .iter()
            .any(|s| matches!(s.segment_type, SegmentType::PostCreditsScene));
        if has_intro {
            with_intro += 1;
        } else {
            no_intro.push(name.clone());
        }
        if has_credits {
            with_credits += 1;
        } else {
            no_credits.push(name.clone());
        }
        if has_post {
            with_post += 1;
        }
        if let Some(seg) = segs
            .iter()
            .find(|s| matches!(s.segment_type, SegmentType::Intro))
        {
            intro_spans.push((seg.start_seconds, seg.end_seconds));
        }
        if let Some(seg) = segs
            .iter()
            .find(|s| matches!(s.segment_type, SegmentType::Credits))
        {
            credit_starts.push(seg.start_seconds);
        }
        if diag.intro_chromaprint.is_some() {
            cross_intro_chromaprint += 1;
        }
        if diag.intro_vhash.is_some() {
            cross_intro_vhash += 1;
        }
        if diag.credits_chromaprint.is_some() {
            cross_credits_chromaprint += 1;
        }
        if diag.credits_vhash.is_some() {
            cross_credits_vhash += 1;
        }
    }

    let n = results.len();
    println!(
        "    pass coverage: intro chroma={}/{}  intro vhash={}/{}  credits chroma={}/{}  credits vhash={}/{}",
        cross_intro_chromaprint,
        n,
        cross_intro_vhash,
        n,
        cross_credits_chromaprint,
        n,
        cross_credits_vhash,
        n
    );
    println!(
        "    picked: intro={}/{}  credits={}/{}  post_credits={}/{}",
        with_intro, n, with_credits, n, with_post, n
    );

    if !intro_spans.is_empty() {
        let starts: Vec<f64> = intro_spans.iter().map(|(s, _)| *s).collect();
        let ends: Vec<f64> = intro_spans.iter().map(|(_, e)| *e).collect();
        println!(
            "    intro start: min={:.1}  max={:.1}  spread={:.1}s   end: min={:.1}  max={:.1}  spread={:.1}s",
            fmin(&starts),
            fmax(&starts),
            fmax(&starts) - fmin(&starts),
            fmin(&ends),
            fmax(&ends),
            fmax(&ends) - fmin(&ends),
        );
    }
    if !credit_starts.is_empty() {
        println!(
            "    credit start: min={:.1}  max={:.1}  spread={:.1}s",
            fmin(&credit_starts),
            fmax(&credit_starts),
            fmax(&credit_starts) - fmin(&credit_starts)
        );
    }

    if !no_intro.is_empty() {
        println!(
            "    NO intro picked for {} ep(s): {}",
            no_intro.len(),
            no_intro.join(", ")
        );
    }
    if !no_credits.is_empty() {
        println!(
            "    NO credits picked for {} ep(s): {}",
            no_credits.len(),
            no_credits.join(", ")
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Prod-parity output — what would have landed in skip_detect_candidates.
// Mirrors `kobbler/api/src/jobs/handlers/skip_detect.rs::candidate_rows_from_diag`
// byte-for-byte so the harness print is the exact persistence shape prod writes.
// ─────────────────────────────────────────────────────────────────────────────

const MATCH_EPSILON: f64 = 0.5;

fn candidate_rows_from_diag(file_id: Uuid, diag: &EpisodeDiagnostics) -> Vec<CandidateRow> {
    let mut rows: Vec<CandidateRow> = Vec::new();
    for seg in &diag.final_segments {
        let matched = diag.candidates_pre_fusion.iter().find(|c| {
            c.segment_type == seg.segment_type
                && (c.start_seconds - seg.start_seconds).abs() < MATCH_EPSILON
        });
        match matched {
            Some(c) => rows.push(candidate_to_row(file_id, c, true, None)),
            None => rows.push(CandidateRow {
                file_id,
                segment_type: seg.segment_type.as_str().to_string(),
                start_seconds: seg.start_seconds,
                end_seconds: seg.end_seconds,
                chromaprint_score: None,
                dhash_score: None,
                boundary_has_black_frame: None,
                boundary_has_silence: None,
                boundary_refined_start: None,
                boundary_refined_end: None,
                audio_profile: None,
                credit_text_score: None,
                chapter_agreement: None,
                file_duration: Some(diag.duration),
                selected: true,
                rejection_reason: None,
            }),
        }
    }
    for rej in &diag.rejected_candidates {
        rows.push(candidate_to_row(
            file_id,
            &rej.candidate,
            false,
            Some(rej.reason.clone()),
        ));
    }
    rows
}

fn candidate_to_row(
    file_id: Uuid,
    c: &Candidate,
    selected: bool,
    rejection_reason: Option<String>,
) -> CandidateRow {
    let (boundary_black, boundary_silence, boundary_start, boundary_end) =
        match &c.signals.boundary_match {
            Some(b) => (
                Some(b.has_black_frame),
                Some(b.has_silence),
                Some(b.refined_start),
                Some(b.refined_end),
            ),
            None => (None, None, None, None),
        };
    CandidateRow {
        file_id,
        segment_type: c.segment_type.as_str().to_string(),
        start_seconds: c.start_seconds,
        end_seconds: c.end_seconds,
        chromaprint_score: c.signals.chromaprint_score,
        dhash_score: c.signals.dhash_score,
        boundary_has_black_frame: boundary_black,
        boundary_has_silence: boundary_silence,
        boundary_refined_start: boundary_start,
        boundary_refined_end: boundary_end,
        audio_profile: c
            .signals
            .audio_profile
            .map(audio_profile_to_str)
            .map(String::from),
        credit_text_score: c.signals.credit_text_score,
        chapter_agreement: c.signals.chapter_agreement,
        file_duration: c.file_duration,
        selected,
        rejection_reason,
    }
}

fn audio_profile_to_str(p: AudioProfile) -> &'static str {
    match p {
        AudioProfile::MusicOnly => "music_only",
        AudioProfile::Dialogue => "dialogue",
        AudioProfile::NarrationLoop => "narration_loop",
        AudioProfile::Mixed => "mixed",
        AudioProfile::Unknown => "unknown",
    }
}

/// Print the exact rows prod would have INSERTed into `skip_detect_candidates`.
/// Every column that would be persisted is shown — selected winners first,
/// then rejected candidates with the reason they were dropped.
fn print_candidate_rows(_label: &str, file_id: Uuid, diag: &EpisodeDiagnostics) {
    let rows = candidate_rows_from_diag(file_id, diag);
    if rows.is_empty() {
        return;
    }
    println!("    skip_detect_candidates rows (what prod would INSERT):");
    println!(
        "      sel segment_type         start..end   chroma  dhash  text   chap   boundary(blk/sil)  audio      rejection"
    );
    for r in &rows {
        let boundary = match (r.boundary_has_black_frame, r.boundary_has_silence) {
            (Some(b), Some(s)) => format!("{}/{}", bool_mark(b), bool_mark(s)),
            _ => "-".into(),
        };
        println!(
            "      [{}] {:<18} {:7.1}..{:7.1} {:>6} {:>6} {:>6} {:>6}  {:<16} {:<10} {}",
            if r.selected { "✓" } else { " " },
            r.segment_type,
            r.start_seconds,
            r.end_seconds,
            fmt_opt_f(r.chromaprint_score),
            fmt_opt_f(r.dhash_score),
            fmt_opt_f(r.credit_text_score),
            fmt_opt_f(r.chapter_agreement),
            boundary,
            r.audio_profile.clone().unwrap_or_else(|| "-".into()),
            r.rejection_reason.clone().unwrap_or_default(),
        );
    }
}

fn fmt_opt_f(v: Option<f64>) -> String {
    v.map(|x| format!("{x:.2}")).unwrap_or_else(|| "-".into())
}

fn bool_mark(b: bool) -> &'static str {
    if b { "y" } else { "n" }
}

// ─────────────────────────────────────────────────────────────────────────────
// Vision coverage report — season-wide answer to "did vision actually contribute?"
// ─────────────────────────────────────────────────────────────────────────────

fn print_vision_coverage(
    label: &str,
    results: &[(Uuid, Vec<DetectedSegment>, EpisodeDiagnostics)],
    _label_by_id: &HashMap<Uuid, String>,
) {
    let mut with_text_score = 0usize;
    let mut with_visual_score = 0usize;
    let mut credits_with_text = 0usize;
    let mut credits_total = 0usize;
    let mut total_candidates = 0usize;
    for (_fid, _segs, diag) in results {
        for c in &diag.candidates_pre_fusion {
            total_candidates += 1;
            if c.signals.credit_text_score.is_some() {
                with_text_score += 1;
            }
            if c.signals.visual_score.is_some() {
                with_visual_score += 1;
            }
            if matches!(c.segment_type, SegmentType::Credits) {
                credits_total += 1;
                if c.signals.credit_text_score.is_some() {
                    credits_with_text += 1;
                }
            }
        }
    }
    println!("\n─── vision coverage: {label}");
    println!(
        "    candidates with credit_text_score: {}/{}",
        with_text_score, total_candidates
    );
    println!(
        "    candidates with visual_score      : {}/{}",
        with_visual_score, total_candidates
    );
    println!(
        "    credits candidates with text score: {}/{}",
        credits_with_text, credits_total
    );
    if credits_total > 0 && credits_with_text == 0 {
        println!(
            "    ⚠ zero credits candidates picked up vision text — either embeddings.bin is missing or classify_from_embeddings returned nothing"
        );
    }
}

fn fmin(xs: &[f64]) -> f64 {
    xs.iter().copied().fold(f64::INFINITY, f64::min)
}
fn fmax(xs: &[f64]) -> f64 {
    xs.iter().copied().fold(f64::NEG_INFINITY, f64::max)
}
