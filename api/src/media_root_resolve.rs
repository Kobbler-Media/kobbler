//! Shared resolve logic for media root files.
//!
//! Extracts the core "create entity + link file" logic so both the HTTP
//! resolve routes and the NATS bulk-resolve job can call the same code.

use crate::jobs::JobContext;
use crate::media_interpreter::InterpretResult;
use crate::repositories::{
    audiobook_files, authors, book_files, books, media_root_files, movie_files, movies,
    tv_episode_files, tv_episodes, tv_seasons, tv_shows,
};
use crate::search::{self, SearchBackend};
use nats_jobs::Publisher;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::repositories::media_root_files::MediaRootFileRow;
use crate::repositories::media_roots::MediaRootRow;

/// Dependencies needed by resolve functions.
pub struct ResolveContext<'a> {
    pub pool: &'a PgPool,
    pub nats: &'a Publisher,
    pub search: &'a (dyn SearchBackend + 'a),
}

/// Result of a successful resolve.
pub struct ResolveOutcome {
    pub entity_type: String,
    pub entity_id: Uuid,
}

/// Dispatch to the right resolve function based on media_type.
pub async fn resolve_file(
    ctx: &ResolveContext<'_>,
    root: &MediaRootRow,
    file: &MediaRootFileRow,
    file_id: Uuid,
    candidate: &serde_json::Value,
) -> Result<ResolveOutcome, String> {
    match root.media_type.as_str() {
        "tv" => resolve_tv(ctx, root, file, file_id, candidate).await,
        "sport" => resolve_sport(ctx, root, file, file_id, candidate).await,
        "movie" => resolve_movie(ctx, root, file, file_id, candidate).await,
        "book" | "audiobook" => resolve_book(ctx, root, file, file_id, candidate).await,
        other => Err(format!("Unknown media type '{other}'")),
    }
}

/// Resolve a TV episode file: create show, season, episode, link file.
pub async fn resolve_tv(
    ctx: &ResolveContext<'_>,
    root: &MediaRootRow,
    file: &MediaRootFileRow,
    file_id: Uuid,
    candidate: &serde_json::Value,
) -> Result<ResolveOutcome, String> {
    let title = candidate["title"].as_str().unwrap_or("").to_string();
    let provider = candidate["provider"].as_str().unwrap_or("tmdb").to_string();
    let provider_id = candidate["provider_id"]
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| candidate["provider_id"].as_u64().map(|n| n.to_string()))
        .unwrap_or_default();
    let year = candidate["year"].as_i64().map(|y| y as i32);
    let id_key = format!("{}_id", provider);

    // Parse the file's interpretation to get season/episode numbers
    let interp: InterpretResult = file
        .interpretation
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .ok_or("File has no interpretation (season/episode info)")?;

    let season_num = interp.season.ok_or("No season number in interpretation")?;

    let episodes = &interp.episodes;
    if episodes.is_empty() {
        return Err("No episode numbers in interpretation".to_string());
    }

    // Create or find the TV show via provider upsert
    let show_input = tv_shows::TvShowInput {
        title: title.clone(),
        year,
        identifiers: Some(json!({&id_key: provider_id})),
        ..Default::default()
    };

    let show_id = tv_shows::upsert_by_provider(ctx.pool, &provider, &provider_id, show_input)
        .await
        .map_err(|e| e.to_string())?;

    // Set media_root_id on the show
    let _ = sqlx::query("UPDATE tv_shows SET media_root_id = $1 WHERE id = $2")
        .bind(root.id)
        .bind(show_id)
        .execute(ctx.pool)
        .await;

    // Find or create season
    let season_input = tv_seasons::SeasonInput {
        show_id,
        season_identifier: season_num.to_string(),
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
    let season_id = tv_seasons::upsert(ctx.pool, season_input)
        .await
        .map_err(|e| e.to_string())?;

    // Find or create all episodes, collecting their IDs
    let mut episode_ids: Vec<Uuid> = Vec::with_capacity(episodes.len());
    for &ep_num in episodes {
        let identifier = ep_num.to_string();
        let episode_id = match sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM tv_episodes WHERE season_id = $1 AND episode_identifier = $2",
        )
        .bind(season_id)
        .bind(&identifier)
        .fetch_optional(ctx.pool)
        .await
        .map_err(|e| e.to_string())?
        {
            Some(id) => id,
            None => {
                let input = tv_episodes::EpisodeInput {
                    season_id,
                    episode_identifier: identifier,
                    episode_sort_order: ep_num as i32,
                    title: Some(format!("Episode {}", ep_num)),
                    description: None,
                    images: None,
                    air_date: None,
                    air_time: None,
                    runtime: None,
                    identifiers: serde_json::json!({}),
                    country: None,
                };
                tv_episodes::create(ctx.pool, input)
                    .await
                    .map_err(|e| e.to_string())?
            }
        };
        episode_ids.push(episode_id);
    }
    let first_episode_id = episode_ids[0];

    // Create episode file record
    let format = std::path::Path::new(&file.file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("unknown")
        .to_uppercase();
    let file_input = tv_episode_files::EpisodeFileInput {
        file_path: file.file_path.clone(),
        file_size: None,
        file_fingerprint: file.file_fingerprint.clone(),
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
    let ep_file_id = tv_episode_files::create_or_update(ctx.pool, file_input)
        .await
        .map_err(|e| format!("Failed to create episode file: {e}"))?;

    // Link all episodes to this file via junction table
    let replaced = tv_episode_files::link_episodes(ctx.pool, ep_file_id, &episode_ids)
        .await
        .map_err(|e| format!("Failed to link episodes to file: {e}"))?;
    crate::repositories::cleanup_replaced_files(&replaced).await;

    // Enqueue media probe job to populate video metadata via ffprobe
    let display = crate::jobs::fetch_job_display(ctx.pool, "tv_episode_file", ep_file_id).await;
    let probe_context = JobContext::for_entity(
        "tv_episode_file",
        ep_file_id,
        "Probing media file".to_string(),
    )
    .with_display_opt(display);
    let _ = crate::jobs::enqueue(
        ctx.pool,
        ctx.nats,
        "media_probe",
        json!({"file_id": ep_file_id, "media_type": "episode"}),
        probe_context,
    )
    .await;

    // Mark file as added (use first episode ID)
    let _ = media_root_files::set_added(ctx.pool, file_id, "tv_episode", first_episode_id).await;
    // Sidecars written by enrichment job after full metadata is available
    let _ = search::index_media_root_file(ctx.search, ctx.pool, file_id).await;

    // Enqueue enrichment and search index jobs (skip if already running)
    if !crate::jobs::has_active_job(ctx.pool, "tv_show_enrich", "show_id", &show_id.to_string())
        .await
        .unwrap_or(false)
    {
        let display = crate::jobs::fetch_job_display(ctx.pool, "tv_show", show_id).await;
        let context = JobContext::for_entity("tv_show", show_id, format!("Enriching '{}'", title))
            .with_display_opt(display);
        let _ = crate::jobs::enqueue(
            ctx.pool,
            ctx.nats,
            "tv_show_enrich",
            json!({"show_id": show_id}),
            context,
        )
        .await;
        crate::jobs::enqueue_tv_show_index(ctx.pool, ctx.nats, show_id).await;
    }

    Ok(ResolveOutcome {
        entity_type: "tv_episode".to_string(),
        entity_id: first_episode_id,
    })
}

/// Resolve a sport file: find league + season + episode from interpretation, link file.
///
/// Sport files use a different interpretation structure than TV (round/session
/// instead of season/episode numbers), so they need dedicated resolve logic
/// matching the approach used in `match_sport_files`.
///
/// Data sources (in priority order):
/// 1. Candidate title → league name (user-confirmed, most reliable)
/// 2. Interpretation raw → round, session_type, event_title
/// 3. Sidecar metadata → title (event name), season (year)
/// 4. Interpretation fields → season/show_year
pub async fn resolve_sport(
    ctx: &ResolveContext<'_>,
    root: &MediaRootRow,
    file: &MediaRootFileRow,
    file_id: Uuid,
    candidate: &serde_json::Value,
) -> Result<ResolveOutcome, String> {
    let league_name = candidate["title"]
        .as_str()
        .or_else(|| {
            file.interpretation
                .as_ref()
                .and_then(|v| v.get("show_name"))
                .and_then(|v| v.as_str())
        })
        .ok_or("No league name in candidate or interpretation")?
        .to_string();

    let provider = candidate["provider"]
        .as_str()
        .unwrap_or("thesportsdb")
        .to_string();
    let provider_id = candidate["provider_id"]
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| candidate["provider_id"].as_u64().map(|n| n.to_string()))
        .unwrap_or_default();
    let id_key = format!("{}_id", provider);

    // Create or find the sport league via provider upsert
    let show_input = tv_shows::TvShowInput {
        title: league_name.clone(),
        identifiers: Some(json!({&id_key: provider_id})),
        content_category: Some("sport".to_string()),
        ..Default::default()
    };
    let show_id = tv_shows::upsert_by_provider(ctx.pool, &provider, &provider_id, show_input)
        .await
        .map_err(|e| e.to_string())?;

    // Set media_root_id on the show
    let _ = sqlx::query("UPDATE tv_shows SET media_root_id = $1 WHERE id = $2")
        .bind(root.id)
        .bind(show_id)
        .execute(ctx.pool)
        .await;

    // Parse interpretation for sport-specific fields
    let interp: Option<InterpretResult> = file
        .interpretation
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    let sport = interp
        .as_ref()
        .and_then(|i| i.raw.as_ref())
        .and_then(|r| r.media.as_sport());

    // Extract round, session, event_title from interpretation
    let round = sport.and_then(|s| s.round);
    let raw_session = sport.and_then(|s| s.session_type.as_deref());
    let interp_event_title = sport.and_then(|s| s.event_title.as_deref());

    // Fall back to sidecar metadata for event title if interpretation lacks it
    let sidecar = file.sidecar_metadata.as_ref();
    let sidecar_title = sidecar
        .and_then(|s| s.get("title"))
        .and_then(|v| v.as_str());
    let event_title = interp_event_title.or(sidecar_title);

    // Determine season year from multiple sources
    let sidecar_season = sidecar
        .and_then(|s| s.get("video"))
        .and_then(|v| v.get("season"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let season_year = interp
        .as_ref()
        .and_then(|i| i.season)
        .or_else(|| interp.as_ref().and_then(|i| i.show_year))
        .or_else(|| sport.and_then(|s| s.year))
        .or(sidecar_season)
        .ok_or("No season year found in interpretation or sidecar metadata")?;

    // Find or create season
    let season_input = tv_seasons::SeasonInput {
        show_id,
        season_identifier: season_year.to_string(),
        season_sort_order: season_year as i32,
        title: Some(season_year.to_string()),
        description: None,
        images: None,
        air_date: None,
        episode_count: None,
        is_monitored: false,
        is_special: false,
        auto_want_mode: "inherit".to_string(),
    };
    let season_id = tv_seasons::upsert(ctx.pool, season_input)
        .await
        .map_err(|e| e.to_string())?;

    // Try to match an existing episode first
    let all_episodes = tv_episodes::list_by_season(ctx.pool, season_id)
        .await
        .map_err(|e| format!("Failed to list episodes: {e}"))?;

    let episode_id = if let Some(id) =
        crate::jobs::handlers::match_sport_episode(round, raw_session, event_title, &all_episodes)
    {
        id
    } else {
        // No match — create a new episode from interpretation data
        let ep_title = event_title
            .map(String::from)
            .unwrap_or_else(|| format!("Round {}", round.unwrap_or(0)));
        let sort_order = round.unwrap_or(0) * 1000;
        let identifier = sort_order.to_string();

        let input = tv_episodes::EpisodeInput {
            season_id,
            episode_identifier: identifier,
            episode_sort_order: sort_order as i32,
            title: Some(ep_title),
            description: None,
            images: None,
            air_date: None,
            air_time: None,
            runtime: None,
            identifiers: serde_json::json!({}),
            country: None,
        };
        tv_episodes::create(ctx.pool, input)
            .await
            .map_err(|e| format!("Failed to create episode: {e}"))?
    };

    // Enqueue enrichment so TheSportsDB populates full season/episode data (skip if already running)
    if !crate::jobs::has_active_job(
        ctx.pool,
        "sports_league_enrich",
        "show_id",
        &show_id.to_string(),
    )
    .await
    .unwrap_or(false)
    {
        let display = crate::jobs::fetch_job_display(ctx.pool, "tv_show", show_id).await;
        let context =
            JobContext::for_entity("tv_show", show_id, format!("Enriching '{league_name}'"))
                .with_display_opt(display);
        let _ = crate::jobs::enqueue(
            ctx.pool,
            ctx.nats,
            "sports_league_enrich",
            json!({"show_id": show_id}),
            context,
        )
        .await;
    }

    // Create episode file record
    let format = std::path::Path::new(&file.file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("unknown")
        .to_uppercase();
    let file_input = tv_episode_files::EpisodeFileInput {
        file_path: file.file_path.clone(),
        file_size: None,
        file_fingerprint: file.file_fingerprint.clone(),
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
    let ep_file_id = tv_episode_files::create_or_update(ctx.pool, file_input)
        .await
        .map_err(|e| format!("Failed to create episode file: {e}"))?;

    // Link episode to file
    let replaced = tv_episode_files::link_episodes(ctx.pool, ep_file_id, &[episode_id])
        .await
        .map_err(|e| format!("Failed to link episode to file: {e}"))?;
    crate::repositories::cleanup_replaced_files(&replaced).await;

    // Enqueue media probe
    let display = crate::jobs::fetch_job_display(ctx.pool, "tv_episode_file", ep_file_id).await;
    let probe_context = JobContext::for_entity(
        "tv_episode_file",
        ep_file_id,
        "Probing media file".to_string(),
    )
    .with_display_opt(display);
    let _ = crate::jobs::enqueue(
        ctx.pool,
        ctx.nats,
        "media_probe",
        json!({"file_id": ep_file_id, "media_type": "episode"}),
        probe_context,
    )
    .await;

    // Mark media root file as added
    let _ = media_root_files::set_added(ctx.pool, file_id, "tv_episode", episode_id).await;
    let _ = search::index_media_root_file(ctx.search, ctx.pool, file_id).await;

    // Enqueue search index for the show (skip if enrichment already running — it will reindex)
    if !crate::jobs::has_active_job(
        ctx.pool,
        "sports_league_enrich",
        "show_id",
        &show_id.to_string(),
    )
    .await
    .unwrap_or(false)
    {
        crate::jobs::enqueue_tv_show_index(ctx.pool, ctx.nats, show_id).await;
    }

    Ok(ResolveOutcome {
        entity_type: "tv_episode".to_string(),
        entity_id: episode_id,
    })
}

/// Resolve a movie file: create movie, link file.
pub async fn resolve_movie(
    ctx: &ResolveContext<'_>,
    root: &MediaRootRow,
    file: &MediaRootFileRow,
    file_id: Uuid,
    candidate: &serde_json::Value,
) -> Result<ResolveOutcome, String> {
    let title = candidate["title"].as_str().unwrap_or("").to_string();
    let provider = candidate["provider"].as_str().unwrap_or("tmdb").to_string();
    let provider_id = candidate["provider_id"]
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| candidate["provider_id"].as_u64().map(|n| n.to_string()))
        .unwrap_or_default();
    let year = candidate["year"].as_i64().map(|y| y as i32);
    let id_key = format!("{}_id", provider);

    // Create or find the movie via provider upsert
    let movie_input = movies::MovieInput {
        title: title.clone(),
        year,
        identifiers: Some(json!({&id_key: provider_id})),
        ..Default::default()
    };

    let movie_id = movies::upsert_by_provider(ctx.pool, &provider, &provider_id, movie_input)
        .await
        .map_err(|e| e.to_string())?;

    // Set media_root_id on the movie
    let _ = sqlx::query("UPDATE movies SET media_root_id = $1 WHERE id = $2")
        .bind(root.id)
        .bind(movie_id)
        .execute(ctx.pool)
        .await;

    // Create movie file record
    let format = std::path::Path::new(&file.file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("unknown")
        .to_uppercase();
    let file_input = movie_files::MovieFileInput {
        movie_id,
        file_path: file.file_path.clone(),
        file_size: None,
        file_fingerprint: file.file_fingerprint.clone(),
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
    let movie_file_id = movie_files::create_or_update(ctx.pool, file_input)
        .await
        .map_err(|e| format!("Failed to create movie file: {e}"))?;

    let replaced = movie_files::delete_other_files_for_movie(ctx.pool, movie_id, movie_file_id)
        .await
        .unwrap_or_default();
    crate::repositories::cleanup_replaced_files(&replaced).await;

    // Enqueue media probe job to populate video metadata via ffprobe
    let display = crate::jobs::fetch_job_display(ctx.pool, "movie_file", movie_file_id).await;
    let probe_context = JobContext::for_entity(
        "movie_file",
        movie_file_id,
        "Probing media file".to_string(),
    )
    .with_display_opt(display);
    let _ = crate::jobs::enqueue(
        ctx.pool,
        ctx.nats,
        "media_probe",
        json!({"file_id": movie_file_id, "media_type": "movie"}),
        probe_context,
    )
    .await;

    // Mark file as added
    let _ = media_root_files::set_added(ctx.pool, file_id, "movie", movie_id).await;
    // Sidecars written by enrichment job after full metadata is available
    let _ = search::index_media_root_file(ctx.search, ctx.pool, file_id).await;

    // Enqueue enrichment and search index jobs (skip if already running)
    if !crate::jobs::has_active_job(ctx.pool, "movie_enrich", "movie_id", &movie_id.to_string())
        .await
        .unwrap_or(false)
    {
        let display = crate::jobs::fetch_job_display(ctx.pool, "movie", movie_id).await;
        let context = JobContext::for_entity("movie", movie_id, format!("Enriching '{}'", title))
            .with_display_opt(display);
        let _ = crate::jobs::enqueue(
            ctx.pool,
            ctx.nats,
            "movie_enrich",
            json!({"movie_id": movie_id.to_string()}),
            context,
        )
        .await;
        crate::jobs::enqueue_movie_index(ctx.pool, ctx.nats, movie_id).await;
    }

    Ok(ResolveOutcome {
        entity_type: "movie".to_string(),
        entity_id: movie_id,
    })
}

/// Resolve a book file: create book from candidate data, link file, enqueue enrichment.
///
/// Unlike the old approach, this does NOT make external HTTP calls to providers.
/// The candidate JSON (from match_candidates) already has title, authors, isbn,
/// cover_url. Full metadata (description, series, rating, etc.) is filled in by
/// the book_enrich job.
pub async fn resolve_book(
    ctx: &ResolveContext<'_>,
    root: &MediaRootRow,
    file: &MediaRootFileRow,
    file_id: Uuid,
    candidate: &serde_json::Value,
) -> Result<ResolveOutcome, String> {
    let title = candidate["title"].as_str().unwrap_or("").to_string();
    let provider = candidate["provider"].as_str().unwrap_or("");
    let provider_id = candidate["provider_id"].as_str().unwrap_or("");

    // Build identifiers from candidate
    let mut identifiers = serde_json::Map::new();
    if let Some(isbn) = candidate["isbn"].as_str() {
        identifiers.insert("isbn".to_string(), json!(isbn));
    }
    if !provider.is_empty() && !provider_id.is_empty() {
        identifiers.insert(format!("{provider}_id"), json!(provider_id));
    }

    // Build cover image from candidate
    let images = candidate["cover_url"].as_str().map(|url| {
        let mut map = crate::images::ImageMap::new();
        map.insert(
            "cover".to_string(),
            crate::images::ImageEntry {
                source_url: Some(url.to_string()),
                ..Default::default()
            },
        );
        map
    });

    let want_format = if root.media_type == "audiobook" {
        Some("audiobook".to_string())
    } else {
        None
    };

    // Check for existing library book by title + author to avoid creating
    // duplicates (e.g. audiobook when ebook already exists).
    let author_names: Vec<String> = candidate["authors"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let existing_id =
        books::find_library_book_by_title_and_author_names(ctx.pool, &title, &author_names)
            .await
            .ok()
            .flatten()
            .map(|m| m.id);

    let book_input = books::BookInput {
        id: existing_id,
        title: title.clone(),
        identifiers: if identifiers.is_empty() {
            None
        } else {
            Some(json!(identifiers))
        },
        images,
        want_format,
        ..Default::default()
    };

    let book_id = books::upsert_library_book(ctx.pool, book_input)
        .await
        .map_err(|e| e.to_string())?;

    // Set media_root_id on the book
    let _ = sqlx::query("UPDATE books SET media_root_id = $1 WHERE id = $2")
        .bind(root.id)
        .bind(book_id)
        .execute(ctx.pool)
        .await;

    // Link authors from candidate data
    if let Some(author_arr) = candidate["authors"].as_array() {
        for author_val in author_arr {
            if let Some(name) = author_val.as_str()
                && let Ok(author_id) = authors::get_or_create_by_name(ctx.pool, name).await
            {
                let _ = books::link_author(ctx.pool, book_id, author_id).await;
            }
        }
    }

    // Create file record — audiobook formats go to audiobook_files, ebooks to book_files
    let format = std::path::Path::new(&file.file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("unknown")
        .to_uppercase();
    let is_audio =
        crate::routes::ingest::AUDIOBOOK_EXTENSIONS.contains(&format.to_lowercase().as_str());
    if is_audio {
        let _ = audiobook_files::upsert_by_path(
            ctx.pool,
            audiobook_files::AudiobookFileInput {
                book_id,
                format,
                file_path: file.file_path.clone(),
                file_size: file.file_size.unwrap_or(0),
                source_type: Some("media_root".to_string()),
                ..Default::default()
            },
        )
        .await;

        // Enqueue probe job to extract duration, codec info, and chapters
        crate::jobs::enqueue_audiobook_probe(ctx.pool, ctx.nats, book_id, &title).await;
    } else {
        let file_input = book_files::BookFileInput {
            book_id,
            format,
            size_bytes: file.file_size.unwrap_or(0),
            file_path: file.file_path.clone(),
            source_type: Some("media_root".to_string()),
            ..Default::default()
        };
        let _ = book_files::upsert(ctx.pool, file_input).await;
    }

    // Create per-device entitlements for auto-sync devices
    crate::auto_sync::create_auto_sync_entitlements(ctx.pool, ctx.nats, book_id).await;

    // Mark file as added
    let _ = media_root_files::set_added(ctx.pool, file_id, "book", book_id).await;
    // Sidecars written by enrichment job after full metadata is available
    let _ = search::index_media_root_file(ctx.search, ctx.pool, file_id).await;

    // Enqueue enrichment (handles full metadata, series, covers, etc.) — skip if already running
    if !crate::jobs::has_active_job(ctx.pool, "book_enrich", "book_id", &book_id.to_string())
        .await
        .unwrap_or(false)
    {
        let display = crate::jobs::fetch_job_display(ctx.pool, "book", book_id).await;
        let context = JobContext::for_entity("book", book_id, format!("Enriching '{}'", title))
            .with_display_opt(display);
        let _ = crate::jobs::enqueue(
            ctx.pool,
            ctx.nats,
            "book_enrich",
            json!({"book_id": book_id.to_string()}),
            context,
        )
        .await;
        crate::jobs::enqueue_book_index(ctx.pool, ctx.nats, book_id).await;
    }

    Ok(ResolveOutcome {
        entity_type: "book".to_string(),
        entity_id: book_id,
    })
}

// =============================================================================
// Grouped resolve functions (for bulk operations)
// =============================================================================

/// A file with its resolved candidate and media root file ID.
pub struct FileWithCandidate {
    pub file_id: Uuid,
    pub file: MediaRootFileRow,
    pub candidate: serde_json::Value,
}

/// Result of a single file resolution within a group.
pub struct GroupFileResult {
    pub file_id: Uuid,
    pub result: Result<ResolveOutcome, String>,
}

/// Resolve a group of TV files that all belong to the same show.
///
/// Creates the show once, batch-creates seasons/episodes, creates file records,
/// and enqueues exactly ONE enrichment job.
pub async fn resolve_tv_group(
    ctx: &ResolveContext<'_>,
    root: &MediaRootRow,
    files: &[FileWithCandidate],
) -> Vec<GroupFileResult> {
    if files.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::with_capacity(files.len());

    // All files share the same entity — extract from first candidate
    let first = &files[0].candidate;
    let title = first["title"].as_str().unwrap_or("").to_string();
    let provider = first["provider"].as_str().unwrap_or("tmdb").to_string();
    let provider_id = first["provider_id"]
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| first["provider_id"].as_u64().map(|n| n.to_string()))
        .unwrap_or_default();
    let year = first["year"].as_i64().map(|y| y as i32);
    let id_key = format!("{}_id", provider);

    // 1. Create/upsert the show ONCE
    let show_input = tv_shows::TvShowInput {
        title: title.clone(),
        year,
        identifiers: Some(json!({&id_key: provider_id})),
        ..Default::default()
    };
    let show_id =
        match tv_shows::upsert_by_provider(ctx.pool, &provider, &provider_id, show_input).await {
            Ok(id) => id,
            Err(e) => {
                let err = e.to_string();
                for f in files {
                    results.push(GroupFileResult {
                        file_id: f.file_id,
                        result: Err(err.clone()),
                    });
                }
                return results;
            }
        };

    let _ = sqlx::query("UPDATE tv_shows SET media_root_id = $1 WHERE id = $2")
        .bind(root.id)
        .bind(show_id)
        .execute(ctx.pool)
        .await;

    // 2. Season cache — upsert each unique season once
    let mut season_cache: std::collections::HashMap<u32, Uuid> = std::collections::HashMap::new();

    // 3. Process each file: parse interpretation, create season/episodes, create file record
    let mut set_added_batch: Vec<(Uuid, &str, Uuid)> = Vec::new();
    let mut search_file_ids: Vec<Uuid> = Vec::new();

    for fwc in files {
        let file_result =
            resolve_tv_file_in_group(ctx, root, fwc, show_id, &mut season_cache).await;
        match file_result {
            Ok((outcome, _episode_ids)) => {
                set_added_batch.push((fwc.file_id, "tv_episode", outcome.entity_id));
                search_file_ids.push(fwc.file_id);
                results.push(GroupFileResult {
                    file_id: fwc.file_id,
                    result: Ok(outcome),
                });
            }
            Err(e) => {
                results.push(GroupFileResult {
                    file_id: fwc.file_id,
                    result: Err(e),
                });
            }
        }
    }

    // 4. Batch operations — log failures so they don't silently leave files in 'review'
    if let Err(e) = media_root_files::set_added_batch(ctx.pool, &set_added_batch).await {
        tracing::error!(show_id = %show_id, error = %e, "Failed to mark files as added — files will stay in 'review' and may be re-processed");
    }
    let _ = search::batch_index_media_root_files(ctx.search, ctx.pool, &search_file_ids).await;

    // 5. ONE enrichment job for the whole show
    let display = crate::jobs::fetch_job_display(ctx.pool, "tv_show", show_id).await;
    let context = JobContext::for_entity("tv_show", show_id, format!("Enriching '{}'", title))
        .with_display_opt(display);
    let _ = crate::jobs::enqueue(
        ctx.pool,
        ctx.nats,
        "tv_show_enrich",
        json!({"show_id": show_id}),
        context,
    )
    .await;
    crate::jobs::enqueue_tv_show_index(ctx.pool, ctx.nats, show_id).await;

    results
}

/// Process a single TV file within a group (creates season/episodes/file record, enqueues probe).
async fn resolve_tv_file_in_group(
    ctx: &ResolveContext<'_>,
    _root: &MediaRootRow,
    fwc: &FileWithCandidate,
    show_id: Uuid,
    season_cache: &mut std::collections::HashMap<u32, Uuid>,
) -> Result<(ResolveOutcome, Vec<Uuid>), String> {
    let file = &fwc.file;

    let interp: InterpretResult = file
        .interpretation
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .ok_or("File has no interpretation (season/episode info)")?;

    let season_num = interp.season.ok_or("No season number in interpretation")?;
    let episodes = &interp.episodes;
    if episodes.is_empty() {
        return Err("No episode numbers in interpretation".to_string());
    }

    // Get or create season (cached)
    let season_id = if let Some(&sid) = season_cache.get(&season_num) {
        sid
    } else {
        let season_input = tv_seasons::SeasonInput {
            show_id,
            season_identifier: season_num.to_string(),
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
        let sid = tv_seasons::upsert(ctx.pool, season_input)
            .await
            .map_err(|e| e.to_string())?;
        season_cache.insert(season_num, sid);
        sid
    };

    // Find or create episodes using upsert
    let mut episode_ids: Vec<Uuid> = Vec::with_capacity(episodes.len());
    for &ep_num in episodes {
        let ep_id = tv_episodes::find_or_create(
            ctx.pool,
            season_id,
            &ep_num.to_string(),
            ep_num as i32,
            Some(&format!("Episode {}", ep_num)),
        )
        .await
        .map_err(|e| e.to_string())?;
        episode_ids.push(ep_id);
    }
    let first_episode_id = episode_ids[0];

    // Create episode file record
    let format = std::path::Path::new(&file.file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("unknown")
        .to_uppercase();
    let file_input = tv_episode_files::EpisodeFileInput {
        file_path: file.file_path.clone(),
        file_size: None,
        file_fingerprint: file.file_fingerprint.clone(),
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
    let ep_file_id = tv_episode_files::create_or_update(ctx.pool, file_input)
        .await
        .map_err(|e| format!("Failed to create episode file: {e}"))?;

    let replaced = tv_episode_files::link_episodes(ctx.pool, ep_file_id, &episode_ids)
        .await
        .map_err(|e| format!("Failed to link episodes to file: {e}"))?;
    crate::repositories::cleanup_replaced_files(&replaced).await;

    // Enqueue probe immediately so probes start while we're still processing
    let display = crate::jobs::fetch_job_display(ctx.pool, "tv_episode_file", ep_file_id).await;
    let probe_context = JobContext::for_entity(
        "tv_episode_file",
        ep_file_id,
        "Probing media file".to_string(),
    )
    .with_display_opt(display);
    let _ = crate::jobs::enqueue(
        ctx.pool,
        ctx.nats,
        "media_probe",
        json!({"file_id": ep_file_id, "media_type": "episode"}),
        probe_context,
    )
    .await;

    Ok((
        ResolveOutcome {
            entity_type: "tv_episode".to_string(),
            entity_id: first_episode_id,
        },
        episode_ids,
    ))
}

/// Resolve a group of sport files that all belong to the same league.
pub async fn resolve_sport_group(
    ctx: &ResolveContext<'_>,
    root: &MediaRootRow,
    files: &[FileWithCandidate],
) -> Vec<GroupFileResult> {
    if files.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::with_capacity(files.len());

    let first = &files[0].candidate;
    let league_name = first["title"]
        .as_str()
        .unwrap_or("Unknown League")
        .to_string();
    let provider = first["provider"]
        .as_str()
        .unwrap_or("thesportsdb")
        .to_string();
    let provider_id = first["provider_id"]
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| first["provider_id"].as_u64().map(|n| n.to_string()))
        .unwrap_or_default();
    let id_key = format!("{}_id", provider);

    // 1. Create/upsert the league ONCE
    let show_input = tv_shows::TvShowInput {
        title: league_name.clone(),
        identifiers: Some(json!({&id_key: provider_id})),
        content_category: Some("sport".to_string()),
        ..Default::default()
    };
    let show_id =
        match tv_shows::upsert_by_provider(ctx.pool, &provider, &provider_id, show_input).await {
            Ok(id) => id,
            Err(e) => {
                let err = e.to_string();
                for f in files {
                    results.push(GroupFileResult {
                        file_id: f.file_id,
                        result: Err(err.clone()),
                    });
                }
                return results;
            }
        };

    let _ = sqlx::query("UPDATE tv_shows SET media_root_id = $1 WHERE id = $2")
        .bind(root.id)
        .bind(show_id)
        .execute(ctx.pool)
        .await;

    // 2. Season + episode caches
    let mut season_cache: std::collections::HashMap<u32, Uuid> = std::collections::HashMap::new();
    let mut episode_cache: std::collections::HashMap<Uuid, Vec<tv_episodes::EpisodeRow>> =
        std::collections::HashMap::new();

    // 3. Process each file
    let mut set_added_batch: Vec<(Uuid, &str, Uuid)> = Vec::new();
    let mut search_file_ids: Vec<Uuid> = Vec::new();

    for fwc in files {
        let file_result = resolve_sport_file_in_group(
            ctx,
            fwc,
            show_id,
            &league_name,
            &mut season_cache,
            &mut episode_cache,
        )
        .await;
        match file_result {
            Ok((outcome, episode_id)) => {
                set_added_batch.push((fwc.file_id, "tv_episode", episode_id));
                search_file_ids.push(fwc.file_id);
                results.push(GroupFileResult {
                    file_id: fwc.file_id,
                    result: Ok(outcome),
                });
            }
            Err(e) => {
                results.push(GroupFileResult {
                    file_id: fwc.file_id,
                    result: Err(e),
                });
            }
        }
    }

    // 4. Batch operations — log failures so they don't silently leave files in 'review'
    if let Err(e) = media_root_files::set_added_batch(ctx.pool, &set_added_batch).await {
        tracing::error!(show_id = %show_id, error = %e, "Failed to mark files as added — files will stay in 'review' and may be re-processed");
    }
    let _ = search::batch_index_media_root_files(ctx.search, ctx.pool, &search_file_ids).await;

    // 5. ONE enrichment job
    let display = crate::jobs::fetch_job_display(ctx.pool, "tv_show", show_id).await;
    let context = JobContext::for_entity("tv_show", show_id, format!("Enriching '{league_name}'"))
        .with_display_opt(display);
    let _ = crate::jobs::enqueue(
        ctx.pool,
        ctx.nats,
        "sports_league_enrich",
        json!({"show_id": show_id}),
        context,
    )
    .await;

    results
}

/// Process a single sport file within a group.
async fn resolve_sport_file_in_group(
    ctx: &ResolveContext<'_>,
    fwc: &FileWithCandidate,
    show_id: Uuid,
    _league_name: &str,
    season_cache: &mut std::collections::HashMap<u32, Uuid>,
    episode_cache: &mut std::collections::HashMap<Uuid, Vec<tv_episodes::EpisodeRow>>,
) -> Result<(ResolveOutcome, Uuid), String> {
    let file = &fwc.file;

    let interp: Option<InterpretResult> = file
        .interpretation
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    let sport = interp
        .as_ref()
        .and_then(|i| i.raw.as_ref())
        .and_then(|r| r.media.as_sport());

    let round = sport.and_then(|s| s.round);
    let raw_session = sport.and_then(|s| s.session_type.as_deref());
    let interp_event_title = sport.and_then(|s| s.event_title.as_deref());

    let sidecar = file.sidecar_metadata.as_ref();
    let sidecar_title = sidecar
        .and_then(|s| s.get("title"))
        .and_then(|v| v.as_str());
    let event_title = interp_event_title.or(sidecar_title);

    let sidecar_season = sidecar
        .and_then(|s| s.get("video"))
        .and_then(|v| v.get("season"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let season_year = interp
        .as_ref()
        .and_then(|i| i.season)
        .or_else(|| interp.as_ref().and_then(|i| i.show_year))
        .or_else(|| sport.and_then(|s| s.year))
        .or(sidecar_season)
        .ok_or("No season year found in interpretation or sidecar metadata")?;

    // Get or create season (cached)
    let season_id = if let Some(&sid) = season_cache.get(&season_year) {
        sid
    } else {
        let season_input = tv_seasons::SeasonInput {
            show_id,
            season_identifier: season_year.to_string(),
            season_sort_order: season_year as i32,
            title: Some(season_year.to_string()),
            description: None,
            images: None,
            air_date: None,
            episode_count: None,
            is_monitored: false,
            is_special: false,
            auto_want_mode: "inherit".to_string(),
        };
        let sid = tv_seasons::upsert(ctx.pool, season_input)
            .await
            .map_err(|e| e.to_string())?;
        season_cache.insert(season_year, sid);
        sid
    };

    // Get episode list for this season (cached)
    if let std::collections::hash_map::Entry::Vacant(e) = episode_cache.entry(season_id) {
        let eps = tv_episodes::list_by_season(ctx.pool, season_id)
            .await
            .map_err(|err| format!("Failed to list episodes: {err}"))?;
        e.insert(eps);
    }
    let all_episodes = episode_cache.get(&season_id).unwrap();

    let episode_id = if let Some(id) =
        crate::jobs::handlers::match_sport_episode(round, raw_session, event_title, all_episodes)
    {
        id
    } else {
        let ep_title = event_title
            .map(String::from)
            .unwrap_or_else(|| format!("Round {}", round.unwrap_or(0)));
        let sort_order = round.unwrap_or(0) * 1000;
        let identifier = sort_order.to_string();

        let input = tv_episodes::EpisodeInput {
            season_id,
            episode_identifier: identifier,
            episode_sort_order: sort_order as i32,
            title: Some(ep_title),
            description: None,
            images: None,
            air_date: None,
            air_time: None,
            runtime: None,
            identifiers: serde_json::json!({}),
            country: None,
        };
        let new_id = tv_episodes::create(ctx.pool, input)
            .await
            .map_err(|e| format!("Failed to create episode: {e}"))?;
        // Invalidate episode cache for this season since we added one
        episode_cache.remove(&season_id);
        new_id
    };

    // Create episode file record
    let format = std::path::Path::new(&file.file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("unknown")
        .to_uppercase();
    let file_input = tv_episode_files::EpisodeFileInput {
        file_path: file.file_path.clone(),
        file_size: None,
        file_fingerprint: file.file_fingerprint.clone(),
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
    let ep_file_id = tv_episode_files::create_or_update(ctx.pool, file_input)
        .await
        .map_err(|e| format!("Failed to create episode file: {e}"))?;

    let replaced = tv_episode_files::link_episodes(ctx.pool, ep_file_id, &[episode_id])
        .await
        .map_err(|e| format!("Failed to link episode to file: {e}"))?;
    crate::repositories::cleanup_replaced_files(&replaced).await;

    // Enqueue probe
    let display = crate::jobs::fetch_job_display(ctx.pool, "tv_episode_file", ep_file_id).await;
    let probe_context = JobContext::for_entity(
        "tv_episode_file",
        ep_file_id,
        "Probing media file".to_string(),
    )
    .with_display_opt(display);
    let _ = crate::jobs::enqueue(
        ctx.pool,
        ctx.nats,
        "media_probe",
        json!({"file_id": ep_file_id, "media_type": "episode"}),
        probe_context,
    )
    .await;

    Ok((
        ResolveOutcome {
            entity_type: "tv_episode".to_string(),
            entity_id: episode_id,
        },
        episode_id,
    ))
}

/// Resolve a group of movie files that all belong to the same movie.
pub async fn resolve_movie_group(
    ctx: &ResolveContext<'_>,
    root: &MediaRootRow,
    files: &[FileWithCandidate],
) -> Vec<GroupFileResult> {
    if files.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::with_capacity(files.len());

    let first = &files[0].candidate;
    let title = first["title"].as_str().unwrap_or("").to_string();
    let provider = first["provider"].as_str().unwrap_or("tmdb").to_string();
    let provider_id = first["provider_id"]
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| first["provider_id"].as_u64().map(|n| n.to_string()))
        .unwrap_or_default();
    let year = first["year"].as_i64().map(|y| y as i32);
    let id_key = format!("{}_id", provider);

    // 1. Create/upsert the movie ONCE
    let movie_input = movies::MovieInput {
        title: title.clone(),
        year,
        identifiers: Some(json!({&id_key: provider_id})),
        ..Default::default()
    };
    let movie_id =
        match movies::upsert_by_provider(ctx.pool, &provider, &provider_id, movie_input).await {
            Ok(id) => id,
            Err(e) => {
                let err = e.to_string();
                for f in files {
                    results.push(GroupFileResult {
                        file_id: f.file_id,
                        result: Err(err.clone()),
                    });
                }
                return results;
            }
        };

    let _ = sqlx::query("UPDATE movies SET media_root_id = $1 WHERE id = $2")
        .bind(root.id)
        .bind(movie_id)
        .execute(ctx.pool)
        .await;

    // 2. Process each file
    let mut set_added_batch: Vec<(Uuid, &str, Uuid)> = Vec::new();
    let mut search_file_ids: Vec<Uuid> = Vec::new();

    for fwc in files {
        let file = &fwc.file;
        let format = std::path::Path::new(&file.file_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("unknown")
            .to_uppercase();
        let file_input = movie_files::MovieFileInput {
            movie_id,
            file_path: file.file_path.clone(),
            file_size: None,
            file_fingerprint: file.file_fingerprint.clone(),
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
        match movie_files::create_or_update(ctx.pool, file_input).await {
            Ok(movie_file_id) => {
                let replaced =
                    movie_files::delete_other_files_for_movie(ctx.pool, movie_id, movie_file_id)
                        .await
                        .unwrap_or_default();
                crate::repositories::cleanup_replaced_files(&replaced).await;

                // Enqueue probe
                let display =
                    crate::jobs::fetch_job_display(ctx.pool, "movie_file", movie_file_id).await;
                let probe_context = JobContext::for_entity(
                    "movie_file",
                    movie_file_id,
                    "Probing media file".to_string(),
                )
                .with_display_opt(display);
                let _ = crate::jobs::enqueue(
                    ctx.pool,
                    ctx.nats,
                    "media_probe",
                    json!({"file_id": movie_file_id, "media_type": "movie"}),
                    probe_context,
                )
                .await;

                set_added_batch.push((fwc.file_id, "movie", movie_id));
                search_file_ids.push(fwc.file_id);
                results.push(GroupFileResult {
                    file_id: fwc.file_id,
                    result: Ok(ResolveOutcome {
                        entity_type: "movie".to_string(),
                        entity_id: movie_id,
                    }),
                });
            }
            Err(e) => {
                results.push(GroupFileResult {
                    file_id: fwc.file_id,
                    result: Err(format!("Failed to create movie file: {e}")),
                });
            }
        }
    }

    // 3. Batch operations — log failures so they don't silently leave files in 'review'
    if let Err(e) = media_root_files::set_added_batch(ctx.pool, &set_added_batch).await {
        tracing::error!(movie_id = %movie_id, error = %e, "Failed to mark files as added — files will stay in 'review' and may be re-processed");
    }
    let _ = search::batch_index_media_root_files(ctx.search, ctx.pool, &search_file_ids).await;

    // 4. ONE enrichment job
    let display = crate::jobs::fetch_job_display(ctx.pool, "movie", movie_id).await;
    let context = JobContext::for_entity("movie", movie_id, format!("Enriching '{}'", title))
        .with_display_opt(display);
    let _ = crate::jobs::enqueue(
        ctx.pool,
        ctx.nats,
        "movie_enrich",
        json!({"movie_id": movie_id.to_string()}),
        context,
    )
    .await;
    crate::jobs::enqueue_movie_index(ctx.pool, ctx.nats, movie_id).await;

    results
}

/// Resolve a group of book/audiobook files that all belong to the same book.
pub async fn resolve_book_group(
    ctx: &ResolveContext<'_>,
    root: &MediaRootRow,
    files: &[FileWithCandidate],
) -> Vec<GroupFileResult> {
    if files.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::with_capacity(files.len());

    let first = &files[0].candidate;
    let title = first["title"].as_str().unwrap_or("").to_string();
    let provider = first["provider"].as_str().unwrap_or("");
    let provider_id = first["provider_id"].as_str().unwrap_or("");

    // Build identifiers
    let mut identifiers = serde_json::Map::new();
    if let Some(isbn) = first["isbn"].as_str() {
        identifiers.insert("isbn".to_string(), json!(isbn));
    }
    if !provider.is_empty() && !provider_id.is_empty() {
        identifiers.insert(format!("{provider}_id"), json!(provider_id));
    }

    let images = first["cover_url"].as_str().map(|url| {
        let mut map = crate::images::ImageMap::new();
        map.insert(
            "cover".to_string(),
            crate::images::ImageEntry {
                source_url: Some(url.to_string()),
                ..Default::default()
            },
        );
        map
    });

    let want_format = if root.media_type == "audiobook" {
        Some("audiobook".to_string())
    } else {
        None
    };

    // Look up existing book by identifier to avoid creating duplicates
    let mut existing_id = if !provider.is_empty() && !provider_id.is_empty() {
        let id_key = format!("{provider}_id");
        books::find_by_identifier(ctx.pool, &id_key, provider_id)
            .await
            .ok()
            .flatten()
            .map(|m| m.id)
    } else {
        None
    };

    // Fallback: check for existing library book by title + author to avoid
    // creating duplicates (e.g. audiobook when ebook already exists).
    if existing_id.is_none() {
        let author_names: Vec<String> = first["authors"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        existing_id =
            books::find_library_book_by_title_and_author_names(ctx.pool, &title, &author_names)
                .await
                .ok()
                .flatten()
                .map(|m| m.id);
    }

    // Last-resort fallback: title-only match within the same media root. Catches
    // cases where the existing book has no authors linked yet (so the
    // title+author check can't fire) or where two providers tag the same book
    // under unrelated identifier keys (so the identifier check misses).
    if existing_id.is_none() && !title.is_empty() {
        existing_id = books::find_library_book_by_title_in_media_root(ctx.pool, &title, root.id)
            .await
            .ok()
            .flatten()
            .map(|m| m.id);
    }

    // 1. Create/upsert the book ONCE
    let book_input = books::BookInput {
        id: existing_id,
        title: title.clone(),
        identifiers: if identifiers.is_empty() {
            None
        } else {
            Some(json!(identifiers))
        },
        images,
        want_format,
        ..Default::default()
    };
    let book_id = match books::upsert_library_book(ctx.pool, book_input).await {
        Ok(id) => id,
        Err(e) => {
            let err = e.to_string();
            for f in files {
                results.push(GroupFileResult {
                    file_id: f.file_id,
                    result: Err(err.clone()),
                });
            }
            return results;
        }
    };

    let _ = sqlx::query("UPDATE books SET media_root_id = $1 WHERE id = $2")
        .bind(root.id)
        .bind(book_id)
        .execute(ctx.pool)
        .await;

    // Link authors ONCE
    if let Some(author_arr) = first["authors"].as_array() {
        for author_val in author_arr {
            if let Some(name) = author_val.as_str()
                && let Ok(author_id) = authors::get_or_create_by_name(ctx.pool, name).await
            {
                let _ = books::link_author(ctx.pool, book_id, author_id).await;
            }
        }
    }

    // 2. Process each file
    let mut set_added_batch: Vec<(Uuid, &str, Uuid)> = Vec::new();
    let mut search_file_ids: Vec<Uuid> = Vec::new();
    let mut has_audio = false;

    for fwc in files {
        let file = &fwc.file;
        let format = std::path::Path::new(&file.file_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("unknown")
            .to_uppercase();
        let is_audio =
            crate::routes::ingest::AUDIOBOOK_EXTENSIONS.contains(&format.to_lowercase().as_str());

        let file_ok = if is_audio {
            has_audio = true;
            audiobook_files::upsert_by_path(
                ctx.pool,
                audiobook_files::AudiobookFileInput {
                    book_id,
                    format,
                    file_path: file.file_path.clone(),
                    file_size: file.file_size.unwrap_or(0),
                    source_type: Some("media_root".to_string()),
                    ..Default::default()
                },
            )
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
        } else {
            let file_input = book_files::BookFileInput {
                book_id,
                format,
                size_bytes: file.file_size.unwrap_or(0),
                file_path: file.file_path.clone(),
                source_type: Some("media_root".to_string()),
                ..Default::default()
            };
            book_files::upsert(ctx.pool, file_input)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        };

        match file_ok {
            Ok(()) => {
                set_added_batch.push((fwc.file_id, "book", book_id));
                search_file_ids.push(fwc.file_id);
                results.push(GroupFileResult {
                    file_id: fwc.file_id,
                    result: Ok(ResolveOutcome {
                        entity_type: "book".to_string(),
                        entity_id: book_id,
                    }),
                });
            }
            Err(e) => {
                results.push(GroupFileResult {
                    file_id: fwc.file_id,
                    result: Err(e),
                });
            }
        }
    }

    // 3. Batch operations — log failures so they don't silently leave files in 'review'
    if let Err(e) = media_root_files::set_added_batch(ctx.pool, &set_added_batch).await {
        tracing::error!(book_id = %book_id, error = %e, "Failed to mark files as added — files will stay in 'review' and may be re-processed");
    }
    let _ = search::batch_index_media_root_files(ctx.search, ctx.pool, &search_file_ids).await;

    // 4. Reindex audiobook files in natural sort order, then probe
    if has_audio && let Err(e) = audiobook_files::reindex_file_order(ctx.pool, book_id).await {
        tracing::warn!(book_id = %book_id, error = %e, "Failed to reindex audiobook file order");
    }
    if has_audio {
        crate::jobs::enqueue_audiobook_probe(ctx.pool, ctx.nats, book_id, &title).await;
    }

    // 5. Auto-sync entitlements ONCE
    crate::auto_sync::create_auto_sync_entitlements(ctx.pool, ctx.nats, book_id).await;

    // 6. ONE enrichment job
    let display = crate::jobs::fetch_job_display(ctx.pool, "book", book_id).await;
    let context = JobContext::for_entity("book", book_id, format!("Enriching '{}'", title))
        .with_display_opt(display);
    let _ = crate::jobs::enqueue(
        ctx.pool,
        ctx.nats,
        "book_enrich",
        json!({"book_id": book_id.to_string()}),
        context,
    )
    .await;
    crate::jobs::enqueue_book_index(ctx.pool, ctx.nats, book_id).await;

    results
}

// =============================================================================
// Manual linking (no provider required)
// =============================================================================

/// Link a media root file to an already-existing library entity.
///
/// Unlike `resolve_file`, this does NOT create the entity or enqueue enrichment.
/// It only creates the physical file record, marks the media root file as added,
/// and enqueues probe jobs where appropriate.
pub async fn link_file_to_entity(
    ctx: &ResolveContext<'_>,
    root: &MediaRootRow,
    file: &MediaRootFileRow,
    file_id: Uuid,
    entity_type: &str,
    entity_id: Uuid,
) -> Result<ResolveOutcome, String> {
    match entity_type {
        "book" => link_to_book(ctx, root, file, file_id, entity_id).await,
        "movie" => link_to_movie(ctx, root, file, file_id, entity_id).await,
        "tv_episode" => link_to_tv_episode(ctx, root, file, file_id, entity_id).await,
        other => Err(format!("Unsupported entity type '{other}'")),
    }
}

async fn link_to_book(
    ctx: &ResolveContext<'_>,
    root: &MediaRootRow,
    file: &MediaRootFileRow,
    file_id: Uuid,
    book_id: Uuid,
) -> Result<ResolveOutcome, String> {
    // Validate book exists
    let title: String = sqlx::query_scalar("SELECT title FROM books WHERE id = $1")
        .bind(book_id)
        .fetch_optional(ctx.pool)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("Book not found")?;

    // Set media_root_id on the book if not already set
    let _ =
        sqlx::query("UPDATE books SET media_root_id = COALESCE(media_root_id, $1) WHERE id = $2")
            .bind(root.id)
            .bind(book_id)
            .execute(ctx.pool)
            .await;

    // Create file record — audiobook formats go to audiobook_files, ebooks to book_files
    let format = std::path::Path::new(&file.file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("unknown")
        .to_uppercase();
    let is_audio =
        crate::routes::ingest::AUDIOBOOK_EXTENSIONS.contains(&format.to_lowercase().as_str());
    if is_audio {
        let _ = audiobook_files::upsert_by_path(
            ctx.pool,
            audiobook_files::AudiobookFileInput {
                book_id,
                format,
                file_path: file.file_path.clone(),
                file_size: file.file_size.unwrap_or(0),
                source_type: Some("media_root".to_string()),
                ..Default::default()
            },
        )
        .await;

        crate::jobs::enqueue_audiobook_probe(ctx.pool, ctx.nats, book_id, &title).await;
    } else {
        let file_input = book_files::BookFileInput {
            book_id,
            format,
            size_bytes: file.file_size.unwrap_or(0),
            file_path: file.file_path.clone(),
            source_type: Some("media_root".to_string()),
            ..Default::default()
        };
        let _ = book_files::upsert(ctx.pool, file_input).await;
    }

    // Create per-device entitlements for auto-sync devices
    crate::auto_sync::create_auto_sync_entitlements(ctx.pool, ctx.nats, book_id).await;

    // Mark file as added
    let _ = media_root_files::set_added(ctx.pool, file_id, "book", book_id).await;
    // Sidecars written by enrichment job after full metadata is available
    let _ = search::index_media_root_file(ctx.search, ctx.pool, file_id).await;
    crate::jobs::enqueue_book_index(ctx.pool, ctx.nats, book_id).await;

    Ok(ResolveOutcome {
        entity_type: "book".to_string(),
        entity_id: book_id,
    })
}

async fn link_to_movie(
    ctx: &ResolveContext<'_>,
    root: &MediaRootRow,
    file: &MediaRootFileRow,
    file_id: Uuid,
    movie_id: Uuid,
) -> Result<ResolveOutcome, String> {
    // Validate movie exists
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM movies WHERE id = $1")
        .bind(movie_id)
        .fetch_optional(ctx.pool)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("Movie not found")?;

    // Set media_root_id on the movie if not already set
    let _ =
        sqlx::query("UPDATE movies SET media_root_id = COALESCE(media_root_id, $1) WHERE id = $2")
            .bind(root.id)
            .bind(movie_id)
            .execute(ctx.pool)
            .await;

    // Create movie file record
    let format = std::path::Path::new(&file.file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("unknown")
        .to_uppercase();
    let file_input = movie_files::MovieFileInput {
        movie_id,
        file_path: file.file_path.clone(),
        file_size: None,
        file_fingerprint: file.file_fingerprint.clone(),
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
    let movie_file_id = movie_files::create_or_update(ctx.pool, file_input)
        .await
        .map_err(|e| format!("Failed to create movie file: {e}"))?;

    let replaced = movie_files::delete_other_files_for_movie(ctx.pool, movie_id, movie_file_id)
        .await
        .unwrap_or_default();
    crate::repositories::cleanup_replaced_files(&replaced).await;

    // Enqueue media probe
    let display = crate::jobs::fetch_job_display(ctx.pool, "movie_file", movie_file_id).await;
    let probe_context = JobContext::for_entity(
        "movie_file",
        movie_file_id,
        "Probing media file".to_string(),
    )
    .with_display_opt(display);
    let _ = crate::jobs::enqueue(
        ctx.pool,
        ctx.nats,
        "media_probe",
        json!({"file_id": movie_file_id, "media_type": "movie"}),
        probe_context,
    )
    .await;

    // Mark file as added
    let _ = media_root_files::set_added(ctx.pool, file_id, "movie", movie_id).await;
    // Sidecars written by enrichment job after full metadata is available
    let _ = search::index_media_root_file(ctx.search, ctx.pool, file_id).await;
    crate::jobs::enqueue_movie_index(ctx.pool, ctx.nats, movie_id).await;

    Ok(ResolveOutcome {
        entity_type: "movie".to_string(),
        entity_id: movie_id,
    })
}

async fn link_to_tv_episode(
    ctx: &ResolveContext<'_>,
    _root: &MediaRootRow,
    file: &MediaRootFileRow,
    file_id: Uuid,
    episode_id: Uuid,
) -> Result<ResolveOutcome, String> {
    // Validate episode exists
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM tv_episodes WHERE id = $1")
        .bind(episode_id)
        .fetch_optional(ctx.pool)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("TV episode not found")?;

    // Create episode file record
    let format = std::path::Path::new(&file.file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("unknown")
        .to_uppercase();
    let file_input = tv_episode_files::EpisodeFileInput {
        file_path: file.file_path.clone(),
        file_size: None,
        file_fingerprint: file.file_fingerprint.clone(),
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
    let ep_file_id = tv_episode_files::create_or_update(ctx.pool, file_input)
        .await
        .map_err(|e| format!("Failed to create episode file: {e}"))?;

    // Link episode to file via junction table
    let replaced = tv_episode_files::link_episodes(ctx.pool, ep_file_id, &[episode_id])
        .await
        .map_err(|e| format!("Failed to link episode to file: {e}"))?;
    crate::repositories::cleanup_replaced_files(&replaced).await;

    // Enqueue media probe
    let display = crate::jobs::fetch_job_display(ctx.pool, "tv_episode_file", ep_file_id).await;
    let probe_context = JobContext::for_entity(
        "tv_episode_file",
        ep_file_id,
        "Probing media file".to_string(),
    )
    .with_display_opt(display);
    let _ = crate::jobs::enqueue(
        ctx.pool,
        ctx.nats,
        "media_probe",
        json!({"file_id": ep_file_id, "media_type": "episode"}),
        probe_context,
    )
    .await;

    // Mark file as added
    let _ = media_root_files::set_added(ctx.pool, file_id, "tv_episode", episode_id).await;
    // Sidecars written by enrichment job after full metadata is available
    let _ = search::index_media_root_file(ctx.search, ctx.pool, file_id).await;

    Ok(ResolveOutcome {
        entity_type: "tv_episode".to_string(),
        entity_id: episode_id,
    })
}

/// Resolve a library entity from a provider id that may not be unique.
///
/// Migration 0093 left `identifiers->>'tvdb_id'` non-unique on purpose — a
/// reboot shares the original's TVDB entity — so prefer the row the candidate's
/// year names, then fall back to the same total order the provider upserts use
/// (`repositories::AMBIGUOUS_ROW_ORDER`). Picking arbitrarily here filed
/// episodes under the wrong reboot, surfacing as a spurious "No episode SxxEyy".
async fn resolve_entity_by_provider_id(
    pool: &sqlx::PgPool,
    table: &str,
    id_key: &str,
    provider_id: &str,
    year: Option<i32>,
) -> Result<Option<Uuid>, sqlx::Error> {
    let order = crate::repositories::AMBIGUOUS_ROW_ORDER;

    if let Some(year) = year {
        let sql = format!(
            "SELECT id FROM {table} WHERE identifiers->>$1 = $2 AND year = $3 {order} LIMIT 1"
        );
        let found = sqlx::query_scalar::<_, Uuid>(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(id_key)
            .bind(provider_id)
            .bind(year)
            .fetch_optional(pool)
            .await?;
        if found.is_some() {
            return Ok(found);
        }
    }

    let sql = format!("SELECT id FROM {table} WHERE identifiers->>$1 = $2 {order} LIMIT 1");
    sqlx::query_scalar::<_, Uuid>(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id_key)
        .bind(provider_id)
        .fetch_optional(pool)
        .await
}

/// Link a media root file to an existing library entity by provider_id.
///
/// Unlike `resolve_file`, this does NOT create entities or enqueue enrichment.
/// It finds the existing entity by provider ID, resolves season/episode for TV,
/// and links the file using `link_to_movie` or `link_to_tv_episode`.
pub async fn link_to_library(
    ctx: &ResolveContext<'_>,
    root: &MediaRootRow,
    file: &MediaRootFileRow,
    file_id: Uuid,
    candidate: &serde_json::Value,
) -> Result<ResolveOutcome, String> {
    let provider = candidate["provider"].as_str().unwrap_or("tmdb");
    let provider_id = candidate["provider_id"]
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| candidate["provider_id"].as_u64().map(|n| n.to_string()))
        .unwrap_or_default();
    let candidate_title = candidate["title"].as_str().unwrap_or("unknown");
    let candidate_year = candidate["year"].as_i64().map(|y| y as i32);
    let id_key = format!("{}_id", provider);

    match root.media_type.as_str() {
        "movie" => {
            let movie_id: Uuid = resolve_entity_by_provider_id(
                ctx.pool,
                "movies",
                &id_key,
                &provider_id,
                candidate_year,
            )
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                format!("No movie '{candidate_title}' in library ({provider} {provider_id})")
            })?;

            link_to_movie(ctx, root, file, file_id, movie_id).await
        }
        "tv" => {
            let show_id: Uuid = resolve_entity_by_provider_id(
                ctx.pool,
                "tv_shows",
                &id_key,
                &provider_id,
                candidate_year,
            )
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                format!("No show '{candidate_title}' in library ({provider} {provider_id})")
            })?;

            // Set media_root_id on the show if not already set
            let _ = sqlx::query(
                "UPDATE tv_shows SET media_root_id = COALESCE(media_root_id, $1) WHERE id = $2",
            )
            .bind(root.id)
            .bind(show_id)
            .execute(ctx.pool)
            .await;

            // Parse interpretation for season/episode numbers
            let interp: InterpretResult = file
                .interpretation
                .as_ref()
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .ok_or("File has no interpretation (season/episode info)")?;

            let season_num = interp.season.ok_or("No season number in interpretation")?;
            let ep_num = interp
                .episodes
                .first()
                .ok_or("No episode numbers in interpretation")?;

            // Find existing episode by show → season → episode
            let episode_id: Uuid = sqlx::query_scalar(
                r#"
                SELECT e.id FROM tv_episodes e
                JOIN tv_seasons s ON s.id = e.season_id
                WHERE s.show_id = $1
                  AND s.season_sort_order = $2
                  AND e.episode_sort_order = $3
                "#,
            )
            .bind(show_id)
            .bind(season_num as i32)
            .bind(*ep_num as i32)
            .fetch_optional(ctx.pool)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                format!(
                    "No episode S{:02}E{:02} in '{candidate_title}' ({provider} {provider_id})",
                    season_num, ep_num
                )
            })?;

            link_to_tv_episode(ctx, root, file, file_id, episode_id).await
        }
        other => Err(format!(
            "Library linking not supported for media type '{other}'"
        )),
    }
}
