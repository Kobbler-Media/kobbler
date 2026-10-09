//! Provider sync orchestration.
//!
//! Coordinates the full sync cycle: fetch streams from provider, run matching,
//! sync EPG, prune stale sources, and log results.

use std::collections::HashMap;

use anyhow::{Context, Result};
use sqlx::PgPool;
use time::OffsetDateTime;
use tracing::{error, info};
use uuid::Uuid;

use super::epg;
use super::iptv_org::IptvOrgDatabase;
use super::matching::{self, MatchStats, derive_category};
use super::xtream::XtreamClient;
use crate::repositories;

pub struct SyncResult {
    pub match_stats: MatchStats,
    pub epg_entries: u32,
    pub sources_pruned: u64,
}

/// Run a full sync for a single IPTV provider.
pub async fn sync_provider(
    pool: &PgPool,
    provider_id: Uuid,
    iptv_org_db: &IptvOrgDatabase,
    auto_create_channels: bool,
) -> Result<SyncResult> {
    let sync_start = OffsetDateTime::now_utc();

    // 1. Fetch provider from DB
    let provider = repositories::iptv_providers::get_by_id(pool, provider_id)
        .await
        .context("fetching provider")?
        .context("provider not found")?;

    info!(
        provider = %provider.name,
        provider_id = %provider_id,
        "starting IPTV provider sync"
    );

    // Create sync log entry
    let sync_log_id = create_sync_log(pool, provider_id).await?;

    // 2. Create Xtream client
    let http = crate::http::new_client();
    let client = XtreamClient::new(
        http.clone(),
        &provider.url,
        &provider.username,
        &provider.password,
    );

    // 3. Fetch account info and update provider
    match client.get_account_info().await {
        Ok(account_info) => {
            // Register max connections for provider capacity tracking
            if let Some(max_conn) = account_info
                .max_connections
                .as_deref()
                .and_then(|s| s.parse::<u32>().ok())
            {
                super::provider_tracker::register_provider(provider_id, max_conn);
            }

            let info_json =
                serde_json::to_value(&account_info).unwrap_or_else(|_| serde_json::json!({}));
            let _ = repositories::iptv_providers::update(
                pool,
                provider_id,
                repositories::iptv_providers::UpdateProvider {
                    account_info: Some(info_json),
                    ..default_provider_update()
                },
            )
            .await;
        }
        Err(e) => {
            info!(error = %e, "failed to fetch account info, continuing with sync");
        }
    }

    // 4. Fetch live streams + categories in parallel
    let (streams_result, categories_result) =
        tokio::join!(client.get_live_streams(), client.get_live_categories(),);

    let streams = streams_result.context("fetching live streams")?;
    info!(count = streams.len(), "fetched live streams from provider");

    // Build category_id -> category_name map
    let categories: HashMap<String, String> = match categories_result {
        Ok(cats) => cats
            .into_iter()
            .map(|c| (c.category_id, c.category_name))
            .collect(),
        Err(e) => {
            info!(error = %e, "failed to fetch categories, continuing without category names");
            HashMap::new()
        }
    };

    // 5. Run matching pipeline
    let match_stats = matching::match_streams(
        pool,
        provider_id,
        streams,
        &categories,
        iptv_org_db,
        auto_create_channels,
    )
    .await
    .context("matching pipeline")?;

    // 5b. Backfill categories on channels that have none
    backfill_channel_categories(pool).await?;

    // 6. Sync EPG
    let epg_url = client.epg_url();
    let epg_entries = match epg::sync_epg(pool, &http, &epg_url, provider_id).await {
        Ok(count) => count,
        Err(e) => {
            error!(error = %e, "EPG sync failed, continuing");
            0
        }
    };

    // 7. Prune stale sources (last_seen_at < sync start)
    let sources_pruned = repositories::iptv_sources::prune_stale(pool, provider_id, sync_start)
        .await
        .context("pruning stale sources")?;

    if sources_pruned > 0 {
        info!(pruned = sources_pruned, "pruned stale sources");
    }

    // 7b. Clean up orphaned channels (channels with no remaining sources)
    match repositories::iptv_channels::delete_orphaned(pool).await {
        Ok(count) if count > 0 => {
            info!(
                orphaned_channels = count,
                "cleaned up orphaned channels after sync"
            );
        }
        Ok(_) => {}
        Err(e) => {
            error!(error = %e, "failed to clean up orphaned channels");
        }
    }

    // 8. Update provider.last_sync_at
    repositories::iptv_providers::update_sync_status(
        pool,
        provider_id,
        OffsetDateTime::now_utc(),
        None,
    )
    .await
    .context("updating provider sync status")?;

    // 9. Complete sync log
    complete_sync_log(
        pool,
        sync_log_id,
        &match_stats,
        epg_entries,
        sources_pruned,
        None,
    )
    .await?;

    let result = SyncResult {
        match_stats,
        epg_entries,
        sources_pruned,
    };

    info!(
        provider = %provider.name,
        matched = result.match_stats.matched,
        new = result.match_stats.new_channels,
        unmatched = result.match_stats.unmatched,
        epg = result.epg_entries,
        pruned = result.sources_pruned,
        "provider sync complete"
    );

    Ok(result)
}

/// Create a sync log entry.
async fn create_sync_log(pool: &PgPool, provider_id: Uuid) -> Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query!(
        r#"
        INSERT INTO iptv_sync_log (id, provider_id, status)
        VALUES ($1, $2, 'running')
        "#,
        id,
        provider_id,
    )
    .execute(pool)
    .await
    .context("creating sync log")?;

    Ok(id)
}

/// Complete a sync log entry.
async fn complete_sync_log(
    pool: &PgPool,
    log_id: Uuid,
    stats: &MatchStats,
    epg_entries: u32,
    sources_pruned: u64,
    error_message: Option<&str>,
) -> Result<()> {
    let status = if error_message.is_some() {
        "error"
    } else {
        "completed"
    };

    sqlx::query!(
        r#"
        UPDATE iptv_sync_log SET
            completed_at = NOW(),
            status = $2,
            channels_matched = $3,
            sources_added = $4,
            sources_removed = $5,
            unmatched_count = $6,
            epg_entries = $7,
            error_message = $8
        WHERE id = $1
        "#,
        log_id,
        status,
        stats.matched as i32,
        stats.new_channels as i32,
        sources_pruned as i32,
        stats.unmatched as i32,
        epg_entries as i32,
        error_message,
    )
    .execute(pool)
    .await
    .context("completing sync log")?;

    Ok(())
}

/// Backfill categories on channels that have `category IS NULL` by deriving
/// from their sources' `remote_category_name` using the pattern-based categorizer.
pub async fn backfill_channel_categories(pool: &PgPool) -> Result<()> {
    // Failover order: priority ASC (0=best..100=worst, see quality::compute_priority),
    // matching idx_iptv_sources_failover. priority defaults to 50 for every row, so the
    // id tie-break is what actually decides most channels.
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT ON (c.id) c.id, c.name, s.remote_category_name
        FROM iptv_channels c
        JOIN iptv_sources s ON s.channel_id = c.id
        WHERE c.category IS NULL
          AND s.remote_category_name IS NOT NULL
        ORDER BY c.id, s.priority ASC, s.health_score DESC, s.id ASC
        "#,
    )
    .fetch_all(pool)
    .await
    .context("fetching channels for category backfill")?;

    let mut updated = 0u32;
    for row in &rows {
        if let Some(cat) = derive_category(&row.name, row.remote_category_name.as_deref()) {
            sqlx::query!(
                r#"UPDATE iptv_channels SET category = $2, updated_at = now() WHERE id = $1"#,
                row.id,
                cat,
            )
            .execute(pool)
            .await
            .context("updating channel category")?;
            updated += 1;
        }
    }

    if updated > 0 {
        info!(
            count = updated,
            "backfilled channel categories from sources"
        );
    }

    Ok(())
}

/// Helper to build a default UpdateProvider with all None fields.
fn default_provider_update() -> repositories::iptv_providers::UpdateProvider {
    repositories::iptv_providers::UpdateProvider {
        name: None,
        url: None,
        username: None,
        password: None,
        preferred_format: None,
        sync_interval_secs: None,
        enabled: None,
        account_info: None,
    }
}
