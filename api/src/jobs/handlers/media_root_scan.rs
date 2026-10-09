//! Media root scan job handler.
//!
//! Walks a media root directory, discovers media files, and creates/updates
//! `media_root_files` records. Marks files no longer on disk as unavailable.
//! Enqueues `media_root_interpret` for newly discovered files.

use crate::convert::EbookFormat;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobContext, JobError, JobResult};
use crate::repositories::{
    external_subtitles, media_root_dir_mtimes, media_root_files, media_roots, movie_files,
    tv_episode_files,
};
use crate::routes::ingest::AUDIOBOOK_EXTENSIONS;
use crate::routes::ingest::parse_subtitle_filename_hints;
use crate::search::{SearchBackend, batch_index_media_root_files};
use crate::sidecar;
use crate::sidecar::images as sidecar_images;
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use time::OffsetDateTime;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

/// Video file extensions to scan for.
const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts", "m2ts",
];

/// Subtitle file extensions to discover as sidecar files.
const SUBTITLE_EXTENSIONS: &[&str] = &["srt", "ass", "ssa", "sub", "vtt", "sup", "idx"];

/// Payload for media_root_scan jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRootScanPayload {
    pub media_root_id: Uuid,
    /// If set, only scan this one file (for watcher events).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub single_file: Option<String>,
}

pub struct MediaRootScanHandler {
    pool: PgPool,
    nats: Publisher,
    search: Arc<dyn SearchBackend>,
    fs_semaphore: Arc<Semaphore>,
    runtime_config: crate::RuntimeConfigManager,
    initial_scan_complete: Arc<AtomicBool>,
}

impl MediaRootScanHandler {
    pub fn new(
        pool: PgPool,
        nats: Publisher,
        search: Arc<dyn SearchBackend>,
        fs_semaphore: Arc<Semaphore>,
        runtime_config: crate::RuntimeConfigManager,
        initial_scan_complete: Arc<AtomicBool>,
    ) -> Self {
        Self {
            pool,
            nats,
            search,
            fs_semaphore,
            runtime_config,
            initial_scan_complete,
        }
    }
}

#[async_trait]
impl JobHandler for MediaRootScanHandler {
    fn job_type(&self) -> &'static str {
        "media_root_scan"
    }

    fn max_concurrent(&self) -> usize {
        self.runtime_config.job_concurrency("media_root_scan", 2)
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let _fs_permit = self
            .fs_semaphore
            .acquire()
            .await
            .map_err(|_| JobError::ExecutionFailed("FS semaphore closed".into()))?;
        let payload: MediaRootScanPayload = serde_json::from_value(payload)?;
        nats_jobs::check_cancelled(&cancel)?;

        let root = match media_roots::get_by_id(&self.pool, payload.media_root_id).await? {
            Some(r) => r,
            None => {
                tracing::warn!(
                    "Media root {} no longer exists, skipping",
                    payload.media_root_id
                );
                return Ok(json!({"skipped": true, "reason": "media_root_deleted"}));
            }
        };

        if !root.enabled {
            return Ok(json!({"skipped": true, "reason": "disabled"}));
        }

        if !media_roots::check_available(&self.pool, &root).await? {
            return Ok(json!({"deferred": true, "reason": "media_root_unavailable"}));
        }
        let root_path = Path::new(&root.path);

        media_roots::set_scanning(&self.pool, root.id).await?;

        // Walk the directory recursively
        let walk = if let Some(single_file) = &payload.single_file {
            // Single file mode (from watcher) — always process, no skip logic
            let mut w = WalkResult {
                discovered_paths: Vec::new(),
                changed_file_ids: Vec::new(),
                extras_file_ids: Vec::new(),
                new_files: 0,
                updated_files: 0,
                skipped_unchanged: 0,
                scan_complete: true,
            };
            let full_path = root_path.join(single_file);
            let fp_is_file = {
                let fp = full_path.clone();
                tokio::task::spawn_blocking(move || fp.is_file())
                    .await
                    .unwrap_or(false)
            };
            if fp_is_file && is_media_file(&full_path, &root.media_type) {
                match process_file(&self.pool, root.id, root_path, &full_path, &root.media_type)
                    .await
                {
                    Ok((file_id, is_new)) => {
                        if is_new {
                            w.new_files += 1;
                        } else {
                            w.updated_files += 1;
                        }
                        w.changed_file_ids.push(file_id);
                        w.discovered_paths
                            .push(full_path.to_string_lossy().to_string());
                    }
                    Err(e) => {
                        warn!(path = %full_path.display(), error = %e, "Failed to process file")
                    }
                }
            }
            w
        } else {
            // Pre-load known file state to skip unchanged files during walk
            let known_files = media_root_files::get_known_file_state(&self.pool, root.id).await?;
            // Pre-load cached directory mtimes to skip unchanged directories during walk
            let dir_mtimes = media_root_dir_mtimes::load_all(&self.pool, root.id)
                .await
                .unwrap_or_default();

            // Full directory scan
            let w = match walk_directory(
                root_path,
                root.id,
                &root.media_type,
                &self.pool,
                &known_files,
                dir_mtimes,
            )
            .await
            {
                Ok(w) => w,
                Err(e) => {
                    error!(root = %root.path, error = %e, "Error walking directory");
                    let next_retry = OffsetDateTime::now_utc()
                        + time::Duration::minutes(std::cmp::min(
                            60,
                            2_i64.pow(root.error_count as u32),
                        ));
                    media_roots::set_scan_error(&self.pool, root.id, &e.to_string(), next_retry)
                        .await?;
                    return Err(JobError::ExecutionFailed(e.to_string()));
                }
            };

            // Only mark missing files when the walk completed fully — partial scans
            // haven't visited every file so discovered_paths is incomplete.
            if w.scan_complete {
                let marked = media_root_files::mark_missing_unavailable(
                    &self.pool,
                    root.id,
                    &w.discovered_paths,
                )
                .await?;

                if marked > 0 {
                    info!(root = %root.path, marked_unavailable = marked, "Marked missing files as unavailable");

                    // Propagate unavailability to entity file tables (movie_files,
                    // tv_episode_files, book_files, audiobook_files) so backfill
                    // jobs don't queue work for files that no longer exist on disk.
                    let propagated = media_root_files::propagate_unavailable_to_entity_files(
                        &self.pool, root.id,
                    )
                    .await?;
                    if propagated > 0 {
                        info!(
                            root = %root.path,
                            entity_files_marked = propagated,
                            "Propagated unavailability to entity file tables"
                        );
                    }
                }
            }

            // Auto-ignore files from extras directories
            if !w.extras_file_ids.is_empty() {
                let extras_count = w.extras_file_ids.len();
                for file_id in &w.extras_file_ids {
                    let _ = media_root_files::set_ignored(&self.pool, *file_id).await;
                }
                info!(root = %root.path, count = extras_count, "Auto-ignored files in extras directories");
            }

            w
        };

        // Only mark scan as fully completed when the walk visited every file.
        // Partial scans (batch limit hit) keep the "scanning" state so the
        // re-enqueued job continues.
        if walk.scan_complete {
            media_roots::set_scan_completed(&self.pool, root.id).await?;

            // Signal that at least one full scan has completed this boot,
            // so backfill handlers can start safely.
            if !self.initial_scan_complete.load(Ordering::Relaxed) {
                self.initial_scan_complete.store(true, Ordering::Release);
                info!("Initial media root scan complete — backfill handlers unblocked");
            }

            // Discover sidecar subtitle files for already-matched video files
            if root.media_type != "book" && root.media_type != "audiobook" {
                let subs_found =
                    discover_sidecar_subtitles(&self.pool, &root.path, &root.media_type).await;
                if subs_found > 0 {
                    info!(root = %root.path, subs_found, "Discovered sidecar subtitle files");
                }
            }
        }

        // If there are new/pending files, enqueue interpretation
        let pending = media_root_files::get_pending(&self.pool, root.id, 1).await?;
        if !pending.is_empty() {
            let display = crate::jobs::fetch_job_display(&self.pool, "media_root", root.id).await;
            let context = JobContext::for_entity(
                "media_root",
                root.id,
                format!("Interpreting files in '{}'", root.name),
            )
            .with_display_opt(display);
            if let Err(e) = crate::jobs::enqueue_routine(
                &self.pool,
                &self.nats,
                "media_root_interpret",
                json!({"media_root_id": root.id}),
                context,
            )
            .await
            {
                warn!(error = %e, "Failed to enqueue media_root_interpret");
            }
        }

        // Index only changed files in search (best-effort), in batches: a first
        // scan or a mass rename can change thousands of files at once.
        if !walk.changed_file_ids.is_empty() {
            let mut indexed = 0usize;
            for chunk in walk.changed_file_ids.chunks(1000) {
                if let Err(e) =
                    batch_index_media_root_files(self.search.as_ref(), &self.pool, chunk).await
                {
                    warn!(files = chunk.len(), error = %e, "Failed to index media root files in search");
                } else {
                    indexed += chunk.len();
                }
            }
            if indexed > 0 {
                debug!(
                    indexed,
                    total = walk.changed_file_ids.len(),
                    "Indexed changed files in search"
                );
            }
        }

        // If the scan was partial (batch limit hit), re-enqueue to continue.
        // The next scan will skip already-processed files via the known_files check.
        if !walk.scan_complete {
            // Stamp progress so the interval gate treats the in-flight
            // continuation as a recent successful scan.
            let _ = media_roots::touch_last_scan_completed_at(&self.pool, root.id).await;
            info!(
                root = %root.path,
                new = walk.new_files,
                updated = walk.updated_files,
                "Scan batch complete, re-enqueueing for remaining files"
            );
            let display = crate::jobs::fetch_job_display(&self.pool, "media_root", root.id).await;
            let context = JobContext::for_entity(
                "media_root",
                root.id,
                format!("Continuing scan of '{}'", root.name),
            )
            .with_display_opt(display);
            if let Err(e) = crate::jobs::enqueue_routine(
                &self.pool,
                &self.nats,
                "media_root_scan",
                json!({"media_root_id": root.id}),
                context,
            )
            .await
            {
                warn!(error = %e, "Failed to re-enqueue media_root_scan");
            }
        }

        info!(
            root = %root.path,
            new = walk.new_files,
            updated = walk.updated_files,
            skipped = walk.skipped_unchanged,
            total = walk.discovered_paths.len(),
            scan_complete = walk.scan_complete,
            "Media root scan complete"
        );

        Ok(json!({
            "new_files": walk.new_files,
            "updated_files": walk.updated_files,
            "skipped_unchanged": walk.skipped_unchanged,
            "total_files": walk.discovered_paths.len(),
            "scan_complete": walk.scan_complete,
        }))
    }
}

/// Maximum number of files to process (upsert) per scan batch.
/// Unchanged files (skipped via known_files check) don't count toward this limit.
/// After hitting this limit, the handler re-enqueues itself to continue.
const SCAN_BATCH_SIZE: u64 = 5000;

/// Mutable outputs accumulated during a directory walk.
struct WalkResult {
    discovered_paths: Vec<String>,
    changed_file_ids: Vec<Uuid>,
    /// File IDs that were inside an extras directory and should be auto-ignored.
    extras_file_ids: Vec<Uuid>,
    new_files: u64,
    updated_files: u64,
    /// Files skipped because (size, mtime) matched known state.
    skipped_unchanged: u64,
    /// Whether the walk completed (visited every file) or was cut short by the batch limit.
    /// When false, mark_missing_unavailable must be skipped since not all paths were discovered.
    scan_complete: bool,
}

/// A file entry discovered during the filesystem walk (Phase 1).
struct DiscoveredEntry {
    path: std::path::PathBuf,
    in_extras: bool,
}

/// Recursively walk a directory and process media files.
///
/// Phase 1: Walk the filesystem in spawn_blocking to collect all entries.
/// Phase 2: Process collected entries in async context (DB ops, sidecar reads).
///
/// Files whose (size, mtime) match `known_files` are skipped — their path is
/// still recorded in `discovered_paths` (needed for mark_missing) but no DB
/// upsert, sidecar read, or image discovery is performed.
async fn walk_directory(
    root_path: &Path,
    media_root_id: Uuid,
    media_type: &str,
    pool: &PgPool,
    known_files: &HashMap<String, media_root_files::KnownFileState>,
    dir_mtimes: HashMap<String, OffsetDateTime>,
) -> Result<WalkResult, Box<dyn std::error::Error + Send + Sync>> {
    // Phase 1: Walk filesystem on blocking thread pool to avoid starving tokio workers.
    let root_owned = root_path.to_owned();
    let media_type_owned = media_type.to_string();
    let known_files_clone = known_files.clone();

    let (entries, all_discovered_paths, skipped_unchanged, mtime_upserts, mtime_deletes) =
        tokio::task::spawn_blocking(move || {
            walk_directory_sync(
                &root_owned,
                &media_type_owned,
                &known_files_clone,
                dir_mtimes,
            )
        })
        .await
        .map_err(|e| {
            Box::new(std::io::Error::other(format!("Join error: {e}")))
                as Box<dyn std::error::Error + Send + Sync>
        })??;

    // Best-effort persist of directory-mtime cache updates. Failures here are
    // not fatal — worst case is the next scan re-walks a directory it could
    // have skipped.
    if let Err(e) =
        media_root_dir_mtimes::apply_updates(pool, media_root_id, &mtime_upserts, &mtime_deletes)
            .await
    {
        warn!(error = %e, "Failed to persist media_root_dir_mtimes updates");
    }

    // Phase 2: Process collected entries in async context for DB/sidecar operations.
    let mut result = WalkResult {
        discovered_paths: all_discovered_paths,
        changed_file_ids: Vec::new(),
        extras_file_ids: Vec::new(),
        new_files: 0,
        updated_files: 0,
        skipped_unchanged,
        scan_complete: true,
    };
    let files_processed_limit = SCAN_BATCH_SIZE;

    for entry in entries {
        match process_file(pool, media_root_id, root_path, &entry.path, media_type).await {
            Ok((file_id, is_new)) => {
                if is_new {
                    result.new_files += 1;
                    result.changed_file_ids.push(file_id);
                } else {
                    result.updated_files += 1;
                }
                if entry.in_extras {
                    result.extras_file_ids.push(file_id);
                }

                // Check batch limit — stop early if we've processed enough
                let total_processed = result.new_files + result.updated_files;
                if total_processed >= files_processed_limit {
                    result.scan_complete = false;
                    return Ok(result);
                }
            }
            Err(e) => {
                debug!(path = %entry.path.display(), error = %e, "Failed to process file");
            }
        }
    }

    Ok(result)
}

/// Synchronous filesystem walk (runs on blocking thread pool).
///
/// Returns (changed_entries, all_discovered_paths, skipped_unchanged_count,
/// dir_mtime_upserts, dir_mtime_deletes).
///
/// The dir-mtime shortcut: when a directory's on-disk mtime matches the
/// cached value (within 2s to tolerate clock skew), we skip read_dir on
/// that directory — POSIX dir mtimes change only when direct children are
/// added/removed/renamed, so the file set is identical to last scan. We
/// still need to recurse into subdirectories because their own contents
/// may have changed, which we discover via the cache itself (subdirs are
/// cached siblings).
#[allow(clippy::type_complexity)]
fn walk_directory_sync(
    root_path: &Path,
    media_type: &str,
    known_files: &HashMap<String, media_root_files::KnownFileState>,
    dir_mtimes: HashMap<String, OffsetDateTime>,
) -> Result<
    (
        Vec<DiscoveredEntry>,
        Vec<String>,
        u64,
        Vec<(String, OffsetDateTime)>,
        Vec<String>,
    ),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let mut changed_entries = Vec::new();
    let mut all_discovered_paths = Vec::new();
    let mut skipped_unchanged: u64 = 0;
    let mut mtime_upserts: Vec<(String, OffsetDateTime)> = Vec::new();
    let mut visited_rel_dirs: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Index known files by their parent directory (absolute path string).
    // Populated lazily only on shortcut hit to avoid the cost when the
    // cache is empty (e.g. first scan after deployment).
    let mut known_files_by_dir: Option<HashMap<String, Vec<String>>> = None;

    // Index cached subdirectories by their parent relative path so that
    // when we shortcut a directory we know which children to enqueue
    // without calling read_dir.
    let mut cache_children: HashMap<String, Vec<String>> = HashMap::new();
    for rel in dir_mtimes.keys() {
        // The root entry (rel="") has no parent — skip it to avoid a
        // self-reference when walking cache_children.
        if rel.is_empty() {
            continue;
        }
        // Parent of rel = everything up to the last '/' (or "" for top-level).
        let parent = match rel.rsplit_once('/') {
            Some((p, _)) => p.to_string(),
            None => String::new(),
        };
        cache_children.entry(parent).or_default().push(rel.clone());
    }

    let rel_of = |dir: &Path| -> String {
        dir.strip_prefix(root_path)
            .ok()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    };

    // Stack entries: (path, is_inside_extras_dir)
    let mut stack: Vec<(std::path::PathBuf, bool)> = vec![(root_path.to_path_buf(), false)];

    while let Some((dir, in_extras)) = stack.pop() {
        // Attempt the dir-mtime shortcut: if we have a cached mtime and it
        // matches (within 2s of tolerance for NAS clock skew), skip read_dir.
        let rel = rel_of(&dir);
        visited_rel_dirs.insert(rel.clone());

        let cur_mtime_opt: Option<OffsetDateTime> = std::fs::metadata(&dir)
            .ok()
            .and_then(|m| m.modified().ok())
            .map(media_root_files::mtime_from_system_time);

        if let (Some(cached), Some(cur)) = (dir_mtimes.get(&rel), cur_mtime_opt) {
            let delta = (cur - *cached).whole_seconds().abs();
            if delta <= 2 {
                // Shortcut: this directory's file set is unchanged.
                //
                // 1. Populate all_discovered_paths from known_files for every
                //    file directly in this directory. This keeps
                //    mark_missing_unavailable correct.
                // 2. Enqueue cached subdirectories so we still check them.
                let dir_str = dir.to_string_lossy().to_string();
                let idx = known_files_by_dir.get_or_insert_with(|| {
                    let mut m: HashMap<String, Vec<String>> = HashMap::new();
                    for p in known_files.keys() {
                        if let Some((parent, _)) = p.rsplit_once('/') {
                            m.entry(parent.to_string()).or_default().push(p.clone());
                        }
                    }
                    m
                });
                if let Some(files) = idx.get(&dir_str) {
                    for f in files {
                        all_discovered_paths.push(f.clone());
                        skipped_unchanged += 1;
                    }
                }

                if let Some(child_rels) = cache_children.get(&rel) {
                    for child_rel in child_rels {
                        let child_name = child_rel
                            .rsplit_once('/')
                            .map(|(_, n)| n)
                            .unwrap_or(child_rel.as_str());
                        if child_name.starts_with('.') || child_name.starts_with('@') {
                            continue;
                        }
                        let is_extras = in_extras || child_name.eq_ignore_ascii_case("extras");
                        stack.push((root_path.join(child_rel), is_extras));
                    }
                }
                continue;
            }
        }

        // No shortcut — read the directory and record its current mtime.
        if let Some(cur) = cur_mtime_opt {
            mtime_upserts.push((rel.clone(), cur));
        }

        let entries = std::fs::read_dir(&dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;

            if file_type.is_dir() {
                let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if dir_name.starts_with('.') || dir_name.starts_with('@') {
                    continue;
                }
                let is_extras = in_extras || dir_name.eq_ignore_ascii_case("extras");
                stack.push((path, is_extras));
            } else if file_type.is_file() {
                let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if file_name.starts_with('.') {
                    continue;
                }
                if !is_media_file(&path, media_type) {
                    continue;
                }

                let path_str = path.to_string_lossy().to_string();

                // Check if file is unchanged since last scan
                if let Some((known_size, known_mtime, _known_fp)) = known_files.get(&path_str)
                    && let Ok(metadata) = std::fs::metadata(&path)
                {
                    let cur_size = metadata.len() as i64;
                    let cur_mtime = metadata
                        .modified()
                        .ok()
                        .map(media_root_files::mtime_from_system_time);
                    if *known_size == Some(cur_size) && *known_mtime == cur_mtime {
                        all_discovered_paths.push(path_str);
                        skipped_unchanged += 1;
                        continue;
                    }
                }

                all_discovered_paths.push(path_str);
                changed_entries.push(DiscoveredEntry { path, in_extras });
            }
        }
    }

    // Directories previously cached but no longer visited should be deleted
    // from the mtime cache so rename/deletion of parent folders doesn't leave
    // stale rows behind.
    let mtime_deletes: Vec<String> = dir_mtimes
        .keys()
        .filter(|k| !visited_rel_dirs.contains(*k))
        .cloned()
        .collect();

    Ok((
        changed_entries,
        all_discovered_paths,
        skipped_unchanged,
        mtime_upserts,
        mtime_deletes,
    ))
}

/// Process a single file - upsert into media_root_files, then discover sidecar metadata.
async fn process_file(
    pool: &PgPool,
    media_root_id: Uuid,
    _root_path: &Path,
    file_path: &Path,
    media_type: &str,
) -> Result<(Uuid, bool), Box<dyn std::error::Error + Send + Sync>> {
    let fp = file_path.to_owned();
    let fp2 = file_path.to_owned();
    let (metadata, fingerprint) = tokio::task::spawn_blocking(move || {
        let meta = std::fs::metadata(&fp)?;
        let fp_result = crate::watcher::compute_file_fingerprint(&fp);
        let fingerprint = match fp_result {
            Ok(fp) => Some(fp),
            Err(e) => {
                tracing::debug!(path = %fp.display(), error = %e, "Failed to compute fingerprint");
                None
            }
        };
        Ok::<_, std::io::Error>((meta, fingerprint))
    })
    .await
    .map_err(|e| {
        Box::new(std::io::Error::other(format!("Join error: {e}")))
            as Box<dyn std::error::Error + Send + Sync>
    })??;
    let file_size = metadata.len() as i64;
    let file_mtime = metadata
        .modified()
        .ok()
        .map(media_root_files::mtime_from_system_time);

    let file_name = fp2
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let fingerprint_for_rename = fingerprint.clone();
    let file_path_str = fp2.to_string_lossy().to_string();

    let input = media_root_files::MediaRootFileInput {
        media_root_id,
        file_path: file_path_str.clone(),
        file_name,
        file_size: Some(file_size),
        file_mtime,
        file_fingerprint: fingerprint,
    };

    let (id, is_new) = media_root_files::upsert_by_path(pool, input).await?;

    // Rename detection: if this is a new record and we have a fingerprint, check
    // if another record in this media root has the same fingerprint+size at a
    // different path. If so, transfer its entity link and metadata, then delete it.
    if is_new && let Some(ref fp) = fingerprint_for_rename {
        match media_root_files::find_renamed_source(
            pool,
            media_root_id,
            fp,
            file_size,
            &file_path_str,
        )
        .await
        {
            Ok(Some(source)) => {
                info!(
                    file_id = %id,
                    old_id = %source.id,
                    old_status = %source.match_status,
                    "Detected file rename, transferring metadata"
                );
                if let Err(e) =
                    media_root_files::transfer_from_renamed(pool, id, &file_path_str, &source).await
                {
                    warn!(file_id = %id, error = %e, "Failed to transfer renamed file metadata");
                }
            }
            Ok(None) => {}
            Err(e) => {
                debug!(file_id = %id, error = %e, "Rename detection query failed");
            }
        }
    }

    // Auto-attach to an existing entity-files row at the same path. This stops
    // re-scans (or fingerprint resets) from dragging files the user has already
    // matched back into the review queue — the entity_files tables are the
    // source of truth for "this file is part of book/movie/episode X".
    match media_root_files::attach_to_existing_entity_if_present(
        pool,
        id,
        &file_path_str,
        media_type,
    )
    .await
    {
        Ok(true) => {
            debug!(file_id = %id, path = %file_path_str, "Auto-attached to existing entity from scan");
        }
        Ok(false) => {}
        Err(e) => {
            debug!(file_id = %id, error = %e, "Auto-attach lookup failed");
        }
    }

    // Discover and parse adjacent sidecar files (NFO, metadata.json, metadata.opf)
    let sidecar_media_type = match media_type {
        "movie" => sidecar::MediaType::Movie,
        "tv" => sidecar::MediaType::TvEpisode,
        "book" | "audiobook" => sidecar::MediaType::Book,
        _ => sidecar::MediaType::Movie,
    };
    if let Some(meta) = sidecar::read_sidecar(file_path, sidecar_media_type).await {
        let meta_json = serde_json::to_value(&meta).unwrap_or_default();
        if let Err(e) = media_root_files::set_sidecar_metadata(pool, id, &meta_json).await {
            trace!(file_id = %id, error = %e, "Failed to store sidecar metadata");
        } else {
            trace!(file_id = %id, format = ?meta.source_format, "Stored sidecar metadata");
        }
    }

    // Discover adjacent image sidecar files (poster.jpg, fanart.jpg, cover.jpg, etc.)
    let images = match media_type {
        "movie" => {
            if let Some(dir) = file_path.parent() {
                sidecar_images::discover_movie_images(dir).await
            } else {
                vec![]
            }
        }
        "tv" => sidecar_images::discover_episode_images(file_path).await,
        "book" | "audiobook" => {
            if let Some(dir) = file_path.parent() {
                sidecar_images::discover_book_images(dir).await
            } else {
                vec![]
            }
        }
        _ => vec![],
    };
    if !images.is_empty() {
        let images_json = serde_json::to_value(&images).unwrap_or_default();
        if let Err(e) = media_root_files::set_sidecar_images(pool, id, &images_json).await {
            trace!(file_id = %id, error = %e, "Failed to store sidecar images");
        } else {
            trace!(file_id = %id, count = images.len(), "Stored sidecar images");
        }
    }

    Ok((id, is_new))
}

/// Check if a file is a media file appropriate for the given media type.
pub(crate) fn is_media_file(path: &Path, media_type: &str) -> bool {
    match media_type {
        "book" => {
            let format = EbookFormat::from_path(path);
            // Exclude KEPUB files - these are generated by our conversion pipeline
            // and should not be scanned as new source books
            !matches!(format, EbookFormat::Unknown | EbookFormat::Kepub)
        }
        "audiobook" => path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| AUDIOBOOK_EXTENSIONS.contains(&ext.to_lowercase().as_str())),
        _ => path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| VIDEO_EXTENSIONS.contains(&ext.to_lowercase().as_str())),
    }
}

/// Check if a file is a subtitle file.
fn is_subtitle_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| SUBTITLE_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
}

/// Discover sidecar subtitle files adjacent to already-matched video files.
///
/// For each movie_file or tv_episode_file within the root path, scans the same
/// directory for subtitle files matching the video base name and creates
/// `external_subtitles` records.
async fn discover_sidecar_subtitles(pool: &PgPool, root_path: &str, media_type: &str) -> u64 {
    use std::collections::HashMap;

    let mut found = 0u64;

    // Ensure the prefix ends with / for the LIKE query
    let prefix = if root_path.ends_with('/') {
        root_path.to_string()
    } else {
        format!("{root_path}/")
    };

    // (movie_file_id, episode_file_id) pair for a matched video file
    type FileRef = (Option<Uuid>, Option<Uuid>);

    // Key: (directory, video_stem) -> Vec<FileRef>
    let mut video_map: HashMap<(String, String), Vec<FileRef>> = HashMap::new();

    if media_type == "movie" || media_type == "movies" {
        match movie_files::list_refs_by_path_prefix(pool, &prefix).await {
            Ok(refs) => {
                for r in refs {
                    let p = Path::new(&r.file_path);
                    if let (Some(dir), Some(stem)) = (
                        p.parent().and_then(|d| d.to_str()),
                        p.file_stem().and_then(|s| s.to_str()),
                    ) {
                        video_map
                            .entry((dir.to_string(), stem.to_string()))
                            .or_default()
                            .push((Some(r.id), None));
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to list movie files for subtitle discovery");
            }
        }
    }

    if media_type == "tv" || media_type == "tv_show" || media_type == "tv_shows" {
        match tv_episode_files::list_refs_by_path_prefix(pool, &prefix).await {
            Ok(refs) => {
                for r in refs {
                    let p = Path::new(&r.file_path);
                    if let (Some(dir), Some(stem)) = (
                        p.parent().and_then(|d| d.to_str()),
                        p.file_stem().and_then(|s| s.to_str()),
                    ) {
                        video_map
                            .entry((dir.to_string(), stem.to_string()))
                            .or_default()
                            .push((None, Some(r.id)));
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to list episode files for subtitle discovery");
            }
        }
    }

    if video_map.is_empty() {
        return 0;
    }

    // For each directory that has video files, scan for subtitle files
    let mut scanned_dirs: std::collections::HashSet<String> = std::collections::HashSet::new();

    for ((dir, video_stem), file_ids) in &video_map {
        // Only scan each directory once; collect all subtitle files
        if scanned_dirs.insert(dir.clone()) {
            // We need to scan the dir, but we process per-video-stem below
        }

        let dir_path = Path::new(dir);
        let video_stem_owned = video_stem.clone();
        let dir_path_owned = dir_path.to_owned();
        // Collect matching subtitle files on the blocking pool
        let sub_entries: Vec<(std::path::PathBuf, String, Option<i64>)> =
            tokio::task::spawn_blocking(move || {
                let mut subs = Vec::new();
                let entries = match std::fs::read_dir(&dir_path_owned) {
                    Ok(e) => e,
                    Err(_) => return subs,
                };
                for entry in entries.flatten() {
                    let sub_path = entry.path();
                    if !is_subtitle_file(&sub_path) {
                        continue;
                    }
                    let sub_name = match sub_path.file_name().and_then(|n| n.to_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };
                    if !sub_name.starts_with(&video_stem_owned) {
                        continue;
                    }
                    let file_size = std::fs::metadata(&sub_path).ok().map(|m| m.len() as i64);
                    subs.push((sub_path, sub_name, file_size));
                }
                subs
            })
            .await
            .unwrap_or_default();

        for (sub_path, sub_name, file_size) in &sub_entries {
            let (language, is_forced, is_hi): (Option<String>, bool, bool) =
                parse_subtitle_filename_hints(sub_name);
            let ext = sub_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("srt")
                .to_lowercase();

            for (movie_file_id, episode_file_id) in file_ids {
                let input = external_subtitles::ExternalSubtitleInput {
                    movie_file_id: *movie_file_id,
                    episode_file_id: *episode_file_id,
                    file_path: sub_path.to_string_lossy().to_string(),
                    file_size: *file_size,
                    format: ext.clone(),
                    language: language.clone(),
                    title: None,
                    is_forced,
                    is_hearing_impaired: is_hi,
                    source: "local".to_string(),
                    source_id: None,
                    source_metadata: None,
                };

                match external_subtitles::upsert_by_path(pool, &input).await {
                    Ok(_) => found += 1,
                    Err(e) => {
                        debug!(
                            path = %sub_path.display(),
                            error = %e,
                            "Failed to upsert external subtitle"
                        );
                    }
                }
            }
        }
    }

    found
}
