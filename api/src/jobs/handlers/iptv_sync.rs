//! IPTV provider sync job handler.
//!
//! Syncs a single IPTV provider: fetches streams, runs channel matching,
//! syncs EPG, and prunes stale sources.

use crate::images::{self, ImageType};
use crate::iptv::iptv_org::IptvOrgDatabase;
use crate::iptv::sync;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories;
use crate::search::{self, SearchBackend};
use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use time::{Duration, OffsetDateTime};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// How long to wait before retrying a channel icon that previously failed to cache.
const ICON_RETRY_BACKOFF: Duration = Duration::hours(24);

/// Payload for iptv_sync jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct IptvSyncPayload {
    pub provider_id: Uuid,
}

/// Handler that syncs a single IPTV provider.
pub struct IptvSyncHandler {
    pool: PgPool,
    http: reqwest::Client,
    runtime_config: crate::RuntimeConfigManager,
    storage_root: String,
    search: Arc<dyn SearchBackend>,
}

impl IptvSyncHandler {
    pub fn new(
        pool: PgPool,
        http: reqwest::Client,
        runtime_config: crate::RuntimeConfigManager,
        storage_root: String,
        search: Arc<dyn SearchBackend>,
    ) -> Self {
        Self {
            pool,
            http,
            runtime_config,
            storage_root,
            search,
        }
    }
}

#[async_trait]
impl JobHandler for IptvSyncHandler {
    fn job_type(&self) -> &'static str {
        "iptv_sync"
    }

    fn max_concurrent(&self) -> usize {
        2
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: IptvSyncPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {e}")))?;

        nats_jobs::check_cancelled(&cancel)?;

        let provider_id = payload.provider_id;
        info!(job_id = %job_id, provider_id = %provider_id, "Syncing IPTV provider");

        // Fetch iptv-org database for channel matching
        let iptv_org_db = IptvOrgDatabase::fetch(&self.http).await;

        let config = self.runtime_config.get().await;
        let auto_create = config.iptv_auto_create_channels;

        match sync::sync_provider(&self.pool, provider_id, &iptv_org_db, auto_create).await {
            Ok(result) => {
                info!(
                    provider_id = %provider_id,
                    matched = result.match_stats.matched,
                    unmatched = result.match_stats.unmatched,
                    epg_entries = result.epg_entries,
                    sources_pruned = result.sources_pruned,
                    "IPTV provider sync complete"
                );

                // Reindex IPTV channels and EPG in MeiliSearch
                match search::fetch_all_iptv_channel_docs(&self.pool).await {
                    Ok(docs) => {
                        if let Err(e) = self.search.reindex_all_iptv_channels(docs).await {
                            warn!(error = %e, "Failed to reindex IPTV channels in MeiliSearch");
                        }
                    }
                    Err(e) => warn!(error = %e, "Failed to fetch IPTV channel docs for reindex"),
                }
                match search::fetch_all_iptv_epg_docs(&self.pool).await {
                    Ok(docs) => {
                        if let Err(e) = self.search.reindex_all_iptv_epg(docs).await {
                            warn!(error = %e, "Failed to reindex IPTV EPG in MeiliSearch");
                        }
                    }
                    Err(e) => warn!(error = %e, "Failed to fetch IPTV EPG docs for reindex"),
                }

                // Spawn non-blocking icon caching
                let pool = self.pool.clone();
                let storage_root = self.storage_root.clone();
                tokio::spawn(async move {
                    cache_channel_icons(&pool, &storage_root).await;
                });

                Ok(json!({
                    "provider_id": provider_id,
                    "matched": result.match_stats.matched,
                    "unmatched": result.match_stats.unmatched,
                    "epg_entries": result.epg_entries,
                    "sources_pruned": result.sources_pruned,
                }))
            }
            Err(e) => {
                let error_msg = format!("{e:#}");
                warn!(provider_id = %provider_id, error = %error_msg, "IPTV provider sync failed");

                // Record failure with exponential backoff so sync_check doesn't
                // re-enqueue this provider every 5 minutes.
                if let Err(db_err) = repositories::iptv_providers::record_sync_failure(
                    &self.pool,
                    provider_id,
                    &error_msg,
                )
                .await
                {
                    warn!(error = %db_err, "Failed to record sync failure backoff");
                }

                // Permanent at the job level, deliberately. The retry for this
                // failure is the `next_sync_retry_at` that `record_sync_failure`
                // just wrote — 10min, 30min, 90min, then a 4h cap, per provider
                // and surviving restarts — and `iptv_sync_check` honours it.
                //
                // A job-level retry cannot see that column, so it re-runs within
                // seconds and does exactly what the line above is there to
                // prevent: hammering a provider already marked as backing off.
                // Two retry mechanisms for one failure, and the wrong one wins.
                Err(JobError::ExecutionFailed(format!(
                    "Provider sync failed: {error_msg}"
                )))
            }
        }
    }
}

/// Download and cache icons for all channels that have a remote icon_url but no local cache.
///
/// Channels whose previous attempt failed within `ICON_RETRY_BACKOFF` are skipped to
/// avoid re-hammering URLs that consistently fail.
async fn cache_channel_icons(pool: &PgPool, storage_root: &str) {
    let channels = match repositories::iptv_channels::list_icon_urls(pool).await {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "Failed to list channels for icon caching");
            return;
        }
    };

    let now = OffsetDateTime::now_utc();
    let mut skipped_recent = 0usize;
    let pending: Vec<_> = channels
        .into_iter()
        .filter(|(id, _, _)| {
            !images::cached_image_exists(storage_root, ImageType::IptvChannelIcon, *id)
        })
        .filter(|(_, _, attempted_at)| {
            let recent = attempted_at.is_some_and(|t| now - t < ICON_RETRY_BACKOFF);
            if recent {
                skipped_recent += 1;
            }
            !recent
        })
        .collect();

    if pending.is_empty() {
        return;
    }

    info!(
        count = pending.len(),
        skipped_recent, "Caching IPTV channel icons"
    );

    let storage_root_owned = storage_root.to_owned();
    let results: Vec<bool> = stream::iter(pending)
        .map(|(id, url, _)| {
            let sr = storage_root_owned.clone();
            let pool = pool.clone();
            async move {
                images::try_cache_image(&sr, ImageType::IptvChannelIcon, id, Some(&url)).await;
                let cached = images::cached_image_exists(&sr, ImageType::IptvChannelIcon, id);
                if let Err(e) =
                    repositories::iptv_channels::mark_icon_cache_attempted(&pool, id).await
                {
                    warn!(channel_id = %id, error = %e, "Failed to stamp icon_cache_attempted_at");
                }
                cached
            }
        })
        .buffer_unordered(10)
        .collect()
        .await;

    let cached = results.iter().filter(|ok| **ok).count();
    let failed = results.len() - cached;
    info!(cached, failed, "IPTV icon caching complete");
}
