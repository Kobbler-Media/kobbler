use sqlx::PgPool;
use time::{Date, OffsetDateTime};

use super::Result;
use crate::tmdb_titles::TitleMedia;
use crate::tmdb_titles::export::ExportTitle;
use crate::video_metadata::tmdb::TmdbTitle;

pub struct SyncState {
    pub export_date: Option<Date>,
    pub localized_at: Option<OffsetDateTime>,
}

pub async fn sync_state(pool: &PgPool, media: TitleMedia) -> Result<SyncState> {
    let row = sqlx::query_as!(
        SyncState,
        "SELECT export_date, localized_at FROM tmdb_title_syncs WHERE media_type = $1",
        media.as_str(),
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.unwrap_or(SyncState {
        export_date: None,
        localized_at: None,
    }))
}

/// Swaps one medium's titles for a new export in one transaction, so suggestions never see
/// a half-loaded set. English titles survive for every title still in the export.
pub async fn replace_export(
    pool: &PgPool,
    media: TitleMedia,
    export_date: Date,
    titles: &[ExportTitle],
) -> Result<()> {
    let ids: Vec<i32> = titles.iter().map(|t| t.tmdb_id).collect();
    let originals: Vec<String> = titles.iter().map(|t| t.original_title.clone()).collect();
    let popularity: Vec<f32> = titles.iter().map(|t| t.popularity).collect();

    let mut tx = pool.begin().await?;
    sqlx::query!(
        "DELETE FROM tmdb_titles
         WHERE media_type = $1 AND tmdb_id NOT IN (SELECT unnest($2::int[]))",
        media.as_str(),
        &ids,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        r#"
        INSERT INTO tmdb_titles (media_type, tmdb_id, original_title, popularity)
        SELECT $1, t.id, t.original_title, t.popularity
        FROM UNNEST($2::int[], $3::text[], $4::real[]) AS t(id, original_title, popularity)
        ON CONFLICT (media_type, tmdb_id) DO UPDATE
        SET original_title = EXCLUDED.original_title,
            title = NULLIF(tmdb_titles.title, EXCLUDED.original_title),
            popularity = EXCLUDED.popularity
        WHERE (tmdb_titles.original_title, tmdb_titles.popularity)
              IS DISTINCT FROM (EXCLUDED.original_title, EXCLUDED.popularity)
        "#,
        media.as_str(),
        &ids,
        &originals,
        &popularity,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "INSERT INTO tmdb_title_syncs (media_type, export_date) VALUES ($1, $2)
         ON CONFLICT (media_type) DO UPDATE SET export_date = EXCLUDED.export_date",
        media.as_str(),
        export_date,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// English titles from TMDB's discover lists. A title the exports have not reached yet is
/// added with discover's popularity; the next export decides whether it stays.
pub async fn apply_english_titles(
    pool: &PgPool,
    media: TitleMedia,
    titles: &[TmdbTitle],
    mark_localized: bool,
) -> Result<()> {
    let ids: Vec<i32> = titles.iter().map(|t| t.id).collect();
    let english: Vec<String> = titles.iter().map(|t| t.title.trim().to_string()).collect();
    let originals: Vec<String> = titles
        .iter()
        .map(|t| t.original_title.trim().to_string())
        .collect();
    let popularity: Vec<f32> = titles.iter().map(|t| t.popularity).collect();

    let mut tx = pool.begin().await?;
    sqlx::query!(
        r#"
        INSERT INTO tmdb_titles (media_type, tmdb_id, original_title, title, popularity)
        SELECT $1, t.id, COALESCE(NULLIF(t.original_title, ''), t.title),
               NULLIF(t.title, COALESCE(NULLIF(t.original_title, ''), t.title)), t.popularity
        FROM UNNEST($2::int[], $3::text[], $4::text[], $5::real[])
             AS t(id, title, original_title, popularity)
        ON CONFLICT (media_type, tmdb_id) DO UPDATE
        SET title = NULLIF(EXCLUDED.title, tmdb_titles.original_title)
        WHERE tmdb_titles.title IS DISTINCT FROM NULLIF(EXCLUDED.title, tmdb_titles.original_title)
        "#,
        media.as_str(),
        &ids,
        &english,
        &originals,
        &popularity,
    )
    .execute(&mut *tx)
    .await?;
    if mark_localized {
        sqlx::query!(
            "INSERT INTO tmdb_title_syncs (media_type, localized_at) VALUES ($1, NOW())
             ON CONFLICT (media_type) DO UPDATE SET localized_at = EXCLUDED.localized_at",
            media.as_str(),
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
