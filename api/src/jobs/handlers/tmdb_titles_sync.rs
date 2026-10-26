use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use crate::jobs::JobResult;
use crate::jobs::worker::JobHandler;
use crate::repositories::{tmdb_titles, video_metadata_providers};
use crate::tmdb_titles::export::{self, ExportLimits};
use crate::tmdb_titles::{TitleMedia, localize};
use crate::video_metadata::tmdb::TmdbProvider;

const LOCALIZE_EVERY: Duration = Duration::days(7);

/// Scheduled every few hours because the scheduler forgets its timers on every restart;
/// the stored export date and localization time are what actually pace the work.
pub struct TmdbTitlesSyncHandler {
    pool: PgPool,
    exports_url: String,
    limits: ExportLimits,
}

impl TmdbTitlesSyncHandler {
    pub fn new(pool: PgPool, exports_url: String, limits: ExportLimits) -> Self {
        Self {
            pool,
            exports_url,
            limits,
        }
    }

    async fn import_export(
        &self,
        media: TitleMedia,
        cancel: &CancellationToken,
    ) -> JobResult<Value> {
        let imported = tmdb_titles::sync_state(&self.pool, media)
            .await?
            .export_date;
        let today = OffsetDateTime::now_utc().date();
        let client = crate::http::new_streaming_client();
        for date in [today, today - Duration::days(1)] {
            if imported.is_some_and(|d| d >= date) {
                return Ok(json!({ "status": "current", "export_date": imported }));
            }
            let url = export::export_url(&self.exports_url, media, date);
            let Some(titles) = export::fetch(&client, &url, self.limits.of(media), cancel).await?
            else {
                continue;
            };
            tmdb_titles::replace_export(&self.pool, media, date, &titles).await?;
            info!(media = media.as_str(), %date, titles = titles.len(), "Imported TMDB export");
            return Ok(
                json!({ "status": "imported", "export_date": date, "titles": titles.len() }),
            );
        }
        Ok(json!({ "status": "unpublished" }))
    }

    async fn localize(
        &self,
        tmdb: &TmdbProvider,
        media: TitleMedia,
        cancel: &CancellationToken,
    ) -> JobResult<Value> {
        let localized_at = tmdb_titles::sync_state(&self.pool, media)
            .await?
            .localized_at;
        if localized_at.is_some_and(|at| at > OffsetDateTime::now_utc() - LOCALIZE_EVERY) {
            return Ok(json!({ "status": "current" }));
        }
        let (titles, errors) = localize::english_titles(tmdb, media, cancel).await?;
        for error in &errors {
            warn!("{error}");
        }
        tmdb_titles::apply_english_titles(&self.pool, media, &titles, errors.is_empty()).await?;
        Ok(json!({ "status": "localized", "titles": titles.len(), "errors": errors }))
    }
}

#[async_trait]
impl JobHandler for TmdbTitlesSyncHandler {
    fn job_type(&self) -> &'static str {
        "tmdb_titles_sync"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        _payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let Some(config) = video_metadata_providers::list_enabled(&self.pool)
            .await?
            .into_iter()
            .find(|p| p.provider_type == "tmdb")
            .map(|p| p.config)
        else {
            return Ok(json!({ "status": "skipped", "reason": "tmdb_disabled" }));
        };
        let tmdb = config
            .get("api_key")
            .and_then(Value::as_str)
            .is_some_and(|key| !key.is_empty())
            .then(|| TmdbProvider::from_config(crate::http::new_client(), &config));

        let mut result = serde_json::Map::new();
        for media in TitleMedia::ALL {
            let exported = self.import_export(media, &cancel).await?;
            let localized = match &tmdb {
                Some(tmdb) => self.localize(tmdb, media, &cancel).await?,
                None => json!({ "status": "skipped", "reason": "no_api_key" }),
            };
            result.insert(
                media.as_str().into(),
                json!({ "export": exported, "localize": localized }),
            );
        }
        Ok(Value::Object(result))
    }
}
