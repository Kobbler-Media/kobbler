use super::hdhomerun::{self, GuideChannel, GuideProgramme};
use crate::repositories::{RepositoryError, livetv_channels, livetv_guide, livetv_tuners};
use sqlx::PgPool;
use std::collections::HashMap;
use time::OffsetDateTime;
use uuid::Uuid;

pub const PRUNE_AFTER_HOURS: i64 = 24;

#[derive(Debug, thiserror::Error)]
pub enum GuideSyncError {
    #[error(transparent)]
    Device(#[from] hdhomerun::HdhrError),
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    #[error("tuner {0} not found")]
    TunerNotFound(Uuid),
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct GuideSyncStats {
    pub channels: usize,
    pub programmes: usize,
    pub upserted: usize,
    pub unmatched_lcns: usize,
    pub unmatched_programmes: usize,
    pub pruned: u64,
    pub removed: u64,
    pub safe_episode_keys: usize,
    pub title_only_keys: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpisodeKeyKind {
    Episode,
    Subtitle,
    Airdate,
    Title,
}

impl EpisodeKeyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EpisodeKeyKind::Episode => "episode",
            EpisodeKeyKind::Subtitle => "subtitle",
            EpisodeKeyKind::Airdate => "airdate",
            EpisodeKeyKind::Title => "title",
        }
    }

    /// A title-only key collapses every airing of `Home Shopping` into one blob,
    /// so the series-rule expander must never auto-record on it.
    pub fn is_safe(self) -> bool {
        self != EpisodeKeyKind::Title
    }
}

fn normalize(s: &str) -> String {
    s.trim().to_lowercase()
}

pub fn episode_key(p: &GuideProgramme) -> (String, EpisodeKeyKind) {
    let base = p
        .series_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("title:{}", normalize(&p.title)));

    if let Some(num) = p
        .episode_number
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return (
            format!("{base}|E:{}", num.to_uppercase()),
            EpisodeKeyKind::Episode,
        );
    }
    if let Some(sub) = p
        .episode_title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return (
            format!("{base}|S:{}", normalize(sub)),
            EpisodeKeyKind::Subtitle,
        );
    }
    if let Some(air) = p.original_airdate.filter(|v| *v > 0) {
        return (format!("{base}|D:{air}"), EpisodeKeyKind::Airdate);
    }
    (format!("{base}|X"), EpisodeKeyKind::Title)
}

fn to_time(epoch: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(epoch).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

pub async fn ingest(
    pool: &PgPool,
    tuner_id: Uuid,
    feed: &[GuideChannel],
) -> Result<GuideSyncStats, GuideSyncError> {
    let lcn_map: HashMap<i32, Uuid> = livetv_channels::lcn_map(pool, tuner_id)
        .await?
        .into_iter()
        .collect();

    let mut stats = GuideSyncStats::default();

    for entry in feed {
        stats.channels += 1;
        stats.programmes += entry.guide.len();

        let Some(channel_id) = entry.lcn().and_then(|lcn| lcn_map.get(&lcn).copied()) else {
            stats.unmatched_lcns += 1;
            stats.unmatched_programmes += entry.guide.len();
            continue;
        };

        livetv_channels::apply_guide_identity(
            pool,
            channel_id,
            entry.guide_name.as_deref(),
            entry.affiliate.as_deref(),
            entry.image_url.as_deref(),
        )
        .await?;

        // What this channel's feed covers, so anything left inside that span which the
        // feed no longer lists can be dropped once the upserts are in.
        let mut kept: Vec<OffsetDateTime> = Vec::with_capacity(entry.guide.len());

        for p in &entry.guide {
            let (key, kind) = episode_key(p);
            kept.push(to_time(p.start_time));
            if kind.is_safe() {
                stats.safe_episode_keys += 1;
            } else {
                stats.title_only_keys += 1;
            }

            livetv_guide::upsert(
                pool,
                livetv_guide::UpsertGuideEntry {
                    channel_id,
                    start_time: to_time(p.start_time),
                    end_time: to_time(p.end_time),
                    title: p.title.clone(),
                    episode_number: p.episode_number.clone(),
                    episode_title: p.episode_title.clone(),
                    synopsis: p.synopsis.clone(),
                    original_airdate: p.original_airdate.map(to_time),
                    series_id: p.series_id.clone(),
                    genres: p.filter.clone(),
                    image_url: p.image_url.clone(),
                    poster_url: p.poster_url.clone(),
                    is_first: p.is_first(),
                    episode_key: Some(key),
                    episode_key_kind: Some(kind.as_str().to_string()),
                },
            )
            .await?;
            stats.upserted += 1;
        }

        if let (Some(from), Some(to)) = (kept.iter().min(), kept.iter().max()) {
            stats.removed +=
                livetv_guide::prune_missing(pool, channel_id, *from, *to, &kept).await?;
        }
    }

    stats.pruned = livetv_guide::prune_expired(pool, PRUNE_AFTER_HOURS).await?;
    livetv_tuners::mark_guide_synced(pool, tuner_id).await?;

    Ok(stats)
}

pub async fn sync_tuner(pool: &PgPool, tuner_id: Uuid) -> Result<GuideSyncStats, GuideSyncError> {
    let tuner = livetv_tuners::get_by_id(pool, tuner_id)
        .await?
        .ok_or(GuideSyncError::TunerNotFound(tuner_id))?;

    // The stored token is only ever refreshed by a scan, and the device reissues
    // it on its own — a rotated one made every sync fail silently until somebody
    // rescanned, which reads as "the guide is empty". Ask the device, which is on
    // the LAN and is the authority, and keep the stored copy as the fallback for
    // when it is briefly unreachable.
    let discovered = hdhomerun::discover(&tuner.base_url)
        .await
        .ok()
        .and_then(|d| d.device_auth);
    let device_auth = [discovered.as_deref(), tuner.device_auth.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|a| !a.is_empty())
        .ok_or(hdhomerun::HdhrError::MissingDeviceAuth)?
        .to_string();

    let feed = hdhomerun::guide(&device_auth).await?;
    ingest(pool, tuner_id, &feed).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUIDE_FIXTURE: &str = include_str!("../../tests/fixtures/livetv/guide.json");
    const LINEUP_FIXTURE: &str = include_str!("../../tests/fixtures/livetv/lineup.json");

    fn feed() -> Vec<GuideChannel> {
        serde_json::from_str(GUIDE_FIXTURE).expect("guide fixture parses")
    }

    #[test]
    fn episode_keys_prefer_episode_number_then_subtitle_then_airdate() {
        let mut p: GuideProgramme = serde_json::from_str(
            r#"{"StartTime":1,"EndTime":2,"Title":"Show","SeriesID":"S1",
                "EpisodeNumber":"S02E05","EpisodeTitle":"Pilot","OriginalAirdate":99}"#,
        )
        .unwrap();
        assert_eq!(
            episode_key(&p),
            ("S1|E:S02E05".into(), EpisodeKeyKind::Episode)
        );

        p.episode_number = None;
        assert_eq!(
            episode_key(&p),
            ("S1|S:pilot".into(), EpisodeKeyKind::Subtitle)
        );

        p.episode_title = None;
        assert_eq!(episode_key(&p), ("S1|D:99".into(), EpisodeKeyKind::Airdate));

        p.original_airdate = None;
        let (key, kind) = episode_key(&p);
        assert_eq!(key, "S1|X");
        assert!(!kind.is_safe());
    }

    #[test]
    fn a_missing_series_id_falls_back_to_the_normalized_title() {
        let p: GuideProgramme =
            serde_json::from_str(r#"{"StartTime":1,"EndTime":2,"Title":"  Home Shopping "}"#)
                .unwrap();
        let (key, kind) = episode_key(&p);
        assert_eq!(key, "title:home shopping|X");
        assert_eq!(kind, EpisodeKeyKind::Title);
    }

    #[test]
    fn real_guide_feed_yields_safe_keys_for_almost_every_programme() {
        let channels = feed();
        assert_eq!(channels.len(), 85);

        let mut total = 0usize;
        let mut safe = 0usize;
        let mut kinds: HashMap<&'static str, usize> = HashMap::new();
        for ch in &channels {
            for p in &ch.guide {
                total += 1;
                let (_, kind) = episode_key(p);
                *kinds.entry(kind.as_str()).or_default() += 1;
                if kind.is_safe() {
                    safe += 1;
                }
            }
        }

        assert_eq!(total, 449);
        assert_eq!(kinds.get("episode").copied().unwrap_or(0), 239);
        assert_eq!(kinds.get("subtitle").copied().unwrap_or(0), 23);
        assert_eq!(kinds.get("airdate").copied().unwrap_or(0), 179);
        assert_eq!(kinds.get("title").copied().unwrap_or(0), 8);
        assert_eq!(safe, 441);
        assert!(safe as f64 / total as f64 > 0.97);
    }

    #[test]
    fn every_guide_lcn_resolves_against_the_lineup() {
        let lineup: Vec<super::super::hdhomerun::LineupEntry> =
            serde_json::from_str(LINEUP_FIXTURE).unwrap();
        let known: std::collections::HashSet<i32> = lineup.iter().filter_map(|e| e.lcn()).collect();
        let unmatched = feed()
            .iter()
            .filter(|c| c.lcn().map(|l| !known.contains(&l)).unwrap_or(true))
            .count();
        assert_eq!(unmatched, 0, "guide LCNs must all resolve to a channel");
    }

    #[test]
    fn affiliate_coverage_is_partial_and_must_not_be_inferred() {
        let with_affiliate = feed().iter().filter(|c| c.affiliate.is_some()).count();
        assert_eq!(with_affiliate, 72);
    }
}
