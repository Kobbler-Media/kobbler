//! IPTV-org database refresh job handler.
//!
//! Scheduled daily job that fetches the iptv-org channel database and
//! updates channel aliases with iptv-org alt_names.

use crate::iptv::iptv_org::IptvOrgDatabase;
use crate::iptv::normalization;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{iptv_channel_aliases, iptv_channels};
use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

/// Handler that refreshes the iptv-org channel database.
pub struct IptvOrgRefreshHandler {
    pool: PgPool,
    http: reqwest::Client,
}

impl IptvOrgRefreshHandler {
    pub fn new(pool: PgPool, http: reqwest::Client) -> Self {
        Self { pool, http }
    }
}

#[async_trait]
impl JobHandler for IptvOrgRefreshHandler {
    fn job_type(&self) -> &'static str {
        "iptv_org_refresh"
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
        info!(job_id = %job_id, "Refreshing iptv-org channel database");

        let iptv_org_db = IptvOrgDatabase::fetch(&self.http).await;

        if iptv_org_db.is_empty() {
            info!("iptv-org database is empty, skipping alias update");
            return Ok(json!({ "status": "skipped", "reason": "empty_database" }));
        }

        // Fetch all channels and look up each in the iptv-org database
        let channels = iptv_channels::list_with_source_counts(&self.pool)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to list channels: {e}")))?;

        let mut aliases_added = 0u64;
        for channel in &channels {
            let normalized = normalization::normalize_stream_name(&channel.name);
            let Some(org_channel) =
                iptv_org_db.lookup(&normalized.normalized_name, channel.country.as_deref())
            else {
                continue;
            };

            if org_channel.alt_names.is_empty() {
                continue;
            }

            // Normalize alt_names and insert as aliases
            let normalized_aliases: Vec<String> = org_channel
                .alt_names
                .iter()
                .map(|alt| normalization::normalize_stream_name(alt).normalized_name)
                .collect();

            match iptv_channel_aliases::bulk_insert(
                &self.pool,
                channel.id,
                &normalized_aliases,
                "iptv-org",
            )
            .await
            {
                Ok(count) => aliases_added += count,
                Err(e) => {
                    // Non-fatal — log and continue
                    tracing::warn!(
                        channel_id = %channel.id,
                        error = %e,
                        "Failed to insert aliases for channel"
                    );
                }
            }
        }

        info!(
            iptv_org_channels = iptv_org_db.len(),
            aliases_added, "iptv-org refresh complete"
        );
        Ok(json!({
            "iptv_org_channels": iptv_org_db.len(),
            "local_channels_checked": channels.len(),
            "aliases_added": aliases_added,
        }))
    }
}
