//! Gathers what `editions` needs to judge a book's files, and carries out
//! its plan: other editions move to books of their own, copies and partial
//! editions leave the library, and listeners' positions follow the audio
//! they were in.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use nats_jobs::Publisher;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::editions::{self, Action, EditionFile, Plan, Release, Sibling};
use super::probe::{self, AudioProbeResult};
use crate::jobs::JobContext;
use crate::repositories::Result;
use crate::repositories::audiobook_editions::{self, AudioPosition};
use crate::repositories::{audiobook_files, audiobook_transcripts, books};

pub type Probes = HashMap<Uuid, AudioProbeResult>;

#[derive(Default)]
pub struct Settled {
    pub created: Vec<Uuid>,
    pub joined: Vec<Uuid>,
    pub report: Vec<Value>,
}

impl Settled {
    pub fn changed(&self) -> bool {
        !self.report.is_empty()
    }

    pub fn other_books(&self) -> impl Iterator<Item = Uuid> + '_ {
        self.created.iter().chain(&self.joined).copied()
    }
}

fn parse_position(value: &str) -> Option<(Uuid, f64)> {
    let (file, seconds) = value.split_once(':')?;
    Some((file.parse().ok()?, seconds.parse().ok()?))
}

async fn probe_of(storage_root: &str, file_path: &str) -> Option<AudioProbeResult> {
    let full = super::path::resolve_file_path(storage_root, file_path);
    if !full.is_file() {
        return None;
    }
    probe::probe_audio(&full).await.ok()
}

async fn load(
    pool: &PgPool,
    storage_root: &str,
    book_id: Uuid,
    probes: Option<&Probes>,
) -> Result<(Vec<EditionFile>, Vec<Sibling>)> {
    let rows = audiobook_files::list_by_book(pool, book_id).await?;
    if rows.len() < 2 {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut listeners: HashMap<Uuid, usize> = HashMap::new();
    for p in audiobook_editions::audio_positions(pool, book_id).await? {
        if let Some((file, _)) = parse_position(&p.location_value) {
            *listeners.entry(file).or_default() += 1;
        }
    }
    let mut files = Vec::with_capacity(rows.len());
    for row in rows {
        let probed = match probes.and_then(|p| p.get(&row.id)) {
            Some(p) => Some(p.clone()),
            None => probe_of(storage_root, &row.file_path).await,
        };
        let Some(probed) = probed else { continue };
        let segments: Vec<(f64, String)> = audiobook_transcripts::get_for_file(pool, row.id)
            .await?
            .into_iter()
            .map(|t| (t.start_seconds, t.text))
            .collect();
        files.push(EditionFile {
            id: row.id,
            index: row.file_index,
            path: row.file_path,
            format: row.format,
            duration: row
                .duration_seconds
                .or(probed.duration_seconds)
                .unwrap_or(0.0),
            narrator: probed.narrator,
            album: probed.album,
            abridged: probed.is_abridged,
            bit_rate: probed.bit_rate,
            chaptered: !probed.chapters.is_empty(),
            listeners: listeners.get(&row.id).copied().unwrap_or(0),
            text: editions::shingles(&segments),
        });
    }
    let mut siblings = Vec::new();
    for s in audiobook_editions::siblings(pool, book_id).await? {
        if let Some(p) = probe_of(storage_root, &s.file_path).await {
            siblings.push(Sibling {
                book_id: s.book_id,
                release: Release::of(&s.format, p.narrator.as_deref()),
            });
        }
    }
    Ok((files, siblings))
}

fn describe(files: &[EditionFile], plan: &Plan) -> Vec<Value> {
    let path_of: HashMap<Uuid, &str> = files.iter().map(|f| (f.id, f.path.as_str())).collect();
    plan.moves
        .iter()
        .map(|m| {
            let (action, target) = match &m.action {
                Action::Split => ("split", None),
                Action::Join(book) => ("join", Some(*book)),
                Action::Drop => ("drop", None),
            };
            json!({
                "action": action,
                "target_book_id": target,
                "reason": m.reason,
                "narrator": m.narrator,
                "files": m.files.iter().map(|f| path_of.get(f).copied()).collect::<Vec<_>>(),
            })
        })
        .collect()
}

pub async fn inspect(pool: &PgPool, storage_root: &str, book_id: Uuid) -> Result<Vec<Value>> {
    let (files, siblings) = load(pool, storage_root, book_id, None).await?;
    Ok(describe(&files, &editions::plan(&files, &siblings)))
}

pub async fn settle(
    pool: &PgPool,
    storage_root: &str,
    book_id: Uuid,
    probes: Option<&Probes>,
) -> Result<Settled> {
    let (files, siblings) = load(pool, storage_root, book_id, probes).await?;
    let plan = editions::plan(&files, &siblings);
    if plan.is_empty() {
        return Ok(Settled::default());
    }
    let report = describe(&files, &plan);
    let leaving = plan.leaving();
    let staying: HashSet<Uuid> = files
        .iter()
        .map(|f| f.id)
        .filter(|id| !leaving.contains(id))
        .collect();
    let dropped: Vec<Uuid> = plan
        .moves
        .iter()
        .filter(|m| m.action == Action::Drop)
        .flat_map(|m| m.files.iter().copied())
        .collect();
    let fallback = files
        .iter()
        .filter(|f| staying.contains(&f.id))
        .min_by_key(|f| f.index)
        .map(|f| (f.id, 0.0));
    let land = |file: Uuid, seconds: f64| {
        if dropped.contains(&file) {
            editions::locate(&files, &plan, file, seconds, &staying).or(fallback)
        } else {
            Some((file, seconds))
        }
    };

    let positions: Vec<(AudioPosition, (Uuid, f64))> =
        audiobook_editions::audio_positions(pool, book_id)
            .await?
            .into_iter()
            .filter_map(|p| {
                let (file, seconds) = parse_position(&p.location_value)?;
                Some((p, land(file, seconds)?))
            })
            .collect();
    let bookmarks: Vec<(Uuid, (Uuid, f64))> = audiobook_editions::bookmarks_in(pool, &dropped)
        .await?
        .into_iter()
        .filter_map(|b| Some((b.id, land(b.audiobook_file_id, b.position_seconds)?)))
        .collect();

    let mut settled = Settled {
        report,
        ..Default::default()
    };
    let mut tx = pool.begin().await?;
    for m in &plan.moves {
        match m.action {
            Action::Split => {
                let id = audiobook_editions::copy_book(
                    &mut tx,
                    book_id,
                    m.narrator.as_deref(),
                    m.abridged,
                )
                .await?;
                audiobook_editions::move_files(&mut tx, &m.files, id).await?;
                settled.created.push(id);
            }
            Action::Join(target) => {
                audiobook_editions::move_files(&mut tx, &m.files, target).await?;
                settled.joined.push(target);
            }
            Action::Drop => {}
        }
    }
    let targets: Vec<Uuid> = positions
        .iter()
        .map(|(_, (f, _))| *f)
        .chain(bookmarks.iter().map(|(_, (f, _))| *f))
        .collect();
    let owner: HashMap<Uuid, Uuid> = audiobook_editions::owners(&mut *tx, &targets)
        .await?
        .into_iter()
        .collect();
    for (p, (file, seconds)) in &positions {
        if let Some(book) = owner.get(file) {
            audiobook_editions::set_position(
                &mut tx,
                p.state_id,
                *book,
                &format!("{file}:{seconds}"),
                None,
            )
            .await?;
        }
    }
    for (id, (file, seconds)) in &bookmarks {
        if let Some(book) = owner.get(file) {
            audiobook_editions::set_bookmark(&mut tx, *id, *book, *file, *seconds).await?;
        }
    }
    audiobook_editions::drop_files(&mut tx, &dropped).await?;
    tx.commit().await?;

    let mut touched = vec![book_id];
    touched.extend(settled.other_books());
    touched.sort();
    touched.dedup();
    for book in &touched {
        refresh(pool, *book).await?;
    }
    for book in &settled.created {
        copy_cover(storage_root, book_id, *book).await;
    }
    Ok(settled)
}

/// File order, the book's length and each listener's percentage all change
/// when files come or go.
async fn refresh(pool: &PgPool, book_id: Uuid) -> Result<()> {
    audiobook_files::reindex_file_order(pool, book_id).await?;
    let files = audiobook_files::list_by_book(pool, book_id).await?;
    let total: f64 = files.iter().filter_map(|f| f.duration_seconds).sum();
    if total > 0.0 {
        books::update(
            pool,
            book_id,
            books::BookUpdate {
                duration_seconds: Some(total as i32),
                ..Default::default()
            },
        )
        .await?;
    }
    let mut offset = 0.0;
    let mut starts = HashMap::new();
    for f in &files {
        starts.insert(f.id, offset);
        offset += f.duration_seconds.unwrap_or(0.0);
    }
    let mut tx = pool.begin().await?;
    for p in audiobook_editions::audio_positions(pool, book_id).await? {
        if let Some((file, seconds)) = parse_position(&p.location_value)
            && let Some(start) = starts.get(&file)
            && total > 0.0
        {
            let percent = ((start + seconds) / total * 100.0).clamp(0.0, 100.0);
            audiobook_editions::set_position(
                &mut tx,
                p.state_id,
                book_id,
                &p.location_value,
                Some(percent),
            )
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

async fn copy_cover(storage_root: &str, from: Uuid, to: Uuid) {
    let covers = Path::new(storage_root).join("covers");
    let source = covers.join(format!("{from}.jpg"));
    if source.is_file()
        && let Err(e) = tokio::fs::copy(&source, covers.join(format!("{to}.jpg"))).await
    {
        tracing::warn!(error = %e, book_id = %to, "failed to copy cover to the new edition");
    }
}

/// Settle the books that gained files with `job_type` and index everything
/// that changed.
pub async fn follow_up(
    pool: &PgPool,
    nats: &Publisher,
    book_id: Uuid,
    settled: &Settled,
    job_type: &str,
) {
    crate::jobs::enqueue_book_index(pool, nats, book_id).await;
    for book in settled.other_books() {
        crate::jobs::enqueue_book_index(pool, nats, book).await;
        let context = JobContext::for_entity("book", book, "Settling audiobook edition")
            .with_display_opt(crate::jobs::fetch_job_display(pool, "book", book).await);
        if let Err(e) =
            crate::jobs::enqueue(pool, nats, job_type, json!({ "book_id": book }), context).await
        {
            tracing::warn!(error = %e, book_id = %book, "failed to enqueue follow-up for edition");
        }
    }
}
