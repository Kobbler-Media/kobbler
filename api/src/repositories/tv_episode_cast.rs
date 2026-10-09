//! Per-episode credit repository operations (guest stars and episode crew).

use super::Result;
use sqlx::{Executor, PgPool, Postgres};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct EpisodeCreditInput {
    pub person_id: Uuid,
    pub credit_type: String,
    pub character_name: Option<String>,
    pub cast_order: Option<i32>,
    pub department: Option<String>,
    pub job: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EpisodeCreditRow {
    pub id: Uuid,
    pub person_id: Uuid,
    pub person_name: String,
    pub person_image_url: Option<String>,
    pub credit_type: String,
    pub character_name: Option<String>,
    pub cast_order: Option<i32>,
    pub department: Option<String>,
    pub job: Option<String>,
}

pub const CREDIT_GUEST: &str = "guest";
pub const CREDIT_CREW: &str = "crew";

pub async fn list_for_episode<'e, E>(exec: E, episode_id: Uuid) -> Result<Vec<EpisodeCreditRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT tec.id, tec.person_id, p.name AS person_name,
               CASE WHEN p.images->'profile' IS NOT NULL
                    AND (p.images->'profile'->>'source_url' IS NOT NULL OR p.images->'profile'->>'cached_at' IS NOT NULL)
                    THEN '/api/v1/admin/people/' || p.id || '/image'
                    ELSE NULL
               END AS person_image_url,
               tec.credit_type, tec.character_name, tec.cast_order,
               tec.department, tec.job
        FROM tv_episode_cast tec
        JOIN people p ON p.id = tec.person_id
        WHERE tec.episode_id = $1
        ORDER BY tec.credit_type ASC, tec.cast_order ASC NULLS LAST, p.name ASC
        "#,
        episode_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| EpisodeCreditRow {
            id: r.id,
            person_id: r.person_id,
            person_name: r.person_name,
            person_image_url: r.person_image_url,
            credit_type: r.credit_type,
            character_name: r.character_name,
            cast_order: r.cast_order,
            department: r.department,
            job: r.job,
        })
        .collect())
}

/// Replace every credit for an episode and stamp `credits_fetched_at`.
///
/// The stamp is what makes an episode with genuinely no guest stars stop
/// re-hitting the provider on every ask.
pub async fn replace_for_episode(
    pool: &PgPool,
    episode_id: Uuid,
    credits: Vec<EpisodeCreditInput>,
) -> Result<()> {
    let mut tx = pool.begin().await?;

    sqlx::query!(
        "DELETE FROM tv_episode_cast WHERE episode_id = $1",
        episode_id
    )
    .execute(&mut *tx)
    .await?;

    for c in credits {
        sqlx::query!(
            r#"
            INSERT INTO tv_episode_cast
                (id, episode_id, person_id, credit_type, character_name, cast_order, department, job)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
            Uuid::now_v7(),
            episode_id,
            c.person_id,
            c.credit_type,
            c.character_name.as_deref(),
            c.cast_order,
            c.department.as_deref(),
            c.job.as_deref(),
        )
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query!(
        "UPDATE tv_episodes SET credits_fetched_at = NOW() WHERE id = $1",
        episode_id
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// `None` when the provider has never been asked for this episode's credits.
pub async fn fetched_at<'e, E>(exec: E, episode_id: Uuid) -> Result<Option<time::OffsetDateTime>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!(
        "SELECT credits_fetched_at FROM tv_episodes WHERE id = $1",
        episode_id
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.flatten())
}
