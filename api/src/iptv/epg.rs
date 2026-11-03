//! XMLTV parser and EPG sync.
//!
//! Fetches XMLTV data from provider EPG URLs and syncs programme entries into
//! the `iptv_epg` table, keyed by channel + time range. Uses bulk UNNEST
//! inserts for performance (~303k entries from a 99MB XML file).

use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use sqlx::PgPool;
use time::OffsetDateTime;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::repositories;

/// Sync EPG data for a provider. Returns the number of entries synced.
pub async fn sync_epg(
    pool: &PgPool,
    http: &reqwest::Client,
    epg_url: &str,
    _provider_id: Uuid,
) -> Result<u32> {
    info!(url = epg_url, "fetching XMLTV EPG data");

    // Stream the response to avoid holding 99MB+ in memory
    let resp = http
        .get(epg_url)
        .timeout(std::time::Duration::from_secs(300))
        .send()
        .await
        .context("fetching XMLTV")?
        .error_for_status()
        .context("XMLTV response status")?;
    let body = resp.bytes().await.context("reading XMLTV body")?;

    // Build EPG channel_id → our channel_id mapping from iptv_sources
    let epg_map = build_epg_channel_map(pool).await?;
    if epg_map.is_empty() {
        warn!("no EPG channel mappings found, skipping EPG sync");
        return Ok(0);
    }

    info!(
        epg_mappings = epg_map.len(),
        body_size = body.len(),
        "parsing XMLTV"
    );

    // Parse and batch-insert in chunks to limit memory usage
    let programmes = parse_xmltv(&body, &epg_map)?;
    info!(programmes = programmes.len(), "parsed XMLTV programmes");

    let (synced, pruned) = insert_epg_entries(pool, &programmes).await?;

    info!(synced, pruned, "EPG sync complete");
    Ok(synced)
}

/// Sync EPG from a local file (for testing).
pub async fn sync_epg_from_file(pool: &PgPool, file_path: &std::path::Path) -> Result<u32> {
    let body = std::fs::read(file_path).context("reading EPG file")?;

    let epg_map = build_epg_channel_map(pool).await?;
    if epg_map.is_empty() {
        warn!("no EPG channel mappings found");
        return Ok(0);
    }

    info!(
        epg_mappings = epg_map.len(),
        body_size = body.len(),
        "parsing XMLTV from file"
    );

    let programmes = parse_xmltv(&body, &epg_map)?;
    info!(programmes = programmes.len(), "parsed XMLTV programmes");

    let (synced, pruned) = insert_epg_entries(pool, &programmes).await?;

    info!(synced, pruned, "EPG sync from file complete");
    Ok(synced)
}

/// Deduplicate, bulk insert, derive languages, and prune EPG entries.
async fn insert_epg_entries(pool: &PgPool, programmes: &[EpgEntry]) -> Result<(u32, u64)> {
    // Deduplicate by (channel_id, start_time, stop_time) — the UNNEST bulk insert
    // fails if the same unique key appears twice in one batch.
    let mut seen = std::collections::HashSet::new();
    let entries: Vec<repositories::iptv_epg::EpgEntry> = programmes
        .iter()
        .filter(|p| {
            let key = (
                p.channel_id,
                p.start_time.unix_timestamp(),
                p.stop_time.unix_timestamp(),
            );
            seen.insert(key)
        })
        .map(|p| repositories::iptv_epg::EpgEntry {
            channel_id: p.channel_id,
            title: p.title.clone(),
            description: p.description.clone(),
            start_time: p.start_time,
            stop_time: p.stop_time,
            categories: p.categories.clone(),
            episode_info: p.episode_info.clone(),
        })
        .collect();

    if entries.len() < programmes.len() {
        info!(
            before = programmes.len(),
            after = entries.len(),
            dupes = programmes.len() - entries.len(),
            "deduplicated EPG entries"
        );
    }

    let synced = repositories::iptv_epg::bulk_upsert(pool, &entries)
        .await
        .context("bulk upserting EPG entries")? as u32;

    // Derive language for channels from EPG programme languages
    derive_channel_languages_from_epg(pool, programmes).await;

    // Tiered prune: 7 days for favourite channels, 1 day for others
    let retention_days = repositories::iptv_epg::get_retention_days(pool).await;
    let pruned = repositories::iptv_epg::prune_tiered(pool, retention_days, 1)
        .await
        .unwrap_or(0);

    Ok((synced, pruned))
}

struct EpgEntry {
    channel_id: Uuid,
    title: Option<String>,
    description: Option<String>,
    start_time: OffsetDateTime,
    stop_time: OffsetDateTime,
    categories: Vec<String>,
    episode_info: Option<serde_json::Value>,
    language: Option<String>,
}

/// Build a mapping from EPG channel_id string → our UUID channel_id.
async fn build_epg_channel_map(pool: &PgPool) -> Result<HashMap<String, Uuid>> {
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT s.epg_channel_id, s.channel_id
        FROM iptv_sources s
        WHERE s.epg_channel_id IS NOT NULL
        "#,
    )
    .fetch_all(pool)
    .await
    .context("loading EPG channel map")?;

    let mut map = HashMap::with_capacity(rows.len());
    for row in rows {
        if let Some(ref epg_id) = row.epg_channel_id {
            map.insert(epg_id.clone(), row.channel_id);
        }
    }

    debug!(mappings = map.len(), "built EPG channel map");
    Ok(map)
}

/// Parse XMLTV XML into programme entries.
fn parse_xmltv(data: &[u8], epg_map: &HashMap<String, Uuid>) -> Result<Vec<EpgEntry>> {
    let mut reader = Reader::from_reader(data);

    let mut programmes = Vec::new();
    let mut buf = Vec::new();

    // State for current <programme>
    let mut in_programme = false;
    let mut current_channel_id: Option<Uuid> = None;
    let mut current_start: Option<OffsetDateTime> = None;
    let mut current_stop: Option<OffsetDateTime> = None;
    let mut current_title: Option<String> = None;
    let mut current_desc: Option<String> = None;
    let mut current_categories: Vec<String> = Vec::new();
    let mut current_episode_num: Option<String> = None;
    let mut current_language: Option<String> = None;
    let mut current_tag: Option<String> = None;
    let mut text_buf = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let local_name = e.local_name().as_ref().to_string();
                match local_name.as_str() {
                    "programme" => {
                        in_programme = true;
                        current_title = None;
                        current_desc = None;
                        current_categories.clear();
                        current_episode_num = None;
                        current_language = None;
                        current_channel_id = None;
                        current_start = None;
                        current_stop = None;

                        for attr in e.attributes().flatten() {
                            let key = attr.key.as_ref().to_string();
                            let val = crate::xml_text::attr_value(&attr);
                            match key.as_str() {
                                "channel" => {
                                    current_channel_id = epg_map.get(&val).copied();
                                }
                                "start" => {
                                    current_start = parse_xmltv_time(&val);
                                }
                                "stop" => {
                                    current_stop = parse_xmltv_time(&val);
                                }
                                _ => {}
                            }
                        }
                    }
                    tag if in_programme => {
                        current_tag = Some(tag.to_string());
                        text_buf.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) if current_tag.is_some() => text_buf.push_str(e.as_ref()),
            Ok(Event::GeneralRef(ref e)) if current_tag.is_some() => {
                crate::xml_text::push_ref(&mut text_buf, e);
            }
            Ok(Event::End(ref e)) => {
                let text = text_buf.trim();
                if let Some(tag) = current_tag.as_deref()
                    && !text.is_empty()
                {
                    let text = text.to_string();
                    match tag {
                        "title" => current_title = Some(text),
                        "desc" => current_desc = Some(text),
                        "category" => current_categories.push(text),
                        "episode-num" => current_episode_num = Some(text),
                        "language" => current_language = Some(text),
                        _ => {}
                    }
                }
                let local_name = e.local_name().as_ref().to_string();
                if local_name == "programme" && in_programme {
                    if let (Some(channel_id), Some(start), Some(stop)) =
                        (current_channel_id, current_start, current_stop)
                    {
                        let episode_info = current_episode_num
                            .as_ref()
                            .map(|num| serde_json::json!({ "episode_num": num }));
                        programmes.push(EpgEntry {
                            channel_id,
                            title: current_title.take(),
                            description: current_desc.take(),
                            start_time: start,
                            stop_time: stop,
                            categories: std::mem::take(&mut current_categories),
                            episode_info,
                            language: current_language.take(),
                        });
                    }
                    in_programme = false;
                }
                current_tag = None;
            }
            Ok(Event::Empty(_)) => {}
            Ok(Event::Eof) => break,
            Err(e) => {
                warn!(error = %e, "XMLTV parse error, continuing");
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(programmes)
}

/// For channels that have no language set, derive it from the most common
/// EPG programme language across their entries.
async fn derive_channel_languages_from_epg(pool: &PgPool, programmes: &[EpgEntry]) {
    // Build per-channel language frequency counts
    let mut channel_lang_counts: HashMap<Uuid, HashMap<String, u32>> = HashMap::new();
    for prog in programmes {
        if let Some(ref lang) = prog.language {
            *channel_lang_counts
                .entry(prog.channel_id)
                .or_default()
                .entry(lang.clone())
                .or_default() += 1;
        }
    }

    let mut updated = 0u32;
    for (channel_id, lang_counts) in &channel_lang_counts {
        let Some((best_lang, _)) = lang_counts.iter().max_by_key(|(_, count)| *count) else {
            continue;
        };

        let result = sqlx::query(
            r#"UPDATE iptv_channels SET language = $1
               WHERE id = $2 AND (language IS NULL OR language = '')"#,
        )
        .bind(best_lang)
        .bind(channel_id)
        .execute(pool)
        .await;

        if let Ok(r) = result
            && r.rows_affected() > 0
        {
            updated += 1;
        }
    }

    if updated > 0 {
        info!(
            channels_updated = updated,
            "derived channel languages from EPG"
        );
    }
}

/// Parse XMLTV time formats into `OffsetDateTime`.
fn parse_xmltv_time(s: &str) -> Option<OffsetDateTime> {
    let s = s.trim();

    // Try ISO 8601 first
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return chrono_to_offset_datetime(dt.with_timezone(&Utc));
    }
    if let Ok(dt) = DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%z") {
        return chrono_to_offset_datetime(dt.with_timezone(&Utc));
    }

    // XMLTV format: YYYYMMDDHHmmss [+/-HHMM]
    let (datetime_part, offset_part) = if s.len() > 14 {
        let dt = &s[..14];
        let rest = s[14..].trim();
        (dt, rest)
    } else {
        (s, "")
    };

    let naive = NaiveDateTime::parse_from_str(datetime_part, "%Y%m%d%H%M%S").ok()?;

    let offset_secs = if offset_part.is_empty() {
        0
    } else {
        parse_offset(offset_part)?
    };

    let utc_dt = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc)
        - chrono::Duration::seconds(offset_secs);
    chrono_to_offset_datetime(utc_dt)
}

fn parse_offset(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 5 {
        return None;
    }
    let sign = match s.as_bytes()[0] {
        b'+' => 1i64,
        b'-' => -1i64,
        _ => return None,
    };
    let hours: i64 = s[1..3].parse().ok()?;
    let mins: i64 = s[3..5].parse().ok()?;
    Some(sign * (hours * 3600 + mins * 60))
}

fn chrono_to_offset_datetime(dt: DateTime<Utc>) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(dt.timestamp()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_xmltv_time_bare() {
        let dt = parse_xmltv_time("20250101120000").unwrap();
        assert_eq!(dt.hour(), 12);
        assert_eq!(dt.month() as u8, 1);
    }

    #[test]
    fn parse_xmltv_time_with_positive_offset() {
        let dt = parse_xmltv_time("20250101120000 +0200").unwrap();
        assert_eq!(dt.hour(), 10);
    }

    #[test]
    fn parse_xmltv_time_with_negative_offset() {
        let dt = parse_xmltv_time("20250101120000 -0500").unwrap();
        assert_eq!(dt.hour(), 17);
    }

    #[test]
    fn parse_xmltv_time_iso8601() {
        let dt = parse_xmltv_time("2025-01-01T12:00:00+00:00").unwrap();
        assert_eq!(dt.hour(), 12);
    }

    #[test]
    fn parse_offset_positive() {
        assert_eq!(parse_offset("+0200"), Some(7200));
    }

    #[test]
    fn parse_offset_negative() {
        assert_eq!(parse_offset("-0500"), Some(-18000));
    }

    #[test]
    fn parse_xmltv_basic() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<tv>
  <programme start="20250101120000 +0000" stop="20250101130000 +0000" channel="bbc1">
    <title>News</title>
    <desc>The news</desc>
    <category>News</category>
  </programme>
</tv>"#;

        let mut map = HashMap::new();
        let id = Uuid::now_v7();
        map.insert("bbc1".to_string(), id);

        let result = parse_xmltv(xml, &map).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title.as_deref(), Some("News"));
        assert_eq!(result[0].channel_id, id);
        assert_eq!(result[0].categories, vec!["News"]);
    }

    #[test]
    fn parse_xmltv_resolves_entity_references() {
        let xml = br#"<tv>
  <programme start="20250101120000 +0000" stop="20250101130000 +0000" channel="bbc1">
    <title>Law &amp; Order</title>
    <desc>Benson &amp; Stabler &#8211; again</desc>
  </programme>
</tv>"#;
        let mut map = HashMap::new();
        map.insert("bbc1".to_string(), Uuid::now_v7());

        let result = parse_xmltv(xml, &map).unwrap();
        assert_eq!(result[0].title.as_deref(), Some("Law & Order"));
        assert_eq!(
            result[0].description.as_deref(),
            Some("Benson & Stabler \u{2013} again")
        );
    }

    #[test]
    fn parse_xmltv_unknown_channel_skipped() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<tv>
  <programme start="20250101120000" stop="20250101130000" channel="unknown">
    <title>Test</title>
  </programme>
</tv>"#;

        let map = HashMap::new();
        let result = parse_xmltv(xml, &map).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_xmltv_with_language() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<tv>
  <programme start="20250101120000 +0000" stop="20250101130000 +0000" channel="bbc1">
    <title>News</title>
    <language>en</language>
  </programme>
</tv>"#;

        let mut map = HashMap::new();
        let id = Uuid::now_v7();
        map.insert("bbc1".to_string(), id);

        let result = parse_xmltv(xml, &map).unwrap();
        assert_eq!(result[0].language.as_deref(), Some("en"));
    }
}
