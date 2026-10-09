//! Person sync check job — auto-wants credits for monitored people.

use nats_jobs::Publisher;
use sqlx::PgPool;

use crate::jobs::JobError;
use crate::jobs::worker::JobHandler;
use crate::repositories::people;
use crate::video_metadata::{VideoProviderRegistry, types::VideoCapability};
use tokio_util::sync::CancellationToken;

pub struct PersonSyncCheckHandler {
    pool: PgPool,
    publisher: Publisher,
}

impl PersonSyncCheckHandler {
    pub fn new(pool: PgPool, publisher: Publisher) -> Self {
        Self { pool, publisher }
    }
}

#[async_trait::async_trait]
impl JobHandler for PersonSyncCheckHandler {
    fn job_type(&self) -> &'static str {
        "person_sync_check"
    }

    fn max_concurrent(&self) -> usize {
        1
    }

    async fn execute(
        &self,
        _job_id: uuid::Uuid,
        _payload: serde_json::Value,
        cancel: CancellationToken,
    ) -> Result<serde_json::Value, JobError> {
        nats_jobs::check_cancelled(&cancel)?;
        let due = people::list_due_for_sync(&self.pool, 3)
            .await
            .map_err(|e| JobError::retryable(format!("Failed to list due people: {e}")))?;

        if due.is_empty() {
            return Ok(serde_json::json!({"checked": 0}));
        }

        let registry = VideoProviderRegistry::new(self.pool.clone());
        let mut total_wanted = 0u32;

        for (i, person) in due.iter().enumerate() {
            nats_jobs::check_cancelled(&cancel)?;
            if i > 0 {
                // Rate limiting pause between people (same as author sync)
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            }

            let resolved = registry
                .resolve_provider_for_identifiers(
                    &person.identifiers,
                    VideoCapability::PersonCombinedCredits,
                )
                .await;

            let Some((provider_type, provider_id)) = resolved else {
                // No provider ID — just bump refresh to avoid re-checking
                let _ = people::bump_next_refresh(&self.pool, person.id).await;
                continue;
            };

            let credits = match registry
                .get_person_credits(&provider_type, &provider_id)
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        person_id = %person.id,
                        error = %e,
                        "Failed to fetch credits for person sync"
                    );
                    let _ = people::bump_next_refresh(&self.pool, person.id).await;
                    continue;
                }
            };

            // Only auto-want cast (acting) credits
            if person.auto_want_mode != "none" {
                let dismissed = people::list_dismissed_credits(&self.pool, person.id)
                    .await
                    .unwrap_or_default();
                let dismissed_set: std::collections::HashSet<(i32, &str)> = dismissed
                    .iter()
                    .map(|d| (d.tmdb_id, d.media_type.as_str()))
                    .collect();

                for credit in &credits.cast {
                    let provider_id: i32 = match credit.provider_id.parse() {
                        Ok(id) => id,
                        Err(_) => continue,
                    };

                    if dismissed_set.contains(&(provider_id, credit.media_type.as_str())) {
                        continue;
                    }

                    // Check if already in library
                    let id_key = format!("{}_id", provider_type);
                    let in_library = match credit.media_type.as_str() {
                        "movie" => crate::repositories::movies::get_by_identifier(
                            &self.pool,
                            &id_key,
                            &credit.provider_id,
                        )
                        .await
                        .ok()
                        .flatten()
                        .map(|m| m.is_in_library)
                        .unwrap_or(false),
                        "tv" => {
                            sqlx::query_scalar::<_, bool>(
                                "SELECT EXISTS (SELECT 1 FROM tv_seasons ts JOIN tv_episodes te ON te.season_id = ts.id JOIN tv_episode_file_episodes efe ON efe.episode_id = te.id WHERE ts.show_id = tv_shows.id) FROM tv_shows WHERE identifiers->>$2::text = $1 LIMIT 1",
                            )
                                .bind(&credit.provider_id)
                                .bind(&id_key)
                                .fetch_optional(&self.pool)
                                .await
                                .ok()
                                .flatten()
                                .unwrap_or(false)
                        }
                        _ => continue,
                    };

                    if in_library {
                        continue;
                    }

                    // Check auto_want_mode
                    let monitoring_started_date: Option<String> =
                        person.monitoring_started_at.map(|t| t.date().to_string());
                    let should_want = match person.auto_want_mode.as_str() {
                        "all" => true,
                        "new_only" => match (&credit.release_date, &monitoring_started_date) {
                            (Some(release), Some(started)) => release.as_str() > started.as_str(),
                            _ => false,
                        },
                        _ => false,
                    };

                    if !should_want {
                        continue;
                    }

                    // Create the movie/show and want it
                    let year: Option<i32> = credit
                        .release_date
                        .as_deref()
                        .and_then(|d| d.get(..4))
                        .and_then(|y| y.parse().ok());

                    match credit.media_type.as_str() {
                        "movie" => {
                            let identifiers = serde_json::json!({&id_key: credit.provider_id});
                            let movie_id = crate::repositories::movies::upsert_by_provider(
                                &self.pool,
                                &provider_type,
                                &credit.provider_id,
                                crate::repositories::movies::MovieInput {
                                    title: credit.title.clone(),
                                    year,
                                    identifiers: Some(identifiers),
                                    ..Default::default()
                                },
                            )
                            .await;

                            if let Ok(movie_id) = movie_id {
                                let _ = crate::repositories::movies::set_want_status(
                                    &self.pool, movie_id, "wanted",
                                )
                                .await;
                                let display =
                                    crate::jobs::fetch_job_display(&self.pool, "movie", movie_id)
                                        .await;
                                let context = nats_jobs::JobContext::for_entity(
                                    "movie",
                                    movie_id,
                                    format!("Auto-want: {}", credit.title),
                                )
                                .with_display_opt(display);
                                let _ = crate::jobs::enqueue(
                                    &self.pool,
                                    &self.publisher,
                                    "movie_want",
                                    serde_json::json!({"movie_id": movie_id}),
                                    context,
                                )
                                .await;
                                total_wanted += 1;
                            }
                        }
                        "tv" => {
                            let identifiers = serde_json::json!({&id_key: credit.provider_id});
                            let show_id = crate::repositories::tv_shows::upsert_by_provider(
                                &self.pool,
                                &provider_type,
                                &credit.provider_id,
                                crate::repositories::tv_shows::TvShowInput {
                                    title: credit.title.clone(),
                                    year,
                                    identifiers: Some(identifiers),
                                    ..Default::default()
                                },
                            )
                            .await;

                            if let Ok(show_id) = show_id {
                                // Enqueue TV show enrichment (which handles wanting episodes)
                                let display =
                                    crate::jobs::fetch_job_display(&self.pool, "tv_show", show_id)
                                        .await;
                                let context = nats_jobs::JobContext::for_entity(
                                    "tv_show",
                                    show_id,
                                    format!("Auto-want: {}", credit.title),
                                )
                                .with_display_opt(display);
                                let _ = crate::jobs::enqueue(
                                    &self.pool,
                                    &self.publisher,
                                    "tv_show_enrich",
                                    serde_json::json!({"show_id": show_id}),
                                    context,
                                )
                                .await;
                                total_wanted += 1;
                            }
                        }
                        _ => {}
                    }
                }
            }

            let _ = people::bump_next_refresh(&self.pool, person.id).await;
        }

        Ok(serde_json::json!({
            "checked": due.len(),
            "wanted": total_wanted,
        }))
    }
}
