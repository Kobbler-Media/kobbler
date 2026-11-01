//! When a position save counts as having finished the title.

use sqlx::PgPool;
use uuid::Uuid;

const FINISHED_FRACTION: f64 = 0.9;
/// Credits detected earlier than this are a misdetection, not an ending; trusting
/// one would mark a title watched halfway through.
const MIN_CREDITS_FRACTION: f64 = 0.5;

#[derive(Debug, Clone, Copy)]
pub struct PlaybackExtent {
    pub duration_seconds: f64,
    finished_from: f64,
}

impl PlaybackExtent {
    fn new(duration_seconds: f64, credits_start: Option<f64>) -> Option<Self> {
        if duration_seconds <= 0.0 {
            return None;
        }
        let finished_from = credits_start
            .filter(|&c| c >= duration_seconds * MIN_CREDITS_FRACTION)
            .map_or(duration_seconds * FINISHED_FRACTION, |c| {
                c.min(duration_seconds * FINISHED_FRACTION)
            });
        Some(Self {
            duration_seconds,
            finished_from,
        })
    }

    pub fn is_finished_at(&self, position_seconds: i32) -> bool {
        f64::from(position_seconds) >= self.finished_from
    }
}

pub async fn for_episode(pool: &PgPool, episode_id: Uuid) -> Option<PlaybackExtent> {
    let row = sqlx::query!(
        r#"
        SELECT f.duration_seconds AS "duration!",
               (SELECT MIN(s.start_seconds) FROM episode_skip_segments s
                WHERE s.episode_file_id = f.id AND s.segment_type = 'credits'
                  AND NOT (s.start_seconds = 0 AND s.end_seconds = 0)) AS credits_start
        FROM tv_episode_files f
        JOIN tv_episode_file_episodes fe ON fe.episode_file_id = f.id
        WHERE fe.episode_id = $1 AND f.available = true AND f.duration_seconds IS NOT NULL
        ORDER BY f.duration_seconds DESC
        LIMIT 1
        "#,
        episode_id,
    )
    .fetch_optional(pool)
    .await
    .ok()??;
    PlaybackExtent::new(row.duration, row.credits_start)
}

pub async fn for_movie(pool: &PgPool, movie_id: Uuid) -> Option<PlaybackExtent> {
    let row = sqlx::query!(
        r#"
        SELECT f.duration_seconds AS "duration!",
               (SELECT MIN(s.start_seconds) FROM movie_skip_segments s
                WHERE s.movie_file_id = f.id AND s.segment_type = 'credits'
                  AND NOT (s.start_seconds = 0 AND s.end_seconds = 0)) AS credits_start
        FROM movie_files f
        WHERE f.movie_id = $1 AND f.available = true AND f.duration_seconds IS NOT NULL
        ORDER BY f.duration_seconds DESC
        LIMIT 1
        "#,
        movie_id,
    )
    .fetch_optional(pool)
    .await
    .ok()??;
    PlaybackExtent::new(row.duration, row.credits_start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_from_takes_the_earlier_of_credits_and_ninety_percent() {
        let cases = [
            (None, 90, true),
            (None, 89, false),
            (Some(80.0), 80, true),
            (Some(80.0), 79, false),
            (Some(95.0), 90, true),
            (Some(40.0), 41, false),
        ];
        for (credits, pos, finished) in cases {
            let extent = PlaybackExtent::new(100.0, credits).unwrap();
            assert_eq!(
                extent.is_finished_at(pos),
                finished,
                "credits {credits:?} at {pos}"
            );
        }
        assert!(PlaybackExtent::new(0.0, None).is_none());
    }
}
