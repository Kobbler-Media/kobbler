//! Source health monitoring.
//!
//! Health scores are updated PASSIVELY during playback — when a source fails
//! to connect, its score decreases; when it works, it recovers. Active probing
//! (HEAD requests to every source) doesn't work for IPTV — providers don't
//! respond to HEAD on stream URLs and it hammers them with 50k requests.

use anyhow::Result;
use sqlx::PgPool;
use tracing::info;

use crate::repositories;

/// Record a playback failure for a source (passive health tracking).
pub async fn record_failure(pool: &PgPool, source_id: uuid::Uuid) -> Result<()> {
    let current = repositories::iptv_sources::get_health_score(pool, source_id)
        .await?
        .unwrap_or(1.0);
    let new_score = (current - 0.2_f32).max(0.0);
    repositories::iptv_sources::update_health(pool, source_id, new_score).await?;
    Ok(())
}

/// Record a playback success for a source (passive health recovery).
pub async fn record_success(pool: &PgPool, source_id: uuid::Uuid) -> Result<()> {
    let current = repositories::iptv_sources::get_health_score(pool, source_id)
        .await?
        .unwrap_or(1.0);
    let new_score = (current + 0.1_f32).min(1.0);
    if (new_score - current).abs() > f32::EPSILON {
        repositories::iptv_sources::update_health(pool, source_id, new_score).await?;
    }
    Ok(())
}

/// Recover health scores for sources that haven't been checked recently.
/// Called periodically — slowly restores scores for sources that might have
/// been temporarily down. Does NOT actively probe sources.
pub async fn passive_recovery(pool: &PgPool) -> Result<u32> {
    // Bump sources with low scores that haven't been checked in 24h+
    let updated = sqlx::query_scalar!(
        r#"
        WITH recovered AS (
            UPDATE iptv_sources
            SET health_score = LEAST(health_score + 0.1, 1.0),
                last_health_check_at = NOW()
            WHERE health_score < 1.0
              AND health_score > 0.0
              AND (last_health_check_at IS NULL OR last_health_check_at < NOW() - INTERVAL '24 hours')
            RETURNING 1
        )
        SELECT COUNT(*)::int AS "count!" FROM recovered
        "#,
    )
    .fetch_one(pool)
    .await?;

    if updated > 0 {
        info!(
            recovered = updated,
            "passively recovered source health scores"
        );
    }
    Ok(updated as u32)
}
