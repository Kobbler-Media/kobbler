//! TV show repository operations.

use super::tv_episodes::EpisodeRow;
use super::{PaginationDirection, Result, normalize_rating, normalize_show_status};
use crate::images::ImageMap;
use serde_json::Value;
use sqlx::{Executor, PgPool, Postgres, Row};
use time::OffsetDateTime;
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

/// Input for creating a TV show.
#[derive(Debug, Default)]
pub struct TvShowInput {
    pub title: String,
    pub sort_title: Option<String>,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub status: Option<String>,
    pub description: Option<String>,
    pub images: Option<ImageMap>,
    pub identifiers: Option<Value>,
    pub genres: Option<Vec<String>>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub content_rating: Option<String>,
    pub network: Option<String>,
    pub is_monitored: Option<bool>,
    pub auto_want_mode: Option<String>,
    pub quality_profile_id: Option<Uuid>,
    pub preferred_languages: Option<Vec<String>>,
    pub media_root_id: Option<Uuid>,
    pub first_aired: Option<time::Date>,
    pub last_aired: Option<time::Date>,
    pub next_airing: Option<time::Date>,
    pub runtime: Option<i32>,
    pub total_episode_count: Option<i32>,
    /// Discriminator: "tv" (default) or "sport".
    pub content_category: Option<String>,
    /// Sport-specific metadata (league_id, session_types, etc.).
    pub sport_metadata: Option<Value>,
    pub streamed_at: Option<time::OffsetDateTime>,
}

/// Input for updating a TV show. All fields optional.
#[derive(Debug, Default)]
pub struct TvShowUpdate {
    pub title: Option<String>,
    pub sort_title: Option<String>,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub status: Option<String>,
    pub description: Option<String>,
    pub images: Option<ImageMap>,
    pub identifiers: Option<Value>,
    pub genres: Option<Vec<String>>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub content_rating: Option<String>,
    pub network: Option<String>,
    pub first_aired: Option<time::Date>,
    pub last_aired: Option<time::Date>,
    pub next_airing: Option<time::Date>,
    pub runtime: Option<i32>,
    pub total_episode_count: Option<i32>,
    pub alternate_titles: Option<Vec<String>>,
    pub quality_profile_id: Option<Option<Uuid>>,
    pub preferred_languages: Option<Option<Vec<String>>>,
    pub media_root_id: Option<Option<Uuid>>,
    /// Sport-specific metadata (league_id, session_types, etc.).
    pub sport_metadata: Option<Value>,
    pub streamed_at: Option<time::OffsetDateTime>,
}

/// Lightweight TV show row for listings.
#[derive(Debug)]
pub struct TvShowRow {
    pub id: Uuid,
    pub title: String,
    pub sort_title: Option<String>,
    pub year: Option<i32>,
    pub images: ImageMap,
    pub identifiers: Value,
    pub status: Option<String>,
    pub network: Option<String>,
    pub is_monitored: bool,
    pub auto_want_mode: String,
    pub quality_profile_id: Option<Uuid>,
    pub preferred_languages: Option<Vec<String>>,
    pub media_root_id: Option<Uuid>,
    pub alternate_titles: Option<Vec<String>>,
    pub content_category: String,
    pub sport_metadata: Option<Value>,
    pub streamed_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    /// Count of episodes that have aired (air_date <= today, excluding specials).
    pub aired_episode_count: i64,
    /// Count of episodes in the local library (excluding specials).
    pub in_library_count: i64,
}

/// Full TV show detail with seasons, episodes, cast, and crew.
#[derive(Debug)]
pub struct TvShowDetail {
    pub id: Uuid,
    pub title: String,
    pub sort_title: Option<String>,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub status: Option<String>,
    pub description: Option<String>,
    pub images: ImageMap,
    pub identifiers: Value,
    pub genres: Option<Vec<String>>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
    pub content_rating: Option<String>,
    pub network: Option<String>,
    pub is_monitored: bool,
    pub auto_want_mode: String,
    pub quality_profile_id: Option<Uuid>,
    pub preferred_languages: Option<Vec<String>>,
    pub media_root_id: Option<Uuid>,
    pub first_aired: Option<time::Date>,
    pub last_aired: Option<time::Date>,
    pub next_airing: Option<time::Date>,
    pub runtime: Option<i32>,
    pub total_episode_count: Option<i32>,
    pub content_category: String,
    pub sport_metadata: Option<Value>,
    pub streamed_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub seasons: Vec<SeasonWithEpisodes>,
    pub cast: Vec<CastRow>,
    pub crew: Vec<CrewRow>,
}

/// Season with its nested episodes.
#[derive(Debug)]
pub struct SeasonWithEpisodes {
    pub id: Uuid,
    pub season_identifier: String,
    pub season_sort_order: i32,
    pub title: Option<String>,
    pub description: Option<String>,
    pub images: ImageMap,
    pub air_date: Option<time::Date>,
    pub episode_count: Option<i32>,
    pub is_monitored: bool,
    pub is_special: bool,
    pub auto_want_mode: String,
    pub created_at: OffsetDateTime,
    pub episodes: Vec<EpisodeRow>,
}

/// Filters for TV show listing.
#[derive(Debug, Default)]
pub struct TvShowListFilters {
    pub status: Option<String>,
    pub monitored: Option<bool>,
    pub search: Option<String>,
    pub limit: i64,
    pub cursor_title: Option<String>,
    pub cursor_id: Option<Uuid>,
    pub direction: PaginationDirection,
    /// Include the cursor row itself in the result instead of paging past it.
    pub cursor_inclusive: bool,
    /// Filter by content category: "tv" or "sport".
    pub content_category: Option<String>,
    /// "title" (default) or "created_at".
    pub sort: Option<String>,
}

/// Lightweight struct for monitor job.
#[derive(Debug)]
pub struct MonitoredShow {
    pub id: Uuid,
    pub title: String,
    pub identifiers: Value,
    pub auto_want_mode: String,
    pub content_category: String,
    pub sport_metadata: Option<Value>,
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

fn row_to_tv_show(r: sqlx::postgres::PgRow) -> TvShowRow {
    let images_json: Value = r.get("images");
    let images: ImageMap = serde_json::from_value(images_json).unwrap_or_default();
    TvShowRow {
        id: r.get("id"),
        title: r.get("title"),
        sort_title: r.get("sort_title"),
        year: r.get("year"),
        images,
        identifiers: r.get("identifiers"),
        status: r.get("status"),
        network: r.get("network"),
        is_monitored: r.get("is_monitored"),
        auto_want_mode: r.get("auto_want_mode"),
        quality_profile_id: r.get("quality_profile_id"),
        preferred_languages: r.get("preferred_languages"),
        alternate_titles: r.get("alternate_titles"),
        media_root_id: r.get("media_root_id"),
        content_category: r.get("content_category"),
        sport_metadata: r.get("sport_metadata"),
        streamed_at: r.get("streamed_at"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
        aired_episode_count: r.get("aired_episode_count"),
        in_library_count: r.get("in_library_count"),
    }
}

// =============================================================================
// Create
// =============================================================================

/// Create a new TV show.
pub async fn create<'e, E>(exec: E, input: TvShowInput) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let identifiers = input.identifiers.unwrap_or_else(|| serde_json::json!({}));
    let is_monitored = input.is_monitored.unwrap_or(false);
    let auto_want_mode = input.auto_want_mode.as_deref().unwrap_or("none");
    let images = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();
    let rating = input.rating.map(normalize_rating);

    let content_category = input.content_category.as_deref().unwrap_or("tv");
    let sport_metadata = input.sport_metadata.as_ref();
    let status = normalize_show_status(input.status);

    let row = sqlx::query!(
        r#"
        INSERT INTO tv_shows (id, title, sort_title, original_title, year, status,
                              description, images, identifiers,
                              genres, rating, rating_count, content_rating, network,
                              is_monitored, auto_want_mode, quality_profile_id, preferred_languages,
                              media_root_id,
                              first_aired, last_aired, next_airing, runtime, total_episode_count,
                              content_category, sport_metadata, streamed_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27)
        RETURNING id
        "#,
        id,
        input.title,
        input.sort_title.as_deref(),
        input.original_title.as_deref(),
        input.year,
        status.as_deref(),
        input.description.as_deref(),
        &images,
        &identifiers,
        input.genres.as_deref(),
        rating,
        input.rating_count,
        input.content_rating.as_deref(),
        input.network.as_deref(),
        is_monitored,
        auto_want_mode,
        input.quality_profile_id,
        input.preferred_languages.as_deref(),
        input.media_root_id,
        input.first_aired,
        input.last_aired,
        input.next_airing,
        input.runtime,
        input.total_episode_count,
        content_category,
        sport_metadata,
        input.streamed_at,
    )
    .fetch_one(exec)
    .await?;

    Ok(row.id)
}

// =============================================================================
// Read
// =============================================================================

/// Get a TV show by ID (lightweight row).
pub async fn get<'e, E>(exec: E, id: Uuid) -> Result<Option<TvShowRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT tv_shows.id, tv_shows.title, tv_shows.sort_title, tv_shows.year,
               tv_shows.images, tv_shows.identifiers,
               tv_shows.status, tv_shows.network, tv_shows.is_monitored, tv_shows.auto_want_mode,
               tv_shows.quality_profile_id, tv_shows.preferred_languages, tv_shows.alternate_titles,
               tv_shows.media_root_id,
               tv_shows.content_category, tv_shows.sport_metadata,
               tv_shows.streamed_at,
               tv_shows.created_at, tv_shows.updated_at,
               COALESCE(ep_counts.aired_count, 0) AS aired_episode_count,
               COALESCE(ep_counts.in_library_count, 0) AS in_library_count
        FROM tv_shows
        LEFT JOIN LATERAL (
            SELECT
                COUNT(*) FILTER (WHERE e.air_date IS NOT NULL AND e.air_date <= CURRENT_DATE) AS aired_count,
                COUNT(*) FILTER (WHERE EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = e.id)) AS in_library_count
            FROM tv_episodes e
            JOIN tv_seasons s ON s.id = e.season_id
            WHERE s.show_id = tv_shows.id AND s.is_special = FALSE
        ) ep_counts ON TRUE
        WHERE tv_shows.id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(exec)
    .await?;

    Ok(row.map(row_to_tv_show))
}

/// Get a TV show with full details including seasons, episodes, cast, and crew.
pub async fn get_with_details(pool: &sqlx::PgPool, id: Uuid) -> Result<Option<TvShowDetail>> {
    let row = sqlx::query!(
        r#"
        SELECT id, title, sort_title, original_title, year, status,
               description, images, identifiers,
               genres, rating, rating_count, content_rating, network,
               is_monitored, auto_want_mode, quality_profile_id, preferred_languages, media_root_id,
               first_aired, last_aired, next_airing, runtime, total_episode_count,
               content_category, sport_metadata, streamed_at,
               created_at, updated_at
        FROM tv_shows
        WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(pool)
    .await?;

    let Some(r) = row else {
        return Ok(None);
    };

    let season_rows = sqlx::query!(
        r#"
        SELECT id, show_id, season_identifier, season_sort_order,
               title, description, images, air_date,
               episode_count, is_monitored, is_special, auto_want_mode, created_at
        FROM tv_seasons
        WHERE show_id = $1
        ORDER BY season_sort_order ASC
        "#,
        id,
    )
    .fetch_all(pool)
    .await?;

    let mut seasons = Vec::with_capacity(season_rows.len());
    for sr in season_rows {
        let season_id = sr.id;

        let episode_rows = sqlx::query!(
            r#"
            SELECT id, season_id, episode_identifier, episode_sort_order,
                   title, description, images, air_date, air_time, runtime,
                   EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = tv_episodes.id) AS "is_in_library!",
                   want_status, identifiers, country, created_at, updated_at
            FROM tv_episodes
            WHERE season_id = $1
            ORDER BY episode_sort_order ASC
            "#,
            season_id,
        )
        .fetch_all(pool)
        .await?;

        let episodes = episode_rows
            .into_iter()
            .map(|er| {
                let images: ImageMap =
                    serde_json::from_value(er.images.clone()).unwrap_or_default();
                EpisodeRow {
                    id: er.id,
                    season_id: er.season_id,
                    episode_identifier: er.episode_identifier,
                    episode_sort_order: er.episode_sort_order,
                    title: er.title,
                    description: er.description,
                    images,
                    air_date: er.air_date,
                    air_time: er.air_time,
                    runtime: er.runtime,
                    is_in_library: er.is_in_library,
                    want_status: er.want_status,
                    identifiers: er.identifiers,
                    country: er.country,
                    created_at: er.created_at,
                    updated_at: er.updated_at,
                }
            })
            .collect();

        let season_images: ImageMap = serde_json::from_value(sr.images.clone()).unwrap_or_default();
        seasons.push(SeasonWithEpisodes {
            id: season_id,
            season_identifier: sr.season_identifier,
            season_sort_order: sr.season_sort_order,
            title: sr.title,
            description: sr.description,
            images: season_images,
            air_date: sr.air_date,
            episode_count: sr.episode_count,
            is_monitored: sr.is_monitored,
            is_special: sr.is_special,
            auto_want_mode: sr.auto_want_mode,
            created_at: sr.created_at,
            episodes,
        });
    }

    let cast = get_cast(pool, id).await?;
    let crew = get_crew(pool, id).await?;

    let detail_images: ImageMap = serde_json::from_value(r.images.clone()).unwrap_or_default();
    Ok(Some(TvShowDetail {
        id: r.id,
        title: r.title,
        sort_title: r.sort_title,
        original_title: r.original_title,
        year: r.year,
        status: r.status,
        description: r.description,
        images: detail_images,
        identifiers: r.identifiers.clone(),
        genres: r.genres,
        rating: r.rating,
        rating_count: r.rating_count,
        content_rating: r.content_rating,
        network: r.network,
        is_monitored: r.is_monitored,
        auto_want_mode: r.auto_want_mode,
        quality_profile_id: r.quality_profile_id,
        preferred_languages: r.preferred_languages,
        media_root_id: r.media_root_id,
        first_aired: r.first_aired,
        last_aired: r.last_aired,
        next_airing: r.next_airing,
        runtime: r.runtime,
        total_episode_count: r.total_episode_count,
        content_category: r.content_category,
        sport_metadata: r.sport_metadata,
        streamed_at: r.streamed_at,
        created_at: r.created_at,
        updated_at: r.updated_at,
        seasons,
        cast,
        crew,
    }))
}

#[derive(Debug, Default)]
pub struct EpisodeTallies {
    pub total: i64,
    pub in_library: i64,
    /// Wanted (or mid-acquisition) with no file yet.
    pub wanted: i64,
    /// Watched by the requesting user.
    pub watched: i64,
}

/// Episode counts across every season of a show, for pages that page through
/// seasons and so cannot total them client-side.
pub async fn episode_tallies<'e, E>(exec: E, show_id: Uuid, user_id: Uuid) -> Result<EpisodeTallies>
where
    E: Executor<'e, Database = Postgres>,
{
    let r = sqlx::query!(
        r#"
        SELECT COUNT(e.id) AS "total!",
               COUNT(e.id) FILTER (WHERE lib.in_library) AS "in_library!",
               COUNT(e.id) FILTER (WHERE e.want_status <> 'none' AND NOT lib.in_library) AS "wanted!",
               COUNT(e.id) FILTER (
                   WHERE EXISTS (SELECT 1 FROM tv_watch_states ws WHERE ws.episode_id = e.id AND ws.user_id = $2 AND ws.watched)
               ) AS "watched!"
        FROM tv_episodes e
        JOIN tv_seasons s ON s.id = e.season_id
        CROSS JOIN LATERAL (
            SELECT EXISTS (SELECT 1 FROM tv_episode_file_episodes efe WHERE efe.episode_id = e.id) AS in_library
        ) lib
        WHERE s.show_id = $1
        "#,
        show_id,
        user_id,
    )
    .fetch_one(exec)
    .await?;

    Ok(EpisodeTallies {
        total: r.total,
        in_library: r.in_library,
        wanted: r.wanted,
        watched: r.watched,
    })
}

/// Get a TV show's metadata only (no seasons, episodes, cast, crew).
/// Returns the same `TvShowDetail` struct but with empty nested collections.
pub async fn get_metadata(pool: &sqlx::PgPool, id: Uuid) -> Result<Option<TvShowDetail>> {
    let row = sqlx::query!(
        r#"
        SELECT id, title, sort_title, original_title, year, status,
               description, images, identifiers,
               genres, rating, rating_count, content_rating, network,
               is_monitored, auto_want_mode, quality_profile_id, preferred_languages, media_root_id,
               first_aired, last_aired, next_airing, runtime, total_episode_count,
               content_category, sport_metadata, streamed_at,
               created_at, updated_at
        FROM tv_shows
        WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(pool)
    .await?;

    let Some(r) = row else {
        return Ok(None);
    };

    let images: ImageMap = serde_json::from_value(r.images.clone()).unwrap_or_default();
    Ok(Some(TvShowDetail {
        id: r.id,
        title: r.title,
        sort_title: r.sort_title,
        original_title: r.original_title,
        year: r.year,
        status: r.status,
        description: r.description,
        images,
        identifiers: r.identifiers.clone(),
        genres: r.genres,
        rating: r.rating,
        rating_count: r.rating_count,
        content_rating: r.content_rating,
        network: r.network,
        is_monitored: r.is_monitored,
        auto_want_mode: r.auto_want_mode,
        quality_profile_id: r.quality_profile_id,
        preferred_languages: r.preferred_languages,
        media_root_id: r.media_root_id,
        first_aired: r.first_aired,
        last_aired: r.last_aired,
        next_airing: r.next_airing,
        runtime: r.runtime,
        total_episode_count: r.total_episode_count,
        content_category: r.content_category,
        sport_metadata: r.sport_metadata,
        streamed_at: r.streamed_at,
        created_at: r.created_at,
        updated_at: r.updated_at,
        seasons: Vec::new(),
        cast: Vec::new(),
        crew: Vec::new(),
    }))
}

/// List TV shows with optional filters and cursor-based pagination.
pub async fn list<'e, E>(exec: E, filters: &TvShowListFilters) -> Result<Vec<TvShowRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let mut sql = String::from(
        r#"
        SELECT tv_shows.id, tv_shows.title, tv_shows.sort_title, tv_shows.year,
               tv_shows.images, tv_shows.identifiers,
               tv_shows.status, tv_shows.network, tv_shows.is_monitored, tv_shows.auto_want_mode,
               tv_shows.quality_profile_id, tv_shows.preferred_languages, tv_shows.alternate_titles,
               tv_shows.media_root_id,
               tv_shows.content_category, tv_shows.sport_metadata,
               tv_shows.streamed_at,
               tv_shows.created_at, tv_shows.updated_at,
               COALESCE(ep_counts.aired_count, 0) AS aired_episode_count,
               COALESCE(ep_counts.in_library_count, 0) AS in_library_count
        FROM tv_shows
        LEFT JOIN LATERAL (
            SELECT
                COUNT(*) FILTER (WHERE e.air_date IS NOT NULL AND e.air_date <= CURRENT_DATE) AS aired_count,
                COUNT(*) FILTER (WHERE EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = e.id)) AS in_library_count
            FROM tv_episodes e
            JOIN tv_seasons s ON s.id = e.season_id
            WHERE s.show_id = tv_shows.id AND s.is_special = FALSE
        ) ep_counts ON TRUE
        WHERE TRUE
        "#,
    );
    let mut params: Vec<String> = Vec::new();

    if let Some(ref cat) = filters.content_category {
        params.push(cat.clone());
        let idx = params.len();
        sql.push_str(&format!(" AND content_category = ${}", idx));
    }

    if let Some(ref status) = filters.status {
        params.push(status.clone());
        let idx = params.len();
        sql.push_str(&format!(" AND status = ${}", idx));
    }

    if let Some(monitored) = filters.monitored {
        if monitored {
            sql.push_str(" AND is_monitored = TRUE");
        } else {
            sql.push_str(" AND is_monitored = FALSE");
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

    let sort_mode = filters.sort.as_deref().unwrap_or("title");
    let sort_by_title = sort_mode == "title";

    // Cursor-based pagination (only with title sort)
    if sort_by_title
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

    match sort_mode {
        "created_at" => {
            sql.push_str(" ORDER BY tv_shows.created_at DESC, tv_shows.id DESC");
        }
        "next_airing" => {
            sql.push_str(
                " ORDER BY tv_shows.next_airing ASC NULLS LAST, COALESCE(tv_shows.sort_title, tv_shows.title) ASC, tv_shows.id ASC",
            );
        }
        _ => match filters.direction {
            PaginationDirection::Forward => {
                sql.push_str(
                    " ORDER BY COALESCE(tv_shows.sort_title, tv_shows.title) ASC, tv_shows.id ASC",
                );
            }
            PaginationDirection::Backward => {
                sql.push_str(
                    " ORDER BY COALESCE(tv_shows.sort_title, tv_shows.title) DESC, tv_shows.id DESC",
                );
            }
        },
    }

    let limit_idx = params.len() + 1;
    sql.push_str(&format!(" LIMIT ${}", limit_idx));

    let mut q = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()));
    for param in &params {
        q = q.bind(param);
    }
    q = q.bind(filters.limit);

    let rows = q.fetch_all(exec).await?;
    let mut result: Vec<TvShowRow> = rows.into_iter().map(row_to_tv_show).collect();

    // Reverse backward results so they're in ascending order
    if filters.direction == PaginationDirection::Backward {
        result.reverse();
    }

    Ok(result)
}

/// Count TV shows matching filters (for total_count on first page).
pub async fn count<'e, E>(exec: E, filters: &TvShowListFilters) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let mut sql = String::from("SELECT COUNT(*) as count FROM tv_shows WHERE TRUE");
    let mut params: Vec<String> = Vec::new();

    if let Some(ref cat) = filters.content_category {
        params.push(cat.clone());
        let idx = params.len();
        sql.push_str(&format!(" AND content_category = ${}", idx));
    }

    if let Some(ref status) = filters.status {
        params.push(status.clone());
        let idx = params.len();
        sql.push_str(&format!(" AND status = ${}", idx));
    }

    if let Some(monitored) = filters.monitored {
        if monitored {
            sql.push_str(" AND is_monitored = TRUE");
        } else {
            sql.push_str(" AND is_monitored = FALSE");
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

/// List all monitored shows (for monitor job).
pub async fn list_monitored<'e, E>(exec: E) -> Result<Vec<MonitoredShow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT id, title, identifiers, auto_want_mode, content_category, sport_metadata
        FROM tv_shows
        WHERE is_monitored = TRUE
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| MonitoredShow {
            id: r.id,
            title: r.title,
            identifiers: r.identifiers.clone(),
            auto_want_mode: r.auto_want_mode,
            content_category: r.content_category,
            sport_metadata: r.sport_metadata,
        })
        .collect())
}

// =============================================================================
// Update
// =============================================================================

/// Update a TV show. Only non-None fields are updated.
/// Identifiers are replaced (not merged) if provided, preserving user edits.
pub async fn update<'e, E>(exec: E, id: Uuid, input: TvShowUpdate) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let images = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();
    let id_update = input.identifiers.is_some();
    let identifiers = input.identifiers.unwrap_or_default();
    let qp_update = input.quality_profile_id;
    let pl_update = input.preferred_languages.is_some();
    let pl_flattened = input.preferred_languages.flatten();
    let mr_update = input.media_root_id;
    let rating = input.rating.map(normalize_rating);
    let status = normalize_show_status(input.status);

    let result = sqlx::query!(
        r#"
        UPDATE tv_shows SET
            title = COALESCE($2, title),
            sort_title = COALESCE($3, sort_title),
            original_title = COALESCE($4, original_title),
            year = COALESCE($5, year),
            status = COALESCE($6, status),
            description = COALESCE($7, description),
            images = images || $8,
            identifiers = CASE WHEN $9 THEN $10 ELSE identifiers END,
            genres = COALESCE($11, genres),
            rating = COALESCE($12, rating),
            rating_count = COALESCE($13, rating_count),
            content_rating = COALESCE($14, content_rating),
            network = COALESCE($15, network),
            first_aired = COALESCE($16, first_aired),
            last_aired = COALESCE($17, last_aired),
            next_airing = COALESCE($18, next_airing),
            runtime = COALESCE($19, runtime),
            total_episode_count = COALESCE($20, total_episode_count),
            quality_profile_id = CASE WHEN $21 THEN $22 ELSE quality_profile_id END,
            preferred_languages = CASE WHEN $23 THEN $24::text[] ELSE preferred_languages END,
            media_root_id = CASE WHEN $25 THEN $26 ELSE media_root_id END,
            alternate_titles = COALESCE($27, alternate_titles),
            streamed_at = COALESCE($28, streamed_at),
            updated_at = NOW()
        WHERE id = $1
        "#,
        id,
        input.title.as_deref(),
        input.sort_title.as_deref(),
        input.original_title.as_deref(),
        input.year,
        status.as_deref(),
        input.description.as_deref(),
        &images,
        id_update,
        &identifiers,
        input.genres.as_deref(),
        rating,
        input.rating_count,
        input.content_rating.as_deref(),
        input.network.as_deref(),
        input.first_aired,
        input.last_aired,
        input.next_airing,
        input.runtime,
        input.total_episode_count,
        qp_update.is_some(),
        qp_update.flatten(),
        pl_update,
        pl_flattened.as_deref(),
        mr_update.is_some(),
        mr_update.flatten(),
        input.alternate_titles.as_deref(),
        input.streamed_at,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete a TV show by ID.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM tv_shows WHERE id = $1", id)
        .execute(exec)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete all TV shows belonging to a media root, returning deleted IDs.
pub async fn delete_by_media_root<'e, E>(exec: E, media_root_id: Uuid) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!(
        "DELETE FROM tv_shows WHERE media_root_id = $1 RETURNING id",
        media_root_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(ids)
}

/// Delete orphaned shows: not monitored or auto_want_mode='none', not
/// debrid-streamed, no seasons remain.
/// Returns IDs of deleted shows for search index cleanup.
pub async fn delete_orphaned<'e, E>(exec: E) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let ids = sqlx::query_scalar!(
        r#"
        DELETE FROM tv_shows
        WHERE (is_monitored = false OR auto_want_mode = 'none')
          AND streamed_at IS NULL
          AND NOT EXISTS (SELECT 1 FROM tv_seasons WHERE show_id = tv_shows.id)
        RETURNING id
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(ids)
}

/// Look up a TV show by an identifier key/value pair (e.g. tmdb_id, imdb_id).
pub async fn get_by_identifier<'e, E>(exec: E, key: &str, value: &str) -> Result<Option<TvShowRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT tv_shows.id, tv_shows.title, tv_shows.sort_title, tv_shows.year,
               tv_shows.images, tv_shows.identifiers,
               tv_shows.status, tv_shows.network, tv_shows.is_monitored, tv_shows.auto_want_mode,
               tv_shows.quality_profile_id, tv_shows.preferred_languages, tv_shows.alternate_titles,
               tv_shows.media_root_id,
               tv_shows.content_category, tv_shows.sport_metadata,
               tv_shows.streamed_at,
               tv_shows.created_at, tv_shows.updated_at,
               COALESCE(ep_counts.aired_count, 0) AS aired_episode_count,
               COALESCE(ep_counts.in_library_count, 0) AS in_library_count
        FROM tv_shows
        LEFT JOIN LATERAL (
            SELECT
                COUNT(*) FILTER (WHERE e.air_date IS NOT NULL AND e.air_date <= CURRENT_DATE) AS aired_count,
                COUNT(*) FILTER (WHERE EXISTS (SELECT 1 FROM tv_episode_file_episodes efe JOIN tv_episode_files ef ON ef.id = efe.episode_file_id WHERE efe.episode_id = e.id)) AS in_library_count
            FROM tv_episodes e
            JOIN tv_seasons s ON s.id = e.season_id
            WHERE s.show_id = tv_shows.id AND s.is_special = FALSE
        ) ep_counts ON TRUE
        WHERE tv_shows.identifiers->>$1 = $2
        "#,
    )
    .bind(key)
    .bind(value)
    .fetch_optional(exec)
    .await?;

    Ok(row.map(row_to_tv_show))
}

/// Set the auto_want_mode for a show without touching its monitored flag.
pub async fn set_auto_want_mode(pool: &PgPool, id: Uuid, mode: &str) -> Result<bool> {
    let result = sqlx::query!(
        "UPDATE tv_shows SET auto_want_mode = $2, updated_at = NOW() WHERE id = $1",
        id,
        mode,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Set a show's monitored status and auto_want_mode, cascading is_monitored
/// to all of the show's seasons.
pub async fn set_monitored(
    pool: &PgPool,
    id: Uuid,
    monitored: bool,
    auto_want_mode: &str,
) -> Result<bool> {
    let result = sqlx::query!(
        "UPDATE tv_shows SET is_monitored = $2, auto_want_mode = $3, updated_at = NOW() WHERE id = $1",
        id,
        monitored,
        auto_want_mode,
    )
    .execute(pool)
    .await?;

    sqlx::query!(
        "UPDATE tv_seasons SET is_monitored = $1 WHERE show_id = $2",
        monitored,
        id,
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Bulk-update shows. Only provided fields are changed; others keep existing values.
/// When `auto_want_mode` is set to anything other than "none", `is_monitored` is
/// automatically set to true.
pub async fn bulk_update(
    pool: &PgPool,
    ids: &[Uuid],
    auto_want_mode: Option<&str>,
    quality_profile_id: Option<Option<Uuid>>,
) -> Result<u64> {
    let mut sql = String::from("UPDATE tv_shows SET updated_at = NOW()");

    if let Some(mode) = auto_want_mode {
        sql.push_str(&format!(
            ", auto_want_mode = '{}'",
            mode.replace('\'', "''")
        ));
        if mode != "none" {
            sql.push_str(", is_monitored = TRUE");
        }
    }

    match quality_profile_id {
        Some(Some(qp_id)) => {
            sql.push_str(&format!(" , quality_profile_id = '{qp_id}'"));
        }
        Some(None) => {
            sql.push_str(" , quality_profile_id = NULL");
        }
        None => {}
    }

    sql.push_str(" WHERE id = ANY($1)");

    let result = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(ids)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

/// Upsert a TV show by a provider-specific identifier.
///
/// Tries an atomic `ON CONFLICT` upsert first. If the provider lacks a unique
/// index, falls back to an advisory-locked SELECT + INSERT/UPDATE to prevent
/// races from concurrent async inserts.
pub async fn upsert_by_provider(
    pool: &PgPool,
    provider_type: &str,
    provider_id: &str,
    input: TvShowInput,
) -> Result<Uuid> {
    let id_key = format!("{provider_type}_id");
    let mut identifiers = input.identifiers.unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = identifiers.as_object_mut() {
        obj.insert(id_key.clone(), Value::String(provider_id.to_string()));
    }
    let is_monitored = input.is_monitored.unwrap_or(false);
    let auto_want_mode = input.auto_want_mode.as_deref().unwrap_or("none");
    let images = serde_json::to_value(input.images.unwrap_or_default()).unwrap_or_default();
    let rating = input.rating.map(normalize_rating);
    let content_category = input.content_category.as_deref().unwrap_or("tv");
    let sport_metadata = input.sport_metadata.as_ref();
    let status = normalize_show_status(input.status);

    // Try atomic ON CONFLICT first (works when a unique index exists)
    let id = Uuid::now_v7();
    let upsert_sql = format!(
        r#"
        INSERT INTO tv_shows (id, title, sort_title, original_title, year, status,
                              description, images, identifiers,
                              genres, rating, rating_count, content_rating, network,
                              is_monitored, auto_want_mode, quality_profile_id, preferred_languages,
                              media_root_id,
                              first_aired, last_aired, next_airing, runtime, total_episode_count,
                              content_category, sport_metadata)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26)
        ON CONFLICT ((identifiers->>'{id_key}'))
        DO UPDATE SET
            title = EXCLUDED.title,
            -- Existing value wins: it is either a hand-entered override or one the
            -- sort_title trigger derived, and the trigger re-derives it from the new
            -- title on this same UPDATE when it was not overridden.
            sort_title = COALESCE(tv_shows.sort_title, EXCLUDED.sort_title),
            original_title = COALESCE(EXCLUDED.original_title, tv_shows.original_title),
            year = COALESCE(EXCLUDED.year, tv_shows.year),
            status = COALESCE(EXCLUDED.status, tv_shows.status),
            description = COALESCE(EXCLUDED.description, tv_shows.description),
            images = tv_shows.images || EXCLUDED.images,
            identifiers = tv_shows.identifiers || EXCLUDED.identifiers,
            genres = COALESCE(EXCLUDED.genres, tv_shows.genres),
            rating = COALESCE(EXCLUDED.rating, tv_shows.rating),
            rating_count = COALESCE(EXCLUDED.rating_count, tv_shows.rating_count),
            content_rating = COALESCE(EXCLUDED.content_rating, tv_shows.content_rating),
            network = COALESCE(EXCLUDED.network, tv_shows.network),
            first_aired = COALESCE(EXCLUDED.first_aired, tv_shows.first_aired),
            last_aired = COALESCE(EXCLUDED.last_aired, tv_shows.last_aired),
            next_airing = COALESCE(EXCLUDED.next_airing, tv_shows.next_airing),
            runtime = COALESCE(EXCLUDED.runtime, tv_shows.runtime),
            total_episode_count = COALESCE(EXCLUDED.total_episode_count, tv_shows.total_episode_count),
            quality_profile_id = COALESCE(EXCLUDED.quality_profile_id, tv_shows.quality_profile_id),
            preferred_languages = COALESCE(EXCLUDED.preferred_languages, tv_shows.preferred_languages),
            media_root_id = COALESCE(EXCLUDED.media_root_id, tv_shows.media_root_id),
            content_category = EXCLUDED.content_category,
            sport_metadata = COALESCE(EXCLUDED.sport_metadata, tv_shows.sport_metadata),
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
        .bind(status.as_deref())
        .bind(input.description.as_deref())
        .bind(&images)
        .bind(&identifiers)
        .bind(input.genres.as_deref())
        .bind(rating)
        .bind(input.rating_count)
        .bind(input.content_rating.as_deref())
        .bind(input.network.as_deref())
        .bind(is_monitored)
        .bind(auto_want_mode)
        .bind(input.quality_profile_id)
        .bind(input.preferred_languages.as_deref())
        .bind(input.media_root_id)
        .bind(input.first_aired)
        .bind(input.last_aired)
        .bind(input.next_airing)
        .bind(input.runtime)
        .bind(input.total_episode_count)
        .bind(content_category)
        .bind(sport_metadata)
        .fetch_one(pool)
        .await;

    match result {
        Ok((row_id,)) => return Ok(row_id),
        Err(ref e) if super::is_no_unique_constraint_error(e) => {}
        Err(e) => return Err(e.into()),
    }

    // Fallback: advisory-locked SELECT + INSERT/UPDATE for providers without a unique index
    let lock_key = super::advisory_lock_key("tv_shows", &id_key, provider_id);
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(lock_key)
        .execute(&mut *tx)
        .await?;

    let existing =
        super::resolve_provider_row(&mut tx, "tv_shows", &id_key, provider_id, &identifiers)
            .await?;

    let result_id = if let Some(existing_id) = existing {
        sqlx::query(
            r#"
            UPDATE tv_shows SET
                title = $2,
                sort_title = COALESCE($3, sort_title),
                original_title = COALESCE($4, original_title),
                year = COALESCE($5, year),
                status = COALESCE($6, status),
                description = COALESCE($7, description),
                images = images || $8,
                identifiers = identifiers || $9,
                genres = COALESCE($10, genres),
                rating = COALESCE($11, rating),
                rating_count = COALESCE($12, rating_count),
                content_rating = COALESCE($13, content_rating),
                network = COALESCE($14, network),
                first_aired = COALESCE($15, first_aired),
                last_aired = COALESCE($16, last_aired),
                next_airing = COALESCE($17, next_airing),
                runtime = COALESCE($18, runtime),
                total_episode_count = COALESCE($19, total_episode_count),
                quality_profile_id = COALESCE($20, quality_profile_id),
                preferred_languages = COALESCE($21, preferred_languages),
                media_root_id = COALESCE($22, media_root_id),
                content_category = $23,
                sport_metadata = COALESCE($24, sport_metadata),
                updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(existing_id)
        .bind(&input.title)
        .bind(input.sort_title.as_deref())
        .bind(input.original_title.as_deref())
        .bind(input.year)
        .bind(status.as_deref())
        .bind(input.description.as_deref())
        .bind(&images)
        .bind(&identifiers)
        .bind(input.genres.as_deref())
        .bind(rating)
        .bind(input.rating_count)
        .bind(input.content_rating.as_deref())
        .bind(input.network.as_deref())
        .bind(input.first_aired)
        .bind(input.last_aired)
        .bind(input.next_airing)
        .bind(input.runtime)
        .bind(input.total_episode_count)
        .bind(input.quality_profile_id)
        .bind(input.preferred_languages.as_deref())
        .bind(input.media_root_id)
        .bind(content_category)
        .bind(sport_metadata)
        .execute(&mut *tx)
        .await?;
        existing_id
    } else {
        let new_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO tv_shows (id, title, sort_title, original_title, year, status,
                                  description, images, identifiers,
                                  genres, rating, rating_count, content_rating, network,
                                  is_monitored, auto_want_mode, quality_profile_id, preferred_languages,
                                  media_root_id,
                                  first_aired, last_aired, next_airing, runtime, total_episode_count,
                                  content_category, sport_metadata)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                    $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26)
            "#,
        )
        .bind(new_id)
        .bind(&input.title)
        .bind(input.sort_title.as_deref())
        .bind(input.original_title.as_deref())
        .bind(input.year)
        .bind(status.as_deref())
        .bind(input.description.as_deref())
        .bind(&images)
        .bind(&identifiers)
        .bind(input.genres.as_deref())
        .bind(rating)
        .bind(input.rating_count)
        .bind(input.content_rating.as_deref())
        .bind(input.network.as_deref())
        .bind(is_monitored)
        .bind(auto_want_mode)
        .bind(input.quality_profile_id)
        .bind(input.preferred_languages.as_deref())
        .bind(input.media_root_id)
        .bind(input.first_aired)
        .bind(input.last_aired)
        .bind(input.next_airing)
        .bind(input.runtime)
        .bind(input.total_episode_count)
        .bind(content_category)
        .bind(sport_metadata)
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

/// Get cast for a TV show, joined with people.
pub async fn get_cast<'e, E>(exec: E, show_id: Uuid) -> Result<Vec<CastRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT tc.id, tc.person_id, p.name AS person_name,
               CASE WHEN p.images->'profile' IS NOT NULL
                    AND (p.images->'profile'->>'source_url' IS NOT NULL OR p.images->'profile'->>'cached_at' IS NOT NULL)
                    THEN '/api/v1/admin/people/' || p.id || '/image'
                    ELSE NULL
               END AS person_image_url,
               tc.character_name, tc.cast_order
        FROM tv_cast tc
        JOIN people p ON p.id = tc.person_id
        WHERE tc.show_id = $1
        ORDER BY tc.cast_order ASC NULLS LAST
        "#,
        show_id,
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

/// Get crew for a TV show, joined with people.
async fn get_crew<'e, E>(exec: E, show_id: Uuid) -> Result<Vec<CrewRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT tc.person_id, p.name AS person_name, tc.department, tc.job
        FROM tv_crew tc
        JOIN people p ON p.id = tc.person_id
        WHERE tc.show_id = $1
        ORDER BY tc.department ASC, tc.job ASC
        "#,
        show_id,
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

/// Replace all cast for a TV show (delete existing, insert new).
pub async fn replace_cast(pool: &sqlx::PgPool, show_id: Uuid, cast: Vec<CastInput>) -> Result<()> {
    let mut tx = pool.begin().await?;

    sqlx::query!("DELETE FROM tv_cast WHERE show_id = $1", show_id)
        .execute(&mut *tx)
        .await?;

    for c in cast {
        let cast_id = Uuid::now_v7();
        sqlx::query!(
            r#"
            INSERT INTO tv_cast (id, show_id, person_id, character_name, cast_order)
            VALUES ($1, $2, $3, $4, $5)
            "#,
            cast_id,
            show_id,
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

/// Replace all crew for a TV show (delete existing, insert new).
pub async fn replace_crew(pool: &sqlx::PgPool, show_id: Uuid, crew: Vec<CrewInput>) -> Result<()> {
    let mut tx = pool.begin().await?;

    sqlx::query!("DELETE FROM tv_crew WHERE show_id = $1", show_id)
        .execute(&mut *tx)
        .await?;

    for c in crew {
        sqlx::query!(
            r#"
            INSERT INTO tv_crew (show_id, person_id, department, job)
            VALUES ($1, $2, $3, $4)
            "#,
            show_id,
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
// First By Letter (Alphabet Bar Jump Navigation)
// =============================================================================

/// Result from first_by_letter query - includes title for cursor creation.
#[derive(Debug)]
pub struct FirstByLetterResult {
    pub id: Uuid,
    pub title: String,
}

/// Find the first TV show starting with a given letter.
/// Used for alphabet bar jump navigation in the library UI.
///
/// # Arguments
/// * `letter` - The letter to search for (A-Z, or "#" for non-alphabetic)
/// * `content_category` - Optional filter by content category ("tv" or "sport")
pub async fn first_by_letter<'e, E>(
    exec: E,
    letter: &str,
    content_category: Option<&str>,
) -> Result<Option<FirstByLetterResult>>
where
    E: Executor<'e, Database = Postgres>,
{
    let mut next_param = 1;
    let letter_condition = if letter == "#" {
        "COALESCE(sort_title, title) !~ '^[A-Za-z]'".to_string()
    } else {
        next_param += 1;
        "UPPER(LEFT(COALESCE(sort_title, title), 1)) = $1".to_string()
    };

    let category_condition = match content_category {
        Some(_) => format!(" AND content_category = ${next_param}"),
        None => String::new(),
    };

    let mut query = sqlx::query(sqlx::AssertSqlSafe(format!(
        r#"
        SELECT id, title
        FROM tv_shows
        WHERE {letter_condition}{category_condition}
        ORDER BY COALESCE(sort_title, title) ASC
        LIMIT 1
        "#
    )));
    if letter != "#" {
        query = query.bind(letter.to_uppercase());
    }
    if let Some(cat) = content_category {
        query = query.bind(cat.to_string());
    }
    let row = query.fetch_optional(exec).await?;

    Ok(row.map(|r| FirstByLetterResult {
        id: r.get("id"),
        title: r.get("title"),
    }))
}
