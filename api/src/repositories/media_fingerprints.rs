//! Media fingerprints repository (audio/visual fingerprints for skip detection).

use super::Result;
use sqlx::{Executor, Postgres, Row};
use uuid::Uuid;

#[derive(Debug)]
pub struct FingerprintRow {
    pub id: Uuid,
    pub fingerprint: Vec<u8>,
    pub duration_seconds: f64,
}

pub async fn upsert<'e, E>(
    exec: E,
    file_id: Uuid,
    file_type: &str,
    region: &str,
    fingerprint_bytes: &[u8],
    duration_seconds: f64,
) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let row = sqlx::query(
        r#"
        INSERT INTO media_fingerprints (id, file_id, file_type, region, fingerprint, duration_seconds)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (file_id, file_type, region) DO UPDATE
            SET fingerprint = EXCLUDED.fingerprint,
                duration_seconds = EXCLUDED.duration_seconds
        RETURNING id
        "#,
    )
    .bind(id)
    .bind(file_id)
    .bind(file_type)
    .bind(region)
    .bind(fingerprint_bytes)
    .bind(duration_seconds)
    .fetch_one(exec)
    .await?;
    Ok(row.get("id"))
}

pub async fn get<'e, E>(
    exec: E,
    file_id: Uuid,
    file_type: &str,
    region: &str,
) -> Result<Option<FingerprintRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT id, fingerprint, duration_seconds
        FROM media_fingerprints
        WHERE file_id = $1 AND file_type = $2 AND region = $3
        "#,
    )
    .bind(file_id)
    .bind(file_type)
    .bind(region)
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| FingerprintRow {
        id: r.get("id"),
        fingerprint: r.get("fingerprint"),
        duration_seconds: r.get("duration_seconds"),
    }))
}

pub async fn delete_for_file<'e, E>(exec: E, file_id: Uuid, file_type: &str) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result =
        sqlx::query("DELETE FROM media_fingerprints WHERE file_id = $1 AND file_type = $2")
            .bind(file_id)
            .bind(file_type)
            .execute(exec)
            .await?;
    Ok(result.rows_affected())
}
