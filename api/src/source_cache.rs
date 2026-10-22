//! Lookups against optional metadata sources (pulselive, WorldSBK, Jolpica,
//! Sportarr's league names and teams, TMDB alternative titles, Hardcover
//! editions).
//!
//! Wants are built from these on every search, so a source that is down must
//! not cost every build a timeout and a warning: a failure is remembered per
//! key for an hour and the source is not asked again until it lapses.

use std::fmt::Display;
use std::future::Future;

use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::PgPool;
use tracing::warn;

use crate::repositories::metadata_cache as cache_repo;
use crate::video_metadata::VideoProviderError;

/// Per-request timeout for these sources, well under the shared client's.
pub const LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

const FAILURE_WINDOW: time::Duration = time::Duration::hours(1);

pub async fn get_text(
    client: &reqwest::Client,
    source: &str,
    url: &str,
) -> Result<String, VideoProviderError> {
    let resp = client.get(url).timeout(LOOKUP_TIMEOUT).send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(VideoProviderError::Other(format!(
            "{source} returned HTTP {status} for {url}"
        )));
    }
    Ok(resp.text().await?)
}

fn failure_key(key: &str) -> String {
    format!("failed:{key}")
}

pub async fn read<T: DeserializeOwned>(pool: &PgPool, source: &str, key: &str) -> Option<T> {
    let cached = cache_repo::get(pool, source, key).await.ok()??;
    serde_json::from_value(cached).ok()
}

/// The cached answer for `key`, or the source's, kept for `ttl`. An answer
/// that there is nothing (no such team, no editions) is an answer and is
/// cached like any other; `fetch` reports those as `Ok`. `None` means the
/// source failed now or within the last hour.
pub async fn lookup<T, E, Fut>(
    pool: &PgPool,
    source: &str,
    key: &str,
    ttl: time::Duration,
    fetch: impl FnOnce() -> Fut,
) -> Option<T>
where
    T: Serialize + DeserializeOwned,
    E: Display,
    Fut: Future<Output = Result<T, E>>,
{
    if let Some(found) = read(pool, source, key).await {
        return Some(found);
    }
    if matches!(
        cache_repo::get(pool, source, &failure_key(key)).await,
        Ok(Some(_))
    ) {
        return None;
    }
    let now = time::OffsetDateTime::now_utc();
    match fetch().await {
        Ok(value) => {
            if let Ok(json) = serde_json::to_value(&value) {
                let _ = cache_repo::set(pool, source, key, &json, now + ttl).await;
            }
            Some(value)
        }
        Err(e) => {
            warn!(
                source,
                key,
                error = %e,
                "Metadata source unavailable; not asking it again for an hour"
            );
            let marker = serde_json::Value::String(e.to_string());
            let _ = cache_repo::set(
                pool,
                source,
                &failure_key(key),
                &marker,
                now + FAILURE_WINDOW,
            )
            .await;
            None
        }
    }
}
