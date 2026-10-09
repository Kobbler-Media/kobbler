use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct TunerRow {
    pub id: Uuid,
    pub device_id: String,
    pub friendly_name: String,
    pub model_number: Option<String>,
    pub firmware_version: Option<String>,
    pub base_url: String,
    pub device_auth: Option<String>,
    pub tuner_count: i32,
    pub last_scan_at: Option<OffsetDateTime>,
    pub last_guide_sync_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug)]
pub struct TunerWithCounts {
    pub tuner: TunerRow,
    pub channel_count: i64,
    pub enabled_channel_count: i64,
}

#[derive(Debug)]
pub struct CreateTuner {
    pub device_id: String,
    pub friendly_name: String,
    pub model_number: Option<String>,
    pub firmware_version: Option<String>,
    pub base_url: String,
    pub device_auth: Option<String>,
    pub tuner_count: i32,
}

pub async fn upsert<'e, E>(exec: E, input: CreateTuner) -> Result<TunerRow>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let r = sqlx::query!(
        r#"
        INSERT INTO livetv_tuners (id, device_id, friendly_name, model_number, firmware_version,
                                   base_url, device_auth, tuner_count)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (device_id) DO UPDATE SET
            friendly_name = EXCLUDED.friendly_name,
            model_number = EXCLUDED.model_number,
            firmware_version = EXCLUDED.firmware_version,
            base_url = EXCLUDED.base_url,
            device_auth = EXCLUDED.device_auth,
            tuner_count = EXCLUDED.tuner_count,
            updated_at = NOW()
        RETURNING id, device_id, friendly_name, model_number, firmware_version, base_url,
                  device_auth, tuner_count, last_scan_at, last_guide_sync_at, created_at, updated_at
        "#,
        id,
        input.device_id,
        input.friendly_name,
        input.model_number,
        input.firmware_version,
        input.base_url,
        input.device_auth,
        input.tuner_count,
    )
    .fetch_one(exec)
    .await?;

    Ok(TunerRow {
        id: r.id,
        device_id: r.device_id,
        friendly_name: r.friendly_name,
        model_number: r.model_number,
        firmware_version: r.firmware_version,
        base_url: r.base_url,
        device_auth: r.device_auth,
        tuner_count: r.tuner_count,
        last_scan_at: r.last_scan_at,
        last_guide_sync_at: r.last_guide_sync_at,
        created_at: r.created_at,
        updated_at: r.updated_at,
    })
}

pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<TunerRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, device_id, friendly_name, model_number, firmware_version, base_url,
               device_auth, tuner_count, last_scan_at, last_guide_sync_at, created_at, updated_at
        FROM livetv_tuners WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| TunerRow {
        id: r.id,
        device_id: r.device_id,
        friendly_name: r.friendly_name,
        model_number: r.model_number,
        firmware_version: r.firmware_version,
        base_url: r.base_url,
        device_auth: r.device_auth,
        tuner_count: r.tuner_count,
        last_scan_at: r.last_scan_at,
        last_guide_sync_at: r.last_guide_sync_at,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }))
}

pub async fn list_all<'e, E>(exec: E) -> Result<Vec<TunerRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, device_id, friendly_name, model_number, firmware_version, base_url,
               device_auth, tuner_count, last_scan_at, last_guide_sync_at, created_at, updated_at
        FROM livetv_tuners ORDER BY created_at
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| TunerRow {
            id: r.id,
            device_id: r.device_id,
            friendly_name: r.friendly_name,
            model_number: r.model_number,
            firmware_version: r.firmware_version,
            base_url: r.base_url,
            device_auth: r.device_auth,
            tuner_count: r.tuner_count,
            last_scan_at: r.last_scan_at,
            last_guide_sync_at: r.last_guide_sync_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect())
}

pub async fn list_with_counts<'e, E>(exec: E) -> Result<Vec<TunerWithCounts>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT t.id, t.device_id, t.friendly_name, t.model_number, t.firmware_version, t.base_url,
               t.device_auth, t.tuner_count, t.last_scan_at, t.last_guide_sync_at,
               t.created_at, t.updated_at,
               (SELECT COUNT(*) FROM livetv_channels c WHERE c.tuner_id = t.id) AS "channel_count!",
               (SELECT COUNT(*) FROM livetv_channels c WHERE c.tuner_id = t.id AND c.enabled)
                   AS "enabled_channel_count!"
        FROM livetv_tuners t
        ORDER BY t.created_at
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| TunerWithCounts {
            tuner: TunerRow {
                id: r.id,
                device_id: r.device_id,
                friendly_name: r.friendly_name,
                model_number: r.model_number,
                firmware_version: r.firmware_version,
                base_url: r.base_url,
                device_auth: r.device_auth,
                tuner_count: r.tuner_count,
                last_scan_at: r.last_scan_at,
                last_guide_sync_at: r.last_guide_sync_at,
                created_at: r.created_at,
                updated_at: r.updated_at,
            },
            channel_count: r.channel_count,
            enabled_channel_count: r.enabled_channel_count,
        })
        .collect())
}

pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let res = sqlx::query!("DELETE FROM livetv_tuners WHERE id = $1", id)
        .execute(exec)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn mark_scanned<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE livetv_tuners SET last_scan_at = NOW(), updated_at = NOW() WHERE id = $1",
        id
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn mark_guide_synced<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE livetv_tuners SET last_guide_sync_at = NOW(), updated_at = NOW() WHERE id = $1",
        id
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn active_lease_counts<'e, E>(exec: E) -> Result<Vec<(Uuid, i64)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT tuner_id, COUNT(*) AS "count!"
        FROM livetv_tuner_leases
        WHERE released_at IS NULL
        GROUP BY tuner_id
        "#,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| (r.tuner_id, r.count)).collect())
}
