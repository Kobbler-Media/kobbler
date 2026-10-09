//! Ebook repair job handler.
//!
//! Re-derives converted book files that the current pipeline would produce
//! better than the one that made them. Two things bring a file here:
//!
//! - **damage.** boko 0.5.0 dropped every KF8 stylesheet flow when importing
//!   AZW3/MOBI, so each converted book linked a stylesheet that was not in the
//!   archive — a comic rendered as a column of unstyled images, and nothing
//!   logged a word about it. Conversion now catches that as it happens
//!   (`ConversionPipeline::native_output_is_broken`), but books converted
//!   before the fix are still on disk in that state.
//! - **a lost fixed layout.** A Kindle fixed-layout book declares itself in the
//!   AZW3's EXTH header, and a conversion made before kob-ebook read that header
//!   says nothing about it. The file is whole and valid; every page just draws
//!   at its literal pixel size in the middle of the screen.
//! - **age.** A KEPUB stamped with an older `kob_ebook::KEPUB_GENERATION` was
//!   made by a kepubifier that has since improved. Nothing else would ever
//!   revisit it: every other path treats a KEPUB that exists as a KEPUB that
//!   is done.
//! - **staleness.** The original a file was converted from has since changed on
//!   disk — a re-download, a fixed scan, a replaced file. The copy is whole and
//!   valid, but it is a copy of a book that no longer exists, and every reader
//!   path that prefers a derived file keeps serving it.
//! - **gone.** The derived file has vanished from disk — cleaned up, a volume
//!   that was not restored. Used to be out of scope on the theory that missing
//!   files are availability's business; but for a file the pipeline made, the
//!   original is still there, so re-making it is the remedy.
//!
//! Two modes, one job type:
//!
//! - **sweep** (no `book_id`) — read every derived file, test it, and enqueue a
//!   repair for each book that needs one. Reports what it found.
//! - **repair** (`book_id` set) — re-convert that book's files from an original
//!   source and replace them in place.
//!
//! Only files the pipeline itself produced are touched. A malformed EPUB that
//! came in from a media root is the user's own copy of the book, and this job
//! will report it but never overwrite it.
//!
//! Not scheduled. Re-making a file costs the reader a re-download and possibly
//! their place in the book, so it happens when someone asks for it.

use crate::convert::{ConversionPipeline, conversion_regressed, epub_defects};
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobContext, JobError, JobResult};
use crate::repositories::{book_files, books, ebook_segments, media_root_files};
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EbookRepairPayload {
    /// Repair one book. Absent means sweep the whole library.
    #[serde(default)]
    pub book_id: Option<Uuid>,
    /// Report what would be repaired without writing anything.
    #[serde(default)]
    pub dry_run: bool,
}

pub struct EbookRepairHandler {
    pool: PgPool,
    storage_root: String,
    nats_publisher: Publisher,
    fs_semaphore: Arc<Semaphore>,
}

impl EbookRepairHandler {
    pub fn new(
        pool: PgPool,
        storage_root: String,
        nats_publisher: Publisher,
        fs_semaphore: Arc<Semaphore>,
    ) -> Self {
        Self {
            pool,
            storage_root,
            nats_publisher,
            fs_semaphore,
        }
    }

    /// Read a derived file and say why it should be re-made, if it should.
    ///
    /// Reasons. The file is damaged — see [`epub_defects`]. It is a KEPUB an
    /// older kepubifier wrote, and re-running the current one would produce a
    /// better book — the generation is stamped in the file itself, so this
    /// holds for KEPUBs made before any of this existed: they read as
    /// generation 0. The original it was converted from has since changed on
    /// disk, so the copy is of a book that no longer exists. Or the file has
    /// gone missing from disk while the original is still there.
    ///
    /// Staleness is judged against the original the row recorded converting
    /// from, not against the best original the book has today: a better
    /// original arriving later is a change of source, not a repair, and
    /// flagging it would re-make the file from a book it was never converted
    /// from. Rows that recorded no source have no baseline to keep, so they
    /// are judged against the current conversion source instead.
    ///
    /// A file that has gone missing from disk is only reported when the
    /// original can still be read — otherwise the book has no copy at all,
    /// re-converting would not be the remedy, and that is availability's
    /// business.
    ///
    /// Judged against the file it was converted from, so a fault the original
    /// already had is not held against the copy. Without that, a book whose
    /// publisher forgot a stylesheet would be re-made on every single sweep,
    /// each pass producing an equally flawed file and costing the device
    /// another download.
    async fn inspect(&self, file: &book_files::DerivedFileInfo) -> Option<Vec<String>> {
        let full = crate::audio::path::resolve_file_path(&self.storage_root, &file.file_path);
        let is_kepub = file.format.eq_ignore_ascii_case("KEPUB");
        let recorded_mtime = file.source_mtime;
        let source = self.source_bytes(file).await;
        let baseline = self.baseline_source(file).await;

        let reasons = tokio::task::spawn_blocking(move || {
            let mut reasons = Vec::new();
            match std::fs::read(&full) {
                Ok(bytes) => {
                    reasons.extend(epub_defects(&bytes, source.as_deref()));
                    // Invisible to every other check: the file is whole and valid, and
                    // draws each page at a fraction of the screen because the layout
                    // the Kindle source declared was never carried across.
                    if let Some(src) = source.as_deref()
                        && kob_ebook::fixed_layout_lost(src, &bytes)
                    {
                        reasons
                            .push("source declares a fixed layout this file does not".to_string());
                    }
                    if is_kepub {
                        let generation = kob_ebook::kepub_generation(&bytes);
                        if generation < kob_ebook::KEPUB_GENERATION {
                            reasons.push(format!(
                                "built by kepubifier generation {generation}, current is {}",
                                kob_ebook::KEPUB_GENERATION
                            ));
                        }
                    }
                    reasons.extend(stale_reason(&full, recorded_mtime, baseline.as_ref()));
                }
                Err(_) => {
                    // The derived file is gone from disk. That is only a repair
                    // finding when the original is still there to convert from;
                    // otherwise the book has no copy at all, and re-converting
                    // would not be the remedy.
                    if source.is_some() {
                        reasons.push(
                            "derived file missing from disk; the original is still present"
                                .to_string(),
                        );
                    }
                }
            }
            reasons
        })
        .await
        .ok()?;

        (!reasons.is_empty()).then_some(reasons)
    }

    /// The original this file was converted from, as it stands on disk now:
    /// its mtime, the staleness baseline.
    ///
    /// The row's recorded source when it has one, otherwise the book's
    /// current conversion source. Absent when neither is on disk — in which
    /// case there is nothing to judge staleness against.
    async fn baseline_source(&self, file: &book_files::DerivedFileInfo) -> Option<OffsetDateTime> {
        // The recorded original first. Rows that never recorded one — KEPUBs
        // the watcher registered from disk, pre-baseline rows — fall back to
        // the book's current conversion source, so a changed original is
        // still caught. The first sweep after a source change re-makes the
        // file once; the repair records the baseline, so it settles.
        let rel = match file.source_path.as_deref() {
            Some(p) => p.to_string(),
            None => {
                let (_, path) =
                    book_files::get_conversion_source(&self.pool, file.book_id, file.id)
                        .await
                        .ok()
                        .flatten()?;
                path
            }
        };
        let full = crate::audio::path::resolve_file_path(&self.storage_root, &rel);
        let mtime = tokio::fs::metadata(&full).await.ok()?.modified().ok()?;
        Some(media_root_files::mtime_from_system_time(mtime))
    }

    /// The file this one was converted from, if there still is one.
    ///
    /// Absent for anything we cannot trace back — in which case the derived
    /// file is judged on its own, which is the stricter reading and the right
    /// one when there is nothing to inherit from.
    async fn source_bytes(&self, file: &book_files::DerivedFileInfo) -> Option<Vec<u8>> {
        let (_, source_path) = book_files::get_conversion_source(&self.pool, file.book_id, file.id)
            .await
            .ok()??;
        let full = crate::audio::path::resolve_file_path(&self.storage_root, &source_path);
        tokio::fs::read(&full).await.ok()
    }

    /// Walk every derived file, enqueueing a repair per book that needs one.
    async fn sweep(&self, dry_run: bool, cancel: &CancellationToken) -> JobResult<Value> {
        let files = book_files::list_derived(&self.pool).await?;
        let total = files.len();

        let mut findings = Vec::new();
        let mut books = std::collections::HashSet::new();
        for file in files {
            nats_jobs::check_cancelled(cancel)?;
            let Some(reasons) = self.inspect(&file).await else {
                continue;
            };
            warn!(
                book = %file.book_title,
                format = %file.format,
                path = %file.file_path,
                reasons = ?reasons,
                "derived ebook needs re-making"
            );
            findings.push(json!({
                "book_id": file.book_id,
                "title": file.book_title,
                "format": file.format,
                "path": file.file_path,
                "reasons": reasons,
            }));
            books.insert((file.book_id, file.book_title));
        }

        if !dry_run {
            for (book_id, title) in &books {
                let context = JobContext::for_entity(
                    "book",
                    *book_id,
                    format!("Re-converting ebook — {title}"),
                );
                if let Err(e) = crate::jobs::enqueue(
                    &self.pool,
                    &self.nats_publisher,
                    "ebook_repair",
                    json!({ "book_id": book_id }),
                    context,
                )
                .await
                {
                    warn!(error = %e, book_id = %book_id, "failed to enqueue ebook repair");
                }
            }
        }

        info!(
            checked = total,
            needs_rework = findings.len(),
            books = books.len(),
            dry_run,
            "Ebook repair sweep complete"
        );

        Ok(json!({
            "checked": total,
            "needs_rework": findings,
            "books_queued": if dry_run { 0 } else { books.len() },
            "dry_run": dry_run,
        }))
    }

    /// Re-convert the files of one book that need it.
    async fn repair_book(&self, book_id: Uuid, cancel: &CancellationToken) -> JobResult<Value> {
        let files: Vec<_> = book_files::list_derived(&self.pool)
            .await?
            .into_iter()
            .filter(|f| f.book_id == book_id)
            .collect();

        let mut repaired = Vec::new();
        let mut failed = Vec::new();
        let mut skipped = Vec::new();

        for file in files {
            nats_jobs::check_cancelled(cancel)?;
            let Some(reasons) = self.inspect(&file).await else {
                continue;
            };

            let source = book_files::get_conversion_source(&self.pool, book_id, file.id).await?;
            let Some((source_format, source_path)) = source else {
                // Nothing left to convert from — the derived file is all there
                // is. Overwriting it with a conversion of itself would only
                // destroy what is still readable.
                warn!(
                    book_id = %book_id,
                    format = %file.format,
                    "derived file needs re-making but has no original to convert from"
                );
                skipped.push(json!({ "format": file.format, "reason": "no source file" }));
                continue;
            };

            let source_full =
                crate::audio::path::resolve_file_path(&self.storage_root, &source_path);
            if !source_full.exists() {
                // The source row points at a file that is not on disk — a
                // media root that is down, a file that was removed. There is
                // nothing to convert from; the availability check owns that.
                skipped.push(json!({
                    "format": file.format,
                    "reason": "source file not on disk"
                }));
                continue;
            }

            match self
                .reconvert(book_id, &file.format, &file.file_path, &source_path)
                .await
            {
                Ok(size) => {
                    // Without this the device keeps the copy it already has:
                    // same path, same RevisionId, nothing to re-fetch. The
                    // staleness baseline moves with the file: re-made from
                    // this source at this mtime, so the next sweep does not
                    // flag the same change again.
                    let source_mtime = tokio::fs::metadata(&source_full)
                        .await
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .map(media_root_files::mtime_from_system_time);
                    book_files::mark_rewritten(
                        &self.pool,
                        file.id,
                        size,
                        Some(&source_path),
                        source_mtime,
                    )
                    .await?;
                    if !file.available {
                        // The file was just rebuilt; the row's unavailable
                        // state is stale until the next availability pass.
                        let _ = book_files::set_available(&self.pool, file.id).await;
                    }
                    info!(
                        book_id = %book_id,
                        format = %file.format,
                        from = %source_format,
                        reasons = ?reasons,
                        "re-converted ebook"
                    );
                    repaired.push(json!({
                        "format": file.format,
                        "from": source_format,
                        "size_bytes": size,
                        "reasons": reasons,
                    }));
                }
                Err(e) => {
                    warn!(error = %e, book_id = %book_id, format = %file.format, "ebook repair failed");
                    failed.push(json!({ "format": file.format, "error": e.to_string() }));
                }
            }
        }

        // The text index was built from bytes that have just been replaced.
        if !repaired.is_empty() && ebook_segments::has_segments(&self.pool, book_id).await? {
            let title = books::get_title(&self.pool, book_id)
                .await
                .unwrap_or_default()
                .unwrap_or_default();
            crate::jobs::enqueue_ebook_extract(&self.pool, &self.nats_publisher, book_id, &title)
                .await;
        }

        Ok(json!({
            "book_id": book_id,
            "repaired": repaired,
            "failed": failed,
            "skipped": skipped,
        }))
    }

    /// Convert `source_path` into `target_format` and put it where the damaged
    /// file was, so every path already recorded against the book stays valid.
    ///
    /// Written to a temporary name and renamed over the original only once the
    /// conversion has succeeded: a failure here must not leave the book with
    /// less than it started with.
    async fn reconvert(
        &self,
        book_id: Uuid,
        target_format: &str,
        dest_path: &str,
        source_path: &str,
    ) -> Result<i64, JobError> {
        let dest = crate::audio::path::resolve_file_path(&self.storage_root, dest_path);
        let source = crate::audio::path::resolve_file_path(&self.storage_root, source_path);
        let dir = dest
            .parent()
            .ok_or_else(|| JobError::ExecutionFailed(format!("no parent dir for {dest_path}")))?;

        let staging = dir.join(format!(".kobbler-repair-{book_id}"));
        tokio::fs::create_dir_all(&staging)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("staging dir: {e}")))?;

        let result = self
            .convert_into(target_format, &source, &staging, &dest)
            .await;
        let _ = tokio::fs::remove_dir_all(&staging).await;
        result
    }

    async fn convert_into(
        &self,
        target_format: &str,
        source: &Path,
        staging: &Path,
        dest: &Path,
    ) -> Result<i64, JobError> {
        let pipeline = ConversionPipeline::new();
        let produced: PathBuf = match target_format.to_uppercase().as_str() {
            "KEPUB" => pipeline.convert_to_kepub(source, staging).await,
            "EPUB" => pipeline.convert_to_epub(source, staging).await,
            other => {
                return Err(JobError::ExecutionFailed(format!(
                    "cannot repair format {other}"
                )));
            }
        }
        .map_err(|e| JobError::ExecutionFailed(format!("conversion failed: {e}")))?;

        // Only replace the old file if the new one is genuinely no worse.
        let bytes = tokio::fs::read(&produced)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("reading conversion output: {e}")))?;
        let size = bytes.len() as i64;
        let source_bytes = tokio::fs::read(source)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("reading conversion source: {e}")))?;

        let regression =
            tokio::task::spawn_blocking(move || conversion_regressed(&bytes, &source_bytes))
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("join error: {e}")))?;

        if let Some(reason) = regression {
            return Err(JobError::ExecutionFailed(format!(
                "{reason}; keeping the existing file"
            )));
        }

        tokio::fs::rename(&produced, dest)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("replacing the old file: {e}")))?;

        Ok(size)
    }
}

#[async_trait]
impl JobHandler for EbookRepairHandler {
    fn job_type(&self) -> &'static str {
        "ebook_repair"
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
        nats_jobs::check_cancelled(&cancel)?;
        let _fs_permit = self
            .fs_semaphore
            .acquire()
            .await
            .map_err(|_| JobError::ExecutionFailed("FS semaphore closed".into()))?;

        let payload: EbookRepairPayload = serde_json::from_value(payload)?;
        match payload.book_id {
            Some(book_id) => self.repair_book(book_id, &cancel).await,
            None => self.sweep(payload.dry_run, &cancel).await,
        }
    }
}

/// Say why a derived file is stale relative to the original it was converted
/// from, if it is.
///
/// The baseline is the source's mtime recorded at conversion time. Rows made
/// before that recording existed fall back to the file's own mtime: a source
/// newer than the file means the original changed after the copy was made.
///
/// A change within a couple of seconds is treated as no change at all — the
/// same tolerance the media-root scan gives NAS clock skew — so a file that
/// is re-touched without a real edit does not cost the reader a re-download
/// on every sweep.
fn stale_reason(
    full: &Path,
    recorded: Option<OffsetDateTime>,
    baseline: Option<&OffsetDateTime>,
) -> Vec<String> {
    let Some(current) = baseline else {
        return Vec::new();
    };
    let stale = match recorded {
        Some(recorded) => (*current - recorded).whole_seconds().abs() > 2,
        None => std::fs::metadata(full)
            .ok()
            .and_then(|m| m.modified().ok())
            .is_some_and(|mtime| media_root_files::mtime_from_system_time(mtime) < *current),
    };
    if stale {
        vec!["source file changed since this was converted".to_string()]
    } else {
        Vec::new()
    }
}
