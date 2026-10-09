//! Shared "do we already have this?" lookups, keyed by TMDB id.
//!
//! Discover rows and provider search results both need to route an owned title
//! straight to its library page instead of the provider preview page.

use std::collections::HashMap;

use serde::Serialize;
use uuid::Uuid;

/// Library presence for one provider result.
///
/// `is_in_library` means "we hold a file" for movies but only "the show exists"
/// for TV; `id` is set whenever a row exists at all, including merely wanted
/// titles with nothing downloaded. Route on `id`, badge on `is_in_library`.
#[derive(Debug, Clone, Serialize)]
pub struct LibraryStatus {
    pub id: Uuid,
    pub want_status: Option<String>,
    pub is_in_library: Option<bool>,
}

pub async fn batch_movie_status(
    pool: &sqlx::PgPool,
    tmdb_ids: &[String],
) -> HashMap<String, LibraryStatus> {
    if tmdb_ids.is_empty() {
        return HashMap::new();
    }

    let rows = sqlx::query(
        "SELECT id, identifiers->>'tmdb_id' AS tmdb_id, want_status,
                EXISTS (SELECT 1 FROM movie_files mf WHERE mf.movie_id = movies.id) AS is_in_library
         FROM movies WHERE identifiers->>'tmdb_id' = ANY($1)",
    )
    .bind(tmdb_ids)
    .fetch_all(pool)
    .await;

    match rows {
        Ok(rows) => {
            use sqlx::Row;
            rows.into_iter()
                .filter_map(|r| {
                    let tmdb_id: Option<String> = r.get("tmdb_id");
                    let tmdb_id = tmdb_id?;
                    Some((
                        tmdb_id,
                        LibraryStatus {
                            id: r.get("id"),
                            want_status: r.get("want_status"),
                            is_in_library: r.get("is_in_library"),
                        },
                    ))
                })
                .collect()
        }
        Err(e) => {
            tracing::warn!("Failed to batch lookup movies: {e}");
            HashMap::new()
        }
    }
}

pub async fn batch_tv_status(
    pool: &sqlx::PgPool,
    tmdb_ids: &[String],
) -> HashMap<String, LibraryStatus> {
    if tmdb_ids.is_empty() {
        return HashMap::new();
    }

    let rows = sqlx::query(
        "SELECT id, identifiers->>'tmdb_id' AS tmdb_id, is_monitored
         FROM tv_shows WHERE identifiers->>'tmdb_id' = ANY($1)",
    )
    .bind(tmdb_ids)
    .fetch_all(pool)
    .await;

    match rows {
        Ok(rows) => {
            use sqlx::Row;
            rows.into_iter()
                .filter_map(|r| {
                    let tmdb_id: Option<String> = r.get("tmdb_id");
                    let tmdb_id = tmdb_id?;
                    let is_monitored: Option<bool> = r.get("is_monitored");
                    let want = if is_monitored.unwrap_or(false) {
                        Some("monitored".to_string())
                    } else {
                        None
                    };
                    Some((
                        tmdb_id,
                        LibraryStatus {
                            id: r.get("id"),
                            want_status: want,
                            is_in_library: Some(true),
                        },
                    ))
                })
                .collect()
        }
        Err(e) => {
            tracing::warn!("Failed to batch lookup TV shows: {e}");
            HashMap::new()
        }
    }
}
