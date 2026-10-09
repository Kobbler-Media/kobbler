//! Media root interpret job handler.
//!
//! Loads pending files for a media root and identifies them:
//! - TV/movies: Uses the MediaInterpreter trait (Kobload HTTP API or local regex)
//! - Books: ebook-meta for embedded metadata, Kobload/LocalInterpreter for filename parsing

use crate::RuntimeConfigManager;
use crate::convert::EbookMeta;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobContext, JobResult};
use crate::media_interpreter::{InterpretItem, MediaInterpreter};
use crate::repositories::{media_root_files, media_roots};
use crate::sidecar::SidecarMetadata;
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::path::Path;
use tokio_util::sync::CancellationToken;
use tracing::{info, trace, warn};
use uuid::Uuid;

const BATCH_SIZE: i64 = 500;

/// Payload for media_root_interpret jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRootInterpretPayload {
    pub media_root_id: Uuid,
}

pub struct MediaRootInterpretHandler {
    pool: PgPool,
    runtime_config: RuntimeConfigManager,
    nats: Publisher,
}

impl MediaRootInterpretHandler {
    pub fn new(pool: PgPool, runtime_config: RuntimeConfigManager, nats: Publisher) -> Self {
        Self {
            pool,
            runtime_config,
            nats,
        }
    }

    /// Get the appropriate interpreter based on config.
    async fn get_interpreter(&self) -> Box<dyn MediaInterpreter> {
        crate::media_interpreter::get_interpreter(&self.runtime_config).await
    }

    /// Interpret TV/movie files using the MediaInterpreter trait.
    async fn interpret_video_files(
        &self,
        root: &media_roots::MediaRootRow,
        pending: &[media_root_files::MediaRootFileRow],
    ) -> (u64, u64) {
        let interpreter = self.get_interpreter().await;

        let media_kind = match root.media_type.as_str() {
            "tv" => "tv",
            "movie" => "movies",
            "sport" => "sports",
            _ => "misc",
        };

        let root_path = Path::new(&root.path);
        let items: Vec<InterpretItem> = pending
            .iter()
            .map(|f| {
                let relative = Path::new(&f.file_path)
                    .strip_prefix(root_path)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| f.file_name.clone());
                InterpretItem {
                    relative_path: relative,
                    file_size: f.file_size,
                    media_kind: media_kind.to_string(),
                }
            })
            .collect();

        let results = match interpreter.interpret_batch(items).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "Video interpretation batch failed");
                return (0, pending.len() as u64);
            }
        };

        let mut interpreted = 0u64;
        let mut failed = 0u64;

        for (file, result) in pending.iter().zip(results.iter()) {
            if result.success {
                let mut raw = serde_json::to_value(result)
                    .unwrap_or_else(|_| json!({"error": "serialization failed"}));

                // Merge sidecar metadata (provider IDs, title) into interpretation
                merge_sidecar_into_interpretation(&mut raw, &file.sidecar_metadata);

                if let Err(e) = media_root_files::set_interpreted(&self.pool, file.id, &raw).await {
                    warn!(file_id = %file.id, error = %e, "Failed to save interpretation");
                    failed += 1;
                } else {
                    interpreted += 1;
                }
            } else {
                if let Err(e) = media_root_files::set_unmatched(&self.pool, file.id).await {
                    warn!(file_id = %file.id, error = %e, "Failed to mark as unmatched");
                }
                failed += 1;
            }
        }

        (interpreted, failed)
    }

    /// Interpret audiobook files using filename parsing + audio probing.
    ///
    /// Priority chain for each field (highest wins):
    /// 1. Sidecar metadata (metadata.json / metadata.opf)
    /// 2. Embedded audio tags (ffprobe: title, artist/narrator, abridged)
    /// 3. Filename parsing (Kobload or LocalInterpreter)
    async fn interpret_audiobook_files(
        &self,
        root: &media_roots::MediaRootRow,
        pending: &[media_root_files::MediaRootFileRow],
    ) -> (u64, u64) {
        let interpreter = self.get_interpreter().await;

        let root_path = Path::new(&root.path);
        let items: Vec<InterpretItem> = pending
            .iter()
            .map(|f| {
                let relative = Path::new(&f.file_path)
                    .strip_prefix(root_path)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| f.file_name.clone());
                InterpretItem {
                    relative_path: relative,
                    file_size: f.file_size,
                    media_kind: "audiobooks".to_string(),
                }
            })
            .collect();

        // Run filename interpretation batch (fast)
        let interpreter_results = match interpreter.interpret_batch(items).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "Audiobook interpretation batch failed");
                vec![]
            }
        };

        let mut interpreted = 0u64;
        let mut failed = 0u64;

        for (i, file) in pending.iter().enumerate() {
            let interp = interpreter_results.get(i);

            // Kobload's interpret handles generic filename detection, directory
            // Author-Title splitting, noise stripping, and folder context
            // resolution — just use its result directly.
            let mut obj = if let Some(ir) = interp
                && ir.success
                && ir.book_title.is_some()
            {
                let mut o = json!({
                    "media_type": "audiobook",
                    "source": "interpret",
                    "title": ir.book_title,
                });
                if !ir.book_authors.is_empty() {
                    let cleaned: Vec<String> = ir
                        .book_authors
                        .iter()
                        .map(|a| clean_author_value(a))
                        .filter(|a| !a.is_empty() && !looks_like_non_author(a))
                        .collect();
                    if !cleaned.is_empty() {
                        o["authors"] = json!(cleaned);
                    }
                }
                // Propagate interpret-derived fields as fallbacks.
                // Probe data (merged below) will overwrite if available.
                if let Some(raw) = &ir.raw
                    && let nats_jobs::ParsedMedia::Audiobook(ab) = &raw.media
                {
                    if ab.is_generic_filename {
                        o["is_generic_filename"] = json!(true);
                    }
                    if let Some(narrator) = &ab.narrator {
                        o["narrator"] = json!(narrator);
                    }
                    if let Some(series) = &ab.series {
                        o["series"] = json!(series);
                    }
                    if let Some(pos) = &ab.series_position {
                        o["series_index"] = json!(pos);
                    }
                    if let Some(year) = ab.year {
                        o["year"] = json!(year);
                    }
                }
                Some(o)
            } else {
                None
            };

            // Probe audio file for embedded metadata
            match crate::audio::probe::probe_audio(Path::new(&file.file_path)).await {
                Ok(probe) => {
                    // For audiobooks, prefer album tag as the book title (title tag
                    // is usually the chapter/track name in multi-file audiobooks)
                    let book_title_from_probe =
                        probe.album.as_ref().filter(|a| !a.is_empty()).or_else(|| {
                            probe
                                .title
                                .as_ref()
                                .filter(|t| !t.is_empty() && !is_chapter_title(t))
                        });

                    if obj.is_none() && book_title_from_probe.is_some() {
                        obj = Some(json!({
                            "media_type": "audiobook",
                            "source": "probe",
                            "title": book_title_from_probe,
                        }));
                    } else if let Some(ref mut o) = obj {
                        // Cross-check probe album against interpretation title
                        if let Some(bt) = book_title_from_probe
                            && probe.album.as_ref().is_some_and(|a| !a.is_empty())
                        {
                            let interp_title =
                                o.get("title").and_then(|v| v.as_str()).unwrap_or("");
                            let current_source = o
                                .get("source")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if titles_agree(interp_title, bt) {
                                // Agree: use probe (often richer, includes subtitles)
                                o["title"] = json!(bt);
                                o["source"] = json!(format!("{}+probe", current_source));
                            } else if is_chapter_title(interp_title) {
                                // Interpretation title is a chapter/track name (e.g. "01",
                                // "Chapter 3") — the probe album tag is clearly the book title
                                o["title"] = json!(bt);
                                o["source"] = json!(format!("{}+probe_override", current_source));
                            } else {
                                // Real title disagreement — keep interpretation, store
                                // probe for reference so matching can use it
                                o["probe_title"] = json!(bt);
                                o["is_title_conflict"] = json!(true);
                                o["source"] = json!(format!("{}+probe_conflict", current_source));
                            }
                        }
                    }

                    if let Some(ref mut o) = obj {
                        let has_authors = o
                            .get("authors")
                            .and_then(|v| v.as_array())
                            .is_some_and(|a| !a.is_empty());

                        // Author merge: probe.author (from author/album_artist tags)
                        // is trustworthy. When probe has no explicit author but has
                        // a "narrator" (actually from the artist tag), the narrator
                        // might be the real author — use interpretation author if available.
                        if let Some(author) = &probe.author {
                            // Probe has explicit author tag — clean and trust it
                            let cleaned = clean_author_value(author);
                            if !cleaned.is_empty() && !looks_like_non_author(&cleaned) {
                                o["authors"] = json!([cleaned]);
                            }
                        } else if !has_authors && let Some(narrator) = &probe.narrator {
                            // No probe author, no interpretation author, but probe
                            // has "narrator" (likely from artist tag = real author).
                            // Use it as author rather than narrator.
                            if !looks_like_non_author(narrator) {
                                o["authors"] = json!([clean_narrator_value(narrator)]);
                            }
                        }

                        // Narrator: only trust when probe also has a separate author.
                        // When probe has no author tag, the "narrator" is likely
                        // the actual author misidentified via the artist tag.
                        if let Some(narrator) = &probe.narrator
                            && probe.author.is_some()
                        {
                            let cleaned = clean_narrator_value(narrator);
                            if !cleaned.is_empty() {
                                o["narrator"] = json!(cleaned);
                            }
                        }

                        // Metadata that always layers on top
                        if let Some(abridged) = probe.is_abridged {
                            o["is_abridged"] = json!(abridged);
                        }
                        if let Some(dur) = probe.duration_seconds {
                            o["duration_seconds"] = json!(dur);
                        }
                        if let Some(asin) = &probe.asin {
                            o["asin"] = json!(asin);
                        }
                        if let Some(isbn) = &probe.isbn {
                            o["isbn"] = json!(isbn);
                        }
                        if let Some(series) = &probe.series
                            && is_valid_series_name(series)
                        {
                            let (clean, idx) = clean_series_name(series, probe.series_index);
                            o["series"] = json!(clean);
                            if let Some(idx) = idx {
                                o["series_index"] = json!(idx);
                            }
                        } else if let Some(idx) = probe.series_index {
                            o["series_index"] = json!(idx);
                        }
                        // Try extracting series from probe title if not set.
                        // Skip ordinal-patterned titles like "Book The 1st-" which
                        // are track/chapter names, not series info.
                        if o.get("series").is_none()
                            && let Some(title) = probe.title.as_deref()
                            && !is_ordinal_chapter_title(title)
                            && let Some((series, pos)) = extract_series_from_title(title)
                        {
                            o["series"] = json!(series);
                            o["series_index"] = json!(pos);
                        }
                        if let Some(publisher) = &probe.publisher {
                            o["publisher"] = json!(publisher);
                        }
                        if let Some(language) = &probe.language {
                            o["language"] = json!(language);
                        }
                        if let Some(description) = &probe.description {
                            o["description"] = json!(description);
                        }
                        if let Some(year) = &probe.year {
                            o["year"] = json!(year);
                        }
                        if let Some(genre) = &probe.genre {
                            o["genre"] = json!(genre);
                        }
                    }
                }
                Err(e) => {
                    trace!(file = %file.file_name, error = %e, "Audio probe failed, using filename only");
                }
            }

            // Merge sidecar metadata (highest priority — overwrites filename/probe)
            if let Some(ref mut o) = obj {
                merge_sidecar_into_audiobook(o, &file.sidecar_metadata);
            }

            // Check if we got at least a title
            let has_title = obj
                .as_ref()
                .and_then(|o| o.get("title"))
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty());

            if has_title {
                if let Err(e) =
                    media_root_files::set_interpreted(&self.pool, file.id, obj.as_ref().unwrap())
                        .await
                {
                    warn!(file_id = %file.id, error = %e, "Failed to save audiobook interpretation");
                    failed += 1;
                } else {
                    interpreted += 1;
                }
            } else {
                if let Err(e) = media_root_files::set_unmatched(&self.pool, file.id).await {
                    warn!(file_id = %file.id, error = %e, "Failed to mark as unmatched");
                }
                failed += 1;
            }
        }

        (interpreted, failed)
    }

    /// Interpret book files.
    ///
    /// Priority:
    /// 1. Sidecar metadata (NFO, metadata.json)
    /// 2. Embedded ebook metadata via ebook-meta (primary: ISBN, series, description)
    /// 3. Kobload/LocalInterpreter for filename parsing (title, authors)
    ///
    /// ebook-meta provides the base interpretation; the interpreter's title/authors
    /// are used as fallback when ebook-meta lacks them.
    async fn interpret_book_files(
        &self,
        root: &media_roots::MediaRootRow,
        pending: &[media_root_files::MediaRootFileRow],
    ) -> (u64, u64) {
        let ebook_meta = EbookMeta::new();
        // Native pipeline always available. We only need Calibre for the
        // long-tail formats (PDF/FB2/RTF/TXT/HTML/DOCX/LIT/PDB).
        let calibre_available = ebook_meta.is_available().await;
        let interpreter = self.get_interpreter().await;

        if !calibre_available {
            info!(
                "ebook-meta not available; native pipeline only — long-tail formats will fall through to filename interpreter"
            );
        }

        // Run interpreter batch for filename parsing (used as fallback for title/authors)
        let root_path = Path::new(&root.path);
        let items: Vec<InterpretItem> = pending
            .iter()
            .map(|f| {
                let relative = Path::new(&f.file_path)
                    .strip_prefix(root_path)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| f.file_name.clone());
                InterpretItem {
                    relative_path: relative,
                    file_size: f.file_size,
                    media_kind: "books".to_string(),
                }
            })
            .collect();

        let interpreter_results = match interpreter.interpret_batch(items).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "Book filename interpretation batch failed");
                vec![]
            }
        };

        let mut interpreted = 0u64;
        let mut failed = 0u64;

        for (i, file) in pending.iter().enumerate() {
            // Try sidecar metadata first (highest confidence)
            let sidecar_interp = try_sidecar_book_interpretation(&file.sidecar_metadata);
            if let Some(interp) = sidecar_interp {
                if let Err(e) =
                    media_root_files::set_interpreted(&self.pool, file.id, &interp).await
                {
                    warn!(file_id = %file.id, error = %e, "Failed to save sidecar book interpretation");
                    failed += 1;
                } else {
                    trace!(file_id = %file.id, "Used sidecar metadata for book interpretation");
                    interpreted += 1;
                }
                continue;
            }

            // Start with embedded metadata (boko native or Calibre fallback).
            let file_path_obj = Path::new(&file.file_path);
            let can_read_meta = kob_ebook::native_supports(file_path_obj) || calibre_available;
            let mut obj = if can_read_meta {
                match ebook_meta.extract(file_path_obj).await {
                    Ok(meta) if meta.title.is_some() => {
                        let title = meta.title.as_deref().unwrap_or("");
                        // Reject garbage titles from embedded metadata
                        if is_garbage_title(title) {
                            trace!(file = %file.file_name, title = %title, "Rejected garbage embedded title");
                            None
                        } else {
                            let mut o = json!({
                                "media_type": "book",
                                "source": "embedded",
                            });
                            if let Some(t) = &meta.title {
                                o["title"] = json!(t);
                            }
                            // Filter out garbage authors from embedded metadata
                            let good_authors: Vec<&str> = meta
                                .authors
                                .iter()
                                .map(|a| a.as_str())
                                .filter(|a| !is_garbage_author(a))
                                .collect();
                            if !good_authors.is_empty() {
                                o["authors"] = json!(good_authors);
                            }
                            if let Some(isbn) = &meta.isbn {
                                o["isbn"] = json!(isbn);
                            }
                            if let Some(series) = &meta.series {
                                let (clean, idx) = clean_series_name(series, meta.series_index);
                                o["series"] = json!(clean);
                                if let Some(idx) = idx {
                                    o["series_index"] = json!(idx);
                                }
                            } else if let Some(idx) = meta.series_index {
                                o["series_index"] = json!(idx);
                            }
                            if let Some(desc) = &meta.description {
                                o["description"] = json!(desc);
                            }
                            if let Some(pub_) = &meta.publisher {
                                o["publisher"] = json!(pub_);
                            }
                            if let Some(lang) = &meta.language {
                                o["language"] = json!(lang);
                            }
                            Some(o)
                        }
                    }
                    Ok(_) => {
                        trace!(file = %file.file_name, "No title in embedded metadata");
                        None
                    }
                    Err(e) => {
                        trace!(file = %file.file_name, error = %e, "Metadata extraction failed");
                        None
                    }
                }
            } else {
                None
            };

            // Get interpreter result for this file (fallback title/authors from filename)
            let interp_result = interpreter_results.get(i);

            if let Some(obj) = &mut obj {
                // ebook-meta succeeded — supplement with interpreter's title/authors if missing,
                // and cross-check titles when both exist
                if let Some(ir) = interp_result
                    && ir.success
                {
                    let embedded_title = obj.get("title").and_then(|v| v.as_str()).unwrap_or("");
                    if embedded_title.is_empty() {
                        if let Some(t) = &ir.book_title {
                            obj["title"] = json!(t);
                            obj["source"] = json!("embedded+filename");
                        }
                    } else if let Some(interp_title) = &ir.book_title
                        && !interp_title.is_empty()
                        && !titles_agree(embedded_title, interp_title)
                    {
                        // Embedded and filename titles disagree — keep embedded
                        // (usually more reliable for ebooks) but flag the conflict
                        obj["interpret_title"] = json!(interp_title);
                        obj["is_title_conflict"] = json!(true);
                    }
                    if obj
                        .get("authors")
                        .and_then(|v| v.as_array())
                        .is_none_or(|a| a.is_empty())
                        && !ir.book_authors.is_empty()
                    {
                        obj["authors"] = json!(ir.book_authors);
                    }
                }
            } else if let Some(ir) = interp_result
                && ir.success
                && ir.book_title.is_some()
            {
                // No ebook-meta — use interpreter result as the interpretation
                let mut o = json!({
                    "media_type": "book",
                    "source": "filename",
                    "title": ir.book_title,
                });
                if !ir.book_authors.is_empty() {
                    o["authors"] = json!(ir.book_authors);
                }
                obj = Some(o);
            }

            // Fallback: try to extract author from parent directory structure
            // e.g. "Dav Pilkey/Dog Man Collection/file.cbz" → author="Dav Pilkey"
            if let Some(obj) = &mut obj {
                // If all authors are substrings of the title, they're likely
                // parsed from the filename, not real metadata (common in CBZ/CBR).
                // Clear them so the directory fallback can fire.
                let title = obj.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let authors: Vec<&str> = obj
                    .get("authors")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();
                if !authors.is_empty() && all_authors_title_derived(title, &authors) {
                    trace!(
                        file = %file.file_name,
                        "Cleared title-derived authors: {:?}", authors
                    );
                    obj.as_object_mut().unwrap().remove("authors");
                }

                let has_authors = obj
                    .get("authors")
                    .and_then(|v| v.as_array())
                    .is_some_and(|a| !a.is_empty());
                if !has_authors
                    && let Some(ir) = interp_result
                    && ir.success
                    && !ir.book_authors.is_empty()
                {
                    obj["authors"] = json!(ir.book_authors);
                    trace!(file = %file.file_name, "Used interpreter author from folder context");
                }
            }

            // Check if we got at least a title
            let has_title = obj
                .as_ref()
                .and_then(|o| o.get("title"))
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty());

            if has_title {
                if let Err(e) =
                    media_root_files::set_interpreted(&self.pool, file.id, obj.as_ref().unwrap())
                        .await
                {
                    warn!(file_id = %file.id, error = %e, "Failed to save book interpretation");
                    failed += 1;
                } else {
                    interpreted += 1;
                }
            } else {
                if let Err(e) = media_root_files::set_unmatched(&self.pool, file.id).await {
                    warn!(file_id = %file.id, error = %e, "Failed to mark as unmatched");
                }
                failed += 1;
            }
        }

        (interpreted, failed)
    }
}

/// Merge sidecar metadata (provider IDs, titles) into a video interpretation result.
///
/// Sidecar data is higher confidence than filename parsing, so it overrides
/// missing fields but doesn't replace fields the interpreter already found
/// (e.g., quality/codec info from the filename).
fn merge_sidecar_into_interpretation(interp: &mut Value, sidecar_json: &Option<Value>) {
    let Some(sidecar_json) = sidecar_json else {
        return;
    };
    let Ok(meta) = serde_json::from_value::<SidecarMetadata>(sidecar_json.clone()) else {
        return;
    };

    let obj = match interp.as_object_mut() {
        Some(o) => o,
        None => return,
    };

    // Store provider IDs for the match phase
    let ids = &meta.identifiers;
    if ids.has_any() {
        let mut sidecar_ids = serde_json::Map::new();
        if let Some(v) = &ids.tmdb_id {
            sidecar_ids.insert("tmdb_id".into(), json!(v));
        }
        if let Some(v) = &ids.imdb_id {
            sidecar_ids.insert("imdb_id".into(), json!(v));
        }
        if let Some(v) = &ids.tvdb_id {
            sidecar_ids.insert("tvdb_id".into(), json!(v));
        }
        if let Some(v) = &ids.tvmaze_id {
            sidecar_ids.insert("tvmaze_id".into(), json!(v));
        }
        obj.insert("sidecar_ids".into(), Value::Object(sidecar_ids));
    }

    // Use sidecar title if interpreter didn't find one
    if let Some(title) = &meta.title
        && !obj.contains_key("movie_title")
        && !obj.contains_key("show_name")
    {
        // Determine from context whether this is a movie or show
        if obj.get("season").is_some() || obj.get("episodes").is_some() {
            obj.entry("show_name").or_insert(json!(title));
        } else {
            obj.entry("movie_title").or_insert(json!(title));
        }
    }

    if let Some(year) = meta.year {
        obj.entry("movie_year").or_insert(json!(year));
    }
}

/// Merge sidecar metadata into an audiobook interpretation.
///
/// Unlike `merge_sidecar_into_interpretation` (video-focused), this handles
/// book-specific fields: title, authors, ISBN, ASIN, series, narrators, etc.
/// Sidecar data overwrites lower-priority sources (filename, probe).
fn merge_sidecar_into_audiobook(interp: &mut Value, sidecar_json: &Option<Value>) {
    let Some(sidecar_json) = sidecar_json else {
        return;
    };
    let Ok(meta) = serde_json::from_value::<SidecarMetadata>(sidecar_json.clone()) else {
        return;
    };

    let obj = match interp.as_object_mut() {
        Some(o) => o,
        None => return,
    };

    // Sidecar title overrides filename/probe/directory-derived title
    let has_series_from_sidecar = meta.book.as_ref().and_then(|b| b.series.as_ref()).is_some();
    if let Some(title) = &meta.title
        && !title.is_empty()
        && !is_garbage_title(title)
    {
        let (clean_title, extracted_series, extracted_index) = strip_series_from_title(title);
        obj.insert("title".into(), json!(clean_title));

        // Use extracted series from title if sidecar doesn't have one
        if !has_series_from_sidecar {
            if let Some(series) = extracted_series {
                obj.insert("series".into(), json!(series));
            }
            if let Some(idx) = extracted_index {
                obj.entry("series_index").or_insert(json!(idx));
            }
        }

        // Track that sidecar contributed
        let prev_source = obj
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !prev_source.is_empty() {
            obj.insert("source".into(), json!(format!("{prev_source}+sidecar")));
        } else {
            obj.insert("source".into(), json!("sidecar"));
        }
    }

    if let Some(book) = &meta.book {
        let good_authors: Vec<String> = book
            .authors
            .iter()
            .filter(|a| !is_garbage_author(a))
            .map(|a| normalize_author_name(a))
            .collect();
        if !good_authors.is_empty() {
            obj.insert("authors".into(), json!(good_authors));
        }
        if let Some(series) = &book.series {
            let (clean, idx) = clean_series_name(series, book.series_index);
            obj.insert("series".into(), json!(clean));
            if let Some(idx) = idx {
                obj.insert("series_index".into(), json!(idx));
            }
        } else if let Some(idx) = book.series_index {
            obj.insert("series_index".into(), json!(idx));
        }
        if let Some(desc) = &book.description {
            obj.insert("description".into(), json!(desc));
        }
        if let Some(pub_) = &book.publisher {
            obj.insert("publisher".into(), json!(pub_));
        }
        if let Some(lang) = &book.language {
            obj.insert("language".into(), json!(lang));
        }
        if !book.narrators.is_empty() {
            obj.insert("narrator".into(), json!(book.narrators.join(", ")));
        }
        if book.abridged {
            obj.insert("is_abridged".into(), json!(true));
        }
    }

    // Provider IDs — book-specific
    let ids = &meta.identifiers;
    if let Some(isbn) = &ids.isbn {
        obj.insert("isbn".into(), json!(isbn));
    }
    if let Some(asin) = &ids.asin {
        obj.insert("asin".into(), json!(asin));
    }
    if ids.has_any() {
        let mut sidecar_ids = serde_json::Map::new();
        if let Some(v) = &ids.isbn {
            sidecar_ids.insert("isbn".into(), json!(v));
        }
        if let Some(v) = &ids.asin {
            sidecar_ids.insert("asin".into(), json!(v));
        }
        if let Some(v) = &ids.openlibrary_id {
            sidecar_ids.insert("openlibrary_id".into(), json!(v));
        }
        if let Some(v) = &ids.hardcover_id {
            sidecar_ids.insert("hardcover_id".into(), json!(v));
        }
        if let Some(v) = &ids.kobbler_id {
            sidecar_ids.insert("kobbler_id".into(), json!(v));
        }
        obj.insert("sidecar_ids".into(), Value::Object(sidecar_ids));
    }
}

/// Strip a trailing parenthetical series suffix from a title.
///
/// Calibre and Audiobookshelf often bake series info into the title:
/// - "Words of Radiance (Stormlight Archive, The)" → ("Words of Radiance", Some("The Stormlight Archive"), None)
/// - "Soulsmith (Cradle Book 2)" → ("Soulsmith", Some("Cradle"), Some(2.0))
/// - "Elantris (Cosmere #1)" → ("Elantris", Some("Cosmere"), Some(1.0))
/// - "Mistborn (Mistborn)" → ("Mistborn", Some("Mistborn"), None)
/// - "Great Book (retail)" → ("Great Book", None, None) — single common words are not series
///
/// Returns (cleaned_title, optional_series_name, optional_series_index).
fn strip_series_from_title(title: &str) -> (String, Option<String>, Option<f64>) {
    use regex::Regex;
    use std::sync::LazyLock;

    static TRAILING_PAREN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s+\(([^)]+)\)\s*$").unwrap());

    let Some(caps) = TRAILING_PAREN.captures(title) else {
        return (title.to_string(), None, None);
    };

    let inner = caps[1].trim();
    let cleaned = title[..caps.get(0).unwrap().start()].trim().to_string();

    // Skip common non-series parentheticals
    let lower = inner.to_lowercase();
    if matches!(
        lower.as_str(),
        "retail" | "unabridged" | "abridged" | "annotated" | "illustrated" | "reprint"
    ) {
        return (cleaned, None, None);
    }

    // Handle Calibre's inverted article: "(Foo, The)" → "The Foo"
    let series_name = if let Some((name, article)) = inner.rsplit_once(", ") {
        let article_lower = article.to_lowercase();
        if matches!(article_lower.as_str(), "the" | "a" | "an") {
            format!("{article} {name}")
        } else {
            inner.to_string()
        }
    } else {
        // Single word in parens — could be series (e.g. "Mistborn") or noise
        // Accept if it looks like a proper noun (starts with uppercase)
        if inner.chars().next().is_some_and(|c| c.is_uppercase()) && inner.len() > 2 {
            inner.to_string()
        } else {
            return (cleaned, None, None);
        }
    };

    // Extract "Book N" / "#N" / ", Book N" pattern from series name
    let (final_name, index) = extract_series_index(&series_name);
    (cleaned, Some(final_name), index)
}

/// Extract a series index ("Book N", "#N", "Volume N") from a series name string.
///
/// Also strips trailing " series" suffix (case-insensitive).
/// Returns (cleaned_name, optional_index).
fn extract_series_index(name: &str) -> (String, Option<f64>) {
    let name = name.trim();

    // Try " Book N" pattern (case-insensitive)
    let lower = name.to_lowercase();
    if let Some(book_pos) = lower.find(" book ") {
        let series = name[..book_pos].trim_end_matches(',').trim();
        let num_str = name[book_pos + 6..].trim();
        if let Ok(idx) = num_str.parse::<f64>()
            && !series.is_empty()
        {
            return (series.to_string(), Some(idx));
        }
    }

    // Try " Volume N" pattern (case-insensitive)
    if let Some(vol_pos) = lower.find(" volume ") {
        let series = name[..vol_pos].trim_end_matches(',').trim();
        let num_str = name[vol_pos + 8..].trim();
        if let Ok(idx) = num_str.parse::<f64>()
            && !series.is_empty()
        {
            return (series.to_string(), Some(idx));
        }
    }

    // Try "#N" pattern
    if let Some(hash_pos) = name.rfind('#') {
        let series = name[..hash_pos].trim_end_matches(',').trim();
        let num_str = name[hash_pos + 1..].trim();
        if let Ok(idx) = num_str.parse::<f64>()
            && !series.is_empty()
        {
            return (series.to_string(), Some(idx));
        }
    }

    // Strip trailing " series" (case-insensitive)
    let result = if lower.ends_with(" series") {
        name[..name.len() - 7].trim().to_string()
    } else {
        name.to_string()
    };

    (result, None)
}

/// Reject garbage probe series values that would corrupt interpret data.
///
/// Returns false for:
/// - Single-character strings
/// - Strings that are all punctuation/digits/whitespace
/// - Date-like patterns ("22-Jul-2014", "Jan 2020")
fn is_valid_series_name(name: &str) -> bool {
    use regex::Regex;
    use std::sync::LazyLock;

    let trimmed = name.trim();
    if trimmed.len() < 2 {
        return false;
    }
    // All punctuation/digits/whitespace — no actual letters
    if !trimmed.chars().any(|c| c.is_alphabetic()) {
        return false;
    }
    // Date-like: contains a month abbreviation adjacent to digits
    static DATE_LIKE_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(?:jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)\w*[\s./-]*\d|\d[\s./-]*(?:jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)").unwrap()
    });
    if DATE_LIKE_RE.is_match(trimmed) {
        return false;
    }
    true
}

/// Detect ordinal-patterned chapter/track titles like "Book The 1st-" or
/// "Chapter The 3rd -". These are track names, not series info.
fn is_ordinal_chapter_title(title: &str) -> bool {
    use regex::Regex;
    use std::sync::LazyLock;

    static ORDINAL_TITLE_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^(?:book|chapter|part|track|disc)\s+(?:the\s+)?\d+(?:st|nd|rd|th)\b")
            .unwrap()
    });
    ORDINAL_TITLE_RE.is_match(title.trim())
}

/// Clean a raw series name from sidecar/ebook-meta/ffprobe metadata.
///
/// Applies `extract_series_index` to strip "Book N" / "#N" / "Volume N"
/// patterns from the series name. If an index is already known (from a
/// separate field), it's preserved unless the name itself yields one.
fn clean_series_name(name: &str, existing_index: Option<f64>) -> (String, Option<f64>) {
    let (cleaned, extracted_index) = extract_series_index(name);
    let index = extracted_index.or(existing_index);
    (cleaned, index)
}

/// Normalize a "Last, First" author name to "First Last".
///
/// Calibre stores authors as "Sanderson, Brandon" — uninvert for display
/// and search. Only applies to single-comma names (avoids breaking
/// "Organization, Inc." or "de Balzac, Honoré").
fn normalize_author_name(name: &str) -> String {
    let name = name.trim();
    // Only uninvert if exactly one comma and both parts look like names
    if let Some((last, first)) = name.split_once(',') {
        let last = last.trim();
        let first = first.trim();
        // Heuristic: both parts non-empty, first part has no comma,
        // and first part starts with uppercase (a name, not a suffix like "Jr.")
        if !first.is_empty()
            && !last.is_empty()
            && first.chars().next().is_some_and(|c| c.is_uppercase())
            && !first.contains(',')
        {
            return format!("{first} {last}");
        }
    }
    name.to_string()
}

/// Check if a title looks like garbage data that shouldn't be trusted.
///
/// Returns true if the title is clearly not a real book title:
/// - Dotted paths like "Stephen.King-11.22.63.(v5.0)"
/// - Starts with a number+period (series position, not title): "2. Diggers"
fn is_garbage_title(title: &str) -> bool {
    // Dotted path: 3+ dots with no spaces suggests a filename, not a title
    if !title.contains(' ') && title.matches('.').count() >= 3 {
        return true;
    }
    // Starts with "N. " or "N - " (series position prefix)
    if let Some(first) = title.chars().next()
        && first.is_ascii_digit()
    {
        let rest = &title[first.len_utf8()..];
        if rest.starts_with(". ") || rest.starts_with(" - ") {
            return true;
        }
    }
    false
}

/// Check if an author name is clearly garbage/placeholder.
fn is_garbage_author(author: &str) -> bool {
    let lower = author.to_lowercase();
    lower == "unknown"
        || lower == "various"
        || lower == "graphic novel"
        || lower == "n/a"
        || author.is_empty()
        // Parenthetical/bracketed groups indicate scene tags or metadata noise,
        // not real author names (e.g. "Cat Kid (2018) (digital) (DrVink-HD-DCP)")
        || author.contains('(')
        || author.contains('[')
}

/// Check if all remaining authors appear to be derived from the title.
///
/// When embedded metadata is garbage (e.g. CBZ files), author names are often
/// just fragments of the title. If every author is a case-insensitive substring
/// of the title, they're likely not real authors.
fn all_authors_title_derived(title: &str, authors: &[&str]) -> bool {
    if authors.is_empty() {
        return false;
    }
    let title_lower = title.to_lowercase();
    authors
        .iter()
        .all(|a| title_lower.contains(&a.to_lowercase()))
}

/// Try to build a book interpretation entirely from sidecar metadata.
///
/// Returns `Some(Value)` if the sidecar has a valid title.
/// Rejects garbage titles/authors so the fallback chain can try better sources.
fn try_sidecar_book_interpretation(sidecar_json: &Option<Value>) -> Option<Value> {
    let sidecar_json = sidecar_json.as_ref()?;
    let meta: SidecarMetadata = serde_json::from_value(sidecar_json.clone()).ok()?;
    let title = meta.title.as_ref().filter(|t| !t.is_empty())?;

    // Reject garbage titles — let fallback chain try filename parsing
    if is_garbage_title(title) {
        return None;
    }

    // Strip parenthetical series suffix from title (Calibre/ABS convention)
    let has_series_from_sidecar = meta.book.as_ref().and_then(|b| b.series.as_ref()).is_some();
    let (clean_title, extracted_series, extracted_index) = strip_series_from_title(title);

    let mut obj = json!({
        "media_type": "book",
        "source": "sidecar",
        "title": clean_title,
    });

    if let Some(book) = &meta.book {
        // Filter out garbage authors and normalize "Last, First" → "First Last"
        let good_authors: Vec<String> = book
            .authors
            .iter()
            .filter(|a| !is_garbage_author(a))
            .map(|a| normalize_author_name(a))
            .collect();
        if !good_authors.is_empty() {
            obj["authors"] = json!(good_authors);
        }
        if let Some(series) = &book.series {
            let (clean, idx) = clean_series_name(series, book.series_index);
            obj["series"] = json!(clean);
            if let Some(idx) = idx {
                obj["series_index"] = json!(idx);
            }
        } else {
            if let Some(ref series) = extracted_series {
                obj["series"] = json!(series);
            }
            if let Some(idx) = extracted_index.or(book.series_index) {
                obj["series_index"] = json!(idx);
            }
        }
        if let Some(desc) = &book.description {
            obj["description"] = json!(desc);
        }
        if let Some(pub_) = &book.publisher {
            obj["publisher"] = json!(pub_);
        }
        if let Some(lang) = &book.language {
            obj["language"] = json!(lang);
        }
        if !book.narrators.is_empty() {
            obj["narrator"] = json!(book.narrators.join(", "));
        }
        if book.abridged {
            obj["is_abridged"] = json!(true);
        }
    } else if !has_series_from_sidecar {
        // No book section at all — use extracted series from title if available
        if let Some(ref series) = extracted_series {
            obj["series"] = json!(series);
        }
    }

    // Include provider IDs for the match phase
    let ids = &meta.identifiers;
    if let Some(isbn) = &ids.isbn {
        obj["isbn"] = json!(isbn);
    }
    if let Some(asin) = &ids.asin {
        obj["asin"] = json!(asin);
    }
    if ids.has_any() {
        let mut sidecar_ids = serde_json::Map::new();
        if let Some(v) = &ids.isbn {
            sidecar_ids.insert("isbn".into(), json!(v));
        }
        if let Some(v) = &ids.asin {
            sidecar_ids.insert("asin".into(), json!(v));
        }
        if let Some(v) = &ids.openlibrary_id {
            sidecar_ids.insert("openlibrary_id".into(), json!(v));
        }
        if let Some(v) = &ids.hardcover_id {
            sidecar_ids.insert("hardcover_id".into(), json!(v));
        }
        obj["sidecar_ids"] = Value::Object(sidecar_ids);
    }

    Some(obj)
}

/// Check if a probe title looks like a chapter/track title rather than a book title.
/// Examples: "01: Introduction", "Chapter 3", "03", "Intro", "00: Intro"
fn is_chapter_title(title: &str) -> bool {
    let t = title.trim();
    // Pure number
    if t.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    // Starts with "NN:" or "NN -" or "NN." (numbered chapter)
    if let Some(pos) = t.find(|c: char| !c.is_ascii_digit())
        && pos > 0
        && matches!(t.as_bytes().get(pos), Some(b':' | b'-' | b'.'))
    {
        return true;
    }
    let lower = t.to_lowercase();
    // "Chapter N", "Part N", "Track N"
    if lower.starts_with("chapter ") || lower.starts_with("part ") || lower.starts_with("track ") {
        return true;
    }
    false
}

/// Check if two titles agree using fuzzy title matching.
/// Normalizes both titles (lowercase, strip punctuation) before comparing.
fn titles_agree(a: &str, b: &str) -> bool {
    use crate::metadata::similarity::{fuzzy_match_title, strip_punctuation};
    let a_norm = strip_punctuation(&a.to_lowercase());
    let b_norm = strip_punctuation(&b.to_lowercase());
    fuzzy_match_title(&a_norm, &b_norm)
}

/// Clean a narrator value by splitting on `/` and `©`, filtering out copyright
/// notices and non-person strings.
///
/// Example: `"Jeff Kinney/Ramon de Ocampo/©2009 Jeff Kinney"` → `"Ramon de Ocampo"`
fn clean_narrator_value(raw: &str) -> String {
    // Split on `/` and filter
    let parts: Vec<&str> = raw
        .split('/')
        .map(|s| s.trim())
        .filter(|s| {
            !s.is_empty()
                && !s.contains('©')
                && !s.starts_with("℗")
                && !s.chars().all(|c| c.is_ascii_digit() || c.is_whitespace())
        })
        .collect();

    // If there are multiple parts, prefer the first one that looks like a person name
    // (2+ words, starts with an uppercase letter)
    for part in &parts {
        let words: Vec<&str> = part.split_whitespace().collect();
        if words.len() >= 2
            && words[0]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase())
        {
            return part.to_string();
        }
    }

    // Fall back to first non-empty part
    parts.first().unwrap_or(&raw).to_string()
}

/// Check if a string looks like a non-author value (cast descriptor, publisher, etc.)
fn looks_like_non_author(s: &str) -> bool {
    let lower = s.to_lowercase();
    // Known publishers/labels that end up as author via audio tags
    const PUBLISHERS: &[&str] = &[
        "graphicaudio",
        "bolinda",
        "audible",
        "audible studios",
        "macmillan",
        "penguin",
        "harpercollins",
        "random house",
        "simon & schuster",
        "hachette",
        "brilliance audio",
        "recorded books",
        "tantor",
        "blackstone",
    ];
    lower.contains("full cast")
        || lower.contains("various")
        || lower.contains("unknown")
        || s.contains('©')
        || s.contains('/') && s.matches('/').count() > 1
        || PUBLISHERS.iter().any(|p| lower == *p)
}

/// Clean an author value by removing common garbage patterns from audio tags
/// and directory-derived names.
fn clean_author_value(author: &str) -> String {
    let mut cleaned = author.to_string();

    // Strip "None" tokens (e.g., "None Andy None Griffiths" -> "Andy Griffiths")
    if cleaned.contains("None") {
        cleaned = cleaned
            .split_whitespace()
            .filter(|w| *w != "None")
            .collect::<Vec<_>>()
            .join(" ");
    }

    // Strip parenthetical/bracket suffixes: "(Flesh & Fire)", "[Unknown Binding]"
    if let Some(pos) = cleaned.find(" (") {
        cleaned.truncate(pos);
    }
    if let Some(pos) = cleaned.find(" [") {
        cleaned.truncate(pos);
    }

    cleaned.trim().to_string()
}

/// Try to extract series name and position from a probe title.
///
/// Matches patterns like:
/// - `"The Diary of a Wimpy Kid - Diary of a Wimpy Kid Series, Book 1"`
/// - `"Title - Series Name, Book 3"`
fn extract_series_from_title(title: &str) -> Option<(String, f64)> {
    use regex::Regex;
    use std::sync::LazyLock;

    static SERIES_IN_TITLE_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)(?:[-–—]\s*)?(?P<series>[^,]+?)\s*(?:Series\s*,?\s*)?(?:Book|Bk\.?|Vol\.?|#)\s*(?P<pos>\d+(?:\.\d+)?)\s*$",
        )
        .unwrap()
    });

    let caps = SERIES_IN_TITLE_RE.captures(title)?;
    let series = caps.name("series")?.as_str().trim();
    let pos: f64 = caps.name("pos")?.as_str().parse().ok()?;

    // Reject very short or suspicious series names
    if series.len() < 3 {
        return None;
    }

    Some((series.to_string(), pos))
}

#[async_trait]
impl JobHandler for MediaRootInterpretHandler {
    fn job_type(&self) -> &'static str {
        "media_root_interpret"
    }

    fn max_concurrent(&self) -> usize {
        self.runtime_config
            .job_concurrency("media_root_interpret", 3)
    }

    async fn execute(
        &self,
        _job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: MediaRootInterpretPayload = serde_json::from_value(payload)?;
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

        if !media_roots::check_available(&self.pool, &root).await? {
            return Ok(json!({"deferred": true, "reason": "media_root_unavailable"}));
        }

        let pending = media_root_files::get_pending(&self.pool, root.id, BATCH_SIZE).await?;
        if pending.is_empty() {
            return Ok(json!({"interpreted": 0, "message": "No pending files"}));
        }

        let (interpreted, failed) = match root.media_type.as_str() {
            "book" => self.interpret_book_files(&root, &pending).await,
            "audiobook" => self.interpret_audiobook_files(&root, &pending).await,
            _ => self.interpret_video_files(&root, &pending).await,
        };

        // Enqueue matching if we interpreted any files
        if interpreted > 0 {
            let display = crate::jobs::fetch_job_display(&self.pool, "media_root", root.id).await;
            let context = JobContext::for_entity(
                "media_root",
                root.id,
                format!("Matching {} files in '{}'", interpreted, root.name),
            )
            .with_display_opt(display);
            if let Err(e) = crate::jobs::enqueue_routine(
                &self.pool,
                &self.nats,
                "media_root_match",
                json!({"media_root_id": root.id}),
                context,
            )
            .await
            {
                warn!(error = %e, "Failed to enqueue media_root_match");
            }
        }

        // If more pending files remain, enqueue another interpret batch
        let remaining = media_root_files::get_pending(&self.pool, root.id, 1).await?;
        if !remaining.is_empty() {
            let display = crate::jobs::fetch_job_display(&self.pool, "media_root", root.id).await;
            let context = JobContext::for_entity(
                "media_root",
                root.id,
                format!("Interpreting more files in '{}'", root.name),
            )
            .with_display_opt(display);
            let _ = crate::jobs::enqueue_routine(
                &self.pool,
                &self.nats,
                "media_root_interpret",
                json!({"media_root_id": root.id}),
                context,
            )
            .await;
        }

        info!(
            root = %root.name,
            interpreted = interpreted,
            failed = failed,
            "Media root interpretation complete"
        );

        Ok(json!({
            "interpreted": interpreted,
            "failed": failed,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // is_garbage_title
    // =========================================================================

    #[test]
    fn garbage_title_dotted_path() {
        assert!(is_garbage_title("Stephen.King-11.22.63.(v5.0)"));
    }

    #[test]
    fn garbage_title_numbered_prefix_dot() {
        assert!(is_garbage_title("2. Diggers"));
    }

    #[test]
    fn garbage_title_numbered_prefix_dash() {
        assert!(is_garbage_title("3 - Truckers"));
    }

    #[test]
    fn garbage_title_normal_title() {
        assert!(!is_garbage_title("The Hobbit"));
    }

    #[test]
    fn garbage_title_title_with_subtitle() {
        assert!(!is_garbage_title("Dune: Part Two"));
    }

    #[test]
    fn garbage_title_number_in_title() {
        // "1984" starts with a digit but has no ". " or " - " after
        assert!(!is_garbage_title("1984"));
    }

    #[test]
    fn garbage_title_empty() {
        assert!(!is_garbage_title(""));
    }

    // =========================================================================
    // is_garbage_author
    // =========================================================================

    #[test]
    fn garbage_author_unknown() {
        assert!(is_garbage_author("Unknown"));
        assert!(is_garbage_author("unknown"));
        assert!(is_garbage_author("UNKNOWN"));
    }

    #[test]
    fn garbage_author_various() {
        assert!(is_garbage_author("Various"));
    }

    #[test]
    fn garbage_author_graphic_novel() {
        assert!(is_garbage_author("Graphic Novel"));
    }

    #[test]
    fn garbage_author_na() {
        assert!(is_garbage_author("N/A"));
        assert!(is_garbage_author("n/a"));
    }

    #[test]
    fn garbage_author_empty() {
        assert!(is_garbage_author(""));
    }

    #[test]
    fn garbage_author_real_name() {
        assert!(!is_garbage_author("Terry Pratchett"));
    }

    #[test]
    fn garbage_author_another_real_name() {
        assert!(!is_garbage_author("Stephen King"));
    }

    #[test]
    fn garbage_author_with_parenthetical() {
        assert!(is_garbage_author(
            "Cat Kid (2018) (digital) (DrVink-HD-DCP)"
        ));
    }

    #[test]
    fn garbage_author_with_brackets() {
        assert!(is_garbage_author("SomeGroup [TAG]"));
    }

    #[test]
    fn garbage_author_real_with_period() {
        assert!(!is_garbage_author("J.K. Rowling"));
    }

    #[test]
    fn garbage_author_real_with_initial() {
        assert!(!is_garbage_author("J. Lynn"));
    }

    // =========================================================================
    // all_authors_title_derived
    // =========================================================================

    #[test]
    fn title_derived_author_matches_title() {
        assert!(all_authors_title_derived("Dog Man v04", &["Dog Man"]));
    }

    #[test]
    fn title_derived_author_no_match() {
        assert!(!all_authors_title_derived("Upgrade", &["Blake Crouch"]));
    }

    #[test]
    fn title_derived_empty_authors() {
        assert!(!all_authors_title_derived("Dog Man v04", &[]));
    }

    #[test]
    fn title_derived_mixed_authors() {
        // One matches, one doesn't → not all title-derived
        assert!(!all_authors_title_derived(
            "Dog Man v04",
            &["Dog Man", "Dav Pilkey"]
        ));
    }

    // =========================================================================
    // is_chapter_title (kept — used in probe metadata filtering)
    // =========================================================================

    #[test]
    fn chapter_title_numbered() {
        assert!(is_chapter_title("03: Pleasure and the Worm"));
        assert!(is_chapter_title("01"));
        assert!(is_chapter_title("00: Intro"));
        assert!(is_chapter_title("12.The End"));
        assert!(is_chapter_title("5-Beginning"));
    }

    #[test]
    fn chapter_title_prefixed() {
        assert!(is_chapter_title("Chapter 3"));
        assert!(is_chapter_title("Part 1"));
        assert!(is_chapter_title("Track 05"));
    }

    #[test]
    fn chapter_title_real_title_not_chapter() {
        assert!(!is_chapter_title("The Thief of Always"));
        assert!(!is_chapter_title("Mistborn: The Final Empire"));
        assert!(!is_chapter_title("8 Bit Warrior"));
    }

    // =========================================================================
    // strip_series_from_title
    // =========================================================================

    #[test]
    fn strip_series_calibre_inverted_article() {
        let (title, series, idx) =
            strip_series_from_title("Words of Radiance (Stormlight Archive, The)");
        assert_eq!(title, "Words of Radiance");
        assert_eq!(series.as_deref(), Some("The Stormlight Archive"));
        assert_eq!(idx, None);
    }

    #[test]
    fn strip_series_simple_parens() {
        let (title, series, idx) = strip_series_from_title("Mistborn (Mistborn)");
        assert_eq!(title, "Mistborn");
        assert_eq!(series.as_deref(), Some("Mistborn"));
        assert_eq!(idx, None);
    }

    #[test]
    fn strip_series_no_parens() {
        let (title, series, _) = strip_series_from_title("The Hobbit");
        assert_eq!(title, "The Hobbit");
        assert!(series.is_none());
    }

    #[test]
    fn strip_series_retail_suffix() {
        let (title, series, _) = strip_series_from_title("Great Book (retail)");
        assert_eq!(title, "Great Book");
        assert!(series.is_none());
    }

    #[test]
    fn strip_series_unabridged() {
        let (title, series, _) = strip_series_from_title("Great Book (Unabridged)");
        assert_eq!(title, "Great Book");
        assert!(series.is_none());
    }

    #[test]
    fn strip_series_multi_word() {
        let (title, series, idx) = strip_series_from_title("Oathbringer (The Stormlight Archive)");
        assert_eq!(title, "Oathbringer");
        assert_eq!(series.as_deref(), Some("The Stormlight Archive"));
        assert_eq!(idx, None);
    }

    #[test]
    fn strip_series_extracts_book_index() {
        let (title, series, idx) =
            strip_series_from_title("We Are Legion (We Are Bob) (Bobiverse Book 1)");
        assert_eq!(title, "We Are Legion (We Are Bob)");
        assert_eq!(series.as_deref(), Some("Bobiverse"));
        assert_eq!(idx, Some(1.0));
    }

    #[test]
    fn strip_series_extracts_hash_index() {
        let (title, series, idx) = strip_series_from_title("Elantris (Cosmere #1)");
        assert_eq!(title, "Elantris");
        assert_eq!(series.as_deref(), Some("Cosmere"));
        assert_eq!(idx, Some(1.0));
    }

    #[test]
    fn strip_series_extracts_comma_book_index() {
        let (title, series, idx) =
            strip_series_from_title("The Way of Kings (The Stormlight Archive, Book 1)");
        assert_eq!(title, "The Way of Kings");
        // ", Book 1" triggers inverted-article check first — "Book 1" is not "the"/"a"/"an",
        // so the whole content becomes the series name, then extract_series_index finds "Book 1"
        assert_eq!(series.as_deref(), Some("The Stormlight Archive"));
        assert_eq!(idx, Some(1.0));
    }

    #[test]
    fn strip_series_trailing_series_word() {
        let (_, series, idx) = strip_series_from_title("Some Book (Covenant series)");
        assert_eq!(series.as_deref(), Some("Covenant"));
        assert_eq!(idx, None);
    }

    // =========================================================================
    // extract_series_index
    // =========================================================================

    #[test]
    fn extract_series_index_book_pattern() {
        let (name, idx) = extract_series_index("Bobiverse Book 1");
        assert_eq!(name, "Bobiverse");
        assert_eq!(idx, Some(1.0));
    }

    #[test]
    fn extract_series_index_hash_pattern() {
        let (name, idx) = extract_series_index("Covenant #4");
        assert_eq!(name, "Covenant");
        assert_eq!(idx, Some(4.0));
    }

    #[test]
    fn extract_series_index_volume_pattern() {
        let (name, idx) = extract_series_index("Wait for You Volume 6");
        assert_eq!(name, "Wait for You");
        assert_eq!(idx, Some(6.0));
    }

    #[test]
    fn extract_series_index_no_pattern() {
        let (name, idx) = extract_series_index("Discworld");
        assert_eq!(name, "Discworld");
        assert_eq!(idx, None);
    }

    #[test]
    fn extract_series_index_trailing_series() {
        let (name, idx) = extract_series_index("Covenant series");
        assert_eq!(name, "Covenant");
        assert_eq!(idx, None);
    }

    // =========================================================================
    // is_valid_series_name
    // =========================================================================

    #[test]
    fn valid_series_name_normal() {
        assert!(is_valid_series_name("Discworld"));
        assert!(is_valid_series_name("A Series of Unfortunate Events"));
    }

    #[test]
    fn valid_series_name_rejects_single_char() {
        assert!(!is_valid_series_name("X"));
        assert!(!is_valid_series_name(" "));
        assert!(!is_valid_series_name(""));
    }

    #[test]
    fn valid_series_name_rejects_all_punctuation_digits() {
        assert!(!is_valid_series_name("123"));
        assert!(!is_valid_series_name("---"));
        assert!(!is_valid_series_name("12.34"));
    }

    #[test]
    fn valid_series_name_rejects_date_patterns() {
        assert!(!is_valid_series_name("22-Jul-2014"));
        assert!(!is_valid_series_name("Jan 2020"));
        assert!(!is_valid_series_name("22-Jul-"));
        assert!(!is_valid_series_name("December 5"));
    }

    #[test]
    fn valid_series_name_allows_month_words_in_context() {
        // "March" alone is ambiguous but valid as a series name
        assert!(is_valid_series_name("The March Family"));
        // "May" followed by a digit still triggers date detection
        assert!(!is_valid_series_name("May 3"));
    }

    // =========================================================================
    // is_ordinal_chapter_title
    // =========================================================================

    #[test]
    fn ordinal_chapter_title_detects_patterns() {
        assert!(is_ordinal_chapter_title("Book The 1st- The Bad Beginning"));
        assert!(is_ordinal_chapter_title("Chapter The 3rd - Something"));
        assert!(is_ordinal_chapter_title("Part 2nd Edition"));
        assert!(is_ordinal_chapter_title("Book 1st"));
    }

    #[test]
    fn ordinal_chapter_title_allows_normal_titles() {
        assert!(!is_ordinal_chapter_title("The Bad Beginning"));
        assert!(!is_ordinal_chapter_title(
            "A Series of Unfortunate Events Book 1"
        ));
        assert!(!is_ordinal_chapter_title("Discworld"));
    }

    // =========================================================================
    // clean_series_name
    // =========================================================================

    #[test]
    fn clean_series_name_book_pattern() {
        let (name, idx) = clean_series_name("Bobiverse Book 1", None);
        assert_eq!(name, "Bobiverse");
        assert_eq!(idx, Some(1.0));
    }

    #[test]
    fn clean_series_name_preserves_existing_index() {
        // Existing index wins when extract_series_index finds nothing
        let (name, idx) = clean_series_name("Discworld", Some(5.0));
        assert_eq!(name, "Discworld");
        assert_eq!(idx, Some(5.0));
    }

    #[test]
    fn clean_series_name_extracted_overrides_existing() {
        // Extracted index takes priority over existing
        let (name, idx) = clean_series_name("Cradle Book 10", Some(3.0));
        assert_eq!(name, "Cradle");
        assert_eq!(idx, Some(10.0));
    }

    #[test]
    fn clean_series_name_no_change() {
        let (name, idx) = clean_series_name("Discworld", None);
        assert_eq!(name, "Discworld");
        assert_eq!(idx, None);
    }

    // =========================================================================
    // normalize_author_name
    // =========================================================================

    #[test]
    fn normalize_author_last_first() {
        assert_eq!(
            normalize_author_name("Sanderson, Brandon"),
            "Brandon Sanderson"
        );
    }

    #[test]
    fn normalize_author_already_normal() {
        assert_eq!(
            normalize_author_name("Brandon Sanderson"),
            "Brandon Sanderson"
        );
    }

    #[test]
    fn normalize_author_with_middle() {
        assert_eq!(
            normalize_author_name("Clarke, Arthur C."),
            "Arthur C. Clarke"
        );
    }

    #[test]
    fn normalize_author_single_name() {
        assert_eq!(normalize_author_name("Plato"), "Plato");
    }
}
