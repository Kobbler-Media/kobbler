use super::Result;
use sqlx::{Executor, PgPool, Postgres, QueryBuilder, Row};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ChannelRow {
    pub id: Uuid,
    pub tuner_id: Uuid,
    pub lcn: Option<i32>,
    pub scan_name: Option<String>,
    pub callsign: Option<String>,
    pub frequency: i64,
    pub program_number: i32,
    pub onid: Option<i32>,
    pub tsid: Option<i32>,
    pub modulation: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub hd: bool,
    pub signal_strength: Option<i32>,
    pub signal_quality: Option<i32>,
    pub last_seen_at: Option<OffsetDateTime>,
    pub enabled: bool,
    pub favourite: bool,
    pub sort_order: i32,
    pub display_name: Option<String>,
    pub logo_url: Option<String>,
    pub kind: String,
    pub region: Option<String>,
    pub network: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone)]
pub struct ChannelListRow {
    pub channel: ChannelRow,
    pub guide_entry_count: i64,
}

#[derive(Debug)]
pub struct UpsertScanChannel {
    pub tuner_id: Uuid,
    pub lcn: Option<i32>,
    pub scan_name: Option<String>,
    pub frequency: i64,
    pub program_number: i32,
    pub onid: Option<i32>,
    pub tsid: Option<i32>,
    pub modulation: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub hd: bool,
    pub signal_strength: Option<i32>,
    pub signal_quality: Option<i32>,
    pub initial_kind: String,
    pub initial_enabled: bool,
}

#[derive(Debug, Default)]
pub struct UpdateCuratedChannel {
    pub enabled: Option<bool>,
    pub favourite: Option<bool>,
    pub sort_order: Option<i32>,
    pub display_name: Option<Option<String>>,
    pub logo_url: Option<Option<String>>,
    pub kind: Option<String>,
    pub region: Option<Option<String>>,
    pub network: Option<Option<String>>,
}

#[derive(Debug, Default, Clone)]
pub struct ChannelFilters {
    pub tuner_id: Option<Uuid>,
    pub search: Option<String>,
    pub network: Option<String>,
    pub region: Option<String>,
    pub kind: Option<String>,
    pub hd: Option<bool>,
    pub enabled: Option<bool>,
    pub favourite: Option<bool>,
    pub has_guide: Option<bool>,
    pub mux: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct FacetSource {
    pub network: Option<String>,
    pub region: Option<String>,
    pub kind: String,
    pub hd: bool,
    pub video_codec: Option<String>,
    pub frequency: i64,
    pub enabled: bool,
    pub favourite: bool,
    pub has_guide: bool,
}

const CHANNEL_COLUMNS: &str = "id, tuner_id, lcn, scan_name, callsign, frequency, program_number, \
     onid, tsid, modulation, video_codec, audio_codec, hd, signal_strength, signal_quality, \
     last_seen_at, enabled, favourite, sort_order, display_name, logo_url, kind, region, network, \
     created_at, updated_at";

fn row_from(r: &sqlx::postgres::PgRow) -> ChannelRow {
    ChannelRow {
        id: r.get("id"),
        tuner_id: r.get("tuner_id"),
        lcn: r.get("lcn"),
        scan_name: r.get("scan_name"),
        callsign: r.get("callsign"),
        frequency: r.get("frequency"),
        program_number: r.get("program_number"),
        onid: r.get("onid"),
        tsid: r.get("tsid"),
        modulation: r.get("modulation"),
        video_codec: r.get("video_codec"),
        audio_codec: r.get("audio_codec"),
        hd: r.get("hd"),
        signal_strength: r.get("signal_strength"),
        signal_quality: r.get("signal_quality"),
        last_seen_at: r.get("last_seen_at"),
        enabled: r.get("enabled"),
        favourite: r.get("favourite"),
        sort_order: r.get("sort_order"),
        display_name: r.get("display_name"),
        logo_url: r.get("logo_url"),
        kind: r.get("kind"),
        region: r.get("region"),
        network: r.get("network"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

pub async fn upsert_scan<'e, E>(exec: E, input: UpsertScanChannel) -> Result<(ChannelRow, bool)>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    // A rescan overwrites the scan-derived columns only. `kind`, `enabled` and the
    // rest of the curated block are set once, on insert, and never again — that
    // separation is what lets curation survive a rescan or a bad lineup read.
    let r = sqlx::query!(
        r#"
        INSERT INTO livetv_channels (
            id, tuner_id, lcn, scan_name, frequency, program_number, onid, tsid, modulation,
            video_codec, audio_codec, hd, signal_strength, signal_quality, last_seen_at,
            kind, enabled)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, NOW(), $15, $16)
        ON CONFLICT (tuner_id, frequency, program_number) DO UPDATE SET
            lcn = EXCLUDED.lcn,
            scan_name = EXCLUDED.scan_name,
            onid = EXCLUDED.onid,
            tsid = EXCLUDED.tsid,
            modulation = EXCLUDED.modulation,
            video_codec = EXCLUDED.video_codec,
            audio_codec = EXCLUDED.audio_codec,
            hd = EXCLUDED.hd,
            signal_strength = EXCLUDED.signal_strength,
            signal_quality = EXCLUDED.signal_quality,
            last_seen_at = NOW(),
            updated_at = NOW()
        RETURNING id, tuner_id, lcn, scan_name, callsign, frequency, program_number, onid, tsid,
                  modulation, video_codec, audio_codec, hd, signal_strength, signal_quality,
                  last_seen_at, enabled, favourite, sort_order, display_name, logo_url, kind,
                  region, network, created_at, updated_at, (xmax = 0) AS "created!"
        "#,
        id,
        input.tuner_id,
        input.lcn,
        input.scan_name,
        input.frequency,
        input.program_number,
        input.onid,
        input.tsid,
        input.modulation,
        input.video_codec,
        input.audio_codec,
        input.hd,
        input.signal_strength,
        input.signal_quality,
        input.initial_kind,
        input.initial_enabled,
    )
    .fetch_one(exec)
    .await?;

    Ok((
        ChannelRow {
            id: r.id,
            tuner_id: r.tuner_id,
            lcn: r.lcn,
            scan_name: r.scan_name,
            callsign: r.callsign,
            frequency: r.frequency,
            program_number: r.program_number,
            onid: r.onid,
            tsid: r.tsid,
            modulation: r.modulation,
            video_codec: r.video_codec,
            audio_codec: r.audio_codec,
            hd: r.hd,
            signal_strength: r.signal_strength,
            signal_quality: r.signal_quality,
            last_seen_at: r.last_seen_at,
            enabled: r.enabled,
            favourite: r.favourite,
            sort_order: r.sort_order,
            display_name: r.display_name,
            logo_url: r.logo_url,
            kind: r.kind,
            region: r.region,
            network: r.network,
            created_at: r.created_at,
            updated_at: r.updated_at,
        },
        r.created,
    ))
}

/// A service that vanished from the lineup is switched off but never deleted, and
/// its `last_seen_at` is left stale — a bad scan must not destroy curation.
pub async fn disable_unseen<'e, E>(exec: E, tuner_id: Uuid, seen: &[Uuid]) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let res = sqlx::query!(
        r#"
        UPDATE livetv_channels
        SET enabled = FALSE, updated_at = NOW()
        WHERE tuner_id = $1 AND NOT (id = ANY($2)) AND enabled
        "#,
        tuner_id,
        seen,
    )
    .execute(exec)
    .await?;
    Ok(res.rows_affected())
}

pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<ChannelRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query(
        "SELECT id, tuner_id, lcn, scan_name, callsign, frequency, program_number, onid, tsid, \
         modulation, video_codec, audio_codec, hd, signal_strength, signal_quality, last_seen_at, \
         enabled, favourite, sort_order, display_name, logo_url, kind, region, network, \
         created_at, updated_at \
         FROM livetv_channels WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(exec)
    .await?;
    Ok(row.as_ref().map(row_from))
}

pub async fn lcn_map<'e, E>(exec: E, tuner_id: Uuid) -> Result<Vec<(i32, Uuid)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"SELECT lcn AS "lcn!", id FROM livetv_channels WHERE tuner_id = $1 AND lcn IS NOT NULL"#,
        tuner_id,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| (r.lcn, r.id)).collect())
}

/// `callsign` is scan-derived so the guide feed owns it outright, but `network`
/// is curated: set it the first time we see an Affiliate, never overwrite.
pub async fn apply_guide_identity<'e, E>(
    exec: E,
    id: Uuid,
    callsign: Option<&str>,
    network: Option<&str>,
    logo_url: Option<&str>,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE livetv_channels
        SET callsign = COALESCE($2, callsign),
            network = COALESCE(network, $3),
            logo_url = COALESCE(logo_url, $4),
            updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        callsign,
        network,
        logo_url,
    )
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn update_curated<'e, E>(
    exec: E,
    id: Uuid,
    input: UpdateCuratedChannel,
) -> Result<Option<ChannelRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        UPDATE livetv_channels SET
            enabled = COALESCE($2, enabled),
            favourite = COALESCE($3, favourite),
            sort_order = COALESCE($4, sort_order),
            display_name = CASE WHEN $5 THEN $6 ELSE display_name END,
            logo_url = CASE WHEN $7 THEN $8 ELSE logo_url END,
            kind = COALESCE($9, kind),
            region = CASE WHEN $10 THEN $11 ELSE region END,
            network = CASE WHEN $12 THEN $13 ELSE network END,
            updated_at = NOW()
        WHERE id = $1
        RETURNING id, tuner_id, lcn, scan_name, callsign, frequency, program_number, onid, tsid,
                  modulation, video_codec, audio_codec, hd, signal_strength, signal_quality,
                  last_seen_at, enabled, favourite, sort_order, display_name, logo_url, kind,
                  region, network, created_at, updated_at
        "#,
        id,
        input.enabled,
        input.favourite,
        input.sort_order,
        input.display_name.is_some(),
        input.display_name.flatten(),
        input.logo_url.is_some(),
        input.logo_url.flatten(),
        input.kind,
        input.region.is_some(),
        input.region.flatten(),
        input.network.is_some(),
        input.network.flatten(),
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| ChannelRow {
        id: r.id,
        tuner_id: r.tuner_id,
        lcn: r.lcn,
        scan_name: r.scan_name,
        callsign: r.callsign,
        frequency: r.frequency,
        program_number: r.program_number,
        onid: r.onid,
        tsid: r.tsid,
        modulation: r.modulation,
        video_codec: r.video_codec,
        audio_codec: r.audio_codec,
        hd: r.hd,
        signal_strength: r.signal_strength,
        signal_quality: r.signal_quality,
        last_seen_at: r.last_seen_at,
        enabled: r.enabled,
        favourite: r.favourite,
        sort_order: r.sort_order,
        display_name: r.display_name,
        logo_url: r.logo_url,
        kind: r.kind,
        region: r.region,
        network: r.network,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }))
}

pub async fn bulk_set<'e, E>(
    exec: E,
    ids: &[Uuid],
    enabled: Option<bool>,
    favourite: Option<bool>,
) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let res = sqlx::query!(
        r#"
        UPDATE livetv_channels
        SET enabled = COALESCE($2, enabled),
            favourite = COALESCE($3, favourite),
            updated_at = NOW()
        WHERE id = ANY($1)
        "#,
        ids,
        enabled,
        favourite,
    )
    .execute(exec)
    .await?;
    Ok(res.rows_affected())
}

pub async fn count_by_tuner<'e, E>(exec: E, tuner_id: Uuid) -> Result<(i64, i64)>
where
    E: Executor<'e, Database = Postgres>,
{
    let r = sqlx::query!(
        r#"
        SELECT COUNT(*) AS "total!", COUNT(*) FILTER (WHERE enabled) AS "enabled!"
        FROM livetv_channels WHERE tuner_id = $1
        "#,
        tuner_id,
    )
    .fetch_one(exec)
    .await?;
    Ok((r.total, r.enabled))
}

// The channel list is the one place dynamic SQL is justified: nine independent,
// all-optional filters plus four sort orders is 2^9 * 4 static queries otherwise.
fn push_filters(qb: &mut QueryBuilder<Postgres>, f: &ChannelFilters) {
    qb.push(" WHERE 1 = 1");
    if let Some(tuner_id) = f.tuner_id {
        qb.push(" AND c.tuner_id = ").push_bind(tuner_id);
    }
    if let Some(search) = f.search.as_ref().filter(|s| !s.trim().is_empty()) {
        let pattern = format!("%{}%", search.trim());
        qb.push(" AND (COALESCE(c.display_name, '') ILIKE ")
            .push_bind(pattern.clone())
            .push(" OR COALESCE(c.scan_name, '') ILIKE ")
            .push_bind(pattern.clone())
            .push(" OR COALESCE(c.callsign, '') ILIKE ")
            .push_bind(pattern.clone())
            .push(" OR COALESCE(c.network, '') ILIKE ")
            .push_bind(pattern)
            .push(")");
    }
    if let Some(v) = f.network.as_ref() {
        qb.push(" AND c.network = ").push_bind(v.clone());
    }
    if let Some(v) = f.region.as_ref() {
        qb.push(" AND c.region = ").push_bind(v.clone());
    }
    if let Some(v) = f.kind.as_ref() {
        qb.push(" AND c.kind = ").push_bind(v.clone());
    }
    if let Some(v) = f.hd {
        qb.push(" AND c.hd = ").push_bind(v);
    }
    if let Some(v) = f.enabled {
        qb.push(" AND c.enabled = ").push_bind(v);
    }
    if let Some(v) = f.favourite {
        qb.push(" AND c.favourite = ").push_bind(v);
    }
    if let Some(v) = f.mux {
        qb.push(" AND c.frequency = ").push_bind(v);
    }
    if let Some(v) = f.has_guide {
        qb.push(if v {
            " AND EXISTS (SELECT 1 FROM livetv_guide g WHERE g.channel_id = c.id)"
        } else {
            " AND NOT EXISTS (SELECT 1 FROM livetv_guide g WHERE g.channel_id = c.id)"
        });
    }
}

fn order_clause(sort: Option<&str>) -> &'static str {
    match sort.unwrap_or("lcn") {
        "name" => {
            " ORDER BY COALESCE(c.display_name, c.scan_name, c.callsign, ''), c.lcn NULLS LAST"
        }
        "signal" => " ORDER BY c.signal_quality DESC NULLS LAST, c.signal_strength DESC NULLS LAST",
        "recent" => " ORDER BY c.last_seen_at DESC NULLS LAST, c.lcn NULLS LAST",
        "favourite" => " ORDER BY c.favourite DESC, c.sort_order, c.lcn NULLS LAST",
        _ => " ORDER BY c.sort_order, c.lcn NULLS LAST, c.id",
    }
}

pub async fn list_filtered(
    pool: &PgPool,
    filters: &ChannelFilters,
    sort: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<(Vec<ChannelListRow>, i64)> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(format!(
        "SELECT {CHANNEL_COLUMNS}, \
         (SELECT COUNT(*) FROM livetv_guide g WHERE g.channel_id = c.id) AS guide_entry_count, \
         COUNT(*) OVER () AS total_count \
         FROM livetv_channels c"
    ));
    push_filters(&mut qb, filters);
    qb.push(order_clause(sort));
    qb.push(" LIMIT ").push_bind(limit);
    qb.push(" OFFSET ").push_bind(offset);

    let rows = qb.build().fetch_all(pool).await?;
    let total = rows
        .first()
        .map(|r| r.get::<i64, _>("total_count"))
        .unwrap_or(0);
    let items = rows
        .iter()
        .map(|r| ChannelListRow {
            channel: row_from(r),
            guide_entry_count: r.get("guide_entry_count"),
        })
        .collect();

    if rows.is_empty() {
        return Ok((items, count_filtered(pool, filters).await?));
    }
    Ok((items, total))
}

async fn count_filtered(pool: &PgPool, filters: &ChannelFilters) -> Result<i64> {
    let mut qb: QueryBuilder<Postgres> =
        QueryBuilder::new("SELECT COUNT(*) AS total FROM livetv_channels c");
    push_filters(&mut qb, filters);
    let row = qb.build().fetch_one(pool).await?;
    Ok(row.get::<i64, _>("total"))
}

pub async fn facet_source(pool: &PgPool, filters: &ChannelFilters) -> Result<Vec<FacetSource>> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT c.network, c.region, c.kind, c.hd, c.video_codec, c.frequency, c.enabled, \
         c.favourite, \
         EXISTS (SELECT 1 FROM livetv_guide g WHERE g.channel_id = c.id) AS has_guide \
         FROM livetv_channels c",
    );
    push_filters(&mut qb, filters);
    let rows = qb.build().fetch_all(pool).await?;
    Ok(rows
        .iter()
        .map(|r| FacetSource {
            network: r.get("network"),
            region: r.get("region"),
            kind: r.get("kind"),
            hd: r.get("hd"),
            video_codec: r.get("video_codec"),
            frequency: r.get("frequency"),
            enabled: r.get("enabled"),
            favourite: r.get("favourite"),
            has_guide: r.get("has_guide"),
        })
        .collect())
}

pub async fn list_for_guide<'e, E>(
    exec: E,
    channel_ids: Option<Vec<Uuid>>,
    enabled_only: bool,
) -> Result<Vec<ChannelRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query(
        "SELECT c.id, c.tuner_id, c.lcn, c.scan_name, c.callsign, c.frequency, c.program_number, \
         c.onid, c.tsid, c.modulation, c.video_codec, c.audio_codec, c.hd, c.signal_strength, \
         c.signal_quality, c.last_seen_at, c.enabled, c.favourite, c.sort_order, c.display_name, \
         c.logo_url, c.kind, c.region, c.network, c.created_at, c.updated_at \
         FROM livetv_channels c \
         WHERE ($1::uuid[] IS NULL OR c.id = ANY($1)) \
           AND (NOT $2 OR c.enabled) \
         ORDER BY c.sort_order, c.lcn NULLS LAST, c.id",
    )
    .bind(channel_ids)
    .bind(enabled_only)
    .fetch_all(exec)
    .await?;
    Ok(rows.iter().map(row_from).collect())
}

pub async fn physical_ids_with_guide<'e, E>(exec: E, tuner_id: Uuid) -> Result<Vec<(i64, i32)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT c.frequency, c.program_number
        FROM livetv_channels c
        WHERE c.tuner_id = $1
          AND EXISTS (SELECT 1 FROM livetv_guide g WHERE g.channel_id = c.id)
        "#,
        tuner_id,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.frequency, r.program_number))
        .collect())
}

pub async fn guide_entry_count<'e, E>(exec: E, channel_id: Uuid) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let r = sqlx::query!(
        r#"SELECT COUNT(*) AS "count!" FROM livetv_guide WHERE channel_id = $1"#,
        channel_id,
    )
    .fetch_one(exec)
    .await?;
    Ok(r.count)
}

/// Display name and LCN for a set of channels, for the recording list's
/// server-computed provenance line.
pub async fn labels_for<'e, E>(exec: E, ids: &[Uuid]) -> Result<Vec<(Uuid, String, Option<i32>)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, display_name, scan_name, callsign, lcn
        FROM livetv_channels WHERE id = ANY($1)
        "#,
        ids,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.id,
                crate::livetv::classify::display_name(
                    r.display_name.as_deref(),
                    r.scan_name.as_deref(),
                    r.callsign.as_deref(),
                ),
                r.lcn,
            )
        })
        .collect())
}
