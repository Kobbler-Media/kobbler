//! Media root match job handler.
//!
//! Groups identified files by inferred entity, searches providers for each
//! unique title, and creates library entities when auto_add is enabled.
//! - TV/movies: Uses VideoProviderRegistry (TMDB)
//! - Books: Uses ProviderRegistry (Hardcover, OpenLibrary, Google Books)

use crate::jobs::worker::JobHandler;
use crate::jobs::{JobContext, JobError, JobResult};
use crate::media_interpreter::InterpretResult;
use crate::metadata::similarity::{fuzzy_match, strip_punctuation};
use crate::metadata::{MetadataQuery, ProviderRegistry};
use crate::repositories::{
    audiobook_files, authors, book_files, book_series, books, media_root_files, media_roots,
    movies, series, tv_episode_files, tv_episodes, tv_seasons, tv_shows,
};
use crate::search::{SearchBackend, batch_index_media_root_files};
use crate::video_metadata::{
    MovieQuery, MovieResult, TvQuery, TvShowResult, VideoProviderRegistry, types::VideoCapability,
};
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace, warn};
use uuid::Uuid;

const BATCH_SIZE: i64 = 1000;

/// Payload for media_root_match jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRootMatchPayload {
    pub media_root_id: Uuid,
}

pub struct MediaRootMatchHandler {
    pool: PgPool,
    nats: Publisher,
    search: Arc<dyn SearchBackend>,
    library_events: tokio::sync::broadcast::Sender<crate::LibraryEvent>,
}

impl MediaRootMatchHandler {
    pub fn new(
        pool: PgPool,
        nats: Publisher,
        search: Arc<dyn SearchBackend>,
        library_events: tokio::sync::broadcast::Sender<crate::LibraryEvent>,
    ) -> Self {
        Self {
            pool,
            nats,
            search,
            library_events,
        }
    }
}

#[async_trait]
impl JobHandler for MediaRootMatchHandler {
    fn job_type(&self) -> &'static str {
        "media_root_match"
    }

    fn max_concurrent(&self) -> usize {
        3
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: MediaRootMatchPayload = serde_json::from_value(payload)?;
        nats_jobs::check_cancelled(&cancel)?;

        let root = match media_roots::get_by_id(&self.pool, payload.media_root_id).await? {
            Some(r) => r,
            None => {
                tracing::warn!(
                    "Media root {} no longer exists, skipping",
                    payload.media_root_id
                );
                return Ok(json!({"skipped": true, "reason": "media_root_deleted"}));
            }
        };

        let files = media_root_files::get_identified(&self.pool, root.id, BATCH_SIZE).await?;
        if files.is_empty() {
            return Ok(json!({"added": 0, "message": "No identified files"}));
        }

        let match_result = match root.media_type.as_str() {
            "tv" => {
                let registry = VideoProviderRegistry::new(self.pool.clone());
                match_tv_files(&self.pool, &self.nats, &registry, &root, &files).await
            }
            "movie" => {
                let registry = VideoProviderRegistry::new(self.pool.clone());
                match_movie_files(&self.pool, &self.nats, &registry, &root, &files).await
            }
            "sport" => match_sport_files(&self.pool, &self.nats, &files).await,
            "book" | "audiobook" => {
                let registry = ProviderRegistry::new(self.pool.clone());
                match_book_files(&self.pool, &self.nats, &registry, &root, &files).await
            }
            _ => {
                return Err(JobError::ExecutionFailed(format!(
                    "Unknown media type: {}",
                    root.media_type
                )));
            }
        };

        let added = match_result.added;
        let review = match_result.review;
        let ambiguous = match_result.ambiguous;
        let unmatched = match_result.unmatched;

        // Batch reindex affected files in search (best-effort)
        let file_ids: Vec<Uuid> = files.iter().map(|f| f.id).collect();
        if let Err(e) =
            batch_index_media_root_files(self.search.as_ref(), &self.pool, &file_ids).await
        {
            warn!(error = %e, "Failed to batch reindex media root files in search");
        }

        // Enqueue per-entity enrichment jobs for newly created entities
        for (entity_type, entity_id, title) in &match_result.created_entities {
            let job_type = match *entity_type {
                "tv_show" => "tv_show_enrich",
                "movie" => "movie_enrich",
                "book" => "book_enrich",
                _ => continue,
            };
            let payload_key = match *entity_type {
                "tv_show" => "show_id",
                "movie" => "movie_id",
                "book" => "book_id",
                _ => continue,
            };
            let display = crate::jobs::fetch_job_display(&self.pool, entity_type, *entity_id).await;
            let context = JobContext::for_entity(
                *entity_type,
                *entity_id,
                format!("Enriching metadata for '{}'", title),
            )
            .with_display_opt(display);
            let _ = crate::jobs::enqueue(
                &self.pool,
                &self.nats,
                job_type,
                json!({ payload_key: entity_id }),
                context,
            )
            .await;
        }

        // Enqueue search index jobs for created entities
        for (entity_type, entity_id, _title) in &match_result.created_entities {
            match *entity_type {
                "book" => {
                    crate::jobs::enqueue_book_index(&self.pool, &self.nats, *entity_id).await;
                    // Also index the book's authors and series
                    if let Ok(author_ids) =
                        authors::get_book_author_ids(&self.pool, *entity_id).await
                    {
                        for author_id in author_ids {
                            crate::jobs::enqueue_author_index(&self.pool, &self.nats, author_id)
                                .await;
                        }
                    }
                    if let Ok(Some((sid, _, _))) =
                        book_series::get_primary(&self.pool, *entity_id).await
                    {
                        crate::jobs::enqueue_series_index(&self.pool, &self.nats, sid).await;
                    }
                }
                "movie" => {
                    crate::jobs::enqueue_movie_index(&self.pool, &self.nats, *entity_id).await;
                }
                "tv_show" => {
                    crate::jobs::enqueue_tv_show_index(&self.pool, &self.nats, *entity_id).await;
                }
                _ => {}
            }
        }

        // Also enqueue media_root_enrich as catch-up orchestrator
        if added > 0 {
            let display = crate::jobs::fetch_job_display(&self.pool, "media_root", root.id).await;
            let context = JobContext::for_entity(
                "media_root",
                root.id,
                format!("Enriching metadata for '{}'", root.name),
            )
            .with_display_opt(display);
            let _ = crate::jobs::enqueue_routine(
                &self.pool,
                &self.nats,
                "media_root_enrich",
                json!({"media_root_id": root.id}),
                context,
            )
            .await;

            // Notify frontend so sidebar counts refresh
            let _ = self
                .library_events
                .send(crate::LibraryEvent::library_updated(None));
        }

        info!(
            root = %root.name,
            added = added,
            review = review,
            ambiguous = ambiguous,
            unmatched = unmatched,
            "Media root matching complete"
        );

        // If more identified files remain, enqueue another match batch
        let remaining = media_root_files::get_identified(&self.pool, root.id, 1).await?;
        if !remaining.is_empty() {
            let display = crate::jobs::fetch_job_display(&self.pool, "media_root", root.id).await;
            let context = JobContext::for_entity(
                "media_root",
                root.id,
                format!("Matching more files in '{}'", root.name),
            )
            .with_display_opt(display);
            let _ = crate::jobs::enqueue_routine(
                &self.pool,
                &self.nats,
                "media_root_match",
                json!({"media_root_id": root.id}),
                context,
            )
            .await;
        }

        Ok(json!({
            "added": added,
            "review": review,
            "ambiguous": ambiguous,
            "unmatched": unmatched,
        }))
    }
}

struct MatchResult {
    added: u64,
    review: u64,
    ambiguous: u64,
    unmatched: u64,
    /// Entity IDs created during matching (for per-entity enrichment jobs).
    /// Tuples of (entity_type, entity_id, title) for enqueue_with_context.
    created_entities: Vec<(&'static str, Uuid, String)>,
}

/// Match TV files by grouping by show name, searching providers, creating entities.
async fn match_tv_files(
    pool: &PgPool,
    nats: &Publisher,
    registry: &VideoProviderRegistry,
    root: &media_roots::MediaRootRow,
    files: &[media_root_files::MediaRootFileRow],
) -> MatchResult {
    let mut result = MatchResult {
        added: 0,
        review: 0,
        ambiguous: 0,
        unmatched: 0,
        created_entities: Vec::new(),
    };

    let preferred = registry
        .preferred_provider(VideoCapability::TvSearch)
        .await
        .unwrap_or_else(|| "tmdb".to_string());

    // Group files by (show name, show year) — year prevents revivals from merging
    // with originals (e.g. "Scrubs" vs "Scrubs (2026)").
    let mut show_groups: HashMap<(String, Option<u32>), Vec<&media_root_files::MediaRootFileRow>> =
        HashMap::new();
    for file in files {
        if let Some(interpretation) = &file.interpretation {
            let interp: InterpretResult = match serde_json::from_value(interpretation.clone()) {
                Ok(i) => i,
                Err(_) => continue,
            };
            if let Some(show_name) = &interp.show_name {
                show_groups
                    .entry((show_name.to_lowercase(), interp.show_year))
                    .or_default()
                    .push(file);
            } else {
                // No show name - mark unmatched
                let _ = media_root_files::set_unmatched(pool, file.id).await;
                result.unmatched += 1;
            }
        }
    }

    // For each unique show, find or create the show entity
    for ((_, group_year), group_files) in &show_groups {
        let sample_interp: InterpretResult =
            match serde_json::from_value(group_files[0].interpretation.clone().unwrap_or_default())
            {
                Ok(i) => i,
                Err(_) => continue,
            };

        // Extract sidecar IDs before consuming the struct
        let sidecar_tmdb_id = extract_sidecar_id(&sample_interp, "tmdb_id");
        let sidecar_imdb_id = extract_sidecar_id(&sample_interp, "imdb_id");
        let show_name = sample_interp.show_name.unwrap_or_default();

        // IMDB→TMDB cross-lookup if we have IMDB but not TMDB
        let resolved_tmdb_id = if sidecar_tmdb_id.is_some() {
            sidecar_tmdb_id
        } else if let Some(imdb_id) = &sidecar_imdb_id {
            match registry.find_by_imdb_id(imdb_id).await {
                Ok((_, tv_ids)) => {
                    if let Some(id) = tv_ids.into_iter().next() {
                        info!(imdb_id = %imdb_id, tmdb_id = %id, "Resolved IMDB→TMDB for TV show");
                        Some(id)
                    } else {
                        None
                    }
                }
                Err(e) => {
                    debug!(imdb_id = %imdb_id, error = %e, "IMDB→TMDB cross-lookup failed");
                    None
                }
            }
        } else {
            None
        };

        // Search for show in DB first (case-insensitive, year-aware)
        let existing = find_existing_tv_show(pool, &show_name, *group_year).await;

        let show_id = if let Some(id) = existing {
            id
        } else {
            // Try sidecar TMDB ID for direct lookup, fall back to title search
            let (search_results, api_called) = if let Some(tmdb_id) = &resolved_tmdb_id {
                info!(show = %show_name, tmdb_id = %tmdb_id, "Using sidecar TMDB ID for TV show lookup");
                match registry.get_tv_show("tmdb", tmdb_id).await {
                    Ok(details) => (
                        vec![TvShowResult {
                            provider: "tmdb".into(),
                            provider_id: tmdb_id.clone(),
                            title: details.title,
                            year: details.year,
                            status: details.status,
                            description: details.description,
                            poster_url: details.poster_url,
                            backdrop_url: details.backdrop_url,
                            network: details.network,
                            rating: details.rating,
                            rating_count: details.rating_count,
                            identifiers: details.identifiers,
                            genres: details.genres,
                        }],
                        true,
                    ), // get_tv_show may or may not hit cache; be conservative
                    Err(e) => {
                        warn!(tmdb_id = %tmdb_id, error = %e, "Sidecar TMDB lookup failed, falling back to search");
                        let query = TvQuery {
                            title: show_name.clone(),
                            ..Default::default()
                        };
                        let outcome = registry.search_tv(&query, Some(&preferred)).await;
                        (outcome.results, !outcome.all_cached)
                    }
                }
            } else {
                let query = TvQuery {
                    title: show_name.clone(),
                    ..Default::default()
                };
                let outcome = registry.search_tv(&query, Some(&preferred)).await;
                (outcome.results, !outcome.all_cached)
            };

            // Rate limit between provider calls (skip if all results came from cache)
            if api_called {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }

            // (rest of the existing search_results handling follows)

            if search_results.is_empty() {
                // No results - mark all files as unmatched
                for file in group_files {
                    let _ = media_root_files::set_unmatched(pool, file.id).await;
                    result.unmatched += 1;
                }
                continue;
            }

            if search_results.len() > 1 {
                let top = &search_results[0];
                let title_match = fuzzy_match(
                    &strip_punctuation(&top.title.to_lowercase()),
                    &strip_punctuation(&show_name.to_lowercase()),
                );

                if !title_match {
                    // Ambiguous - store candidates on all files
                    let candidates = json!(
                        search_results
                            .iter()
                            .take(5)
                            .map(|r| json!({
                                "provider": r.provider,
                                "provider_id": r.provider_id,
                                "title": r.title,
                                "year": r.year,
                                "poster_url": r.poster_url,
                            }))
                            .collect::<Vec<_>>()
                    );

                    for file in group_files {
                        let _ = media_root_files::set_ambiguous(pool, file.id, &candidates).await;
                        result.ambiguous += 1;
                    }
                    continue;
                }
            }

            let top = &search_results[0];

            if !root.auto_add {
                // Selective mode: confident match, awaiting user confirmation
                for file in group_files {
                    let candidate = json!([{
                        "provider": top.provider,
                        "provider_id": top.provider_id,
                        "title": top.title,
                        "year": top.year,
                        "poster_url": top.poster_url,
                    }]);
                    let _ = media_root_files::set_review(pool, file.id, &candidate).await;
                    result.review += 1;
                }
                continue;
            }

            // Auto-add mode: create the show
            let id_key = format!("{}_id", top.provider);
            let show_title = top.title.clone();
            let show_input = tv_shows::TvShowInput {
                title: show_title.clone(),
                year: top.year.map(|y| y as i32),
                images: None,
                identifiers: Some(json!({&id_key: top.provider_id})),
                ..Default::default()
            };

            match tv_shows::create(pool, show_input).await {
                Ok(id) => {
                    // Set media_root_id on the show
                    let _ = sqlx::query("UPDATE tv_shows SET media_root_id = $1 WHERE id = $2")
                        .bind(root.id)
                        .bind(id)
                        .execute(pool)
                        .await;
                    result.created_entities.push(("tv_show", id, show_title));
                    id
                }
                Err(e) => {
                    warn!(show = %show_name, error = %e, "Failed to create TV show");
                    for file in group_files {
                        let _ = media_root_files::set_unmatched(pool, file.id).await;
                        result.unmatched += 1;
                    }
                    continue;
                }
            }
        };

        // Now match each file to episodes, collecting batch updates
        let mut added_updates: Vec<(Uuid, &str, Uuid)> = Vec::new();
        // Cache season IDs to avoid redundant DB calls within this show group
        let mut season_cache: HashMap<u32, Uuid> = HashMap::new();

        for file in group_files {
            let interp: InterpretResult =
                match serde_json::from_value(file.interpretation.clone().unwrap_or_default()) {
                    Ok(i) => i,
                    Err(_) => continue,
                };

            if let (Some(season_num), episodes) = (interp.season, &interp.episodes) {
                if episodes.is_empty() {
                    let _ = media_root_files::set_unmatched(pool, file.id).await;
                    result.unmatched += 1;
                    continue;
                }

                // Find or create season (cached within show group)
                let season_id = if let Some(&cached) = season_cache.get(&season_num) {
                    cached
                } else {
                    match find_or_create_season(pool, show_id, season_num).await {
                        Ok(id) => {
                            season_cache.insert(season_num, id);
                            id
                        }
                        Err(e) => {
                            warn!(error = %e, "Failed to find/create season");
                            let _ = media_root_files::set_unmatched(pool, file.id).await;
                            result.unmatched += 1;
                            continue;
                        }
                    }
                };

                // Find or create all episodes (single upsert per episode)
                let mut episode_ids = Vec::new();
                let mut ep_failed = false;
                for &ep_num in episodes {
                    let identifier = ep_num.to_string();
                    match tv_episodes::find_or_create(
                        pool,
                        season_id,
                        &identifier,
                        ep_num as i32,
                        Some(&format!("Episode {}", ep_num)),
                    )
                    .await
                    {
                        Ok(id) => episode_ids.push(id),
                        Err(e) => {
                            warn!(error = %e, ep_num, "Failed to find/create episode");
                            ep_failed = true;
                            break;
                        }
                    }
                }
                if ep_failed || episode_ids.is_empty() {
                    let _ = media_root_files::set_unmatched(pool, file.id).await;
                    result.unmatched += 1;
                    continue;
                }

                // Create episode file record and enqueue probe
                if let Some(ep_file_id) =
                    create_episode_file(pool, &file.file_path, file.file_fingerprint.as_deref())
                        .await
                {
                    match tv_episode_files::link_episodes(pool, ep_file_id, &episode_ids).await {
                        Ok(replaced) => {
                            crate::repositories::cleanup_replaced_files(&replaced).await
                        }
                        Err(e) => warn!(error = %e, "Failed to link episodes to file"),
                    }
                    enqueue_media_probe(pool, nats, ep_file_id, "episode").await;
                }

                // Collect for batch updates
                added_updates.push((file.id, "tv_episode", episode_ids[0]));
                result.added += 1;
            } else {
                let _ = media_root_files::set_unmatched(pool, file.id).await;
                result.unmatched += 1;
            }
        }

        // Batch: mark media_root_files as added
        if !added_updates.is_empty()
            && let Err(e) = media_root_files::set_added_batch(pool, &added_updates).await
        {
            warn!(error = %e, "Failed to batch set files as added");
        }
    }

    result
}

/// Match sport files against existing sport leagues in the DB.
///
/// Unlike TV/movie matching, sport matching does NOT search external providers
/// or auto-create leagues. Leagues must already exist (added via TheSportsDB).
/// Files are matched by league name → season year → round+session episode identifier.
async fn match_sport_files(
    pool: &PgPool,
    nats: &Publisher,
    files: &[media_root_files::MediaRootFileRow],
) -> MatchResult {
    let mut result = MatchResult {
        added: 0,
        review: 0,
        ambiguous: 0,
        unmatched: 0,
        created_entities: Vec::new(),
    };

    // Group files by league name (show_name in InterpretResult)
    let mut league_groups: HashMap<String, Vec<&media_root_files::MediaRootFileRow>> =
        HashMap::new();
    for file in files {
        if let Some(interpretation) = &file.interpretation {
            let interp: InterpretResult = match serde_json::from_value(interpretation.clone()) {
                Ok(i) => i,
                Err(_) => continue,
            };
            if let Some(show_name) = &interp.show_name {
                league_groups
                    .entry(show_name.to_lowercase())
                    .or_default()
                    .push(file);
            } else {
                let _ = media_root_files::set_unmatched(pool, file.id).await;
                result.unmatched += 1;
            }
        }
    }

    for group_files in league_groups.values() {
        let sample_interp: InterpretResult =
            match serde_json::from_value(group_files[0].interpretation.clone().unwrap_or_default())
            {
                Ok(i) => i,
                Err(_) => continue,
            };
        let league_name = sample_interp.show_name.unwrap_or_default();

        // Find existing sport league in DB
        let show_id = match find_existing_sport_league(pool, &league_name).await {
            Some(id) => id,
            None => {
                info!(league = %league_name, "No matching sport league in DB, marking unmatched");
                for file in group_files {
                    let _ = media_root_files::set_unmatched(pool, file.id).await;
                    result.unmatched += 1;
                }
                continue;
            }
        };

        // Pre-fetch all seasons for this league
        let seasons = match tv_seasons::list_by_show(pool, show_id).await {
            Ok(s) => s,
            Err(e) => {
                warn!(show_id = %show_id, error = %e, "Failed to list seasons");
                for file in group_files {
                    let _ = media_root_files::set_unmatched(pool, file.id).await;
                    result.unmatched += 1;
                }
                continue;
            }
        };

        let mut added_updates: Vec<(Uuid, &str, Uuid)> = Vec::new();
        // Cache episodes per season to avoid refetching
        let mut season_episodes: HashMap<Uuid, Vec<tv_episodes::EpisodeRow>> = HashMap::new();

        for file in group_files {
            let interp: InterpretResult =
                match serde_json::from_value(file.interpretation.clone().unwrap_or_default()) {
                    Ok(i) => i,
                    Err(_) => continue,
                };

            // Extract sport fields from typed ParsedRelease
            let sport = match interp.raw.as_ref().and_then(|r| r.media.as_sport()) {
                Some(s) => s,
                None => {
                    let _ = media_root_files::set_unmatched(pool, file.id).await;
                    result.unmatched += 1;
                    continue;
                }
            };

            let round = sport.round;
            let raw_session = sport.session_type.as_deref();
            let event_title = sport.event_title.as_deref();
            let season_year = interp.season.or(interp.show_year).or(sport.year);

            // Find matching season by year
            let season_id = season_year.and_then(|year| {
                let year_str = year.to_string();
                seasons
                    .iter()
                    .find(|s| s.season_identifier == year_str)
                    .map(|s| s.id)
            });

            let season_id = match season_id {
                Some(id) => id,
                None => {
                    debug!(
                        file = %file.file_name,
                        season = ?season_year,
                        "No matching season for sport file"
                    );
                    let _ = media_root_files::set_unmatched(pool, file.id).await;
                    result.unmatched += 1;
                    continue;
                }
            };

            // Pre-fetch episodes for the season (cached across files in this league group)
            let all_episodes = if let Some(eps) = season_episodes.get(&season_id) {
                eps
            } else {
                match tv_episodes::list_by_season(pool, season_id).await {
                    Ok(eps) => {
                        season_episodes.insert(season_id, eps);
                        season_episodes.get(&season_id).unwrap()
                    }
                    Err(_) => {
                        let _ = media_root_files::set_unmatched(pool, file.id).await;
                        result.unmatched += 1;
                        continue;
                    }
                }
            };

            let episode_id = match_sport_episode(round, raw_session, event_title, all_episodes);

            match episode_id {
                Some(episode_id) => {
                    if let Some(ep_file_id) =
                        create_episode_file(pool, &file.file_path, file.file_fingerprint.as_deref())
                            .await
                    {
                        match tv_episode_files::link_episodes(pool, ep_file_id, &[episode_id]).await
                        {
                            Ok(replaced) => {
                                crate::repositories::cleanup_replaced_files(&replaced).await
                            }
                            Err(e) => warn!(error = %e, "Failed to link episode to file"),
                        }
                        enqueue_media_probe(pool, nats, ep_file_id, "episode").await;
                    }
                    added_updates.push((file.id, "tv_episode", episode_id));
                    result.added += 1;
                }
                None => {
                    debug!(
                        file = %file.file_name,
                        round = ?round,
                        session = ?raw_session,
                        "No matching episode for sport file"
                    );
                    let _ = media_root_files::set_unmatched(pool, file.id).await;
                    result.unmatched += 1;
                }
            }
        }

        // Batch: mark media_root_files as added
        if !added_updates.is_empty()
            && let Err(e) = media_root_files::set_added_batch(pool, &added_updates).await
        {
            warn!(error = %e, "Failed to batch set sport files as added");
        }
    }

    result
}

/// Session alias map for fuzzy matching sport files to episodes.
/// Maps abbreviated session names to their full TheSportsDB equivalents.
pub const SESSION_ALIASES: &[(&str, &str)] = &[
    ("fp1", "free practice 1"),
    ("fp2", "free practice 2"),
    ("fp3", "free practice 3"),
    ("fp", "free practice"),
    ("q1", "qualifying 1"),
    ("q2", "qualifying 2"),
    ("q", "qualifying"),
    ("qual", "qualifying"),
    ("sprint race", "sprint race"),
    ("warmup", "warm up"),
    ("warm-up", "warm up"),
];

/// Match a sport file to an episode using multiple strategies.
///
/// Supports:
/// - Motorsport files (round + session_type): match by round range + session fuzzy match
/// - Team sport files (event_title with "vs"/"v"): fuzzy match across season
/// - Fallback: if only one episode in a round, auto-match
pub fn match_sport_episode(
    round: Option<u32>,
    session_type: Option<&str>,
    event_title: Option<&str>,
    episodes: &[tv_episodes::EpisodeRow],
) -> Option<Uuid> {
    // Strategy 1: Motorsport — round + session matching
    if let (Some(round), Some(session)) = (round, session_type) {
        let session_lower = session.to_lowercase();

        // Resolve session aliases
        let canonical_session = SESSION_ALIASES
            .iter()
            .find(|(alias, _)| *alias == session_lower)
            .map(|(_, full)| *full)
            .unwrap_or(&session_lower);

        // Filter episodes to target round — check both new (R*1000) and old (R*100) formats
        let round_episodes: Vec<&tv_episodes::EpisodeRow> = episodes
            .iter()
            .filter(|e| {
                let order = e.episode_sort_order as u32;
                // New format: round * 1000 .. round * 1000 + 999
                let new_start = round * 1000;
                let new_end = new_start + 999;
                // Old format: round * 100 .. round * 100 + 99
                let old_start = round * 100;
                let old_end = old_start + 99;
                (order >= new_start && order <= new_end) || (order >= old_start && order <= old_end)
            })
            .collect();

        // Try to match session against episode title (prefer exact suffix over containment)
        // Pass 1: exact suffix — prefer shortest title so "Race" matches "Thailand Race" not "Thailand Sprint Race"
        if let Some(ep) = round_episodes
            .iter()
            .filter(|e| {
                let ep_title = e.title.as_deref().unwrap_or("").to_lowercase();
                ep_title.ends_with(canonical_session)
                    || ep_title.ends_with(session_lower.as_str())
                    || e.episode_identifier
                        .to_lowercase()
                        .ends_with(session_lower.as_str())
            })
            .min_by_key(|e| e.title.as_deref().unwrap_or("").len())
        {
            return Some(ep.id);
        }
        // Pass 2: containment fallback for unusual title formats
        if let Some(ep) = round_episodes.iter().find(|e| {
            let ep_title = e.title.as_deref().unwrap_or("").to_lowercase();
            ep_title.contains(canonical_session)
                || ep_title.contains(&session_lower)
                || e.episode_identifier.to_lowercase().contains(&session_lower)
        }) {
            return Some(ep.id);
        }

        // Fallback: if only one episode in the round, auto-match
        if round_episodes.len() == 1 {
            return Some(round_episodes[0].id);
        }
    }

    // Strategy 2: Team sport — event_title fuzzy match
    if let Some(title) = event_title {
        let title_lower = title.to_lowercase();

        // Direct match: episode title contains the event title
        if let Some(ep) = episodes.iter().find(|e| {
            e.title
                .as_deref()
                .map(|t| t.to_lowercase() == title_lower)
                .unwrap_or(false)
        }) {
            return Some(ep.id);
        }

        // Partial match: episode title contains key parts of event title
        if let Some(ep) = episodes.iter().find(|e| {
            let ep_title = e.title.as_deref().unwrap_or("").to_lowercase();
            // For "Arsenal vs Chelsea", check if episode contains both team names
            if title_lower.contains(" vs ") || title_lower.contains(" v ") {
                let parts: Vec<&str> = title_lower
                    .split([' '])
                    .filter(|p| *p != "vs" && *p != "v" && !p.is_empty())
                    .collect();
                parts.iter().all(|part| ep_title.contains(part))
            } else {
                ep_title.contains(&title_lower)
            }
        }) {
            return Some(ep.id);
        }
    }

    // Strategy 3: Round-only fallback (e.g., combat sports with round = event number)
    if let Some(round) = round {
        let round_episodes: Vec<&tv_episodes::EpisodeRow> = episodes
            .iter()
            .filter(|e| {
                let order = e.episode_sort_order as u32;
                let new_start = round * 1000;
                let new_end = new_start + 999;
                let old_start = round * 100;
                let old_end = old_start + 99;
                (order >= new_start && order <= new_end) || (order >= old_start && order <= old_end)
            })
            .collect();
        if round_episodes.len() == 1 {
            return Some(round_episodes[0].id);
        }
    }

    None
}

/// Find an existing sport league by title (case-insensitive, content_category='sport').
///
/// Titles are not unique, so the ordering is load-bearing: without a total order
/// a re-run of the matcher can split one release across two show records.
pub async fn find_existing_sport_league(pool: &PgPool, name: &str) -> Option<Uuid> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM tv_shows WHERE LOWER(title) = LOWER($1) AND content_category = 'sport' \
         ORDER BY created_at ASC, id ASC LIMIT 1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

/// Match movie files by searching providers for each unique title.
async fn match_movie_files(
    pool: &PgPool,
    nats: &Publisher,
    registry: &VideoProviderRegistry,
    root: &media_roots::MediaRootRow,
    files: &[media_root_files::MediaRootFileRow],
) -> MatchResult {
    let mut result = MatchResult {
        added: 0,
        review: 0,
        ambiguous: 0,
        unmatched: 0,
        created_entities: Vec::new(),
    };

    let preferred = registry
        .preferred_provider(VideoCapability::MovieSearch)
        .await
        .unwrap_or_else(|| "tmdb".to_string());

    // Group by movie title + year
    let mut movie_groups: HashMap<String, Vec<&media_root_files::MediaRootFileRow>> =
        HashMap::new();
    for file in files {
        if let Some(interpretation) = &file.interpretation {
            let interp: InterpretResult = match serde_json::from_value(interpretation.clone()) {
                Ok(i) => i,
                Err(_) => continue,
            };
            let key = format!(
                "{}|{}",
                interp.movie_title.as_deref().unwrap_or(""),
                interp.movie_year.unwrap_or(0)
            );
            if interp.movie_title.is_some() {
                movie_groups.entry(key).or_default().push(file);
            } else {
                let _ = media_root_files::set_unmatched(pool, file.id).await;
                result.unmatched += 1;
            }
        }
    }

    for group_files in movie_groups.values() {
        let sample_interp: InterpretResult =
            match serde_json::from_value(group_files[0].interpretation.clone().unwrap_or_default())
            {
                Ok(i) => i,
                Err(_) => continue,
            };

        // Extract sidecar IDs before consuming the struct
        let sidecar_tmdb_id = extract_sidecar_id(&sample_interp, "tmdb_id");
        let sidecar_imdb_id = extract_sidecar_id(&sample_interp, "imdb_id");
        let title = sample_interp.movie_title.unwrap_or_default();
        let year = sample_interp.movie_year;

        // IMDB→TMDB cross-lookup if we have IMDB but not TMDB
        let resolved_tmdb_id = if sidecar_tmdb_id.is_some() {
            sidecar_tmdb_id
        } else if let Some(imdb_id) = &sidecar_imdb_id {
            match registry.find_by_imdb_id(imdb_id).await {
                Ok((movie_ids, _)) => {
                    if let Some(id) = movie_ids.into_iter().next() {
                        info!(imdb_id = %imdb_id, tmdb_id = %id, "Resolved IMDB→TMDB for movie");
                        Some(id)
                    } else {
                        None
                    }
                }
                Err(e) => {
                    debug!(imdb_id = %imdb_id, error = %e, "IMDB→TMDB cross-lookup failed");
                    None
                }
            }
        } else {
            None
        };

        // Search existing movies by title + year
        let existing = find_existing_movie(pool, &title, year, None, None).await;

        let movie_id = if let Some(id) = existing {
            id
        } else {
            // Try sidecar TMDB ID for direct lookup, fall back to title search
            let (search_results, api_called) = if let Some(tmdb_id) = &resolved_tmdb_id {
                info!(movie = %title, tmdb_id = %tmdb_id, "Using sidecar TMDB ID for movie lookup");
                match registry.get_movie("tmdb", tmdb_id).await {
                    Ok(details) => (
                        vec![MovieResult {
                            provider: "tmdb".into(),
                            provider_id: tmdb_id.clone(),
                            title: details.title,
                            year: details.year,
                            description: details.description,
                            poster_url: details.poster_url,
                            backdrop_url: details.backdrop_url,
                            rating: details.rating,
                            rating_count: details.rating_count,
                            identifiers: details.identifiers,
                            genres: details.genres,
                            network: None,
                        }],
                        true,
                    ), // get_movie may or may not hit cache; be conservative
                    Err(e) => {
                        warn!(tmdb_id = %tmdb_id, error = %e, "Sidecar TMDB lookup failed, falling back to search");
                        let query = MovieQuery {
                            title: title.clone(),
                            year: year.map(|y| y as u16),
                        };
                        let outcome = registry.search_movies(&query, Some(&preferred)).await;
                        (outcome.results, !outcome.all_cached)
                    }
                }
            } else {
                let query = MovieQuery {
                    title: title.clone(),
                    year: year.map(|y| y as u16),
                };
                let outcome = registry.search_movies(&query, Some(&preferred)).await;
                (outcome.results, !outcome.all_cached)
            };

            // Rate limit between provider calls (skip if all results came from cache)
            if api_called {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }

            if search_results.is_empty() {
                for file in group_files {
                    let _ = media_root_files::set_unmatched(pool, file.id).await;
                    result.unmatched += 1;
                }
                continue;
            }

            if search_results.len() > 1 {
                let top = &search_results[0];
                let title_match = fuzzy_match(
                    &strip_punctuation(&top.title.to_lowercase()),
                    &strip_punctuation(&title.to_lowercase()),
                );

                if !title_match {
                    let candidates = json!(
                        search_results
                            .iter()
                            .take(5)
                            .map(|r| json!({
                                "provider": r.provider,
                                "provider_id": r.provider_id,
                                "title": r.title,
                                "year": r.year,
                                "poster_url": r.poster_url,
                            }))
                            .collect::<Vec<_>>()
                    );

                    for file in group_files {
                        let _ = media_root_files::set_ambiguous(pool, file.id, &candidates).await;
                        result.ambiguous += 1;
                    }
                    continue;
                }
            }

            let top = &search_results[0];

            if !root.auto_add {
                // Confident match, awaiting user confirmation
                for file in group_files {
                    let candidate = json!([{
                        "provider": top.provider,
                        "provider_id": top.provider_id,
                        "title": top.title,
                        "year": top.year,
                        "poster_url": top.poster_url,
                    }]);
                    let _ = media_root_files::set_review(pool, file.id, &candidate).await;
                    result.review += 1;
                }
                continue;
            }

            // Find or create movie (check by provider ID to prevent duplicates)
            let id_key = format!("{}_id", top.provider);
            let movie_title = top.title.clone();

            if let Some(existing_id) = find_existing_movie(
                pool,
                &top.title,
                top.year.map(|y| y as u32),
                Some(&top.provider),
                Some(&top.provider_id),
            )
            .await
            {
                existing_id
            } else {
                let movie_input = movies::MovieInput {
                    title: movie_title.clone(),
                    year: top.year.map(|y| y as i32),
                    identifiers: Some(json!({&id_key: top.provider_id})),
                    ..Default::default()
                };

                match movies::create(pool, movie_input).await {
                    Ok(id) => {
                        let _ = sqlx::query("UPDATE movies SET media_root_id = $1 WHERE id = $2")
                            .bind(root.id)
                            .bind(id)
                            .execute(pool)
                            .await;
                        result.created_entities.push(("movie", id, movie_title));
                        id
                    }
                    Err(e) => {
                        // If creation failed due to unique constraint, try finding again
                        if let Some(existing_id) = find_existing_movie(
                            pool,
                            &top.title,
                            top.year.map(|y| y as u32),
                            Some(&top.provider),
                            Some(&top.provider_id),
                        )
                        .await
                        {
                            existing_id
                        } else {
                            warn!(movie = %title, error = %e, "Failed to create movie");
                            for file in group_files {
                                let _ = media_root_files::set_unmatched(pool, file.id).await;
                                result.unmatched += 1;
                            }
                            continue;
                        }
                    }
                }
            }
        };

        // Link files to movie
        let mut added_updates = Vec::new();
        for file in group_files {
            if let Some(mf_id) = create_movie_file(
                pool,
                movie_id,
                &file.file_path,
                file.file_fingerprint.as_deref(),
            )
            .await
            {
                enqueue_media_probe(pool, nats, mf_id, "movie").await;
            }
            added_updates.push((file.id, "movie", movie_id));
            result.added += 1;
        }

        // Batch: mark media_root_files as added
        if !added_updates.is_empty()
            && let Err(e) = media_root_files::set_added_batch(pool, &added_updates).await
        {
            warn!(error = %e, "Failed to batch set movie files as added");
        }
    }

    result
}

/// Metadata extracted from a group of files for matching.
struct GroupMetadata {
    title: String,
    isbn: Option<String>,
    authors: Vec<String>,
    narrator: Option<String>,
    abridged: bool,
    asin: Option<String>,
    is_generic_filename: bool,
    is_title_conflict: bool,
    series: Option<String>,
    series_index: Option<String>,
}

/// Extract metadata from a single sample interpretation (used for book roots and
/// standalone audiobook files that aren't directory-grouped).
fn extract_sample_metadata(sample: &serde_json::Value) -> GroupMetadata {
    let title = sample
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let isbn = sample
        .get("isbn")
        .and_then(|v| v.as_str())
        .map(String::from);
    let authors: Vec<String> = sample
        .get("authors")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let narrator = sample
        .get("narrator")
        .and_then(|v| v.as_str())
        .map(String::from);
    let abridged = sample
        .get("is_abridged")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let asin = sample
        .get("asin")
        .and_then(|v| v.as_str())
        .map(String::from);
    let is_generic = sample
        .get("is_generic_filename")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let is_conflict = sample
        .get("is_title_conflict")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let series = sample
        .get("series")
        .and_then(|v| v.as_str())
        .map(String::from);
    let series_index = sample.get("series_index").and_then(|v| match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    });
    GroupMetadata {
        title,
        isbn,
        authors,
        narrator,
        abridged,
        asin,
        is_generic_filename: is_generic,
        is_title_conflict: is_conflict,
        series,
        series_index,
    }
}

/// Pick the best metadata across all files in a directory-grouped audiobook.
/// Prefers probe_title (album tag) over filename-derived title, first non-empty
/// authors, and merges narrator/asin/abridged across files.
fn best_audiobook_group_metadata(files: &[&media_root_files::MediaRootFileRow]) -> GroupMetadata {
    let mut best_title = String::new();
    let mut best_isbn: Option<String> = None;
    let mut best_authors: Vec<String> = Vec::new();
    let mut best_narrator: Option<String> = None;
    let mut any_abridged = false;
    let mut best_asin: Option<String> = None;
    let mut best_series: Option<String> = None;
    let mut best_series_index: Option<String> = None;
    let mut all_generic = true;
    let mut used_probe_title = false;

    for file in files {
        let Some(interp) = &file.interpretation else {
            continue;
        };

        // Best title: prefer probe_title (album tag = book title for multi-file audiobooks)
        if !used_probe_title
            && let Some(pt) = interp.get("probe_title").and_then(|v| v.as_str())
            && !pt.is_empty()
        {
            best_title = pt.to_string();
            used_probe_title = true;
        }
        if best_title.is_empty()
            && let Some(t) = interp.get("title").and_then(|v| v.as_str())
            && !t.is_empty()
        {
            best_title = t.to_string();
        }

        // First non-empty ISBN
        if best_isbn.is_none() {
            best_isbn = interp
                .get("isbn")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);
        }

        // First non-empty authors
        if best_authors.is_empty()
            && let Some(arr) = interp.get("authors").and_then(|v| v.as_array())
        {
            let authors: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            if !authors.is_empty() {
                best_authors = authors;
            }
        }

        // First non-empty narrator
        if best_narrator.is_none() {
            best_narrator = interp
                .get("narrator")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);
        }

        // OR of is_abridged
        if interp
            .get("is_abridged")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            any_abridged = true;
        }

        // First non-empty ASIN
        if best_asin.is_none() {
            best_asin = interp
                .get("asin")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);
        }

        // First non-empty series
        if best_series.is_none() {
            best_series = interp
                .get("series")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);
        }
        if best_series_index.is_none() {
            best_series_index = interp.get("series_index").and_then(|v| match v {
                serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
                serde_json::Value::Number(n) => Some(n.to_string()),
                _ => None,
            });
        }

        // is_generic_filename: true only if ALL files have it
        if !interp
            .get("is_generic_filename")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            all_generic = false;
        }
    }

    // If we picked a probe_title, the title conflict is resolved
    let is_conflict = !used_probe_title
        && files.iter().any(|f| {
            f.interpretation
                .as_ref()
                .and_then(|i| i.get("is_title_conflict"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        });

    GroupMetadata {
        title: best_title,
        isbn: best_isbn,
        authors: best_authors,
        narrator: best_narrator,
        abridged: any_abridged,
        asin: best_asin,
        is_generic_filename: all_generic,
        is_title_conflict: is_conflict,
        series: best_series,
        series_index: best_series_index,
    }
}

async fn match_book_files(
    pool: &PgPool,
    nats: &Publisher,
    registry: &ProviderRegistry,
    root: &media_roots::MediaRootRow,
    files: &[media_root_files::MediaRootFileRow],
) -> MatchResult {
    let mut result = MatchResult {
        added: 0,
        review: 0,
        ambiguous: 0,
        unmatched: 0,
        created_entities: Vec::new(),
    };

    // Group files by title+author from interpretation.
    // For audiobook roots, group by parent directory instead — multi-file audiobooks
    // (MP3 chapters) share a directory but have different per-track titles.
    // Standalone files directly in the root fall back to title|author grouping.
    let is_audiobook_root = root.media_type == "audiobook";
    let root_path = std::path::Path::new(&root.path);
    let mut groups: HashMap<String, Vec<&media_root_files::MediaRootFileRow>> = HashMap::new();
    for file in files {
        if let Some(interp) = &file.interpretation {
            let title = interp.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let isbn = interp.get("isbn").and_then(|v| v.as_str()).unwrap_or("");
            let author = interp
                .get("authors")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // Files with ISBN get their own group (highest confidence match)
            let key = if !isbn.is_empty() {
                format!("isbn:{}", isbn.to_lowercase())
            } else if is_audiobook_root {
                // Group by parent directory for multi-file audiobooks.
                // Files directly in the root dir fall back to title|author.
                let parent = std::path::Path::new(&file.file_path).parent();
                let is_root_level = parent.is_none_or(|p| p == root_path);
                if is_root_level {
                    if !title.is_empty() {
                        format!("{}|{}", title.to_lowercase(), author.to_lowercase())
                    } else {
                        let _ = media_root_files::set_unmatched(pool, file.id).await;
                        result.unmatched += 1;
                        continue;
                    }
                } else {
                    format!("dir:{}", parent.unwrap().display())
                }
            } else if !title.is_empty() {
                format!("{}|{}", title.to_lowercase(), author.to_lowercase())
            } else {
                let _ = media_root_files::set_unmatched(pool, file.id).await;
                result.unmatched += 1;
                continue;
            };

            groups.entry(key).or_default().push(file);
        } else {
            let _ = media_root_files::set_unmatched(pool, file.id).await;
            result.unmatched += 1;
        }
    }

    for (group_key, group_files) in &groups {
        let sample = group_files[0].interpretation.as_ref().unwrap();
        let is_dir_group = group_key.starts_with("dir:");

        // For directory-grouped audiobooks, pick the best metadata across all files.
        // For title|author groups (books, standalone audiobooks), use the first file.
        let meta = if is_dir_group {
            best_audiobook_group_metadata(group_files)
        } else {
            extract_sample_metadata(sample)
        };

        // When is_title_conflict is set, the probe found a better title (album tag)
        // than the filename-derived one. Use it for the search query.
        let query_title = if meta.is_title_conflict {
            sample
                .get("probe_title")
                .and_then(|v| v.as_str())
                .unwrap_or(&meta.title)
        } else {
            &meta.title
        };

        // Build query with all available fields — providers decide what to use
        let query = MetadataQuery {
            isbn: meta.isbn.clone(),
            title: Some(query_title.to_string()).filter(|s| !s.is_empty()),
            authors: meta.authors.clone(),
            asin: meta.asin.clone(),
            free_text: None,
        };

        let mut outcome = registry.search(&query, None).await;
        let all_cached = outcome.all_cached;
        outcome.adjust_relevance(&query);
        outcome.boost_series_matches();
        let search_results = outcome.results;

        // Rate limit between provider searches (skip if all results came from cache)
        if !all_cached {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }

        if search_results.is_empty() {
            // Content-based fallback: try extracting text from EPUB files
            let epub_file = group_files
                .iter()
                .find(|f| f.file_path.to_lowercase().ends_with(".epub"));

            let mut content_results = Vec::new();
            if let Some(epub) = epub_file {
                let path = std::path::Path::new(&epub.file_path);
                match crate::ebook::extract::extract_content_sample(path, 200) {
                    Ok(Some(sample)) => {
                        debug!(
                            title = %meta.title,
                            sample_len = sample.len(),
                            "Attempting content-based fallback search"
                        );

                        let content_query = MetadataQuery::by_free_text(&sample);
                        let content_outcome = registry.search(&content_query, None).await;

                        if !content_outcome.all_cached {
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        }

                        // Apply confidence penalty — content matches are less certain
                        content_results = content_outcome.results;
                        for r in &mut content_results {
                            r.confidence *= 0.6;
                        }
                        content_results.sort_by(|a, b| {
                            b.confidence
                                .partial_cmp(&a.confidence)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                    }
                    Ok(None) => {
                        debug!(title = %meta.title, "No suitable content sample found in EPUB");
                    }
                    Err(e) => {
                        debug!(title = %meta.title, error = %e, "Failed to extract EPUB content for fallback");
                    }
                }
            }

            if content_results.is_empty() {
                for file in group_files {
                    let _ = media_root_files::set_unmatched(pool, file.id).await;
                    result.unmatched += 1;
                }
                continue;
            }

            // Content-based results: always present as ambiguous (user must confirm)
            let candidates = json!(
                content_results
                    .iter()
                    .take(5)
                    .map(|r| {
                        json!({
                            "provider": r.provider,
                            "provider_id": r.external_id,
                            "title": r.title,
                            "authors": r.authors,
                            "year": r.publish_date.as_deref().and_then(|d| d.get(..4)).and_then(|y| y.parse::<i32>().ok()),
                            "cover_url": r.cover_url,
                            "isbn": r.isbn,
                            "confidence": r.confidence,
                            "series": r.series,
                            "series_index": r.series_index,
                            "match_method": "content",
                        })
                    })
                    .collect::<Vec<_>>()
            );

            for file in group_files {
                let _ = media_root_files::set_ambiguous(pool, file.id, &candidates).await;
                result.ambiguous += 1;
            }
            continue;
        }

        let top = &search_results[0];

        // High confidence + auto_add: create book entity
        // Skip auto-add when title came from generic filename (e.g. "01.mp3") —
        // folder-derived titles are less reliable, always send to review.
        if top.confidence >= 0.8
            && root.auto_add
            && !meta.is_generic_filename
            && !meta.is_title_conflict
        {
            match create_book_from_result(pool, nats, root, top, &meta).await {
                Ok(book_id) => {
                    result
                        .created_entities
                        .push(("book", book_id, top.title.clone()));
                    let mut has_audio = false;
                    let mut added_updates = Vec::new();
                    for file in group_files {
                        create_book_file_record(pool, book_id, file).await;
                        added_updates.push((file.id, "book", book_id));
                        result.added += 1;
                        if !has_audio {
                            let ext = std::path::Path::new(&file.file_path)
                                .extension()
                                .and_then(|e| e.to_str())
                                .unwrap_or("");
                            has_audio = crate::routes::ingest::AUDIOBOOK_EXTENSIONS
                                .contains(&ext.to_lowercase().as_str());
                        }
                    }
                    // Batch: mark media_root_files as added
                    if !added_updates.is_empty()
                        && let Err(e) =
                            media_root_files::set_added_batch(pool, &added_updates).await
                    {
                        warn!(error = %e, "Failed to batch set book files as added");
                    }
                    // Enqueue audiobook probe if any audio files were added
                    if has_audio {
                        let display = crate::jobs::fetch_job_display(pool, "book", book_id).await;
                        let probe_context = JobContext::for_entity(
                            "book",
                            book_id,
                            format!("Probing audiobook '{}'", top.title),
                        )
                        .with_display_opt(display);
                        let _ = crate::jobs::enqueue(
                            pool,
                            nats,
                            "audiobook_probe",
                            json!({"book_id": book_id}),
                            probe_context,
                        )
                        .await;
                    }
                    // Sidecars written by enrichment job after full metadata is available
                }
                Err(e) => {
                    warn!(title = %meta.title, error = %e, "Failed to create book from match");
                    for file in group_files {
                        let _ = media_root_files::set_unmatched(pool, file.id).await;
                        result.unmatched += 1;
                    }
                }
            }
            continue;
        }

        // High confidence but auto_add off: confident match awaiting user confirmation
        if top.confidence >= 0.8 {
            let candidates = json!(
                search_results
                    .iter()
                    .take(5)
                    .map(|r| {
                        json!({
                            "provider": r.provider,
                            "provider_id": r.external_id,
                            "title": r.title,
                            "authors": r.authors,
                            "year": r.publish_date.as_deref().and_then(|d| d.get(..4)).and_then(|y| y.parse::<i32>().ok()),
                            "cover_url": r.cover_url,
                            "isbn": r.isbn,
                            "confidence": r.confidence,
                            "series": r.series,
                            "series_index": r.series_index,
                        })
                    })
                    .collect::<Vec<_>>()
            );

            for file in group_files {
                let _ = media_root_files::set_review(pool, file.id, &candidates).await;
                result.review += 1;
            }
            continue;
        }

        // Low confidence or multiple results: genuinely ambiguous
        let candidates = json!(
            search_results
                .iter()
                .take(5)
                .map(|r| {
                    json!({
                        "provider": r.provider,
                        "provider_id": r.external_id,
                        "title": r.title,
                        "authors": r.authors,
                        "year": r.publish_date.as_deref().and_then(|d| d.get(..4)).and_then(|y| y.parse::<i32>().ok()),
                        "cover_url": r.cover_url,
                        "isbn": r.isbn,
                        "confidence": r.confidence,
                        "series": r.series,
                        "series_index": r.series_index,
                    })
                })
                .collect::<Vec<_>>()
        );

        for file in group_files {
            let _ = media_root_files::set_ambiguous(pool, file.id, &candidates).await;
            result.ambiguous += 1;
        }
    }

    result
}

/// Try to find an existing book that matches the metadata result.
/// Checks ISBN, provider IDs, and title+author to avoid creating duplicates.
async fn find_existing_book(pool: &PgPool, meta: &crate::metadata::MetadataResult) -> Option<Uuid> {
    // 1. Check by ISBN (most reliable identifier)
    if let Some(isbn) = &meta.isbn
        && let Ok(Some(m)) = books::find_by_identifier(pool, "isbn", isbn).await
    {
        trace!(book_id = %m.id, isbn = %isbn, "Matched existing book by ISBN");
        return Some(m.id);
    }

    // 2. Check by provider-specific ID (e.g., hardcover_id)
    if let Some(ext_id) = &meta.external_id {
        let key = format!("{}_id", meta.provider);
        if let Ok(Some(m)) = books::find_by_identifier(pool, &key, ext_id).await {
            trace!(book_id = %m.id, key = %key, id = %ext_id, "Matched existing book by provider ID");
            return Some(m.id);
        }
    }

    // 3. Check by provider-specific identifiers (e.g., hardcover_slug)
    if let Some(obj) = meta.identifiers.as_object() {
        for (k, v) in obj {
            if let Some(val) = v.as_str()
                && let Ok(Some(m)) = books::find_by_identifier(pool, k, val).await
            {
                trace!(book_id = %m.id, key = %k, val = %val, "Matched existing book by identifier");
                return Some(m.id);
            }
        }
    }

    // 4. Check by title + first author (fuzzy fallback)
    if let Some(first_author) = meta.authors.first() {
        let title = &meta.title;
        let existing: Option<(Uuid,)> = sqlx::query_as(
            r#"
            SELECT b.id FROM books b
            JOIN book_authors ba ON ba.book_id = b.id
            JOIN authors a ON a.id = ba.author_id
            WHERE LOWER(b.title) = LOWER($1)
              AND LOWER(a.name) = LOWER($2)
            ORDER BY b.created_at ASC, b.id ASC
            LIMIT 1
            "#,
        )
        .bind(title)
        .bind(first_author)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

        if let Some((id,)) = existing {
            trace!(book_id = %id, title = %title, author = %first_author, "Matched existing book by title+author");
            return Some(id);
        }
    }

    // 5. Check by title only for non-library discovered books (enrichment race condition)
    {
        let title = &meta.title;
        let fallback: Option<(Uuid,)> = sqlx::query_as(
            r#"
            SELECT b.id FROM books b
            WHERE LOWER(b.title) = LOWER($1)
              AND NOT (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
              AND b.created_at > NOW() - INTERVAL '1 hour'
            ORDER BY b.created_at ASC, b.id ASC
            LIMIT 1
            "#,
        )
        .bind(title)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

        if let Some((id,)) = fallback {
            trace!(book_id = %id, title = %title, "Matched existing discovered book by title");
            return Some(id);
        }
    }

    None
}

/// Create a book entity from a metadata search result, or match to an existing one.
async fn create_book_from_result(
    pool: &PgPool,
    nats: &Publisher,
    root: &media_roots::MediaRootRow,
    meta: &crate::metadata::MetadataResult,
    interp: &GroupMetadata,
) -> Result<Uuid, crate::repositories::RepositoryError> {
    // Try to find an existing book by identifiers before creating a new one.
    // Check ISBN first (most reliable), then provider-specific IDs.
    let existing_id = find_existing_book(pool, meta).await;

    // Handle series — provider takes priority, interpret fallback
    let effective_series = meta.series.as_deref().or(interp.series.as_deref());
    let effective_series_index = meta.series_index.or_else(|| {
        interp
            .series_index
            .as_deref()
            .and_then(|s| s.parse::<f64>().ok())
    });
    let series_id = if let Some(series_name) = effective_series {
        // Collect author IDs for the book's authors (create them first if needed)
        let mut author_ids = Vec::new();
        for author_name in &meta.authors {
            if let Ok(author_id) = authors::get_or_create_by_name(pool, author_name).await {
                author_ids.push(author_id);
            }
        }

        // Build series identifiers from provider (e.g. Hardcover series ID)
        let series_identifiers = meta
            .series_provider_id
            .as_ref()
            .map(|id| json!({ "hardcover": id }));

        // Use author overlap to disambiguate series with the same name
        series::get_or_create_with_author_overlap(
            pool,
            series_name,
            &author_ids,
            series_identifiers,
        )
        .await?
    } else {
        None
    };

    // Build identifiers from provider + interpretation
    let mut identifiers = serde_json::Map::new();
    if let Some(isbn) = &meta.isbn {
        identifiers.insert("isbn".to_string(), json!(isbn));
    }
    if let Some(asin) = &interp.asin {
        identifiers.entry("asin".to_string()).or_insert(json!(asin));
    }
    if let Some(ext_id) = &meta.external_id {
        identifiers.insert(format!("{}_id", meta.provider), json!(ext_id));
    }
    // Merge provider-specific identifiers (e.g. hardcover_slug)
    if let Some(obj) = meta.identifiers.as_object() {
        for (k, v) in obj {
            identifiers.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }

    let images = meta.cover_url.as_ref().map(|url| {
        let mut map = crate::images::ImageMap::new();
        map.insert(
            "cover".to_string(),
            crate::images::ImageEntry {
                source_url: Some(url.clone()),
                ..Default::default()
            },
        );
        map
    });

    let book_input = books::BookInput {
        id: existing_id,
        title: meta.title.clone(),
        description: meta.description.clone(),
        language: meta.language.clone(),
        publisher: meta.publisher.clone(),
        pubdate: meta.publish_date.as_ref().and_then(|s| {
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
                .ok()
                .or_else(|| {
                    time::Date::parse(
                        s,
                        &time::format_description::parse_borrowed::<2>("[year]-[month]-[day]")
                            .ok()?,
                    )
                    .ok()
                    .map(|d| d.with_hms(0, 0, 0).unwrap().assume_utc())
                })
        }),
        identifiers: Some(json!(identifiers)),
        page_count: meta.page_count.map(|p| p as i32),
        rating: meta.rating,
        rating_count: meta.rating_count,
        images,
        narrator: interp.narrator.clone(),
        is_abridged: if interp.abridged { Some(true) } else { None },
        want_format: if root.media_type == "audiobook" {
            Some("audiobook".to_string())
        } else {
            None
        },
    };

    let book_id = books::upsert_library_book(pool, book_input).await?;

    // Add to series via junction table
    if let Some(sid) = series_id {
        let _ = book_series::add(pool, book_id, sid, effective_series_index, false).await;
        let _ = book_series::auto_select_primary(pool, book_id, false).await;
    }

    // Set media_root_id on the book
    let _ = sqlx::query("UPDATE books SET media_root_id = $1 WHERE id = $2")
        .bind(root.id)
        .bind(book_id)
        .execute(pool)
        .await;

    // Link authors
    for author_name in &meta.authors {
        if let Ok(author_id) = authors::get_or_create_by_name(pool, author_name).await {
            let _ = books::link_author(pool, book_id, author_id).await;
        }
    }

    // Create per-device entitlements for auto-sync devices
    crate::auto_sync::create_auto_sync_entitlements(pool, nats, book_id).await;

    Ok(book_id)
}

/// Create a book_files or audiobook_files record for an added file.
/// Audio formats go to audiobook_files (supports multiple files per book),
/// ebook formats go to book_files.
async fn create_book_file_record(
    pool: &PgPool,
    book_id: Uuid,
    file: &media_root_files::MediaRootFileRow,
) {
    let ext = std::path::Path::new(&file.file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("unknown");
    let format = ext.to_uppercase();
    let is_audio =
        crate::routes::ingest::AUDIOBOOK_EXTENSIONS.contains(&ext.to_lowercase().as_str());

    if is_audio {
        let input = audiobook_files::AudiobookFileInput {
            book_id,
            format,
            file_path: file.file_path.clone(),
            file_size: file.file_size.unwrap_or(0),
            source_type: Some("media_root".to_string()),
            ..Default::default()
        };
        if let Err(e) = audiobook_files::create(pool, input).await {
            trace!(book_id = %book_id, error = %e, "Failed to create audiobook file (may already exist)");
        }
    } else {
        let input = book_files::BookFileInput {
            book_id,
            format,
            size_bytes: file.file_size.unwrap_or(0),
            file_path: file.file_path.clone(),
            source_type: Some("media_root".to_string()),
            ..Default::default()
        };
        if let Err(e) = book_files::upsert(pool, input).await {
            trace!(book_id = %book_id, error = %e, "Failed to create book file (may already exist)");
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Extract a specific provider ID from an InterpretResult's sidecar_ids.
fn extract_sidecar_id(interp: &InterpretResult, key: &str) -> Option<String> {
    interp
        .sidecar_ids
        .as_ref()
        .and_then(|ids| ids.get(key))
        .and_then(|v| v.as_str())
        .map(String::from)
}

pub async fn find_existing_tv_show(pool: &PgPool, name: &str, year: Option<u32>) -> Option<Uuid> {
    // When a year is available, prefer an exact title+year match to avoid
    // confusing revivals with originals (e.g. "Scrubs" 2001 vs 2026). Both
    // queries then take the oldest row under a total order — same rule the
    // provider upserts use — so repeated matcher runs cannot disagree and split
    // one release across two shows.
    if let Some(y) = year
        && let Some(id) = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM tv_shows WHERE LOWER(title) = LOWER($1) AND year = $2 \
             ORDER BY created_at ASC, id ASC LIMIT 1",
        )
        .bind(name)
        .bind(y as i32)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
    {
        return Some(id);
    }
    // Fall back to title-only when no year provided or no year match found.
    // This keeps existing shows without years working.
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM tv_shows WHERE LOWER(title) = LOWER($1) \
         ORDER BY created_at ASC, id ASC LIMIT 1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

async fn find_existing_movie(
    pool: &PgPool,
    title: &str,
    year: Option<u32>,
    provider: Option<&str>,
    provider_id: Option<&str>,
) -> Option<Uuid> {
    // Check by provider ID first (uses unique index, race-safe)
    if let Some(provider) = provider
        && let Some(pid) = provider_id
    {
        let key = format!("{provider}_id");
        let found = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM movies WHERE identifiers->>$1 = $2 \
             ORDER BY created_at ASC, id ASC LIMIT 1",
        )
        .bind(&key)
        .bind(pid)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
        if found.is_some() {
            return found;
        }
    }

    // Fall back to title + year
    if let Some(year) = year {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM movies WHERE LOWER(title) = LOWER($1) AND year = $2 \
             ORDER BY created_at ASC, id ASC LIMIT 1",
        )
        .bind(title)
        .bind(year as i32)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
    } else {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM movies WHERE LOWER(title) = LOWER($1) \
             ORDER BY created_at ASC, id ASC LIMIT 1",
        )
        .bind(title)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
    }
}

async fn find_or_create_season(
    pool: &PgPool,
    show_id: Uuid,
    season_num: u32,
) -> Result<Uuid, crate::repositories::RepositoryError> {
    // Use plain number as identifier to match TMDB enrichment format
    let identifier = season_num.to_string();

    let input = tv_seasons::SeasonInput {
        show_id,
        season_identifier: identifier,
        season_sort_order: season_num as i32,
        title: Some(format!("Season {}", season_num)),
        description: None,
        images: None,
        air_date: None,
        episode_count: None,
        is_monitored: false,
        is_special: season_num == 0,
        auto_want_mode: "inherit".to_string(),
    };
    tv_seasons::upsert(pool, input).await
}

async fn create_episode_file(
    pool: &PgPool,
    file_path: &str,
    fingerprint: Option<&str>,
) -> Option<Uuid> {
    use crate::repositories::tv_episode_files;
    let format = std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("unknown")
        .to_uppercase();
    let input = tv_episode_files::EpisodeFileInput {
        file_path: file_path.to_string(),
        file_size: None,
        file_fingerprint: fingerprint.map(str::to_string),
        format,
        quality: None,
        languages: None,
        subtitles: None,
        source_type: Some("media_root".to_string()),
        source_metadata: None,
        video_codec: None,
        video_width: None,
        video_height: None,
        video_bitrate_kbps: None,
        framerate: None,
        bit_depth: None,
        is_hdr: false,
        is_interlaced: false,
        hdr_format: None,
        duration_seconds: None,
        container: None,
        overall_bitrate_kbps: None,
    };
    match tv_episode_files::create_or_update(pool, input).await {
        Ok(id) => Some(id),
        Err(e) => {
            trace!(file_path = %file_path, error = %e, "Failed to upsert episode file");
            None
        }
    }
}

async fn create_movie_file(
    pool: &PgPool,
    movie_id: Uuid,
    file_path: &str,
    fingerprint: Option<&str>,
) -> Option<Uuid> {
    use crate::repositories::movie_files;
    let format = std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("unknown")
        .to_uppercase();
    let input = movie_files::MovieFileInput {
        movie_id,
        file_path: file_path.to_string(),
        file_size: None,
        file_fingerprint: fingerprint.map(str::to_string),
        format,
        quality: None,
        languages: None,
        subtitles: None,
        source_type: Some("media_root".to_string()),
        source_metadata: None,
        video_codec: None,
        video_width: None,
        video_height: None,
        video_bitrate_kbps: None,
        framerate: None,
        bit_depth: None,
        is_hdr: false,
        hdr_format: None,
        is_interlaced: false,
        duration_seconds: None,
        container: None,
        overall_bitrate_kbps: None,
    };
    match movie_files::create_or_update(pool, input).await {
        Ok(id) => {
            let replaced = movie_files::delete_other_files_for_movie(pool, movie_id, id)
                .await
                .unwrap_or_default();
            crate::repositories::cleanup_replaced_files(&replaced).await;
            Some(id)
        }
        Err(e) => {
            trace!(movie_id = %movie_id, error = %e, "Failed to upsert movie file");
            None
        }
    }
}

/// Enqueue a media_probe job for a newly created file.
async fn enqueue_media_probe(pool: &PgPool, nats: &Publisher, file_id: Uuid, media_type: &str) {
    let entity_type = if media_type == "movie" {
        "movie_file"
    } else {
        "tv_episode_file"
    };
    let display = crate::jobs::fetch_job_display(pool, entity_type, file_id).await;
    let context = JobContext::for_entity(entity_type, file_id, "Probing media file".to_string())
        .with_display_opt(display);
    let _ = crate::jobs::enqueue(
        pool,
        nats,
        "media_probe",
        json!({"file_id": file_id, "media_type": media_type}),
        context,
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::types::MetadataResult;

    #[test]
    fn test_fuzzy_match_exact() {
        assert!(fuzzy_match("arthur c clarke", "arthur c clarke"));
    }

    #[test]
    fn test_fuzzy_match_substring() {
        assert!(fuzzy_match("arthur clarke", "arthur c clarke"));
    }

    #[test]
    fn test_fuzzy_match_reordered() {
        // Sidecar "Last, First" → "c clarke arthur" vs provider "arthur c clarke"
        assert!(fuzzy_match("c clarke arthur", "arthur c clarke"));
    }

    #[test]
    fn test_fuzzy_match_abbreviation() {
        assert!(fuzzy_match("a clarke", "arthur clarke"));
    }

    #[test]
    fn test_fuzzy_match_no_overlap() {
        assert!(!fuzzy_match("stephen king", "completely different"));
    }

    #[test]
    fn test_fuzzy_match_empty() {
        assert!(!fuzzy_match("", "arthur clarke"));
        assert!(!fuzzy_match("arthur clarke", ""));
    }

    #[test]
    fn test_strip_punctuation() {
        assert_eq!(strip_punctuation("C., Clarke, Arthur"), "C Clarke Arthur");
        assert_eq!(strip_punctuation("Arthur C. Clarke"), "Arthur C Clarke");
    }

    /// Simulate the confidence boost logic from match_book_group.
    /// When a result has matching title + author but low confidence, it should
    /// be promoted to 0.8.
    #[test]
    fn test_confidence_boost_title_author_match() {
        let title = "The Hobbit";
        let author = Some("J.R.R. Tolkien");

        let mut results = vec![MetadataResult {
            provider: "hardcover".to_string(),
            title: "The Hobbit".to_string(),
            authors: vec!["J.R.R. Tolkien".to_string()],
            confidence: 0.6,
            ..Default::default()
        }];

        // Apply the same boost logic as match_book_group
        let title_norm = strip_punctuation(&title.to_lowercase());
        for result in &mut results {
            let result_title_norm = strip_punctuation(&result.title.to_lowercase());
            let title_matches = fuzzy_match(&result_title_norm, &title_norm);
            if title_matches {
                let author_matches = author.is_some_and(|qa| {
                    let qa_norm = strip_punctuation(&qa.to_lowercase());
                    result.authors.iter().any(|a: &String| {
                        fuzzy_match(&strip_punctuation(&a.to_lowercase()), &qa_norm)
                    })
                });
                if author_matches && result.confidence < 0.8 {
                    result.confidence = 0.8;
                }
            }
        }

        assert_eq!(
            results[0].confidence, 0.8,
            "confidence should be boosted from 0.6 to 0.8"
        );
    }

    /// Confidence should not be boosted when title matches but author doesn't.
    #[test]
    fn test_confidence_no_boost_without_author_match() {
        let title = "The Hobbit";
        let author = Some("Wrong Author");

        let mut result = MetadataResult {
            provider: "hardcover".to_string(),
            title: "The Hobbit".to_string(),
            authors: vec!["J.R.R. Tolkien".to_string()],
            confidence: 0.6,
            ..Default::default()
        };

        let title_norm = strip_punctuation(&title.to_lowercase());
        let result_title_norm = strip_punctuation(&result.title.to_lowercase());
        let title_matches = fuzzy_match(&result_title_norm, &title_norm);
        assert!(title_matches);

        let author_matches = author.is_some_and(|qa| {
            let qa_norm = strip_punctuation(&qa.to_lowercase());
            result
                .authors
                .iter()
                .any(|a: &String| fuzzy_match(&strip_punctuation(&a.to_lowercase()), &qa_norm))
        });
        if author_matches && result.confidence < 0.8 {
            result.confidence = 0.8;
        }

        assert_eq!(
            result.confidence, 0.6,
            "confidence should NOT be boosted without author match"
        );
    }

    fn make_episode(id: Uuid, title: &str, sort_order: i32) -> tv_episodes::EpisodeRow {
        tv_episodes::EpisodeRow {
            id,
            season_id: Uuid::nil(),
            episode_identifier: format!("E{sort_order}"),
            episode_sort_order: sort_order,
            title: Some(title.to_string()),
            description: None,
            images: Default::default(),
            air_date: None,
            air_time: None,
            runtime: None,
            is_in_library: true,
            want_status: "none".to_string(),
            identifiers: serde_json::json!({}),
            country: None,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn test_race_matches_race_not_sprint_race() {
        let race_id = Uuid::new_v4();
        let sprint_race_id = Uuid::new_v4();
        let qualifying_id = Uuid::new_v4();
        let episodes = vec![
            make_episode(qualifying_id, "Thailand Qualifying", 1001),
            make_episode(sprint_race_id, "Thailand Sprint Race", 1002),
            make_episode(race_id, "Thailand Race", 1003),
        ];
        // "Race" must match "Thailand Race", not "Thailand Sprint Race"
        let result = match_sport_episode(Some(1), Some("Race"), None, &episodes);
        assert_eq!(result, Some(race_id));
    }

    #[test]
    fn test_sprint_matches_sprint_not_sprint_race() {
        let sprint_id = Uuid::new_v4();
        let sprint_race_id = Uuid::new_v4();
        let episodes = vec![
            make_episode(sprint_id, "Thailand Sprint", 1001),
            make_episode(sprint_race_id, "Thailand Sprint Race", 1002),
        ];
        // "Sprint" must match "Thailand Sprint", not "Thailand Sprint Race"
        let result = match_sport_episode(Some(1), Some("Sprint"), None, &episodes);
        assert_eq!(result, Some(sprint_id));
    }

    #[test]
    fn test_sprint_race_matches_sprint_race() {
        let race_id = Uuid::new_v4();
        let sprint_race_id = Uuid::new_v4();
        let episodes = vec![
            make_episode(race_id, "Thailand Race", 1001),
            make_episode(sprint_race_id, "Thailand Sprint Race", 1002),
        ];
        // "Sprint Race" must match "Thailand Sprint Race"
        let result = match_sport_episode(Some(1), Some("Sprint Race"), None, &episodes);
        assert_eq!(result, Some(sprint_race_id));
    }
}
