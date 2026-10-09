use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct AdBreakRow {
    pub id: Uuid,
    pub recording_id: Uuid,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub method: String,
    pub confidence: f64,
    pub ads_known: i32,
    pub ads_total: i32,
    pub corrected_by: Option<Uuid>,
    pub corrected_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
}

#[derive(Debug)]
pub struct CreateAdBreak {
    pub recording_id: Uuid,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub method: String,
    pub confidence: f64,
    pub ads_known: i32,
    pub ads_total: i32,
}

macro_rules! to_row {
    ($r:expr) => {
        AdBreakRow {
            id: $r.id,
            recording_id: $r.recording_id,
            start_seconds: $r.start_seconds,
            end_seconds: $r.end_seconds,
            method: $r.method,
            confidence: $r.confidence,
            ads_known: $r.ads_known,
            ads_total: $r.ads_total,
            corrected_by: $r.corrected_by,
            corrected_at: $r.corrected_at,
            created_at: $r.created_at,
        }
    };
}

pub async fn create<'e, E>(exec: E, input: CreateAdBreak) -> Result<AdBreakRow>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let r = sqlx::query!(
        r#"
        INSERT INTO livetv_ad_breaks
            (id, recording_id, start_seconds, end_seconds, method, confidence, ads_known, ads_total)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
        RETURNING id, recording_id, start_seconds, end_seconds, method, confidence,
                  ads_known, ads_total, corrected_by, corrected_at, created_at
        "#,
        id,
        input.recording_id,
        input.start_seconds,
        input.end_seconds,
        input.method,
        input.confidence,
        input.ads_known,
        input.ads_total,
    )
    .fetch_one(exec)
    .await?;
    Ok(to_row!(r))
}

pub async fn list_for_recording<'e, E>(exec: E, recording_id: Uuid) -> Result<Vec<AdBreakRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, recording_id, start_seconds, end_seconds, method, confidence,
               ads_known, ads_total, corrected_by, corrected_at, created_at
        FROM livetv_ad_breaks WHERE recording_id = $1 ORDER BY start_seconds
        "#,
        recording_id
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| to_row!(r)).collect())
}

pub async fn list_for_recordings<'e, E>(exec: E, ids: &[Uuid]) -> Result<Vec<AdBreakRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query!(
        r#"
        SELECT id, recording_id, start_seconds, end_seconds, method, confidence,
               ads_known, ads_total, corrected_by, corrected_at, created_at
        FROM livetv_ad_breaks WHERE recording_id = ANY($1) ORDER BY recording_id, start_seconds
        "#,
        ids
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| to_row!(r)).collect())
}

pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<AdBreakRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, recording_id, start_seconds, end_seconds, method, confidence,
               ads_known, ads_total, corrected_by, corrected_at, created_at
        FROM livetv_ad_breaks WHERE id = $1
        "#,
        id
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| to_row!(r)))
}

pub async fn correct<'e, E>(
    exec: E,
    id: Uuid,
    start_seconds: f64,
    end_seconds: f64,
    corrected_by: Option<Uuid>,
) -> Result<Option<AdBreakRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        UPDATE livetv_ad_breaks
        SET start_seconds = $2, end_seconds = $3, corrected_by = $4, corrected_at = NOW()
        WHERE id = $1
        RETURNING id, recording_id, start_seconds, end_seconds, method, confidence,
                  ads_known, ads_total, corrected_by, corrected_at, created_at
        "#,
        id,
        start_seconds,
        end_seconds,
        corrected_by,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| to_row!(r)))
}

pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let r = sqlx::query!("DELETE FROM livetv_ad_breaks WHERE id = $1", id)
        .execute(exec)
        .await?;
    Ok(r.rows_affected() > 0)
}

/// A re-detection replaces its own marks; a boundary a user has corrected is
/// the ground truth the next pass is trying to reproduce, so it survives.
pub async fn delete_uncorrected_for_recording<'e, E>(exec: E, recording_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let r = sqlx::query!(
        "DELETE FROM livetv_ad_breaks WHERE recording_id = $1 AND corrected_at IS NULL",
        recording_id
    )
    .execute(exec)
    .await?;
    Ok(r.rows_affected())
}
