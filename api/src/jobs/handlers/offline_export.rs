use std::time::Duration;

use async_trait::async_trait;
use kob_ffmpeg::RunContext;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::offline_exports;
use crate::streaming::decision::aac_encoder;
use crate::streaming::offline_export::{
    Encoders, ExportPaths, ExportPlan, hls_config, remux_faststart, transcoded_fraction,
};

/// The remux after the transcode is quick but not free; leave it the last slice of the bar.
const TRANSCODE_SHARE: f64 = 0.95;
const PROGRESS_POLL: Duration = Duration::from_secs(2);

pub struct OfflineExportHandler {
    state: AppState,
}

impl OfflineExportHandler {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

enum ExportError {
    Cancelled,
    Failed(String),
}

impl From<kob_ffmpeg::FfmpegError> for ExportError {
    fn from(e: kob_ffmpeg::FfmpegError) -> Self {
        if e.is_cancelled() {
            Self::Cancelled
        } else {
            Self::Failed(e.to_string())
        }
    }
}

impl From<std::io::Error> for ExportError {
    fn from(e: std::io::Error) -> Self {
        Self::Failed(e.to_string())
    }
}

impl From<tokio::task::JoinError> for ExportError {
    fn from(e: tokio::task::JoinError) -> Self {
        Self::Failed(format!("export task failed: {e}"))
    }
}

#[async_trait]
impl JobHandler for OfflineExportHandler {
    fn job_type(&self) -> &'static str {
        "offline_export"
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
        let plan: ExportPlan = serde_json::from_value(payload)?;
        nats_jobs::check_cancelled(&cancel)?;
        let pool = &self.state.pool;
        if offline_exports::claim(pool, plan.export_id)
            .await?
            .is_none()
        {
            return Ok(json!({ "export_id": plan.export_id, "skipped": true }));
        }

        let paths = ExportPaths::new(&self.state.config.storage_root, plan.export_id);
        let outcome = produce(&self.state, &plan, &paths, &cancel).await;
        remove_work_files(&paths).await;

        match outcome {
            Ok(size) => {
                offline_exports::mark_ready(pool, plan.export_id, size as i64).await?;
                info!(export_id = %plan.export_id, size, "offline export ready");
                Ok(json!({ "export_id": plan.export_id, "size_bytes": size }))
            }
            Err(ExportError::Cancelled) => {
                offline_exports::mark_failed(pool, plan.export_id, "cancelled").await?;
                Err(JobError::Cancelled)
            }
            Err(ExportError::Failed(message)) => {
                warn!(export_id = %plan.export_id, error = %message, "offline export failed");
                offline_exports::mark_failed(pool, plan.export_id, &message).await?;
                Err(JobError::ExecutionFailed(message))
            }
        }
    }
}

async fn produce(
    state: &AppState,
    plan: &ExportPlan,
    paths: &ExportPaths,
    cancel: &CancellationToken,
) -> Result<u64, ExportError> {
    remove_work_files(paths).await;
    tokio::fs::create_dir_all(&paths.dir).await?;

    let encoders = Encoders {
        video: state.encoder_caps.h264.clone(),
        audio: aac_encoder().to_string(),
        hw_device: state.hw_device.get(),
    };
    let config = hls_config(
        plan,
        &paths.work_dir,
        &encoders,
        RunContext::cancellable(cancel.clone()),
    );
    let input = plan.input_path.clone();
    let mut transcode = tokio::task::spawn_blocking(move || {
        kob_ffmpeg::transcode::transcode_to_hls(&input, &config).map(|_| ())
    });

    let mut tick = tokio::time::interval(PROGRESS_POLL);
    let mut reported = 0.0;
    loop {
        tokio::select! {
            done = &mut transcode => {
                done??;
                break;
            }
            _ = tick.tick() => {
                let Ok(playlist) = tokio::fs::read_to_string(&paths.playlist).await else {
                    continue;
                };
                if let Some(fraction) = transcoded_fraction(&playlist, plan.duration_seconds) {
                    let progress = fraction * TRANSCODE_SHARE;
                    if progress - reported >= 0.01 {
                        reported = progress;
                        if let Err(e) =
                            offline_exports::set_progress(&state.pool, plan.export_id, progress).await
                        {
                            warn!(export_id = %plan.export_id, error = %e, "failed to record export progress");
                        }
                    }
                }
            }
        }
    }

    let playlist = paths.playlist.clone();
    let partial = paths.partial.clone();
    let run = RunContext::cancellable(cancel.clone());
    tokio::task::spawn_blocking(move || remux_faststart(&playlist, &partial, &run)).await??;
    tokio::fs::rename(&paths.partial, &paths.file).await?;
    Ok(tokio::fs::metadata(&paths.file).await?.len())
}

async fn remove_work_files(paths: &ExportPaths) {
    if let Err(e) = tokio::fs::remove_dir_all(&paths.work_dir).await
        && e.kind() != std::io::ErrorKind::NotFound
    {
        warn!(dir = %paths.work_dir.display(), error = %e, "failed to remove export work dir");
    }
    if let Err(e) = tokio::fs::remove_file(&paths.partial).await
        && e.kind() != std::io::ErrorKind::NotFound
    {
        warn!(file = %paths.partial.display(), error = %e, "failed to remove partial export");
    }
}
