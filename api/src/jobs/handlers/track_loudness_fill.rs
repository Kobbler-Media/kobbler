//! Gives files analysed before each audio stream was measured on its own — and finished
//! debrid downloads nothing measured — just that measurement: an audio-only decode, far
//! cheaper than re-running the whole single pass. Its own job type, so a library's worth of
//! re-measuring never queues ahead of a new import's analysis.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult, MediaRootIoGate};
use crate::media_cache;
use crate::repositories::file_loudness::{self, FileKind};
use crate::repositories::media_roots;
use crate::streaming::loudness::PlaybackLoudness;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub const JOB_TYPE: &str = "track_loudness_fill";

#[derive(Debug, Serialize, Deserialize)]
pub struct TrackLoudnessFillPayload {
    pub kind: FileKind,
    pub file_id: Uuid,
}

pub struct TrackLoudnessFillHandler {
    pool: PgPool,
    storage_root: String,
    io_gate: MediaRootIoGate,
}

impl TrackLoudnessFillHandler {
    pub fn new(pool: PgPool, storage_root: String, io_gate: MediaRootIoGate) -> Self {
        Self {
            pool,
            storage_root,
            io_gate,
        }
    }

    async fn file_path(&self, kind: FileKind, file_id: Uuid) -> JobResult<Option<PathBuf>> {
        let path: Option<String> = match kind {
            FileKind::Movie => {
                sqlx::query_scalar("SELECT file_path FROM movie_files WHERE id = $1")
            }
            FileKind::Episode => {
                sqlx::query_scalar("SELECT file_path FROM tv_episode_files WHERE id = $1")
            }
            FileKind::Audiobook => {
                sqlx::query_scalar("SELECT file_path FROM audiobook_files WHERE id = $1")
            }
            FileKind::Debrid => {
                sqlx::query_scalar("SELECT cache_path FROM debrid_files WHERE id = $1")
            }
        }
        .bind(file_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;
        Ok(path
            .map(|p| match kind {
                FileKind::Audiobook => {
                    crate::audio::path::resolve_file_path(&self.storage_root, &p)
                }
                _ => PathBuf::from(p),
            })
            .filter(|p| kind != FileKind::Debrid || p.is_file()))
    }

    async fn has_gap(&self, kind: FileKind, file_id: Uuid) -> JobResult<bool> {
        let db = |e: crate::repositories::RepositoryError| {
            JobError::ExecutionFailed(format!("DB error: {e}"))
        };
        Ok(match kind {
            FileKind::Audiobook => {
                let current = file_loudness::audiobook(&self.pool, file_id)
                    .await
                    .map_err(db)?;
                current.lufs.is_some() && current.downmix_peak_db.is_none()
            }
            _ => file_loudness::tracks(&self.pool, kind, file_id)
                .await
                .map_err(db)?
                .is_none(),
        })
    }
}

#[async_trait]
impl JobHandler for TrackLoudnessFillHandler {
    fn job_type(&self) -> &'static str {
        JOB_TYPE
    }

    // Reads are bounded by each media root's IO gate; this only needs to keep every root busy.
    fn max_concurrent(&self) -> usize {
        8
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        nats_jobs::check_cancelled(&cancel)?;
        let TrackLoudnessFillPayload { kind, file_id } = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {e}")))?;
        let db = |e: crate::repositories::RepositoryError| {
            JobError::ExecutionFailed(format!("DB error: {e}"))
        };
        let _file_lock = crate::jobs::file_lock::lock_file(file_id).await;

        if !self.has_gap(kind, file_id).await? {
            return Ok(json!({"status": "no_gap", "file_id": file_id}));
        }
        let Some(file_path) = self.file_path(kind, file_id).await? else {
            return Ok(json!({"status": "file_gone", "file_id": file_id}));
        };

        let spool = media_cache::source_spool_path(&self.storage_root, file_id);
        let (path, permits) = if spool.is_file() {
            (spool, Vec::new())
        } else if kind == FileKind::Debrid {
            (file_path, Vec::new())
        } else {
            let roots =
                media_roots::resolve_media_root_ids_for_file(&self.pool, kind.as_str(), file_id)
                    .await
                    .unwrap_or_default();
            (file_path, self.io_gate.acquire_many(roots).await)
        };
        nats_jobs::check_cancelled(&cancel)?;
        let measured = crate::ffmpeg::transcode::measure_loudness(&path)
            .await
            .map_err(|e| JobError::ExecutionFailed(e.to_string()))?;
        drop(permits);

        match kind {
            FileKind::Audiobook => {
                let downmix_peak_db = measured.first().and_then(|t| t.loudness.downmix_peak_db);
                file_loudness::record_audiobook(
                    &self.pool,
                    file_id,
                    PlaybackLoudness {
                        lufs: None,
                        downmix_peak_db,
                    },
                )
                .await
                .map_err(db)?;
            }
            _ => file_loudness::record_tracks(&self.pool, kind, file_id, &measured)
                .await
                .map_err(db)?,
        }
        Ok(json!({"status": "filled", "file_id": file_id, "tracks": measured}))
    }
}
