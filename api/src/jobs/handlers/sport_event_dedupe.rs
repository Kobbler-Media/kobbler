//! One-shot, idempotent dedup of duplicate sport-event episode rows.
//!
//! TheSportsDB can expose the same real session under two event IDs (e.g. `ev-567724`
//! and `2417416`), which historically produced two `tv_episodes` rows (identifier
//! `evt-{provider_id}`) → two watch patterns → duplicate downloads. New ingestion now
//! builds a natural-key identifier (`evt-r{round}-{session}`) so the UNIQUE constraint
//! collapses dups going forward; this job converts and merges the rows that already exist.
//!
//! Enqueued once at startup (see `main.rs`). Idempotent: once every sport episode carries
//! its natural-key identifier, re-running is a no-op.

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::video_metadata::thesportsdb::sport_episode_identifier;
use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

pub struct SportEventDedupeHandler {
    pool: PgPool,
}

impl SportEventDedupeHandler {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

struct SportEpisode {
    id: Uuid,
    season_id: Uuid,
    episode_identifier: String,
    episode_sort_order: i32,
    title: Option<String>,
    provider_id: Option<String>,
    want_status: String,
    in_library: bool,
    created_at: time::OffsetDateTime,
}

/// Higher = keep. Prefer a row that has files, then the most-advanced want_status.
fn survivor_rank(ep: &SportEpisode) -> i32 {
    if ep.in_library {
        return 100;
    }
    match ep.want_status.as_str() {
        "imported" => 60,
        "watching" => 50,
        "downloading" => 40,
        "searching" => 30,
        "wanted" => 20,
        _ => 0,
    }
}

#[async_trait]
impl JobHandler for SportEventDedupeHandler {
    fn job_type(&self) -> &'static str {
        "sport_event_dedupe"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        job_id: Uuid,
        _payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        nats_jobs::check_cancelled(&cancel)?;

        let rows = sqlx::query!(
            r#"
            SELECT e.id, e.season_id, e.episode_identifier, e.episode_sort_order,
                   e.title, e.identifiers->>'thesportsdb_id' AS provider_id,
                   e.want_status, e.created_at,
                   EXISTS (
                       SELECT 1 FROM tv_episode_file_episodes efe WHERE efe.episode_id = e.id
                   ) AS "in_library!"
            FROM tv_episodes e
            JOIN tv_seasons s ON s.id = e.season_id
            JOIN tv_shows sh ON sh.id = s.show_id
            WHERE sh.content_category = 'sport'
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("load sport episodes: {e}")))?;

        let episodes: Vec<SportEpisode> = rows
            .into_iter()
            .map(|r| SportEpisode {
                id: r.id,
                season_id: r.season_id,
                episode_identifier: r.episode_identifier,
                episode_sort_order: r.episode_sort_order,
                title: r.title,
                provider_id: r.provider_id,
                want_status: r.want_status,
                in_library: r.in_library,
                created_at: r.created_at,
            })
            .collect();

        // Group by (season_id, natural-key target identifier).
        let mut groups: HashMap<(Uuid, String), Vec<SportEpisode>> = HashMap::new();
        for ep in episodes {
            let target = sport_episode_identifier(
                ep.episode_sort_order,
                ep.title.as_deref(),
                ep.provider_id.as_deref(),
            );
            groups.entry((ep.season_id, target)).or_default().push(ep);
        }

        let mut merged_groups = 0u64;
        let mut deleted_rows = 0u64;
        let mut rewritten = 0u64;

        for ((season_id, target), mut group) in groups {
            nats_jobs::check_cancelled(&cancel)?;

            // Survivor: highest rank, then oldest (stable).
            group.sort_by(|a, b| {
                survivor_rank(b)
                    .cmp(&survivor_rank(a))
                    .then(a.created_at.cmp(&b.created_at))
            });
            let survivor = &group[0];
            let losers: Vec<&SportEpisode> = group.iter().skip(1).collect();

            if losers.is_empty() && survivor.episode_identifier == target {
                continue; // already canonical
            }

            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("begin tx: {e}")))?;

            for loser in &losers {
                // Repoint files (PK episode_file_id, episode_id) — skip links the survivor
                // already has; the rest cascade-delete with the loser.
                sqlx::query!(
                    r#"UPDATE tv_episode_file_episodes t SET episode_id = $1
                       WHERE t.episode_id = $2
                         AND NOT EXISTS (
                             SELECT 1 FROM tv_episode_file_episodes x
                             WHERE x.episode_file_id = t.episode_file_id AND x.episode_id = $1
                         )"#,
                    survivor.id,
                    loser.id,
                )
                .execute(&mut *tx)
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("repoint files: {e}")))?;

                // Repoint watch states (UNIQUE episode_id, user_id) — skip conflicts.
                sqlx::query!(
                    r#"UPDATE tv_watch_states t SET episode_id = $1
                       WHERE t.episode_id = $2
                         AND NOT EXISTS (
                             SELECT 1 FROM tv_watch_states x
                             WHERE x.user_id = t.user_id AND x.episode_id = $1
                         )"#,
                    survivor.id,
                    loser.id,
                )
                .execute(&mut *tx)
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("repoint watch states: {e}")))?;

                // Repoint no-FK soft references so nothing dangles to a deleted episode.
                sqlx::query!(
                    "UPDATE media_root_files SET entity_id = $1 WHERE entity_type = 'tv_episode' AND entity_id = $2",
                    survivor.id,
                    loser.id,
                )
                .execute(&mut *tx)
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("repoint media_root_files: {e}")))?;

                sqlx::query!(
                    "UPDATE debrid_stream_sessions SET media_id = $1 WHERE media_id = $2",
                    survivor.id,
                    loser.id,
                )
                .execute(&mut *tx)
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("repoint debrid sessions: {e}")))?;

                sqlx::query!(
                    "UPDATE activity_log SET entity_id = $1 WHERE entity_id = $2",
                    survivor.id,
                    loser.id,
                )
                .execute(&mut *tx)
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("repoint activity_log: {e}")))?;

                // Delete the loser; CASCADE clears wanted_attempts, cached_segments and any
                // leftover (conflicting) file/watch links.
                sqlx::query!("DELETE FROM tv_episodes WHERE id = $1", loser.id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| JobError::ExecutionFailed(format!("delete loser: {e}")))?;
                deleted_rows += 1;
            }

            // Canonicalize the survivor's identifier (safe: losers for this target are gone).
            if survivor.episode_identifier != target {
                sqlx::query!(
                    "UPDATE tv_episodes SET episode_identifier = $1, updated_at = NOW() WHERE id = $2",
                    target,
                    survivor.id,
                )
                .execute(&mut *tx)
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("rewrite identifier: {e}")))?;
                rewritten += 1;
            }

            tx.commit()
                .await
                .map_err(|e| JobError::ExecutionFailed(format!("commit: {e}")))?;

            if !losers.is_empty() {
                merged_groups += 1;
                warn!(
                    season_id = %season_id,
                    target = %target,
                    survivor = %survivor.id,
                    losers = losers.len(),
                    "Merged duplicate sport-event rows"
                );
            }
        }

        info!(
            job_id = %job_id,
            merged_groups,
            deleted_rows,
            rewritten,
            "sport_event_dedupe complete"
        );

        Ok(json!({
            "merged_groups": merged_groups,
            "deleted_rows": deleted_rows,
            "rewritten": rewritten,
        }))
    }
}
