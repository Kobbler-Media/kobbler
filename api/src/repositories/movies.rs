//! Movie repository operations.

use super::{PaginationDirection, Result, normalize_rating};
use crate::images::ImageMap;
use serde_json::Value;
use sqlx::{Executor, PgPool, Postgres, Row};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Input for creating a movie.
#[derive(Debug, Default)]
pub struct MovieInput {
    pub title: String,
    pub sort_title: Option<String>,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub runtime: Option<i32>,
    pub description: Option<String>,
    pub tagline: Option<String>,
    pub images: Option<ImageMap>,
    pub identifiers: Option<Value>,
    pub genres: Option<Vec<String>>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub content_rating: Option<String>,
    pub release_date: Option<time::Date>,
    pub is_released: Option<bool>,
    pub quality_profile_id: Option<Uuid>,
    pub preferred_languages: Option<Vec<String>>,
    pub media_root_id: Option<Uuid>,
    pub streamed_at: Option<time::OffsetDateTime>,
}

/// Input for updating a movie. All fields optional.
#[derive(Debug, Default)]
pub struct MovieUpdate {
    pub title: Option<String>,
    pub sort_title: Option<String>,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub runtime: Option<i32>,
    pub description: Option<String>,
    pub tagline: Option<String>,
    pub images: Option<ImageMap>,
    pub identifiers: Option<Value>,
    pub genres: Option<Vec<String>>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub content_rating: Option<String>,
    pub release_date: Option<time::Date>,
    pub is_released: Option<bool>,
    pub alternate_titles: Option<Vec<String>>,
    pub quality_profile_id: Option<Option<Uuid>>,
    pub preferred_languages: Option<Option<Vec<String>>>,
    pub media_root_id: Option<Option<Uuid>>,
    pub streamed_at: Option<time::OffsetDateTime>,
}

/// Lightweight movie row for listings.
#[derive(Debug)]
pub struct MovieRow {
    pub id: Uuid,
    pub title: String,
    pub sort_title: Option<String>,
    pub year: Option<i32>,
    pub images: ImageMap,
    pub identifiers: Value,
    pub is_in_library: bool,
    pub want_status: String,
    pub release_date: Option<time::Date>,
    pub quality_profile_id: Option<Uuid>,
    pub preferred_languages: Option<Vec<String>>,
    pub alternate_titles: Option<Vec<String>>,
    pub media_root_id: Option<Uuid>,
    pub streamed_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// Full movie detail including cast and crew.
#[derive(Debug)]
pub struct MovieDetail {
    pub id: Uuid,
    pub title: String,
    pub sort_title: Option<String>,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub runtime: Option<i32>,
    pub description: Option<String>,
    pub tagline: Option<String>,
    pub images: ImageMap,
    pub identifiers: Value,
    pub genres: Option<Vec<String>>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub content_rating: Option<String>,
    pub release_date: Option<time::Date>,
    pub is_released: Option<bool>,
    pub quality_profile_id: Option<Uuid>,
    pub preferred_languages: Option<Vec<String>>,
    pub media_root_id: Option<Uuid>,
    pub is_in_library: bool,
    pub want_status: String,
    pub streamed_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub cast: Vec<CastRow>,
    pub crew: Vec<CrewRow>,
}

/// Filters for movie listing.
#[derive(Debug, Default)]
pub struct MovieListFilters {
    pub want_status: Option<String>,
    pub is_in_library: Option<bool>,
    pub search: Option<String>,
    pub limit: i64,
    pub cursor_title: Option<String>,
    pub cursor_id: Option<Uuid>,
    pub direction: PaginationDirection,
    /// Include the cursor row itself in the result instead of paging past it.
    pub cursor_inclusive: bool,
    /// "title" (default) or "created_at".
    pub sort: Option<String>,
}

/// Cast input for replace_cast.
#[derive(Debug)]
pub struct CastInput {
    pub person_id: Uuid,
    pub character_name: Option<String>,
    pub cast_order: Option<i32>,
}

/// Crew input for replace_crew.
#[derive(Debug)]
pub struct CrewInput {
    pub person_id: Uuid,
    pub department: String,
    pub job: String,
}

/// Cast row from joined queries.
#[derive(Debug)]
pub struct CastRow {
    pub id: Uuid,
    pub person_id: Uuid,
    pub person_name: String,
    pub person_image_url: Option<String>,
    pub character_name: Option<String>,
    pub cast_order: Option<i32>,
}

/// Crew row from joined queries.
#[derive(Debug)]
pub struct CrewRow {
    pub person_id: Uuid,
    pub person_name: String,
    pub department: String,
    pub job: String,
}

fn row_to_movie(r: sqlx::postgres::PgRow) -> MovieRow {
    let images_json: Value = r.get("images");
    let images: ImageMap = serde_json::from_value(images_json).unwrap_or_default();
    MovieRow {
        id: r.get("id"),
        title: r.get("title"),
        sort_title: r.get("sort_title"),
        year: r.get("year"),
        images,
        identifiers: r.get("identifiers"),
        is_in_library: r.get("is_in_library"),
        want_status: r.get("want_status"),
        release_date: r.get("release_date"),
        quality_profile_id: r.get("quality_profile_id"),
        preferred_languages: r.get("preferred_languages"),
        alternate_titles: r.get("alternate_titles"),
        media_root_id: r.get("media_root_id"),
        streamed_at: r.get("streamed_at"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

// =============================================================================
// Create
// =============================================================================

/// Create a new movie.
pub async fn create<'e, E>(exec: E, input: MovieInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let identifiers = input.identifiers.unwrap_or_else(|| serde_json::json!({}));
    let is_released = input.is_released.unwrap_or(true);
    let images = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();
    let rating = input.rating.map(normalize_rating);

    let row = sqlx::query!(
        r#"
        INSERT INTO movies (id, title, sort_title, original_title, year, runtime,
                            description, tagline, images, identifiers,
                            genres, rating, rating_count, content_rating,
                            release_date, is_released, quality_profile_id,
                            preferred_languages, media_root_id, streamed_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)
        RETURNING id
        "#,
        id,
        input.title,
        input.sort_title.as_deref(),
        input.original_title.as_deref(),
        input.year,
        input.runtime,
        input.description.as_deref(),
        input.tagline.as_deref(),
        &images,
        &identifiers,
        input.genres.as_deref(),
        rating,
        input.rating_count,
        input.content_rating.as_deref(),
        input.release_date,
        is_released,
        input.quality_profile_id,
        input.preferred_languages.as_deref(),
        input.media_root_id,
        input.streamed_at,
    )
    .fetch_one(exec)
    .await?;

    Ok(row.id)
}

// =============================================================================
// Read
// =============================================================================

/// Get a movie by ID (lightweight row).
pub async fn get<'e, E>(exec: E, id: Uuid) -> Result<Option<MovieRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT id, title, sort_title, year, images, identifiers,
               EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = movies.id) AS is_in_library,
               want_status, release_date, quality_profile_id,
               preferred_languages, alternate_titles, media_root_id, streamed_at, created_at, updated_at
        FROM movies
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(exec)
    .await?;

    Ok(row.map(row_to_movie))
}

/// Get a movie with full details including cast and crew.
pub async fn get_with_details(pool: &sqlx::PgPool, id: Uuid) -> Result<Option<MovieDetail>> {
    let row = sqlx::query!(
        r#"
        SELECT id, title, sort_title, original_title, year, runtime,
               description, tagline, images, identifiers,
               genres, rating, rating_count, content_rating,
               release_date, is_released, quality_profile_id, preferred_languages,
               media_root_id,
               EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = movies.id) AS "is_in_library!",
               want_status, streamed_at, created_at, updated_at
        FROM movies
        WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(pool)
    .await?;

    let Some(r) = row else {
        return Ok(None);
    };

    let cast = get_cast(pool, id).await?;
    let crew = get_crew(pool, id).await?;

    let images: ImageMap = serde_json::from_value(r.images.clone()).unwrap_or_default();

    Ok(Some(MovieDetail {
        id: r.id,
        title: r.title,
        sort_title: r.sort_title,
        original_title: r.original_title,
        year: r.year,
        runtime: r.runtime,
        description: r.description,
        tagline: r.tagline,
        images,
        identifiers: r.identifiers.clone(),
        genres: r.genres,
        rating: r.rating,
        rating_count: r.rating_count,
        content_rating: r.content_rating,
        release_date: r.release_date,
        is_released: r.is_released,
        quality_profile_id: r.quality_profile_id,
        preferred_languages: r.preferred_languages,
        media_root_id: r.media_root_id,
        is_in_library: r.is_in_library,
        want_status: r.want_status,
        streamed_at: r.streamed_at,
        created_at: r.created_at,
        updated_at: r.updated_at,
        cast,
        crew,
    }))
}

/// List movies with optional filters and cursor-based pagination.
pub async fn list<'e, E>(exec: E, filters: &MovieListFilters) -> Result<Vec<MovieRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let mut sql = String::from(
        r#"
        SELECT id, title, sort_title, year, images, identifiers,
               EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = movies.id) AS is_in_library,
               want_status, release_date, quality_profile_id,
               preferred_languages, alternate_titles, media_root_id, streamed_at, created_at, updated_at
        FROM movies
        WHERE TRUE
        "#,
    );
    let mut params: Vec<String> = Vec::new();

    if let Some(ref status) = filters.want_status {
        params.push(status.clone());
        let idx = params.len();
        sql.push_str(&format!(" AND want_status = ${}", idx));
    }

    if let Some(in_lib) = filters.is_in_library {
        if in_lib {
            sql.push_str(
                " AND EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = movies.id)",
            );
        } else {
            sql.push_str(
                " AND NOT EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = movies.id)",
            );
        }
    }

    if let Some(ref q) = filters.search {
        let trimmed = q.trim();
        if !trimmed.is_empty() {
            params.push(format!("%{}%", trimmed));
            let idx = params.len();
            sql.push_str(&format!(" AND title ILIKE ${}", idx));
        }
    }

    let sort_by_created = filters.sort.as_deref() == Some("created_at");

    // Cursor-based pagination (only with title sort)
    if !sort_by_created
        && let (Some(cursor_title), Some(cursor_id)) = (&filters.cursor_title, filters.cursor_id)
    {
        let title_idx = params.len() + 1;
        let id_idx = params.len() + 2;
        let op = match (filters.direction, filters.cursor_inclusive) {
            (PaginationDirection::Forward, false) => ">",
            (PaginationDirection::Forward, true) => ">=",
            (PaginationDirection::Backward, false) => "<",
            (PaginationDirection::Backward, true) => "<=",
        };
        sql.push_str(&format!(
            " AND (COALESCE(sort_title, title), id) {} (${}, ${}::uuid)",
            op, title_idx, id_idx
        ));
        params.push(cursor_title.clone());
        params.push(cursor_id.to_string());
    }

    if sort_by_created {
        sql.push_str(" ORDER BY created_at DESC, id DESC");
    } else {
        match filters.direction {
            PaginationDirection::Forward => {
                sql.push_str(" ORDER BY COALESCE(sort_title, title) ASC, id ASC");
            }
            PaginationDirection::Backward => {
                sql.push_str(" ORDER BY COALESCE(sort_title, title) DESC, id DESC");
            }
        }
    }

    let limit_idx = params.len() + 1;
    sql.push_str(&format!(" LIMIT ${}", limit_idx));

    let mut q = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()));
    for param in &params {
        q = q.bind(param);
    }
    q = q.bind(filters.limit);

    let rows = q.fetch_all(exec).await?;
    let mut result: Vec<MovieRow> = rows.into_iter().map(row_to_movie).collect();

    // Reverse backward results so they're in ascending order
    if filters.direction == PaginationDirection::Backward {
        result.reverse();
    }

    Ok(result)
}

/// Count movies matching filters (for total_count on first page).
pub async fn count<'e, E>(exec: E, filters: &MovieListFilters) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let mut sql = String::from("SELECT COUNT(*) as count FROM movies WHERE TRUE");
    let mut params: Vec<String> = Vec::new();

    if let Some(ref status) = filters.want_status {
        params.push(status.clone());
        let idx = params.len();
        sql.push_str(&format!(" AND want_status = ${}", idx));
    }

    if let Some(in_lib) = filters.is_in_library {
        if in_lib {
            sql.push_str(
                " AND EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = movies.id)",
            );
        } else {
            sql.push_str(
                " AND NOT EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = movies.id)",
            );
        }
    }

    if let Some(ref q) = filters.search {
        let trimmed = q.trim();
        if !trimmed.is_empty() {
            params.push(format!("%{}%", trimmed));
            let idx = params.len();
            sql.push_str(&format!(" AND title ILIKE ${}", idx));
        }
    }

    let mut q = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()));
    for param in &params {
        q = q.bind(param);
    }

    let row = q.fetch_one(exec).await?;
    Ok(row.get("count"))
}

// =============================================================================
// Update
// =============================================================================

/// Update a movie. Only non-None fields are updated.
/// Identifiers are replaced (not merged) if provided, preserving user edits.
pub async fn update<'e, E>(exec: E, id: Uuid, input: MovieUpdate) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let images = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();
    let id_update = input.identifiers.is_some();
    let identifiers = input.identifiers.unwrap_or_default();
    let qp_update = input.quality_profile_id;
    let pl_update = input.preferred_languages.flatten();
    let mr_update = input.media_root_id;
    let rating = input.rating.map(normalize_rating);

    let result = sqlx::query!(
        r#"
        UPDATE movies SET
            title = COALESCE($2, title),
            sort_title = COALESCE($3, sort_title),
            original_title = COALESCE($4, original_title),
            year = COALESCE($5, year),
            runtime = COALESCE($6, runtime),
            description = COALESCE($7, description),
            tagline = COALESCE($8, tagline),
            images = images || $9,
            identifiers = CASE WHEN $10 THEN $11 ELSE identifiers END,
            genres = COALESCE($12, genres),
            rating = COALESCE($13, rating),
            rating_count = COALESCE($14, rating_count),
            content_rating = COALESCE($15, content_rating),
            release_date = COALESCE($16, release_date),
            is_released = COALESCE($17, is_released),
            quality_profile_id = CASE WHEN $18 THEN $19 ELSE quality_profile_id END,
            preferred_languages = COALESCE($20, preferred_languages),
            media_root_id = CASE WHEN $21 THEN $22 ELSE media_root_id END,
            alternate_titles = COALESCE($23, alternate_titles),
            streamed_at = COALESCE($24, streamed_at),
            updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        input.title.as_deref(),
        input.sort_title.as_deref(),
        input.original_title.as_deref(),
        input.year,
        input.runtime,
        input.description.as_deref(),
        input.tagline.as_deref(),
        &images,
        id_update,
        &identifiers,
        input.genres.as_deref(),
        rating,
        input.rating_count,
        input.content_rating.as_deref(),
        input.release_date,
        input.is_released,
        qp_update.is_some(),
        qp_update.flatten(),
        pl_update.as_deref(),
        mr_update.is_some(),
        mr_update.flatten(),
        input.alternate_titles.as_deref(),
        input.streamed_at,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete a movie by ID.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM movies WHERE id = $1", id)
        .execute(exec)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete all movies belonging to a media root, returning deleted IDs.
pub async fn delete_by_media_root<'e, E>(exec: E, media_root_id: Uuid) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!(
        "DELETE FROM movies WHERE media_root_id = $1 RETURNING id",
        media_root_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(ids)
}

/// Delete orphaned movies: no *available* files, not wanted, no *available*
/// media root file pointing to them, not debrid-streamed, and not recently
/// updated (grace period for in-progress jobs).
/// Returns IDs of deleted movies for search index cleanup.
pub async fn delete_orphaned_movies<'e, E>(exec: E) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!(
        r#"
        DELETE FROM movies
        WHERE want_status = 'none'
          AND streamed_at IS NULL
          AND updated_at < NOW() - INTERVAL '30 minutes'
          AND NOT EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = movies.id AND mf.available = true)
          AND NOT EXISTS (
              SELECT 1 FROM media_root_files mrf
              WHERE mrf.entity_id = movies.id AND mrf.entity_type = 'movie' AND mrf.available = true
          )
        RETURNING id
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(ids)
}

// =============================================================================
// Status Operations
// =============================================================================

/// Set a movie's want_status.
pub async fn set_want_status<'e, E>(exec: E, id: Uuid, status: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE movies SET want_status = $2, updated_at = NOW() WHERE id = $1",
        id,
        status,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Upsert a movie by a provider-specific identifier.
///
/// Tries atomic `ON CONFLICT` first; falls back to advisory-locked
/// SELECT + INSERT/UPDATE for providers without a unique index.
pub async fn upsert_by_provider(
    pool: &PgPool,
    provider_type: &str,
    provider_id: &str,
    input: MovieInput,
) -> Result<Uuid> {
    let id_key = format!("{provider_type}_id");
    let mut identifiers = input.identifiers.unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = identifiers.as_object_mut() {
        obj.insert(id_key.clone(), Value::String(provider_id.to_string()));
    }
    let is_released = input.is_released.unwrap_or(true);
    let images = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();
    let rating = input.rating.map(normalize_rating);

    // Try atomic ON CONFLICT first
    let id = Uuid::now_v7();
    let upsert_sql = format!(
        r#"
        INSERT INTO movies (id, title, sort_title, original_title, year, runtime,
                            description, tagline, images, identifiers,
                            genres, rating, rating_count, content_rating,
                            release_date, is_released, quality_profile_id,
                            preferred_languages, media_root_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)
        ON CONFLICT ((identifiers->>'{id_key}'))
        DO UPDATE SET
            title = EXCLUDED.title,
            -- Existing value wins: it is either a hand-entered override or one the
            -- sort_title trigger derived, and the trigger re-derives it from the new
            -- title on this same UPDATE when it was not overridden.
            sort_title = COALESCE(movies.sort_title, EXCLUDED.sort_title),
            original_title = COALESCE(EXCLUDED.original_title, movies.original_title),
            year = COALESCE(EXCLUDED.year, movies.year),
            runtime = COALESCE(EXCLUDED.runtime, movies.runtime),
            description = COALESCE(EXCLUDED.description, movies.description),
            tagline = COALESCE(EXCLUDED.tagline, movies.tagline),
            images = movies.images || EXCLUDED.images,
            identifiers = movies.identifiers || EXCLUDED.identifiers,
            genres = COALESCE(EXCLUDED.genres, movies.genres),
            rating = COALESCE(EXCLUDED.rating, movies.rating),
            rating_count = COALESCE(EXCLUDED.rating_count, movies.rating_count),
            content_rating = COALESCE(EXCLUDED.content_rating, movies.content_rating),
            release_date = COALESCE(EXCLUDED.release_date, movies.release_date),
            is_released = EXCLUDED.is_released,
            quality_profile_id = COALESCE(EXCLUDED.quality_profile_id, movies.quality_profile_id),
            preferred_languages = COALESCE(EXCLUDED.preferred_languages, movies.preferred_languages),
            media_root_id = COALESCE(EXCLUDED.media_root_id, movies.media_root_id),
            updated_at = NOW()
        RETURNING id
        "#
    );

    let result = sqlx::query_as::<_, (Uuid,)>(sqlx::AssertSqlSafe(upsert_sql.as_str()))
        .bind(id)
        .bind(&input.title)
        .bind(input.sort_title.as_deref())
        .bind(input.original_title.as_deref())
        .bind(input.year)
        .bind(input.runtime)
        .bind(input.description.as_deref())
        .bind(input.tagline.as_deref())
        .bind(&images)
        .bind(&identifiers)
        .bind(input.genres.as_deref())
        .bind(rating)
        .bind(input.rating_count)
        .bind(input.content_rating.as_deref())
        .bind(input.release_date)
        .bind(is_released)
        .bind(input.quality_profile_id)
        .bind(input.preferred_languages.as_deref())
        .bind(input.media_root_id)
        .fetch_one(pool)
        .await;

    match result {
        Ok((row_id,)) => return Ok(row_id),
        Err(ref e) if super::is_no_unique_constraint_error(e) => {}
        Err(e) => return Err(e.into()),
    }

    // Fallback: advisory-locked SELECT + INSERT/UPDATE
    let lock_key = super::advisory_lock_key("movies", &id_key, provider_id);
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(lock_key)
        .execute(&mut *tx)
        .await?;

    let existing =
        super::resolve_provider_row(&mut tx, "movies", &id_key, provider_id, &identifiers).await?;

    let result_id = if let Some(existing_id) = existing {
        sqlx::query(
            r#"
            UPDATE movies SET
                title = $2,
                sort_title = COALESCE($3, sort_title),
                original_title = COALESCE($4, original_title),
                year = COALESCE($5, year),
                runtime = COALESCE($6, runtime),
                description = COALESCE($7, description),
                tagline = COALESCE($8, tagline),
                images = images || $9,
                identifiers = identifiers || $10,
                genres = COALESCE($11, genres),
                rating = COALESCE($12, rating),
                rating_count = COALESCE($13, rating_count),
                content_rating = COALESCE($14, content_rating),
                release_date = COALESCE($15, release_date),
                is_released = $16,
                quality_profile_id = COALESCE($17, quality_profile_id),
                preferred_languages = COALESCE($18, preferred_languages),
                media_root_id = COALESCE($19, media_root_id),
                updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(existing_id)
        .bind(&input.title)
        .bind(input.sort_title.as_deref())
        .bind(input.original_title.as_deref())
        .bind(input.year)
        .bind(input.runtime)
        .bind(input.description.as_deref())
        .bind(input.tagline.as_deref())
        .bind(&images)
        .bind(&identifiers)
        .bind(input.genres.as_deref())
        .bind(rating)
        .bind(input.rating_count)
        .bind(input.content_rating.as_deref())
        .bind(input.release_date)
        .bind(is_released)
        .bind(input.quality_profile_id)
        .bind(input.preferred_languages.as_deref())
        .bind(input.media_root_id)
        .execute(&mut *tx)
        .await?;
        existing_id
    } else {
        let new_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO movies (id, title, sort_title, original_title, year, runtime,
                                description, tagline, images, identifiers,
                                genres, rating, rating_count, content_rating,
                                release_date, is_released, quality_profile_id,
                                preferred_languages, media_root_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)
            "#,
        )
        .bind(new_id)
        .bind(&input.title)
        .bind(input.sort_title.as_deref())
        .bind(input.original_title.as_deref())
        .bind(input.year)
        .bind(input.runtime)
        .bind(input.description.as_deref())
        .bind(input.tagline.as_deref())
        .bind(&images)
        .bind(&identifiers)
        .bind(input.genres.as_deref())
        .bind(rating)
        .bind(input.rating_count)
        .bind(input.content_rating.as_deref())
        .bind(input.release_date)
        .bind(is_released)
        .bind(input.quality_profile_id)
        .bind(input.preferred_languages.as_deref())
        .bind(input.media_root_id)
        .execute(&mut *tx)
        .await?;
        new_id
    };

    tx.commit().await?;
    Ok(result_id)
}

// =============================================================================
// Cast & Crew
// =============================================================================

/// Get cast for a movie, joined with people.
async fn get_cast<'e, E>(exec: E, movie_id: Uuid) -> Result<Vec<CastRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT mc.id, mc.person_id, p.name AS person_name,
               CASE WHEN p.images->'profile' IS NOT NULL
                    AND (p.images->'profile'->>'source_url' IS NOT NULL OR p.images->'profile'->>'cached_at' IS NOT NULL)
                    THEN '/api/v1/admin/people/' || p.id || '/image'
                    ELSE NULL
               END AS person_image_url,
               mc.character_name, mc.cast_order
        FROM movie_cast mc
        JOIN people p ON p.id = mc.person_id
        WHERE mc.movie_id = $1
        ORDER BY mc.cast_order ASC NULLS LAST
        "#,
        movie_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| CastRow {
            id: r.id,
            person_id: r.person_id,
            person_name: r.person_name,
            person_image_url: r.person_image_url,
            character_name: r.character_name,
            cast_order: r.cast_order,
        })
        .collect())
}

/// Get crew for a movie, joined with people.
async fn get_crew<'e, E>(exec: E, movie_id: Uuid) -> Result<Vec<CrewRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT mc.person_id, p.name AS person_name, mc.department, mc.job
        FROM movie_crew mc
        JOIN people p ON p.id = mc.person_id
        WHERE mc.movie_id = $1
        ORDER BY mc.department ASC, mc.job ASC
        "#,
        movie_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| CrewRow {
            person_id: r.person_id,
            person_name: r.person_name,
            department: r.department,
            job: r.job,
        })
        .collect())
}

/// Replace all cast for a movie (delete existing, insert new).
pub async fn replace_cast(pool: &sqlx::PgPool, movie_id: Uuid, cast: Vec<CastInput>) -> Result<()> {
    let mut tx = pool.begin().await?;

    sqlx::query!("DELETE FROM movie_cast WHERE movie_id = $1", movie_id)
        .execute(&mut *tx)
        .await?;

    for c in cast {
        sqlx::query!(
            r#"
            INSERT INTO movie_cast (movie_id, person_id, character_name, cast_order)
            VALUES ($1, $2, $3, $4)
            "#,
            movie_id,
            c.person_id,
            c.character_name.as_deref(),
            c.cast_order,
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Replace all crew for a movie (delete existing, insert new).
pub async fn replace_crew(pool: &sqlx::PgPool, movie_id: Uuid, crew: Vec<CrewInput>) -> Result<()> {
    let mut tx = pool.begin().await?;

    sqlx::query!("DELETE FROM movie_crew WHERE movie_id = $1", movie_id)
        .execute(&mut *tx)
        .await?;

    for c in crew {
        sqlx::query!(
            r#"
            INSERT INTO movie_crew (movie_id, person_id, department, job)
            VALUES ($1, $2, $3, $4)
            "#,
            movie_id,
            c.person_id,
            &c.department,
            &c.job,
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

// =============================================================================
// Lookup by identifier
// =============================================================================

// =============================================================================
// First By Letter (Alphabet Bar Jump Navigation)
// =============================================================================

/// Result from first_by_letter query - includes title for cursor creation.
#[derive(Debug)]
pub struct FirstByLetterResult {
    pub id: Uuid,
    pub title: String,
}

/// Find the first movie starting with a given letter.
/// Used for alphabet bar jump navigation in the library UI.
///
/// # Arguments
/// * `letter` - The letter to search for (A-Z, or "#" for non-alphabetic)
pub async fn first_by_letter<'e, E>(exec: E, letter: &str) -> Result<Option<FirstByLetterResult>>
where
    E: Executor<'e, Database = Postgres>,
{
    let letter_condition = if letter == "#" {
        "COALESCE(sort_title, title) !~ '^[A-Za-z]'"
    } else {
        "UPPER(LEFT(COALESCE(sort_title, title), 1)) = $1"
    };

    let mut query = sqlx::query(sqlx::AssertSqlSafe(format!(
        r#"
        SELECT id, title
        FROM movies
        WHERE {letter_condition}
        ORDER BY COALESCE(sort_title, title) ASC
        LIMIT 1
        "#
    )));
    if letter != "#" {
        query = query.bind(letter.to_uppercase());
    }
    let row = query.fetch_optional(exec).await?;

    Ok(row.map(|r| FirstByLetterResult {
        id: r.get("id"),
        title: r.get("title"),
    }))
}

// =============================================================================
// Lookup by identifier
// =============================================================================

/// Find a movie by a specific identifier key/value (e.g. "tmdb_id" = "12345").
pub async fn get_by_identifier<'e, E>(exec: E, key: &str, value: &str) -> Result<Option<MovieRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT id, title, sort_title, year, images, identifiers,
               EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = movies.id) AS is_in_library,
               want_status, release_date, quality_profile_id,
               preferred_languages, alternate_titles, media_root_id, streamed_at, created_at, updated_at
        FROM movies
        WHERE identifiers->>$1 = $2
        "#,
    )
    .bind(key)
    .bind(value)
    .fetch_optional(exec)
    .await?;

    Ok(row.map(row_to_movie))
}

// =============================================================================
// Rejected Releases
// =============================================================================

/// Append a rejected release entry to a movie's rejected_releases array.
pub async fn add_rejected_release<'e, E>(
    exec: E,
    id: Uuid,
    release_info: serde_json::Value,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE movies SET rejected_releases = rejected_releases || $2::jsonb, updated_at = NOW() WHERE id = $1",
        id,
        serde_json::json!([release_info]),
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Clear all rejected releases for a movie.
pub async fn clear_rejected_releases<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE movies SET rejected_releases = '[]'::jsonb, updated_at = NOW() WHERE id = $1",
        id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Get rejected releases for a movie.
pub async fn get_rejected_releases<'e, E>(exec: E, id: Uuid) -> Result<serde_json::Value>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!("SELECT rejected_releases FROM movies WHERE id = $1", id,)
        .fetch_optional(exec)
        .await?;

    Ok(row.unwrap_or(serde_json::json!([])))
}
