use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::livetv::{guide_sync, series_rules};
use crate::repositories::livetv_tuners;
use crate::search::{self, SearchBackend};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct LivetvGuideSyncPayload {
    pub tuner_id: Option<Uuid>,
}

pub struct LivetvGuideSyncHandler {
    pool: PgPool,
    search: Arc<dyn SearchBackend>,
    library_events: tokio::sync::broadcast::Sender<crate::events::LibraryEvent>,
}

impl LivetvGuideSyncHandler {
    pub fn new(
        pool: PgPool,
        search: Arc<dyn SearchBackend>,
        library_events: tokio::sync::broadcast::Sender<crate::events::LibraryEvent>,
    ) -> Self {
        Self {
            pool,
            search,
            library_events,
        }
    }
}

#[async_trait]
impl JobHandler for LivetvGuideSyncHandler {
    fn job_type(&self) -> &'static str {
        "livetv_guide_sync"
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
        let payload: LivetvGuideSyncPayload =
            serde_json::from_value(payload).unwrap_or_else(|_| LivetvGuideSyncPayload::default());
        nats_jobs::check_cancelled(&cancel)?;

        let tuner_ids = match payload.tuner_id {
            Some(id) => vec![id],
            None => livetv_tuners::list_all(&self.pool)
                .await?
                .into_iter()
                .map(|t| t.id)
                .collect(),
        };

        if tuner_ids.is_empty() {
            return Ok(json!({ "tuners": 0, "programmes": 0 }));
        }

        let mut totals = guide_sync::GuideSyncStats::default();
        let mut failures = Vec::new();

        for tuner_id in &tuner_ids {
            nats_jobs::check_cancelled(&cancel)?;
            match guide_sync::sync_tuner(&self.pool, *tuner_id).await {
                Ok(s) => {
                    info!(
                        job_id = %job_id,
                        tuner_id = %tuner_id,
                        channels = s.channels,
                        programmes = s.programmes,
                        unmatched_lcns = s.unmatched_lcns,
                        safe_episode_keys = s.safe_episode_keys,
                        pruned = s.pruned,
                        "Live TV guide sync complete"
                    );
                    let _ =
                        self.library_events
                            .send(crate::events::LibraryEvent::LivetvGuideSynced {
                                tuner_id: *tuner_id,
                                entries: s.programmes as i64,
                            });
                    totals.channels += s.channels;
                    totals.programmes += s.programmes;
                    totals.upserted += s.upserted;
                    totals.unmatched_lcns += s.unmatched_lcns;
                    totals.unmatched_programmes += s.unmatched_programmes;
                    totals.pruned += s.pruned;
                    totals.safe_episode_keys += s.safe_episode_keys;
                    totals.title_only_keys += s.title_only_keys;
                }
                Err(e) => {
                    warn!(tuner_id = %tuner_id, error = %e, "Live TV guide sync failed");
                    failures.push(format!("{tuner_id}: {e}"));
                }
            }
        }

        if totals.upserted == 0 && !failures.is_empty() {
            return Err(JobError::retryable(format!(
                "Guide sync failed for every tuner: {}",
                failures.join("; ")
            )));
        }

        match search::fetch_all_livetv_channel_docs(&self.pool).await {
            Ok(docs) => {
                if let Err(e) = self.search.reindex_all_livetv_channels(docs).await {
                    warn!(error = %e, "Failed to reindex Live TV channels in MeiliSearch");
                }
            }
            Err(e) => warn!(error = %e, "Failed to fetch Live TV channel docs for reindex"),
        }
        match search::fetch_all_livetv_guide_docs(&self.pool).await {
            Ok(docs) => {
                if let Err(e) = self.search.reindex_all_livetv_guide(docs).await {
                    warn!(error = %e, "Failed to reindex Live TV guide in MeiliSearch");
                }
            }
            Err(e) => warn!(error = %e, "Failed to fetch Live TV guide docs for reindex"),
        }

        // The 48 h ceiling means a rule cannot plan a week out, so the expander
        // re-runs on every refresh instead of scheduling once.
        let expanded = match series_rules::expand_all(&self.pool).await {
            Ok(stats) => {
                info!(
                    job_id = %job_id,
                    rules = stats.rules,
                    scheduled = stats.scheduled,
                    suppressed = stats.suppressed,
                    moved = stats.moved,
                    "Live TV series rules expanded"
                );
                Some(stats)
            }
            Err(e) => {
                warn!(error = %e, "Live TV series rule expansion failed");
                None
            }
        };

        Ok(json!({
            "tuners": tuner_ids.len(),
            "series_rules": expanded,
            "channels": totals.channels,
            "programmes": totals.programmes,
            "upserted": totals.upserted,
            "unmatched_lcns": totals.unmatched_lcns,
            "unmatched_programmes": totals.unmatched_programmes,
            "safe_episode_keys": totals.safe_episode_keys,
            "title_only_keys": totals.title_only_keys,
            "pruned": totals.pruned,
            "failures": failures,
        }))
    }
}
