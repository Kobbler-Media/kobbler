//! Who is in what — the edge between a person and a title.
//!
//! Read-only, and postgres rather than Meili: the movies projection carries
//! directors and the people projection carries counts, so before these two the
//! model could find Ashton Kutcher, find a film, and never join them. Adding
//! cast to the index would be a schema bump and a reindex to answer a question
//! a join already answers.
//!
//! Both tools take a plain name or title, like `next_airing` does, not a ref
//! from `resolve_media` — the act tools need a ref because they change
//! something and the confirmation has to name it exactly; a read does not, and
//! a two-call chain is a round trip this model spends its step budget on.

use serde_json::{Value, json};

use crate::AppState;
use crate::chatbot::events::{Source, source_url};
use crate::repositories::chat_credits::{self, TitleKind};

use super::read::ToolOutcome;

const MAX_CREDITS: usize = 40;
const MAX_CAST: usize = 40;
const MAX_CANDIDATES: i64 = 5;

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn limit(args: &Value, default: usize, max: usize) -> usize {
    args.get("limit")
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        })
        .map(|n| n as usize)
        .unwrap_or(default)
        .clamp(1, max)
}

fn kind_arg(args: &Value) -> Option<TitleKind> {
    match arg_str(args, "media_type") {
        Some("movie") => Some(TitleKind::Movie),
        Some("show") => Some(TitleKind::Show),
        _ => None,
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    match n {
        1 => format!("1 {one}"),
        _ => format!("{n} {many}"),
    }
}

/// What the library holds crediting one person.
///
/// The held/not-held split is the whole point. Rows exist in `movies` and
/// `tv_shows` for things nobody ever downloaded — want-listed, or pulled in by
/// enrichment as somebody's filmography — and reading those back as "you have
/// them" is the failure this tool exists to avoid, so held titles are listed
/// and the rest are only ever a count.
pub async fn person_credits(state: &AppState, args: &Value) -> ToolOutcome {
    let Some(name) = arg_str(args, "person") else {
        return ToolOutcome::failed("person_credits needs a person");
    };
    let want = limit(args, 15, MAX_CREDITS);
    let kind = kind_arg(args);

    let people = match chat_credits::find_people(&state.pool, name, MAX_CANDIDATES).await {
        Ok(p) => p,
        Err(e) => return ToolOutcome::failed(format!("could not look that person up: {e}")),
    };
    let Some(first) = people.first() else {
        return ToolOutcome::ok(
            json!({
                "ok": true,
                "found": false,
                "person": name,
                "note": format!("Nobody in the library is called '{name}'. Say so plainly — do not guess a filmography from memory."),
            }),
            format!("no one called {name}"),
        );
    };

    let exact: Vec<_> = people
        .iter()
        .filter(|p| p.name.eq_ignore_ascii_case(name))
        .collect();
    if exact.len() != 1 && people.len() > 1 {
        let options: Vec<Value> = people
            .iter()
            .map(|p| json!({"name": p.name, "credits": p.credits}))
            .collect();
        return ToolOutcome::ok(
            json!({
                "ok": true,
                "found": false,
                "ambiguous": true,
                "candidates": options,
                "note": "Several people match. Ask which one they mean; do not pick.",
            }),
            format!("{} people match {name}", people.len()),
        );
    }
    let person = exact.first().copied().unwrap_or(first);

    let movies = match kind {
        Some(TitleKind::Show) => Vec::new(),
        _ => match chat_credits::movie_credits(&state.pool, person.id).await {
            Ok(m) => m,
            Err(e) => return ToolOutcome::failed(format!("could not read film credits: {e}")),
        },
    };
    let shows = match kind {
        Some(TitleKind::Movie) => Vec::new(),
        _ => match chat_credits::show_credits(&state.pool, person.id).await {
            Ok(s) => s,
            Err(e) => return ToolOutcome::failed(format!("could not read TV credits: {e}")),
        },
    };

    let held_movies: Vec<_> = movies.iter().filter(|m| m.held).collect();
    let held_shows: Vec<_> = shows.iter().filter(|s| s.held()).collect();
    let not_held_movies = movies.len() - held_movies.len();
    let not_held_shows = shows.len() - held_shows.len();

    let mut sources: Vec<Source> = Vec::new();
    let movie_items: Vec<Value> = held_movies
        .iter()
        .take(want)
        .map(|m| {
            sources.push(Source {
                url: source_url("movie", &m.id.to_string()),
                item_type: "movie".to_string(),
                id: m.id.to_string(),
                title: m.title.clone(),
                cover_url: None,
                year: m.year.map(i64::from),
                description: None,
            });
            json!({
                "id": m.id,
                "title": m.title,
                "year": m.year,
                "character": m.character,
                "job": m.job,
            })
        })
        .collect();
    let show_items: Vec<Value> = held_shows
        .iter()
        .take(want)
        .map(|s| {
            sources.push(Source {
                url: source_url("tv_show", &s.id.to_string()),
                item_type: "tv_show".to_string(),
                id: s.id.to_string(),
                title: s.title.clone(),
                cover_url: None,
                year: s.year.map(i64::from),
                description: None,
            });
            json!({
                "id": s.id,
                "title": s.title,
                "year": s.year,
                "kind": s.content_category,
                "character": s.character,
                "job": s.job,
                "guest_in_episodes": (s.guest_episodes > 0).then_some(s.guest_episodes),
                "episodes_held": s.episodes_held,
            })
        })
        .collect();

    let shown = movie_items.len() + show_items.len();
    let held_total = held_movies.len() + held_shows.len();
    let not_held_total = not_held_movies + not_held_shows;

    let mut say = match held_total {
        0 => format!("The library has nothing with {} in it.", person.name),
        _ => {
            let mut parts = Vec::new();
            if !held_movies.is_empty() {
                parts.push(plural(held_movies.len(), "film", "films"));
            }
            if !held_shows.is_empty() {
                parts.push(plural(held_shows.len(), "show", "shows"));
            }
            format!("You have {} with {} in.", parts.join(" and "), person.name)
        }
    };
    if shown < held_total {
        say.push_str(&format!(" I'm only listing {shown} of them."));
    }
    if not_held_total > 0 {
        say.push_str(&format!(
            " {} more {} credited to them that you don't have.",
            not_held_total,
            match not_held_total {
                1 => "title is",
                _ => "titles are",
            }
        ));
    }

    let detail = match held_total {
        0 => format!("nothing held with {}", person.name),
        n => format!("{n} with {}", person.name),
    };
    ToolOutcome::ok(
        json!({
            "ok": true,
            "found": true,
            "person": person.name,
            "in_library": {"movies": held_movies.len(), "shows": held_shows.len()},
            "movies": movie_items,
            "shows": show_items,
            "truncated": shown < held_total,
            "credited_but_not_held": {"movies": not_held_movies, "shows": not_held_shows},
            "note": "Everything under movies/shows is actually held. credited_but_not_held is only a count — never read those out as things they have.",
            "say": say,
        }),
        detail,
    )
    .with_sources(sources)
}

/// Who is in one film or show.
pub async fn cast_of(state: &AppState, args: &Value) -> ToolOutcome {
    let Some(title) = arg_str(args, "title") else {
        return ToolOutcome::failed("cast_of needs a title");
    };
    let want = limit(args, 15, MAX_CAST);
    let kind = kind_arg(args);

    let titles = match chat_credits::find_titles(&state.pool, title, kind, MAX_CANDIDATES).await {
        Ok(t) => t,
        Err(e) => return ToolOutcome::failed(format!("could not look that title up: {e}")),
    };
    let Some(first) = titles.first() else {
        return ToolOutcome::ok(
            json!({
                "ok": true,
                "found": false,
                "title": title,
                "note": format!("Nothing in the library matches '{title}'."),
            }),
            format!("no title matching {title}"),
        );
    };

    let exact: Vec<_> = titles
        .iter()
        .filter(|t| t.title.eq_ignore_ascii_case(title))
        .collect();
    if exact.len() != 1 && titles.len() > 1 {
        let options: Vec<Value> = titles
            .iter()
            .map(|t| json!({"title": t.title, "year": t.year, "type": t.kind.as_str()}))
            .collect();
        return ToolOutcome::ok(
            json!({
                "ok": true,
                "found": false,
                "ambiguous": true,
                "candidates": options,
                "note": "Several titles match. Ask which one they mean; do not pick.",
            }),
            format!("{} titles match {title}", titles.len()),
        );
    }
    let subject = exact.first().copied().unwrap_or(first);

    // One over the cap tells us there was more without a second count query.
    let probe = want as i64 + 1;
    let (cast, crew) = match subject.kind {
        TitleKind::Movie => (
            chat_credits::movie_cast(&state.pool, subject.id, probe).await,
            chat_credits::movie_directors(&state.pool, subject.id).await,
        ),
        TitleKind::Show => (
            chat_credits::show_cast(&state.pool, subject.id, probe).await,
            chat_credits::show_creators(&state.pool, subject.id).await,
        ),
    };
    let mut cast = match cast {
        Ok(c) => c,
        Err(e) => return ToolOutcome::failed(format!("could not read the cast: {e}")),
    };
    let crew = match crew {
        Ok(c) => c,
        Err(e) => return ToolOutcome::failed(format!("could not read the crew: {e}")),
    };
    let truncated = cast.len() > want;
    cast.truncate(want);

    let cast_items: Vec<Value> = cast
        .iter()
        .map(|c| json!({"name": c.name, "character": c.character}))
        .collect();
    let crew_items: Vec<Value> = crew
        .iter()
        .map(|c| json!({"name": c.name, "job": c.job}))
        .collect();

    let label = match subject.year {
        Some(y) => format!("{} ({y})", subject.title),
        None => subject.title.clone(),
    };
    let mut say = match cast_items.len() {
        0 => format!("There are no cast credits stored for {label}."),
        n => format!("{label} has {} credited.", plural(n, "actor", "actors")),
    };
    if truncated {
        say.push_str(" That's the top of the billing, not everyone.");
    }
    if !subject.held {
        say.push_str(" You don't actually have this one.");
    }

    let item_type = match subject.kind {
        TitleKind::Movie => "movie",
        TitleKind::Show => "tv_show",
    };
    let sources = vec![Source {
        url: source_url(item_type, &subject.id.to_string()),
        item_type: item_type.to_string(),
        id: subject.id.to_string(),
        title: subject.title.clone(),
        cover_url: None,
        year: subject.year.map(i64::from),
        description: None,
    }];

    ToolOutcome::ok(
        json!({
            "ok": true,
            "found": true,
            "title": subject.title,
            "year": subject.year,
            "type": subject.kind.as_str(),
            "in_library": subject.held,
            "cast": cast_items,
            "crew": crew_items,
            "truncated": truncated,
            "say": say,
        }),
        format!("{} in {label}", cast.len()),
    )
    .with_sources(sources)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_clamped_rather_than_trusted() {
        assert_eq!(limit(&json!({}), 15, 40), 15);
        assert_eq!(limit(&json!({"limit": 900}), 15, 40), 40);
        assert_eq!(limit(&json!({"limit": 0}), 15, 40), 1);
        assert_eq!(limit(&json!({"limit": "7"}), 15, 40), 7);
    }

    /// "any" is the model's way of saying it does not know, and a wrong hint is
    /// worse than none — it must not narrow the search.
    #[test]
    fn an_any_media_type_is_no_hint_at_all() {
        assert_eq!(kind_arg(&json!({"media_type": "any"})), None);
        assert_eq!(kind_arg(&json!({})), None);
        assert_eq!(
            kind_arg(&json!({"media_type": "movie"})),
            Some(TitleKind::Movie)
        );
        assert_eq!(
            kind_arg(&json!({"media_type": "show"})),
            Some(TitleKind::Show)
        );
    }
}
