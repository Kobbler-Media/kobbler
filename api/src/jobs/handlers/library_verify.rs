//! Runs the import check over the books already in the library, for files
//! filed before it existed. It only reports: deciding what is wrong stays with
//! a person. Narration is never transcribed here, a library's worth would hold
//! the GPU for hours.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use crate::RuntimeConfigManager;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobEvent, JobResult, update_progress};
use crate::repositories::books;
use crate::wanted::verify::{Expected, Inspector, Verdict, inspect, judge};

pub struct LibraryVerifyHandler {
    pool: PgPool,
    runtime_config: RuntimeConfigManager,
    job_events: broadcast::Sender<JobEvent>,
    scratch_dir: PathBuf,
}

impl LibraryVerifyHandler {
    pub fn new(
        pool: PgPool,
        runtime_config: RuntimeConfigManager,
        job_events: broadcast::Sender<JobEvent>,
        storage_root: &str,
    ) -> Self {
        Self {
            pool,
            runtime_config,
            job_events,
            scratch_dir: Path::new(storage_root).join("downloads"),
        }
    }
}

#[async_trait]
impl JobHandler for LibraryVerifyHandler {
    fn job_type(&self) -> &'static str {
        "library_verify"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        job_id: Uuid,
        _payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let files = books::list_library_files(&self.pool)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?;
        let household = self.runtime_config.get().await.preferred_audio_language;
        let inspector = Inspector {
            stt_url: None,
            scratch_dir: self.scratch_dir.clone(),
        };
        let total = files.len().max(1);
        let mut mismatches = Vec::new();
        let mut unreadable = 0;
        for (done, (book_id, path)) in files.iter().enumerate() {
            nats_jobs::check_cancelled(&cancel)?;
            let Some(expected) = Expected::for_book(&self.pool, *book_id, &household).await else {
                continue;
            };
            let Some(found) = inspect(Path::new(path), &expected.authors, &inspector).await else {
                unreadable += 1;
                continue;
            };
            if let Verdict::WrongBook(reason) = judge(&expected, &found) {
                warn!(%book_id, path, %reason, "Library file disagrees with its book");
                mismatches.push(json!({
                    "book_id": book_id,
                    "title": expected.title,
                    "authors": expected.authors,
                    "path": path,
                    "reason": reason,
                }));
            }
            if done % 25 == 0 {
                let _ = update_progress(
                    &self.pool,
                    &self.job_events,
                    job_id,
                    self.job_type(),
                    (done * 100 / total) as i32,
                    None,
                )
                .await;
            }
        }
        info!(
            checked = files.len(),
            unreadable,
            mismatches = mismatches.len(),
            "Library verification finished"
        );
        Ok(json!({
            "checked": files.len(),
            "unreadable": unreadable,
            "mismatches": mismatches,
        }))
    }
}
