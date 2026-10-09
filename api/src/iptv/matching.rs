//! Channel matching pipeline — stages 0-4.
//!
//! Matches raw provider streams to canonical channels through a cascade of
//! increasingly fuzzy strategies: aliases, iptv-org lookup, token-based fuzzy
//! matching, and finally auto-creation of new channels.
//!
//! Performance: streams are pre-grouped by normalized name so matching logic
//! runs once per unique name (~41k) instead of once per stream (~50k). Channel
//! creation is batched via UNNEST. Fuzzy matching uses first-character blocking
//! to avoid O(n*m) comparisons.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde_json::json;
use sqlx::PgPool;
use tracing::{debug, info};
use uuid::Uuid;

use super::iptv_org::IptvOrgDatabase;
use super::normalization;
use super::quality;
use super::xtream::XtreamStream;
use crate::repositories;
use crate::repositories::iptv_channels::CreateChannelWithId;
use crate::repositories::iptv_sources::UpsertSource;
use crate::repositories::iptv_unmatched::UpsertUnmatched;

/// Derive a canonical category from channel name patterns and remote category name.
///
/// Name patterns take priority (24/7, VOD, PPV), then remote category is mapped
/// to a normalized category string.
pub fn derive_category(name: &str, remote_category: Option<&str>) -> Option<String> {
    // Name prefix patterns
    if name.starts_with("24/7:") || name.starts_with("24/7 ") {
        return Some("24/7".into());
    }
    if name.contains("ᴿᴬᵂ") {
        return Some("vod".into());
    }
    if name.to_ascii_lowercase().contains("ppv") || name.starts_with("NO EVENT STREAMING") {
        return Some("ppv".into());
    }

    // Remote category mapping
    let rc = remote_category?.to_ascii_lowercase();
    if rc.contains("sport") {
        Some("sports".into())
    } else if rc.contains("movie") || rc.contains("film") || rc.contains("cinema") {
        Some("movies".into())
    } else if rc.contains("news") {
        Some("news".into())
    } else if rc.contains("kid")
        || rc.contains("child")
        || rc.contains("cartoon")
        || rc.contains("animation")
    {
        Some("kids".into())
    } else if rc.contains("music") {
        Some("music".into())
    } else if rc.contains("document") {
        Some("documentary".into())
    } else if rc.contains("entertain") || rc.contains("general") {
        Some("entertainment".into())
    } else if rc.contains("religious") {
        Some("religious".into())
    } else if rc.contains("lifestyle") {
        Some("lifestyle".into())
    } else {
        None
    }
}

pub struct MatchResult {
    pub channel_id: Uuid,
    /// How the match was made: "alias", "iptv_org", "token", "fuzzy", "new".
    pub method: String,
    pub confidence: f32,
}

pub struct MatchStats {
    pub matched: u32,
    pub new_channels: u32,
    pub unmatched: u32,
}

/// Per-stream data we carry through the pipeline after normalization.
struct StreamContext<'a> {
    stream: &'a XtreamStream,
    raw_name: String,
    normalized: normalization::NormalizedStream,
    category_id: Option<i64>,
    category_name: Option<String>,
}

/// Run the full matching pipeline on a batch of streams from a provider.
///
/// Streams are pre-grouped by (normalized_name, language, country) so that
/// matching logic and channel creation happen once per unique group. Channel
/// creation and source inserts are batched at the end using UNNEST.
#[allow(clippy::explicit_auto_deref)]
pub async fn match_streams(
    pool: &PgPool,
    provider_id: Uuid,
    streams: Vec<XtreamStream>,
    categories: &HashMap<String, String>,
    iptv_org_db: &IptvOrgDatabase,
    auto_create_channels: bool,
) -> Result<MatchStats> {
    let mut stats = MatchStats {
        matched: 0,
        new_channels: 0,
        unmatched: 0,
    };

    // =========================================================================
    // Phase 1: Normalize all streams and group by (normalized_name, language, country)
    // =========================================================================
    let mut groups: HashMap<(String, Option<String>, Option<String>), Vec<StreamContext>> =
        HashMap::new();

    for stream in &streams {
        let Some(stream_id) = stream.stream_id_i64() else {
            debug!(raw_id = ?stream.stream_id, "skipping stream with unparseable ID");
            continue;
        };

        let raw_name = match &stream.name {
            Some(n) if !n.trim().is_empty() => n.clone(),
            _ => {
                debug!(stream_id, "skipping stream with no name");
                continue;
            }
        };

        let mut normalized = normalization::normalize_stream_name(&raw_name);

        let category_id: Option<i64> = stream.category_id.as_deref().and_then(|s| s.parse().ok());
        let category_name: Option<String> = stream
            .category_id
            .as_deref()
            .and_then(|cid| categories.get(cid))
            .cloned();

        // Detect language from category if not detected from stream name
        if normalized.language.is_none() {
            normalized.language = category_name
                .as_deref()
                .and_then(normalization::detect_language_from_category)
                .map(String::from);
        }

        let key = (
            normalized.normalized_name.clone(),
            normalized.language.clone(),
            normalized.country.clone(),
        );

        groups.entry(key).or_default().push(StreamContext {
            stream,
            raw_name,
            normalized,
            category_id,
            category_name,
        });
    }

    info!(
        total_streams = streams.len(),
        unique_groups = groups.len(),
        "pre-grouped streams for matching"
    );

    // =========================================================================
    // Phase 2: Pre-load existing channels and build first-char index for fuzzy matching
    // =========================================================================
    let existing_channels = repositories::iptv_channels::list_with_source_counts(pool)
        .await
        .context("loading existing channels for matching")?;

    // First-character blocking index: maps first char -> list of channel indices
    let mut char_index: HashMap<u8, Vec<usize>> = HashMap::new();
    for (idx, ch) in existing_channels.iter().enumerate() {
        if let Some(first_byte) = ch.normalized_name.as_bytes().first() {
            char_index.entry(*first_byte).or_default().push(idx);
        }
    }

    // =========================================================================
    // Phase 3: Match each unique group — classify into matched/new/unmatched
    // =========================================================================
    // Accumulate results in memory: channel_id -> (method, confidence)
    // and the list of new channels to batch-create.
    type GroupKey = (String, Option<String>, Option<String>);
    type GroupResult = (Uuid, String, f32);
    let mut group_results: HashMap<GroupKey, GroupResult> = HashMap::with_capacity(groups.len());
    let mut unmatched_suggestions: HashMap<GroupKey, (Option<Uuid>, f32)> = HashMap::new();
    let mut new_channels: Vec<CreateChannelWithId> = Vec::new();

    let mut tx = pool
        .begin()
        .await
        .context("starting matching transaction")?;

    for (key, group_streams) in &groups {
        let (ref norm_name, _, _) = *key;
        let first_ctx = &group_streams[0];

        // Stage 1: Check iptv_channel_aliases for exact match
        if let Some(channel_id) = lookup_alias(&mut *tx, norm_name).await? {
            group_results.insert(key.clone(), (channel_id, "alias".into(), 1.0));
            continue;
        }

        // Stage 2: Check iptv-org database (prefer country-matched entry)
        if let Some(org_channel) =
            iptv_org_db.lookup(norm_name, first_ctx.normalized.country.as_deref())
        {
            let channel_id =
                find_or_create_channel_from_org(&mut *tx, org_channel, &first_ctx.normalized)
                    .await?;
            group_results.insert(key.clone(), (channel_id, "iptv_org".into(), 0.95));
            continue;
        }

        // Stage 3: Token-set fuzzy matching using first-character blocking
        let mut best_score = 0.0_f64;
        let mut best_channel_id = None;

        if let Some(first_byte) = norm_name.as_bytes().first()
            && let Some(candidates) = char_index.get(first_byte)
        {
            for &idx in candidates {
                let ch = &existing_channels[idx];
                let score = strsim::jaro_winkler(norm_name, &ch.normalized_name);
                if score > best_score {
                    best_score = score;
                    best_channel_id = Some(ch.id);
                }
            }
        }

        if best_score >= 0.88 {
            let channel_id = best_channel_id.expect("score > 0 implies a channel");
            group_results.insert(key.clone(), (channel_id, "fuzzy".into(), best_score as f32));
            continue;
        }

        // Stage 4: anything below the fuzzy-match threshold gets its own
        // channel row when auto-create is on. The previous gate (`< 0.40`)
        // silently dropped streams scoring 0.40–0.88 into iptv_unmatched_sources
        // where they had no row in iptv_channels and were invisible in the
        // channel list. Visible-but-noisy is the desired behavior — the user
        // can merge near-duplicates via the UI.
        if auto_create_channels {
            let channel_id = Uuid::now_v7();
            let language = first_ctx.normalized.language.clone();
            let country = first_ctx.normalized.country.clone();
            let icon_url = group_streams
                .iter()
                .find_map(|ctx| ctx.stream.stream_icon.clone())
                .filter(|u: &String| !u.is_empty());

            new_channels.push(CreateChannelWithId {
                id: channel_id,
                name: first_ctx.raw_name.clone(),
                normalized_name: norm_name.clone(),
                external_id: None,
                language: language.clone(),
                country: country.clone(),
                category: derive_category(&first_ctx.raw_name, first_ctx.category_name.as_deref()),
                icon_url,
            });

            group_results.insert(key.clone(), (channel_id, "new".into(), 1.0));
        } else if best_score >= 0.70 {
            // Auto-create disabled — record the fuzzy candidate so the
            // unmatched-review UI can still suggest a match.
            unmatched_suggestions.insert(key.clone(), (best_channel_id, best_score as f32));
        }
    }

    // =========================================================================
    // Phase 4: Batch-create all new channels
    // =========================================================================
    if !new_channels.is_empty() {
        let created = repositories::iptv_channels::bulk_create(&mut *tx, &new_channels)
            .await
            .context("batch creating new channels")?;
        info!(
            requested = new_channels.len(),
            created, "batch-created channels"
        );
    }

    // =========================================================================
    // Phase 5: Build source and unmatched vectors
    // =========================================================================
    let mut pending_sources: Vec<UpsertSource> = Vec::with_capacity(streams.len());
    let mut pending_unmatched: Vec<UpsertUnmatched> = Vec::new();

    for (key, group_streams) in &groups {
        if let Some((channel_id, method, confidence)) = group_results.get(key) {
            if method == "new" {
                stats.new_channels += group_streams.len() as u32;
            } else {
                stats.matched += group_streams.len() as u32;
            }
            for ctx in group_streams {
                pending_sources.push(build_source(
                    provider_id,
                    ctx.stream,
                    *channel_id,
                    &ctx.normalized,
                    ctx.category_id,
                    ctx.category_name.clone(),
                    method,
                    *confidence,
                ));
            }
        } else {
            // Unmatched group — include fuzzy suggestion if available
            let (suggested_id, suggested_conf) = unmatched_suggestions
                .get(key)
                .map(|(id, conf)| (*id, *conf))
                .unwrap_or((None, 0.0));
            stats.unmatched += group_streams.len() as u32;
            for ctx in group_streams {
                pending_unmatched.push(build_unmatched(
                    provider_id,
                    ctx.stream,
                    &ctx.normalized,
                    ctx.category_id,
                    suggested_id,
                    suggested_conf,
                ));
            }
        }
    }

    // =========================================================================
    // Phase 6: Batch insert all sources and unmatched
    // =========================================================================
    repositories::iptv_sources::bulk_upsert(&mut *tx, &pending_sources)
        .await
        .context("batch upserting matched sources")?;

    // Streams that resolved to a channel on this sync may have a stale row in
    // iptv_unmatched_sources from a prior sync (back when their score landed in
    // the 0.40–0.88 invisibility band). Clear those so the unmatched-review UI
    // doesn't display already-resolved entries.
    if !pending_sources.is_empty() {
        let resolved_ids: Vec<i64> = pending_sources.iter().map(|s| s.remote_stream_id).collect();
        sqlx::query!(
            r#"DELETE FROM iptv_unmatched_sources
               WHERE provider_id = $1 AND remote_stream_id = ANY($2)"#,
            provider_id,
            &resolved_ids,
        )
        .execute(&mut *tx)
        .await
        .context("clearing stale unmatched rows for resolved streams")?;
    }

    repositories::iptv_unmatched::bulk_upsert(&mut *tx, &pending_unmatched)
        .await
        .context("batch upserting unmatched sources")?;

    // =========================================================================
    // Phase 7: Backfill channel icons from sources
    // =========================================================================
    backfill_channel_icons(&mut *tx).await?;

    tx.commit()
        .await
        .context("committing matching transaction")?;

    info!(
        matched = stats.matched,
        new = stats.new_channels,
        unmatched = stats.unmatched,
        "matching pipeline complete"
    );

    Ok(stats)
}

/// Backfill `iptv_channels.icon_url` from the best available source icon.
pub async fn backfill_channel_icons<'e, E>(exec: E) -> Result<usize>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    // Failover order: priority ASC (0=best..100=worst, see quality::compute_priority),
    // matching idx_iptv_sources_failover. priority defaults to 50 for every row, so the
    // id tie-break is what actually decides most channels.
    let backfilled = sqlx::query_scalar::<_, i32>(
        r#"
        WITH best_icons AS (
            SELECT DISTINCT ON (s.channel_id) s.channel_id, s.icon_url
            FROM iptv_sources s
            JOIN iptv_channels c ON c.id = s.channel_id
            WHERE c.icon_url IS NULL
              AND s.icon_url IS NOT NULL
              AND s.icon_url != ''
            ORDER BY s.channel_id, s.priority ASC, s.health_score DESC, s.id ASC
        )
        UPDATE iptv_channels c
        SET icon_url = bi.icon_url, updated_at = now()
        FROM best_icons bi
        WHERE c.id = bi.channel_id
        RETURNING 1
        "#,
    )
    .fetch_all(exec)
    .await
    .context("backfilling channel icons from sources")?;

    if !backfilled.is_empty() {
        info!(
            count = backfilled.len(),
            "backfilled channel icons from sources"
        );
    }

    Ok(backfilled.len())
}
/// Look up a channel via the aliases table.
async fn lookup_alias<'e, E>(exec: E, normalized_name: &str) -> Result<Option<Uuid>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row = sqlx::query_scalar!(
        r#"SELECT channel_id FROM iptv_channel_aliases WHERE normalized_alias = $1"#,
        normalized_name,
    )
    .fetch_optional(exec)
    .await
    .context("looking up alias")?;

    Ok(row)
}

/// Find or create a channel based on iptv-org data.
async fn find_or_create_channel_from_org(
    tx: &mut sqlx::PgConnection,
    org_channel: &super::iptv_org::IptvOrgChannel,
    normalized: &normalization::NormalizedStream,
) -> Result<Uuid> {
    let language = normalized
        .language
        .clone()
        .or_else(|| org_channel.language.clone());
    let country = normalized.country.clone().or_else(|| {
        let code = normalization::normalize_country_code(&org_channel.country);
        if code == "XX" {
            None
        } else {
            Some(code.to_string())
        }
    });

    if let Some(existing) = repositories::iptv_channels::find_by_normalized_name(
        &mut *tx,
        &normalized.normalized_name,
        language.as_deref(),
        country.as_deref(),
    )
    .await?
    {
        return Ok(existing.id);
    }

    match repositories::iptv_channels::create(
        &mut *tx,
        repositories::iptv_channels::CreateChannel {
            name: org_channel.name.clone(),
            normalized_name: normalized.normalized_name.clone(),
            external_id: Some(org_channel.id.clone()),
            language: language.clone(),
            country,
            category: org_channel.categories.first().cloned(),
            icon_url: org_channel.logo.clone(),
            epg_channel_id: None,
        },
    )
    .await
    {
        Ok(channel) => Ok(channel.id),
        Err(_) => {
            let existing = repositories::iptv_channels::find_by_normalized_name(
                &mut *tx,
                &normalized.normalized_name,
                language.as_deref(),
                None,
            )
            .await?
            .context("channel should exist after unique constraint violation")?;
            Ok(existing.id)
        }
    }
}

/// Build an `UpsertSource` from stream data without inserting it.
#[allow(clippy::too_many_arguments)]
fn build_source(
    provider_id: Uuid,
    stream: &XtreamStream,
    channel_id: Uuid,
    normalized: &normalization::NormalizedStream,
    category_id: Option<i64>,
    category_name: Option<String>,
    method: &str,
    confidence: f32,
) -> UpsertSource {
    let stream_id = stream.stream_id_i64().expect("stream_id already validated");
    let priority = quality::compute_priority(&normalized.quality);

    let mut metadata = serde_json::Map::new();
    if let Some(ref archive) = stream.tv_archive {
        metadata.insert("tv_archive".into(), json!(archive));
    }
    if let Some(ref duration) = stream.tv_archive_duration {
        metadata.insert("tv_archive_duration".into(), json!(duration));
    }
    let metadata_value = if metadata.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(metadata))
    };

    UpsertSource {
        channel_id,
        provider_id,
        remote_stream_id: stream_id,
        remote_category_id: category_id,
        remote_category_name: category_name,
        name: stream.name.clone().unwrap_or_default(),
        icon_url: stream.stream_icon.clone(),
        epg_channel_id: stream.epg_channel_id.clone(),
        stream_type: stream.stream_type.clone(),
        quality: Some(normalized.quality.clone()),
        priority: Some(priority),
        match_method: Some(method.to_string()),
        match_confidence: Some(confidence),
        metadata: metadata_value,
    }
}

/// Build an `UpsertUnmatched` from stream data without inserting it.
fn build_unmatched(
    provider_id: Uuid,
    stream: &XtreamStream,
    normalized: &normalization::NormalizedStream,
    category_id: Option<i64>,
    suggested_channel_id: Option<Uuid>,
    suggested_confidence: f32,
) -> UpsertUnmatched {
    let stream_id = stream.stream_id_i64().expect("stream_id already validated");

    if suggested_channel_id.is_some() {
        debug!(
            stream_id,
            name = stream.name.as_deref().unwrap_or("?"),
            confidence = suggested_confidence,
            "staging unmatched stream with suggestion"
        );
    }

    UpsertUnmatched {
        provider_id,
        remote_stream_id: stream_id,
        remote_category_id: category_id,
        remote_category_name: None,
        name: stream.name.clone().unwrap_or_default(),
        icon_url: stream.stream_icon.clone(),
        epg_channel_id: stream.epg_channel_id.clone(),
        stream_type: stream.stream_type.clone(),
        language: normalized.language.clone(),
        suggested_channel_id,
        suggested_confidence: Some(suggested_confidence),
    }
}
