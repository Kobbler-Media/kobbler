//! Watch-history rollup (Decision 14).
//!
//! Pure: no sqlx, no clock. The repository layer supplies [`WatchFact`]s and
//! stores whatever [`roll_up`] returns.

use std::collections::HashMap;
use time::{OffsetDateTime, UtcOffset, Weekday};
use uuid::Uuid;

/// Below this many viewings there is nothing to be confident about, and the UI
/// hides the dots rather than rendering a fabricated one.
const MIN_HISTORY: i32 = 3;

/// Where the volume term saturates: eight viewings of a series is already "you
/// always watch this", and more should not out-rank a better completion rate.
const VOLUME_SATURATION: f64 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grain {
    Series,
    TimeOfDay,
    Channel,
    Genre,
}

impl Grain {
    pub fn as_str(self) -> &'static str {
        match self {
            Grain::Series => "series",
            Grain::TimeOfDay => "timeofday",
            Grain::Channel => "channel",
            Grain::Genre => "genre",
        }
    }
}

#[derive(Debug, Clone)]
pub struct WatchFact {
    pub user_id: Uuid,
    pub channel_id: Uuid,
    pub series_id: Option<String>,
    pub genres: Vec<String>,
    pub started_at: OffsetDateTime,
    pub watched_secs: i32,
    pub programme_secs: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GrainKey {
    pub series_id: Option<String>,
    pub channel_id: Option<Uuid>,
    pub genre: Option<String>,
    pub weekday: Option<i16>,
    pub hour: Option<i16>,
}

impl GrainKey {
    pub fn series(series_id: String) -> Self {
        Self {
            series_id: Some(series_id),
            channel_id: None,
            genre: None,
            weekday: None,
            hour: None,
        }
    }

    pub fn time_of_day(weekday: i16, hour: i16) -> Self {
        Self {
            series_id: None,
            channel_id: None,
            genre: None,
            weekday: Some(weekday),
            hour: Some(hour),
        }
    }

    pub fn channel(channel_id: Uuid) -> Self {
        Self {
            series_id: None,
            channel_id: Some(channel_id),
            genre: None,
            weekday: None,
            hour: None,
        }
    }

    pub fn genre(genre: String) -> Self {
        Self {
            series_id: None,
            channel_id: None,
            genre: Some(genre),
            weekday: None,
            hour: None,
        }
    }

    pub fn grain(&self) -> Grain {
        if self.series_id.is_some() {
            Grain::Series
        } else if self.genre.is_some() {
            Grain::Genre
        } else if self.channel_id.is_some() {
            Grain::Channel
        } else {
            Grain::TimeOfDay
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GrainStats {
    pub watch_count: i32,
    pub total_secs: i64,
    pub mean_completion: f64,
    pub last_watched_at: OffsetDateTime,
}

impl GrainStats {
    /// A stored `mean_completion` of exactly zero is "nobody told us the
    /// programme length", not "watched none of it": a viewing with no
    /// `programme_secs` never contributes a ratio, and any viewing that does
    /// contribute has `watched_secs > 0` and so a non-zero one.
    pub fn completion_known(&self) -> bool {
        self.mean_completion > 0.0
    }
}

#[derive(Debug, Default)]
struct Accumulator {
    watch_count: i32,
    total_secs: i64,
    completion_sum: f64,
    completion_n: u32,
    last_watched_at: Option<OffsetDateTime>,
}

impl Accumulator {
    fn push(&mut self, fact: &WatchFact) {
        self.watch_count += 1;
        self.total_secs += i64::from(fact.watched_secs.max(0));
        // An unknown denominator is skipped rather than treated as a complete
        // viewing: guessing 1.0 would make every un-guided tune-in outrank a
        // series the user genuinely finishes.
        if let Some(completion) = completion_of(fact) {
            self.completion_sum += completion;
            self.completion_n += 1;
        }
        self.last_watched_at = Some(match self.last_watched_at {
            Some(prev) if prev >= fact.started_at => prev,
            _ => fact.started_at,
        });
    }

    fn finish(self) -> Option<GrainStats> {
        Some(GrainStats {
            watch_count: self.watch_count,
            total_secs: self.total_secs,
            mean_completion: if self.completion_n > 0 {
                self.completion_sum / f64::from(self.completion_n)
            } else {
                0.0
            },
            last_watched_at: self.last_watched_at?,
        })
    }
}

fn completion_of(fact: &WatchFact) -> Option<f64> {
    let programme = fact.programme_secs?;
    if programme <= 0 {
        return None;
    }
    Some((f64::from(fact.watched_secs.max(0)) / f64::from(programme)).clamp(0.0, 1.0))
}

pub fn weekday_index(d: Weekday) -> i16 {
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

/// The local (weekday, hour) slot a viewing falls in.
fn slot_of(started_at: OffsetDateTime, offset: UtcOffset) -> (i16, i16) {
    let local = started_at.to_offset(offset);
    (weekday_index(local.weekday()), i16::from(local.hour()))
}

fn keys_for(fact: &WatchFact, offset: UtcOffset) -> Vec<GrainKey> {
    let mut keys = Vec::with_capacity(3 + fact.genres.len());
    if let Some(series_id) = fact.series_id.as_ref().filter(|s| !s.trim().is_empty()) {
        keys.push(GrainKey::series(series_id.clone()));
    }
    let (weekday, hour) = slot_of(fact.started_at, offset);
    keys.push(GrainKey::time_of_day(weekday, hour));
    keys.push(GrainKey::channel(fact.channel_id));
    for genre in &fact.genres {
        if !genre.trim().is_empty() {
            keys.push(GrainKey::genre(genre.clone()));
        }
    }
    keys
}

/// `offset_at` is asked per fact rather than once: the rollup window spans a DST
/// transition, and a single fixed offset smears "the 7pm news" across two hour
/// slots either side of it.
pub fn roll_up(
    facts: &[WatchFact],
    offset_at: impl Fn(OffsetDateTime) -> UtcOffset,
) -> Vec<(Uuid, GrainKey, GrainStats)> {
    let mut acc: HashMap<(Uuid, GrainKey), Accumulator> = HashMap::new();
    for fact in facts {
        for key in keys_for(fact, offset_at(fact.started_at)) {
            acc.entry((fact.user_id, key)).or_default().push(fact);
        }
    }
    let mut out: Vec<(Uuid, GrainKey, GrainStats)> = acc
        .into_iter()
        .filter_map(|((user_id, key), a)| a.finish().map(|s| (user_id, key, s)))
        .collect();
    out.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    out
}

/// Fraction of viewings landing in the single most common slot.
pub fn slot_regularity(slots: &[(i16, i16)]) -> f64 {
    if slots.is_empty() {
        return 0.0;
    }
    let mut counts: HashMap<(i16, i16), usize> = HashMap::new();
    for slot in slots {
        *counts.entry(*slot).or_default() += 1;
    }
    let modal = counts.values().copied().max().unwrap_or(0);
    modal as f64 / slots.len() as f64
}

/// 0..1, or `None` when there is not enough history to say anything.
///
/// Completion is a *multiplier*, not another additive term: abandonment has to
/// be able to drag a heavily-watched series below a lightly-watched one that
/// gets finished, which an additive blend can never do once the volume term
/// saturates. Unknown completion sits between the two rather than being
/// silently rewarded.
pub fn confidence(
    watch_count: i32,
    mean_completion: f64,
    slot_regularity: f64,
    completion_known: bool,
) -> Option<f64> {
    if watch_count < MIN_HISTORY {
        return None;
    }
    let volume = (f64::from(watch_count) / VOLUME_SATURATION).min(1.0);
    let base = 0.65 * volume + 0.35 * slot_regularity.clamp(0.0, 1.0);
    let multiplier = if completion_known {
        0.25 + 0.75 * mean_completion.clamp(0.0, 1.0)
    } else {
        0.6
    };
    Some((base * multiplier).clamp(0.0, 1.0))
}

pub fn clock_minutes_apart(a: time::Time, b: time::Time) -> u16 {
    let minutes = |t: time::Time| u16::from(t.hour()) * 60 + u16::from(t.minute());
    let diff = minutes(a).abs_diff(minutes(b));
    diff.min(24 * 60 - diff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::{Date, Month};

    fn at(year: i32, month: Month, day: u8, hour: u8) -> OffsetDateTime {
        Date::from_calendar_date(year, month, day)
            .unwrap()
            .with_hms(hour, 0, 0)
            .unwrap()
            .assume_utc()
    }

    fn fact(
        user: Uuid,
        channel: Uuid,
        series: Option<&str>,
        genres: &[&str],
        started_at: OffsetDateTime,
        watched: i32,
        programme: Option<i32>,
    ) -> WatchFact {
        WatchFact {
            user_id: user,
            channel_id: channel,
            series_id: series.map(str::to_string),
            genres: genres.iter().map(|g| g.to_string()).collect(),
            started_at,
            watched_secs: watched,
            programme_secs: programme,
        }
    }

    #[test]
    fn abandonment_ranks_below_completion_at_equal_counts() {
        let finished = confidence(6, 0.95, 1.0, true).unwrap();
        let bailed = confidence(6, 0.10, 1.0, true).unwrap();
        assert!(
            bailed < finished,
            "bailing must rank below finishing: {bailed} vs {finished}"
        );
    }

    #[test]
    fn abandonment_can_sink_below_a_smaller_but_completed_history() {
        let three_finished = confidence(3, 1.0, 1.0, true).unwrap();
        let many_bailed = confidence(20, 0.10, 1.0, true).unwrap();
        assert!(
            many_bailed < three_finished,
            "abandonment is a negative signal, not a neutral one: {many_bailed} vs {three_finished}"
        );
    }

    #[test]
    fn there_is_no_confidence_below_the_history_floor() {
        for count in 0..MIN_HISTORY {
            assert_eq!(confidence(count, 1.0, 1.0, true), None, "count {count}");
        }
        assert!(confidence(MIN_HISTORY, 1.0, 1.0, true).is_some());
    }

    #[test]
    fn a_regular_slot_outranks_a_scattered_one() {
        let regular = confidence(6, 0.9, 1.0, true).unwrap();
        let scattered = confidence(6, 0.9, 0.2, true).unwrap();
        assert!(scattered < regular, "{scattered} vs {regular}");
    }

    #[test]
    fn unknown_completion_sits_between_finished_and_abandoned() {
        let finished = confidence(6, 0.95, 1.0, true).unwrap();
        let unknown = confidence(6, 0.0, 1.0, false).unwrap();
        let bailed = confidence(6, 0.10, 1.0, true).unwrap();
        assert!(
            bailed < unknown && unknown < finished,
            "{bailed} {unknown} {finished}"
        );
    }

    #[test]
    fn clock_minutes_apart_wraps_midnight() {
        let t = |h, m| time::Time::from_hms(h, m, 0).unwrap();
        assert_eq!(clock_minutes_apart(t(19, 0), t(20, 30)), 90);
        assert_eq!(clock_minutes_apart(t(23, 30), t(0, 45)), 75);
        assert_eq!(clock_minutes_apart(t(0, 45), t(23, 30)), 75);
        assert_eq!(clock_minutes_apart(t(7, 0), t(19, 0)), 720);
    }

    #[test]
    fn confidence_stays_in_range() {
        for count in [3, 8, 400] {
            for completion in [0.0, 0.5, 1.0, 4.0, -1.0] {
                for regularity in [0.0, 0.5, 1.0, 3.0, -2.0] {
                    let c = confidence(count, completion, regularity, true).unwrap();
                    assert!(
                        (0.0..=1.0).contains(&c),
                        "{c} for {count}/{completion}/{regularity}"
                    );
                }
            }
        }
    }

    #[test]
    fn slot_regularity_is_the_modal_share() {
        assert_eq!(slot_regularity(&[]), 0.0);
        assert_eq!(slot_regularity(&[(6, 12), (6, 12), (6, 12)]), 1.0);
        assert_eq!(slot_regularity(&[(6, 12), (6, 12), (1, 20), (3, 9)]), 0.5);
    }

    #[test]
    fn a_fact_with_no_programme_length_does_not_fabricate_completion() {
        let user = Uuid::now_v7();
        let channel = Uuid::now_v7();
        let rows = roll_up(
            &[fact(
                user,
                channel,
                Some("S1"),
                &[],
                at(2026, Month::August, 29, 3),
                1800,
                None,
            )],
            |_| UtcOffset::UTC,
        );
        let series = rows
            .iter()
            .find(|(_, k, _)| k.grain() == Grain::Series)
            .expect("a series grain");
        assert_eq!(series.2.mean_completion, 0.0);
        assert!(!series.2.completion_known());
        assert_eq!(series.2.watch_count, 1);
        assert_eq!(series.2.total_secs, 1800);
    }

    #[test]
    fn completion_averages_only_the_facts_that_know_their_length() {
        let user = Uuid::now_v7();
        let channel = Uuid::now_v7();
        let rows = roll_up(
            &[
                fact(
                    user,
                    channel,
                    Some("S1"),
                    &[],
                    at(2026, Month::August, 29, 3),
                    1800,
                    Some(1800),
                ),
                fact(
                    user,
                    channel,
                    Some("S1"),
                    &[],
                    at(2026, Month::August, 30, 3),
                    600,
                    Some(1800),
                ),
                fact(
                    user,
                    channel,
                    Some("S1"),
                    &[],
                    at(2026, Month::August, 31, 3),
                    900,
                    None,
                ),
            ],
            |_| UtcOffset::UTC,
        );
        let series = rows
            .iter()
            .find(|(_, k, _)| k.grain() == Grain::Series)
            .expect("a series grain");
        assert_eq!(series.2.watch_count, 3);
        assert!(
            (series.2.mean_completion - (1.0 + 1.0 / 3.0) / 2.0).abs() < 1e-9,
            "{}",
            series.2.mean_completion
        );
        assert_eq!(series.2.last_watched_at, at(2026, Month::August, 31, 3));
    }

    #[test]
    fn overwatching_a_programme_is_capped_at_one() {
        let user = Uuid::now_v7();
        let channel = Uuid::now_v7();
        let rows = roll_up(
            &[fact(
                user,
                channel,
                Some("S1"),
                &[],
                at(2026, Month::August, 29, 3),
                9000,
                Some(1800),
            )],
            |_| UtcOffset::UTC,
        );
        let series = rows
            .iter()
            .find(|(_, k, _)| k.grain() == Grain::Series)
            .expect("a series grain");
        assert_eq!(series.2.mean_completion, 1.0);
    }

    #[test]
    fn every_grain_is_emitted_and_genres_fan_out() {
        let user = Uuid::now_v7();
        let channel = Uuid::now_v7();
        let rows = roll_up(
            &[fact(
                user,
                channel,
                Some("S1"),
                &["Drama", "Comedy"],
                at(2026, Month::August, 29, 3),
                1800,
                Some(1800),
            )],
            |_| UtcOffset::UTC,
        );
        assert_eq!(
            rows.len(),
            5,
            "series + slot + channel + two genres: {rows:?}"
        );
        let genres: Vec<&str> = rows
            .iter()
            .filter_map(|(_, k, _)| k.genre.as_deref())
            .collect();
        assert_eq!(genres, vec!["Comedy", "Drama"]);
        assert_eq!(
            rows.iter()
                .filter(|(_, k, _)| k.grain() == Grain::Channel)
                .count(),
            1
        );
        assert_eq!(
            rows.iter()
                .filter(|(_, k, _)| k.grain() == Grain::TimeOfDay)
                .count(),
            1
        );
    }

    #[test]
    fn a_fact_with_no_series_still_produces_the_other_grains() {
        let user = Uuid::now_v7();
        let channel = Uuid::now_v7();
        let rows = roll_up(
            &[fact(
                user,
                channel,
                None,
                &[],
                at(2026, Month::August, 29, 3),
                600,
                Some(1800),
            )],
            |_| UtcOffset::UTC,
        );
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|(_, k, _)| k.grain() != Grain::Series));
    }

    #[test]
    fn blank_series_and_genre_strings_are_not_grains() {
        let user = Uuid::now_v7();
        let channel = Uuid::now_v7();
        let rows = roll_up(
            &[fact(
                user,
                channel,
                Some("  "),
                &["", "  "],
                at(2026, Month::August, 29, 3),
                600,
                Some(1800),
            )],
            |_| UtcOffset::UTC,
        );
        assert_eq!(rows.len(), 2, "{rows:?}");
    }

    #[test]
    fn the_slot_is_bucketed_in_local_time_not_utc() {
        let user = Uuid::now_v7();
        let channel = Uuid::now_v7();
        // Saturday 23:00 UTC is Sunday 09:00 in Melbourne, so both the weekday
        // and the hour have to move with the offset.
        let started = at(2026, Month::August, 29, 23);
        assert_eq!(weekday_index(started.weekday()), 6);

        let utc = roll_up(
            &[fact(user, channel, None, &[], started, 600, Some(1800))],
            |_| UtcOffset::UTC,
        );
        let slot_utc = utc
            .iter()
            .find(|(_, k, _)| k.grain() == Grain::TimeOfDay)
            .expect("a slot grain");
        assert_eq!((slot_utc.1.weekday, slot_utc.1.hour), (Some(6), Some(23)));

        let melbourne = roll_up(
            &[fact(user, channel, None, &[], started, 600, Some(1800))],
            |_| UtcOffset::from_hms(10, 0, 0).unwrap(),
        );
        let slot_local = melbourne
            .iter()
            .find(|(_, k, _)| k.grain() == Grain::TimeOfDay)
            .expect("a slot grain");
        assert_eq!(
            (slot_local.1.weekday, slot_local.1.hour),
            (Some(0), Some(9))
        );
    }

    #[test]
    fn a_dst_transition_does_not_smear_a_habit_across_two_hour_slots() {
        let user = Uuid::now_v7();
        let channel = Uuid::now_v7();
        // Both are 19:00 on a Thursday in Melbourne; only the UTC offset differs
        // (AEDT +11 in January, AEST +10 in July).
        let summer = at(2026, Month::January, 15, 8);
        let winter = at(2026, Month::July, 16, 9);
        let zone = crate::notifications::prefs::Zone::parse("Australia/Melbourne").unwrap();

        let rows = roll_up(
            &[
                fact(user, channel, None, &[], summer, 600, Some(1800)),
                fact(user, channel, None, &[], winter, 600, Some(1800)),
            ],
            |t| zone.offset_at(t),
        );
        let slots: Vec<_> = rows
            .iter()
            .filter(|(_, k, _)| k.grain() == Grain::TimeOfDay)
            .collect();
        assert_eq!(slots.len(), 1, "one habit, one slot: {slots:?}");
        assert_eq!((slots[0].1.weekday, slots[0].1.hour), (Some(4), Some(19)));
        assert_eq!(slots[0].2.watch_count, 2);
    }

    #[test]
    fn users_are_never_pooled() {
        let a = Uuid::now_v7();
        let b = Uuid::now_v7();
        let channel = Uuid::now_v7();
        let rows = roll_up(
            &[
                fact(
                    a,
                    channel,
                    Some("S1"),
                    &[],
                    at(2026, Month::August, 29, 3),
                    1800,
                    Some(1800),
                ),
                fact(
                    b,
                    channel,
                    Some("S1"),
                    &[],
                    at(2026, Month::August, 29, 3),
                    1800,
                    Some(1800),
                ),
            ],
            |_| UtcOffset::UTC,
        );
        let series: Vec<_> = rows
            .iter()
            .filter(|(_, k, _)| k.grain() == Grain::Series)
            .collect();
        assert_eq!(series.len(), 2);
        assert!(series.iter().all(|(_, _, s)| s.watch_count == 1));
    }
}
