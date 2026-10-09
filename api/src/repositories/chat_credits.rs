//! Who is in what, for the chat bot.
//!
//! This is a join, not a search. The Meili projections carry counts for people
//! and directors for movies, so the model could find Ashton Kutcher and could
//! find a film, but never the edge between them — that edge only exists in
//! postgres.
//!
//! Two rules the queries encode rather than leave to the caller:
//!
//! * `person_dismissed_credits` is keyed on the *provider* id, so the join is
//!   `tmdb_id::text = identifiers->>'tmdb_id'` and never an internal id;
//! * held is a file test — `movie_files` / `tv_episode_files`, the same test the
//!   search indexer uses — never `movies.is_in_library`, which is a cached flag
//!   and a want-listed row carries a title with no bytes behind it.

use sqlx::PgPool;
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, sqlx::Error>;

#[derive(Debug, Clone)]
pub struct PersonMatch {
    pub id: Uuid,
    pub name: String,
    pub credits: i64,
}

pub async fn find_people(pool: &PgPool, name: &str, limit: i64) -> Result<Vec<PersonMatch>> {
    let name = name.trim();
    let pattern = format!("%{name}%");
    let rows = sqlx::query!(
        r#"
        SELECT q.id AS "id!", q.name AS "name!", q.credits AS "credits!"
        FROM (
            SELECT p.id, p.name,
                   (SELECT COUNT(*) FROM movie_cast mc WHERE mc.person_id = p.id)
                 + (SELECT COUNT(*) FROM movie_crew mcr WHERE mcr.person_id = p.id)
                 + (SELECT COUNT(*) FROM tv_cast tc WHERE tc.person_id = p.id)
                 + (SELECT COUNT(*) FROM tv_crew tcr WHERE tcr.person_id = p.id)
                 + (SELECT COUNT(*) FROM tv_episode_cast tec WHERE tec.person_id = p.id)
                   AS credits
            FROM people p
            WHERE p.name ILIKE $1 OR p.sort_name ILIKE $1
        ) q
        ORDER BY (LOWER(q.name) = LOWER($2)) DESC, q.credits DESC, q.name ASC, q.id ASC
        LIMIT $3
        "#,
        pattern,
        name,
        limit,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| PersonMatch {
            id: r.id,
            name: r.name,
            credits: r.credits,
        })
        .collect())
}

#[derive(Debug, Clone)]
pub struct MovieCredit {
    pub id: Uuid,
    pub title: String,
    pub year: Option<i32>,
    pub character: Option<String>,
    pub job: Option<String>,
    pub held: bool,
    pub want_status: String,
}

pub async fn movie_credits(pool: &PgPool, person_id: Uuid) -> Result<Vec<MovieCredit>> {
    let rows = sqlx::query!(
        r#"
        SELECT q.id AS "id!", q.title AS "title!", q.year, q.character, q.job,
               q.held AS "held!", q.want_status AS "want_status!"
        FROM (
            SELECT m.id, m.title, m.year, m.want_status,
                   (SELECT string_agg(DISTINCT mc.character_name, ', ')
                    FROM movie_cast mc
                    WHERE mc.movie_id = m.id AND mc.person_id = $1) AS character,
                   (SELECT string_agg(DISTINCT mcr.job, ', ')
                    FROM movie_crew mcr
                    WHERE mcr.movie_id = m.id AND mcr.person_id = $1) AS job,
                   EXISTS (SELECT 1 FROM movie_files mf
                           WHERE mf.movie_id = m.id AND mf.available) AS held
            FROM movies m
            WHERE (EXISTS (SELECT 1 FROM movie_cast mc
                           WHERE mc.movie_id = m.id AND mc.person_id = $1)
                OR EXISTS (SELECT 1 FROM movie_crew mcr
                           WHERE mcr.movie_id = m.id AND mcr.person_id = $1))
              AND NOT EXISTS (
                  SELECT 1 FROM person_dismissed_credits d
                  WHERE d.person_id = $1
                    AND d.media_type = 'movie'
                    AND d.tmdb_id::text = m.identifiers->>'tmdb_id')
        ) q
        ORDER BY q.held DESC, q.year DESC NULLS LAST, q.title ASC, q.id ASC
        "#,
        person_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| MovieCredit {
            id: r.id,
            title: r.title,
            year: r.year,
            character: r.character,
            job: r.job,
            held: r.held,
            want_status: r.want_status,
        })
        .collect())
}

#[derive(Debug, Clone)]
pub struct ShowCredit {
    pub id: Uuid,
    pub title: String,
    pub year: Option<i32>,
    pub content_category: String,
    pub character: Option<String>,
    pub job: Option<String>,
    pub guest_episodes: i32,
    pub episodes_held: i32,
}

impl ShowCredit {
    pub fn held(&self) -> bool {
        self.episodes_held > 0
    }
}

pub async fn show_credits(pool: &PgPool, person_id: Uuid) -> Result<Vec<ShowCredit>> {
    let rows = sqlx::query!(
        r#"
        SELECT q.id AS "id!", q.title AS "title!", q.year,
               q.content_category AS "content_category!",
               q.character, q.job,
               q.guest_episodes AS "guest_episodes!",
               q.episodes_held AS "episodes_held!"
        FROM (
            SELECT s.id, s.title, s.year, s.content_category,
                   (SELECT string_agg(DISTINCT tc.character_name, ', ')
                    FROM tv_cast tc
                    WHERE tc.show_id = s.id AND tc.person_id = $1) AS character,
                   (SELECT string_agg(DISTINCT tcr.job, ', ')
                    FROM tv_crew tcr
                    WHERE tcr.show_id = s.id AND tcr.person_id = $1) AS job,
                   COALESCE((SELECT COUNT(DISTINCT te.id)::int
                    FROM tv_episode_cast tec
                    JOIN tv_episodes te ON te.id = tec.episode_id
                    JOIN tv_seasons tsn ON tsn.id = te.season_id
                    WHERE tsn.show_id = s.id AND tec.person_id = $1
                      AND tec.credit_type = 'guest'), 0) AS guest_episodes,
                   COALESCE((SELECT COUNT(DISTINCT te.id)::int
                    FROM tv_episodes te
                    JOIN tv_seasons tsn ON tsn.id = te.season_id
                    JOIN tv_episode_file_episodes tefe ON tefe.episode_id = te.id
                    JOIN tv_episode_files tef ON tef.id = tefe.episode_file_id AND tef.available
                    WHERE tsn.show_id = s.id), 0) AS episodes_held
            FROM tv_shows s
            WHERE (EXISTS (SELECT 1 FROM tv_cast tc
                           WHERE tc.show_id = s.id AND tc.person_id = $1)
                OR EXISTS (SELECT 1 FROM tv_crew tcr
                           WHERE tcr.show_id = s.id AND tcr.person_id = $1)
                OR EXISTS (SELECT 1 FROM tv_episode_cast tec
                           JOIN tv_episodes te ON te.id = tec.episode_id
                           JOIN tv_seasons tsn ON tsn.id = te.season_id
                           WHERE tsn.show_id = s.id AND tec.person_id = $1))
              AND NOT EXISTS (
                  SELECT 1 FROM person_dismissed_credits d
                  WHERE d.person_id = $1
                    AND d.media_type = 'tv'
                    AND d.tmdb_id::text = s.identifiers->>'tmdb_id')
        ) q
        ORDER BY (q.episodes_held > 0) DESC, q.year DESC NULLS LAST, q.title ASC, q.id ASC
        "#,
        person_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| ShowCredit {
            id: r.id,
            title: r.title,
            year: r.year,
            content_category: r.content_category,
            character: r.character,
            job: r.job,
            guest_episodes: r.guest_episodes,
            episodes_held: r.episodes_held,
        })
        .collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleKind {
    Movie,
    Show,
}

impl TitleKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TitleKind::Movie => "movie",
            TitleKind::Show => "show",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TitleMatch {
    pub kind: TitleKind,
    pub id: Uuid,
    pub title: String,
    pub year: Option<i32>,
    pub held: bool,
}

/// Candidate titles for something the user said out loud, films and shows
/// together. Several on purpose: which one they meant is a question for the
/// model to ask, exactly as `find_shows` does for `next_airing`.
pub async fn find_titles(
    pool: &PgPool,
    title: &str,
    kind: Option<TitleKind>,
    limit: i64,
) -> Result<Vec<TitleMatch>> {
    let title = title.trim();
    let pattern = format!("%{title}%");
    let mut matches = Vec::new();

    if kind != Some(TitleKind::Show) {
        let rows = sqlx::query!(
            r#"
            SELECT q.id AS "id!", q.title AS "title!", q.year, q.held AS "held!"
            FROM (
                SELECT m.id, m.title, m.year,
                       EXISTS (SELECT 1 FROM movie_files mf
                               WHERE mf.movie_id = m.id AND mf.available) AS held
                FROM movies m
                WHERE m.title ILIKE $1 OR m.original_title ILIKE $1
            ) q
            ORDER BY (LOWER(q.title) = LOWER($2)) DESC, q.held DESC,
                     q.year DESC NULLS LAST, q.title ASC, q.id ASC
            LIMIT $3
            "#,
            pattern,
            title,
            limit,
        )
        .fetch_all(pool)
        .await?;
        matches.extend(rows.into_iter().map(|r| TitleMatch {
            kind: TitleKind::Movie,
            id: r.id,
            title: r.title,
            year: r.year,
            held: r.held,
        }));
    }

    if kind != Some(TitleKind::Movie) {
        let rows = sqlx::query!(
            r#"
            SELECT q.id AS "id!", q.title AS "title!", q.year, q.held AS "held!"
            FROM (
                SELECT s.id, s.title, s.year,
                       EXISTS (SELECT 1 FROM tv_episodes te
                               JOIN tv_seasons tsn ON tsn.id = te.season_id
                               JOIN tv_episode_file_episodes tefe ON tefe.episode_id = te.id
                               JOIN tv_episode_files tef
                                 ON tef.id = tefe.episode_file_id AND tef.available
                               WHERE tsn.show_id = s.id) AS held
                FROM tv_shows s
                WHERE s.title ILIKE $1 OR s.original_title ILIKE $1
            ) q
            ORDER BY (LOWER(q.title) = LOWER($2)) DESC, q.held DESC,
                     q.year DESC NULLS LAST, q.title ASC, q.id ASC
            LIMIT $3
            "#,
            pattern,
            title,
            limit,
        )
        .fetch_all(pool)
        .await?;
        matches.extend(rows.into_iter().map(|r| TitleMatch {
            kind: TitleKind::Show,
            id: r.id,
            title: r.title,
            year: r.year,
            held: r.held,
        }));
    }

    matches.sort_by(|a, b| {
        let exact = |m: &TitleMatch| m.title.eq_ignore_ascii_case(title);
        exact(b)
            .cmp(&exact(a))
            .then(b.held.cmp(&a.held))
            .then(b.year.cmp(&a.year))
            .then(a.title.cmp(&b.title))
    });
    matches.truncate(limit as usize);
    Ok(matches)
}

#[derive(Debug, Clone)]
pub struct CastMember {
    pub person_id: Uuid,
    pub name: String,
    pub character: Option<String>,
    pub job: Option<String>,
}

pub async fn movie_cast(pool: &PgPool, movie_id: Uuid, limit: i64) -> Result<Vec<CastMember>> {
    let rows = sqlx::query!(
        r#"
        SELECT p.id AS "person_id!", p.name AS "name!", mc.character_name
        FROM movie_cast mc
        JOIN people p ON p.id = mc.person_id
        JOIN movies m ON m.id = mc.movie_id
        WHERE mc.movie_id = $1
          AND NOT EXISTS (
              SELECT 1 FROM person_dismissed_credits d
              WHERE d.person_id = p.id
                AND d.media_type = 'movie'
                AND d.tmdb_id::text = m.identifiers->>'tmdb_id')
        ORDER BY mc.cast_order ASC NULLS LAST, p.name ASC, p.id ASC
        LIMIT $2
        "#,
        movie_id,
        limit,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| CastMember {
            person_id: r.person_id,
            name: r.name,
            character: r.character_name,
            job: None,
        })
        .collect())
}

pub async fn show_cast(pool: &PgPool, show_id: Uuid, limit: i64) -> Result<Vec<CastMember>> {
    let rows = sqlx::query!(
        r#"
        SELECT p.id AS "person_id!", p.name AS "name!", tc.character_name
        FROM tv_cast tc
        JOIN people p ON p.id = tc.person_id
        JOIN tv_shows s ON s.id = tc.show_id
        WHERE tc.show_id = $1
          AND NOT EXISTS (
              SELECT 1 FROM person_dismissed_credits d
              WHERE d.person_id = p.id
                AND d.media_type = 'tv'
                AND d.tmdb_id::text = s.identifiers->>'tmdb_id')
        ORDER BY tc.cast_order ASC NULLS LAST, p.name ASC, p.id ASC
        LIMIT $2
        "#,
        show_id,
        limit,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| CastMember {
            person_id: r.person_id,
            name: r.name,
            character: r.character_name,
            job: None,
        })
        .collect())
}

pub async fn movie_directors(pool: &PgPool, movie_id: Uuid) -> Result<Vec<CastMember>> {
    let rows = sqlx::query!(
        r#"
        SELECT p.id AS "person_id!", p.name AS "name!", mcr.job
        FROM movie_crew mcr
        JOIN people p ON p.id = mcr.person_id
        WHERE mcr.movie_id = $1 AND mcr.job = 'Director'
        ORDER BY p.name ASC, p.id ASC
        "#,
        movie_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| CastMember {
            person_id: r.person_id,
            name: r.name,
            character: None,
            job: Some(r.job),
        })
        .collect())
}

/// Show-level crew is `Creator` far more often than `Director` — a series has
/// a director per episode, and those live in `tv_episode_cast`, not here.
pub async fn show_creators(pool: &PgPool, show_id: Uuid) -> Result<Vec<CastMember>> {
    let rows = sqlx::query!(
        r#"
        SELECT p.id AS "person_id!", p.name AS "name!", tcr.job
        FROM tv_crew tcr
        JOIN people p ON p.id = tcr.person_id
        WHERE tcr.show_id = $1 AND tcr.job IN ('Creator', 'Director')
        ORDER BY tcr.job ASC, p.name ASC, p.id ASC
        "#,
        show_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| CastMember {
            person_id: r.person_id,
            name: r.name,
            character: None,
            job: Some(r.job),
        })
        .collect())
}
