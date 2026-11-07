//! Approves every voice of a cast still waiting for it, each with its default
//! take, through the same path as approving one by hand. A voice that fails
//! stops the job; a rerun approves only the voices still left. It stops too
//! once the cast is gone or back in analysis.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::AppState;
use crate::book_cast_render::unapproved;
use crate::book_cast_vetting::takes::approve_step;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::book_cast_vetting as vetting;
use crate::repositories::book_casts::{self, BookCast, CastState};

pub const APPROVE_JOB: &str = "book_cast_approve";

#[derive(Debug, Deserialize)]
struct Payload {
    book_id: Uuid,
}

pub struct BookCastApproveHandler {
    state: AppState,
}

impl BookCastApproveHandler {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    async fn vetting(&self, book_id: Uuid) -> JobResult<Option<BookCast>> {
        Ok(book_casts::get(&self.state.pool, book_id)
            .await?
            .filter(|c| {
                matches!(
                    c.state,
                    CastState::Casting | CastState::Rendering | CastState::Done | CastState::Failed
                )
            }))
    }
}

#[async_trait]
impl JobHandler for BookCastApproveHandler {
    fn job_type(&self) -> &'static str {
        APPROVE_JOB
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let Payload { book_id } = serde_json::from_value(payload)?;
        let pool = &self.state.pool;
        if self.vetting(book_id).await?.is_none() {
            return Ok(json!({"status": "not_vetting"}));
        }
        let (characters, voices) = tokio::try_join!(
            vetting::characters(pool, book_id),
            vetting::voices(pool, book_id),
        )?;
        let todo: Vec<_> = unapproved(&characters, &voices)
            .into_iter()
            .filter_map(|u| Some((u.name, u.step?)))
            .collect();
        for (n, (name, (character_id, from_chapter))) in todo.iter().enumerate() {
            nats_jobs::check_cancelled(&cancel)?;
            let Some(cast) = self.vetting(book_id).await? else {
                return Ok(json!({"status": "not_vetting", "approved": n}));
            };
            approve_step(&self.state, &cast, *character_id, *from_chapter, None)
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("approving {name}: {e}")))?;
            let progress = ((n + 1) * 100 / todo.len()) as i32;
            let _ = crate::jobs::update_progress(
                pool,
                &self.state.job_events,
                job_id,
                APPROVE_JOB,
                progress,
                None,
            )
            .await;
        }
        Ok(json!({"approved": todo.len()}))
    }
}
