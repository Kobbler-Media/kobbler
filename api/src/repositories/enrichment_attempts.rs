//! Enrichment attempt tracking — prevents infinite re-enqueue of entities
//! that legitimately produce no results at a given config.

use super::Result;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

/// Record that an enrichment job was attempted for an entity at a config tag.
/// Upserts so re-runs just update the timestamp.
pub async fn record<'e, E>(
    exec: E,
    entity_type: &str,
    entity_id: Uuid,
    job_type: &str,
    config_tag: &str,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query(
        r#"
        INSERT INTO enrichment_attempts (entity_type, entity_id, job_type, config_tag)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (entity_type, entity_id, job_type, config_tag)
        DO UPDATE SET attempted_at = NOW()
        "#,
    )
    .bind(entity_type)
    .bind(entity_id)
    .bind(job_type)
    .bind(config_tag)
    .execute(exec)
    .await?;
    Ok(())
}

/// Delete enrichment attempt records for an entity + job type so re-runs are
/// not blocked by the backfill gate.
pub async fn delete_for_entity<'e, E>(
    exec: E,
    entity_type: &str,
    entity_id: Uuid,
    job_type: &str,
) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query(
        "DELETE FROM enrichment_attempts WHERE entity_type = $1 AND entity_id = $2 AND job_type = $3",
    )
    .bind(entity_type)
    .bind(entity_id)
    .bind(job_type)
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}
