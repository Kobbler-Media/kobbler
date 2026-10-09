use sqlx::PgPool;
use time::{Duration, OffsetDateTime};

use super::types::{MetadataResult, SeriesSearchResult};
use crate::repositories::{RepositoryError, metadata_cache as metadata_cache_repo};

/// Cache for metadata search results
pub struct MetadataCache {
    pool: PgPool,
    /// Cache TTL in hours
    ttl_hours: i64,
}

impl MetadataCache {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            ttl_hours: 24 * 7, // 1 week default
        }
    }

    /// Get cached results for a provider and query
    pub async fn get(
        &self,
        provider_type: &str,
        query_key: &str,
    ) -> Result<Option<Vec<MetadataResult>>, RepositoryError> {
        match metadata_cache_repo::get(&self.pool, provider_type, query_key).await {
            Ok(Some(response)) => match serde_json::from_value(response) {
                Ok(results) => Ok(Some(results)),
                Err(e) => {
                    tracing::warn!("Failed to deserialize cached results: {}", e);
                    Ok(None)
                }
            },
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Store results in cache (skips empty results to avoid caching transient failures)
    pub async fn set(
        &self,
        provider_type: &str,
        query_key: &str,
        results: &[MetadataResult],
    ) -> Result<(), RepositoryError> {
        if results.is_empty() {
            return Ok(());
        }
        let expires_at = OffsetDateTime::now_utc() + Duration::hours(self.ttl_hours);
        let response = serde_json::to_value(results).unwrap_or(serde_json::Value::Array(vec![]));

        metadata_cache_repo::set(&self.pool, provider_type, query_key, &response, expires_at).await
    }

    /// Get cached series search results for a provider and query
    pub async fn get_series(
        &self,
        provider_type: &str,
        query_key: &str,
    ) -> Result<Option<Vec<SeriesSearchResult>>, RepositoryError> {
        match metadata_cache_repo::get(&self.pool, provider_type, query_key).await {
            Ok(Some(response)) => match serde_json::from_value(response) {
                Ok(results) => Ok(Some(results)),
                Err(e) => {
                    tracing::warn!("Failed to deserialize cached series results: {}", e);
                    Ok(None)
                }
            },
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Store series search results in cache (skips empty results to avoid caching transient failures)
    pub async fn set_series(
        &self,
        provider_type: &str,
        query_key: &str,
        results: &[SeriesSearchResult],
    ) -> Result<(), RepositoryError> {
        if results.is_empty() {
            return Ok(());
        }
        let expires_at = OffsetDateTime::now_utc() + Duration::hours(self.ttl_hours);
        let response = serde_json::to_value(results).unwrap_or(serde_json::Value::Array(vec![]));

        metadata_cache_repo::set(&self.pool, provider_type, query_key, &response, expires_at).await
    }
}
