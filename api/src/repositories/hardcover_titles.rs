use sqlx::PgPool;
use time::OffsetDateTime;

use super::Result;
use crate::metadata::hardcover::MostReadTitle;

pub struct SyncState {
    pub attempted_at: Option<OffsetDateTime>,
    pub synced_at: Option<OffsetDateTime>,
}

pub async fn sync_state(pool: &PgPool) -> Result<SyncState> {
    let row = sqlx::query_as!(
        SyncState,
        "SELECT attempted_at, synced_at FROM hardcover_title_syncs"
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.unwrap_or(SyncState {
        attempted_at: None,
        synced_at: None,
    }))
}

pub async fn mark_attempted(pool: &PgPool) -> Result<()> {
    sqlx::query!(
        "INSERT INTO hardcover_title_syncs (attempted_at) VALUES (NOW())
         ON CONFLICT (singleton) DO UPDATE SET attempted_at = EXCLUDED.attempted_at"
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn book_provider_jobs_running(pool: &PgPool, job_types: &[&str]) -> Result<bool> {
    let job_types: Vec<String> = job_types.iter().map(|t| t.to_string()).collect();
    Ok(sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM job_status
               WHERE status IN ('running', 'retrying') AND job_type = ANY($1)
           ) AS "running!""#,
        &job_types,
    )
    .fetch_one(pool)
    .await?)
}

/// Swaps the whole set in one transaction, so suggestions never see a half-loaded list.
pub async fn replace(pool: &PgPool, titles: &[MostReadTitle]) -> Result<()> {
    let ids: Vec<i32> = titles.iter().map(|t| t.id).collect();
    let names: Vec<String> = titles.iter().map(|t| t.title.clone()).collect();
    let authors: Vec<Option<String>> = titles.iter().map(|t| t.author.clone()).collect();
    let users: Vec<i32> = titles.iter().map(|t| t.users_count).collect();

    let mut tx = pool.begin().await?;
    sqlx::query!("DELETE FROM hardcover_titles")
        .execute(&mut *tx)
        .await?;
    sqlx::query!(
        r#"
        INSERT INTO hardcover_titles (hardcover_id, title, author, users_count)
        SELECT * FROM UNNEST($1::int[], $2::text[], $3::text[], $4::int[])
        ON CONFLICT (hardcover_id) DO NOTHING
        "#,
        &ids,
        &names,
        &authors as &[Option<String>],
        &users,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "INSERT INTO hardcover_title_syncs (synced_at) VALUES (NOW())
         ON CONFLICT (singleton) DO UPDATE SET synced_at = EXCLUDED.synced_at"
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
