//! What the three cast passes share: each runs only in its own state, records
//! its error there so the cast resumes from it, and on success moves the cast
//! on and queues the next pass. All of it only while the run still holds the
//! cast: a redo or rerun starts a new one.

use nats_jobs::Publisher;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use tracing::info;
use uuid::Uuid;

use crate::book_cast::llm::LlmError;
use crate::jobs::{JobError, JobResult};
use crate::repositories::book_casts::{self, BookCast, CastState, Run};

#[derive(Debug, Deserialize)]
struct Payload {
    book_id: Uuid,
}

pub(super) struct Next<'a> {
    pub state: CastState,
    pub job: Option<(&'a Publisher, &'static str)>,
}

pub(super) async fn run<F, Fut>(
    pool: &PgPool,
    payload: Value,
    this: CastState,
    next: Next<'_>,
    pass: F,
) -> JobResult<Value>
where
    F: FnOnce(BookCast, Run) -> Fut,
    Fut: Future<Output = JobResult<Value>>,
{
    let Payload { book_id } = serde_json::from_value(payload)?;
    let Some(cast) = book_casts::get(pool, book_id).await? else {
        return Ok(json!({"status": "no_cast"}));
    };
    if cast.state != this {
        return Ok(json!({"status": "skipped", "state": cast.state}));
    }
    let run = Run::of(&cast);
    match pass(cast, run).await {
        Ok(result) => {
            if !book_casts::settle(pool, &run, next.state, None).await? {
                return Ok(superseded());
            }
            if let Some((nats, job)) = next.job {
                crate::jobs::enqueue_book_cast(pool, nats, job, book_id).await?;
            }
            info!(book_id = %book_id, state = this.as_str(), %result, "Book cast pass complete");
            Ok(result)
        }
        Err(e) => {
            if !book_casts::settle(pool, &run, this, Some(&e.to_string())).await? {
                return Ok(superseded());
            }
            Err(e)
        }
    }
}

pub(super) fn superseded() -> Value {
    json!({"status": "superseded"})
}

/// Every model failure is worth another go later, since each pass resumes
/// where it stopped; only a missing koblem needs a person.
pub(super) fn job_error(e: LlmError) -> JobError {
    match e {
        LlmError::NoKoblem => JobError::ExecutionFailed(e.to_string()),
        LlmError::Failed(_) => JobError::Retryable(e.to_string()),
        LlmError::Cancelled => JobError::Cancelled,
    }
}
