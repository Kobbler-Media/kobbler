use std::path::Path;
use std::time::Duration;

use crate::AppState;
use crate::events::LibraryEvent;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::livetv::library;
use crate::livetv::live_session;
use crate::livetv::recorder::{self, CaptureError};
use crate::livetv::trickplay;
use crate::livetv::tuner_lease::{HolderKind, LeaseRequest, TunerLeases};
use crate::media_cache::analyze::{AnalyzeInputs, run_analysis};
use crate::repositories::{livetv_channels, livetv_recordings, livetv_tuners};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Deserialize, Serialize)]
pub struct LivetvRecordPayload {
    pub recording_id: Uuid,
    #[serde(default)]
    pub resume: bool,
}

/// Defensive cap on the pre-start wait so a row with a start time far in the
/// future cannot wedge a worker slot.
const MAX_START_WAIT: Duration = Duration::from_secs(300);
/// Full re-opens of the device. ffmpeg already reconnects internally, so this
/// budget only covers failures that survive that.
const MAX_SOURCE_ATTEMPTS: usize = 5;
const SOURCE_RETRY_DELAY: Duration = Duration::from_secs(3);
const HEARTBEAT: Duration = Duration::from_secs(30);

pub struct LivetvRecordHandler {
    state: AppState,
}

impl LivetvRecordHandler {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

fn start_wait(now: OffsetDateTime, start: OffsetDateTime, padding_before: i32) -> Duration {
    let capture_start = start - time::Duration::seconds(i64::from(padding_before));
    let remaining = capture_start - now;
    if remaining.is_positive() {
        remaining.unsigned_abs().min(MAX_START_WAIT)
    } else {
        Duration::ZERO
    }
}

async fn post_process(
    state: &AppState,
    recording_id: Uuid,
    dir: &Path,
    cancel: &CancellationToken,
) {
    // The parts stitched into one local playlist: ffmpeg reads an HLS playlist as
    // an input, so the existing analysis seam needs no parallel path.
    let Some((playlist, status)) = recorder::read_playlist(dir, true, "").await else {
        return;
    };
    let manifest = dir.join("recording.m3u8");
    if let Err(e) = tokio::fs::write(&manifest, playlist).await {
        warn!(error = %e, "writing the recording manifest failed");
        return;
    }

    let size = recorder::dir_size(dir).await as i64;
    if let Err(e) = livetv_recordings::update_media_metadata(
        &state.pool,
        recording_id,
        Some(status.duration_seconds),
        Some(size),
        None,
        None,
    )
    .await
    {
        warn!(error = %e, "saving live TV recording metadata failed");
    }

    // A recording joins the library on completion, never while partial. The
    // match decides whether there is an episode cache to write into at all, so
    // it has to happen before the analysis pass, not after it.
    let row = match livetv_recordings::get_by_id(&state.pool, recording_id).await {
        Ok(Some(row)) => row,
        _ => return,
    };
    let imported = match library::import(&state.pool, &row, &manifest).await {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, recording_id = %recording_id, "matching a recording to the library failed");
            library::Imported::Unmatched
        }
    };
    // Decision 8: skip detect is season-scoped and gates on siblings having a
    // completed media_cache, so an unmatched recording has nothing to read and
    // gets no episode cache written for it.
    let (episode_cache_dir, analyzed_file_id) = match imported {
        library::Imported::Episode { file_id, .. } => (
            Some(crate::media_cache::episode_cache::episode_cache_dir(
                &state.config.storage_root,
                &crate::media_cache::episode_cache::MediaKind::TvFile { file_id },
            )),
            Some(file_id),
        ),
        // `MediaKind::Movie` is keyed on `movies.id`, which the owned copy also
        // writes to — a recording must not overwrite the owned copy's cache.
        library::Imported::Movie { .. } => (None, None),
        library::Imported::Unmatched => (None, None),
    };

    let cache_dir = dir.join("media_cache");
    let trickplay_dir = cache_dir.join("trickplay");
    // The in-progress pass sized its sprites to a growing file, so its tail can
    // outrun the final pass and leave sprites the finished VTT never names.
    let _ = tokio::fs::remove_dir_all(&trickplay_dir).await;
    let inputs = AnalyzeInputs {
        input_path: manifest,
        cache_dir,
        trickplay_dir,
        is_video: true,
        known_duration: Some(status.duration_seconds),
        want_embed_src: false,
        media_type: "recording".to_string(),
        write_manifest_on_success: true,
        only_embed_src: false,
        episode_cache_dir,
        analyzed_file_id,
    };
    match run_analysis(inputs, cancel).await {
        Ok(outcome) => info!(
            recording_id = %recording_id,
            branches = outcome.branch_results.len(),
            imported = ?imported,
            "live TV recording analysis complete"
        ),
        Err(e) => {
            warn!(error = %e, recording_id = %recording_id, "live TV recording analysis failed")
        }
    }

    if let Err(e) = state
        .nats
        .publisher()
        .enqueue_job(
            "kobbler",
            "livetv_ad_detect",
            json!({ "recording_id": recording_id }),
        )
        .await
    {
        warn!(error = %e, recording_id = %recording_id, "enqueueing ad detection failed");
    }
}

#[async_trait]
impl JobHandler for LivetvRecordHandler {
    fn job_type(&self) -> &'static str {
        "livetv_record"
    }

    fn max_concurrent(&self) -> usize {
        4
    }

    /// Unlike `iptv_record`'s `Some(1)`: a redelivery here appends a part to a
    /// recording that is still inside its window, so a worker that dies mid
    /// capture loses seconds instead of the rest of the programme.
    fn max_deliver(&self) -> Option<i64> {
        Some(5)
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: LivetvRecordPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {e}")))?;
        let recording_id = payload.recording_id;

        let row = livetv_recordings::get_by_id(&self.state.pool, recording_id)
            .await?
            .ok_or_else(|| JobError::ExecutionFailed("Recording not found".to_string()))?;
        if !matches!(row.status.as_str(), "scheduled" | "recording") {
            return Ok(json!({ "recording_id": recording_id, "skipped": row.status }));
        }

        if !payload.resume {
            let delay = start_wait(
                OffsetDateTime::now_utc(),
                row.scheduled_start,
                row.padding_before_secs,
            );
            if !delay.is_zero() {
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = cancel.cancelled() => {}
                }
                nats_jobs::check_cancelled(&cancel)?;
            }
        }

        // The double-enqueue from the lookahead window is safe: exactly one
        // worker wins the claim, and a resume only wins once the previous
        // worker's heartbeat has gone stale.
        let claimed = livetv_recordings::try_claim_for_recording(&self.state.pool, recording_id)
            .await?
            || livetv_recordings::try_claim_for_resume(
                &self.state.pool,
                recording_id,
                recorder::RESUME_STALE_SECS,
            )
            .await?;
        if !claimed {
            return Ok(json!({ "recording_id": recording_id, "skipped": "already_claimed" }));
        }

        // Everything from here to `mark_started` owns a claimed row, so a failure
        // has to hand it back: a permanent one fails it, a busy tuner returns it to
        // `scheduled` for the next scheduler tick rather than stranding it
        // mid-status until the next boot.
        let hardware = async {
            let channel = livetv_channels::get_by_id(&self.state.pool, row.channel_id)
                .await?
                .ok_or_else(|| JobError::ExecutionFailed("Channel not found".to_string()))?;
            let tuner = livetv_tuners::get_by_id(&self.state.pool, channel.tuner_id)
                .await?
                .ok_or_else(|| JobError::ExecutionFailed("Tuner not found".to_string()))?;
            let lcn = channel.lcn.ok_or_else(|| {
                JobError::ExecutionFailed("Channel has no LCN to tune".to_string())
            })?;
            Ok::<_, JobError>((tuner, lcn))
        }
        .await;
        let (tuner, lcn) = match hardware {
            Ok(v) => v,
            Err(e) => {
                livetv_recordings::finish(
                    &self.state.pool,
                    recording_id,
                    "failed",
                    Some(&e.to_string()),
                )
                .await?;
                return Err(e);
            }
        };

        // A recording outranks a viewer, and the device gives each HTTP client
        // its own tuner, so this never shares a viewer's slot.
        let lease = self
            .state
            .livetv
            .leases
            .acquire(LeaseRequest {
                tuner_id: tuner.id,
                channel_id: row.channel_id,
                share_key: format!("r:{recording_id}"),
                capacity: tuner.tuner_count.max(1) as u32,
                holder_kind: HolderKind::Recording,
                session_id: None,
                recording_id: Some(recording_id),
                user_id: None,
            })
            .map_err(|e| JobError::Retryable(format!("no tuner available: {e}")));
        let lease = match lease {
            Ok(l) => l,
            Err(e) => {
                livetv_recordings::set_status(
                    &self.state.pool,
                    recording_id,
                    "scheduled",
                    Some(&e.to_string()),
                )
                .await?;
                return Err(e);
            }
        };
        if lease.fresh {
            TunerLeases::persist_new(&self.state.pool, &lease.info).await?;
        }
        let _ = self
            .state
            .library_events
            .send(LibraryEvent::LivetvLeaseChanged {
                channel_id: row.channel_id,
                tuner_id: tuner.id,
                in_use: self.state.livetv.leases.in_use(tuner.id),
                capacity: tuner.tuner_count.max(1) as u32,
            });

        let runtime = self.state.runtime_config.get().await;
        let root = recorder::recording_root(&self.state.pool, &self.state.config, &runtime).await?;
        let dir = recorder::recording_dir(&root, recording_id);
        livetv_recordings::mark_started(
            &self.state.pool,
            recording_id,
            &dir.to_string_lossy(),
            lease.info.tuner_index,
        )
        .await?;
        let _ = self
            .state
            .library_events
            .send(LibraryEvent::LivetvRecordingUpdated {
                recording_id,
                status: "recording".to_string(),
            });

        // The heartbeat is also what makes Stop work: the capture is a single long
        // ffmpeg run, so the row is the only channel a stop request has.
        let capture = cancel.child_token();
        let beat = {
            let pool = self.state.pool.clone();
            let cancel = cancel.clone();
            let capture = capture.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(HEARTBEAT);
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = tick.tick() => {
                            let _ = livetv_recordings::heartbeat(&pool, recording_id).await;
                            match livetv_recordings::get_by_id(&pool, recording_id).await {
                                Ok(Some(row)) if row.status == "recording" => {}
                                Ok(_) => {
                                    capture.cancel();
                                    return;
                                }
                                Err(e) => warn!(error = %e, "reading recording status failed"),
                            }
                        }
                    }
                }
            })
        };

        // Decision 7: sprites and a VTT appear while the back half is still
        // being captured, so scrubbing works from the first minute.
        let trickplay_task = {
            let dir = dir.clone();
            let trickplay_dir = dir.join("media_cache").join("trickplay");
            let cancel = capture.clone();
            tokio::spawn(trickplay::run(dir, trickplay_dir, cancel))
        };

        let stream_url = live_session::tuner_stream_url(&tuner.base_url, lcn);
        let end_at = row.scheduled_end + time::Duration::seconds(i64::from(row.padding_after_secs));
        let mut attempts = 0usize;
        let mut parts = recorder::part_count(&dir).await;
        let mut segments = 0usize;
        let mut last_error: Option<String> = None;

        loop {
            let now = OffsetDateTime::now_utc();
            if now >= end_at || capture.is_cancelled() {
                break;
            }
            let remaining = (end_at - now).unsigned_abs();
            let part_dir = recorder::next_part_dir(&dir).await;
            info!(recording_id = %recording_id, job_id = %job_id, part = %part_dir.display(), "live TV capture part starting");

            match recorder::capture_part(&stream_url, &part_dir, remaining, capture.clone()).await {
                Ok(outcome) => {
                    // Segments, not packets: a pure remux never touches the video
                    // encoder, so `video_packets_written` is always zero here.
                    if outcome.segments == 0 {
                        attempts += 1;
                        let _ = tokio::fs::remove_dir_all(&part_dir).await;
                    } else {
                        parts += 1;
                        segments += outcome.segments;
                        // A part that captured something clears the budget: five
                        // reconnects over three hours is a working recording.
                        attempts = 0;
                    }
                }
                Err(CaptureError::Disk(msg)) => {
                    beat.abort();
                    drop(lease);
                    livetv_recordings::finish(&self.state.pool, recording_id, "failed", Some(&msg))
                        .await?;
                    let _ = self
                        .state
                        .library_events
                        .send(LibraryEvent::LivetvRecordingUpdated {
                            recording_id,
                            status: "failed".to_string(),
                        });
                    // The one failure that must not retry: another attempt writes
                    // nothing either.
                    return Err(JobError::ExecutionFailed(msg));
                }
                Err(CaptureError::Source(msg)) => {
                    warn!(recording_id = %recording_id, error = %msg, "live TV capture part failed");
                    last_error = Some(msg);
                    attempts += 1;
                    let _ = tokio::fs::remove_dir_all(&part_dir).await;
                }
            }

            if attempts >= MAX_SOURCE_ATTEMPTS {
                break;
            }
            // Unconditional: a source that ends instantly must not spin the loop.
            if OffsetDateTime::now_utc() < end_at {
                tokio::select! {
                    _ = tokio::time::sleep(SOURCE_RETRY_DELAY) => {}
                    _ = capture.cancelled() => break,
                }
            }
        }

        beat.abort();
        trickplay_task.abort();
        drop(lease);
        let _ = self
            .state
            .library_events
            .send(LibraryEvent::LivetvLeaseChanged {
                channel_id: row.channel_id,
                tuner_id: tuner.id,
                in_use: self.state.livetv.leases.in_use(tuner.id),
                capacity: tuner.tuner_count.max(1) as u32,
            });

        // Partial is better than nothing: any part on disk is a completed
        // recording, and only a capture with no bytes at all is a failure.
        let status = if parts > 0 { "completed" } else { "failed" };
        livetv_recordings::finish(
            &self.state.pool,
            recording_id,
            status,
            last_error.as_deref().filter(|_| parts == 0),
        )
        .await?;
        let _ = self
            .state
            .library_events
            .send(LibraryEvent::LivetvRecordingUpdated {
                recording_id,
                status: status.to_string(),
            });

        if parts > 0 {
            post_process(&self.state, recording_id, &dir, &cancel).await;
        }

        Ok(json!({
            "recording_id": recording_id,
            "status": status,
            "parts": parts,
            "segments": segments,
            "attempts": attempts,
        }))
    }
}
