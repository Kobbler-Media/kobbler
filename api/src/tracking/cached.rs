//! Caching wrapper for author/series provider API calls.
//!
//! Wraps any `TrackedAuthorProvider` with `metadata_cache` lookups so that
//! repeated queries (job retries, periodic syncs) don't hit external APIs.

use crate::repositories::metadata_cache;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tracing::debug;

use super::hardcover::{
    AuthorSearchResult, AuthorWithBooks, SeriesWithBooks, TrackedAuthorProvider,
};

const CACHE_TTL_HOURS: i64 = 24 * 7; // 7 days, matching MetadataCache

/// Result wrapper that includes whether the data came from cache.
pub struct CachedResult<T> {
    pub data: T,
    pub cached: bool,
}

/// Caching wrapper around any `TrackedAuthorProvider`.
pub struct CachedAuthorProvider {
    inner: Box<dyn TrackedAuthorProvider>,
    pool: PgPool,
}

impl CachedAuthorProvider {
    pub fn new(inner: Box<dyn TrackedAuthorProvider>, pool: PgPool) -> Self {
        Self { inner, pool }
    }

    /// The provider ID, delegated from the inner provider.
    pub fn provider_id(&self) -> &str {
        self.inner.provider_id()
    }

    fn expires_at() -> OffsetDateTime {
        OffsetDateTime::now_utc() + Duration::hours(CACHE_TTL_HOURS)
    }

    pub async fn search_authors(
        &self,
        query: &str,
    ) -> Result<CachedResult<Vec<AuthorSearchResult>>, String> {
        let ns = self.inner.provider_id();
        let cache_key = format!("author_search:{}", query.to_lowercase());

        if let Ok(Some(cached)) = metadata_cache::get(&self.pool, ns, &cache_key).await
            && let Ok(results) = serde_json::from_value(cached)
        {
            debug!(query = %query, "Cache hit for author search");
            return Ok(CachedResult {
                data: results,
                cached: true,
            });
        }

        let results = self.inner.search_authors(query).await?;

        if let Ok(json) = serde_json::to_value(&results) {
            let _ =
                metadata_cache::set(&self.pool, ns, &cache_key, &json, Self::expires_at()).await;
        }

        Ok(CachedResult {
            data: results,
            cached: false,
        })
    }

    pub async fn get_author_books(
        &self,
        author_id: &str,
    ) -> Result<CachedResult<AuthorWithBooks>, String> {
        let ns = self.inner.provider_id();
        let cache_key = format!("author_books:{}", author_id);

        if let Ok(Some(cached)) = metadata_cache::get(&self.pool, ns, &cache_key).await
            && let Ok(result) = serde_json::from_value(cached)
        {
            debug!(author_id = %author_id, "Cache hit for author books");
            return Ok(CachedResult {
                data: result,
                cached: true,
            });
        }

        let result = self.inner.get_author_books(author_id).await?;

        if let Ok(json) = serde_json::to_value(&result) {
            let _ =
                metadata_cache::set(&self.pool, ns, &cache_key, &json, Self::expires_at()).await;
        }

        Ok(CachedResult {
            data: result,
            cached: false,
        })
    }

    pub async fn get_series_books(
        &self,
        series_id: &str,
    ) -> Result<CachedResult<SeriesWithBooks>, String> {
        let ns = self.inner.provider_id();
        let cache_key = format!("series_books:{}", series_id);

        if let Ok(Some(cached)) = metadata_cache::get(&self.pool, ns, &cache_key).await
            && let Ok(result) = serde_json::from_value(cached)
        {
            debug!(series_id = %series_id, "Cache hit for series books");
            return Ok(CachedResult {
                data: result,
                cached: true,
            });
        }

        let result = self.inner.get_series_books(series_id).await?;

        if let Ok(json) = serde_json::to_value(&result) {
            let _ =
                metadata_cache::set(&self.pool, ns, &cache_key, &json, Self::expires_at()).await;
        }

        Ok(CachedResult {
            data: result,
            cached: false,
        })
    }
}
