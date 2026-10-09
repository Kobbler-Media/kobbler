//! Per-entity sidecar write-back with merge.
//!
//! Builds `SidecarMetadata` from DB entities and writes sidecars to disk, merging
//! with any existing sidecar data to preserve fields Kobbler doesn't manage
//! (e.g. sonarr_id, custom uniqueids, narrators, chapters).

use crate::repositories::{
    book_files, books, media_root_files, media_roots, movie_files, movies, tv_episode_files,
    tv_episodes, tv_shows,
};
use crate::sidecar::{
    BookSidecar, NfoType, SidecarFormat, SidecarIdentifiers, SidecarMetadata, VideoSidecar,
};
use sqlx::PgPool;
use std::path::{Path, PathBuf};
use tracing::warn;
use uuid::Uuid;

// =============================================================================
// Merge
// =============================================================================

/// Merge enriched sidecar data over existing on-disk data.
///
/// - For `Option` fields: enriched wins when `Some`, else keep existing.
/// - For `Vec` fields: enriched wins when non-empty, else keep existing.
/// - `extra_uniqueids`: keep all existing entries (enriched doesn't produce these).
pub fn merge_sidecar(existing: &SidecarMetadata, enriched: &SidecarMetadata) -> SidecarMetadata {
    SidecarMetadata {
        source_format: enriched.source_format.or(existing.source_format),
        title: enriched.title.clone().or_else(|| existing.title.clone()),
        original_title: enriched
            .original_title
            .clone()
            .or_else(|| existing.original_title.clone()),
        year: enriched.year.or(existing.year),
        plot: enriched.plot.clone().or_else(|| existing.plot.clone()),
        rating: enriched.rating.or(existing.rating),
        genres: if enriched.genres.is_empty() {
            existing.genres.clone()
        } else {
            enriched.genres.clone()
        },
        identifiers: merge_identifiers(&existing.identifiers, &enriched.identifiers),
        video: merge_video(existing.video.as_ref(), enriched.video.as_ref()),
        book: merge_book(existing.book.as_ref(), enriched.book.as_ref()),
    }
}

fn merge_identifiers(
    existing: &SidecarIdentifiers,
    enriched: &SidecarIdentifiers,
) -> SidecarIdentifiers {
    SidecarIdentifiers {
        imdb_id: enriched
            .imdb_id
            .clone()
            .or_else(|| existing.imdb_id.clone()),
        tmdb_id: enriched
            .tmdb_id
            .clone()
            .or_else(|| existing.tmdb_id.clone()),
        tvdb_id: enriched
            .tvdb_id
            .clone()
            .or_else(|| existing.tvdb_id.clone()),
        tvmaze_id: enriched
            .tvmaze_id
            .clone()
            .or_else(|| existing.tvmaze_id.clone()),
        isbn: enriched.isbn.clone().or_else(|| existing.isbn.clone()),
        asin: enriched.asin.clone().or_else(|| existing.asin.clone()),
        openlibrary_id: enriched
            .openlibrary_id
            .clone()
            .or_else(|| existing.openlibrary_id.clone()),
        hardcover_id: enriched
            .hardcover_id
            .clone()
            .or_else(|| existing.hardcover_id.clone()),
        kobbler_id: enriched
            .kobbler_id
            .clone()
            .or_else(|| existing.kobbler_id.clone()),
        // Keep all existing extra uniqueids — enriched doesn't produce these
        extra_uniqueids: existing.extra_uniqueids.clone(),
    }
}

fn merge_video(
    existing: Option<&VideoSidecar>,
    enriched: Option<&VideoSidecar>,
) -> Option<VideoSidecar> {
    match (existing, enriched) {
        (_, Some(e)) => {
            let base = existing.cloned().unwrap_or_default();
            Some(VideoSidecar {
                nfo_type: e.nfo_type.or(base.nfo_type),
                season: e.season.or(base.season),
                episode: e.episode.or(base.episode),
                aired: e.aired.clone().or(base.aired),
                mpaa: e.mpaa.clone().or(base.mpaa),
                studio: e.studio.clone().or(base.studio),
                status: e.status.clone().or(base.status),
                cast: if e.cast.is_empty() {
                    base.cast
                } else {
                    e.cast.clone()
                },
            })
        }
        (Some(e), None) => Some(e.clone()),
        (None, None) => None,
    }
}

fn merge_book(
    existing: Option<&BookSidecar>,
    enriched: Option<&BookSidecar>,
) -> Option<BookSidecar> {
    match (existing, enriched) {
        (_, Some(e)) => {
            let base = existing.cloned().unwrap_or_default();
            Some(BookSidecar {
                authors: if e.authors.is_empty() {
                    base.authors
                } else {
                    e.authors.clone()
                },
                narrators: if e.narrators.is_empty() {
                    base.narrators
                } else {
                    e.narrators.clone()
                },
                publisher: e.publisher.clone().or(base.publisher),
                language: e.language.clone().or(base.language),
                series: e.series.clone().or(base.series),
                series_index: e.series_index.or(base.series_index),
                description: e.description.clone().or(base.description),
                abridged: e.abridged || base.abridged,
                chapters: if e.chapters.is_empty() {
                    base.chapters
                } else {
                    e.chapters.clone()
                },
            })
        }
        (Some(e), None) => Some(e.clone()),
        (None, None) => None,
    }
}

// =============================================================================
// DB → SidecarMetadata builders
// =============================================================================

pub async fn build_movie_sidecar(pool: &PgPool, movie_id: Uuid) -> Option<SidecarMetadata> {
    let movie = movies::get_with_details(pool, movie_id).await.ok()??;

    let tmdb_id = movie
        .identifiers
        .get("tmdb_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let imdb_id = movie
        .identifiers
        .get("imdb_id")
        .and_then(|v| v.as_str())
        .map(String::from);

    Some(SidecarMetadata {
        title: Some(movie.title),
        original_title: movie.original_title,
        year: movie.year.map(|y| y as u16),
        plot: movie.description,
        rating: movie.rating,
        genres: movie.genres.unwrap_or_default(),
        identifiers: SidecarIdentifiers {
            tmdb_id,
            imdb_id,
            kobbler_id: Some(movie_id.to_string()),
            ..Default::default()
        },
        video: Some(VideoSidecar {
            nfo_type: Some(NfoType::Movie),
            mpaa: movie.content_rating,
            ..Default::default()
        }),
        ..Default::default()
    })
}

pub async fn build_tvshow_sidecar(pool: &PgPool, show_id: Uuid) -> Option<SidecarMetadata> {
    let show = tv_shows::get(pool, show_id).await.ok()??;

    let tmdb_id = show
        .identifiers
        .get("tmdb_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let imdb_id = show
        .identifiers
        .get("imdb_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let tvdb_id = show
        .identifiers
        .get("tvdb_id")
        .and_then(|v| v.as_str())
        .map(String::from);

    Some(SidecarMetadata {
        title: Some(show.title),
        year: show.year.map(|y| y as u16),
        identifiers: SidecarIdentifiers {
            tmdb_id,
            imdb_id,
            tvdb_id,
            kobbler_id: Some(show_id.to_string()),
            ..Default::default()
        },
        video: Some(VideoSidecar {
            nfo_type: Some(NfoType::TvShow),
            status: show.status,
            studio: show.network,
            ..Default::default()
        }),
        ..Default::default()
    })
}

pub async fn build_episode_sidecar(pool: &PgPool, episode_id: Uuid) -> Option<SidecarMetadata> {
    let ep = tv_episodes::get(pool, episode_id).await.ok()??;

    let season_num = ep.episode_identifier.parse::<u32>().ok();
    let season_sort_order: Option<i32> =
        sqlx::query_scalar("SELECT ts.season_sort_order FROM tv_seasons ts WHERE ts.id = $1")
            .bind(ep.season_id)
            .fetch_optional(pool)
            .await
            .ok()?;
    let season_sort_order = season_sort_order?;

    // Episode-level identifiers only — never use show-level IDs in episode NFOs
    let ep_tmdb_id = ep
        .identifiers
        .get("tmdb_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let ep_tvdb_id = ep
        .identifiers
        .get("tvdb_id")
        .and_then(|v| v.as_str())
        .map(String::from);

    Some(SidecarMetadata {
        title: ep.title,
        plot: ep.description,
        identifiers: SidecarIdentifiers {
            tmdb_id: ep_tmdb_id,
            tvdb_id: ep_tvdb_id,
            kobbler_id: Some(episode_id.to_string()),
            ..Default::default()
        },
        video: Some(VideoSidecar {
            nfo_type: Some(NfoType::Episode),
            season: Some(season_sort_order as u32),
            episode: season_num,
            aired: ep.air_date.map(|d| d.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    })
}

pub async fn build_book_sidecar(pool: &PgPool, book_id: Uuid) -> Option<SidecarMetadata> {
    let book = books::get_by_id(pool, book_id).await.ok()??;

    let authors: Vec<String> = book
        .authors
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let isbn = book
        .identifiers
        .get("isbn")
        .and_then(|v| v.as_str())
        .map(String::from);
    let asin = book
        .identifiers
        .get("asin")
        .and_then(|v| v.as_str())
        .map(String::from);

    let narrators: Vec<String> = book
        .narrator
        .iter()
        .flat_map(|n| n.split(", "))
        .map(String::from)
        .collect();

    Some(SidecarMetadata {
        title: Some(book.title),
        plot: book.description.clone(),
        rating: book.rating,
        genres: book.genres,
        identifiers: SidecarIdentifiers {
            isbn,
            asin,
            kobbler_id: Some(book_id.to_string()),
            ..Default::default()
        },
        book: Some(BookSidecar {
            authors,
            narrators,
            publisher: book.publisher,
            language: book.language,
            series: book.series_name,
            series_index: book.series_index,
            description: book.description,
            abridged: book.is_abridged,
            ..Default::default()
        }),
        ..Default::default()
    })
}

// =============================================================================
// Per-entity write functions (best-effort, never fail callers)
// =============================================================================

/// Write movie sidecar (NFO) for all added files of a movie.
pub async fn write_movie_sidecars(pool: &PgPool, movie_id: Uuid) {
    if let Err(e) = write_movie_sidecars_inner(pool, movie_id).await {
        warn!(movie_id = %movie_id, error = %e, "Failed to write movie sidecars");
    }
}

async fn write_movie_sidecars_inner(
    pool: &PgPool,
    movie_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Check media root write_sidecars flag
    let media_root_id: Option<Uuid> =
        sqlx::query_scalar("SELECT media_root_id FROM movies WHERE id = $1")
            .bind(movie_id)
            .fetch_optional(pool)
            .await?
            .flatten();

    let Some(mr_id) = media_root_id else {
        return Ok(());
    };
    let root = media_roots::get_by_id(pool, mr_id)
        .await?
        .ok_or("media root not found")?;
    if !root.write_sidecars {
        return Ok(());
    }

    let file_path_str = resolve_movie_file_path(pool, movie_id).await?;
    let Some(file_path_str) = file_path_str else {
        return Ok(());
    };

    let enriched = match build_movie_sidecar(pool, movie_id).await {
        Some(m) => m,
        None => return Ok(()),
    };

    let file_path = Path::new(&file_path_str);
    let dir = file_path.parent().ok_or("no parent dir")?;
    let stem = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("movie");

    // Read existing, merge, write
    let existing = crate::sidecar::read_sidecar(file_path, crate::sidecar::MediaType::Movie).await;
    let merged = match existing {
        Some(ref ex) => merge_sidecar(ex, &enriched),
        None => enriched,
    };

    crate::sidecar::write_sidecar(dir, stem, &merged, &[SidecarFormat::Nfo]).await?;
    Ok(())
}

/// Every episode file of a show, `media_root_files` first with
/// `tv_episode_files` as the fallback, ordered by path.
///
/// The order is the invariant: `tvshow.nfo`, the artwork and any per-episode
/// run all derive the show directory from the first entry, so a show whose
/// episodes span two directories must resolve to the same one in all of them
/// or each pass writes its own orphan copy.
async fn show_episode_files(
    pool: &PgPool,
    show_id: Uuid,
) -> Result<Vec<(Uuid, String)>, Box<dyn std::error::Error + Send + Sync>> {
    let files: Vec<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT mrf.entity_id, mrf.file_path
        FROM media_root_files mrf
        JOIN tv_episodes te ON te.id = mrf.entity_id
        JOIN tv_seasons ts ON ts.id = te.season_id
        WHERE ts.show_id = $1
          AND mrf.entity_type = 'tv_episode'
          AND mrf.match_status = 'added'
        ORDER BY mrf.file_path
        "#,
    )
    .bind(show_id)
    .fetch_all(pool)
    .await?;

    if !files.is_empty() {
        return Ok(files);
    }

    Ok(sqlx::query_as(
        r#"
        SELECT tefe.episode_id, tef.file_path
        FROM tv_episode_files tef
        JOIN tv_episode_file_episodes tefe ON tefe.episode_file_id = tef.id
        JOIN tv_episodes te ON te.id = tefe.episode_id
        JOIN tv_seasons ts ON ts.id = te.season_id
        WHERE ts.show_id = $1
          AND tef.available = true
        ORDER BY tef.file_path
        "#,
    )
    .bind(show_id)
    .fetch_all(pool)
    .await?)
}

/// Show directory of an ordered episode file list: up two levels from the first
/// path (`Show/Season XX/file.mkv`).
fn show_dir_from(episode_files: &[(Uuid, String)]) -> Option<PathBuf> {
    let first = episode_files.first()?;
    Path::new(&first.1)
        .parent()
        .and_then(|s| s.parent())
        .map(Path::to_path_buf)
}

/// The one directory a show's `tvshow.nfo` and artwork belong in.
async fn resolve_show_dir(
    pool: &PgPool,
    show_id: Uuid,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(show_dir_from(&show_episode_files(pool, show_id).await?))
}

/// Write TV show sidecar (tvshow.nfo) and per-episode NFOs for all added files.
pub async fn write_tvshow_sidecars(pool: &PgPool, show_id: Uuid) {
    if let Err(e) = write_tvshow_sidecars_inner(pool, show_id).await {
        warn!(show_id = %show_id, error = %e, "Failed to write tvshow sidecars");
    }
}

async fn write_tvshow_sidecars_inner(
    pool: &PgPool,
    show_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Check media root write_sidecars flag
    let media_root_id: Option<Uuid> =
        sqlx::query_scalar("SELECT media_root_id FROM tv_shows WHERE id = $1")
            .bind(show_id)
            .fetch_optional(pool)
            .await?
            .flatten();

    let Some(mr_id) = media_root_id else {
        return Ok(());
    };
    let root = media_roots::get_by_id(pool, mr_id)
        .await?
        .ok_or("media root not found")?;
    if !root.write_sidecars {
        return Ok(());
    }

    let episode_files = show_episode_files(pool, show_id).await?;
    if episode_files.is_empty() {
        return Ok(());
    }

    let show_dir = show_dir_from(&episode_files).ok_or("cannot determine show dir")?;
    let show_dir = show_dir.as_path();

    // Write tvshow.nfo (merged)
    if let Some(enriched) = build_tvshow_sidecar(pool, show_id).await {
        let existing = crate::sidecar::read_tvshow_nfo(show_dir).await;
        let merged = match existing {
            Some(ref ex) => merge_sidecar(ex, &enriched),
            None => enriched,
        };
        crate::sidecar::write_sidecar(show_dir, "tvshow", &merged, &[SidecarFormat::Nfo]).await?;

        // Clean up stale movie.nfo that was incorrectly written for TV shows
        cleanup_stale_movie_nfo(show_dir);
    }

    // Write per-episode NFOs
    for (episode_id, file_path_str) in &episode_files {
        if let Some(enriched) = build_episode_sidecar(pool, *episode_id).await {
            let file_path = Path::new(file_path_str);
            let dir = match file_path.parent() {
                Some(d) => d,
                None => continue,
            };
            let stem = file_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("episode");

            let existing =
                crate::sidecar::read_sidecar(file_path, crate::sidecar::MediaType::TvEpisode).await;
            let merged = match existing {
                Some(ref ex) => merge_sidecar(ex, &enriched),
                None => enriched,
            };
            let _ = crate::sidecar::write_sidecar(dir, stem, &merged, &[SidecarFormat::Nfo]).await;
        }
    }

    Ok(())
}

/// Write episode sidecar (NFO) for a single episode's file.
/// Also writes tvshow.nfo in the show directory if it doesn't exist.
pub async fn write_episode_sidecar(pool: &PgPool, episode_id: Uuid) {
    if let Err(e) = write_episode_sidecar_inner(pool, episode_id).await {
        warn!(episode_id = %episode_id, error = %e, "Failed to write episode sidecar");
    }
}

async fn write_episode_sidecar_inner(
    pool: &PgPool,
    episode_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Look up episode → season → show → media_root
    let show_info: Option<(Uuid, Uuid)> = sqlx::query_as(
        r#"
        SELECT ts.show_id, COALESCE(tvs.media_root_id, '00000000-0000-0000-0000-000000000000')
        FROM tv_episodes te
        JOIN tv_seasons ts ON ts.id = te.season_id
        JOIN tv_shows tvs ON tvs.id = ts.show_id
        WHERE te.id = $1
        "#,
    )
    .bind(episode_id)
    .fetch_optional(pool)
    .await?;

    let Some((show_id, mr_id)) = show_info else {
        return Ok(());
    };
    if mr_id == Uuid::nil() {
        return Ok(());
    }

    let root = media_roots::get_by_id(pool, mr_id)
        .await?
        .ok_or("media root not found")?;
    if !root.write_sidecars {
        return Ok(());
    }

    let file_path_str = resolve_episode_file_path(pool, episode_id).await?;
    let Some(file_path_str) = file_path_str else {
        return Ok(());
    };

    let file_path = Path::new(&file_path_str);
    let dir = file_path.parent().ok_or("no parent dir")?;
    let stem = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("episode");

    // Write episode NFO
    if let Some(enriched) = build_episode_sidecar(pool, episode_id).await {
        let existing =
            crate::sidecar::read_sidecar(file_path, crate::sidecar::MediaType::TvEpisode).await;
        let merged = match existing {
            Some(ref ex) => merge_sidecar(ex, &enriched),
            None => enriched,
        };
        crate::sidecar::write_sidecar(dir, stem, &merged, &[SidecarFormat::Nfo]).await?;
    }

    // Also write tvshow.nfo if it doesn't exist. The show directory is the
    // show's, not this episode's parent — episodes of one show can sit under
    // two different directories.
    let show_dir = match resolve_show_dir(pool, show_id).await? {
        Some(d) => d,
        None => dir
            .parent()
            .ok_or("cannot determine show dir")?
            .to_path_buf(),
    };
    let show_dir = show_dir.as_path();
    if !show_dir.join("tvshow.nfo").exists()
        && let Some(enriched) = build_tvshow_sidecar(pool, show_id).await
    {
        let existing = crate::sidecar::read_tvshow_nfo(show_dir).await;
        let merged = match existing {
            Some(ref ex) => merge_sidecar(ex, &enriched),
            None => enriched,
        };
        let _ =
            crate::sidecar::write_sidecar(show_dir, "tvshow", &merged, &[SidecarFormat::Nfo]).await;

        // Clean up stale movie.nfo that was incorrectly written for TV shows
        cleanup_stale_movie_nfo(show_dir);
    }

    Ok(())
}

/// Delete a stale `movie.nfo` in a TV show directory if it contains `<tvshow>` XML.
///
/// Before the NFO filename fix, TV show sidecars were incorrectly written as `movie.nfo`.
/// This cleans them up organically as shows get processed.
fn cleanup_stale_movie_nfo(show_dir: &Path) {
    let stale = show_dir.join("movie.nfo");
    let is_stale_tvshow = stale
        .exists()
        .then(|| std::fs::read_to_string(&stale).ok())
        .flatten()
        .is_some_and(|c| c.contains("<tvshow>") || c.contains("<tvshow "));
    if is_stale_tvshow && let Err(e) = std::fs::remove_file(&stale) {
        warn!(path = %stale.display(), error = %e, "Failed to remove stale movie.nfo");
    }
}

/// Write book sidecars (metadata.json merged + metadata.opf) for all added files.
pub async fn write_book_sidecars(pool: &PgPool, book_id: Uuid) {
    if let Err(e) = write_book_sidecars_inner(pool, book_id).await {
        warn!(book_id = %book_id, error = %e, "Failed to write book sidecars");
    }
}

async fn write_book_sidecars_inner(
    pool: &PgPool,
    book_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Check media root write_sidecars flag
    let media_root_id: Option<Uuid> =
        sqlx::query_scalar("SELECT media_root_id FROM books WHERE id = $1")
            .bind(book_id)
            .fetch_optional(pool)
            .await?
            .flatten();

    let Some(mr_id) = media_root_id else {
        return Ok(());
    };
    let root = media_roots::get_by_id(pool, mr_id)
        .await?
        .ok_or("media root not found")?;
    if !root.write_sidecars {
        return Ok(());
    }

    let file_path_str = resolve_book_file_path(pool, book_id).await?;
    let Some(file_path_str) = file_path_str else {
        return Ok(());
    };

    let enriched = match build_book_sidecar(pool, book_id).await {
        Some(m) => m,
        None => return Ok(()),
    };

    let file_path = Path::new(&file_path_str);
    let dir = file_path.parent().ok_or("no parent dir")?;
    let stem = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("book");

    // metadata.json: merge-based writing
    let json_path = dir.join("metadata.json");
    crate::sidecar::book_json::write_metadata_json_merged(&json_path, &enriched).await?;

    // metadata.opf: full regeneration
    crate::sidecar::write_sidecar(dir, stem, &enriched, &[SidecarFormat::CalibreOpf]).await?;

    Ok(())
}

// =============================================================================
// Per-entity image write functions (best-effort, never fail callers)
// =============================================================================

/// Write movie images (poster, backdrop, logo) next to the movie file.
pub async fn write_movie_images(pool: &PgPool, storage_root: &str, movie_id: Uuid) {
    if let Err(e) = write_movie_images_inner(pool, storage_root, movie_id).await {
        warn!(movie_id = %movie_id, error = %e, "Failed to write movie images");
    }
}

async fn write_movie_images_inner(
    pool: &PgPool,
    storage_root: &str,
    movie_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (root, file_path_str) = resolve_entity_root_and_path(pool, "movies", movie_id).await?;
    let Some(root) = root else { return Ok(()) };
    if !root.write_images {
        return Ok(());
    }
    let Some(file_path_str) = file_path_str else {
        return Ok(());
    };

    let dir = Path::new(&file_path_str).parent().ok_or("no parent dir")?;
    let images = load_entity_images(pool, "movies", movie_id).await;

    // Download all images concurrently
    tokio::join!(
        write_entity_image(
            storage_root,
            pool,
            &images,
            "poster",
            crate::images::ImageType::MoviePoster,
            movie_id,
            dir,
            "movie"
        ),
        write_entity_image(
            storage_root,
            pool,
            &images,
            "backdrop",
            crate::images::ImageType::MovieBackdrop,
            movie_id,
            dir,
            "movie"
        ),
        write_entity_image(
            storage_root,
            pool,
            &images,
            "logo",
            crate::images::ImageType::MovieLogo,
            movie_id,
            dir,
            "movie"
        ),
    );

    Ok(())
}

/// Write TV show images (poster, backdrop, logo) next to the show directory.
pub async fn write_tvshow_images(pool: &PgPool, storage_root: &str, show_id: Uuid) {
    if let Err(e) = write_tvshow_images_inner(pool, storage_root, show_id).await {
        warn!(show_id = %show_id, error = %e, "Failed to write TV show images");
    }
}

async fn write_tvshow_images_inner(
    pool: &PgPool,
    storage_root: &str,
    show_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (root, _) = resolve_entity_root_and_path(pool, "tv_shows", show_id).await?;
    let Some(root) = root else { return Ok(()) };
    if !root.write_images {
        return Ok(());
    }

    // Same show directory the sidecars use, or the poster lands in a sibling
    // directory the media player never reads.
    let Some(show_dir) = resolve_show_dir(pool, show_id).await? else {
        return Ok(());
    };
    let show_dir = show_dir.as_path();

    let images = load_entity_images(pool, "tv_shows", show_id).await;

    // Download all images concurrently
    tokio::join!(
        write_entity_image(
            storage_root,
            pool,
            &images,
            "poster",
            crate::images::ImageType::TvShowPoster,
            show_id,
            show_dir,
            "tv"
        ),
        write_entity_image(
            storage_root,
            pool,
            &images,
            "backdrop",
            crate::images::ImageType::TvShowBackdrop,
            show_id,
            show_dir,
            "tv"
        ),
        write_entity_image(
            storage_root,
            pool,
            &images,
            "logo",
            crate::images::ImageType::TvShowLogo,
            show_id,
            show_dir,
            "tv"
        ),
    );

    Ok(())
}

/// Write book images (cover) next to the book file.
pub async fn write_book_images(pool: &PgPool, storage_root: &str, book_id: Uuid) {
    if let Err(e) = write_book_images_inner(pool, storage_root, book_id).await {
        warn!(book_id = %book_id, error = %e, "Failed to write book images");
    }
}

async fn write_book_images_inner(
    pool: &PgPool,
    storage_root: &str,
    book_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (root, file_path_str) = resolve_entity_root_and_path(pool, "books", book_id).await?;
    let Some(root) = root else { return Ok(()) };
    if !root.write_images {
        return Ok(());
    }
    let Some(file_path_str) = file_path_str else {
        return Ok(());
    };

    let dir = Path::new(&file_path_str).parent().ok_or("no parent dir")?;
    let images = load_entity_images(pool, "books", book_id).await;

    // Books use ImageType::Series for cover caching (matches cover endpoint)
    write_entity_image(
        storage_root,
        pool,
        &images,
        "cover",
        crate::images::ImageType::Series,
        book_id,
        dir,
        "book",
    )
    .await;

    Ok(())
}

/// Helper: look up the media root for an entity and resolve a file path.
async fn resolve_entity_root_and_path(
    pool: &PgPool,
    table: &str,
    entity_id: Uuid,
) -> Result<
    (Option<media_roots::MediaRootRow>, Option<String>),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let sql = format!("SELECT media_root_id FROM {table} WHERE id = $1");
    let mr_id: Option<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(entity_id)
        .fetch_optional(pool)
        .await?
        .flatten();

    let Some(mr_id) = mr_id else {
        return Ok((None, None));
    };
    let root = media_roots::get_by_id(pool, mr_id)
        .await?
        .ok_or("media root not found")?;

    let file_path = match table {
        "movies" => resolve_movie_file_path(pool, entity_id).await?,
        "books" => resolve_book_file_path(pool, entity_id).await?,
        _ => None,
    };

    Ok((Some(root), file_path))
}

/// Load images JSONB from an entity table.
async fn load_entity_images(
    pool: &PgPool,
    table: &str,
    entity_id: Uuid,
) -> crate::images::ImageMap {
    let sql =
        format!("SELECT COALESCE(images, '{{}}'::jsonb) AS images FROM {table} WHERE id = $1");
    let row: Option<(serde_json::Value,)> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(entity_id)
        .fetch_optional(pool)
        .await
        .unwrap_or(None);

    match row {
        Some((val,)) => serde_json::from_value(val).unwrap_or_default(),
        None => crate::images::ImageMap::new(),
    }
}

/// Cache (if needed) and write a single image sidecar file.
#[allow(clippy::too_many_arguments)]
async fn write_entity_image(
    storage_root: &str,
    pool: &PgPool,
    images: &crate::images::ImageMap,
    key: &str,
    img_type: crate::images::ImageType,
    entity_id: Uuid,
    target_dir: &Path,
    media_type: &str,
) {
    // Ensure the image is cached locally
    if !crate::images::cached_image_exists(storage_root, img_type, entity_id) {
        if let Some(url) = crate::images::source_url(images, key) {
            crate::images::try_cache_image(storage_root, img_type, entity_id, Some(&url)).await;
            // Mark cached_at in DB
            let _ = crate::images::mark_cached(
                pool,
                match media_type {
                    "movie" => "movies",
                    "tv" => "tv_shows",
                    "book" => "books",
                    _ => return,
                },
                entity_id,
                key,
            )
            .await;
        } else {
            return;
        }
    }

    if let Err(e) = crate::sidecar::images::write_image_sidecar(
        storage_root,
        img_type,
        entity_id,
        target_dir,
        key,
        media_type,
    )
    .await
    {
        warn!(
            entity_id = %entity_id,
            key,
            error = %e,
            "Failed to write image sidecar"
        );
    }
}

// =============================================================================
// File path resolution helpers (media_root_files → entity file table fallback)
// =============================================================================

/// Lowest added path among an entity's media root files.
///
/// `get_by_entity` is unordered, so picking the first `added` row would put an
/// entity's sidecars next to a different file on every run once it has two.
fn lowest_added_path(files: &[media_root_files::MediaRootFileRow]) -> Option<String> {
    files
        .iter()
        .filter(|f| f.match_status == "added")
        .map(|f| &f.file_path)
        .min()
        .cloned()
}

/// Resolve a file path for a movie: try media_root_files first, fall back to movie_files.
async fn resolve_movie_file_path(
    pool: &PgPool,
    movie_id: Uuid,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let files = media_root_files::get_by_entity(pool, "movie", movie_id).await?;
    if let Some(path) = lowest_added_path(&files) {
        return Ok(Some(path));
    }
    // Fallback: first available movie_files entry
    let mf = movie_files::get_preferred_available(pool, movie_id).await?;
    Ok(mf.map(|f| f.file_path))
}

/// Resolve a file path for a TV episode: try media_root_files first, fall back to tv_episode_files.
async fn resolve_episode_file_path(
    pool: &PgPool,
    episode_id: Uuid,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let files = media_root_files::get_by_entity(pool, "tv_episode", episode_id).await?;
    if let Some(path) = lowest_added_path(&files) {
        return Ok(Some(path));
    }
    let ef = tv_episode_files::get_preferred_available(pool, episode_id).await?;
    Ok(ef.map(|f| f.file_path))
}

/// Resolve a file path for a book: try media_root_files first, fall back to book_files.
async fn resolve_book_file_path(
    pool: &PgPool,
    book_id: Uuid,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let files = media_root_files::get_by_entity(pool, "book", book_id).await?;
    if let Some(path) = lowest_added_path(&files) {
        return Ok(Some(path));
    }
    let path = book_files::get_any_available_path(pool, book_id).await?;
    Ok(path)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::{CastEntry, SidecarFormat};

    #[test]
    fn test_merge_sidecar_enriched_wins() {
        let existing = SidecarMetadata {
            title: Some("Old Title".to_string()),
            year: Some(2020),
            genres: vec!["Drama".to_string()],
            identifiers: SidecarIdentifiers {
                tmdb_id: Some("111".to_string()),
                extra_uniqueids: vec![("sonarr".to_string(), "42".to_string())],
                ..Default::default()
            },
            ..Default::default()
        };

        let enriched = SidecarMetadata {
            title: Some("New Title".to_string()),
            year: Some(2024),
            plot: Some("A great plot".to_string()),
            genres: vec!["Action".to_string(), "Comedy".to_string()],
            identifiers: SidecarIdentifiers {
                tmdb_id: Some("222".to_string()),
                imdb_id: Some("tt1234567".to_string()),
                kobbler_id: Some("uuid-123".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let merged = merge_sidecar(&existing, &enriched);

        assert_eq!(merged.title.as_deref(), Some("New Title"));
        assert_eq!(merged.year, Some(2024));
        assert_eq!(merged.plot.as_deref(), Some("A great plot"));
        assert_eq!(merged.genres, vec!["Action", "Comedy"]);
        assert_eq!(merged.identifiers.tmdb_id.as_deref(), Some("222"));
        assert_eq!(merged.identifiers.imdb_id.as_deref(), Some("tt1234567"));
        assert_eq!(merged.identifiers.kobbler_id.as_deref(), Some("uuid-123"));
        // extra_uniqueids preserved from existing
        assert_eq!(merged.identifiers.extra_uniqueids.len(), 1);
        assert_eq!(merged.identifiers.extra_uniqueids[0].0, "sonarr");
    }

    #[test]
    fn test_merge_sidecar_keeps_existing_when_enriched_empty() {
        let existing = SidecarMetadata {
            title: Some("Existing Title".to_string()),
            year: Some(2020),
            genres: vec!["Drama".to_string()],
            video: Some(VideoSidecar {
                mpaa: Some("PG-13".to_string()),
                cast: vec![CastEntry {
                    name: "Actor".to_string(),
                    role: Some("Hero".to_string()),
                    thumb: None,
                }],
                ..Default::default()
            }),
            ..Default::default()
        };

        let enriched = SidecarMetadata::default();

        let merged = merge_sidecar(&existing, &enriched);

        assert_eq!(merged.title.as_deref(), Some("Existing Title"));
        assert_eq!(merged.year, Some(2020));
        assert_eq!(merged.genres, vec!["Drama"]);
        let video = merged.video.expect("Should have video");
        assert_eq!(video.mpaa.as_deref(), Some("PG-13"));
        assert_eq!(video.cast.len(), 1);
    }

    #[test]
    fn test_merge_sidecar_book_fields() {
        let existing = SidecarMetadata {
            book: Some(BookSidecar {
                authors: vec!["Old Author".to_string()],
                publisher: Some("Old Publisher".to_string()),
                language: Some("en".to_string()),
                series: Some("Old Series".to_string()),
                series_index: Some(1.0),
                description: Some("Old desc".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let enriched = SidecarMetadata {
            title: Some("New Book".to_string()),
            book: Some(BookSidecar {
                authors: vec!["New Author".to_string()],
                description: Some("New desc".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let merged = merge_sidecar(&existing, &enriched);
        let book = merged.book.expect("Should have book");

        assert_eq!(book.authors, vec!["New Author"]);
        assert_eq!(book.description.as_deref(), Some("New desc"));
        // Kept from existing since enriched was None
        assert_eq!(book.publisher.as_deref(), Some("Old Publisher"));
        assert_eq!(book.language.as_deref(), Some("en"));
        assert_eq!(book.series.as_deref(), Some("Old Series"));
        assert_eq!(book.series_index, Some(1.0));
    }

    #[test]
    fn test_merge_source_format() {
        let existing = SidecarMetadata {
            source_format: Some(SidecarFormat::Nfo),
            ..Default::default()
        };
        let enriched = SidecarMetadata::default();

        let merged = merge_sidecar(&existing, &enriched);
        assert_eq!(merged.source_format, Some(SidecarFormat::Nfo));
    }
}
