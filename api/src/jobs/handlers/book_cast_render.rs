//! Voices a vetted cast chapter by chapter, then muxes the chapters into one
//! m4b and imports it as the book's audiobook, in place of any it made before.
//! Each finished chapter is kept with what it was read with, so a crash or a
//! failure resumes at the first chapter still unrendered and a later render
//! reads only the chapters the cast has changed since. The cast and its voices'
//! clips are read once, at the start, and every voice is approved by then: an
//! edit made while it renders leaves its chapter stale.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::book_cast_render::casting::{Casting, CastingError};
use crate::book_cast_render::fingerprint::{self, Fingerprint};
use crate::book_cast_render::{
    Rendered, chapter, chapter_file, m4b, script, status, unapproved, work_dir,
};
use crate::book_cast_vetting::voice_for;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::book_cast_render::{self as repo, RenderedChapter};
use crate::repositories::book_cast_vetting as vetting;
use crate::repositories::book_casts::{self, BookCast, CastState};
use crate::repositories::user_voice_presets::{self, UserVoicePreset};
use crate::repositories::{audiobook_files, ebook_segments, ingest_queue};
use crate::tts::timeline::{Passage, Plan, Route};
use crate::tts::{self, TtsBackend};
use crate::watcher::{FileMode, IngestEntitlements, IngestOptions};

#[derive(Debug, Deserialize)]
struct Payload {
    book_id: Uuid,
}

pub struct BookCastRenderHandler {
    state: AppState,
}

impl BookCastRenderHandler {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    async fn render(
        &self,
        job_id: Uuid,
        cast: &BookCast,
        cancel: &CancellationToken,
    ) -> JobResult<Value> {
        let pool = &self.state.pool;
        let book_id = cast.book_id;
        let config = self.state.runtime_config.get().await;
        let service_url = config
            .effective_tts_url()
            .filter(|u| config.tts_enabled && !u.is_empty())
            .ok_or_else(|| JobError::ExecutionFailed("TTS is not configured".into()))?;
        let (chapters, segments, lines, characters, voices, rendered) = tokio::try_join!(
            book_casts::chapters(pool, book_id),
            ebook_segments::get_for_file(pool, cast.book_file_id),
            book_casts::lines(pool, book_id, None),
            vetting::characters(pool, book_id),
            vetting::voices(pool, book_id),
            repo::rendered(pool, book_id),
        )?;
        let missing = unapproved(&characters, &voices);
        if !missing.is_empty() {
            let names: Vec<&str> = missing.iter().map(|u| u.name.as_str()).collect();
            return Err(JobError::ExecutionFailed(format!(
                "every voice must be approved before the cast is rendered; not yet: {}",
                names.join(", ")
            )));
        }
        let preset_ids: Vec<Uuid> = voices.iter().filter_map(|v| v.preset_id).collect();
        let presets: HashMap<Uuid, UserVoicePreset> =
            user_voice_presets::presets_by_ids(pool, cast.owner_id, &preset_ids)
                .await?
                .into_iter()
                .map(|p| (p.id, p))
                .collect();
        let work = work_dir(&self.state.config.storage_root, book_id);
        tokio::fs::create_dir_all(&work).await.map_err(io)?;

        let snapshot = fingerprint::Cast {
            characters: &characters,
            voices: &voices,
            presets: &presets,
        };
        let todo: HashMap<i32, Fingerprint> =
            status(&chapters, &lines, &snapshot, &rendered, &work)
                .into_iter()
                .filter(|s| s.rendered != Rendered::Fresh)
                .map(|s| (s.chapter_index, s.fingerprint))
                .collect();
        let mut done: HashMap<i32, i64> = rendered
            .iter()
            .filter(|r| !todo.contains_key(&r.chapter_index))
            .map(|r| (r.chapter_index, r.duration_ms))
            .collect();
        let kept = done.len();
        let mut casting = Casting::new(
            book_id,
            &work,
            &service_url,
            TtsBackend::from_config(&config.tts_service_type),
            &presets,
        );
        let total = todo.len();
        let read = async {
            casting
                .keep_clips(&self.state.config.storage_root)
                .await
                .map_err(casting_error)?;
            for ch in chapters.iter() {
                nats_jobs::check_cancelled(cancel)?;
                let Some(read_with) = todo.get(&ch.chapter_index) else {
                    continue;
                };
                let started = Instant::now();
                let title = chapter_title(ch.chapter_index, ch.title.as_deref());
                let texts = segments
                    .iter()
                    .filter(|s| (ch.first_segment..=ch.last_segment).contains(&s.segment_index))
                    .map(|s| (s.segment_index, s.text.as_str()));
                let own: Vec<_> = lines
                    .iter()
                    .filter(|l| l.chapter_index == ch.chapter_index)
                    .cloned()
                    .collect();
                let route =
                    Route::for_config(&config, service_url.clone(), self.state.gpu_lock.clone())
                        .unaligned();
                let mut passages = Vec::new();
                for piece in script(texts, &own) {
                    let voice = voice_for(&characters, &voices, piece.character_id, ch.chapter_index)
                        .and_then(|v| v.preset_id)
                        .ok_or_else(|| {
                            JobError::ExecutionFailed(format!("{title}: a line has no voice"))
                        })?;
                    let voice = casting
                        .voice(voice, piece.cue.as_deref())
                        .await
                        .map_err(casting_error)?;
                    for text in route.split(&tts::normalize_for_synthesis(&piece.text)) {
                        passages.push(Passage {
                            text,
                            voice: voice.clone(),
                        });
                    }
                }
                let duration_ms = if passages.is_empty() {
                    0
                } else {
                    let audio = chapter::render(Plan { passages, route }, cancel)
                        .await
                        .map_err(|e| JobError::ExecutionFailed(format!("{title}: {e}")))?;
                    let path = chapter_file(&work, ch.chapter_index);
                    let tmp = path.with_extension("part");
                    tokio::fs::write(&tmp, &audio.adts).await.map_err(io)?;
                    tokio::fs::rename(&tmp, &path).await.map_err(io)?;
                    audio.duration_ms as i64
                };
                let saved = RenderedChapter {
                    chapter_index: ch.chapter_index,
                    duration_ms,
                    fingerprint: sqlx::types::Json(read_with.clone()),
                    render_ms: started.elapsed().as_millis() as i64,
                };
                repo::save_rendered(pool, book_id, &saved).await?;
                done.insert(ch.chapter_index, duration_ms);
                let progress = ((done.len() - kept) * 100 / (total + 1)) as i32;
                let _ = crate::jobs::update_progress(
                    pool,
                    &self.state.job_events,
                    job_id,
                    "book_cast_render",
                    progress,
                    None,
                )
                .await;
            }

            let markers: Vec<m4b::Marker> = chapters
                .iter()
                .filter_map(|ch| {
                    let ms = *done.get(&ch.chapter_index)?;
                    (ms > 0).then(|| m4b::Marker {
                        title: chapter_title(ch.chapter_index, ch.title.as_deref()),
                        file: chapter_file(&work, ch.chapter_index),
                        duration_ms: ms as u64,
                    })
                })
                .collect();
            if markers.is_empty() {
                return Err(JobError::ExecutionFailed(
                    "the book has nothing to read".into(),
                ));
            }
            let tags = repo::tags(pool, book_id)
                .await?
                .ok_or_else(|| JobError::ExecutionFailed("the book is gone".into()))?;
            let tags = m4b::Tags {
                title: tags.title,
                author: tags.author,
            };
            let name = format!(
                "{} - {}.m4b",
                crate::media_root_naming::sanitize_for_filesystem(&tags.title),
                m4b::NARRATOR
            );
            let out = work.join(&name);
            m4b::mux(&tags, &markers, &work, &out)
                .await
                .map_err(JobError::ExecutionFailed)?;

            let previous = repo::audiobook(pool, book_id).await?;
            let target = previous.as_ref().map_or(book_id, |a| a.book_id);
            let file_id = self.import(target, &out, &name).await?;
            repo::finish(pool, book_id, file_id).await?;
            if let Some(previous) = previous.filter(|p| p.file_id != file_id) {
                self.retire(&previous).await;
            }
            if let Err(e) = tokio::fs::remove_file(&out).await {
                warn!(error = %e, book_id = %book_id, "failed to remove the cast render's muxed book");
            }
            info!(book_id = %book_id, chapters = markers.len(), rendered = total, kept, "Full-cast audiobook imported");
            Ok::<_, JobError>(
                json!({"chapters": markers.len(), "rendered": total, "kept": kept, "audiobook_file_id": file_id}),
            )
        }
        .await;
        casting.release().await;
        read
    }

    /// The audiobook an earlier render imported, once a new one has replaced
    /// it under another path.
    async fn retire(&self, previous: &repo::CastAudiobook) {
        let pool = &self.state.pool;
        let path =
            match audiobook_files::get_by_id_for_book(pool, previous.file_id, previous.book_id)
                .await
            {
                Ok(Some(f)) => f.file_path,
                Ok(None) => return,
                Err(e) => {
                    warn!(error = %e, "failed to look up a replaced cast audiobook");
                    return;
                }
            };
        if let Err(e) = audiobook_files::delete(pool, previous.file_id).await {
            warn!(error = %e, "failed to remove a replaced cast audiobook");
            return;
        }
        if let Some(path) = path
            && let Err(e) = tokio::fs::remove_file(&path).await
        {
            warn!(error = %e, path, "failed to delete a replaced cast audiobook's file");
        }
    }

    async fn import(&self, book_id: Uuid, m4b: &Path, name: &str) -> JobResult<Uuid> {
        let pool = &self.state.pool;
        let path = m4b.to_string_lossy().to_string();
        let size = tokio::fs::metadata(m4b).await.map_err(io)?.len() as i64;
        ingest_queue::create(
            pool,
            ingest_queue::IngestQueueInput {
                file_path: path.clone(),
                file_name: name.to_string(),
                file_size: size,
                source_format: "m4b".to_string(),
                source_metadata: Some(json!({"source_type": "full_cast"})),
                update_target_book_id: Some(book_id),
                status: "pending".to_string(),
                ..Default::default()
            },
        )
        .await?;
        crate::watcher::process_queue_item(
            pool,
            &self.state.config.storage_root,
            &self.state.library_events,
            self.state.nats.publisher(),
            &self.state.runtime_config,
            book_id,
            IngestOptions {
                file_mode: FileMode::Copy,
                entitlements: IngestEntitlements::AutoSync,
            },
        )
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("importing the audiobook failed: {e}")))?;
        repo::imported_file(pool, book_id, &path)
            .await?
            .ok_or_else(|| JobError::ExecutionFailed("the imported audiobook is missing".into()))
    }
}

fn io(e: std::io::Error) -> JobError {
    JobError::ExecutionFailed(e.to_string())
}

fn chapter_title(chapter_index: i32, title: Option<&str>) -> String {
    title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map_or_else(|| format!("Chapter {}", chapter_index + 1), str::to_string)
}

fn casting_error(e: CastingError) -> JobError {
    JobError::ExecutionFailed(e.to_string())
}

#[async_trait]
impl JobHandler for BookCastRenderHandler {
    fn job_type(&self) -> &'static str {
        "book_cast_render"
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
        let Some(cast) = book_casts::get(&self.state.pool, book_id).await? else {
            return Ok(json!({"status": "no_cast"}));
        };
        if cast.state != CastState::Rendering {
            return Ok(json!({"status": "skipped", "state": cast.state}));
        }
        match self.render(job_id, &cast, &cancel).await {
            Ok(v) => Ok(v),
            Err(e) => {
                book_casts::set_state(
                    &self.state.pool,
                    book_id,
                    CastState::Failed,
                    Some(&e.to_string()),
                )
                .await?;
                Err(e)
            }
        }
    }
}
