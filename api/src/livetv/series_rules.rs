//! Series rules and repeat suppression (Decision 6).
//!
//! Half of a day's guide is duplicate airings, so the expander's real job is
//! deciding which one of nine airings to keep. Everything here is pure apart
//! from [`expand_all`], so `POST /series-rules/preview` can render the same
//! decisions live with no side effects.

use std::collections::{HashMap, HashSet};
use time::{Duration, OffsetDateTime, Time, Weekday};
use uuid::Uuid;

use crate::repositories::{
    RepositoryError, livetv_guide, livetv_recordings, livetv_series_rules, livetv_tuners,
};

/// Simulcast feeds drift 1-3 minutes, so equal start times are not a usable
/// identity. Five minutes covers every drift observed in the real feed without
/// swallowing a genuine later repeat.
pub const CLUSTER_TOLERANCE: Duration = Duration::minutes(5);

/// The free guide feed only reaches 48 h, which is why the expander re-runs on
/// every refresh instead of scheduling ahead.
pub const GUIDE_HORIZON: Duration = Duration::hours(48);

/// `new_only`'s second test, after "I don't already have this episode key".
/// Deliberately generous: an imported drama's `OriginalAirdate` is the overseas
/// première, not the local one, so a tight window would refuse first runs.
pub const NEW_EPISODE_MAX_AGE: Duration = Duration::days(90);

#[derive(Debug, Clone)]
pub struct Airing {
    pub guide_id: Uuid,
    pub channel_id: Uuid,
    pub channel_name: String,
    pub lcn: Option<i32>,
    pub hd: bool,
    pub signal_quality: Option<i32>,
    pub channel_enabled: bool,
    pub start_time: OffsetDateTime,
    pub end_time: OffsetDateTime,
    pub title: String,
    pub episode_number: Option<String>,
    pub episode_title: Option<String>,
    pub episode_key: Option<String>,
    pub episode_key_kind: Option<String>,
    pub series_id: Option<String>,
    pub synopsis: Option<String>,
    pub original_airdate: Option<OffsetDateTime>,
    pub image_url: Option<String>,
}

impl From<livetv_guide::AiringRow> for Airing {
    fn from(r: livetv_guide::AiringRow) -> Self {
        Self {
            guide_id: r.guide.id,
            channel_id: r.guide.channel_id,
            channel_name: r.channel_name,
            lcn: r.lcn,
            hd: r.hd,
            signal_quality: r.signal_quality,
            channel_enabled: r.channel_enabled,
            start_time: r.guide.start_time,
            end_time: r.guide.end_time,
            title: r.guide.title,
            episode_number: r.guide.episode_number,
            episode_title: r.guide.episode_title,
            episode_key: r.guide.episode_key,
            episode_key_kind: r.guide.episode_key_kind,
            series_id: r.guide.series_id,
            synopsis: r.guide.synopsis,
            original_airdate: r.guide.original_airdate,
            image_url: r.guide.image_url.or(r.guide.poster_url),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Record,
    Skip,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Record => "record",
            Action::Skip => "skip",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Member {
    pub airing: Airing,
    pub action: Action,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct Cluster {
    pub label: String,
    pub start_time: OffsetDateTime,
    pub members: Vec<Member>,
}

/// The rule as the expander reads it. `preview` builds one from an unsaved body,
/// so this deliberately does not carry an id.
#[derive(Debug, Clone)]
pub struct RuleSpec {
    pub series_id: String,
    pub channel_ids: Vec<Uuid>,
    /// 0 = Sunday .. 6 = Saturday, matching the editor's chip order.
    pub weekdays: Vec<i16>,
    pub window_start: Option<Time>,
    pub window_end: Option<Time>,
    pub new_only: bool,
    pub min_season: Option<i32>,
    pub on_clash: String,
    pub padding_before_secs: i32,
    pub padding_after_secs: i32,
}

impl From<&livetv_series_rules::SeriesRuleRow> for RuleSpec {
    fn from(r: &livetv_series_rules::SeriesRuleRow) -> Self {
        Self {
            series_id: r.series_id.clone(),
            channel_ids: r.channel_ids.clone(),
            weekdays: r.weekdays.clone(),
            window_start: r.window_start,
            window_end: r.window_end,
            new_only: r.new_only,
            min_season: r.min_season,
            on_clash: r.on_clash.clone(),
            padding_before_secs: r.padding_before_secs,
            padding_after_secs: r.padding_after_secs,
        }
    }
}

fn weekday_index(d: Weekday) -> i16 {
    match d {
        Weekday::Sunday => 0,
        Weekday::Monday => 1,
        Weekday::Tuesday => 2,
        Weekday::Wednesday => 3,
        Weekday::Thursday => 4,
        Weekday::Friday => 5,
        Weekday::Saturday => 6,
    }
}

/// `S03E07` -> 3. A rule's `min_season` is the only place this matters, so a
/// number we cannot read is never a reason to refuse.
fn season_of(episode_number: Option<&str>) -> Option<i32> {
    let raw = episode_number?.trim().to_ascii_uppercase();
    let rest = raw.strip_prefix('S')?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Anchored, not chained: a member joins the cluster its *first* member started,
/// never its previous one. Both agree on today's feed, but chained clustering
/// walks arbitrarily far given enough closely-spaced airings.
pub fn cluster_airings(airings: &[Airing]) -> Vec<Vec<Airing>> {
    let mut by_key: HashMap<String, Vec<Airing>> = HashMap::new();
    for a in airings {
        let key = a
            .episode_key
            .clone()
            .unwrap_or_else(|| format!("guide:{}", a.guide_id));
        by_key.entry(key).or_default().push(a.clone());
    }

    let mut clusters: Vec<Vec<Airing>> = Vec::new();
    for (_, mut group) in by_key {
        group.sort_by_key(|a| (a.start_time, a.lcn.unwrap_or(i32::MAX), a.guide_id));
        let mut current: Vec<Airing> = Vec::new();
        let mut anchor: Option<OffsetDateTime> = None;
        for a in group {
            match anchor {
                Some(t) if (a.start_time - t).abs() <= CLUSTER_TOLERANCE => current.push(a),
                _ => {
                    if !current.is_empty() {
                        clusters.push(std::mem::take(&mut current));
                    }
                    anchor = Some(a.start_time);
                    current.push(a);
                }
            }
        }
        if !current.is_empty() {
            clusters.push(current);
        }
    }
    clusters.sort_by_key(|c| (c[0].start_time, c[0].lcn.unwrap_or(i32::MAX), c[0].guide_id));
    clusters
}

/// HD beats SD, then signal quality, then the rule's preferred channel order.
fn best_member(members: &[&Airing], preferred: &[Uuid]) -> usize {
    let rank = |a: &Airing| {
        let pref = preferred
            .iter()
            .position(|id| *id == a.channel_id)
            .unwrap_or(usize::MAX);
        (
            !a.hd,
            -(a.signal_quality.unwrap_or(0) as i64),
            pref,
            a.lcn.unwrap_or(i32::MAX),
            a.guide_id,
        )
    };
    let mut best = 0usize;
    for i in 1..members.len() {
        if rank(members[i]) < rank(members[best]) {
            best = i;
        }
    }
    best
}

/// Why this airing can never be recorded by this rule, independent of what any
/// other airing is doing.
fn hard_skip(a: &Airing, rule: &RuleSpec, offset: time::UtcOffset) -> Option<String> {
    if !a.channel_enabled {
        return Some("channel is hidden".to_string());
    }
    if !rule.channel_ids.is_empty() && !rule.channel_ids.contains(&a.channel_id) {
        return Some(format!("{} is not on the rule's channels", a.channel_name));
    }
    // `SeriesID` alone does not separate a first run from a sister channel's
    // back catalogue; the weekday/time window is what does.
    let local = a.start_time.to_offset(offset);
    if !rule.weekdays.is_empty() && !rule.weekdays.contains(&weekday_index(local.weekday())) {
        return Some("outside the rule's days".to_string());
    }
    if let (Some(from), Some(to)) = (rule.window_start, rule.window_end) {
        let t = local.time();
        let inside = if from <= to {
            t >= from && t <= to
        } else {
            t >= from || t <= to
        };
        if !inside {
            return Some("outside the rule's time window".to_string());
        }
    }
    // Decision 6.4: a title-only key collapses `Home Shopping` into a single
    // 45-airing blob, so it is never safe to auto-record on.
    if a.episode_key_kind.as_deref() == Some("title") || a.episode_key.is_none() {
        return Some("no episode identity beyond the title".to_string());
    }
    if let Some(min) = rule.min_season
        && let Some(season) = season_of(a.episode_number.as_deref())
        && season < min
    {
        return Some(format!("season {season} is below the rule's minimum"));
    }
    None
}

/// Decide every airing of the rule's series inside the guide window.
///
/// `claimed` is the set of episode keys already recorded or already scheduled;
/// Decision 6.3 builds "new only" on exactly that rather than the `First` flag,
/// which in this feed is live sport and news.
pub fn evaluate(rule: &RuleSpec, airings: &[Airing], claimed: &HashSet<String>) -> Vec<Cluster> {
    let offset = crate::notifications::prefs::server_local_offset();
    let now = OffsetDateTime::now_utc();
    let clusters = cluster_airings(airings);

    let mut recorded_keys: HashSet<String> = HashSet::new();
    let mut out: Vec<Cluster> = Vec::new();

    for group in clusters {
        let label = group[0]
            .episode_number
            .clone()
            .or_else(|| group[0].episode_title.clone())
            .map(|s| format!("{} — {s}", group[0].title))
            .unwrap_or_else(|| group[0].title.clone());
        let start_time = group[0].start_time;
        let key = group[0].episode_key.clone().unwrap_or_default();

        let eligible: Vec<&Airing> = group
            .iter()
            .filter(|a| hard_skip(a, rule, offset).is_none())
            .collect();

        let already_have = claimed.contains(&key);
        let stale = rule.new_only
            && group[0]
                .original_airdate
                .is_some_and(|d| now - d > NEW_EPISODE_MAX_AGE);

        let winner: Option<Uuid> =
            if eligible.is_empty() || already_have || stale || recorded_keys.contains(&key) {
                None
            } else {
                let idx = best_member(&eligible, &rule.channel_ids);
                Some(eligible[idx].guide_id)
            };
        if winner.is_some() {
            recorded_keys.insert(key.clone());
        }

        let members = group
            .iter()
            .map(|a| {
                if let Some(reason) = hard_skip(a, rule, offset) {
                    return Member {
                        airing: a.clone(),
                        action: Action::Skip,
                        reason,
                    };
                }
                if winner == Some(a.guide_id) {
                    return Member {
                        airing: a.clone(),
                        action: Action::Record,
                        reason: if a.hd {
                            "best copy of this airing (HD)".to_string()
                        } else {
                            "best copy of this airing".to_string()
                        },
                    };
                }
                let reason = if already_have {
                    "already recorded or scheduled".to_string()
                } else if stale {
                    "not a new episode".to_string()
                } else if winner.is_none() {
                    "repeat of an earlier airing".to_string()
                } else {
                    "simultaneous duplicate of this airing".to_string()
                };
                Member {
                    airing: a.clone(),
                    action: Action::Skip,
                    reason,
                }
            })
            .collect();

        out.push(Cluster {
            label,
            start_time,
            members,
        });
    }

    out.sort_by_key(|c| c.start_time);
    out
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ExpandStats {
    pub rules: usize,
    pub clusters: usize,
    pub scheduled: usize,
    pub suppressed: usize,
    pub moved: usize,
    pub dropped: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum ExpandError {
    #[error(transparent)]
    Repository(#[from] RepositoryError),
}

pub(crate) async fn tuner_capacity(pool: &sqlx::PgPool) -> Result<u32, RepositoryError> {
    let tuners = livetv_tuners::list_all(pool).await?;
    Ok(tuners
        .iter()
        .map(|t| t.tuner_count.max(1) as u32)
        .sum::<u32>()
        .max(1))
}

/// How many distinct physical tunes are already committed across `span`. Two
/// recordings of the same channel share one tune, so they do not contend.
pub(crate) fn concurrent_tunes(
    booked: &[(OffsetDateTime, OffsetDateTime, Uuid)],
    start: OffsetDateTime,
    end: OffsetDateTime,
    channel_id: Uuid,
) -> u32 {
    let mut channels: HashSet<Uuid> = HashSet::new();
    for (s, e, ch) in booked {
        if *s < end && *e > start {
            channels.insert(*ch);
        }
    }
    channels.remove(&channel_id);
    channels.len() as u32
}

/// Re-runs on every guide refresh: the 48 h ceiling means a rule cannot plan a
/// week out, so scheduling once would leave the back half of every week empty.
pub async fn expand_all(pool: &sqlx::PgPool) -> Result<ExpandStats, ExpandError> {
    let rules = livetv_series_rules::list_all(pool, true).await?;
    let mut stats = ExpandStats {
        rules: rules.len(),
        ..Default::default()
    };
    if rules.is_empty() {
        return Ok(stats);
    }

    let now = OffsetDateTime::now_utc();
    let horizon = now + GUIDE_HORIZON;
    let capacity = tuner_capacity(pool).await?;
    let offset = crate::notifications::prefs::server_local_offset();

    let mut booked: Vec<(OffsetDateTime, OffsetDateTime, Uuid)> =
        livetv_recordings::list_overlapping(pool, now, horizon)
            .await?
            .into_iter()
            .map(|r| (r.scheduled_start, r.scheduled_end, r.channel_id))
            .collect();

    for rule in &rules {
        let spec = RuleSpec::from(rule);
        let airings: Vec<Airing> =
            livetv_guide::list_airings_for_series(pool, &rule.series_id, now, horizon)
                .await?
                .into_iter()
                .map(Airing::from)
                .collect();
        if airings.is_empty() {
            continue;
        }

        let keys: Vec<String> = airings
            .iter()
            .filter_map(|a| a.episode_key.clone())
            .collect();
        let claimed: HashSet<String> = livetv_recordings::claimed_episode_keys(pool, &keys)
            .await?
            .into_iter()
            .collect();

        let clusters = evaluate(&spec, &airings, &claimed);
        stats.clusters += clusters.len();

        for cluster in &clusters {
            for member in &cluster.members {
                if member.action != Action::Record {
                    stats.suppressed += 1;
                    continue;
                }
                let a = &member.airing;
                if livetv_recordings::get_by_guide_id(pool, a.guide_id)
                    .await?
                    .is_some()
                {
                    continue;
                }

                let start = a.start_time - Duration::seconds(spec.padding_before_secs as i64);
                let end = a.end_time + Duration::seconds(spec.padding_after_secs as i64);
                let row = create_from_airing(pool, rule, a, start, end).await?;

                if concurrent_tunes(&booked, start, end, a.channel_id) < capacity {
                    booked.push((start, end, a.channel_id));
                    stats.scheduled += 1;
                    continue;
                }

                // Decision 6.6: the guide usually airs it again, so move rather
                // than drop.
                let alternative = airings.iter().find(|other| {
                    other.guide_id != a.guide_id
                        && other.episode_key == a.episode_key
                        && other.start_time > a.start_time
                        && hard_skip(other, &spec, offset).is_none()
                        && concurrent_tunes(
                            &booked,
                            other.start_time - Duration::seconds(spec.padding_before_secs as i64),
                            other.end_time + Duration::seconds(spec.padding_after_secs as i64),
                            other.channel_id,
                        ) < capacity
                });

                match (spec.on_clash.as_str(), alternative) {
                    ("move", Some(alt)) => {
                        livetv_recordings::mark_moved(pool, row.id, alt.guide_id).await?;
                        let alt_start =
                            alt.start_time - Duration::seconds(spec.padding_before_secs as i64);
                        let alt_end =
                            alt.end_time + Duration::seconds(spec.padding_after_secs as i64);
                        create_from_airing(pool, rule, alt, alt_start, alt_end).await?;
                        booked.push((alt_start, alt_end, alt.channel_id));
                        stats.moved += 1;
                        stats.scheduled += 1;
                    }
                    ("ask", _) => {
                        livetv_recordings::set_status(
                            pool,
                            row.id,
                            "scheduled",
                            Some("all tuners are busy at this time"),
                        )
                        .await?;
                        booked.push((start, end, a.channel_id));
                        stats.scheduled += 1;
                    }
                    _ => {
                        livetv_recordings::set_status(
                            pool,
                            row.id,
                            "cancelled",
                            Some("all tuners are busy at this time"),
                        )
                        .await?;
                        stats.dropped += 1;
                    }
                }
            }
        }
    }

    Ok(stats)
}

async fn create_from_airing(
    pool: &sqlx::PgPool,
    rule: &livetv_series_rules::SeriesRuleRow,
    a: &Airing,
    start: OffsetDateTime,
    end: OffsetDateTime,
) -> Result<livetv_recordings::RecordingRow, RepositoryError> {
    livetv_recordings::create(
        pool,
        livetv_recordings::CreateRecording {
            channel_id: a.channel_id,
            guide_id: Some(a.guide_id),
            series_rule_id: Some(rule.id),
            title: a.title.clone(),
            episode_number: a.episode_number.clone(),
            episode_title: a.episode_title.clone(),
            episode_key: a.episode_key.clone(),
            series_id: a.series_id.clone(),
            synopsis: a.synopsis.clone(),
            scheduled_start: start,
            scheduled_end: end,
            actual_start: None,
            status: "scheduled".to_string(),
            output_dir: None,
            container: Some("hls".to_string()),
            video_codec: None,
            audio_codec: None,
            padding_before_secs: rule.padding_before_secs,
            padding_after_secs: rule.padding_after_secs,
            image_url: a.image_url.clone(),
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::livetv::guide_sync::episode_key;
    use crate::livetv::hdhomerun::GuideChannel;

    const GUIDE_FIXTURE: &str = include_str!("../../tests/fixtures/livetv/guide.json");

    /// The captured 85-channel feed, as the expander would see it. HD is unknown
    /// from a guide feed alone, so the picker's HD tier is exercised separately.
    fn fixture_airings() -> Vec<Airing> {
        let feed: Vec<GuideChannel> = serde_json::from_str(GUIDE_FIXTURE).expect("fixture parses");
        let mut out = Vec::new();
        for ch in &feed {
            let lcn = ch.lcn();
            for p in &ch.guide {
                let (key, kind) = episode_key(p);
                out.push(Airing {
                    guide_id: Uuid::new_v4(),
                    channel_id: Uuid::new_v4(),
                    channel_name: ch.guide_name.clone().unwrap_or_default(),
                    lcn,
                    hd: false,
                    signal_quality: Some(90),
                    channel_enabled: true,
                    start_time: OffsetDateTime::from_unix_timestamp(p.start_time).unwrap(),
                    end_time: OffsetDateTime::from_unix_timestamp(p.end_time).unwrap(),
                    title: p.title.clone(),
                    episode_number: p.episode_number.clone(),
                    episode_title: p.episode_title.clone(),
                    episode_key: Some(key),
                    episode_key_kind: Some(kind.as_str().to_string()),
                    series_id: p.series_id.clone(),
                    synopsis: p.synopsis.clone(),
                    original_airdate: p
                        .original_airdate
                        .and_then(|v| OffsetDateTime::from_unix_timestamp(v).ok()),
                    image_url: p.image_url.clone().or_else(|| p.poster_url.clone()),
                });
            }
        }
        out
    }

    fn spec(series_id: &str) -> RuleSpec {
        RuleSpec {
            series_id: series_id.to_string(),
            channel_ids: Vec::new(),
            weekdays: Vec::new(),
            window_start: None,
            window_end: None,
            new_only: false,
            min_season: None,
            on_clash: "move".to_string(),
            padding_before_secs: 120,
            padding_after_secs: 360,
        }
    }

    fn chained(airings: &[Airing]) -> Vec<Vec<Airing>> {
        let mut by_key: HashMap<String, Vec<Airing>> = HashMap::new();
        for a in airings {
            by_key
                .entry(a.episode_key.clone().unwrap_or_default())
                .or_default()
                .push(a.clone());
        }
        let mut out = Vec::new();
        for (_, mut group) in by_key {
            group.sort_by_key(|a| (a.start_time, a.lcn.unwrap_or(i32::MAX)));
            let mut current: Vec<Airing> = Vec::new();
            let mut prev: Option<OffsetDateTime> = None;
            for a in group {
                match prev {
                    Some(t) if (a.start_time - t).abs() <= CLUSTER_TOLERANCE => current.push(a),
                    _ => {
                        if !current.is_empty() {
                            out.push(std::mem::take(&mut current));
                        }
                        current.push(a);
                    }
                }
                prev = current.last().map(|a| a.start_time);
            }
            if !current.is_empty() {
                out.push(current);
            }
        }
        out
    }

    #[test]
    fn half_the_real_guide_is_a_duplicate_airing() {
        let airings = fixture_airings();
        assert_eq!(airings.len(), 449);

        let clusters = cluster_airings(&airings);
        let simultaneous: usize = clusters.iter().map(|c| c.len() - 1).sum();

        let mut per_key: HashMap<String, usize> = HashMap::new();
        for c in &clusters {
            *per_key
                .entry(c[0].episode_key.clone().unwrap_or_default())
                .or_default() += 1;
        }
        let later_repeats: usize = per_key.values().map(|n| n - 1).sum();
        let series_with_later_repeats = per_key.values().filter(|n| **n > 1).count();

        assert_eq!(clusters.len(), 258);
        assert_eq!(simultaneous, 191, "simulcast duplicates");
        assert_eq!(later_repeats, 35, "genuine later repeats");
        assert_eq!(series_with_later_repeats, 16);
        assert_eq!(
            simultaneous + later_repeats,
            226,
            "half the guide is a duplicate airing"
        );
        assert_eq!(
            chained(&airings).len(),
            clusters.len(),
            "anchored and chained agree on this feed"
        );
    }

    /// Chained clustering walks: four airings four minutes apart are one cluster
    /// spanning twelve minutes, which is not a simulcast of anything.
    #[test]
    fn anchored_clustering_refuses_to_walk_where_chained_does() {
        let base = OffsetDateTime::from_unix_timestamp(1_787_974_200).unwrap();
        let airings: Vec<Airing> = [0i64, 4, 8, 12]
            .iter()
            .map(|m| Airing {
                start_time: base + Duration::minutes(*m),
                end_time: base + Duration::minutes(*m + 30),
                ..fixture_airings()[0].clone()
            })
            .map(|mut a| {
                a.guide_id = Uuid::new_v4();
                a.episode_key = Some("S|E:S01E01".to_string());
                a
            })
            .collect();

        assert_eq!(chained(&airings).len(), 1, "chained walks the whole span");
        let anchored = cluster_airings(&airings);
        assert_eq!(anchored.len(), 2);
        assert_eq!(anchored[0].len(), 2);
        assert_eq!(anchored[1].len(), 2);
    }

    /// The real shape the spec describes, on real data: a simulcast group whose
    /// members drift by two minutes, plus a later repeat — one recording.
    #[test]
    fn a_drifting_simulcast_plus_a_later_repeat_records_once() {
        let airings: Vec<Airing> = fixture_airings()
            .into_iter()
            .filter(|a| a.series_id.as_deref() == Some("C33677068ENP2IK"))
            .collect();
        assert_eq!(airings.len(), 7, "Country House Facelift S02E05");

        let clusters = evaluate(&spec("C33677068ENP2IK"), &airings, &HashSet::new());
        assert_eq!(
            clusters.len(),
            2,
            "a drifting simulcast plus a later repeat"
        );
        assert_eq!(clusters[0].members.len(), 6);
        assert_eq!(clusters[1].members.len(), 1);

        // Two distinct start times inside the one cluster is the whole point:
        // dedup on equal start times would have made four of these unique.
        let starts: HashSet<OffsetDateTime> = clusters[0]
            .members
            .iter()
            .map(|m| m.airing.start_time)
            .collect();
        assert_eq!(starts.len(), 2);

        let recorded: Vec<&Member> = clusters
            .iter()
            .flat_map(|c| c.members.iter())
            .filter(|m| m.action == Action::Record)
            .collect();
        assert_eq!(recorded.len(), 1, "seven airings, one recording");
        assert!(
            clusters[0]
                .members
                .iter()
                .any(|m| m.action == Action::Record),
            "the earliest cluster is the one that records"
        );
        // The winner starts two minutes after the cluster's anchor, which is
        // exactly the drift that an equal-start-time dedup would have missed.
        assert_ne!(recorded[0].airing.start_time, clusters[0].start_time);
        assert_eq!(clusters[1].members[0].reason, "repeat of an earlier airing");
    }

    #[test]
    fn hd_wins_then_signal_quality_then_the_rules_preferred_channel() {
        let base = fixture_airings()
            .into_iter()
            .find(|a| a.series_id.as_deref() == Some("C33677068ENP2IK"))
            .unwrap();
        let mk = |hd: bool, quality: i32, lcn: i32| Airing {
            guide_id: Uuid::new_v4(),
            channel_id: Uuid::new_v4(),
            hd,
            signal_quality: Some(quality),
            lcn: Some(lcn),
            ..base.clone()
        };
        let sd_good = mk(false, 99, 1);
        let hd_poor = mk(true, 40, 50);
        let members = [&sd_good, &hd_poor];
        assert_eq!(best_member(&members, &[]), 1, "HD beats a better signal");

        let a = mk(true, 40, 5);
        let b = mk(true, 90, 6);
        assert_eq!(best_member(&[&a, &b], &[]), 1, "then signal quality");

        let c = mk(true, 90, 7);
        assert_eq!(
            best_member(&[&b, &c], &[c.channel_id]),
            1,
            "then the rule's preferred channel"
        );
    }

    /// `Home Shopping` is the reason the column exists: one key across unrelated
    /// channels all day. A rule must refuse to auto-record on it.
    #[test]
    fn a_title_only_key_never_auto_records() {
        let mut airings = fixture_airings();
        for a in &mut airings {
            a.episode_key_kind = Some("title".to_string());
        }
        let airings: Vec<Airing> = airings
            .into_iter()
            .filter(|a| a.series_id.as_deref() == Some("C33677068ENP2IK"))
            .collect();

        let clusters = evaluate(&spec("C33677068ENP2IK"), &airings, &HashSet::new());
        let recorded = clusters
            .iter()
            .flat_map(|c| c.members.iter())
            .filter(|m| m.action == Action::Record)
            .count();
        assert_eq!(recorded, 0);
        assert!(
            clusters[0].members[0]
                .reason
                .contains("no episode identity")
        );
    }

    #[test]
    fn an_episode_key_already_recorded_or_scheduled_is_skipped() {
        let airings: Vec<Airing> = fixture_airings()
            .into_iter()
            .filter(|a| a.series_id.as_deref() == Some("C33677068ENP2IK"))
            .collect();
        let claimed: HashSet<String> = airings[0].episode_key.clone().into_iter().collect();

        let clusters = evaluate(&spec("C33677068ENP2IK"), &airings, &claimed);
        let recorded = clusters
            .iter()
            .flat_map(|c| c.members.iter())
            .filter(|m| m.action == Action::Record)
            .count();
        assert_eq!(recorded, 0);
        assert_eq!(
            clusters[0].members[0].reason,
            "already recorded or scheduled"
        );
    }

    /// Decision 6.5: the same SeriesID covers a first run and a sister channel's
    /// back catalogue, so the weekday/time window is what separates them.
    #[test]
    fn a_time_window_separates_a_first_run_from_a_rerun_channel() {
        let airings: Vec<Airing> = fixture_airings()
            .into_iter()
            .filter(|a| a.series_id.as_deref() == Some("C33677068ENP2IK"))
            .collect();
        let offset = crate::notifications::prefs::server_local_offset();
        let late = airings
            .iter()
            .max_by_key(|a| a.start_time)
            .unwrap()
            .start_time
            .to_offset(offset)
            .time();

        let mut rule = spec("C33677068ENP2IK");
        rule.window_start = Some(late);
        rule.window_end = Some(late.replace_minute(59).unwrap());

        let clusters = evaluate(&rule, &airings, &HashSet::new());
        let recorded: Vec<&Member> = clusters
            .iter()
            .flat_map(|c| c.members.iter())
            .filter(|m| m.action == Action::Record)
            .collect();
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            recorded[0].airing.start_time.to_offset(offset).time(),
            late,
            "only the airing inside the window survives"
        );
    }

    #[test]
    fn season_numbers_come_off_the_episode_number() {
        assert_eq!(season_of(Some("S03E07")), Some(3));
        assert_eq!(season_of(Some("s12e01")), Some(12));
        assert_eq!(season_of(Some("E07")), None);
        assert_eq!(season_of(None), None);
    }
}
