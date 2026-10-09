//! IPTV unmatched sources repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

#[derive(Debug)]
pub struct UnmatchedRow {
    pub id: Uuid,
    pub provider_id: Uuid,
    pub remote_stream_id: i64,
    pub remote_category_id: Option<i64>,
    pub remote_category_name: Option<String>,
    pub name: Option<String>,
    pub icon_url: Option<String>,
    pub epg_channel_id: Option<String>,
    pub stream_type: String,
    pub language: Option<String>,
    pub suggested_channel_id: Option<Uuid>,
    pub suggested_confidence: Option<f32>,
    pub dismissed: bool,
    pub last_seen_at: OffsetDateTime,
}

#[derive(Debug)]
pub struct UpsertUnmatched {
    pub provider_id: Uuid,
    pub remote_stream_id: i64,
    pub remote_category_id: Option<i64>,
    pub remote_category_name: Option<String>,
    pub name: String,
    pub icon_url: Option<String>,
    pub epg_channel_id: Option<String>,
    pub stream_type: Option<String>,
    pub language: Option<String>,
    pub suggested_channel_id: Option<Uuid>,
    pub suggested_confidence: Option<f32>,
}

// =============================================================================
// Operations
// =============================================================================

/// Upsert an unmatched source.
pub async fn upsert<'e, E>(exec: E, input: UpsertUnmatched) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let row = sqlx::query_scalar!(
        r#"
        INSERT INTO iptv_unmatched_sources (id, provider_id, remote_stream_id,
                                            remote_category_id, remote_category_name,
                                            name, icon_url, epg_channel_id,
                                            stream_type, language,
                                            suggested_channel_id, suggested_confidence,
                                            last_seen_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, COALESCE($9, 'live'), $10, $11, $12, NOW())
        ON CONFLICT (provider_id, remote_stream_id)
        DO UPDATE SET
            remote_category_id = EXCLUDED.remote_category_id,
            remote_category_name = EXCLUDED.remote_category_name,
            name = EXCLUDED.name,
            icon_url = EXCLUDED.icon_url,
            epg_channel_id = EXCLUDED.epg_channel_id,
            stream_type = EXCLUDED.stream_type,
            language = EXCLUDED.language,
            suggested_channel_id = EXCLUDED.suggested_channel_id,
            suggested_confidence = EXCLUDED.suggested_confidence,
            last_seen_at = NOW()
        RETURNING id
        "#,
        id,
        input.provider_id,
        input.remote_stream_id,
        input.remote_category_id,
        input.remote_category_name,
        input.name,
        input.icon_url,
        input.epg_channel_id,
        input.stream_type,
        input.language,
        input.suggested_channel_id,
        input.suggested_confidence,
    )
    .fetch_one(exec)
    .await?;

    Ok(row)
}

/// Bulk upsert unmatched sources using UNNEST for batch performance.
/// Processes in chunks of 1000 to stay within Postgres parameter limits.
/// Returns the total number of rows inserted/updated.
pub async fn bulk_upsert(
    conn: &mut sqlx::PgConnection,
    sources: &[UpsertUnmatched],
) -> Result<u64> {
    if sources.is_empty() {
        return Ok(0);
    }

    let mut total = 0u64;
    for chunk in sources.chunks(1000) {
        let mut ids = Vec::with_capacity(chunk.len());
        let mut provider_ids = Vec::with_capacity(chunk.len());
        let mut remote_stream_ids = Vec::with_capacity(chunk.len());
        let mut remote_category_ids: Vec<Option<i64>> = Vec::with_capacity(chunk.len());
        let mut remote_category_names: Vec<Option<String>> = Vec::with_capacity(chunk.len());
        let mut names: Vec<String> = Vec::with_capacity(chunk.len());
        let mut icon_urls: Vec<Option<String>> = Vec::with_capacity(chunk.len());
        let mut epg_channel_ids: Vec<Option<String>> = Vec::with_capacity(chunk.len());
        let mut stream_types: Vec<Option<String>> = Vec::with_capacity(chunk.len());
        let mut languages: Vec<Option<String>> = Vec::with_capacity(chunk.len());
        let mut suggested_channel_ids: Vec<Option<Uuid>> = Vec::with_capacity(chunk.len());
        let mut suggested_confidences: Vec<Option<f32>> = Vec::with_capacity(chunk.len());

        for src in chunk {
            ids.push(Uuid::now_v7());
            provider_ids.push(src.provider_id);
            remote_stream_ids.push(src.remote_stream_id);
            remote_category_ids.push(src.remote_category_id);
            remote_category_names.push(src.remote_category_name.clone());
            names.push(src.name.clone());
            icon_urls.push(src.icon_url.clone());
            epg_channel_ids.push(src.epg_channel_id.clone());
            stream_types.push(src.stream_type.clone());
            languages.push(src.language.clone());
            suggested_channel_ids.push(src.suggested_channel_id);
            suggested_confidences.push(src.suggested_confidence);
        }

        let result = sqlx::query(
            r#"
            INSERT INTO iptv_unmatched_sources
                (id, provider_id, remote_stream_id, remote_category_id, remote_category_name,
                 name, icon_url, epg_channel_id, stream_type, language,
                 suggested_channel_id, suggested_confidence, last_seen_at)
            SELECT
                u.id, u.provider_id, u.remote_stream_id, u.remote_category_id,
                u.remote_category_name, u.name, u.icon_url, u.epg_channel_id,
                COALESCE(u.stream_type, 'live'),
                u.language, u.suggested_channel_id, u.suggested_confidence, NOW()
            FROM UNNEST(
                $1::uuid[], $2::uuid[], $3::bigint[], $4::bigint[], $5::text[],
                $6::text[], $7::text[], $8::text[], $9::text[], $10::text[],
                $11::uuid[], $12::real[]
            ) AS u(id, provider_id, remote_stream_id, remote_category_id, remote_category_name,
                   name, icon_url, epg_channel_id, stream_type, language,
                   suggested_channel_id, suggested_confidence)
            ON CONFLICT (provider_id, remote_stream_id)
            DO UPDATE SET
                remote_category_id = EXCLUDED.remote_category_id,
                remote_category_name = EXCLUDED.remote_category_name,
                name = EXCLUDED.name,
                icon_url = EXCLUDED.icon_url,
                epg_channel_id = EXCLUDED.epg_channel_id,
                stream_type = EXCLUDED.stream_type,
                language = EXCLUDED.language,
                suggested_channel_id = EXCLUDED.suggested_channel_id,
                suggested_confidence = EXCLUDED.suggested_confidence,
                last_seen_at = NOW()
            "#,
        )
        .bind(&ids)
        .bind(&provider_ids)
        .bind(&remote_stream_ids)
        .bind(&remote_category_ids as &[Option<i64>])
        .bind(&remote_category_names as &[Option<String>])
        .bind(&names)
        .bind(&icon_urls as &[Option<String>])
        .bind(&epg_channel_ids as &[Option<String>])
        .bind(&stream_types as &[Option<String>])
        .bind(&languages as &[Option<String>])
        .bind(&suggested_channel_ids as &[Option<Uuid>])
        .bind(&suggested_confidences as &[Option<f32>])
        .execute(&mut *conn)
        .await?;

        total += result.rows_affected();
    }

    Ok(total)
}

/// List pending (non-dismissed) unmatched sources with pagination.
pub async fn list_pending<'e, E>(exec: E, limit: i64, offset: i64) -> Result<Vec<UnmatchedRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, provider_id, remote_stream_id, remote_category_id, remote_category_name,
               name, icon_url, epg_channel_id, stream_type, language,
               suggested_channel_id, suggested_confidence, dismissed, last_seen_at
        FROM iptv_unmatched_sources
        WHERE dismissed = false
        ORDER BY name ASC
        LIMIT $1 OFFSET $2
        "#,
        limit,
        offset,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| UnmatchedRow {
            id: r.id,
            provider_id: r.provider_id,
            remote_stream_id: r.remote_stream_id,
            remote_category_id: r.remote_category_id,
            remote_category_name: r.remote_category_name,
            name: r.name,
            icon_url: r.icon_url,
            epg_channel_id: r.epg_channel_id,
            stream_type: r.stream_type,
            language: r.language,
            suggested_channel_id: r.suggested_channel_id,
            suggested_confidence: r.suggested_confidence,
            dismissed: r.dismissed,
            last_seen_at: r.last_seen_at,
        })
        .collect())
}

/// Assign an unmatched source to an existing channel.
/// Deletes from unmatched and inserts into iptv_sources.
/// Returns the new source ID.
pub async fn assign_to_channel(
    pool: &sqlx::PgPool,
    unmatched_id: Uuid,
    channel_id: Uuid,
) -> Result<Uuid> {
    let mut tx = pool.begin().await?;

    let unmatched = sqlx::query!(
        r#"
        DELETE FROM iptv_unmatched_sources WHERE id = $1
        RETURNING provider_id, remote_stream_id, remote_category_id, remote_category_name,
                  name, icon_url, epg_channel_id, stream_type
        "#,
        unmatched_id,
    )
    .fetch_one(&mut *tx)
    .await?;

    let source_id = Uuid::now_v7();
    sqlx::query!(
        r#"
        INSERT INTO iptv_sources (id, channel_id, provider_id, remote_stream_id,
                                  remote_category_id, remote_category_name, name, icon_url,
                                  epg_channel_id, stream_type, match_method, last_seen_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'manual', NOW())
        "#,
        source_id,
        channel_id,
        unmatched.provider_id,
        unmatched.remote_stream_id,
        unmatched.remote_category_id,
        unmatched.remote_category_name,
        unmatched.name,
        unmatched.icon_url,
        unmatched.epg_channel_id,
        unmatched.stream_type,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(source_id)
}

/// Create a new channel from an unmatched source, then move it to iptv_sources.
/// Returns (channel_id, source_id).
pub async fn create_channel_from(
    pool: &sqlx::PgPool,
    unmatched_id: Uuid,
    normalized_name: &str,
) -> Result<(Uuid, Uuid)> {
    let mut tx = pool.begin().await?;

    let unmatched = sqlx::query!(
        r#"
        DELETE FROM iptv_unmatched_sources WHERE id = $1
        RETURNING provider_id, remote_stream_id, remote_category_id, remote_category_name,
                  name, icon_url, epg_channel_id, stream_type, language
        "#,
        unmatched_id,
    )
    .fetch_one(&mut *tx)
    .await?;

    let channel_id = Uuid::now_v7();
    sqlx::query!(
        r#"
        INSERT INTO iptv_channels (id, name, normalized_name, icon_url, epg_channel_id, language)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
        channel_id,
        unmatched.name,
        normalized_name,
        unmatched.icon_url,
        unmatched.epg_channel_id,
        unmatched.language,
    )
    .execute(&mut *tx)
    .await?;

    let source_id = Uuid::now_v7();
    sqlx::query!(
        r#"
        INSERT INTO iptv_sources (id, channel_id, provider_id, remote_stream_id,
                                  remote_category_id, remote_category_name, name, icon_url,
                                  epg_channel_id, stream_type, match_method, last_seen_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'manual', NOW())
        "#,
        source_id,
        channel_id,
        unmatched.provider_id,
        unmatched.remote_stream_id,
        unmatched.remote_category_id,
        unmatched.remote_category_name,
        unmatched.name,
        unmatched.icon_url,
        unmatched.epg_channel_id,
        unmatched.stream_type,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok((channel_id, source_id))
}

/// Dismiss an unmatched source.
pub async fn dismiss<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE iptv_unmatched_sources SET dismissed = true WHERE id = $1
        "#,
        id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Count pending (non-dismissed) unmatched sources.
pub async fn count_pending<'e, E>(exec: E) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let count = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*) AS "count!" FROM iptv_unmatched_sources WHERE dismissed = false
        "#,
    )
    .fetch_one(exec)
    .await?;

    Ok(count)
}
