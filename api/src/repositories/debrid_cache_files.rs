use super::Result;
use sqlx::{Executor, PgPool, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Default)]
pub struct CacheRelease {
    pub debrid_provider_id: Option<Uuid>,
    pub info_hash: Option<String>,
    pub file_id: Option<i32>,
    pub filename: String,
}

#[derive(Debug, Clone)]
pub struct CachedFile {
    pub file_path: String,
    pub file_size: i64,
}

pub async fn upsert<'e, E>(
    exec: E,
    release: &CacheRelease,
    file_path: &str,
    file_size: i64,
    expires_at: OffsetDateTime,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO debrid_cache_files
            (file_path, file_size, expires_at, debrid_provider_id, info_hash, file_id, filename)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (file_path) DO UPDATE SET
            last_accessed_at = NOW(),
            expires_at = GREATEST(debrid_cache_files.expires_at, EXCLUDED.expires_at),
            file_size = EXCLUDED.file_size,
            debrid_provider_id = COALESCE(EXCLUDED.debrid_provider_id, debrid_cache_files.debrid_provider_id),
            info_hash = COALESCE(EXCLUDED.info_hash, debrid_cache_files.info_hash),
            file_id = COALESCE(EXCLUDED.file_id, debrid_cache_files.file_id),
            filename = EXCLUDED.filename
        "#,
        file_path,
        file_size,
        expires_at,
        release.debrid_provider_id,
        release.info_hash,
        release.file_id,
        release.filename,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Rows from before releases were recorded carry no provider and match any.
pub async fn find_release<'e, E>(
    exec: E,
    debrid_provider_id: Uuid,
    info_hash: &str,
    file_id: Option<i32>,
) -> Result<Option<CachedFile>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        CachedFile,
        r#"
        SELECT file_path, file_size
        FROM debrid_cache_files
        WHERE info_hash = $1
          AND file_id IS NOT DISTINCT FROM $2
          AND (debrid_provider_id IS NULL OR debrid_provider_id = $3)
        ORDER BY last_accessed_at DESC
        LIMIT 1
        "#,
        info_hash,
        file_id,
        debrid_provider_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row)
}

pub async fn touch<'e, E>(exec: E, file_path: &str) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "UPDATE debrid_cache_files SET last_accessed_at = NOW() WHERE file_path = $1",
        file_path,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn touch_for_session<'e, E>(exec: E, session_id: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE debrid_cache_files SET last_accessed_at = NOW()
        WHERE file_path = (SELECT cache_path FROM debrid_stream_sessions WHERE id = $1)
        "#,
        session_id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn set_pinned_for_session<'e, E>(exec: E, session_id: Uuid, pinned: bool) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let done = sqlx::query!(
        r#"
        UPDATE debrid_cache_files SET pinned = $2
        WHERE file_path = (SELECT cache_path FROM debrid_stream_sessions WHERE id = $1)
        "#,
        session_id,
        pinned,
    )
    .execute(exec)
    .await?;
    Ok(done.rows_affected())
}

/// Not touched within the hour: the session heartbeat touches a file every 30s while
/// it plays.
pub async fn expired<'e, E>(exec: E) -> Result<Vec<CachedFile>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        CachedFile,
        r#"
        SELECT file_path, file_size
        FROM debrid_cache_files
        WHERE NOT pinned
          AND expires_at < NOW()
          AND last_accessed_at < NOW() - INTERVAL '1 hour'
        "#,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

pub async fn unpinned_bytes<'e, E>(exec: E) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let total = sqlx::query_scalar!(
        r#"SELECT COALESCE(SUM(file_size)::bigint, 0) AS "total!: i64" FROM debrid_cache_files WHERE NOT pinned"#,
    )
    .fetch_one(exec)
    .await?;
    Ok(total)
}

pub async fn least_recently_used<'e, E>(exec: E) -> Result<Vec<CachedFile>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        CachedFile,
        r#"
        SELECT file_path, file_size
        FROM debrid_cache_files
        WHERE NOT pinned
          AND last_accessed_at < NOW() - INTERVAL '1 hour'
        ORDER BY last_accessed_at ASC
        "#,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows)
}

/// Forget a cache file. Sessions that pointed at it lose their local copy, so the next
/// play of them proxies again instead of reading a missing path.
pub async fn forget(pool: &PgPool, file_path: &str) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query!(
        "UPDATE debrid_stream_sessions SET cache_path = NULL, updated_at = NOW() WHERE cache_path = $1",
        file_path,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "DELETE FROM debrid_cache_files WHERE file_path = $1",
        file_path
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
