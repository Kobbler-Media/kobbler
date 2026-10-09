//! IPTV sources repository operations.

use super::Result;
use serde_json::Value;
use sqlx::{Executor, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

#[derive(Debug)]
pub struct SourceRow {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub provider_id: Uuid,
    pub remote_stream_id: i64,
    pub remote_category_id: Option<i64>,
    pub remote_category_name: Option<String>,
    pub name: Option<String>,
    pub icon_url: Option<String>,
    pub epg_channel_id: Option<String>,
    pub stream_type: String,
    pub quality: String,
    pub priority: i32,
    pub health_score: f32,
    pub last_health_check_at: Option<OffsetDateTime>,
    pub last_seen_at: OffsetDateTime,
    pub match_method: Option<String>,
    pub match_confidence: f32,
    pub metadata: Value,
}

#[derive(Debug)]
pub struct UpsertSource {
    pub channel_id: Uuid,
    pub provider_id: Uuid,
    pub remote_stream_id: i64,
    pub remote_category_id: Option<i64>,
    pub remote_category_name: Option<String>,
    pub name: String,
    pub icon_url: Option<String>,
    pub epg_channel_id: Option<String>,
    pub stream_type: Option<String>,
    pub quality: Option<String>,
    pub priority: Option<i32>,
    pub match_method: Option<String>,
    pub match_confidence: Option<f32>,
    pub metadata: Option<Value>,
}

// =============================================================================
// Operations
// =============================================================================

/// Upsert a source (insert or update on conflict of provider_id + remote_stream_id).
pub async fn upsert<'e, E>(exec: E, input: UpsertSource) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let row = sqlx::query_scalar!(
        r#"
        INSERT INTO iptv_sources (id, channel_id, provider_id, remote_stream_id,
                                  remote_category_id, remote_category_name, name, icon_url,
                                  epg_channel_id, stream_type, quality, priority,
                                  match_method, match_confidence, metadata, last_seen_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9,
                COALESCE($10, 'live'), COALESCE($11, 'unknown'), COALESCE($12, 50),
                $13, COALESCE($14, 1.0::real), COALESCE($15, '{}'::jsonb), NOW())
        ON CONFLICT (provider_id, remote_stream_id)
        DO UPDATE SET
            channel_id = EXCLUDED.channel_id,
            remote_category_id = EXCLUDED.remote_category_id,
            remote_category_name = EXCLUDED.remote_category_name,
            name = EXCLUDED.name,
            icon_url = EXCLUDED.icon_url,
            epg_channel_id = EXCLUDED.epg_channel_id,
            stream_type = EXCLUDED.stream_type,
            quality = EXCLUDED.quality,
            match_method = EXCLUDED.match_method,
            match_confidence = EXCLUDED.match_confidence,
            metadata = EXCLUDED.metadata,
            last_seen_at = NOW()
        RETURNING id
        "#,
        id,
        input.channel_id,
        input.provider_id,
        input.remote_stream_id,
        input.remote_category_id,
        input.remote_category_name,
        input.name,
        input.icon_url,
        input.epg_channel_id,
        input.stream_type,
        input.quality,
        input.priority,
        input.match_method,
        input.match_confidence,
        input.metadata,
    )
    .fetch_one(exec)
    .await?;

    Ok(row)
}

/// Bulk upsert sources using UNNEST for batch performance.
/// Processes in chunks of 1000 to stay within Postgres parameter limits.
/// Returns the total number of rows inserted/updated.
pub async fn bulk_upsert(conn: &mut sqlx::PgConnection, sources: &[UpsertSource]) -> Result<u64> {
    if sources.is_empty() {
        return Ok(0);
    }

    let mut total = 0u64;
    for chunk in sources.chunks(1000) {
        let mut ids = Vec::with_capacity(chunk.len());
        let mut channel_ids = Vec::with_capacity(chunk.len());
        let mut provider_ids = Vec::with_capacity(chunk.len());
        let mut remote_stream_ids = Vec::with_capacity(chunk.len());
        let mut remote_category_ids: Vec<Option<i64>> = Vec::with_capacity(chunk.len());
        let mut remote_category_names: Vec<Option<String>> = Vec::with_capacity(chunk.len());
        let mut names: Vec<String> = Vec::with_capacity(chunk.len());
        let mut icon_urls: Vec<Option<String>> = Vec::with_capacity(chunk.len());
        let mut epg_channel_ids: Vec<Option<String>> = Vec::with_capacity(chunk.len());
        let mut stream_types: Vec<Option<String>> = Vec::with_capacity(chunk.len());
        let mut qualities: Vec<Option<String>> = Vec::with_capacity(chunk.len());
        let mut priorities: Vec<Option<i32>> = Vec::with_capacity(chunk.len());
        let mut match_methods: Vec<Option<String>> = Vec::with_capacity(chunk.len());
        let mut match_confidences: Vec<Option<f32>> = Vec::with_capacity(chunk.len());
        let mut metadatas: Vec<Value> = Vec::with_capacity(chunk.len());

        for src in chunk {
            ids.push(Uuid::now_v7());
            channel_ids.push(src.channel_id);
            provider_ids.push(src.provider_id);
            remote_stream_ids.push(src.remote_stream_id);
            remote_category_ids.push(src.remote_category_id);
            remote_category_names.push(src.remote_category_name.clone());
            names.push(src.name.clone());
            icon_urls.push(src.icon_url.clone());
            epg_channel_ids.push(src.epg_channel_id.clone());
            stream_types.push(src.stream_type.clone());
            qualities.push(src.quality.clone());
            priorities.push(src.priority);
            match_methods.push(src.match_method.clone());
            match_confidences.push(src.match_confidence);
            metadatas.push(
                src.metadata
                    .clone()
                    .unwrap_or(Value::Object(Default::default())),
            );
        }

        let result = sqlx::query(
            r#"
            INSERT INTO iptv_sources (id, channel_id, provider_id, remote_stream_id,
                                      remote_category_id, remote_category_name, name, icon_url,
                                      epg_channel_id, stream_type, quality, priority,
                                      match_method, match_confidence, metadata, last_seen_at)
            SELECT
                u.id, u.channel_id, u.provider_id, u.remote_stream_id,
                u.remote_category_id, u.remote_category_name, u.name, u.icon_url,
                u.epg_channel_id,
                COALESCE(u.stream_type, 'live'),
                COALESCE(u.quality, 'unknown'),
                COALESCE(u.priority, 50),
                u.match_method,
                COALESCE(u.match_confidence, 1.0::real),
                u.metadata,
                NOW()
            FROM UNNEST(
                $1::uuid[], $2::uuid[], $3::uuid[], $4::bigint[],
                $5::bigint[], $6::text[], $7::text[], $8::text[],
                $9::text[], $10::text[], $11::text[], $12::integer[],
                $13::text[], $14::real[], $15::jsonb[]
            ) AS u(id, channel_id, provider_id, remote_stream_id,
                   remote_category_id, remote_category_name, name, icon_url,
                   epg_channel_id, stream_type, quality, priority,
                   match_method, match_confidence, metadata)
            ON CONFLICT (provider_id, remote_stream_id)
            DO UPDATE SET
                channel_id = EXCLUDED.channel_id,
                remote_category_id = EXCLUDED.remote_category_id,
                remote_category_name = EXCLUDED.remote_category_name,
                name = EXCLUDED.name,
                icon_url = EXCLUDED.icon_url,
                epg_channel_id = EXCLUDED.epg_channel_id,
                stream_type = EXCLUDED.stream_type,
                quality = EXCLUDED.quality,
                match_method = EXCLUDED.match_method,
                match_confidence = EXCLUDED.match_confidence,
                metadata = EXCLUDED.metadata,
                last_seen_at = NOW()
            "#,
        )
        .bind(&ids)
        .bind(&channel_ids)
        .bind(&provider_ids)
        .bind(&remote_stream_ids)
        .bind(&remote_category_ids as &[Option<i64>])
        .bind(&remote_category_names as &[Option<String>])
        .bind(&names)
        .bind(&icon_urls as &[Option<String>])
        .bind(&epg_channel_ids as &[Option<String>])
        .bind(&stream_types as &[Option<String>])
        .bind(&qualities as &[Option<String>])
        .bind(&priorities as &[Option<i32>])
        .bind(&match_methods as &[Option<String>])
        .bind(&match_confidences as &[Option<f32>])
        .bind(&metadatas)
        .execute(&mut *conn)
        .await?;

        total += result.rows_affected();
    }

    Ok(total)
}

/// List sources for a channel in failover order: priority ASC (0=best..100=worst), then
/// health_score DESC, then id ASC — priority alone ties on the schema default of 50.
pub async fn list_by_channel<'e, E>(exec: E, channel_id: Uuid) -> Result<Vec<SourceRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, channel_id, provider_id, remote_stream_id, remote_category_id,
               remote_category_name, name, icon_url, epg_channel_id, stream_type, quality,
               priority, health_score, last_health_check_at, last_seen_at,
               match_method, match_confidence, metadata
        FROM iptv_sources
        WHERE channel_id = $1
        ORDER BY priority ASC, health_score DESC, id ASC
        "#,
        channel_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| SourceRow {
            id: r.id,
            channel_id: r.channel_id,
            provider_id: r.provider_id,
            remote_stream_id: r.remote_stream_id,
            remote_category_id: r.remote_category_id,
            remote_category_name: r.remote_category_name,
            name: r.name,
            icon_url: r.icon_url,
            epg_channel_id: r.epg_channel_id,
            stream_type: r.stream_type,
            quality: r.quality,
            priority: r.priority,
            health_score: r.health_score,
            last_health_check_at: r.last_health_check_at,
            last_seen_at: r.last_seen_at,
            match_method: r.match_method,
            match_confidence: r.match_confidence,
            metadata: r.metadata,
        })
        .collect())
}

/// List sources for a provider.
pub async fn list_by_provider<'e, E>(exec: E, provider_id: Uuid) -> Result<Vec<SourceRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, channel_id, provider_id, remote_stream_id, remote_category_id,
               remote_category_name, name, icon_url, epg_channel_id, stream_type, quality,
               priority, health_score, last_health_check_at, last_seen_at,
               match_method, match_confidence, metadata
        FROM iptv_sources
        WHERE provider_id = $1
        ORDER BY name ASC
        "#,
        provider_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| SourceRow {
            id: r.id,
            channel_id: r.channel_id,
            provider_id: r.provider_id,
            remote_stream_id: r.remote_stream_id,
            remote_category_id: r.remote_category_id,
            remote_category_name: r.remote_category_name,
            name: r.name,
            icon_url: r.icon_url,
            epg_channel_id: r.epg_channel_id,
            stream_type: r.stream_type,
            quality: r.quality,
            priority: r.priority,
            health_score: r.health_score,
            last_health_check_at: r.last_health_check_at,
            last_seen_at: r.last_seen_at,
            match_method: r.match_method,
            match_confidence: r.match_confidence,
            metadata: r.metadata,
        })
        .collect())
}

/// Get catchup info for a set of channels.
/// Returns channel IDs that have at least one source with tv_archive support,
/// along with the max archive duration in days.
pub async fn get_catchup_channels(
    pool: &sqlx::PgPool,
    channel_ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, i32>> {
    if channel_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    // Failover order: priority ASC (0=best..100=worst, see quality::compute_priority),
    // matching idx_iptv_sources_failover. priority defaults to 50 for every row, so the
    // id tie-break is what actually decides most channels.
    let rows: Vec<(Uuid, Option<serde_json::Value>)> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (channel_id) channel_id, metadata
        FROM iptv_sources
        WHERE channel_id = ANY($1)
          AND metadata->>'tv_archive' = '1'
        ORDER BY channel_id, priority ASC, health_score DESC, id ASC
        "#,
    )
    .bind(channel_ids)
    .fetch_all(pool)
    .await?;

    let mut result = std::collections::HashMap::new();
    for (channel_id, metadata) in rows {
        let days = metadata
            .as_ref()
            .and_then(|m| m.get("tv_archive_duration"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(1);
        result.insert(channel_id, days);
    }
    Ok(result)
}

/// Update the health score of a source.
pub async fn update_health<'e, E>(exec: E, id: Uuid, score: f32) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE iptv_sources SET health_score = $2, last_health_check_at = NOW() WHERE id = $1
        "#,
        id,
        score,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Get the current health score for a source.
pub async fn get_health_score<'e, E>(exec: E, id: Uuid) -> Result<Option<f32>>
where
    E: Executor<'e, Database = Postgres>,
{
    let score = sqlx::query_scalar!(r#"SELECT health_score FROM iptv_sources WHERE id = $1"#, id,)
        .fetch_optional(exec)
        .await?;

    Ok(score)
}

/// Prune stale sources for a provider (last_seen_at before the given timestamp).
/// Returns the number of rows deleted.
pub async fn prune_stale<'e, E>(exec: E, provider_id: Uuid, before: OffsetDateTime) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        DELETE FROM iptv_sources
        WHERE provider_id = $1 AND last_seen_at < $2
        "#,
        provider_id,
        before,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected())
}

/// Count sources for a channel.
pub async fn count_by_channel<'e, E>(exec: E, channel_id: Uuid) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let count = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*) AS "count!" FROM iptv_sources WHERE channel_id = $1
        "#,
        channel_id,
    )
    .fetch_one(exec)
    .await?;

    Ok(count)
}
