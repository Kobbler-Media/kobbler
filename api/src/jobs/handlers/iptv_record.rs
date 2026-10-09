//! IPTV record job handler.
//!
//! Runs ffmpeg to capture a single recording. Resolves the channel's sources,
//! tries them in priority/health order with fallback, and updates status on
//! completion/failure. Supports segmented recording with automatic source
//! failover — if a stream dies mid-recording, the captured segment is saved
//! and recording continues on the next available source.

use std::path::{Path, PathBuf};

use crate::iptv::health;
use crate::iptv::live_session::is_connection_limit_error;
use crate::iptv::provider_tracker;
use crate::iptv::recording::{RecordingConfig, start_recording};
use crate::iptv::xtream::XtreamClient;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::media_cache::analyze::{AnalyzeInputs, run_analysis};
use crate::repositories::{iptv_providers, iptv_recordings, iptv_sources};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// Payload for iptv_record jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct IptvRecordPayload {
    pub recording_id: Uuid,
}

/// Handler that runs ffmpeg for a single IPTV recording.
pub struct IptvRecordHandler {
    pool: PgPool,
    storage_root: String,
}

impl IptvRecordHandler {
    pub fn new(pool: PgPool, storage_root: String) -> Self {
        Self { pool, storage_root }
    }

    /// Probe a completed recording and run single-pass media analysis. Writes
    /// the full sidecar cache (trickplay, vhash, chromaprint, silence,
    /// loudness, black_frames) into `<recording_dir>/media_cache/` so the
    /// player scrubber has thumbnails and a future skip-detect pass can read
    /// cached signals. CPU-only — no embeddings, same policy as debrid.
    async fn post_process_recording(
        &self,
        recording_id: Uuid,
        file_path: &Path,
        cancel: &CancellationToken,
    ) {
        let duration_seconds = match crate::ffmpeg::probe::probe(file_path).await {
            Ok(probe) => {
                let duration = probe.format.duration_seconds;
                let video_codec = probe.video_streams.first().map(|v| v.codec.as_str());
                let container = Some(probe.format.format_name.as_str());

                if let Err(e) = iptv_recordings::update_media_metadata(
                    &self.pool,
                    recording_id,
                    duration,
                    video_codec,
                    container,
                )
                .await
                {
                    warn!(error = %e, "Failed to save recording media metadata");
                }
                duration
            }
            Err(e) => {
                warn!(error = %e, "Failed to probe completed recording");
                None
            }
        };

        // Single-pass media analysis — sibling media_cache/ dir next to file.
        let media_cache_dir = file_path.parent().unwrap_or(file_path).join("media_cache");
        let trickplay_dir = media_cache_dir.join("trickplay");
        let inputs = AnalyzeInputs {
            input_path: file_path.to_path_buf(),
            cache_dir: media_cache_dir,
            trickplay_dir,
            is_video: true,
            known_duration: duration_seconds,
            want_embed_src: false,
            media_type: "recording".to_string(),
            write_manifest_on_success: true,
            only_embed_src: false,
            // Recordings of live TV channels don't map to a canonical
            // tv_episode/movie row — they're channel-bound, not content-bound.
            // No episode_cache copy; trickplay + audio_peaks land per-file
            // and that's all the recording needs.
            episode_cache_dir: None,
            analyzed_file_id: None,
        };
        match run_analysis(inputs, cancel).await {
            Ok(outcome) => {
                info!(
                    recording_id = %recording_id,
                    branches = outcome.branch_results.len(),
                    errors = outcome.per_branch_errors.len(),
                    duration_secs = outcome.input_duration_secs,
                    "IPTV recording media analysis complete"
                );
            }
            Err(e) => {
                warn!(
                    error = %e,
                    recording_id = %recording_id,
                    "Failed to run media analysis for IPTV recording"
                );
            }
        }
    }

    /// Wraps a successful recording result with post-processing (probe + thumbnails).
    async fn finish_and_post_process(
        &self,
        recording_id: Uuid,
        final_path: &Path,
        result: JobResult<Value>,
        cancel: &CancellationToken,
    ) -> JobResult<Value> {
        if result.is_ok() {
            self.post_process_recording(recording_id, final_path, cancel)
                .await;
        }
        result
    }
}

const MAX_ATTEMPTS: usize = 3;

/// Defensive cap on how long the handler will wait for capture-start, so a bad
/// row (start_time far in the future) can't wedge a worker slot.
const MAX_RECORDING_WAIT: std::time::Duration = std::time::Duration::from_secs(300);

/// Wall-clock delay until a recording's capture should begin
/// (`start_time - pre_padding`). Zero if that moment has already passed.
fn capture_delay(
    now: OffsetDateTime,
    start_time: OffsetDateTime,
    pre_padding_secs: i32,
) -> std::time::Duration {
    let capture_start = start_time - time::Duration::seconds(i64::from(pre_padding_secs));
    let remaining = capture_start - now;
    if remaining.is_positive() {
        remaining.unsigned_abs().min(MAX_RECORDING_WAIT)
    } else {
        std::time::Duration::ZERO
    }
}

#[async_trait]
impl JobHandler for IptvRecordHandler {
    fn job_type(&self) -> &'static str {
        "iptv_record"
    }

    fn max_concurrent(&self) -> usize {
        5
    }

    fn max_deliver(&self) -> Option<i64> {
        Some(1)
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: IptvRecordPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {e}")))?;

        let recording_id = payload.recording_id;
        info!(job_id = %job_id, recording_id = %recording_id, "Starting IPTV recording");

        // Fetch the recording
        let recording = iptv_recordings::get_by_id(&self.pool, recording_id)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to fetch recording: {e}")))?
            .ok_or_else(|| JobError::ExecutionFailed("Recording not found".to_string()))?;

        // Wait for the precise capture-start. The scheduler enqueues up to
        // RECORDING_LOOKAHEAD early so we hit the mark instead of starting up
        // to one poll-interval late; cancellation (shutdown / manual cancel)
        // aborts the wait, leaving the recording `scheduled` for re-pickup.
        let delay = capture_delay(
            OffsetDateTime::now_utc(),
            recording.start_time,
            recording.pre_padding_secs,
        );
        if !delay.is_zero() {
            info!(
                recording_id = %recording_id,
                wait_secs = delay.as_secs(),
                "Waiting for recording start"
            );
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = cancel.cancelled() => {}
            }
            nats_jobs::check_cancelled(&cancel)?;
        }

        // Atomically claim the recording. If a duplicate enqueue from the
        // lookahead window already claimed it, stop — the winner owns the
        // capture.
        match iptv_recordings::try_claim_for_recording(&self.pool, recording_id).await {
            Ok(true) => {}
            Ok(false) => {
                info!(recording_id = %recording_id, "Recording already claimed, skipping duplicate");
                return Ok(json!({ "recording_id": recording_id, "skipped": "already_claimed" }));
            }
            Err(e) => {
                return Err(JobError::ExecutionFailed(format!(
                    "Failed to claim recording: {e}"
                )));
            }
        }

        // Get all sources for the channel, ordered by priority/health
        let sources = iptv_sources::list_by_channel(&self.pool, recording.channel_id)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to list sources: {e}")))?;

        if sources.is_empty() {
            let error_msg = "No sources available for channel";
            let _ = iptv_recordings::update_status(
                &self.pool,
                recording_id,
                "failed",
                None,
                None,
                Some(error_msg),
            )
            .await;
            return Err(JobError::ExecutionFailed(error_msg.to_string()));
        }

        // Calculate total duration including padding
        let total_duration_secs = (recording.end_time - recording.start_time).whole_seconds()
            + i64::from(recording.pre_padding_secs)
            + i64::from(recording.post_padding_secs);

        // Build output path — use configured recording_path if set, else default
        let configured_path = iptv_recordings::get_recording_path(&self.pool)
            .await
            .map_err(|e| {
                JobError::ExecutionFailed(format!("Failed to read recording path config: {e}"))
            })?;
        let recordings_dir = match configured_path {
            Some(p) if !p.is_empty() => PathBuf::from(p),
            _ => PathBuf::from(&self.storage_root).join("iptv_recordings"),
        };
        tokio::fs::create_dir_all(&recordings_dir)
            .await
            .map_err(|e| {
                JobError::ExecutionFailed(format!("Failed to create recordings dir: {e}"))
            })?;
        let final_path = recordings_dir.join(format!("{recording_id}.mp4"));

        let mut segments: Vec<PathBuf> = Vec::new();
        let mut remaining_secs = total_duration_secs;
        let mut last_error = String::new();
        let mut attempt = 0;

        // Try sources in order with segmented failover
        for source in sources.iter().take(MAX_ATTEMPTS) {
            nats_jobs::check_cancelled(&cancel)?;
            attempt += 1;

            // Check provider capacity before attempting
            if !provider_tracker::has_capacity(source.provider_id) {
                warn!(
                    attempt,
                    provider_id = %source.provider_id,
                    "provider at max connections, skipping"
                );
                continue;
            }

            let provider = match iptv_providers::get_by_id(&self.pool, source.provider_id).await {
                Ok(Some(p)) => p,
                Ok(None) => {
                    warn!(attempt, provider_id = %source.provider_id, "provider not found, skipping");
                    continue;
                }
                Err(e) => {
                    warn!(attempt, error = %e, "failed to fetch provider, skipping");
                    continue;
                }
            };

            let client = XtreamClient::new(
                crate::http::new_client(),
                &provider.url,
                &provider.username,
                &provider.password,
            );
            let stream_url = client.stream_url(source.remote_stream_id, &provider.preferred_format);

            // Segment path: use part suffix if we already have segments
            let segment_path = if segments.is_empty() {
                final_path.clone()
            } else {
                recordings_dir.join(format!("{recording_id}_part{}.mp4", segments.len() + 1))
            };

            let config = RecordingConfig {
                source_url: stream_url,
                output_path: segment_path.clone(),
                duration_secs: remaining_secs,
                hw_accel: None,
            };

            match start_recording(config).await {
                Ok(result) => {
                    let _ = health::record_success(&self.pool, source.id).await;

                    if remaining_secs > 0
                        && result.duration_recorded_secs >= (remaining_secs as f64 - 5.0)
                    {
                        // Full duration recorded (within 5s tolerance)
                        if segments.is_empty() {
                            // Single segment, already at final path
                            let r = finish_recording(
                                &self.pool,
                                recording_id,
                                &final_path,
                                attempt,
                                false,
                            )
                            .await;
                            return self
                                .finish_and_post_process(recording_id, &final_path, r, &cancel)
                                .await;
                        }
                        segments.push(segment_path);
                        let r = concat_and_finish(
                            &self.pool,
                            recording_id,
                            &segments,
                            &final_path,
                            attempt,
                        )
                        .await;
                        return self
                            .finish_and_post_process(recording_id, &final_path, r, &cancel)
                            .await;
                    }

                    // Partial — stream ended early but we got data
                    if result.packets_written > 0 {
                        // First segment needs renaming if it was written to final_path
                        if segments.is_empty() {
                            let part1 = recordings_dir.join(format!("{recording_id}_part1.mp4"));
                            if let Err(e) = tokio::fs::rename(&final_path, &part1).await {
                                warn!(error = %e, "failed to rename first segment");
                                segments.push(final_path.clone());
                            } else {
                                segments.push(part1);
                            }
                        } else {
                            segments.push(segment_path);
                        }
                        remaining_secs -= result.duration_recorded_secs as i64;
                        if remaining_secs <= 0 {
                            let r = concat_and_finish(
                                &self.pool,
                                recording_id,
                                &segments,
                                &final_path,
                                attempt,
                            )
                            .await;
                            return self
                                .finish_and_post_process(recording_id, &final_path, r, &cancel)
                                .await;
                        }
                        let msg = format!(
                            "Attempt {attempt}: partial ({:.0}s captured), continuing",
                            result.duration_recorded_secs
                        );
                        warn!(
                            attempt,
                            duration = result.duration_recorded_secs,
                            "partial recording, switching source"
                        );
                        if !last_error.is_empty() {
                            last_error.push_str("; ");
                        }
                        last_error.push_str(&msg);
                        continue;
                    }

                    // Zero packets — useless
                    let _ = tokio::fs::remove_file(&segment_path).await;
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    let is_conn_limit = is_connection_limit_error(&err_msg);

                    if !is_conn_limit {
                        let _ = health::record_failure(&self.pool, source.id).await;
                    }

                    let msg = format!("Attempt {attempt}: {err_msg}");
                    warn!(attempt, error = %e, conn_limit = is_conn_limit, "recording attempt failed");
                    if !last_error.is_empty() {
                        last_error.push_str("; ");
                    }
                    last_error.push_str(&msg);

                    // Clean up empty/partial segment on error
                    let _ = tokio::fs::remove_file(&segment_path).await;
                }
            }
        }

        // If we have any segments, concatenate and finish (partial recording is better than nothing)
        if !segments.is_empty() {
            let r =
                concat_and_finish(&self.pool, recording_id, &segments, &final_path, attempt).await;
            return self
                .finish_and_post_process(recording_id, &final_path, r, &cancel)
                .await;
        }

        // All direct attempts failed — try catch-up URL if any source has tv_archive
        for source in &sources {
            let has_archive = source
                .metadata
                .get("tv_archive")
                .and_then(|v| v.as_str())
                .is_some_and(|v| v == "1");

            if !has_archive {
                continue;
            }

            attempt += 1;
            let provider = match iptv_providers::get_by_id(&self.pool, source.provider_id).await {
                Ok(Some(p)) => p,
                _ => continue,
            };

            // Use cached timeshift format if available, otherwise default to B1G format
            let duration_minutes = total_duration_secs.unsigned_abs().div_ceil(60);
            let fmt = provider
                .account_info
                .get("timeshift_format")
                .and_then(|v| v.as_str())
                .unwrap_or("{base}/timeshift/{user}/{pass}/{duration}/{date}/{stream_id}.ts");

            let start = recording.start_time;
            let date_str = format!(
                "{}-{:02}-{:02}:{:02}-{:02}",
                start.year(),
                start.month() as u8,
                start.day(),
                start.hour(),
                start.minute(),
            );
            let catchup_url = fmt
                .replace("{base}", provider.url.trim_end_matches('/'))
                .replace("{user}", &provider.username)
                .replace("{pass}", &provider.password)
                .replace("{duration}", &duration_minutes.to_string())
                .replace("{date}", &date_str)
                .replace("{stream_id}", &source.remote_stream_id.to_string());

            let config = RecordingConfig {
                source_url: catchup_url,
                output_path: final_path.clone(),
                duration_secs: total_duration_secs,
                hw_accel: None,
            };

            match start_recording(config).await {
                Ok(_) => {
                    let r = finish_recording(&self.pool, recording_id, &final_path, attempt, true)
                        .await;
                    return self
                        .finish_and_post_process(recording_id, &final_path, r, &cancel)
                        .await;
                }
                Err(e) => {
                    let msg = format!("Catch-up attempt {attempt}: {e}");
                    warn!(attempt, error = %e, "catch-up recording attempt failed");
                    if !last_error.is_empty() {
                        last_error.push_str("; ");
                    }
                    last_error.push_str(&msg);
                    break;
                }
            }
        }

        // All attempts failed
        let _ = iptv_recordings::update_status(
            &self.pool,
            recording_id,
            "failed",
            None,
            None,
            Some(&last_error),
        )
        .await;

        Err(JobError::ExecutionFailed(format!(
            "Recording failed after {attempt} attempts: {last_error}"
        )))
    }
}

/// Concatenate segments and mark recording as completed.
async fn concat_and_finish(
    pool: &PgPool,
    recording_id: Uuid,
    segments: &[PathBuf],
    final_path: &PathBuf,
    attempt: usize,
) -> JobResult<Value> {
    let segs: Vec<PathBuf> = segments.to_vec();
    let final_out = final_path.clone();

    let concat_result = tokio::task::spawn_blocking(move || {
        let seg_refs: Vec<&std::path::Path> = segs.iter().map(|p| p.as_path()).collect();
        kob_ffmpeg::iptv::concat_segments(&seg_refs, &final_out)
    })
    .await
    .map_err(|e| JobError::ExecutionFailed(format!("concat task join error: {e}")))?;

    if let Err(e) = concat_result {
        // Concat failed — keep segments, report error
        let msg = format!("Segment concat failed: {e}");
        let _ =
            iptv_recordings::update_status(pool, recording_id, "failed", None, None, Some(&msg))
                .await;
        return Err(JobError::ExecutionFailed(msg));
    }

    // Clean up segment files (concat_segments renames single segments, so check existence)
    for seg in segments {
        if seg != final_path && seg.exists() {
            let _ = tokio::fs::remove_file(seg).await;
        }
    }

    finish_recording(pool, recording_id, final_path, attempt, false).await
}

/// Mark recording as completed with file metadata.
async fn finish_recording(
    pool: &PgPool,
    recording_id: Uuid,
    final_path: &PathBuf,
    attempt: usize,
    catchup: bool,
) -> JobResult<Value> {
    let file_size = tokio::fs::metadata(final_path)
        .await
        .map(|m| m.len() as i64)
        .ok();

    let file_path_str = final_path.to_string_lossy();
    let _ = iptv_recordings::update_status(
        pool,
        recording_id,
        "completed",
        Some(&file_path_str),
        file_size,
        None,
    )
    .await;

    info!(recording_id = %recording_id, attempt, catchup, "Recording completed");
    Ok(json!({
        "recording_id": recording_id,
        "status": "completed",
        "file_path": file_path_str.as_ref(),
        "file_size_bytes": file_size,
        "attempts": attempt,
        "catchup": catchup,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_delay_waits_until_padded_start() {
        let now = OffsetDateTime::from_unix_timestamp(1_000).unwrap();
        // Starts in 100s with 30s pre-padding => capture should begin in 70s.
        let start = now + time::Duration::seconds(100);
        let delay = capture_delay(now, start, 30);
        assert_eq!(delay, std::time::Duration::from_secs(70));
    }

    #[test]
    fn capture_delay_zero_when_start_already_passed() {
        let now = OffsetDateTime::from_unix_timestamp(1_000).unwrap();
        let start = now - time::Duration::seconds(10);
        assert!(capture_delay(now, start, 0).is_zero());
    }

    #[test]
    fn capture_delay_zero_when_padding_covers_start() {
        let now = OffsetDateTime::from_unix_timestamp(1_000).unwrap();
        // Starts in 20s but 60s pre-padding => padded start is already past.
        let start = now + time::Duration::seconds(20);
        assert!(capture_delay(now, start, 60).is_zero());
    }

    #[test]
    fn capture_delay_capped_for_far_future() {
        let now = OffsetDateTime::from_unix_timestamp(1_000).unwrap();
        let start = now + time::Duration::hours(10);
        assert_eq!(capture_delay(now, start, 0), MAX_RECORDING_WAIT);
    }
}
