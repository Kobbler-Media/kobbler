//! `recommend` and `whats_new`.
//!
//! `recommend` is the vibe question — "I feel like a horror film like Carrie" —
//! and the important thing about it is what it is *not*. There is no embedder
//! and no vector store: transcript embeddings went in at search schema v15 and
//! came out at v23 on RAM cost, and the model already is the embedding. Gemma
//! knows what Carrie is. So the shape is:
//!
//! 1. the model turns the vibe into filters it can already express;
//! 2. Meili returns 20-40 candidates **with the descriptions already on the
//!    documents** (schema v33 — nothing here adds a field or bumps the version);
//! 3. the model reasons over that shortlist and picks three.
//!
//! Two things make it materially better and both already exist: the caller's
//! watch state and their Tier 3 facts. Watch state is per-user and deliberately
//! *not* in the index — `MovieDocument.watched` was removed at v32 because it
//! read an arbitrary user's row — so candidates come back from Meili and the
//! seen ones are subtracted in Rust, scoped by `user_id`. Passing a second
//! viewer turns it into "what should *we* watch", which is the query nothing
//! off-the-shelf can answer.

use std::collections::HashSet;

use serde_json::{Value, json};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::AppState;
use crate::chatbot::events::source_from_doc;
use crate::repositories::chat_memory;
use crate::routes::admin::calendar;
use crate::search::indexes;

use super::read::ToolOutcome;

/// Candidates handed to the model. The design's own number: a few hundred
/// tokens of synopsis, well inside budget for a library this size.
const CANDIDATE_POOL: usize = 40;
/// Per index, before the watched set is subtracted — over-fetching a little is
/// what keeps a household that has seen most of its films from getting three
/// candidates back.
const PER_INDEX_FETCH: usize = 40;
/// How many the model is asked to pick.
const PICK: usize = 3;

const WHATS_NEW_CAVEAT: &str = "Air dates come from TMDB and are unreliable for streaming drops — a whole season can land at once, or a date can move. Say what the provider claims rather than promising it. Only episodes with an air_time have a known time; for the rest name the day, never an hour.";

// =============================================================================
// recommend
// =============================================================================

/// `recommend` — a vibe, turned into filters, turned into a shortlist.
///
/// `viewers` is everyone whose watch state should be subtracted: the caller,
/// plus anyone named in `also_for`.
pub async fn recommend(state: &AppState, viewers: &[Uuid], args: &Value) -> ToolOutcome {
    let media_type = arg_str(args, "media_type")
        .filter(|m| *m != "any")
        .map(str::to_string);
    let genre = arg_str(args, "genre").map(str::to_string);
    let years = arg_str(args, "years").and_then(parse_years);
    let include_watched = args
        .get("include_watched")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut viewers: Vec<Uuid> = viewers.to_vec();
    let mut also_for_note: Option<String> = None;
    if let Some(name) = arg_str(args, "also_for") {
        match chat_memory::find_user_by_name(&state.pool, name).await {
            Ok(Some(id)) => {
                if !viewers.contains(&id) {
                    viewers.push(id);
                }
            }
            Ok(None) => {
                also_for_note = Some(format!(
                    "there is no account here called {name}, so only your own watch history was used"
                ));
            }
            Err(e) => return ToolOutcome::failed(format!("could not look up {name}: {e}")),
        }
    }

    let targets = target_indexes(media_type.as_deref());
    let mut queries = Vec::new();
    for index in &targets {
        queries.push(candidate_query(index, genre.as_deref(), years, false));
    }
    let mut results = match multi_search(state, &json!({"queries": queries})).await {
        Ok(v) => v,
        Err(e) => return ToolOutcome::failed(e),
    };

    // A genre the library does not use as a facet is not an empty library, so
    // fall back to matching it as text before telling anyone there is nothing.
    let mut relaxed = false;
    if total_hits(&results) == 0 && genre.is_some() {
        let retry: Vec<Value> = targets
            .iter()
            .map(|index| candidate_query(index, genre.as_deref(), years, true))
            .collect();
        if let Ok(v) = multi_search(state, &json!({"queries": retry})).await
            && total_hits(&v) > 0
        {
            results = v;
            relaxed = true;
        }
    }

    let found = flatten(&results);
    if found.is_empty() {
        return ToolOutcome::ok(
            json!({
                "ok": true,
                "count": 0,
                "filters": describe_filters(&media_type, &genre, years),
                "candidates": [],
                "note": "nothing in the library matches those filters — say so and offer to loosen one",
            }),
            "nothing matched",
        );
    }

    let seen = match include_watched {
        true => HashSet::new(),
        false => match watched_ids(state, &viewers, &targets).await {
            Ok(s) => s,
            Err(e) => return ToolOutcome::failed(e),
        },
    };

    let total_found = found.len();
    let mut candidates: Vec<Value> = found
        .into_iter()
        .filter(|doc| {
            doc.get("id")
                .and_then(Value::as_str)
                .is_none_or(|id| !seen.contains(id))
        })
        .collect();
    candidates.truncate(CANDIDATE_POOL);

    // Everything that matched has already been watched. That is a real answer
    // and a much better one than quietly recommending a repeat.
    if candidates.is_empty() {
        return ToolOutcome::ok(
            json!({
                "ok": true,
                "count": 0,
                "all_seen": true,
                "matched_before_filtering": total_found,
                "filters": describe_filters(&media_type, &genre, years),
                "candidates": [],
                "note": "every match has already been watched — say so plainly, and offer either a rewatch or a different vibe",
            }),
            format!("all {total_found} matches already watched"),
        );
    }

    let sources = candidates.iter().filter_map(source_from_doc).collect();
    let detail = format!("{} to choose from", candidates.len());
    let mut observation = json!({
        "ok": true,
        "count": candidates.len(),
        "pick": PICK,
        "watch_state_applied": !include_watched,
        "viewers": viewers.len(),
        "filters": describe_filters(&media_type, &genre, years),
        "candidates": candidates,
        "note": "pick three from these and give one sentence each on why; do not invent titles that are not in this list",
    });
    if relaxed {
        observation["genre_matched_as_text"] = json!(true);
    }
    if let Some(note) = also_for_note {
        observation["also_for_note"] = json!(note);
    }
    ToolOutcome::ok(observation, detail).with_sources(sources)
}

fn target_indexes(media_type: Option<&str>) -> Vec<&'static str> {
    match media_type {
        Some("movie") => vec![indexes::MOVIES],
        Some("show") => vec![indexes::TV_SHOWS],
        Some("book") => vec![indexes::BOOKS],
        _ => vec![indexes::MOVIES, indexes::TV_SHOWS],
    }
}

/// One index's slice of the shortlist.
///
/// `q` is empty in the normal case: this is a filtered browse, not a search, and
/// a text query would rank on title words nobody typed. Sorting by rating gives
/// "something well-reviewed" for free on the two indexes that can sort on it,
/// and every query ends on `id:asc` so the shortlist boundary is reproducible.
fn candidate_query(
    index: &str,
    genre: Option<&str>,
    years: Option<(Option<i64>, Option<i64>)>,
    genre_as_text: bool,
) -> Value {
    let mut filters: Vec<String> = Vec::new();
    match index {
        indexes::MOVIES => filters.push("is_in_library = true".to_string()),
        indexes::TV_SHOWS => filters.push("available_episode_count > 0".to_string()),
        indexes::BOOKS => filters.push("is_in_library = true".to_string()),
        _ => {}
    }
    if let Some(g) = genre
        && !genre_as_text
    {
        filters.push(format!("genres = \"{}\"", g.replace('"', "")));
    }
    // `year` is filterable on movies and shows only; books do not carry one.
    if let Some((from, to)) = years
        && index != indexes::BOOKS
    {
        if let Some(from) = from {
            filters.push(format!("year >= {from}"));
        }
        if let Some(to) = to {
            filters.push(format!("year <= {to}"));
        }
    }

    let mut query = json!({
        "indexUid": index,
        "q": match genre_as_text { true => genre.unwrap_or_default(), false => "" },
        "limit": PER_INDEX_FETCH,
        "filter": filters.join(" AND "),
        "attributesToRetrieve": candidate_attributes(index),
    });
    // `id` is the total-order tie-break: equal ratings would otherwise fall back
    // to Meili's internal document order, which is not stable across a reindex.
    match index {
        indexes::MOVIES | indexes::TV_SHOWS => {
            query["sort"] = json!(["rating:desc", "id:asc"]);
        }
        indexes::BOOKS => query["sort"] = json!(["id:asc"]),
        _ => {}
    }
    query
}

/// What the model needs to choose, and nothing else. `description` is the one
/// that matters — it is why this works without an embedder — and it is already
/// on the document, truncated, at schema v33.
fn candidate_attributes(index: &str) -> Vec<&'static str> {
    match index {
        indexes::MOVIES => vec![
            "id",
            "title",
            "year",
            "genres",
            "runtime",
            "content_rating",
            "rating",
            "directors",
            "description",
            "poster_url",
        ],
        indexes::TV_SHOWS => vec![
            "id",
            "title",
            "year",
            "genres",
            "network",
            "content_rating",
            "rating",
            "available_episode_count",
            "description",
            "poster_url",
        ],
        indexes::BOOKS => vec![
            "id",
            "title",
            "authors",
            "series_name",
            "genres",
            "has_audiobook",
            "description",
            "cover_url",
        ],
        _ => vec!["id", "title", "description"],
    }
}

fn describe_filters(
    media_type: &Option<String>,
    genre: &Option<String>,
    years: Option<(Option<i64>, Option<i64>)>,
) -> Value {
    let mut out = json!({"media_type": media_type.clone().unwrap_or_else(|| "any".to_string())});
    if let Some(g) = genre {
        out["genre"] = json!(g);
    }
    if let Some((from, to)) = years {
        if let Some(from) = from {
            out["year_from"] = json!(from);
        }
        if let Some(to) = to {
            out["year_to"] = json!(to);
        }
    }
    out
}

/// "1970-1985", "1970s", "the 70s", "since 2020", "before 1990", "2019".
///
/// Tolerant because the model writes this argument from someone's phrasing, and
/// a rejected range silently drops the constraint that made the answer good.
pub fn parse_years(raw: &str) -> Option<(Option<i64>, Option<i64>)> {
    let text = raw.trim().to_lowercase();
    let numbers: Vec<i64> = text
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<i64>().ok())
        .collect();

    let decade = |n: i64| match n {
        0..=99 => Some(1900 + n),
        1000..=2999 => Some(n),
        _ => None,
    };

    if text.contains('s') && numbers.len() == 1 && text.ends_with('s') {
        let start = decade(numbers[0])?;
        return Some((Some(start), Some(start + 9)));
    }
    match numbers.as_slice() {
        [only] => {
            let year = decade(*only)?;
            if text.contains("since") || text.contains("after") || text.contains("from") {
                return Some((Some(year), None));
            }
            if text.contains("before") || text.contains("until") || text.contains("up to") {
                return Some((None, Some(year)));
            }
            Some((Some(year), Some(year)))
        }
        [from, to, ..] => Some((decade(*from), decade(*to))),
        [] => None,
    }
}

/// Everything `viewers` have already seen, as document ids.
///
/// Per media kind and scoped by `user_id` in SQL. This is the half of the answer
/// the search index cannot hold.
async fn watched_ids(
    state: &AppState,
    viewers: &[Uuid],
    targets: &[&str],
) -> Result<HashSet<String>, String> {
    let mut seen = HashSet::new();
    if viewers.is_empty() {
        return Ok(seen);
    }
    for index in targets {
        let ids = match *index {
            indexes::MOVIES => chat_memory::watched_movie_ids(&state.pool, viewers).await,
            indexes::TV_SHOWS => chat_memory::watched_show_ids(&state.pool, viewers).await,
            indexes::BOOKS => chat_memory::read_book_ids(&state.pool, viewers).await,
            _ => Ok(Vec::new()),
        }
        .map_err(|e| format!("could not read watch state: {e}"))?;
        seen.extend(ids.into_iter().map(|id| id.to_string()));
    }
    Ok(seen)
}

// =============================================================================
// whats_new
// =============================================================================

/// `whats_new` — the calendar, both directions, on demand.
///
/// Pull, not push. No cron and no digest: nothing about this arrives uninvited.
pub async fn whats_new(state: &AppState, args: &Value) -> ToolOutcome {
    let window = arg_str(args, "window").unwrap_or("past_week");
    let today = OffsetDateTime::now_utc().date();
    let (from, to) = match window {
        "coming_week" => (today, today + Duration::days(8)),
        "past_week" => (today - Duration::days(7), today),
        other => return ToolOutcome::failed(format!("{other} is not a window I know")),
    };

    let items = match calendar::items_between(state, from, to).await {
        Ok(items) => items,
        Err(e) => return ToolOutcome::failed(format!("the calendar query failed: {e}")),
    };

    let episode_ids: Vec<Uuid> = items
        .iter()
        .filter(|i| i.media_type == "tv" || i.media_type == "sport")
        .map(|i| i.id)
        .collect();
    let air_times = match chat_memory::episode_air_times(&state.pool, &episode_ids).await {
        Ok(rows) => rows,
        Err(e) => return ToolOutcome::failed(format!("the schedule query failed: {e}")),
    };

    let entries: Vec<Value> = items
        .iter()
        .map(|item| {
            let mut entry = json!({
                "id": item.id,
                "type": item.media_type,
                "title": item.title,
                "date": item.date.to_string(),
            });
            if let Some(subtitle) = &item.subtitle {
                entry["subtitle"] = json!(subtitle);
            }
            if let Some((_, at)) = air_times.iter().find(|(id, _)| *id == item.id) {
                entry["air_time"] = json!(at.format(&Rfc3339).unwrap_or_default());
            }
            entry
        })
        .collect();

    let detail = format!(
        "{} item{} {}",
        entries.len(),
        if entries.len() == 1 { "" } else { "s" },
        match window {
            "coming_week" => "coming up",
            _ => "this past week",
        }
    );
    ToolOutcome::ok(
        json!({
            "ok": true,
            "window": window,
            "from": from.to_string(),
            "to": to.to_string(),
            "count": entries.len(),
            "items": entries,
            "date_confidence": "approximate",
            "caveat": WHATS_NEW_CAVEAT,
        }),
        detail,
    )
}

// =============================================================================
// Shared plumbing
// =============================================================================

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn total_hits(results: &[Value]) -> usize {
    results
        .iter()
        .filter_map(|r| r.get("hits").and_then(Value::as_array))
        .map(Vec::len)
        .sum()
}

/// Interleave the indexes rather than concatenating them: a shortlist that is
/// forty films and no shows is not the shortlist "something to watch" wanted.
fn flatten(results: &[Value]) -> Vec<Value> {
    let per_index: Vec<Vec<Value>> = results
        .iter()
        .map(|result| {
            let kind = source_kind(
                result
                    .get("indexUid")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            result
                .get("hits")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|mut hit| {
                    if let Some(obj) = hit.as_object_mut() {
                        obj.insert("type".to_string(), json!(kind));
                    }
                    hit
                })
                .collect()
        })
        .collect();

    let longest = per_index.iter().map(Vec::len).max().unwrap_or(0);
    let mut out = Vec::new();
    for n in 0..longest {
        for hits in &per_index {
            if let Some(hit) = hits.get(n) {
                out.push(hit.clone());
            }
        }
    }
    out
}

fn source_kind(index: &str) -> &'static str {
    match index {
        indexes::MOVIES => "movie",
        indexes::TV_SHOWS => "tv_show",
        indexes::BOOKS => "book",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vibe_becomes_a_filter_the_index_can_already_answer() {
        let query = candidate_query(
            indexes::MOVIES,
            Some("Horror"),
            Some((Some(1970), Some(1985))),
            false,
        );
        let filter = query["filter"].as_str().unwrap();
        assert!(filter.contains("genres = \"Horror\""), "{filter}");
        assert!(filter.contains("year >= 1970"), "{filter}");
        assert!(filter.contains("year <= 1985"), "{filter}");
        assert!(filter.contains("is_in_library = true"), "{filter}");
        assert_eq!(query["sort"], json!(["rating:desc", "id:asc"]));
        assert_eq!(query["q"], "");
    }

    /// The whole point: descriptions come off the document, so there is no
    /// second lookup and no embedder.
    #[test]
    fn candidates_carry_the_synopsis_the_model_reasons_over() {
        for index in [indexes::MOVIES, indexes::TV_SHOWS, indexes::BOOKS] {
            assert!(
                candidate_attributes(index).contains(&"description"),
                "{index}"
            );
        }
    }

    #[test]
    fn books_get_no_year_filter_because_the_index_has_none() {
        let query = candidate_query(
            indexes::BOOKS,
            Some("Horror"),
            Some((Some(1970), None)),
            false,
        );
        let filter = query["filter"].as_str().unwrap();
        assert!(!filter.contains("year"), "{filter}");
        // Books have no rating to lead on, so `id` is the whole sort — without it
        // the shortlist was Meili's internal document order, which a reindex changes.
        assert_eq!(query["sort"], json!(["id:asc"]));
    }

    #[test]
    fn year_phrasings_become_ranges() {
        let cases = [
            ("1970-1985", (Some(1970), Some(1985))),
            ("1970..1985", (Some(1970), Some(1985))),
            ("1970s", (Some(1970), Some(1979))),
            ("the 70s", (Some(1970), Some(1979))),
            ("since 2020", (Some(2020), None)),
            ("before 1990", (None, Some(1990))),
            ("2019", (Some(2019), Some(2019))),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_years(input), Some(expected), "{input}");
        }
        assert_eq!(parse_years("recently"), None);
    }

    #[test]
    fn the_shortlist_interleaves_the_indexes() {
        let results = vec![
            json!({"indexUid": indexes::MOVIES, "hits": [{"id": "m1", "title": "M1"}, {"id": "m2", "title": "M2"}]}),
            json!({"indexUid": indexes::TV_SHOWS, "hits": [{"id": "s1", "title": "S1"}]}),
        ];
        let flat = flatten(&results);
        let ids: Vec<&str> = flat.iter().map(|d| d["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["m1", "s1", "m2"]);
        assert_eq!(flat[0]["type"], "movie");
        assert_eq!(flat[1]["type"], "tv_show");
    }
}
