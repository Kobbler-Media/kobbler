//! Full-cast pass 1: the resident LLM attributes every quote and profiles each
//! speaker, chapter by chapter. Each chapter is saved whole, so a rerun skips
//! the chapters already saved.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use async_trait::async_trait;
use nats_jobs::Publisher;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::book_cast_stage::{self, Next, job_error, superseded};
use crate::book_cast::chapters::{self, Chapter, CharacterNote, NOT_SPEECH, is_nobody, same_name};
use crate::book_cast::quotes::{self, QuoteStyle};
use crate::book_cast::{LLM_RETRY, WINDOW_CHARS, llm};
use crate::chatbot::llm::LlmConfig;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::book_casts::{self, CastChapter, CastLine, CastState, Run};
use crate::repositories::ebook_segments;
use crate::runtime_config::RuntimeConfigManager;

pub struct BookCastChaptersHandler {
    pool: PgPool,
    runtime_config: RuntimeConfigManager,
    nats_publisher: Publisher,
    retry: &'static [Duration],
}

impl BookCastChaptersHandler {
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
        let segments = ebook_segments::get_for_file(&self.pool, file_id).await?;
        if segments.is_empty() {
            return Err(JobError::ExecutionFailed(
                "the book has no extracted text; run ebook extraction first".into(),
            ));
        }
        let style = quotes::detect(segments.iter().map(|s| s.text.as_str()));
        let chapters = chapters::group(
            segments
                .into_iter()
                .map(|s| (s.segment_index, s.text, s.chapter_title)),
        );
        let done: HashSet<i32> = book_casts::chapter_times(&self.pool, book_id)
            .await?
            .into_iter()
            .map(|t| t.chapter_index)
            .collect();
        let mut known: Vec<String> = Vec::new();
        for saved in book_casts::chapters(&self.pool, book_id).await? {
            remember(&mut known, &chapters::notes(&saved));
        }
        let cfg = llm::config(&self.runtime_config.get().await).map_err(job_error)?;

        let mut attributed = 0;
        for chapter in chapters.iter().filter(|c| !done.contains(&c.index)) {
            nats_jobs::check_cancelled(cancel)?;
            let (saved, lines, notes) =
                self.attribute(&cfg, chapter, style, &known, cancel).await?;
            if !book_casts::save_chapter(&self.pool, run, &saved, &cfg.model, &lines).await? {
                return Ok(superseded());
            }
            remember(&mut known, &notes);
            attributed += lines.len();
        }
        Ok(json!({"chapters": chapters.len(), "resumed_after": done.len(), "lines": attributed}))
    }

    async fn attribute(
        &self,
        cfg: &LlmConfig,
        chapter: &Chapter,
        style: QuoteStyle,
        known: &[String],
        cancel: &CancellationToken,
    ) -> JobResult<(CastChapter, Vec<CastLine>, Vec<CharacterNote>)> {
        let mut known = known.to_vec();
        let mut lines = Vec::new();
        let mut notes = Vec::new();
        for window in chapters::windows(chapter, style, WINDOW_CHARS) {
            if window.quotes.is_empty() {
                continue;
            }
            let ask = llm::Ask {
                system: chapters::SYSTEM,
                user: &chapters::prompt(&window, chapter.title.as_deref(), &known),
                name: "dialogue",
                schema: chapters::schema(),
                max_tokens: chapters::MAX_TOKENS,
            };
            let reply: chapters::Reply = llm::json(cfg, self.retry, cancel, &ask)
                .await
                .map_err(job_error)?;
            let by_q: HashMap<usize, &chapters::ReplyLine> =
                reply.lines.iter().map(|l| (l.q, l)).collect();
            for (q, quote) in window.quotes.iter().enumerate() {
                let said = by_q.get(&q);
                if said.is_some_and(|l| l.speaker == NOT_SPEECH) {
                    continue;
                }
                lines.push(CastLine {
                    segment_index: quote.segment_index,
                    span_start: quote.span.start as i32,
                    span_end: quote.span.end as i32,
                    chapter_index: chapter.index,
                    speaker: said
                        .map(|l| l.speaker.trim())
                        .filter(|s| !is_nobody(s))
                        .map(str::to_string),
                    character_id: None,
                    cue: said.and_then(|l| chapters::delivery(&l.cue)),
                    source: "gemma".into(),
                });
            }
            remember(&mut known, &reply.characters);
            notes.extend(reply.characters);
        }
        let saved = CastChapter {
            chapter_index: chapter.index,
            title: chapter.title.clone(),
            first_segment: chapter.paragraphs.first().map_or(0, |p| p.segment_index),
            last_segment: chapter.paragraphs.last().map_or(0, |p| p.segment_index),
            notes: json!({"characters": notes}),
        };
        Ok((saved, lines, notes))
    }
}

fn remember(known: &mut Vec<String>, notes: &[CharacterNote]) {
    for n in notes.iter().filter(|n| !is_nobody(&n.name)) {
        if !known.iter().any(|k| same_name(k, &n.name)) {
            known.push(n.name.clone());
        }
    }
}

#[async_trait]
impl JobHandler for BookCastChaptersHandler {
    fn job_type(&self) -> &'static str {
        "book_cast_chapters"
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
            state: CastState::Bible,
            job: Some((&self.nats_publisher, "book_cast_bible")),
        };
        book_cast_stage::run(
            &self.pool,
            payload,
            CastState::Chapters,
            next,
            |cast, run| async move { self.run(&run, cast.book_file_id, &cancel).await },
        )
        .await
    }
}
