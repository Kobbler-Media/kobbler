//! Background job that syncs trending/popular content from TMDB
//! into the metadata cache for the Discover page.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::repositories::{metadata_cache, video_metadata_providers};
use crate::runtime_config::RuntimeConfigManager;
use crate::video_metadata::tmdb::DiscoverQuery;
use crate::video_metadata::types::MovieResult;
use crate::video_metadata::types::TvShowResult;

/// Cache provider type used for all discover lists.
pub const DISCOVER_PROVIDER: &str = "discover";

/// How long discover lists stay valid in the cache (6 hours).
const CACHE_TTL_SECS: i64 = 6 * 3600;

/// A section of discover content with metadata for display ordering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverSection {
    pub title: String,
    pub section_key: String,
    pub sort_order: i32,
    /// Built from rating- and genre-filtered queries, and the only shelves a Kids profile gets.
    #[serde(default)]
    pub kids: bool,
    pub items: Vec<DiscoverItem>,
}

/// Normalized item stored in the discover cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverItem {
    pub provider: String,
    pub provider_id: String,
    pub media_type: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poster_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backdrop_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating_count: Option<i32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(default)]
    pub identifiers: Value,
}

fn movie_to_discover_item(m: MovieResult) -> DiscoverItem {
    DiscoverItem {
        provider: "tmdb".into(),
        provider_id: m.provider_id.clone(),
        media_type: "movie".into(),
        title: m.title,
        year: m.year.map(|y| y as i32),
        description: m.description,
        poster_url: m.poster_url,
        backdrop_url: m.backdrop_url,
        genres: m.genres,
        rating: m.rating,
        rating_count: m.rating_count,
        authors: vec![],
        identifiers: serde_json::to_value(&m.identifiers).unwrap_or_default(),
    }
}

fn tv_to_discover_item(s: TvShowResult) -> DiscoverItem {
    DiscoverItem {
        provider: "tmdb".into(),
        provider_id: s.provider_id.clone(),
        media_type: "tv".into(),
        title: s.title,
        year: s.year.map(|y| y as i32),
        description: s.description,
        poster_url: s.poster_url,
        backdrop_url: s.backdrop_url,
        genres: s.genres,
        rating: s.rating,
        rating_count: s.rating_count,
        authors: vec![],
        identifiers: serde_json::to_value(&s.identifiers).unwrap_or_default(),
    }
}

/// Handler for periodic discover content sync.
pub struct DiscoverSyncHandler {
    pool: PgPool,
    config: RuntimeConfigManager,
    storage_root: String,
}

impl DiscoverSyncHandler {
    pub fn new(pool: PgPool, config: RuntimeConfigManager, storage_root: String) -> Self {
        Self {
            pool,
            config,
            storage_root,
        }
    }

    /// Every shelf picture onto disk before the first viewer asks for it.
    async fn warm_art(&self) {
        let sections = match metadata_cache::list_by_provider(&self.pool, DISCOVER_PROVIDER).await {
            Ok(sections) => sections,
            Err(e) => {
                warn!("Failed to read discover shelves to warm their art: {e}");
                return;
            }
        };
        let sections: Vec<DiscoverSection> = sections
            .into_iter()
            .filter_map(|(_, value)| serde_json::from_value(value).ok())
            .collect();
        let srcs: Vec<String> = sections
            .into_iter()
            .flat_map(|s| s.items)
            .flat_map(|item| [item.backdrop_url, item.poster_url])
            .flatten()
            .collect();
        crate::discover_art::warm_and_prune(&self.storage_root, srcs.iter().map(String::as_str))
            .await;
    }

    async fn cache_section(
        &self,
        section: &DiscoverSection,
        expires: OffsetDateTime,
    ) -> Result<(), String> {
        metadata_cache::set(
            &self.pool,
            DISCOVER_PROVIDER,
            &section.section_key,
            &json!(section),
            expires,
        )
        .await
        .map_err(|e| format!("Failed to cache section '{}': {e}", section.section_key))
    }

    async fn sync_tmdb(&self) -> (usize, Vec<String>) {
        let mut total = 0;
        let mut errors = Vec::new();

        // Get TMDB config from DB
        let tmdb_config = match video_metadata_providers::list_enabled(&self.pool).await {
            Ok(providers) => providers
                .into_iter()
                .find(|p| p.provider_type == "tmdb")
                .map(|p| p.config),
            Err(e) => {
                errors.push(format!("Failed to query video providers: {e}"));
                return (0, errors);
            }
        };

        let config = match tmdb_config {
            Some(c) => c,
            None => {
                info!("TMDB not enabled, skipping discover sync");
                return (0, errors);
            }
        };

        let has_key = config
            .get("api_key")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty());
        if !has_key {
            // Seeded `enabled = true` by migration 0049 with an empty config —
            // every fresh install starts here. Not configured yet is not an
            // error four times over.
            info!("TMDB enabled but no API key set, skipping discover sync");
            return (0, errors);
        }

        let client = crate::http::new_client();
        let tmdb = crate::video_metadata::tmdb::TmdbProvider::from_config(client, &config);
        let expires = OffsetDateTime::now_utc() + time::Duration::seconds(CACHE_TTL_SECS);

        // Standard rows — TV first
        let standard_sections: Vec<(&str, &str, i32)> = vec![
            ("trending_tv", "Trending TV Shows", 0),
            ("popular_tv", "Popular TV Shows", 1),
            ("trending_movies", "Trending Movies", 2),
            ("popular_movies", "Popular Movies", 3),
        ];

        for (key, title, order) in &standard_sections {
            let items: Vec<DiscoverItem> = match *key {
                "trending_tv" => match tmdb.trending_tv().await {
                    Ok(shows) => shows.into_iter().map(tv_to_discover_item).collect(),
                    Err(e) => {
                        errors.push(format!("TMDB {key}: {e}"));
                        continue;
                    }
                },
                "popular_tv" => match tmdb.popular_tv().await {
                    Ok(shows) => shows.into_iter().map(tv_to_discover_item).collect(),
                    Err(e) => {
                        errors.push(format!("TMDB {key}: {e}"));
                        continue;
                    }
                },
                "trending_movies" => match tmdb.trending_movies().await {
                    Ok(movies) => movies.into_iter().map(movie_to_discover_item).collect(),
                    Err(e) => {
                        errors.push(format!("TMDB {key}: {e}"));
                        continue;
                    }
                },
                "popular_movies" => match tmdb.popular_movies().await {
                    Ok(movies) => movies.into_iter().map(movie_to_discover_item).collect(),
                    Err(e) => {
                        errors.push(format!("TMDB {key}: {e}"));
                        continue;
                    }
                },
                _ => continue,
            };

            total += items.len();
            let section = DiscoverSection {
                title: title.to_string(),
                section_key: key.to_string(),
                sort_order: *order,
                kids: false,
                items,
            };
            if let Err(e) = self.cache_section(&section, expires).await {
                errors.push(e);
            }
        }

        // Streaming service rows from config
        let rt_config = self.config.get().await;
        let region = &rt_config.discover_region;
        let services = &rt_config.discover_streaming_services;

        // Clean up old service rows before writing new ones
        for prefix in ["service_", "kids_service_"] {
            if let Err(e) =
                metadata_cache::delete_by_prefix(&self.pool, DISCOVER_PROVIDER, prefix).await
            {
                errors.push(format!("Failed to clean old {prefix} rows: {e}"));
            }
        }

        let mut shelves: Vec<(String, String, i32, DiscoverQuery<'_>)> = vec![
            (
                "kids_movies".into(),
                "Family Movies".into(),
                0,
                DiscoverQuery {
                    provider_id: None,
                    region,
                    kids: true,
                },
            ),
            (
                "kids_tv".into(),
                "Kids TV".into(),
                1,
                DiscoverQuery {
                    provider_id: None,
                    region,
                    kids: true,
                },
            ),
        ];
        for (i, service) in services.iter().enumerate() {
            let sort_order = 10 + i as i32;
            for kids in [false, true] {
                let (key, title) = if kids {
                    (
                        format!("kids_service_{}", service.id),
                        format!("Kids on {}", service.name),
                    )
                } else {
                    (
                        format!("service_{}", service.id),
                        format!("Popular on {}", service.name),
                    )
                };
                shelves.push((
                    key,
                    title,
                    sort_order,
                    DiscoverQuery {
                        provider_id: Some(service.id),
                        region,
                        kids,
                    },
                ));
            }
        }

        for (section_key, title, sort_order, query) in shelves {
            // A kids-wide shelf is one medium each; a service shelf mixes both, as the
            // service's own home screen does.
            let (want_tv, want_movies) = match section_key.as_str() {
                "kids_movies" => (false, true),
                "kids_tv" => (true, false),
                _ => (true, true),
            };
            let mut items = Vec::new();
            if want_tv {
                match tmdb.discover_tv(&query).await {
                    Ok(shows) => items.extend(shows.into_iter().map(tv_to_discover_item)),
                    Err(e) => errors.push(format!("TMDB discover TV for {title}: {e}")),
                }
            }
            if want_movies {
                match tmdb.discover_movies(&query).await {
                    Ok(movies) => items.extend(movies.into_iter().map(movie_to_discover_item)),
                    Err(e) => errors.push(format!("TMDB discover movies for {title}: {e}")),
                }
            }

            if items.is_empty() {
                continue;
            }

            total += items.len();
            let section = DiscoverSection {
                title,
                section_key,
                sort_order,
                kids: query.kids,
                items,
            };
            if let Err(e) = self.cache_section(&section, expires).await {
                errors.push(e);
            }
        }

        (total, errors)
    }
}

#[async_trait]
impl JobHandler for DiscoverSyncHandler {
    fn job_type(&self) -> &'static str {
        "discover_sync"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        job_id: Uuid,
        _payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        nats_jobs::check_cancelled(&cancel)?;
        info!(job_id = %job_id, "Running discover content sync");

        let (item_count, sync_errors) = self.sync_tmdb().await;
        self.warm_art().await;

        if !sync_errors.is_empty() {
            for err in &sync_errors {
                warn!("Discover sync error: {err}");
            }
        }

        info!(
            items = item_count,
            errors = sync_errors.len(),
            "Discover sync complete"
        );

        Ok(json!({
            "items": item_count,
            "errors": sync_errors,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_items_keep_the_backdrop() {
        let movie: MovieResult = serde_json::from_value(json!({
            "provider": "tmdb", "provider_id": "1", "title": "M",
            "backdrop_url": "https://image.tmdb.org/t/p/w1280/m.jpg"
        }))
        .unwrap();
        let show: TvShowResult = serde_json::from_value(json!({
            "provider": "tmdb", "provider_id": "2", "title": "S",
            "backdrop_url": "https://image.tmdb.org/t/p/w1280/s.jpg"
        }))
        .unwrap();

        assert_eq!(
            movie_to_discover_item(movie).backdrop_url.as_deref(),
            Some("https://image.tmdb.org/t/p/w1280/m.jpg")
        );
        assert_eq!(
            tv_to_discover_item(show).backdrop_url.as_deref(),
            Some("https://image.tmdb.org/t/p/w1280/s.jpg")
        );
    }
}
