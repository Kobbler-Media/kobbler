//! One season of one show, counted in postgres.
//!
//! The seasons projection carries `episode_count` — the total — and no
//! availability at all, so the nearest number in reach was the whole show's
//! `available_episode_count`, and the episodes index truncates, so a tally by
//! eye off a fuzzy-matched list was the only other option. Both were wrong live.
//!
//! The part no index can answer at all: an episode nobody has and an episode
//! nobody has *made yet* look identical everywhere except `tv_episodes.air_date`.
//! Only aired-and-unheld is a gap, so the split is counted here rather than left
//! to the model.
//!
//! Held is the file test `chat_credits` uses — `tv_episode_file_episodes` joined
//! to an available `tv_episode_files`. Never a cached flag: `is_in_library` was
//! dropped from `tv_episodes` in 0124 because it lied.

use sqlx::PgPool;
use time::Date;
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, sqlx::Error>;

#[derive(Debug, Clone)]
pub struct Season {
    pub id: Uuid,
    pub season_identifier: String,
    pub season_number: i32,
    pub title: Option<String>,
    pub is_special: bool,
    pub metadata_episode_count: Option<i32>,
}

pub async fn season_by_number(pool: &PgPool, show_id: Uuid, number: i32) -> Result<Option<Season>> {
    let row = sqlx::query!(
        r#"
        SELECT ts.id, ts.season_identifier, ts.season_sort_order, ts.title,
               ts.is_special, ts.episode_count
        FROM tv_seasons ts
        WHERE ts.show_id = $1 AND ts.season_sort_order = $2
        ORDER BY ts.season_identifier ASC, ts.id ASC
        LIMIT 1
        "#,
        show_id,
        number,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| Season {
        id: r.id,
        season_identifier: r.season_identifier,
        season_number: r.season_sort_order,
        title: r.title,
        is_special: r.is_special,
        metadata_episode_count: r.episode_count,
    }))
}

/// The season "the current one" means: the highest-numbered ordinary season
/// with at least one episode already aired.
///
/// Highest-numbered alone picks a season TMDB has announced and nobody has
/// seen, which is the wrong answer to "how many episodes does the current
/// season have" — that season has none yet. Where nothing has aired at all the
/// show has not started, and the *lowest* season is the one about to, so the
/// ordering flips.
///
/// Specials never win: season 0 is not a season anyone means by "current", and
/// its `season_sort_order` of 0 would otherwise sort it above nothing at all.
pub async fn current_season(pool: &PgPool, show_id: Uuid) -> Result<Option<Season>> {
    let row = sqlx::query!(
        r#"
        SELECT q.id AS "id!", q.season_identifier AS "season_identifier!",
               q.season_sort_order AS "season_sort_order!", q.title,
               q.is_special AS "is_special!", q.episode_count
        FROM (
            SELECT ts.id, ts.season_identifier, ts.season_sort_order, ts.title,
                   ts.is_special, ts.episode_count,
                   EXISTS (
                       SELECT 1 FROM tv_episodes te
                       WHERE te.season_id = ts.id
                         AND te.air_date IS NOT NULL
                         AND te.air_date <= CURRENT_DATE
                   ) AS has_aired
            FROM tv_seasons ts
            WHERE ts.show_id = $1 AND ts.is_special = FALSE AND ts.season_sort_order > 0
        ) q
        ORDER BY q.has_aired DESC,
                 CASE WHEN q.has_aired THEN -q.season_sort_order ELSE q.season_sort_order END ASC,
                 q.id ASC
        LIMIT 1
        "#,
        show_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| Season {
        id: r.id,
        season_identifier: r.season_identifier,
        season_number: r.season_sort_order,
        title: r.title,
        is_special: r.is_special,
        metadata_episode_count: r.episode_count,
    }))
}

/// Every season the show has, specials included — the candidate list for a
/// season number that matched nothing. Sports number their seasons by year, so
/// "season 3" of a competition legitimately does not exist and the honest reply
/// is the years there are.
pub async fn seasons_of(pool: &PgPool, show_id: Uuid) -> Result<Vec<Season>> {
    let rows = sqlx::query!(
        r#"
        SELECT ts.id, ts.season_identifier, ts.season_sort_order, ts.title,
               ts.is_special, ts.episode_count
        FROM tv_seasons ts
        WHERE ts.show_id = $1
        ORDER BY ts.season_sort_order ASC, ts.id ASC
        "#,
        show_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Season {
            id: r.id,
            season_identifier: r.season_identifier,
            season_number: r.season_sort_order,
            title: r.title,
            is_special: r.is_special,
            metadata_episode_count: r.episode_count,
        })
        .collect())
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SeasonCounts {
    pub episodes: i64,
    pub held: i64,
    pub aired: i64,
    pub to_come: i64,
    pub undated: i64,
    pub held_aired: i64,
    pub held_unaired: i64,
}

impl SeasonCounts {
    pub fn missing_aired(self) -> i64 {
        self.aired - self.held_aired
    }
}

/// Counted in SQL rather than off a fetched list, so the numbers can never be a
/// truncated page's length — that is the failure this whole tool replaces.
pub async fn season_counts(pool: &PgPool, season_id: Uuid) -> Result<SeasonCounts> {
    let row = sqlx::query!(
        r#"
        SELECT
            COUNT(*) AS "episodes!",
            COUNT(*) FILTER (WHERE q.held) AS "held!",
            COUNT(*) FILTER (WHERE q.aired) AS "aired!",
            COUNT(*) FILTER (WHERE q.air_date > CURRENT_DATE) AS "to_come!",
            COUNT(*) FILTER (WHERE q.air_date IS NULL) AS "undated!",
            COUNT(*) FILTER (WHERE q.held AND q.aired) AS "held_aired!",
            COUNT(*) FILTER (WHERE q.held AND NOT q.aired) AS "held_unaired!"
        FROM (
            SELECT te.air_date,
                   (te.air_date IS NOT NULL AND te.air_date <= CURRENT_DATE) AS aired,
                   EXISTS (
                       SELECT 1 FROM tv_episode_file_episodes tefe
                       JOIN tv_episode_files tef
                         ON tef.id = tefe.episode_file_id AND tef.available
                       WHERE tefe.episode_id = te.id
                   ) AS held
            FROM tv_episodes te
            WHERE te.season_id = $1
        ) q
        "#,
        season_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(SeasonCounts {
        episodes: row.episodes,
        held: row.held,
        aired: row.aired,
        to_come: row.to_come,
        undated: row.undated,
        held_aired: row.held_aired,
        held_unaired: row.held_unaired,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    MissingAired,
    ToCome,
    NoAirDate,
}

#[derive(Debug, Clone)]
pub struct SeasonEpisode {
    pub episode_number: i32,
    pub title: Option<String>,
    pub air_date: Option<Date>,
    pub held: bool,
    pub standing: Standing,
}

/// The episodes worth naming: the aired ones with no file, the ones still to
/// come, and the ones with no air date at all. Everything else is already had
/// and needs no mention.
pub async fn season_episode_standings(
    pool: &PgPool,
    season_id: Uuid,
    limit: i64,
) -> Result<Vec<SeasonEpisode>> {
    let rows = sqlx::query!(
        r#"
        SELECT q.episode_sort_order AS "episode_sort_order!",
               q.title, q.air_date, q.held AS "held!", q.aired AS "aired!"
        FROM (
            SELECT te.episode_sort_order, te.title, te.air_date,
                   (te.air_date IS NOT NULL AND te.air_date <= CURRENT_DATE) AS aired,
                   EXISTS (
                       SELECT 1 FROM tv_episode_file_episodes tefe
                       JOIN tv_episode_files tef
                         ON tef.id = tefe.episode_file_id AND tef.available
                       WHERE tefe.episode_id = te.id
                   ) AS held,
                   te.id
            FROM tv_episodes te
            WHERE te.season_id = $1
        ) q
        WHERE q.air_date IS NULL
           OR q.air_date > CURRENT_DATE
           OR NOT q.held
        ORDER BY q.episode_sort_order ASC, q.id ASC
        LIMIT $2
        "#,
        season_id,
        limit,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let standing = match (r.aired, r.air_date.is_some()) {
                (true, _) => Standing::MissingAired,
                (false, true) => Standing::ToCome,
                (false, false) => Standing::NoAirDate,
            };
            SeasonEpisode {
                episode_number: r.episode_sort_order,
                title: r.title,
                air_date: r.air_date,
                held: r.held,
                standing,
            }
        })
        .collect())
}
