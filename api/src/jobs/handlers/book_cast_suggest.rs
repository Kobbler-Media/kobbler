//! Asks the model who says a character's lines, each read in its scene, and
//! keeps its answers in the job's result for the admin to accept or not. The
//! cast is not touched.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::book_cast_stage::job_error;
use crate::book_cast::suggest::{self, LineKey};
use crate::book_cast::{LLM_RETRY, llm};
use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::repositories::book_casts::{self, CastLine};
use crate::repositories::{book_cast_vetting as vetting, ebook_segments};
use crate::runtime_config::RuntimeConfigManager;

pub const SUGGEST_JOB: &str = "book_cast_suggest";

#[derive(Debug, Deserialize)]
struct Payload {
    book_id: Uuid,
    character_id: Uuid,
    /// The lines to ask about; all the character's when empty.
    #[serde(default)]
    lines: Vec<LineKey>,
}

pub struct BookCastSuggestHandler {
    pool: sqlx::PgPool,
    runtime_config: RuntimeConfigManager,
    retry: &'static [Duration],
}

impl BookCastSuggestHandler {
    pub fn new(pool: sqlx::PgPool, runtime_config: RuntimeConfigManager) -> Self {
        Self {
            pool,
            runtime_config,
            retry: LLM_RETRY,
        }
    }

    pub fn with_retry(mut self, retry: &'static [Duration]) -> Self {
        self.retry = retry;
        self
    }
}

#[async_trait]
impl JobHandler for BookCastSuggestHandler {
    fn job_type(&self) -> &'static str {
        SUGGEST_JOB
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
        let p: Payload = serde_json::from_value(payload)?;
        let pool = &self.pool;
        let Some(cast) = book_casts::get(pool, p.book_id).await? else {
            return Ok(json!({"status": "gone"}));
        };
        let Some(who) = vetting::character(pool, p.book_id, p.character_id).await? else {
            return Ok(json!({"status": "gone"}));
        };
        let (segments, lines, chapters, characters) = tokio::try_join!(
            ebook_segments::get_for_file(pool, cast.book_file_id),
            book_casts::lines(pool, p.book_id, None),
            book_casts::chapters(pool, p.book_id),
            vetting::characters(pool, p.book_id),
        )?;
        let asked: Vec<&CastLine> = lines
            .iter()
            .filter(|l| l.character_id == Some(who.id))
            .filter(|l| p.lines.is_empty() || p.lines.contains(&LineKey::of(l)))
            .take(suggest::MOST_LINES)
            .collect();
        let texts: HashMap<i32, String> = segments
            .into_iter()
            .map(|s| (s.segment_index, s.text))
            .collect();
        let names: HashMap<Uuid, &str> =
            characters.iter().map(|c| (c.id, c.name.as_str())).collect();
        let cfg = llm::config(&self.runtime_config.get().await).map_err(job_error)?;

        let mut out = Vec::new();
        for batch in suggest::batches(&asked, &lines, &texts, &chapters, &names) {
            nats_jobs::check_cancelled(&cancel)?;
            let prompt = suggest::prompt(&who, &characters, &batch);
            let ask = llm::Ask {
                system: suggest::SYSTEM,
                user: &prompt,
                name: "speakers",
                schema: suggest::schema(),
                max_tokens: suggest::max_tokens(batch.lines.len()),
            };
            let reply: suggest::Reply = llm::json(&cfg, self.retry, &cancel, &ask)
                .await
                .map_err(job_error)?;
            out.extend(suggest::suggestions(&asked, &batch, &reply, &characters));
        }
        Ok(json!({"character_id": who.id, "asked": asked.len(), "suggestions": out}))
    }
}
