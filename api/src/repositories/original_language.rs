use super::Result;
use sqlx::PgPool;
use uuid::Uuid;

pub struct Known {
    pub id: Uuid,
    pub original_language: Option<String>,
    pub tmdb_id: Option<String>,
}

pub async fn movie(pool: &PgPool, movie_id: Uuid) -> Result<Option<Known>> {
    Ok(sqlx::query_as!(
        Known,
        "SELECT id, original_language, identifiers->>'tmdb_id' AS tmdb_id
         FROM movies WHERE id = $1",
        movie_id,
    )
    .fetch_optional(pool)
    .await?)
}

pub async fn movie_by_tmdb(pool: &PgPool, tmdb_id: &str) -> Result<Option<Known>> {
    Ok(sqlx::query_as!(
        Known,
        "SELECT id, original_language, identifiers->>'tmdb_id' AS tmdb_id
         FROM movies WHERE identifiers->>'tmdb_id' = $1
         LIMIT 1",
        tmdb_id,
    )
    .fetch_optional(pool)
    .await?)
}

pub async fn show_of_episode(pool: &PgPool, episode_id: Uuid) -> Result<Option<Known>> {
    Ok(sqlx::query_as!(
        Known,
        "SELECT s.id, s.original_language, s.identifiers->>'tmdb_id' AS tmdb_id
         FROM tv_episodes e
         JOIN tv_seasons se ON se.id = e.season_id
         JOIN tv_shows s ON s.id = se.show_id
         WHERE e.id = $1",
        episode_id,
    )
    .fetch_optional(pool)
    .await?)
}

pub async fn show_of_episode_file(pool: &PgPool, file_id: Uuid) -> Result<Option<Known>> {
    Ok(sqlx::query_as!(
        Known,
        "SELECT s.id, s.original_language, s.identifiers->>'tmdb_id' AS tmdb_id
         FROM tv_episode_file_episodes f
         JOIN tv_episodes e ON e.id = f.episode_id
         JOIN tv_seasons se ON se.id = e.season_id
         JOIN tv_shows s ON s.id = se.show_id
         WHERE f.episode_file_id = $1
         LIMIT 1",
        file_id,
    )
    .fetch_optional(pool)
    .await?)
}

pub async fn show_by_tmdb(pool: &PgPool, tmdb_id: &str) -> Result<Option<Known>> {
    Ok(sqlx::query_as!(
        Known,
        "SELECT id, original_language, identifiers->>'tmdb_id' AS tmdb_id
         FROM tv_shows WHERE identifiers->>'tmdb_id' = $1
         LIMIT 1",
        tmdb_id,
    )
    .fetch_optional(pool)
    .await?)
}

pub async fn set_movie(pool: &PgPool, movie_id: Uuid, language: &str) -> Result<()> {
    sqlx::query!(
        "UPDATE movies SET original_language = $2 WHERE id = $1",
        movie_id,
        language,
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_show(pool: &PgPool, show_id: Uuid, language: &str) -> Result<()> {
    sqlx::query!(
        "UPDATE tv_shows SET original_language = $2 WHERE id = $1",
        show_id,
        language,
    )
    .execute(pool)
    .await?;
    Ok(())
}
