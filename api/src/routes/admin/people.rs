//! People admin route handlers.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Serialize;
use serde_json::json;
use std::collections::HashSet;
use uuid::Uuid;

use serde::Deserialize;

use crate::AppState;
use crate::images::{self, ImageEntry, endpoint_url, source_url};
use crate::repositories::{PaginationDirection, people};
use crate::routes::AuthUser;
use crate::routes::library::helpers::{NameCursor, decode_name_cursor, encode_name_cursor};
use crate::routes::library::types::PaginatedResponse;
use crate::search;
use crate::video_metadata::VideoProviderRegistry;

// =============================================================================
// Error Type
// =============================================================================

pub enum AppError {
    Forbidden(String),
    NotFound(String),
    BadRequest(String),
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            AppError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

// =============================================================================
// List People
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ListPeopleQuery {
    pub q: Option<String>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
    pub direction: Option<String>,
    pub search_offset: Option<usize>,
}

#[derive(Serialize)]
struct PersonListResponse {
    id: Uuid,
    name: String,
    sort_name: Option<String>,
    image_url: Option<String>,
}

fn person_to_list_response(p: &people::PersonRow) -> PersonListResponse {
    let image_url = endpoint_url(
        &p.images,
        "profile",
        "/api/v1/admin/people/",
        p.id,
        "/image",
    );
    PersonListResponse {
        id: p.id,
        name: p.name.clone(),
        sort_name: p.sort_name.clone(),
        image_url,
    }
}

/// GET /api/v1/admin/people
pub async fn list_people(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(params): Query<ListPeopleQuery>,
) -> Result<impl IntoResponse, AppError> {
    let limit = params.limit.unwrap_or(50).min(200);
    let search_offset = params.search_offset.unwrap_or(0);
    let search_limit = limit as usize + 1;

    let is_backward = params
        .direction
        .as_deref()
        .map(|d| d.eq_ignore_ascii_case("backward"))
        .unwrap_or(false);
    let direction = if is_backward {
        PaginationDirection::Backward
    } else {
        PaginationDirection::Forward
    };

    let is_search = params.q.as_ref().is_some_and(|q| !q.trim().is_empty());
    let mut search_has_more = false;

    // Determine search_ids vs ILIKE fallback
    let (search_ids, search_text) = if let Some(ref q) = params.q {
        if !q.trim().is_empty() {
            match state
                .search
                .search_people(q, search_limit, search_offset)
                .await
            {
                Ok(mut ids) => {
                    search_has_more = ids.len() > limit as usize;
                    ids.truncate(limit as usize);
                    (Some(ids), None)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "People search backend failed, falling back to ILIKE");
                    (None, Some(q.clone()))
                }
            }
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let cursor = params.cursor.as_deref().and_then(decode_name_cursor);

    let filter = people::PersonListFilter {
        cursor: cursor.map(|c| (c.name, c.id)),
        direction,
        search: search_text,
        search_ids,
        limit: limit + 1,
    };

    // Only count on first page
    let total_count = if params.cursor.is_none() {
        people::count(&state.pool, params.q.as_deref()).await.ok()
    } else {
        None
    };

    let rows = people::list_with_cursor(&state.pool, &filter)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to list people: {e}")))?;

    let has_more = if is_search {
        search_has_more
    } else {
        rows.len() as i64 > limit
    };
    let rows: Vec<_> = rows.into_iter().take(limit as usize).collect();

    let rows: Vec<_> = if is_backward {
        rows.into_iter().rev().collect()
    } else {
        rows
    };

    let items: Vec<PersonListResponse> = rows.iter().map(person_to_list_response).collect();

    let (next_cursor, prev_cursor) = if is_search {
        let next = if has_more {
            Some(format!("search_offset:{}", search_offset + limit as usize))
        } else {
            None
        };
        let prev = if search_offset > 0 {
            Some(format!(
                "search_offset:{}",
                search_offset.saturating_sub(limit as usize)
            ))
        } else {
            None
        };
        (next, prev)
    } else if is_backward {
        let next = items.last().map(|p| {
            encode_name_cursor(&NameCursor {
                id: p.id,
                name: p.sort_name.clone().unwrap_or_else(|| p.name.clone()),
            })
        });
        let prev = if has_more {
            items.first().map(|p| {
                encode_name_cursor(&NameCursor {
                    id: p.id,
                    name: p.sort_name.clone().unwrap_or_else(|| p.name.clone()),
                })
            })
        } else {
            None
        };
        (next, prev)
    } else {
        let next = if has_more {
            items.last().map(|p| {
                encode_name_cursor(&NameCursor {
                    id: p.id,
                    name: p.sort_name.clone().unwrap_or_else(|| p.name.clone()),
                })
            })
        } else {
            None
        };
        let prev = if params.cursor.is_some() {
            items.first().map(|p| {
                encode_name_cursor(&NameCursor {
                    id: p.id,
                    name: p.sort_name.clone().unwrap_or_else(|| p.name.clone()),
                })
            })
        } else {
            None
        };
        (next, prev)
    };

    Ok(Json(PaginatedResponse {
        items,
        next_cursor,
        prev_cursor,
        total_count,
    }))
}

// =============================================================================
// Response Types
// =============================================================================

#[derive(Serialize)]
struct PersonResponse {
    id: Uuid,
    name: String,
    sort_name: Option<String>,
    image_url: Option<String>,
    bio: Option<String>,
    #[serde(with = "crate::serde_date::option_date")]
    birth_date: Option<time::Date>,
    #[serde(with = "crate::serde_date::option_date")]
    death_date: Option<time::Date>,
    identifiers: serde_json::Value,
    #[serde(with = "time::serde::rfc3339")]
    created_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: time::OffsetDateTime,
    is_monitored: bool,
    auto_want_mode: String,
    known_for_department: Option<String>,
    place_of_birth: Option<String>,
}

// =============================================================================
// Get Person
// =============================================================================

/// GET /api/v1/admin/people/:id
pub async fn get_person(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let person = crate::repositories::people::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get person: {e}")))?
        .ok_or_else(|| AppError::NotFound("Person not found".into()))?;

    let known_for_department = person
        .identifiers
        .get("known_for_department")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let place_of_birth = person
        .identifiers
        .get("place_of_birth")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(Json(PersonResponse {
        id: person.id,
        name: person.name,
        sort_name: person.sort_name,
        image_url: endpoint_url(
            &person.images,
            "profile",
            "/api/v1/admin/people/",
            person.id,
            "/image",
        ),
        bio: person.bio,
        birth_date: person.birth_date,
        death_date: person.death_date,
        identifiers: person.identifiers,
        created_at: person.created_at,
        updated_at: person.updated_at,
        is_monitored: person.is_monitored,
        auto_want_mode: person.auto_want_mode,
        known_for_department,
        place_of_birth,
    }))
}

// =============================================================================
// Create Person
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct CreatePersonRequest {
    pub name: String,
    pub sort_name: Option<String>,
    pub identifiers: Option<serde_json::Value>,
    pub image_url: Option<String>,
    pub bio: Option<String>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub birth_date: Option<time::Date>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub death_date: Option<time::Date>,
}

/// POST /api/v1/admin/people
pub async fn create_person(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<CreatePersonRequest>,
) -> Result<impl IntoResponse, AppError> {
    auth.require_contributor()
        .map_err(|_| AppError::Forbidden("Forbidden".into()))?;

    if body.name.trim().is_empty() {
        return Err(AppError::BadRequest("Name is required".into()));
    }

    let has_tmdb_id = body
        .identifiers
        .as_ref()
        .and_then(|ids| ids.get("tmdb_id"))
        .and_then(|v| v.as_str())
        .is_some();

    let images = body.image_url.map(|url| {
        let mut m = std::collections::HashMap::new();
        m.insert(
            "profile".to_string(),
            ImageEntry {
                source_url: Some(url),
                cached_at: None,
            },
        );
        m
    });

    let input = people::PersonInput {
        name: body.name,
        sort_name: body.sort_name,
        identifiers: body.identifiers,
        images,
        bio: body.bio,
        birth_date: body.birth_date,
        death_date: body.death_date,
    };

    let id = people::create(&state.pool, input)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to create person: {e}")))?;

    // Index in search
    if let Err(e) = search::index_person(state.search.as_ref(), &state.pool, id).await {
        tracing::warn!(error = %e, person_id = %id, "Failed to index person in search");
    }

    // Auto-enrich if person has a TMDB ID
    if has_tmdb_id {
        let display = crate::jobs::fetch_job_display(&state.pool, "person", id).await;
        let context = nats_jobs::JobContext::for_entity(
            "person",
            id,
            "Enriching newly created person".to_string(),
        )
        .with_display_opt(display);
        if let Err(e) = crate::jobs::enqueue(
            &state.pool,
            state.nats.publisher(),
            "person_enrich",
            json!({"person_id": id}),
            context,
        )
        .await
        {
            tracing::warn!(error = %e, person_id = %id, "Failed to enqueue person_enrich on creation");
        }
    }

    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

// =============================================================================
// Update Person
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct UpdatePersonRequest {
    pub name: Option<String>,
    pub sort_name: Option<String>,
    pub identifiers: Option<serde_json::Value>,
    pub image_url: Option<String>,
    pub bio: Option<String>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub birth_date: Option<time::Date>,
    #[serde(default, with = "crate::serde_date::option_date")]
    pub death_date: Option<time::Date>,
}

/// PATCH /api/v1/admin/people/:id
pub async fn update_person(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdatePersonRequest>,
) -> Result<impl IntoResponse, AppError> {
    auth.require_contributor()
        .map_err(|_| AppError::Forbidden("Forbidden".into()))?;

    let images = body.image_url.map(|url| {
        let mut m = std::collections::HashMap::new();
        m.insert(
            "profile".to_string(),
            ImageEntry {
                source_url: Some(url),
                cached_at: None,
            },
        );
        m
    });

    let input = people::PersonUpdate {
        name: body.name,
        sort_name: body.sort_name,
        identifiers: body.identifiers,
        images,
        bio: body.bio,
        birth_date: body.birth_date,
        death_date: body.death_date,
    };

    let updated = people::update(&state.pool, id, input)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to update person: {e}")))?;

    if !updated {
        return Err(AppError::NotFound("Person not found".into()));
    }

    // Re-index in search
    if let Err(e) = search::index_person(state.search.as_ref(), &state.pool, id).await {
        tracing::warn!(error = %e, person_id = %id, "Failed to re-index person in search");
    }

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Delete Person
// =============================================================================

/// DELETE /api/v1/admin/people/:id
pub async fn delete_person(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    auth.require_contributor()
        .map_err(|_| AppError::Forbidden("Forbidden".into()))?;

    let deleted = people::delete(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to delete person: {e}")))?;

    if !deleted {
        return Err(AppError::NotFound("Person not found".into()));
    }

    // Remove from search
    if let Err(e) = state.search.delete_person(id).await {
        tracing::warn!(error = %e, person_id = %id, "Failed to delete person from search");
    }

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Person Image
// =============================================================================

/// POST /api/v1/admin/people/:id/image
pub async fn upload_person_image(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, AppError> {
    auth.require_contributor()
        .map_err(|_| AppError::Forbidden("Forbidden".into()))?;

    people::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get person: {e}")))?
        .ok_or_else(|| AppError::NotFound("Person not found".into()))?;

    crate::images::save_image_bytes(
        &state.config.storage_root,
        crate::images::ImageType::PersonImage,
        id,
        &body,
    )
    .await
    .map_err(|e| AppError::Internal(format!("Failed to save image: {e}")))?;

    let _ = images::mark_cached(&state.pool, "people", id, "profile").await;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/v1/admin/people/:id/image
pub async fn get_person_image(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    use crate::images::{ImageType, cache_image, cached_image_exists, load_cached_image};

    let image_type = ImageType::PersonImage;

    if cached_image_exists(&state.config.storage_root, image_type, id) {
        let bytes = load_cached_image(&state.config.storage_root, image_type, id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to load image: {e}")))?;
        return Ok((
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    crate::images::detect_content_type(&bytes),
                ),
                (axum::http::header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            bytes,
        )
            .into_response());
    }

    let person = people::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get person: {e}")))?
        .ok_or_else(|| AppError::NotFound("Person not found".into()))?;

    if let Some(url) = source_url(&person.images, "profile") {
        cache_image(&state.config.storage_root, image_type, id, &url)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to cache image: {e}")))?;
        let _ = images::mark_cached(&state.pool, "people", id, "profile").await;
        let bytes = load_cached_image(&state.config.storage_root, image_type, id)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to load image: {e}")))?;
        return Ok((
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    crate::images::detect_content_type(&bytes),
                ),
                (axum::http::header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            bytes,
        )
            .into_response());
    }

    Err(AppError::NotFound("No image available".into()))
}

// =============================================================================
// Filmography (Provider Credits)
// =============================================================================

#[derive(Serialize)]
struct FilmographyCreditResponse {
    provider_id: String,
    media_type: String,
    title: String,
    year: Option<i32>,
    poster_url: Option<String>,
    character: Option<String>,
    department: Option<String>,
    job: Option<String>,
    credit_type: String,
    in_library: bool,
    library_id: Option<Uuid>,
    want_status: Option<String>,
    dismissed: bool,
}

#[derive(Serialize)]
struct FilmographyResponse {
    credits: Vec<FilmographyCreditResponse>,
}

/// Query library-only credits for a person, formatted as FilmographyCreditResponse.
/// Credits from movies/shows whose tmdb_id is in the exclude sets are skipped
/// (those are already represented in the TMDB provider results).
async fn get_library_credits(
    pool: &sqlx::PgPool,
    person_id: Uuid,
    exclude_movie_tmdb_ids: &HashSet<String>,
    exclude_tv_tmdb_ids: &HashSet<String>,
) -> Result<Vec<FilmographyCreditResponse>, AppError> {
    let mut credits = Vec::new();
    let mut seen = HashSet::new();

    let movie_cast = sqlx::query!(
        r#"
        SELECT m.id, m.title, m.year, m.images, mc.character_name,
               m.identifiers->>'tmdb_id' AS tmdb_id,
               m.want_status,
               EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = m.id) AS is_in_library
        FROM movie_cast mc
        JOIN movies m ON m.id = mc.movie_id
        WHERE mc.person_id = $1
        ORDER BY m.year DESC NULLS LAST
        "#,
        person_id,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("Failed to query movie cast: {e}")))?;

    let movie_crew = sqlx::query!(
        r#"
        SELECT m.id, m.title, m.year, m.images,
               mcr.department, mcr.job,
               m.identifiers->>'tmdb_id' AS tmdb_id,
               m.want_status,
               EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = m.id) AS is_in_library
        FROM movie_crew mcr
        JOIN movies m ON m.id = mcr.movie_id
        WHERE mcr.person_id = $1
        ORDER BY m.year DESC NULLS LAST
        "#,
        person_id,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("Failed to query movie crew: {e}")))?;

    let tv_cast = sqlx::query!(
        r#"
        SELECT s.id, s.title, s.year, s.images, tc.character_name,
               s.identifiers->>'tmdb_id' AS tmdb_id,
               EXISTS (SELECT 1 FROM tv_episodes te
                       JOIN tv_seasons tsn ON tsn.id = te.season_id
                       JOIN tv_episode_file_episodes tefe ON tefe.episode_id = te.id
                       JOIN tv_episode_files tef ON tef.id = tefe.episode_file_id AND tef.available
                       WHERE tsn.show_id = s.id) AS is_in_library
        FROM tv_cast tc
        JOIN tv_shows s ON s.id = tc.show_id
        WHERE tc.person_id = $1
        ORDER BY s.year DESC NULLS LAST
        "#,
        person_id,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("Failed to query tv cast: {e}")))?;

    let tv_crew = sqlx::query!(
        r#"
        SELECT s.id, s.title, s.year, s.images,
               tcr.department, tcr.job,
               s.identifiers->>'tmdb_id' AS tmdb_id,
               EXISTS (SELECT 1 FROM tv_episodes te
                       JOIN tv_seasons tsn ON tsn.id = te.season_id
                       JOIN tv_episode_file_episodes tefe ON tefe.episode_id = te.id
                       JOIN tv_episode_files tef ON tef.id = tefe.episode_file_id AND tef.available
                       WHERE tsn.show_id = s.id) AS is_in_library
        FROM tv_crew tcr
        JOIN tv_shows s ON s.id = tcr.show_id
        WHERE tcr.person_id = $1
        ORDER BY s.year DESC NULLS LAST
        "#,
        person_id,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("Failed to query tv crew: {e}")))?;

    for row in &movie_cast {
        if let Some(ref tid) = row.tmdb_id
            && exclude_movie_tmdb_ids.contains(tid)
        {
            continue;
        }
        if !seen.insert((row.id, "movie", "cast")) {
            continue;
        }
        let img_map: crate::images::ImageMap =
            serde_json::from_value(row.images.clone()).unwrap_or_default();
        credits.push(FilmographyCreditResponse {
            provider_id: row
                .tmdb_id
                .clone()
                .unwrap_or_else(|| format!("lib:{}", row.id)),
            media_type: "movie".into(),
            title: row.title.clone(),
            year: row.year,
            poster_url: endpoint_url(
                &img_map,
                "poster",
                "/api/v1/admin/movies/",
                row.id,
                "/poster",
            ),
            character: row.character_name.clone(),
            department: None,
            job: None,
            credit_type: "cast".into(),
            in_library: row.is_in_library.unwrap_or(false),
            library_id: Some(row.id),
            want_status: Some(row.want_status.clone()),
            dismissed: false,
        });
    }

    for row in &movie_crew {
        if let Some(ref tid) = row.tmdb_id
            && exclude_movie_tmdb_ids.contains(tid)
        {
            continue;
        }
        if !seen.insert((row.id, "movie", "crew")) {
            continue;
        }
        let img_map: crate::images::ImageMap =
            serde_json::from_value(row.images.clone()).unwrap_or_default();
        credits.push(FilmographyCreditResponse {
            provider_id: row
                .tmdb_id
                .clone()
                .unwrap_or_else(|| format!("lib:{}", row.id)),
            media_type: "movie".into(),
            title: row.title.clone(),
            year: row.year,
            poster_url: endpoint_url(
                &img_map,
                "poster",
                "/api/v1/admin/movies/",
                row.id,
                "/poster",
            ),
            character: None,
            department: Some(row.department.clone()),
            job: Some(row.job.clone()),
            credit_type: "crew".into(),
            in_library: row.is_in_library.unwrap_or(false),
            library_id: Some(row.id),
            want_status: Some(row.want_status.clone()),
            dismissed: false,
        });
    }

    for row in &tv_cast {
        if let Some(ref tid) = row.tmdb_id
            && exclude_tv_tmdb_ids.contains(tid)
        {
            continue;
        }
        if !seen.insert((row.id, "tv", "cast")) {
            continue;
        }
        let img_map: crate::images::ImageMap =
            serde_json::from_value(row.images.clone()).unwrap_or_default();
        credits.push(FilmographyCreditResponse {
            provider_id: row
                .tmdb_id
                .clone()
                .unwrap_or_else(|| format!("lib:{}", row.id)),
            media_type: "tv".into(),
            title: row.title.clone(),
            year: row.year,
            poster_url: endpoint_url(
                &img_map,
                "poster",
                "/api/v1/admin/tv/shows/",
                row.id,
                "/poster",
            ),
            character: row.character_name.clone(),
            department: None,
            job: None,
            credit_type: "cast".into(),
            in_library: row.is_in_library.unwrap_or(false),
            library_id: Some(row.id),
            want_status: Some("none".into()),
            dismissed: false,
        });
    }

    for row in &tv_crew {
        if let Some(ref tid) = row.tmdb_id
            && exclude_tv_tmdb_ids.contains(tid)
        {
            continue;
        }
        if !seen.insert((row.id, "tv", "crew")) {
            continue;
        }
        let img_map: crate::images::ImageMap =
            serde_json::from_value(row.images.clone()).unwrap_or_default();
        credits.push(FilmographyCreditResponse {
            provider_id: row
                .tmdb_id
                .clone()
                .unwrap_or_else(|| format!("lib:{}", row.id)),
            media_type: "tv".into(),
            title: row.title.clone(),
            year: row.year,
            poster_url: endpoint_url(
                &img_map,
                "poster",
                "/api/v1/admin/tv/shows/",
                row.id,
                "/poster",
            ),
            character: None,
            department: Some(row.department.clone()),
            job: Some(row.job.clone()),
            credit_type: "crew".into(),
            in_library: row.is_in_library.unwrap_or(false),
            library_id: Some(row.id),
            want_status: Some("none".into()),
            dismissed: false,
        });
    }

    Ok(credits)
}

/// GET /api/v1/admin/people/:id/filmography
pub async fn get_filmography(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let mut filmography = build_filmography(&state, id).await?;
    if auth.is_kids {
        keep_kids_credits(&state.pool, &mut filmography.credits)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to filter credits: {e}")))?;
    }
    Ok(Json(filmography))
}

/// A credit outside the library has no rating we hold, so a Kids profile sees only the
/// library titles the rule allows.
async fn keep_kids_credits(
    pool: &sqlx::PgPool,
    credits: &mut Vec<FilmographyCreditResponse>,
) -> sqlx::Result<()> {
    let ids = |media_type: &str| -> Vec<Uuid> {
        credits
            .iter()
            .filter(|c| c.media_type == media_type)
            .filter_map(|c| c.library_id)
            .collect()
    };
    let movies = crate::kids::allowed_movie_ids(pool, &ids("movie")).await?;
    let shows = crate::kids::allowed_show_ids(pool, &ids("tv")).await?;
    credits.retain(|c| match (c.media_type.as_str(), c.library_id) {
        ("movie", Some(id)) => movies.contains(&id),
        ("tv", Some(id)) => shows.contains(&id),
        _ => false,
    });
    Ok(())
}

async fn build_filmography(state: &AppState, id: Uuid) -> Result<FilmographyResponse, AppError> {
    let person = people::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get person: {e}")))?
        .ok_or_else(|| AppError::NotFound("Person not found".into()))?;

    let tmdb_id = person
        .identifiers
        .get("tmdb_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let empty_set = HashSet::new();

    let Some(tmdb_id) = tmdb_id else {
        // No TMDB ID — return library credits only
        let library_credits = get_library_credits(&state.pool, id, &empty_set, &empty_set).await?;
        return Ok(FilmographyResponse {
            credits: library_credits,
        });
    };

    let registry = VideoProviderRegistry::new(state.pool.clone());
    let combined = match registry.get_person_credits("tmdb", &tmdb_id).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(person_id = %id, error = %e, "Failed to fetch person credits from provider");
            // Fall back to library credits
            let library_credits =
                get_library_credits(&state.pool, id, &empty_set, &empty_set).await?;
            return Ok(FilmographyResponse {
                credits: library_credits,
            });
        }
    };

    // Collect all unique tmdb_ids for movies and tv
    let movie_tmdb_ids: HashSet<String> = combined
        .cast
        .iter()
        .filter(|c| c.media_type == "movie")
        .map(|c| c.provider_id.clone())
        .chain(
            combined
                .crew
                .iter()
                .filter(|c| c.media_type == "movie")
                .map(|c| c.provider_id.clone()),
        )
        .collect();

    let tv_tmdb_ids: HashSet<String> = combined
        .cast
        .iter()
        .filter(|c| c.media_type == "tv")
        .map(|c| c.provider_id.clone())
        .chain(
            combined
                .crew
                .iter()
                .filter(|c| c.media_type == "tv")
                .map(|c| c.provider_id.clone()),
        )
        .collect();

    // Batch-query movies by tmdb_ids
    // Key: tmdb_id -> (library_id, want_status, in_library)
    let mut movie_library: std::collections::HashMap<String, (Uuid, String, bool)> =
        std::collections::HashMap::new();
    if !movie_tmdb_ids.is_empty() {
        let placeholders: String = movie_tmdb_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("${}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, identifiers->>'tmdb_id' AS tmdb_id, want_status, EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = movies.id) AS is_in_library FROM movies WHERE identifiers->>'tmdb_id' IN ({})",
            placeholders
        );
        let mut q = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()));
        for tid in &movie_tmdb_ids {
            q = q.bind(tid);
        }
        if let Ok(rows) = q.fetch_all(&state.pool).await {
            use sqlx::Row as _;
            for row in rows {
                let tid: Option<String> = row.get("tmdb_id");
                let lib_id: Uuid = row.get("id");
                let want: String = row.get("want_status");
                let in_lib: bool = row.get("is_in_library");
                if let Some(tid) = tid {
                    movie_library.insert(tid, (lib_id, want, in_lib));
                }
            }
        }
    }

    // Batch-query tv_shows by tmdb_ids
    // A tv_shows row is metadata. Holding a show means holding episodes of it, so the
    // library test is availability, never the existence of the row.
    let mut tv_library: std::collections::HashMap<String, (Uuid, String, bool)> =
        std::collections::HashMap::new();
    if !tv_tmdb_ids.is_empty() {
        let placeholders: String = tv_tmdb_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("${}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, identifiers->>'tmdb_id' AS tmdb_id FROM tv_shows WHERE identifiers->>'tmdb_id' IN ({})",
            placeholders
        );
        let mut q = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()));
        for tid in &tv_tmdb_ids {
            q = q.bind(tid);
        }
        if let Ok(rows) = q.fetch_all(&state.pool).await {
            use sqlx::Row as _;
            for row in rows {
                let tid: Option<String> = row.get("tmdb_id");
                let lib_id: Uuid = row.get("id");
                if let Some(tid) = tid {
                    tv_library.insert(tid, (lib_id, "none".to_string(), true));
                }
            }
        }
    }

    // Load dismissed credits
    let dismissed_list = people::list_dismissed_credits(&state.pool, id)
        .await
        .unwrap_or_default();
    let dismissed_set: HashSet<(i32, String)> = dismissed_list
        .into_iter()
        .map(|d| (d.tmdb_id, d.media_type))
        .collect();

    let mut credits: Vec<FilmographyCreditResponse> = Vec::new();

    for credit in &combined.cast {
        let year = credit
            .release_date
            .as_deref()
            .and_then(|d| d.get(..4))
            .and_then(|y| y.parse::<i32>().ok());

        let provider_id_i32 = credit.provider_id.parse::<i32>().unwrap_or(0);
        let dismissed = dismissed_set.contains(&(provider_id_i32, credit.media_type.clone()));

        let (in_library, library_id, want_status) = match credit.media_type.as_str() {
            "movie" => {
                if let Some((lid, ws, in_lib)) = movie_library.get(&credit.provider_id) {
                    (*in_lib, Some(*lid), Some(ws.clone()))
                } else {
                    (false, None, None)
                }
            }
            "tv" => {
                if let Some((lid, ws, in_lib)) = tv_library.get(&credit.provider_id) {
                    (*in_lib, Some(*lid), Some(ws.clone()))
                } else {
                    (false, None, None)
                }
            }
            _ => (false, None, None),
        };

        credits.push(FilmographyCreditResponse {
            provider_id: credit.provider_id.clone(),
            media_type: credit.media_type.clone(),
            title: credit.title.clone(),
            year,
            poster_url: credit.poster_url.clone(),
            character: credit.character.clone(),
            department: None,
            job: None,
            credit_type: "cast".into(),
            in_library,
            library_id,
            want_status,
            dismissed,
        });
    }

    for credit in &combined.crew {
        let year = credit
            .release_date
            .as_deref()
            .and_then(|d| d.get(..4))
            .and_then(|y| y.parse::<i32>().ok());

        let provider_id_i32 = credit.provider_id.parse::<i32>().unwrap_or(0);
        let dismissed = dismissed_set.contains(&(provider_id_i32, credit.media_type.clone()));

        let (in_library, library_id, want_status) = match credit.media_type.as_str() {
            "movie" => {
                if let Some((lid, ws, in_lib)) = movie_library.get(&credit.provider_id) {
                    (*in_lib, Some(*lid), Some(ws.clone()))
                } else {
                    (false, None, None)
                }
            }
            "tv" => {
                if let Some((lid, ws, in_lib)) = tv_library.get(&credit.provider_id) {
                    (*in_lib, Some(*lid), Some(ws.clone()))
                } else {
                    (false, None, None)
                }
            }
            _ => (false, None, None),
        };

        credits.push(FilmographyCreditResponse {
            provider_id: credit.provider_id.clone(),
            media_type: credit.media_type.clone(),
            title: credit.title.clone(),
            year,
            poster_url: credit.poster_url.clone(),
            character: None,
            department: credit.department.clone(),
            job: credit.job.clone(),
            credit_type: "crew".into(),
            in_library,
            library_id,
            want_status,
            dismissed,
        });
    }

    // Append library-only credits (movies/shows not covered by TMDB results)
    let library_only = get_library_credits(&state.pool, id, &movie_tmdb_ids, &tv_tmdb_ids).await?;
    credits.extend(library_only);

    Ok(FilmographyResponse { credits })
}

// =============================================================================
// Want Credit
// =============================================================================

#[derive(Deserialize)]
pub struct WantCreditRequest {
    pub provider_id: String,
    pub media_type: String,
    pub title: Option<String>,
    pub year: Option<i32>,
    /// Provider type (e.g. "tmdb", "tvdb"). Defaults to "tmdb" for backwards compatibility.
    pub provider: Option<String>,
}

/// POST /api/v1/admin/people/:id/filmography/want
pub async fn want_credit(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<WantCreditRequest>,
) -> Result<impl IntoResponse, AppError> {
    auth.require_contributor()
        .map_err(|_| AppError::Forbidden("Forbidden".into()))?;

    // Verify person exists
    people::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get person: {e}")))?
        .ok_or_else(|| AppError::NotFound("Person not found".into()))?;

    let title = body
        .title
        .clone()
        .unwrap_or_else(|| body.provider_id.clone());

    let provider_type = body.provider.as_deref().unwrap_or("tmdb");
    let id_key = format!("{provider_type}_id");

    match body.media_type.as_str() {
        "movie" => {
            let identifiers = json!({&id_key: body.provider_id});
            let movie_id = crate::repositories::movies::upsert_by_provider(
                &state.pool,
                provider_type,
                &body.provider_id,
                crate::repositories::movies::MovieInput {
                    title,
                    year: body.year,
                    identifiers: Some(identifiers),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| AppError::Internal(format!("Failed to upsert movie: {e}")))?;

            crate::repositories::movies::set_want_status(&state.pool, movie_id, "wanted")
                .await
                .map_err(|e| AppError::Internal(format!("Failed to set want status: {e}")))?;

            let display = crate::jobs::fetch_job_display(&state.pool, "movie", movie_id).await;
            let context = nats_jobs::JobContext::for_entity(
                "movie",
                movie_id,
                "Wanting movie from person filmography".to_string(),
            )
            .with_display_opt(display);
            if let Err(e) = crate::jobs::enqueue(
                &state.pool,
                state.nats.publisher(),
                "movie_want",
                json!({"movie_id": movie_id, "user_id": auth.user_id}),
                context,
            )
            .await
            {
                tracing::warn!(error = %e, movie_id = %movie_id, "Failed to enqueue movie_want job");
            }

            Ok((
                StatusCode::OK,
                Json(json!({"id": movie_id, "media_type": "movie"})),
            )
                .into_response())
        }
        "tv" => {
            let identifiers = json!({&id_key: body.provider_id});
            let show_id = crate::repositories::tv_shows::upsert_by_provider(
                &state.pool,
                provider_type,
                &body.provider_id,
                crate::repositories::tv_shows::TvShowInput {
                    title,
                    year: body.year,
                    identifiers: Some(identifiers),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| AppError::Internal(format!("Failed to upsert TV show: {e}")))?;

            let display = crate::jobs::fetch_job_display(&state.pool, "tv_show", show_id).await;
            let context = nats_jobs::JobContext::for_entity(
                "tv_show",
                show_id,
                "Wanting TV show from person filmography".to_string(),
            )
            .with_display_opt(display);
            if let Err(e) = crate::jobs::enqueue(
                &state.pool,
                state.nats.publisher(),
                "tv_show_enrich",
                json!({"show_id": show_id}),
                context,
            )
            .await
            {
                tracing::warn!(error = %e, show_id = %show_id, "Failed to enqueue tv_show_enrich job");
            }

            Ok((
                StatusCode::OK,
                Json(json!({"id": show_id, "media_type": "tv"})),
            )
                .into_response())
        }
        other => Err(AppError::BadRequest(format!(
            "Unsupported media_type: {other}"
        ))),
    }
}

// =============================================================================
// Dismiss Credit
// =============================================================================

#[derive(Deserialize)]
pub struct DismissCreditRequest {
    pub provider_id: String,
    pub media_type: String,
}

/// POST /api/v1/admin/people/:id/filmography/dismiss
pub async fn dismiss_credit(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<DismissCreditRequest>,
) -> Result<impl IntoResponse, AppError> {
    auth.require_contributor()
        .map_err(|_| AppError::Forbidden("Forbidden".into()))?;

    people::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get person: {e}")))?
        .ok_or_else(|| AppError::NotFound("Person not found".into()))?;

    let tmdb_id: i32 = body
        .provider_id
        .parse()
        .map_err(|_| AppError::BadRequest(format!("Invalid provider_id: {}", body.provider_id)))?;

    people::dismiss_credit(&state.pool, id, tmdb_id, &body.media_type)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to dismiss credit: {e}")))?;

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/v1/admin/people/:id/filmography/dismiss
pub async fn undismiss_credit(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<DismissCreditRequest>,
) -> Result<impl IntoResponse, AppError> {
    auth.require_contributor()
        .map_err(|_| AppError::Forbidden("Forbidden".into()))?;

    people::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get person: {e}")))?
        .ok_or_else(|| AppError::NotFound("Person not found".into()))?;

    let tmdb_id: i32 = body
        .provider_id
        .parse()
        .map_err(|_| AppError::BadRequest(format!("Invalid provider_id: {}", body.provider_id)))?;

    people::undismiss_credit(&state.pool, id, tmdb_id, &body.media_type)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to undismiss credit: {e}")))?;

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Person Monitoring
// =============================================================================

#[derive(Deserialize)]
pub struct UpdatePersonMonitoringRequest {
    pub is_monitored: Option<bool>,
    pub auto_want_mode: Option<String>,
}

/// PUT /api/v1/admin/people/:id/monitoring
pub async fn update_person_monitoring(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdatePersonMonitoringRequest>,
) -> Result<impl IntoResponse, AppError> {
    auth.require_contributor()
        .map_err(|_| AppError::Forbidden("Forbidden".into()))?;

    if let Some(ref mode) = body.auto_want_mode {
        let valid_modes = ["none", "new_only", "all"];
        if !valid_modes.contains(&mode.as_str()) {
            return Err(AppError::BadRequest(format!(
                "invalid auto_want_mode: must be one of {:?}",
                valid_modes
            )));
        }
    }

    if body.is_monitored.is_none() && body.auto_want_mode.is_none() {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }

    let updated = people::update_monitoring(
        &state.pool,
        id,
        body.is_monitored,
        body.auto_want_mode.as_deref(),
    )
    .await
    .map_err(|e| AppError::Internal(format!("Failed to update monitoring: {e}")))?;

    if !updated {
        return Err(AppError::NotFound("Person not found".into()));
    }

    // When monitoring is turned on, trigger an immediate enrich
    if body.is_monitored == Some(true) {
        let display = crate::jobs::fetch_job_display(&state.pool, "person", id).await;
        let context = nats_jobs::JobContext::for_entity(
            "person",
            id,
            "Enriching person after monitoring enabled".to_string(),
        )
        .with_display_opt(display);
        if let Err(e) = crate::jobs::enqueue(
            &state.pool,
            state.nats.publisher(),
            "person_enrich",
            json!({"person_id": id}),
            context,
        )
        .await
        {
            tracing::warn!(error = %e, person_id = %id, "Failed to enqueue person_enrich on monitoring enable");
        }
    }

    Ok(StatusCode::NO_CONTENT.into_response())
}

// =============================================================================
// Person Refresh
// =============================================================================

/// POST /api/v1/admin/people/:id/refresh
pub async fn refresh_person(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    auth.require_contributor()
        .map_err(|_| AppError::Forbidden("Forbidden".into()))?;

    let person = people::get(&state.pool, id)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to get person: {e}")))?
        .ok_or_else(|| AppError::NotFound("Person not found".into()))?;

    let display = crate::jobs::fetch_job_display(&state.pool, "person", id).await;
    let context = nats_jobs::JobContext::for_entity(
        "person",
        id,
        format!("Enriching person: {}", person.name),
    )
    .with_display_opt(display);
    if let Err(e) = crate::jobs::enqueue(
        &state.pool,
        state.nats.publisher(),
        "person_enrich",
        json!({"person_id": id, "invalidate_cache": true}),
        context,
    )
    .await
    {
        tracing::warn!(error = %e, person_id = %id, "Failed to enqueue person_enrich job");
    }

    Ok(Json(json!({
        "message": format!("Metadata refresh queued for person: {}", person.name)
    })))
}
