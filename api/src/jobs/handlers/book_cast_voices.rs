//! Full-cast pass 3: the model sums up and designs a voice for each character
//! who gets one of their own, a few at a time from their chapter profiles.
//! Each batch is saved as it lands, so a rerun designs only the characters
//! still without a voice. The voices are then seeded, and the clones of any
//! the cast no longer needs are let go.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::book_cast_stage::{self, Next, job_error, superseded};
use crate::book_cast::voices::{self, needs_design};
use crate::book_cast::{LLM_RETRY, llm};
use crate::book_cast_vetting::seed;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::book_cast_vetting::VettingCharacter;
use crate::repositories::book_casts::{self, BookCast, CastState, Run};
use crate::repositories::{book_cast_vetting, ebook_segments};
use crate::runtime_config::RuntimeConfigManager;

/// Enough of the book for its tone, little enough to sit beside a batch.
const OPENING_CHARS: usize = 6_000;

pub struct BookCastVoicesHandler {
    pool: PgPool,
    runtime_config: RuntimeConfigManager,
    storage_root: String,
    retry: &'static [Duration],
}

impl BookCastVoicesHandler {
    pub fn new(pool: PgPool, runtime_config: RuntimeConfigManager, storage_root: String) -> Self {
        Self {
            pool,
            runtime_config,
            storage_root,
            retry: LLM_RETRY,
        }
    }

    pub fn with_retry(mut self, retry: &'static [Duration]) -> Self {
        self.retry = retry;
        self
    }

    async fn run(
        &self,
        cast: &BookCast,
        run: &Run,
        cancel: &CancellationToken,
    ) -> JobResult<Value> {
        let book_id = cast.book_id;
        book_cast_vetting::ensure_narrator(&self.pool, book_id).await?;
        let chapters = book_casts::chapters(&self.pool, book_id).await?;
        let mut opening = String::new();
        for s in ebook_segments::get_for_file(&self.pool, cast.book_file_id).await? {
            if opening.len() >= OPENING_CHARS {
                break;
            }
            opening.push_str(&s.text);
            opening.push('\n');
        }
        let characters = book_cast_vetting::characters(&self.pool, book_id).await?;
        let todo: Vec<&VettingCharacter> = characters.iter().filter(|c| needs_design(c)).collect();
        let cfg = llm::config(&self.runtime_config.get().await).map_err(job_error)?;

        let mut designed = 0;
        for (batch, prompt) in voices::batches(&todo, &chapters, &opening) {
            nats_jobs::check_cancelled(cancel)?;
            let reply: voices::Reply = llm::json(
                &cfg,
                self.retry,
                cancel,
                voices::SYSTEM,
                &prompt,
                "voices",
                voices::schema(),
            )
            .await
            .map_err(job_error)?;
            for (c, v) in voices::pair(&batch, &reply.characters) {
                book_casts::set_voice(&self.pool, c.id, &voices::design(v)).await?;
                designed += 1;
            }
        }
        if designed < todo.len() {
            return Err(JobError::Retryable(format!(
                "{} of {} characters got no voice design; a rerun asks for just those",
                todo.len() - designed,
                todo.len()
            )));
        }
        let seeded = seed::seed_run(
            &self.pool,
            &self.runtime_config,
            &self.storage_root,
            run,
            cast.owner_id,
        )
        .await?;
        if !seeded {
            return Ok(superseded());
        }
        Ok(json!({"designed": designed}))
    }
}

#[async_trait]
impl JobHandler for BookCastVoicesHandler {
    fn job_type(&self) -> &'static str {
        "book_cast_voices"
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
        let next = Next {
            state: CastState::Casting,
            job: None,
        };
        book_cast_stage::run(
            &self.pool,
            payload,
            CastState::Voices,
            next,
            |cast, run| async move { self.run(&cast, &run, &cancel).await },
        )
        .await
    }
}
