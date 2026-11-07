//! Full-cast pass 2: the model reads a summary of each first-pass speaker,
//! says who is who, and rechecks the lines pass 1 could not attribute. The
//! code then tidies the names it left loose.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use nats_jobs::Publisher;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::book_cast_stage::{self, Next, job_error, superseded};
use crate::book_cast::chapters::{self, CharacterNote, is_nobody, same_name};
use crate::book_cast::resolve::Speaker;
use crate::book_cast::{LLM_RETRY, POOL_BELOW, bible, llm, resolve};
use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::repositories::book_casts::{self, CastState, NewCharacter, Reattribution, Run};
use crate::repositories::ebook_segments;
use crate::runtime_config::RuntimeConfigManager;

pub struct BookCastBibleHandler {
    pool: PgPool,
    runtime_config: RuntimeConfigManager,
    nats_publisher: Publisher,
    retry: &'static [Duration],
}

impl BookCastBibleHandler {
    pub fn new(
        pool: PgPool,
        runtime_config: RuntimeConfigManager,
        nats_publisher: Publisher,
    ) -> Self {
        Self {
            pool,
            runtime_config,
            nats_publisher,
            retry: LLM_RETRY,
        }
    }

    pub fn with_retry(mut self, retry: &'static [Duration]) -> Self {
        self.retry = retry;
        self
    }

    async fn run(&self, run: &Run, file_id: Uuid, cancel: &CancellationToken) -> JobResult<Value> {
        let book_id = run.book_id;
        let chapters = book_casts::chapters(&self.pool, book_id).await?;
        let lines = book_casts::lines(&self.pool, book_id, None).await?;
        let segments = ebook_segments::get_for_file(&self.pool, file_id).await?;

        let paragraphs: HashMap<i32, String> = segments
            .into_iter()
            .map(|s| (s.segment_index, s.text))
            .collect();

        let (prompt, ids) = bible::prompt(&chapters, &lines, &paragraphs);
        let cfg = llm::config(&self.runtime_config.get().await).map_err(job_error)?;
        let reply: bible::Reply = llm::json(
            &cfg,
            self.retry,
            cancel,
            bible::SYSTEM,
            &prompt,
            "cast",
            bible::schema(),
        )
        .await
        .map_err(job_error)?;

        let reattributed: Vec<Reattribution> = reply
            .lines
            .iter()
            .filter(|r| !is_nobody(&r.speaker))
            .filter_map(|r| {
                ids.get(r.id).map(|line| Reattribution {
                    segment_index: line.segment_index,
                    span_start: line.span_start,
                    speaker: r.speaker.clone(),
                })
            })
            .collect();

        let characters: Vec<NewCharacter> = reply
            .characters
            .iter()
            .map(|c| NewCharacter {
                name: c.name.clone(),
                aliases: c.aliases.clone(),
                generic: c.generic,
                bible: json!({}),
            })
            .collect();
        let notes: Vec<CharacterNote> = chapters.iter().flat_map(chapters::notes).collect();
        let mut names: Vec<String> = lines.iter().filter_map(|l| l.speaker.clone()).collect();
        names.extend(reattributed.iter().map(|r| r.speaker.clone()));
        names.sort();
        names.dedup();
        let speakers: Vec<Speaker> = names
            .into_iter()
            .map(|name| {
                let mut profiled = notes
                    .iter()
                    .filter(|n| same_name(&n.name, &name))
                    .peekable();
                let generic = profiled.peek().is_some() && profiled.all(|n| n.generic);
                Speaker { name, generic }
            })
            .collect();
        let mut characters = resolve::tidy(characters, &speakers);
        for c in &mut characters {
            c.bible = bible::traits(c, &notes);
        }

        if !book_casts::replace_characters(&self.pool, run, &characters, &reattributed, POOL_BELOW)
            .await?
        {
            return Ok(superseded());
        }
        Ok(json!({
            "characters": characters.len(),
            "rechecked": ids.len(),
            "reattributed": reattributed.len(),
        }))
    }
}

#[async_trait]
impl JobHandler for BookCastBibleHandler {
    fn job_type(&self) -> &'static str {
        "book_cast_bible"
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
            state: CastState::Voices,
            job: Some((&self.nats_publisher, "book_cast_voices")),
        };
        book_cast_stage::run(
            &self.pool,
            payload,
            CastState::Bible,
            next,
            |cast, run| async move { self.run(&run, cast.book_file_id, &cancel).await },
        )
        .await
    }
}
