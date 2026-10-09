//! Bulk resolve job handler.
//!
//! Processes a batch of media root files. With a candidate override on
//! non-auto-add roots, confirms identity only (moves to review). Otherwise
//! creates the library entity and marks the file as added.
//! Reports progress via SSE so the UI can show "Adding... (X/N)".

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobEvent, update_progress};
use crate::media_root_resolve::{self, FileWithCandidate, ResolveContext};
use crate::repositories::{media_root_files, media_roots};
use crate::search::{self, SearchBackend};
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

pub struct MediaRootBulkResolveHandler {
    pool: PgPool,
    nats: Publisher,
    search: Arc<dyn SearchBackend>,
    job_events: broadcast::Sender<JobEvent>,
}

impl MediaRootBulkResolveHandler {
    pub fn new(
        pool: PgPool,
        nats: Publisher,
        search: Arc<dyn SearchBackend>,
        job_events: broadcast::Sender<JobEvent>,
    ) -> Self {
        Self {
            pool,
            nats,
            search,
            job_events,
        }
    }
}

#[async_trait]
impl JobHandler for MediaRootBulkResolveHandler {
    fn job_type(&self) -> &'static str {
        "media_root_bulk_resolve"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> Result<Value, JobError> {
        nats_jobs::check_cancelled(&cancel)?;
        let file_ids: Vec<Uuid> = payload["file_ids"]
            .as_array()
            .ok_or_else(|| JobError::ExecutionFailed("Missing file_ids array".into()))?
            .iter()
            .filter_map(|v| v.as_str().and_then(|s| s.parse().ok()))
            .collect();

        // Optional candidate override: if present, use this for all files
        // instead of each file's stored match_candidates.
        let candidate_override: Option<Value> =
            payload.get("candidate").filter(|v| !v.is_null()).cloned();

        // link_to_library mode: link files to existing entities instead of
        // creating new ones. Skips enrichment — only creates file records.
        let link_to_library = payload
            .get("link_to_library")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let total = file_ids.len();
        if total == 0 {
            return Ok(
                json!({"resolved": 0, "reviewed": 0, "failed": 0, "errors": [], "total": 0}),
            );
        }

        info!(job_id = %job_id, total, link_to_library, has_candidate_override = candidate_override.is_some(), "Starting bulk resolve");

        let ctx = ResolveContext {
            pool: &self.pool,
            nats: &self.nats,
            search: self.search.as_ref(),
        };

        let mut resolved = 0u32;
        let mut reviewed = 0u32;
        let mut failed = 0u32;
        let mut errors: Vec<String> = Vec::new();
        let mut root_cache: HashMap<Uuid, media_roots::MediaRootRow> = HashMap::new();

        // =====================================================================
        // Phase 1: Load all files + candidates, classify into groups
        // =====================================================================

        /// Entity grouping key: (media_type, provider, provider_id)
        type EntityKey = (String, String, String);

        // Files that need grouped resolve (normal add path)
        let mut entity_groups: HashMap<EntityKey, Vec<FileWithCandidate>> = HashMap::new();
        // Files for review-only path (candidate override on non-auto-add root)
        let mut review_files: Vec<(Uuid, media_root_files::MediaRootFileRow, Value)> = Vec::new();
        // Files for link-to-library path
        let mut link_files: Vec<(
            Uuid,
            media_root_files::MediaRootFileRow,
            media_roots::MediaRootRow,
            Value,
        )> = Vec::new();

        for file_id in &file_ids {
            let file = match media_root_files::get_by_id(&self.pool, *file_id).await {
                Ok(Some(f)) => f,
                Ok(None) => {
                    failed += 1;
                    errors.push(format!("{file_id}: not found"));
                    continue;
                }
                Err(e) => {
                    failed += 1;
                    errors.push(format!("{file_id}: {e}"));
                    continue;
                }
            };

            let candidate = if let Some(ref override_candidate) = candidate_override {
                override_candidate.clone()
            } else {
                if file.match_status != "review" {
                    failed += 1;
                    errors.push(format!(
                        "{}: status is '{}', not 'review'",
                        file.file_name, file.match_status
                    ));
                    continue;
                }
                let candidates: Vec<Value> = file
                    .match_candidates
                    .as_ref()
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                match candidates.into_iter().next() {
                    Some(c) => c,
                    None => {
                        failed += 1;
                        errors.push(format!("{}: no match candidates", file.file_name));
                        continue;
                    }
                }
            };

            // Load media root (cached)
            let root = if let Some(r) = root_cache.get(&file.media_root_id) {
                r.clone()
            } else {
                match media_roots::get_by_id(&self.pool, file.media_root_id).await {
                    Ok(Some(r)) => {
                        root_cache.insert(file.media_root_id, r.clone());
                        r
                    }
                    Ok(None) => {
                        failed += 1;
                        errors.push(format!("{}: media root not found", file.file_name));
                        continue;
                    }
                    Err(e) => {
                        failed += 1;
                        errors.push(format!("{}: {e}", file.file_name));
                        continue;
                    }
                }
            };

            // Classify this file
            if candidate_override.is_some() && !root.auto_add {
                review_files.push((*file_id, file, candidate));
            } else if link_to_library {
                link_files.push((*file_id, file, root, candidate));
            } else {
                let media_type = root.media_type.clone();
                let provider = candidate["provider"]
                    .as_str()
                    .unwrap_or("unknown")
                    .to_string();
                let pid = candidate["provider_id"]
                    .as_str()
                    .map(|s| s.to_string())
                    .or_else(|| candidate["provider_id"].as_u64().map(|n| n.to_string()))
                    .unwrap_or_default();
                let key = (media_type, provider, pid);
                entity_groups
                    .entry(key)
                    .or_default()
                    .push(FileWithCandidate {
                        file_id: *file_id,
                        file,
                        candidate,
                    });
            }
        }

        // =====================================================================
        // Phase 2a: Process review-only files (per-file, fast)
        // =====================================================================
        let mut review_search_ids: Vec<Uuid> = Vec::new();
        for (file_id, file, candidate) in &review_files {
            let confirmed = media_root_files::build_confirmed_candidates(file, candidate);
            match media_root_files::set_review(&self.pool, *file_id, &confirmed).await {
                Ok(()) => {
                    review_search_ids.push(*file_id);
                    reviewed += 1;
                }
                Err(e) => {
                    failed += 1;
                    errors.push(format!("{}: {e}", file.file_name));
                }
            }
        }
        if !review_search_ids.is_empty() {
            let _ = search::batch_index_media_root_files(
                self.search.as_ref(),
                &self.pool,
                &review_search_ids,
            )
            .await;
        }

        // =====================================================================
        // Phase 2b: Process link-to-library files (per-file, no grouping needed)
        // =====================================================================
        for (file_id, file, root, candidate) in &link_files {
            match media_root_resolve::link_to_library(&ctx, root, file, *file_id, candidate).await {
                Ok(_) => resolved += 1,
                Err(e) => {
                    failed += 1;
                    errors.push(format!("{}: {e}", file.file_name));
                }
            }
        }

        // =====================================================================
        // Phase 3: Process entity groups (the main bulk path)
        // =====================================================================
        let group_count = entity_groups.len();
        let mut groups_done = 0usize;

        for ((media_type, _provider, _pid), files) in &entity_groups {
            // Get root from cache (always present — loaded during phase 1)
            let first_root_id = files[0].file.media_root_id;
            let Some(root) = root_cache.get(&first_root_id).cloned() else {
                for f in files {
                    failed += 1;
                    errors.push(format!("{}: media root not in cache", f.file.file_name));
                }
                groups_done += 1;
                continue;
            };

            let group_results = match media_type.as_str() {
                "tv" => media_root_resolve::resolve_tv_group(&ctx, &root, files).await,
                "sport" => media_root_resolve::resolve_sport_group(&ctx, &root, files).await,
                "movie" => media_root_resolve::resolve_movie_group(&ctx, &root, files).await,
                "book" | "audiobook" => {
                    media_root_resolve::resolve_book_group(&ctx, &root, files).await
                }
                other => {
                    for f in files {
                        failed += 1;
                        errors.push(format!(
                            "{}: unknown media type '{other}'",
                            f.file.file_name
                        ));
                    }
                    groups_done += 1;
                    continue;
                }
            };

            for gr in group_results {
                match gr.result {
                    Ok(_) => resolved += 1,
                    Err(e) => {
                        failed += 1;
                        errors.push(e);
                    }
                }
            }

            groups_done += 1;
            let pct = (groups_done as f64 / group_count.max(1) as f64 * 100.0) as i32;
            let _ = update_progress(
                &self.pool,
                &self.job_events,
                job_id,
                "media_root_bulk_resolve",
                pct,
                Some(json!({
                    "resolved": resolved,
                    "reviewed": reviewed,
                    "failed": failed,
                    "total": total,
                })),
            )
            .await;
        }

        info!(
            job_id = %job_id,
            resolved,
            reviewed,
            failed,
            total,
            entity_groups = group_count,
            "Bulk resolve complete"
        );

        Ok(json!({
            "resolved": resolved,
            "reviewed": reviewed,
            "failed": failed,
            "total": total,
            "errors": errors,
        }))
    }
}
