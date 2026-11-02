//! The picture area `media_cache::picture_area` finds inside movie and episode files.

use super::Result;
use crate::media_cache::picture_area::PictureArea;
use crate::streaming::session::MediaType;
use sqlx::PgPool;
use uuid::Uuid;

/// A file whose thumbnails exist and have not been looked at for bars yet.
pub struct PictureCheckFile {
    pub media_type: MediaType,
    pub file_id: Uuid,
}

/// Files the analyze pipeline has run on, so their trickplay is on disk.
pub async fn list_needing_check(pool: &PgPool, limit: i64) -> Result<Vec<PictureCheckFile>> {
    let movies = sqlx::query_scalar!(
        r#"SELECT id FROM movie_files WHERE picture_checked_at IS NULL AND media_cache_version > 0 LIMIT $1"#,
        limit,
    )
    .fetch_all(pool)
    .await?;
    let episodes = sqlx::query_scalar!(
        r#"SELECT id FROM tv_episode_files WHERE picture_checked_at IS NULL AND media_cache_version > 0 LIMIT $1"#,
        (limit - movies.len() as i64).max(0),
    )
    .fetch_all(pool)
    .await?;
    Ok(movies
        .into_iter()
        .map(|file_id| PictureCheckFile {
            media_type: MediaType::Movie,
            file_id,
        })
        .chain(episodes.into_iter().map(|file_id| PictureCheckFile {
            media_type: MediaType::Episode,
            file_id,
        }))
        .collect())
}

/// `None` area records a file checked and found to have no bars, or none that could be told.
pub async fn record(
    pool: &PgPool,
    file: &PictureCheckFile,
    area: Option<PictureArea>,
) -> Result<()> {
    let area = area.filter(|a| *a != PictureArea::FULL);
    let (x, y, w, h) = (
        area.map(|a| a.x),
        area.map(|a| a.y),
        area.map(|a| a.width),
        area.map(|a| a.height),
    );
    match file.media_type {
        MediaType::Movie => sqlx::query!(
            r#"UPDATE movie_files SET picture_x = $2, picture_y = $3, picture_width = $4, picture_height = $5,
                      picture_checked_at = NOW() WHERE id = $1"#,
            file.file_id, x, y, w, h,
        )
        .execute(pool)
        .await?,
        MediaType::Episode => sqlx::query!(
            r#"UPDATE tv_episode_files SET picture_x = $2, picture_y = $3, picture_width = $4, picture_height = $5,
                      picture_checked_at = NOW() WHERE id = $1"#,
            file.file_id, x, y, w, h,
        )
        .execute(pool)
        .await?,
        MediaType::Recording => return Ok(()),
    };
    Ok(())
}

/// The bars-free picture of a file, when it has bars.
pub async fn get(
    pool: &PgPool,
    media_type: MediaType,
    file_id: Uuid,
) -> Result<Option<PictureArea>> {
    let row = match media_type {
        MediaType::Movie => sqlx::query!(
            r#"SELECT picture_x, picture_y, picture_width, picture_height FROM movie_files WHERE id = $1"#,
            file_id,
        )
        .fetch_optional(pool)
        .await?
        .map(|r| (r.picture_x, r.picture_y, r.picture_width, r.picture_height)),
        MediaType::Episode => sqlx::query!(
            r#"SELECT picture_x, picture_y, picture_width, picture_height FROM tv_episode_files WHERE id = $1"#,
            file_id,
        )
        .fetch_optional(pool)
        .await?
        .map(|r| (r.picture_x, r.picture_y, r.picture_width, r.picture_height)),
        MediaType::Recording => None,
    };
    Ok(row.and_then(|(x, y, width, height)| {
        Some(PictureArea {
            x: x?,
            y: y?,
            width: width?,
            height: height?,
        })
    }))
}
