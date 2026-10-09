use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct AdFingerprintRow {
    pub id: Uuid,
    pub fingerprint: Vec<i32>,
    pub duration_secs: f64,
    pub label: Option<String>,
    pub origin: String,
    pub seen_count: i32,
    pub first_seen_at: OffsetDateTime,
    pub last_seen_at: OffsetDateTime,
}

#[derive(Debug)]
pub struct CreateAdFingerprint {
    pub fingerprint: Vec<i32>,
    pub duration_secs: f64,
    pub label: Option<String>,
    pub origin: String,
}

macro_rules! to_row {
    ($r:expr) => {
        AdFingerprintRow {
            id: $r.id,
            fingerprint: $r.fingerprint,
            duration_secs: $r.duration_secs,
            label: $r.label,
            origin: $r.origin,
            seen_count: $r.seen_count,
            first_seen_at: $r.first_seen_at,
            last_seen_at: $r.last_seen_at,
        }
    };
}

pub async fn create<'e, E>(exec: E, input: CreateAdFingerprint) -> Result<AdFingerprintRow>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let r = sqlx::query!(
        r#"
        INSERT INTO livetv_ad_fingerprints (id, fingerprint, duration_secs, label, origin)
        VALUES ($1,$2,$3,$4,$5)
        RETURNING id, fingerprint, duration_secs, label, origin, seen_count,
                  first_seen_at, last_seen_at
        "#,
        id,
        &input.fingerprint,
        input.duration_secs,
        input.label,
        input.origin,
    )
    .fetch_one(exec)
    .await?;
    Ok(to_row!(r))
}

/// Newest first: the bank is rolling, and a print nobody has matched in months
/// is an ad that has stopped airing.
pub async fn list_bank<'e, E>(exec: E, limit: i64) -> Result<Vec<AdFingerprintRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, fingerprint, duration_secs, label, origin, seen_count,
               first_seen_at, last_seen_at
        FROM livetv_ad_fingerprints
        ORDER BY origin = 'confirmed' DESC, last_seen_at DESC
        LIMIT $1
        "#,
        limit
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| to_row!(r)).collect())
}

pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<AdFingerprintRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, fingerprint, duration_secs, label, origin, seen_count,
               first_seen_at, last_seen_at
        FROM livetv_ad_fingerprints WHERE id = $1
        "#,
        id
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| to_row!(r)))
}

pub async fn mark_seen<'e, E>(exec: E, id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE livetv_ad_fingerprints SET seen_count = seen_count + 1, last_seen_at = NOW() WHERE id = $1",
        id
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Keeps the bank rolling: auto prints that never matched again are noise, and
/// a confirmed print is a user's correction and is never swept.
pub async fn prune_stale<'e, E>(exec: E, older_than_days: i64) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let r = sqlx::query!(
        r#"
        DELETE FROM livetv_ad_fingerprints
        WHERE origin = 'auto'
          AND seen_count <= 1
          AND last_seen_at < NOW() - make_interval(days => $1::int)
        "#,
        older_than_days as i32
    )
    .execute(exec)
    .await?;
    Ok(r.rows_affected())
}
