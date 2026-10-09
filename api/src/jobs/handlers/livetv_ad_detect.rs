//! Ad detection over a completed recording (Decision 9).
//!
//! Marks only, never cuts. Two independent signals must agree before anything
//! is written, because a false positive is worth roughly 5-10 false negatives.

use std::path::{Path, PathBuf};

use crate::AppState;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::livetv::ad_detect::{self, bank, blocks, teletext, vision_signal};
use crate::livetv::recorder;
use crate::repositories::{livetv_ad_breaks, livetv_ad_fingerprints, livetv_recordings};
use crate::skip_detect::primitives::fingerprint_match::Fingerprint;
use async_trait::async_trait;
use kob_ffmpeg::single_pass::{
    BlackFramesParams, ChromaprintParams, SilenceParams, SinglePassRequest, TrickplayParams,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// The `pix_th` the Seven capture needed: comskip's 0.10 finds almost nothing on
/// Australian material, which fades to *near* black rather than black.
const BLACK_PIXEL_THRESHOLD: f64 = 0.15;
/// The other half of that departure: comskip's 0.98 `pic_th` found 5 events in
/// 11 minutes, indistinguishable from a detector that does not work here.
const BLACK_PICTURE_THRESHOLD: f64 = 0.85;
const BLACK_MIN_DURATION: f64 = 0.04;
/// Below the 0.9 s discriminator so `blocks` can apply it itself; the p50 of
/// 0.143 s is just speech pausing and carries no information.
const SILENCE_MIN_DURATION: f64 = 0.1;
const SILENCE_NOISE_DB: f64 = -30.0;
const BANK_LIMIT: i64 = 2000;
/// One trickplay frame every 5 s, which is also the zero-shot vision grid.
const FRAME_FPS: f64 = 0.2;
/// A whole recording of frames would not fit the vision service in one call.
const VISION_BATCH: usize = 64;

#[derive(Debug, Deserialize)]
pub struct LivetvAdDetectPayload {
    pub recording_id: Uuid,
    #[serde(default)]
    pub reprint_break_id: Option<Uuid>,
    #[serde(default = "default_true")]
    pub detect: bool,
}

fn default_true() -> bool {
    true
}

pub struct LivetvAdDetectHandler {
    state: AppState,
}

impl LivetvAdDetectHandler {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

/// Teletext is metered per capture part while the recording is being written,
/// so the sidecars have to be laid end to end on the recording's own timeline.
async fn teletext_counts(dir: &Path) -> Vec<teletext::UnitCount> {
    let mut parts = recorder::part_dirs(dir).await;
    if parts.is_empty() {
        parts.push(dir.to_path_buf());
    }
    let mut out: Vec<teletext::UnitCount> = Vec::new();
    let mut offset = 0u32;
    for part in parts {
        let Ok(raw) = tokio::fs::read_to_string(part.join("stream.m3u8")).await else {
            continue;
        };
        let (entries, _) = recorder::parse_part(&raw);
        let part_secs: f64 = entries.iter().map(|(s, _)| *s).sum();
        if let Some(counts) = teletext::read_sidecar(&part.join(recorder::TELETEXT_SIDECAR)).await {
            out.extend(counts.into_iter().map(|c| teletext::UnitCount {
                second: c.second + offset,
                units: c.units,
            }));
        }
        offset += part_secs.ceil() as u32;
    }
    out
}

struct PassResult {
    black: Vec<(f64, f64)>,
    silence: Vec<(f64, f64)>,
    chromaprint: Option<Fingerprint>,
    frames: Vec<(f64, Vec<u8>)>,
}

fn run_pass(manifest: PathBuf, want_frames: bool) -> Result<PassResult, String> {
    let mut req = SinglePassRequest::new(&manifest);
    req.black_frames = Some(BlackFramesParams {
        min_duration_secs: BLACK_MIN_DURATION,
        pixel_threshold: BLACK_PIXEL_THRESHOLD,
        picture_threshold: BLACK_PICTURE_THRESHOLD,
    });
    req.silence = Some(SilenceParams {
        noise_db: SILENCE_NOISE_DB,
        min_duration_secs: SILENCE_MIN_DURATION,
    });
    req.chromaprint = Some(ChromaprintParams {
        offset_secs: None,
        length_secs: None,
    });
    if want_frames {
        req.trickplay = Some(TrickplayParams {
            width: 320,
            fps: FRAME_FPS,
            jpeg_quality: 5,
        });
    }
    let result = kob_ffmpeg::single_pass::run_single_pass(req, None).map_err(|e| e.to_string())?;
    Ok(PassResult {
        black: result
            .black_frames
            .unwrap_or_default()
            .into_iter()
            .map(|b| (b.start_secs, b.end_secs))
            .collect(),
        silence: result
            .silence
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.start_secs, s.end_secs))
            .collect(),
        chromaprint: result.chromaprint.map(|c| Fingerprint {
            data: c.raw,
            duration_seconds: c.duration_secs,
        }),
        frames: result
            .trickplay
            .unwrap_or_default()
            .into_iter()
            .map(|f| (f.timestamp_secs, f.jpeg_data))
            .collect(),
    })
}

fn slice_fingerprint(fp: &Fingerprint, start: f64, end: f64) -> Option<(Vec<u32>, f64)> {
    if fp.data.is_empty() || fp.duration_seconds <= 0.0 {
        return None;
    }
    let per_sample = fp.duration_seconds / fp.data.len() as f64;
    let from = (start / per_sample).floor().max(0.0) as usize;
    let to = ((end / per_sample).ceil() as usize).min(fp.data.len());
    if to <= from {
        return None;
    }
    Some((fp.data[from..to].to_vec(), (to - from) as f64 * per_sample))
}

#[async_trait]
impl JobHandler for LivetvAdDetectHandler {
    fn job_type(&self) -> &'static str {
        "livetv_ad_detect"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: LivetvAdDetectPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {e}")))?;
        let row = livetv_recordings::get_by_id(&self.state.pool, payload.recording_id)
            .await?
            .ok_or_else(|| JobError::ExecutionFailed("Recording not found".to_string()))?;
        let Some(dir) = row.output_dir.clone() else {
            return Ok(json!({ "skipped": "nothing captured" }));
        };
        let dir = PathBuf::from(dir);
        let manifest = dir.join("recording.m3u8");
        if !manifest.exists() {
            return Ok(json!({ "skipped": "no manifest" }));
        }
        nats_jobs::check_cancelled(&cancel)?;

        let counts = teletext_counts(&dir).await;
        let teletext_runs = teletext::ad_runs(&counts, &teletext::TeletextConfig::default());

        let pass_manifest = manifest.clone();
        let want_frames = payload.detect && teletext_runs.is_empty();
        let pass = tokio::task::spawn_blocking(move || run_pass(pass_manifest, want_frames))
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("analysis task panicked: {e}")))?
            .map_err(JobError::ExecutionFailed)?;
        nats_jobs::check_cancelled(&cancel)?;

        let block_candidates =
            blocks::candidates(&pass.black, &pass.silence, &blocks::BlockConfig::default());

        let bank_rows = livetv_ad_fingerprints::list_bank(&self.state.pool, BANK_LIMIT).await?;
        let entries: Vec<bank::BankEntry> = bank_rows
            .iter()
            .map(|r| bank::BankEntry {
                id: r.id,
                fingerprint: r.fingerprint.iter().map(|v| *v as u32).collect(),
                duration_secs: r.duration_secs,
                origin: r.origin.clone(),
            })
            .collect();
        let hits = match pass.chromaprint.as_ref() {
            Some(fp) => bank::match_bank(fp, &entries, &bank::BankConfig::default()),
            None => Vec::new(),
        };
        for hit in &hits {
            let _ = livetv_ad_fingerprints::mark_seen(&self.state.pool, hit.entry_id).await;
        }
        let bank_spans = bank::merge_hits_into_breaks(&hits, 5.0);

        // Zero-shot vision is the only second signal with no cold-start problem,
        // so it only runs where teletext gave us nothing to agree with.
        let mut vision_runs = Vec::new();
        if payload.detect && teletext_runs.is_empty() && !pass.frames.is_empty() {
            let runtime = self.state.runtime_config.get().await;
            if let Some(url) = runtime.vision_service_url.clone() {
                let client = crate::skip_detect::vision::VisionClient::new(&url);
                if client.health().await {
                    let _guard = self.state.gpu_lock.acquire_for_vision().await;
                    let mut scores: Vec<(f64, f64)> = Vec::new();
                    for chunk in pass.frames.chunks(VISION_BATCH) {
                        match vision_signal::classify_frames(&client, chunk).await {
                            Ok(mut s) => scores.append(&mut s),
                            Err(e) => {
                                warn!(error = %e, "zero-shot ad classification failed");
                                break;
                            }
                        }
                    }
                    vision_runs = vision_signal::runs_from_scores(
                        &scores,
                        &vision_signal::VisionConfig::default(),
                    );
                }
            }
        }

        let signals = ad_detect::Signals {
            teletext: teletext_runs,
            blocks: block_candidates
                .iter()
                .map(|c| (c.start_seconds, c.end_seconds, c.score))
                .collect(),
            bank: bank_spans,
            vision: vision_runs,
        };
        // A correction only ever wants the re-fingerprint; re-running the
        // detector over a boundary a person just fixed would fight them.
        let breaks = if payload.detect {
            ad_detect::fuse(&signals, &ad_detect::FuseConfig::default())
        } else {
            Vec::new()
        };

        // A user's corrected boundary is the ground truth this pass is trying to
        // reproduce, so it survives a re-detection.
        if payload.detect {
            livetv_ad_breaks::delete_uncorrected_for_recording(&self.state.pool, row.id).await?;
        }
        let mut written = 0usize;
        for b in &breaks {
            livetv_ad_breaks::create(
                &self.state.pool,
                livetv_ad_breaks::CreateAdBreak {
                    recording_id: row.id,
                    start_seconds: b.start_seconds,
                    end_seconds: b.end_seconds,
                    method: b.method.to_string(),
                    confidence: b.confidence,
                    ads_known: b.ads_known as i32,
                    ads_total: b.ads_total as i32,
                },
            )
            .await?;
            written += 1;

            // Seed the rolling bank so the next airing of this pod matches on
            // audio alone, which is frame-accurate where the signals are not.
            if let Some(fp) = pass.chromaprint.as_ref()
                && b.ads_known == 0
                && let Some((data, secs)) =
                    slice_fingerprint(fp, b.start_seconds, b.start_seconds + 30.0)
            {
                let _ = livetv_ad_fingerprints::create(
                    &self.state.pool,
                    livetv_ad_fingerprints::CreateAdFingerprint {
                        fingerprint: data.into_iter().map(|v| v as i32).collect(),
                        duration_secs: secs,
                        label: Some(row.title.clone()),
                        origin: "auto".to_string(),
                    },
                )
                .await;
            }
        }

        // The correction loop is the compounding asset: a corrected span is
        // stored as `confirmed`, and one fix then lands every future airing.
        let mut confirmed = 0usize;
        if let Some(break_id) = payload.reprint_break_id
            && let Some(corrected) = livetv_ad_breaks::get_by_id(&self.state.pool, break_id).await?
            && let Some(fp) = pass.chromaprint.as_ref()
            && let Some((data, secs)) =
                slice_fingerprint(fp, corrected.start_seconds, corrected.end_seconds)
        {
            livetv_ad_fingerprints::create(
                &self.state.pool,
                livetv_ad_fingerprints::CreateAdFingerprint {
                    fingerprint: data.into_iter().map(|v| v as i32).collect(),
                    duration_secs: secs,
                    label: Some(row.title.clone()),
                    origin: "confirmed".to_string(),
                },
            )
            .await?;
            confirmed += 1;
        }

        // The player is already subscribed to this event for status changes, and
        // new marks are the other thing it has to re-fetch for.
        let _ =
            self.state
                .library_events
                .send(crate::events::LibraryEvent::LivetvRecordingUpdated {
                    recording_id: row.id,
                    status: row.status.clone(),
                });

        info!(
            recording_id = %row.id,
            breaks = written,
            teletext_seconds = counts.len(),
            "live TV ad detection complete"
        );
        Ok(json!({
            "recording_id": row.id,
            "breaks": written,
            "bank_hits": hits.len(),
            "confirmed_prints": confirmed,
            "teletext_seconds": counts.len(),
        }))
    }
}
