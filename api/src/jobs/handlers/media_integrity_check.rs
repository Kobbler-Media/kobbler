//! Finds media files whose bytes no longer match what kobbler believes it has.
//!
//! Three files sat corrupt for two months without anything noticing, because
//! every check kobbler ran had already passed before the damage: they were
//! probed, analyzed, and in one case skip-detected, and the corruption arrived
//! afterwards or was never visible from the header. What was missing was
//! anything that looks at a file *again*.
//!
//! Two tiers, cheapest first:
//!
//! 1. **Stamp** — compare the file's current size and mtime against the ones
//!    recorded in its cache manifest. One `stat`, no reads, and exact: a
//!    mismatch means the cache describes bytes that are no longer there.
//!    Catches the "download reported complete, got analyzed, then a retry
//!    re-created it" case. Blind to a file that was already partial when it
//!    was analyzed, and to caches written before the manifest carried a stamp.
//!
//! 2. **Sample** — read a few 64 KB blocks spread through the file and look for
//!    any that is entirely zero. Compressed video essentially never produces a
//!    full zero block, while an unfinished download is full of them: measured
//!    over this library, 156 healthy files produced zero hits and all three
//!    known-bad files produced between 5 and 12. Catches partial downloads that
//!    the stamp cannot, at the cost of ~768 KB read per file.
//!
//! Reports by default. Deleting and re-acquiring on the strength of a
//! heuristic is a decision for a person, and the report carries what they need
//! to make it — so `remediate`, which acts on the content-sample verdict too,
//! stays opt-in and manual.
//!
//! The one exception is `remediate_missing`: a file that is no longer on disk
//! is not a heuristic, and leaving its row behind is what does the damage,
//! since `is_in_library` is EXISTS-a-file-row and every want path skips the
//! episode while it stands. Deleting a file by hand on the NAS otherwise
//! strands its episode at `imported` forever. That runs on the daily schedule,
//! interlocked on every media root being readable so an unmounted share cannot
//! be mistaken for a library-wide deletion.

use std::path::Path;

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::media_cache::episode_cache::read_manifest;
use crate::media_cache::integrity::{MIN_SAMPLE_SIZE, count_zero_blocks};
use crate::repositories::{movie_files, tv_episode_files, tv_episodes, tv_shows};
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Default, Deserialize)]
pub struct MediaIntegrityCheckPayload {
    /// Skip tier 2. Stamp-only is effectively free and touches no file data,
    /// so it is the right mode for a frequent schedule; the content sample
    /// suits an occasional deep pass.
    #[serde(default)]
    pub stamp_only: bool,
    /// Stop after this many files. Unset means the whole library.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Act on what is found instead of only reporting it: drop the file and its
    /// row, and mark the episode wanted so the next want pass re-acquires it.
    ///
    /// Off by default and deliberately so — this deletes media on the strength
    /// of a heuristic. Only `Missing` and `ZeroBlocks` are ever acted on.
    /// `StampMismatch` is not: a file that changed since it was analyzed is
    /// often simply a better copy someone dropped in, and the right response
    /// there is to re-analyze, not to delete.
    #[serde(default)]
    pub remediate: bool,
    /// Act on `Missing` only — the file is simply gone from disk.
    ///
    /// Split out from `remediate` because the two carry very different risk.
    /// `ZeroBlocks` is a heuristic read of file *content*, so acting on it
    /// deletes media that may well be fine. A file that no longer exists is
    /// not a heuristic: there is nothing left to delete, and leaving the row
    /// behind is what actively causes harm, since `is_in_library` is
    /// EXISTS-a-file-row and every want path therefore skips the episode
    /// forever.
    ///
    /// That makes this the one verdict safe to act on unattended, which is why
    /// it is enabled on the daily schedule while `remediate` stays manual.
    /// Guarded by [`roots_all_available`] — see there for why.
    #[serde(default)]
    pub remediate_missing: bool,
}

pub struct MediaIntegrityCheckHandler {
    pool: PgPool,
    publisher: Publisher,
    storage_root: String,
}

impl MediaIntegrityCheckHandler {
    pub fn new(pool: PgPool, publisher: Publisher, storage_root: String) -> Self {
        Self {
            pool,
            publisher,
            storage_root,
        }
    }
}

/// What a single file's check concluded.
#[derive(Debug, PartialEq)]
enum Verdict {
    Ok,
    Missing,
    /// Size or mtime differs from what the cache was built against.
    StampMismatch,
    /// At least one sampled block was entirely zero.
    ZeroBlocks(u64),
    /// No manifest stamp and the file was too small to sample.
    Unverifiable,
}

#[async_trait]
impl JobHandler for MediaIntegrityCheckHandler {
    fn job_type(&self) -> &'static str {
        "media_integrity_check"
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
        let payload: MediaIntegrityCheckPayload =
            serde_json::from_value(payload).unwrap_or_default();

        let mut targets: Vec<(Uuid, String, &'static str)> =
            tv_episode_files::list_available_with_paths(&self.pool)
                .await
                .map_err(|e| JobError::Retryable(format!("DB error: {e}")))?
                .into_iter()
                .map(|(id, path)| (id, path, "episode"))
                .collect();
        targets.extend(
            movie_files::list_available_with_paths(&self.pool)
                .await
                .map_err(|e| JobError::Retryable(format!("DB error: {e}")))?
                .into_iter()
                .map(|(id, path)| (id, path, "movie")),
        );
        if let Some(limit) = payload.limit {
            targets.truncate(limit);
        }

        let total = targets.len();
        let storage_root = self.storage_root.clone();
        let stamp_only = payload.stamp_only;
        let cancel_for_walk = cancel.clone();

        // Every step below is synchronous filesystem work over a network mount.
        let (suspects, checked, unverifiable) = tokio::task::spawn_blocking(move || {
            let mut suspects: Vec<(Uuid, String, &'static str, Verdict)> = Vec::new();
            let mut checked = 0usize;
            let mut unverifiable = 0usize;
            for (id, path, kind) in targets {
                if cancel_for_walk.is_cancelled() {
                    break;
                }
                checked += 1;
                let verdict = check_one(&storage_root, id, Path::new(&path), kind, stamp_only);
                match verdict {
                    Verdict::Ok => {}
                    Verdict::Unverifiable => unverifiable += 1,
                    other => {
                        warn!(
                            file_id = %id,
                            path = %path,
                            verdict = ?other,
                            "media_integrity_check: suspect file"
                        );
                        suspects.push((id, path, kind, other));
                    }
                }
            }
            (suspects, checked, unverifiable)
        })
        .await
        .expect("media_integrity_check walk task panicked");

        // Close the loop: a file confirmed bad is no use to anyone sitting on
        // disk, and while its row exists the episode still counts as "in
        // library" (`is_in_library` is EXISTS-a-file-row, not
        // EXISTS-a-*working*-file-row), so no want pass will ever replace it.
        // Removing the row is what makes it eligible again.
        let mut remediated = 0usize;
        // `remediate` keeps acting on both verdicts; `remediate_missing` is the
        // unattended subset and never touches a ZeroBlocks heuristic.
        let missing_only = !payload.remediate && payload.remediate_missing;
        let wants_remediation = payload.remediate || payload.remediate_missing;

        // Only consult the roots when a Missing verdict is actually on the
        // table — that is the verdict an unmounted root fakes. ZeroBlocks means
        // the file was read, so the mount was up.
        let has_missing = suspects.iter().any(|(_, _, _, v)| *v == Verdict::Missing);
        let roots_ok = !wants_remediation || !has_missing || roots_all_available(&self.pool).await;
        if wants_remediation && !roots_ok {
            warn!(
                suspects = suspects.len(),
                "media_integrity_check: a media root is unreadable, skipping remediation — \
                 every file under an unmounted root reads as Missing"
            );
        }

        if wants_remediation && roots_ok {
            for (id, path, kind, verdict) in &suspects {
                let actionable = if missing_only {
                    *verdict == Verdict::Missing
                } else {
                    matches!(verdict, Verdict::Missing | Verdict::ZeroBlocks(_))
                };
                if !actionable {
                    continue;
                }
                if *kind != "episode" {
                    continue;
                }
                match self.requeue_episode(*id, path).await {
                    Ok(true) => remediated += 1,
                    Ok(false) => {}
                    Err(e) => {
                        warn!(file_id = %id, error = %e, "media_integrity_check: remediation failed")
                    }
                }
            }
        }

        if !suspects.is_empty() {
            let sample: Vec<String> = suspects
                .iter()
                .take(5)
                .map(|(_, path, _, v)| {
                    let name = std::path::Path::new(path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.clone());
                    format!("{name} ({v:?})")
                })
                .collect();
            let more = suspects.len().saturating_sub(sample.len());
            let tail = if more > 0 {
                format!(" …and {more} more")
            } else {
                String::new()
            };
            crate::notifications::notify_agents(
                &self.pool,
                &self.publisher,
                &crate::notifications::NotificationPayload {
                    trigger: crate::notifications::NotificationTrigger::OnMediaCorrupt,
                    title: format!(
                        "{} file{} failed the integrity check",
                        suspects.len(),
                        if suspects.len() == 1 { "" } else { "s" }
                    ),
                    message: format!(
                        "Checked {checked} files. {}{}{}",
                        sample.join(", "),
                        tail,
                        if remediated > 0 {
                            format!(" — {remediated} dropped and re-wanted.")
                        } else if !roots_ok {
                            " — a media root is unreadable, so remediation was skipped.".to_string()
                        } else {
                            " — reported only; nothing was deleted.".to_string()
                        }
                    ),
                    entity_type: None,
                    entity_id: None,
                    entity_name: None,
                    url: None,
                    public_url: None,
                    show_id: None,
                    show_name: None,
                    season_number: None,
                    show_year: None,
                    upgraded: false,
                    details: None,
                },
            )
            .await;
        }

        info!(
            checked,
            total,
            suspects = suspects.len(),
            unverifiable,
            remediated,
            stamp_only,
            "media_integrity_check complete"
        );

        Ok(json!({
            "checked": checked,
            "total": total,
            "unverifiable": unverifiable,
            "stamp_only": stamp_only,
            "remediated": remediated,
            "remediation_skipped_roots_unavailable": wants_remediation && !roots_ok,
            "suspects": suspects
                .iter()
                .map(|(id, path, kind, v)| json!({
                    "file_id": id,
                    "media_type": kind,
                    "path": path,
                    "verdict": format!("{v:?}"),
                }))
                .collect::<Vec<_>>(),
        }))
    }
}

impl MediaIntegrityCheckHandler {
    /// Drop a confirmed-bad episode file and put its episode back in the want
    /// queue. Returns whether anything was actually requeued.
    async fn requeue_episode(&self, file_id: Uuid, path: &str) -> Result<bool, String> {
        // Row first: while it exists the episode reads as in-library and every
        // want path skips it. `delete` hands back the episodes it was linked to
        // before the cascade removes the junction rows.
        let Some(episode_ids) = tv_episode_files::delete(&self.pool, file_id)
            .await
            .map_err(|e| format!("delete row: {e}"))?
        else {
            // Already gone — nothing to re-want.
            return Ok(false);
        };

        // Then the file and its caches. Best-effort — a missing file is the
        // expected case for the `Missing` verdict.
        crate::repositories::cleanup_replaced_files(&[path.to_string()]).await;
        if let Err(e) = crate::media_cache::episode_cache::remove(
            &self.storage_root,
            &crate::media_cache::episode_cache::MediaKind::TvFile { file_id },
        ) {
            warn!(error = %e, file_id = %file_id, "integrity: episode_cache remove failed");
        }
        if let Err(e) = crate::media_cache::remove_per_file(&self.storage_root, file_id) {
            warn!(error = %e, file_id = %file_id, "integrity: per-file cache remove failed");
        }

        if episode_ids.is_empty() {
            return Ok(false);
        }
        tv_episodes::set_want_status_batch(&self.pool, &episode_ids, "wanted")
            .await
            .map_err(|e| format!("set want_status: {e}"))?;

        // Nudge the acquirer now rather than waiting on the hourly monitor.
        //
        // Sport events must go to `sports_want`, not `tv_want`: only that
        // handler builds the round/location/session want a motorsport session
        // needs, and a TV-shaped want for one matches nothing.
        let scopes = tv_episodes::list_scopes(&self.pool, &episode_ids)
            .await
            .map_err(|e| format!("resolve episode scopes: {e}"))?;
        for scope in &scopes {
            let is_sport = tv_shows::get(&self.pool, scope.show_id)
                .await
                .ok()
                .flatten()
                .is_some_and(|show| show.content_category == "sport");
            let (job, payload) = if is_sport {
                (
                    "sports_want",
                    json!({
                        "show_id": scope.show_id,
                        "season_id": scope.season_id,
                        "episode_ids": [scope.id],
                    }),
                )
            } else {
                (
                    "tv_want",
                    json!({ "season_id": scope.season_id, "episode_ids": [scope.id] }),
                )
            };
            if let Err(e) = self.publisher.enqueue_job("kobbler", job, payload).await {
                warn!(error = %e, job, "integrity: want enqueue failed");
            }
        }
        info!(file_id = %file_id, path, "media_integrity_check: dropped corrupt file and re-wanted");
        Ok(true)
    }
}

/// Whether every enabled media root is readable right now.
///
/// The safety interlock for `remediate_missing`. `Missing` is decided by a
/// failed `metadata()` call, which cannot tell "someone deleted this file"
/// apart from "the NAS is not mounted". Unattended, that difference is the
/// whole library: an unmounted root makes every file under it look missing at
/// once, and acting on that would drop every file row and re-want everything.
///
/// A root is judged present by whether the directory itself resolves, and an
/// empty root list means nothing to verify against, so remediation stays off.
/// Deliberately read-only — unlike `media_roots::check_available`, this records
/// no scan error and starts no backoff; a transient blip should skip a night's
/// remediation, not mark roots broken.
async fn roots_all_available(pool: &sqlx::PgPool) -> bool {
    let Ok(roots) = crate::repositories::media_roots::list_all(pool).await else {
        return false;
    };
    let paths: Vec<String> = roots
        .into_iter()
        .filter(|r| r.enabled)
        .map(|r| r.path)
        .collect();
    if paths.is_empty() {
        return false;
    }
    tokio::task::spawn_blocking(move || paths.iter().all(|p| std::path::Path::new(p).is_dir()))
        .await
        .unwrap_or(false)
}

/// Tier 1 then tier 2 for one file. Synchronous by design; the caller runs it
/// on the blocking pool.
fn check_one(
    storage_root: &str,
    file_id: Uuid,
    path: &Path,
    kind: &'static str,
    stamp_only: bool,
) -> Verdict {
    let Ok(md) = std::fs::metadata(path) else {
        return Verdict::Missing;
    };
    let size = md.len();

    // Tier 1: does the file still match what its cache was built from?
    let stamped = manifest_stamp(storage_root, file_id, kind);
    if let Some((cached_size, cached_mtime)) = stamped {
        let now_mtime = md
            .modified()
            .ok()
            .map(chrono::DateTime::<chrono::Utc>::from);
        if cached_size != size as i64 || now_mtime != Some(cached_mtime) {
            return Verdict::StampMismatch;
        }
    }

    if stamp_only {
        return if stamped.is_some() {
            Verdict::Ok
        } else {
            Verdict::Unverifiable
        };
    }

    // Tier 2: does the file contain blocks that are entirely zero?
    if size < MIN_SAMPLE_SIZE {
        return if stamped.is_some() {
            Verdict::Ok
        } else {
            Verdict::Unverifiable
        };
    }
    match count_zero_blocks(path, size) {
        Some(0) => Verdict::Ok,
        Some(n) => Verdict::ZeroBlocks(n),
        None => Verdict::Unverifiable,
    }
}

/// `(input_size_bytes, input_mtime)` from the file's cache manifest, when the
/// cache is new enough to carry them.
fn manifest_stamp(
    storage_root: &str,
    file_id: Uuid,
    kind: &'static str,
) -> Option<(i64, chrono::DateTime<chrono::Utc>)> {
    let media_kind = match kind {
        "episode" => crate::media_cache::episode_cache::MediaKind::TvFile { file_id },
        _ => return None,
    };
    let m = read_manifest(storage_root, &media_kind)?;
    Some((m.input_size_bytes?, m.input_mtime?))
}
