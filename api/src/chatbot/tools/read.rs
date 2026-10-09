//! The read-only tools. Every one of these is safe for any role.
//!
//! Two rules shape what comes back:
//!
//! * observations are for the model, so they are compact JSON with explicit
//!   fields — poster URLs and first-letter facets are prompt budget spent on
//!   nothing;
//! * where the underlying data is unreliable, the observation carries the
//!   hedge as *data* (`date_confidence`, `caveat`). Asking the model to
//!   remember to hedge is asking it to forget.

use std::str::FromStr as _;

use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::AppState;
use crate::chatbot::events::{Source, source_from_doc};
use crate::chatbot::resolve;
use crate::repositories::{chat_tools, dashboard};
use crate::search::indexes;

/// What one tool call produced.
pub struct ToolOutcome {
    pub ok: bool,
    /// Compact JSON the model reads back as the tool result.
    pub observation: Value,
    /// One line for a transport to show a human.
    pub detail: String,
    /// Library items the answer may cite.
    pub sources: Vec<Source>,
}

impl ToolOutcome {
    pub fn ok(observation: Value, detail: impl Into<String>) -> Self {
        Self {
            ok: true,
            observation,
            detail: detail.into(),
            sources: Vec::new(),
        }
    }

    pub fn with_sources(mut self, sources: Vec<Source>) -> Self {
        self.sources = sources;
        self
    }

    pub fn failed(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            ok: false,
            observation: json!({"ok": false, "error": reason}),
            detail: reason,
            sources: Vec::new(),
        }
    }
}

const MAX_SEARCH_HITS: usize = 40;
const MAX_PASSAGES: usize = 20;
const MAX_DOWNLOADS: usize = 25;
const MAX_RECENT: usize = 30;

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn arg_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

fn limit(args: &Value, default: usize, max: usize) -> usize {
    arg_u64(args, "limit")
        .map(|n| n as usize)
        .unwrap_or(default)
        .clamp(1, max)
}

/// Which Meili indexes a `media_type` argument covers.
fn indexes_for(media_type: Option<&str>) -> Vec<&'static str> {
    match media_type {
        Some("book") => vec![indexes::BOOKS],
        Some("movie") => vec![indexes::MOVIES],
        Some("show") => vec![indexes::TV_SHOWS],
        Some("episode") => vec![indexes::TV_EPISODES],
        Some("person") => vec![indexes::PEOPLE],
        Some("author") => vec![indexes::AUTHORS],
        Some("series") => vec![indexes::SERIES],
        Some("genre") => vec![indexes::GENRES],
        _ => vec![
            indexes::BOOKS,
            indexes::MOVIES,
            indexes::TV_SHOWS,
            indexes::TV_EPISODES,
            indexes::PEOPLE,
            indexes::AUTHORS,
            indexes::SERIES,
            indexes::GENRES,
        ],
    }
}

/// Fields worth spending prompt tokens on, per index. Everything not listed —
/// posters, first-letter facets, sort keys — is deliberately left behind.
fn attributes_for(index: &str) -> Vec<&'static str> {
    match index {
        indexes::BOOKS => vec![
            "id",
            "title",
            "authors",
            "series_name",
            "series_index",
            "genres",
            "narrator",
            "is_in_library",
            "want_status",
            "has_audiobook",
            "has_ebook",
            "description",
            "cover_url",
        ],
        indexes::MOVIES => vec![
            "id",
            "title",
            "year",
            "directors",
            "genres",
            "runtime",
            "content_rating",
            "rating",
            "is_in_library",
            "want_status",
            "description",
            "poster_url",
        ],
        indexes::TV_SHOWS => vec![
            "id",
            "title",
            "year",
            "genres",
            "network",
            "status",
            "content_rating",
            "rating",
            "is_monitored",
            "available_episode_count",
            "total_episodes",
            "description",
            "poster_url",
        ],
        indexes::TV_EPISODES => vec![
            "id",
            "display_title",
            "show_title",
            "episode_title",
            "episode_code",
            "air_date",
            "is_in_library",
            "want_status",
            "show_id",
            "season_id",
        ],
        indexes::PEOPLE => vec!["id", "name", "movie_count", "tv_show_count"],
        indexes::AUTHORS => vec!["id", "name", "book_count", "is_monitored"],
        indexes::SERIES => vec!["id", "name", "book_count", "is_monitored"],
        indexes::GENRES => vec!["id", "name", "book_count"],
        _ => vec!["id", "title", "name"],
    }
}

/// Multi-index search over Meili's plain search API.
///
/// One `/multi-search` round trip rather than eight sequential ones — the model
/// searches on nearly every turn, so the difference is per-message latency, not
/// a micro-optimisation. Meili is a search engine here and nothing more; it
/// stopped being an LLM proxy when the loop moved into kobbler.
pub async fn search_library(state: &AppState, args: &Value) -> ToolOutcome {
    let Some(query) = arg_str(args, "query") else {
        return ToolOutcome::failed("search_library needs a query");
    };
    let per_index = limit(args, 10, MAX_SEARCH_HITS);
    let media_type = arg_str(args, "media_type").filter(|m| *m != "any");
    let targets = indexes_for(media_type);

    let numbering = episode_filter(arg_u64(args, "season"), arg_u64(args, "episode"));

    let queries: Vec<Value> = targets
        .iter()
        .map(|index| {
            let mut query_spec = json!({
                "indexUid": index,
                "q": query,
                "limit": per_index,
                "attributesToRetrieve": attributes_for(index),
            });
            if *index == indexes::TV_EPISODES
                && let Some(filter) = &numbering
            {
                query_spec["filter"] = json!(filter);
                query_spec["sort"] = json!(["season_sort_order:asc", "episode_sort_order:asc"]);
            }
            query_spec
        })
        .collect();

    let body = json!({"queries": queries});
    let results = match multi_search(state, &body).await {
        Ok(v) => v,
        Err(e) => return ToolOutcome::failed(e),
    };

    let mut hits = Vec::new();
    let mut sources = Vec::new();
    for result in results {
        let index = result
            .get("indexUid")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let kind = source_kind(&index);
        for hit in result
            .get("hits")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let mut typed = hit.clone();
            if let Some(obj) = typed.as_object_mut() {
                obj.insert("type".to_string(), json!(kind));
                if let Some(aired) = has_aired(obj.get("air_date")) {
                    obj.insert("aired".to_string(), json!(aired));
                }
            }
            if let Some(source) = source_from_doc(&typed) {
                sources.push(source);
            }
            hits.push(typed);
        }
    }

    let detail = format!(
        "{} result{}",
        hits.len(),
        if hits.len() == 1 { "" } else { "s" }
    );
    ToolOutcome::ok(
        json!({"ok": true, "query": query, "count": hits.len(), "results": hits}),
        detail,
    )
    .with_sources(sources)
}

/// Whether an episode's air date has come, for the hits that carry one.
///
/// The index stores ISO dates, so a string compare against today is the whole
/// comparison. Handed over as a fact rather than left to the model: shown a
/// scatter of episodes with their dates, it read the highest episode number as
/// "the latest" and offered one that airs next month.
fn has_aired(air_date: Option<&Value>) -> Option<bool> {
    let air_date = air_date?.as_str()?;
    let today = OffsetDateTime::now_utc().date();
    let today = format!(
        "{:04}-{:02}-{:02}",
        today.year(),
        u8::from(today.month()),
        today.day()
    );
    Some(air_date <= today.as_str())
}

/// A real filter on the episodes index, not a padded code bolted onto the query
/// text.
///
/// `episode_code` is searchable and not filterable, which is why the code used
/// to ride in the query — but `season_sort_order` and `episode_sort_order` are
/// both filterable, and fuzzy-matching "S03" against titles is what returned a
/// scatter of eight episodes across three seasons and let the model tally them
/// as a season. `season_sort_order` holds the season number for TV and the
/// *year* for sport, so "season 3" of a competition correctly matches nothing
/// rather than matching its third event.
fn episode_filter(season: Option<u64>, episode: Option<u64>) -> Option<String> {
    let clauses: Vec<String> = [
        season.map(|s| format!("season_sort_order = {s}")),
        episode.map(|e| format!("episode_sort_order = {e}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    match clauses.is_empty() {
        true => None,
        false => Some(clauses.join(" AND ")),
    }
}

fn source_kind(index: &str) -> &'static str {
    match index {
        indexes::BOOKS => "book",
        indexes::MOVIES => "movie",
        indexes::TV_SHOWS => "tv_show",
        indexes::TV_EPISODES => "tv_episode",
        indexes::PEOPLE => "person",
        indexes::AUTHORS => "author",
        indexes::SERIES => "series",
        indexes::GENRES => "genre",
        _ => "unknown",
    }
}

async fn multi_search(state: &AppState, body: &Value) -> Result<Vec<Value>, String> {
    let config = state.runtime_config.get().await;
    let url = config
        .meili_url
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&state.config.meili_url)
        .trim_end_matches('/')
        .to_string();
    let key = config
        .meili_master_key
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&state.config.meili_master_key)
        .to_string();

    let response = crate::http::new_client()
        .post(format!("{url}/multi-search"))
        .bearer_auth(key)
        .json(body)
        .send()
        .await
        .map_err(|e| format!("the search index is unreachable: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "the search index returned {}",
            response.status().as_u16()
        ));
    }
    let value: Value = response
        .json()
        .await
        .map_err(|e| format!("the search index sent something unreadable: {e}"))?;
    Ok(value
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// `resolve_media` — which one thing the user means.
///
/// The resolver is a plain library with no chat, no LLM and no GPU in it; this
/// is the seam that hands it the caller's identity (watch state is per-user) and
/// the previous turn's ref, so "the new one" and "no, the other one" have a
/// referent. An ambiguous answer is returned as candidates plus the question —
/// picking one here would be exactly the failure that ends the trial.
pub async fn resolve_media(
    state: &AppState,
    user_id: Uuid,
    context: Option<&str>,
    args: &Value,
) -> ToolOutcome {
    let Some(query) = arg_str(args, "query") else {
        return ToolOutcome::failed("resolve_media needs a query");
    };

    let config = state.runtime_config.get().await;
    let url = config
        .meili_url
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&state.config.meili_url)
        .to_string();
    let key = config
        .meili_master_key
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&state.config.meili_master_key)
        .to_string();
    let index = match resolve::index::MeiliLibraryIndex::new(&url, &key) {
        Ok(i) => i,
        Err(e) => return ToolOutcome::failed(format!("could not reach the search index: {e}")),
    };

    let books = crate::metadata::ProviderRegistry::new(state.pool.clone());
    let video = crate::video_metadata::VideoProviderRegistry::new(state.pool.clone());
    let providers = resolve::providers::RegistryLookup {
        books: &books,
        video: &video,
    };
    let store = resolve::store::PgResolveStore::new(state.pool.clone());

    let mut request = resolve::ResolveRequest::new(query);
    request.media_type = arg_str(args, "media_type").and_then(resolve::MediaTypeHint::parse);
    request.season = arg_u64(args, "season").map(|n| n as i32);
    request.episode = arg_u64(args, "episode").map(|n| n as i32);
    request.author = arg_str(args, "author").map(str::to_string);
    request.context = context.and_then(|r| resolve::MediaRef::from_str(r).ok());

    let ctx = resolve::ResolveContext {
        index: &index,
        providers: &providers,
        store: &store,
        user_id,
        now: time::OffsetDateTime::now_utc(),
    };

    match resolve::resolve_media(&ctx, &request).await {
        Ok(found) => {
            let detail = match found.resolution {
                resolve::Resolution::Single => found
                    .candidates
                    .first()
                    .map(|c| c.title.clone())
                    .unwrap_or_else(|| query.to_string()),
                resolve::Resolution::Ambiguous => {
                    format!("{} possibilities", found.candidates.len())
                }
                resolve::Resolution::NeedsContext => "needs more to go on".to_string(),
                resolve::Resolution::NotFound => format!("no match for {query}"),
            };
            let sources = found
                .candidates
                .iter()
                .filter(|c| c.in_library)
                .filter_map(candidate_source)
                .collect();
            let mut observation =
                serde_json::to_value(&found).unwrap_or_else(|_| json!({"ok": false}));
            if let Some(obj) = observation.as_object_mut() {
                obj.insert("ok".to_string(), json!(true));
            }
            ToolOutcome::ok(observation, detail).with_sources(sources)
        }
        Err(e) => ToolOutcome::failed(format!("could not work out which one that is: {e}")),
    }
}

fn candidate_source(candidate: &resolve::Candidate) -> Option<Source> {
    let item_type = match candidate.kind() {
        resolve::MediaKind::Book => "book",
        resolve::MediaKind::Movie => "movie",
        resolve::MediaKind::Show => "tv_show",
        resolve::MediaKind::Season => "tv_season",
        resolve::MediaKind::Episode => "tv_episode",
        resolve::MediaKind::Author => "author",
        resolve::MediaKind::Series => "series",
        // Sports fixtures ride the TV rails, so they navigate as episodes.
        resolve::MediaKind::SportsEvent => "tv_episode",
    };
    let id = candidate.media_ref.local_id()?.to_string();
    Some(Source {
        url: crate::chatbot::events::source_url(item_type, &id),
        item_type: item_type.to_string(),
        id,
        title: candidate.title.clone(),
        cover_url: None,
        year: candidate.year.map(i64::from),
        description: candidate
            .subtitle
            .clone()
            .or_else(|| candidate.description.clone()),
    })
}

/// The ref a previous `resolve_media` settled on, for the next turn's context.
/// Only a confident single answer becomes a referent — carrying an ambiguous
/// list forward would make "the other one" mean whatever happened to rank first.
pub fn context_ref_from(observation: &Value) -> Option<String> {
    if observation.get("resolution").and_then(Value::as_str) != Some("single") {
        return None;
    }
    observation
        .pointer("/candidates/0/ref")
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub async fn library_stats(state: &AppState) -> ToolOutcome {
    match chat_tools::library_stats(&state.pool).await {
        Ok(s) => ToolOutcome::ok(
            json!({
                "ok": true,
                "books": s.books,
                "audiobooks": s.audiobooks,
                "authors": s.authors,
                "book_series": s.series,
                "movies": s.movies,
                "tv_shows": s.tv_shows,
                "sports": s.sports,
                "episodes_available": s.episodes_available,
                "wanted": {
                    "books": s.books_wanted,
                    "movies": s.movies_wanted,
                    "episodes": s.episodes_wanted,
                },
                "note": "Counts are for the whole library. Nothing here is per-person — reading and watch progress are not counted.",
            }),
            format!(
                "{} books, {} movies, {} shows",
                s.books, s.movies, s.tv_shows
            ),
        ),
        Err(e) => ToolOutcome::failed(format!("could not read the library counts: {e}")),
    }
}

pub async fn find_passage(state: &AppState, args: &Value) -> ToolOutcome {
    let Some(query) = arg_str(args, "query") else {
        return ToolOutcome::failed("find_passage needs a query");
    };
    let book_id = arg_str(args, "book_id").and_then(|s| Uuid::parse_str(s).ok());
    let want = limit(args, 5, MAX_PASSAGES);

    match state
        .search
        .search_transcript_segments(query, book_id, want)
        .await
    {
        Ok(segments) => {
            let passages: Vec<Value> = segments
                .iter()
                .map(|s| {
                    json!({
                        "book_id": s.book_id,
                        "book_title": s.book_title,
                        "chapter": s.chapter_title,
                        "source": s.source_type,
                        "start_seconds": s.start_seconds,
                        "cfi": s.cfi,
                        "text": s.text,
                    })
                })
                .collect();
            let detail = format!(
                "{} passage{}",
                passages.len(),
                if passages.len() == 1 { "" } else { "s" }
            );
            ToolOutcome::ok(
                json!({"ok": true, "count": passages.len(), "passages": passages}),
                detail,
            )
        }
        Err(e) => ToolOutcome::failed(format!("could not search the transcripts: {e}")),
    }
}

/// Download state lives in kobload, not in postgres, so this is an HTTP read
/// rather than a query. Kept deliberately thin: the model wants a name, a
/// status and a percentage, not kobload's release payload.
pub async fn whats_downloading(state: &AppState, args: &Value) -> ToolOutcome {
    let want = limit(args, 10, MAX_DOWNLOADS);
    let config = state.runtime_config.get().await;
    let (Some(base), Some(key)) = (
        config.kobload_base_url.as_deref().filter(|s| !s.is_empty()),
        config.kobload_api_key.as_deref().filter(|s| !s.is_empty()),
    ) else {
        return ToolOutcome::failed(
            "downloads are not available — kobload is not configured on this server",
        );
    };

    let url = format!(
        "{}/api/v1/downloads?limit={want}&status={}",
        base.trim_end_matches('/'),
        "pending,queued,searching,downloading,processing"
    );
    let response = match crate::http::new_client()
        .get(&url)
        .bearer_auth(key)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return ToolOutcome::failed(format!("kobload is unreachable: {e}")),
    };
    if !response.status().is_success() {
        return ToolOutcome::failed(format!("kobload returned {}", response.status().as_u16()));
    }
    let body: Value = match response.json().await {
        Ok(v) => v,
        Err(e) => return ToolOutcome::failed(format!("kobload sent something unreadable: {e}")),
    };

    // The raw kobload name is a release string ("Some.Show.S24E07.1080p.WEB-DL...").
    // Reusing the downloads page's enrichment gives the library title instead, which
    // is the only form worth reading aloud in a chat message.
    let enriched = crate::routes::admin::downloads::enrich_downloads(state, body).await;
    let items: Vec<Value> = enriched
        .items
        .iter()
        .take(want)
        .map(|d| {
            let title = d
                .pointer("/media/title")
                .and_then(Value::as_str)
                .or_else(|| d.get("name").and_then(Value::as_str))
                .unwrap_or("unknown");
            json!({
                "title": title,
                "release": d.get("name").and_then(Value::as_str),
                "status": d.get("status").and_then(Value::as_str).unwrap_or("unknown"),
                "percent": d.get("progress").and_then(Value::as_f64),
                "eta": d.pointer("/client_progress/eta_time"),
            })
        })
        .collect();

    let detail = match items.len() {
        0 => "nothing downloading".to_string(),
        n => format!("{n} downloading"),
    };
    ToolOutcome::ok(
        json!({"ok": true, "count": items.len(), "downloads": items}),
        detail,
    )
}

pub async fn recently_added(state: &AppState, args: &Value) -> ToolOutcome {
    let want = limit(args, 10, MAX_RECENT);
    let media_type = arg_str(args, "media_type").unwrap_or("any");
    let mut items: Vec<(time::OffsetDateTime, Value, Option<Source>)> = Vec::new();

    if matches!(media_type, "any" | "book" | "audiobook") {
        // An audiobook is dated from its own file: one added to a book shelved years ago
        // is new to the listener and would never surface on the book's own age.
        let audiobooks_only = media_type == "audiobook";
        let label = if audiobooks_only { "audiobook" } else { "book" };
        match dashboard::recent_books(&state.pool, audiobooks_only).await {
            Ok(rows) => items.extend(rows.into_iter().map(|r| {
                let authors = author_names(r.authors.as_ref());
                (
                    r.added_at,
                    json!({"type": label, "id": r.id, "title": r.title, "authors": authors}),
                    Some(Source {
                        item_type: "book".to_string(),
                        url: crate::chatbot::events::source_url("book", &r.id.to_string()),
                        id: r.id.to_string(),
                        title: r.title,
                        cover_url: None,
                        year: None,
                        description: None,
                    }),
                )
            })),
            Err(e) => return ToolOutcome::failed(format!("could not read recent {label}s: {e}")),
        }
    }
    if matches!(media_type, "any" | "movie") {
        match dashboard::recent_movies(&state.pool).await {
            Ok(rows) => items.extend(rows.into_iter().map(|r| {
                (
                    r.added_at,
                    json!({"type": "movie", "id": r.id, "title": r.title, "year": r.year}),
                    Some(Source {
                        item_type: "movie".to_string(),
                        url: crate::chatbot::events::source_url("movie", &r.id.to_string()),
                        id: r.id.to_string(),
                        title: r.title,
                        cover_url: None,
                        year: r.year.map(i64::from),
                        description: None,
                    }),
                )
            })),
            Err(e) => return ToolOutcome::failed(format!("could not read recent movies: {e}")),
        }
    }
    for category in ["tv", "sport"] {
        let wanted = match media_type {
            "any" => true,
            "episode" => category == "tv",
            "sport" => category == "sport",
            _ => false,
        };
        if !wanted {
            continue;
        }
        match dashboard::recent_episodes(&state.pool, category).await {
            Ok(rows) => items.extend(rows.into_iter().map(|r| {
                let label = format!("S{}E{}", r.season_identifier, r.episode_identifier);
                (
                    r.added_at,
                    json!({
                        "type": if category == "sport" { "sport" } else { "episode" },
                        "id": r.episode_id,
                        "show": r.show_title,
                        "episode": label,
                        "title": r.episode_title,
                    }),
                    Some(Source {
                        item_type: "tv_episode".to_string(),
                        url: crate::chatbot::events::source_url(
                            "tv_episode",
                            &r.episode_id.to_string(),
                        ),
                        id: r.episode_id.to_string(),
                        title: format!("{} {label}", r.show_title),
                        cover_url: None,
                        year: None,
                        description: None,
                    }),
                )
            })),
            Err(e) => return ToolOutcome::failed(format!("could not read recent episodes: {e}")),
        }
    }

    items.sort_by_key(|(added_at, _, _)| std::cmp::Reverse(*added_at));
    items.truncate(want);
    let sources: Vec<Source> = items.iter().filter_map(|(_, _, s)| s.clone()).collect();
    let payload: Vec<Value> = items.into_iter().map(|(_, v, _)| v).collect();
    let detail = format!("{} recently added", payload.len());
    ToolOutcome::ok(
        json!({"ok": true, "count": payload.len(), "items": payload}),
        detail,
    )
    .with_sources(sources)
}

fn author_names(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.get("name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The air-date honesty caveat is returned as data, not left for the model to
/// remember. TMDB dates are wrong often enough to matter — a full-season binge
/// drop breaks the weekly assumption entirely — and there is no airs_time
/// anywhere in the schema, so the honest granularity is a day, never a time.
pub async fn next_airing(state: &AppState, args: &Value) -> ToolOutcome {
    let Some(title) = arg_str(args, "title") else {
        return ToolOutcome::failed("next_airing needs a title");
    };

    let candidates = match chat_tools::find_shows(&state.pool, title, 5).await {
        Ok(c) => c,
        Err(e) => return ToolOutcome::failed(format!("could not look that show up: {e}")),
    };
    let Some(first) = candidates.first() else {
        return ToolOutcome::ok(
            json!({
                "ok": true,
                "found": false,
                "note": format!("Nothing in the library matches '{title}'."),
            }),
            format!("no show matching {title}"),
        );
    };
    if candidates.len() > 1 && !candidates[1..].iter().any(|c| c.id == first.id) {
        let exact = candidates
            .iter()
            .filter(|c| c.title.eq_ignore_ascii_case(title))
            .count();
        if exact != 1 {
            let options: Vec<Value> = candidates
                .iter()
                .map(|c| json!({"id": c.id, "title": c.title, "year": c.year}))
                .collect();
            return ToolOutcome::ok(
                json!({
                    "ok": true,
                    "found": false,
                    "ambiguous": true,
                    "candidates": options,
                    "note": "Several shows match. Ask which one they mean; do not pick.",
                }),
                format!("{} shows match {title}", candidates.len()),
            );
        }
    }

    let show = candidates
        .iter()
        .find(|c| c.title.eq_ignore_ascii_case(title))
        .unwrap_or(first);

    let next = match chat_tools::next_airing(&state.pool, show.id).await {
        Ok(n) => n,
        Err(e) => return ToolOutcome::failed(format!("could not read the schedule: {e}")),
    };
    let caveat = "Air dates come from TMDB and are often a day out; whole-season drops break \
                  them entirely. There is no broadcast time in the data, so say the day, never \
                  a time.";

    if let Some(e) = next {
        let label = format!("S{}E{}", e.season_identifier, e.episode_identifier);
        return ToolOutcome::ok(
            json!({
                "ok": true,
                "found": true,
                "show": e.show_title,
                "kind": e.content_category,
                "episode": label,
                "episode_title": e.title,
                "air_date": e.air_date.to_string(),
                "date_confidence": "approximate",
                "caveat": caveat,
            }),
            format!("{} {label} on {}", e.show_title, e.air_date),
        );
    }

    let last = chat_tools::last_aired(&state.pool, show.id)
        .await
        .ok()
        .flatten();
    ToolOutcome::ok(
        json!({
            "ok": true,
            "found": false,
            "show": show.title,
            "status": show.status,
            "last_aired": last.as_ref().map(|e| json!({
                "episode": format!("S{}E{}", e.season_identifier, e.episode_identifier),
                "air_date": e.air_date.to_string(),
            })),
            "note": "Nothing is scheduled. Say so plainly rather than guessing a date.",
            "caveat": caveat,
        }),
        format!("nothing scheduled for {}", show.title),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The model was shown "Ted Lasso S04E09 · 2026-09-29" alongside an episode
    /// from 2023 and called the September one the latest. The date was already
    /// in front of it; what it lacked was the comparison.
    #[test]
    fn an_episode_that_has_not_aired_yet_is_marked_as_such() {
        assert_eq!(has_aired(Some(&json!("1999-01-01"))), Some(true));
        assert_eq!(has_aired(Some(&json!("2999-01-01"))), Some(false));
        assert_eq!(has_aired(Some(&json!(null))), None);
        assert_eq!(has_aired(None), None);

        let today = OffsetDateTime::now_utc().date();
        let today = format!(
            "{:04}-{:02}-{:02}",
            today.year(),
            u8::from(today.month()),
            today.day()
        );
        assert_eq!(
            has_aired(Some(&json!(today))),
            Some(true),
            "an episode airing today has aired"
        );
    }

    /// Both sort orders are filterable on the episodes index, so a named season
    /// narrows the result set instead of fuzzy-matching a padded code against
    /// episode titles and returning a scatter across three seasons.
    #[test]
    fn a_named_season_becomes_a_filter_on_the_episodes_index() {
        assert_eq!(
            episode_filter(Some(24), None).as_deref(),
            Some("season_sort_order = 24")
        );
        assert_eq!(
            episode_filter(Some(2), Some(5)).as_deref(),
            Some("season_sort_order = 2 AND episode_sort_order = 5")
        );
        assert_eq!(
            episode_filter(None, Some(5)).as_deref(),
            Some("episode_sort_order = 5")
        );
        assert_eq!(episode_filter(None, None), None);
    }

    #[test]
    fn any_media_type_fans_out_across_every_content_index() {
        assert_eq!(indexes_for(None).len(), 8);
        assert_eq!(indexes_for(Some("movie")), vec![indexes::MOVIES]);
    }

    /// A poster URL in an observation is prompt budget spent on a string the
    /// model cannot use; the citation carries the image instead.
    #[test]
    fn retrieved_attributes_leave_out_the_fields_a_prompt_cannot_use() {
        for index in indexes_for(None) {
            let attributes = attributes_for(index);
            assert!(attributes.contains(&"id"), "{index}");
            assert!(
                !attributes.contains(&"first_letter"),
                "{index} ships a facet into the prompt"
            );
        }
        assert!(!attributes_for(indexes::TV_EPISODES).contains(&"poster_url"));
    }

    #[test]
    fn limits_are_clamped_rather_than_trusted() {
        assert_eq!(limit(&json!({}), 10, 40), 10);
        assert_eq!(limit(&json!({"limit": 500}), 10, 40), 40);
        assert_eq!(limit(&json!({"limit": 0}), 10, 40), 1);
        assert_eq!(limit(&json!({"limit": "7"}), 10, 40), 7);
    }

    #[test]
    fn blank_string_arguments_are_treated_as_absent() {
        assert_eq!(arg_str(&json!({"query": "   "}), "query"), None);
        assert_eq!(
            arg_str(&json!({"query": "carrie"}), "query"),
            Some("carrie")
        );
    }
}
