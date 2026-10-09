//! Person enrichment job handler — fetches detailed bio/dates from TMDB.

use crate::images::{ImageEntry, ImageMap};
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::people;
use crate::video_metadata::{VideoProviderRegistry, types::VideoCapability};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::macros::format_description;
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

/// Payload for person_enrich jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct PersonEnrichPayload {
    pub person_id: Uuid,
    /// When true, invalidate provider cache before fetching (manual refresh only).
    #[serde(default)]
    pub invalidate_cache: bool,
}

pub struct PersonEnrichHandler {
    pool: PgPool,
}

impl PersonEnrichHandler {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl JobHandler for PersonEnrichHandler {
    fn job_type(&self) -> &'static str {
        "person_enrich"
    }

    fn max_concurrent(&self) -> usize {
        2
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: PersonEnrichPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {e}")))?;
        nats_jobs::check_cancelled(&cancel)?;

        info!(job_id = %job_id, person_id = %payload.person_id, "Starting person enrichment");

        let result = enrich_person(&self.pool, payload.person_id, payload.invalidate_cache).await?;

        info!(job_id = %job_id, person_id = %payload.person_id, "Person enrichment complete");

        Ok(result)
    }
}

/// Shared person enrichment logic used by both the job handler and admin routes.
///
/// Fetches full TMDB details (bio, birth/death dates, profile image) and updates the person.
/// When `invalidate_cache` is true, the provider cache is cleared before fetching
/// (used for manual refresh). Automatic enrichment should pass `false`.
pub async fn enrich_person(
    pool: &PgPool,
    person_id: Uuid,
    invalidate_cache: bool,
) -> Result<Value, JobError> {
    // Resolve provider from entity identifiers
    let stored_identifiers: serde_json::Value = sqlx::query_scalar(
        "SELECT identifiers FROM people WHERE id = $1 AND identifiers IS NOT NULL",
    )
    .bind(person_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| JobError::ExecutionFailed(format!("DB error: {e}")))?
    .ok_or_else(|| {
        JobError::ExecutionFailed("Person has no identifiers, skipping enrichment".to_string())
    })?;

    let registry = VideoProviderRegistry::new(pool.clone());
    let (provider_type, provider_id) = registry
        .resolve_provider_for_identifiers(&stored_identifiers, VideoCapability::PersonDetails)
        .await
        .ok_or_else(|| {
            JobError::ExecutionFailed("No enabled provider found for person identifiers".into())
        })?;

    if invalidate_cache {
        registry
            .invalidate_person_cache(&provider_type, &provider_id)
            .await;
    }
    let details = registry
        .get_person_details(&provider_type, &provider_id)
        .await
        .map_err(|e| {
            JobError::retryable(format!(
                "Failed to fetch person details from {provider_type}: {e}"
            ))
        })?;

    // Build images map from profile URL
    let images = details.profile_url.as_ref().map(|url| {
        let mut map = ImageMap::new();
        map.insert(
            "profile".to_string(),
            ImageEntry {
                source_url: Some(url.clone()),
                cached_at: None,
            },
        );
        map
    });

    // Build merged identifiers JSON: preserve existing + add provider details
    let identifiers = {
        let mut obj = stored_identifiers.as_object().cloned().unwrap_or_default();
        // Ensure the resolved provider ID is set
        obj.insert(
            format!("{}_id", provider_type),
            Value::String(provider_id.clone()),
        );
        for (k, v) in &details.identifiers {
            obj.insert(k.clone(), Value::String(v.clone()));
        }
        Value::Object(obj)
    };

    // Parse dates
    let date_format = format_description!("[year]-[month]-[day]");
    let birth_date = details
        .birthday
        .as_deref()
        .and_then(|d| time::Date::parse(d, &date_format).ok());
    let death_date = details
        .deathday
        .as_deref()
        .and_then(|d| time::Date::parse(d, &date_format).ok());

    let update = people::PersonUpdate {
        name: Some(details.name.clone()),
        bio: details.biography.clone(),
        birth_date,
        death_date,
        identifiers: Some(identifiers),
        images,
        ..Default::default()
    };

    people::update(pool, person_id, update)
        .await
        .map_err(|e| JobError::ExecutionFailed(format!("Failed to update person: {e}")))?;

    info!(
        person_id = %person_id,
        name = %details.name,
        "Person enriched successfully"
    );

    Ok(json!({
        "person_id": person_id,
        "name": details.name,
        "has_bio": details.biography.is_some(),
    }))
}
