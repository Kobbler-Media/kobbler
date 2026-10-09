//! Per-directory mtime cache used by `media_root_scan` to skip unchanged dirs.

use super::Result;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use time::OffsetDateTime;
use uuid::Uuid;

/// Load all cached (relative_path -> last_mtime) entries for a media root.
pub async fn load_all(
    pool: &PgPool,
    media_root_id: Uuid,
) -> Result<HashMap<String, OffsetDateTime>> {
    let rows = sqlx::query(
        "SELECT relative_path, last_mtime FROM media_root_dir_mtimes WHERE media_root_id = $1",
    )
    .bind(media_root_id)
    .fetch_all(pool)
    .await?;

    let mut out = HashMap::with_capacity(rows.len());
    for row in rows {
        let rel: String = row.get("relative_path");
        let mtime: OffsetDateTime = row.get("last_mtime");
        out.insert(rel, mtime);
    }
    Ok(out)
}

/// Apply a set of upserts and deletions in one transaction.
pub async fn apply_updates(
    pool: &PgPool,
    media_root_id: Uuid,
    upserts: &[(String, OffsetDateTime)],
    deletes: &[String],
) -> Result<()> {
    if upserts.is_empty() && deletes.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;

    for (rel_path, mtime) in upserts {
        sqlx::query(
            "INSERT INTO media_root_dir_mtimes (media_root_id, relative_path, last_mtime) \
             VALUES ($1, $2, $3) \
             ON CONFLICT (media_root_id, relative_path) DO UPDATE SET last_mtime = EXCLUDED.last_mtime",
        )
        .bind(media_root_id)
        .bind(rel_path)
        .bind(mtime)
        .execute(&mut *tx)
        .await?;
    }

    if !deletes.is_empty() {
        sqlx::query(
            "DELETE FROM media_root_dir_mtimes \
             WHERE media_root_id = $1 AND relative_path = ANY($2)",
        )
        .bind(media_root_id)
        .bind(deletes)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}
