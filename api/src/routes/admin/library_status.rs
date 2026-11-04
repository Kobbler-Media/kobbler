//! Shared "do we already have this?" lookups, keyed by provider id.
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

/// Audnexus ids are ASINs, which books also carry under `asin` from other sources.
fn book_identifier_keys(provider: &str) -> Vec<String> {
    let mut keys = vec![provider.to_string()];
    if provider == crate::http::provider_ids::AUDNEXUS {
        keys.push("asin".to_string());
    }
    keys
}

pub async fn book_status(
    pool: &sqlx::PgPool,
    provider: &str,
    external_id: &str,
) -> Option<LibraryStatus> {
    let row = sqlx::query!(
        r#"
        SELECT b.id, b.want_status,
               (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)
                OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id)) AS "is_in_library!"
        FROM books b
        WHERE b.ingest_status IS NULL
          AND EXISTS (SELECT 1 FROM unnest($1::text[]) AS k(key) WHERE b.identifiers->>k.key = $2)
        ORDER BY 3 DESC, b.created_at ASC
        LIMIT 1
        "#,
        &book_identifier_keys(provider),
        external_id,
    )
    .fetch_optional(pool)
    .await;

    match row {
        Ok(row) => row.map(|r| LibraryStatus {
            id: r.id,
            want_status: Some(r.want_status),
            is_in_library: Some(r.is_in_library),
        }),
        Err(e) => {
            tracing::warn!("Failed to look up library book for {provider}/{external_id}: {e}");
            None
        }
    }
}
