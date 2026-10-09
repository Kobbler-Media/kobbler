//! Video chapters repository operations.

use super::Result;
use sqlx::{Executor, PgPool, Postgres, Row};
use uuid::Uuid;

#[derive(Debug)]
pub struct ChapterInput {
    pub movie_file_id: Option<Uuid>,
    pub episode_file_id: Option<Uuid>,
    pub chapter_index: i32,
    pub title: Option<String>,
    pub start_seconds: f64,
    pub end_seconds: f64,
}

#[derive(Debug, serde::Serialize)]
pub struct ChapterRow {
    pub id: Uuid,
    pub chapter_index: i32,
    pub title: Option<String>,
    pub start_seconds: f64,
    pub end_seconds: f64,
}

pub async fn insert_chapters(pool: &PgPool, inputs: &[ChapterInput]) -> Result<()> {
    if inputs.is_empty() {
        return Ok(());
    }

    let mut tx = pool.begin().await?;
    for input in inputs {
        let id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO video_chapters (id, movie_file_id, episode_file_id, chapter_index, title, start_seconds, end_seconds)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(id)
        .bind(input.movie_file_id)
        .bind(input.episode_file_id)
        .bind(input.chapter_index)
        .bind(input.title.as_deref())
        .bind(input.start_seconds)
        .bind(input.end_seconds)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn list_by_movie_file<'e, E>(exec: E, file_id: Uuid) -> Result<Vec<ChapterRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT id, chapter_index, title, start_seconds, end_seconds
        FROM video_chapters
        WHERE movie_file_id = $1
        ORDER BY chapter_index
        "#,
    )
    .bind(file_id)
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ChapterRow {
            id: r.get("id"),
            chapter_index: r.get("chapter_index"),
            title: r.get("title"),
            start_seconds: r.get("start_seconds"),
            end_seconds: r.get("end_seconds"),
        })
        .collect())
}

pub async fn list_by_episode_file<'e, E>(exec: E, file_id: Uuid) -> Result<Vec<ChapterRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT id, chapter_index, title, start_seconds, end_seconds
        FROM video_chapters
        WHERE episode_file_id = $1
        ORDER BY chapter_index
        "#,
    )
    .bind(file_id)
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ChapterRow {
            id: r.get("id"),
            chapter_index: r.get("chapter_index"),
            title: r.get("title"),
            start_seconds: r.get("start_seconds"),
            end_seconds: r.get("end_seconds"),
        })
        .collect())
}

pub async fn delete_by_movie_file<'e, E>(exec: E, file_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("DELETE FROM video_chapters WHERE movie_file_id = $1")
        .bind(file_id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected())
}

pub async fn delete_by_episode_file<'e, E>(exec: E, file_id: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query("DELETE FROM video_chapters WHERE episode_file_id = $1")
        .bind(file_id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected())
}
