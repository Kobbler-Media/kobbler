//! `keep_count` / `keep_until` enforcement for series-rule recordings.
//!
//! `keep_until` decides *whether* a recording past the keep window may go:
//! `deleted` never expires automatically, `watched` expires once the household
//! has watched it, `date` expires on a calendar date, and `space` — the default
//! — only expires while the recording volume is actually short of room.

use std::collections::HashMap;
use std::path::Path;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::repositories::livetv_recordings::RetentionCandidate;

/// Below this the `space` policy starts reclaiming. Above it, a recording the
/// owner has not watched is worth more than the free space it occupies.
pub const SPACE_FLOOR_RATIO: f64 = 0.10;

#[derive(Debug, Default, Clone, Copy)]
pub struct DiskState {
    pub free_ratio: f64,
}

impl DiskState {
    pub fn short_of_room(&self) -> bool {
        self.free_ratio < SPACE_FLOOR_RATIO
    }
}

pub fn disk_state(path: &Path) -> Option<DiskState> {
    let stat = nix::sys::statvfs::statvfs(path).ok()?;
    let total = stat.blocks() as f64;
    if total <= 0.0 {
        return None;
    }
    Some(DiskState {
        free_ratio: stat.blocks_available() as f64 / total,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expired {
    pub recording_id: Uuid,
    pub reason: &'static str,
}

/// Pure: `candidates` must already be grouped by rule and ordered newest first,
/// which is what `livetv_recordings::retention_candidates` returns.
pub fn expired(
    candidates: &[RetentionCandidate],
    watched: &[Uuid],
    now: OffsetDateTime,
    disk: DiskState,
) -> Vec<Expired> {
    let watched: std::collections::HashSet<Uuid> = watched.iter().copied().collect();
    let mut index_in_rule: HashMap<Uuid, usize> = HashMap::new();
    let mut out = Vec::new();

    for c in candidates {
        let idx = index_in_rule.entry(c.series_rule_id).or_insert(0);
        let position = *idx;
        *idx += 1;

        if c.keep_until == "deleted" {
            continue;
        }

        let over_count = c
            .keep_count
            .is_some_and(|k| k > 0 && position >= k as usize);

        let reason = match c.keep_until.as_str() {
            "watched" if watched.contains(&c.id) => Some("watched"),
            "date" if c.keep_until_date.is_some_and(|d| now >= d) => Some("past its keep date"),
            _ => None,
        };
        let reason = match reason {
            Some(r) => Some(r),
            None if over_count && (c.keep_until != "space" || disk.short_of_room()) => {
                Some("beyond the rule's keep count")
            }
            None => None,
        };

        if let Some(reason) = reason {
            out.push(Expired {
                recording_id: c.id,
                reason,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    fn candidate(
        rule: Uuid,
        keep_count: Option<i32>,
        keep_until: &str,
        keep_until_date: Option<OffsetDateTime>,
    ) -> RetentionCandidate {
        RetentionCandidate {
            id: Uuid::now_v7(),
            series_rule_id: rule,
            output_dir: None,
            episode_id: None,
            guide_id: None,
            actual_end: None,
            scheduled_end: OffsetDateTime::now_utc(),
            size_bytes: None,
            keep_count,
            keep_until: keep_until.to_string(),
            keep_until_date,
        }
    }

    fn full_disk() -> DiskState {
        DiskState { free_ratio: 0.01 }
    }
    fn roomy_disk() -> DiskState {
        DiskState { free_ratio: 0.80 }
    }

    #[test]
    fn keep_count_drops_everything_past_the_newest_n() {
        let rule = Uuid::now_v7();
        let rows: Vec<_> = (0..5)
            .map(|_| candidate(rule, Some(2), "watched", None))
            .collect();
        let now = OffsetDateTime::now_utc();
        let gone = expired(&rows, &[], now, roomy_disk());
        assert_eq!(gone.len(), 3);
        assert_eq!(gone[0].recording_id, rows[2].id);
    }

    #[test]
    fn keep_until_deleted_never_expires_anything() {
        let rule = Uuid::now_v7();
        let rows: Vec<_> = (0..5)
            .map(|_| candidate(rule, Some(1), "deleted", None))
            .collect();
        let watched: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
        assert!(expired(&rows, &watched, OffsetDateTime::now_utc(), full_disk()).is_empty());
    }

    #[test]
    fn keep_until_watched_expires_only_what_was_watched() {
        let rule = Uuid::now_v7();
        let rows: Vec<_> = (0..3)
            .map(|_| candidate(rule, None, "watched", None))
            .collect();
        let gone = expired(
            &rows,
            &[rows[1].id],
            OffsetDateTime::now_utc(),
            roomy_disk(),
        );
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].recording_id, rows[1].id);
        assert_eq!(gone[0].reason, "watched");
    }

    #[test]
    fn keep_until_date_expires_on_the_date_and_not_before() {
        let rule = Uuid::now_v7();
        let now = OffsetDateTime::now_utc();
        let rows = vec![
            candidate(rule, None, "date", Some(now - Duration::days(1))),
            candidate(rule, None, "date", Some(now + Duration::days(1))),
        ];
        let gone = expired(&rows, &[], now, roomy_disk());
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].recording_id, rows[0].id);
    }

    #[test]
    fn keep_until_space_waits_for_the_disk_to_actually_fill() {
        let rule = Uuid::now_v7();
        let rows: Vec<_> = (0..4)
            .map(|_| candidate(rule, Some(1), "space", None))
            .collect();
        let now = OffsetDateTime::now_utc();
        assert!(expired(&rows, &[], now, roomy_disk()).is_empty());
        assert_eq!(expired(&rows, &[], now, full_disk()).len(), 3);
    }

    #[test]
    fn keep_counts_are_per_rule() {
        let a = Uuid::now_v7();
        let b = Uuid::now_v7();
        let rows = vec![
            candidate(a, Some(1), "watched", None),
            candidate(a, Some(1), "watched", None),
            candidate(b, Some(1), "watched", None),
            candidate(b, Some(1), "watched", None),
        ];
        let gone = expired(&rows, &[], OffsetDateTime::now_utc(), roomy_disk());
        assert_eq!(gone.len(), 2);
        assert_eq!(gone[0].recording_id, rows[1].id);
        assert_eq!(gone[1].recording_id, rows[3].id);
    }
}
