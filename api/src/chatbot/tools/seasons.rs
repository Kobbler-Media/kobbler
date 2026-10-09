//! How complete one season is.
//!
//! Read-only, and postgres rather than Meili for the same reason `credits` is:
//! the answer is a join and an aggregate, and the index it would otherwise come
//! off carries neither. The seasons projection has `episode_count` and no
//! availability, so "how many episodes does the current season have" reached
//! the *show's* `available_episode_count` and answered 22 for a season of 8.
//!
//! The observation states held, aired and still-to-come as separate numbers on
//! purpose. An episode that has not aired is not a gap, and left to count a
//! truncated list by eye the model reported two unaired episodes as two
//! missing ones.
//!
//! The show comes in as a plain name, like `next_airing` and `cast_of` — a read
//! does not need a resolver ref, and a two-call chain is a step this model
//! spends its budget on.

use serde_json::{Value, json};

use crate::AppState;
use crate::chatbot::events::{Source, source_url};
use crate::repositories::chat_seasons::{self, Season, SeasonCounts, SeasonEpisode, Standing};
use crate::repositories::chat_tools;

use super::read::ToolOutcome;

const MAX_CANDIDATE_SHOWS: i64 = 5;
const MAX_LISTED: i64 = 30;

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn arg_i32(args: &Value, key: &str) -> Option<i32> {
    args.get(key).and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .filter(|n| (0..=10_000).contains(n))
            .map(|n| n as i32)
    })
}

fn episode_code(season: &Season, episode: &SeasonEpisode) -> String {
    format!(
        "S{:02}E{:02}",
        season.season_number.max(0),
        episode.episode_number.max(0)
    )
}

fn listed(season: &Season, episodes: &[SeasonEpisode], standing: Standing) -> Vec<Value> {
    episodes
        .iter()
        .filter(|e| e.standing == standing)
        .map(|e| {
            json!({
                "episode": episode_code(season, e),
                "number": e.episode_number,
                "title": e.title,
                "air_date": e.air_date.map(|d| d.to_string()),
                "held": e.held,
            })
        })
        .collect()
}

fn codes(items: &[Value], take: usize) -> String {
    items
        .iter()
        .take(take)
        .filter_map(|v| v["episode"].as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn plural(n: i64, one: &str, many: &str) -> String {
    match n {
        1 => format!("1 {one}"),
        _ => format!("{n} {many}"),
    }
}

fn season_label(season: &Season) -> String {
    match season.is_special {
        true => "The specials".to_string(),
        false => format!("Season {}", season.season_identifier),
    }
}

/// Every number in the `say` line is already correct, and each of held, aired
/// and to-come is named rather than left to be inferred from the others — the
/// live failure was arithmetic the model did itself.
fn say_line(show: &str, season: &Season, counts: SeasonCounts, missing: &[Value]) -> String {
    let label = season_label(season);
    let mut say = format!(
        "{label} of {show} has {}.",
        plural(counts.episodes, "episode", "episodes")
    );

    if counts.aired > 0 {
        say.push_str(&format!(
            " {} aired so far, and you have {} of {}.",
            plural(counts.aired, "episode has", "episodes have"),
            counts.held_aired,
            counts.aired
        ));
    } else {
        say.push_str(" None of them have aired yet.");
    }

    match counts.missing_aired() {
        0 if counts.aired > 0 => say.push_str(" Nothing that has aired is missing."),
        0 => {}
        n => {
            let names = codes(missing, 6);
            say.push_str(&format!(
                " {} that {} aired {} missing{}.",
                plural(n, "episode", "episodes"),
                match n {
                    1 => "has",
                    _ => "have",
                },
                match n {
                    1 => "is",
                    _ => "are",
                },
                match names.is_empty() {
                    true => String::new(),
                    false => format!(" — {names}"),
                }
            ));
        }
    }

    if counts.to_come > 0 {
        say.push_str(&format!(
            " The other {} not aired yet, so {} not missing — do not count {} as gaps.",
            match counts.to_come {
                1 => "one has".to_string(),
                n => format!("{n} have"),
            },
            match counts.to_come {
                1 => "it is",
                _ => "they are",
            },
            match counts.to_come {
                1 => "it",
                _ => "them",
            },
        ));
    }

    if counts.undated > 0 {
        say.push_str(&format!(
            " {} no air date recorded, so I cannot say whether {} aired.",
            plural(counts.undated, "episode has", "episodes have"),
            match counts.undated {
                1 => "it has",
                _ => "they have",
            },
        ));
    }

    if counts.held_unaired > 0 {
        say.push_str(&format!(
            " You already have {} of the ones that have not aired.",
            counts.held_unaired
        ));
    }

    if let Some(total) = season.metadata_episode_count
        && i64::from(total) != counts.episodes
    {
        say.push_str(&format!(
            " The show's own listing says {total} for this season, so the episode list here may be incomplete."
        ));
    }

    say
}

/// How complete one season of one show is, split so that "not aired yet" can
/// never be read back as "missing".
pub async fn season_status(state: &AppState, args: &Value) -> ToolOutcome {
    let Some(title) = arg_str(args, "show") else {
        return ToolOutcome::failed("season_status needs a show");
    };
    let wanted = arg_i32(args, "season");

    let candidates = match chat_tools::find_shows(&state.pool, title, MAX_CANDIDATE_SHOWS).await {
        Ok(c) => c,
        Err(e) => return ToolOutcome::failed(format!("could not look that show up: {e}")),
    };
    let Some(first) = candidates.first() else {
        return ToolOutcome::ok(
            json!({
                "ok": true,
                "found": false,
                "show": title,
                "note": format!("Nothing in the library matches '{title}'."),
            }),
            format!("no show matching {title}"),
        );
    };
    let exact = candidates
        .iter()
        .filter(|c| c.title.eq_ignore_ascii_case(title))
        .count();
    if exact != 1 && candidates.len() > 1 {
        let options: Vec<Value> = candidates
            .iter()
            .map(|c| json!({"title": c.title, "year": c.year, "kind": c.content_category}))
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
    let show = candidates
        .iter()
        .find(|c| c.title.eq_ignore_ascii_case(title))
        .unwrap_or(first);

    let season = match wanted {
        Some(n) => chat_seasons::season_by_number(&state.pool, show.id, n).await,
        None => chat_seasons::current_season(&state.pool, show.id).await,
    };
    let season = match season {
        Ok(s) => s,
        Err(e) => return ToolOutcome::failed(format!("could not read the seasons: {e}")),
    };
    let Some(season) = season else {
        let all = chat_seasons::seasons_of(&state.pool, show.id)
            .await
            .unwrap_or_default();
        let options: Vec<Value> = all
            .iter()
            .map(|s| {
                json!({
                    "season": s.season_identifier,
                    "specials": s.is_special,
                    "episodes_listed": s.metadata_episode_count,
                })
            })
            .collect();
        let note = match (wanted, all.is_empty()) {
            (_, true) => format!("{} has no seasons recorded at all.", show.title),
            (Some(n), false) => format!(
                "{} has no season {n}. Ask which of the seasons listed they mean; do not pick.",
                show.title
            ),
            (None, false) => format!(
                "No ordinary season of {} has aired yet. Ask which season they mean; do not pick.",
                show.title
            ),
        };
        return ToolOutcome::ok(
            json!({
                "ok": true,
                "found": false,
                "show": show.title,
                "seasons": options,
                "note": note,
            }),
            format!("no such season of {}", show.title),
        );
    };

    let counts = match chat_seasons::season_counts(&state.pool, season.id).await {
        Ok(c) => c,
        Err(e) => return ToolOutcome::failed(format!("could not count the season: {e}")),
    };
    let episodes = chat_seasons::season_episode_standings(&state.pool, season.id, MAX_LISTED)
        .await
        .unwrap_or_default();

    let missing = listed(&season, &episodes, Standing::MissingAired);
    let to_come = listed(&season, &episodes, Standing::ToCome);
    let undated = listed(&season, &episodes, Standing::NoAirDate);
    let say = say_line(&show.title, &season, counts, &missing);

    let sources = vec![Source {
        url: source_url("tv_season", &season.id.to_string()),
        item_type: "tv_season".to_string(),
        id: season.id.to_string(),
        title: format!("{} — {}", show.title, season_label(&season)),
        cover_url: None,
        year: show.year.map(i64::from),
        description: None,
    }];

    let detail = format!(
        "{} S{}: {} of {} aired held",
        show.title, season.season_identifier, counts.held_aired, counts.aired
    );

    ToolOutcome::ok(
        json!({
            "ok": true,
            "found": true,
            "show": show.title,
            "kind": show.content_category,
            "season": season.season_identifier,
            "season_title": season.title,
            "is_specials": season.is_special,
            "picked_current_season": wanted.is_none(),
            "episodes_in_season": counts.episodes,
            "episodes_listed_by_metadata": season.metadata_episode_count,
            "aired": counts.aired,
            "still_to_come": counts.to_come,
            "no_air_date": counts.undated,
            "held": counts.held,
            "held_of_aired": counts.held_aired,
            "missing_aired_count": counts.missing_aired(),
            "missing_aired": missing,
            "still_to_come_episodes": to_come,
            "no_air_date_episodes": undated,
            "truncated": (episodes.len() as i64) >= MAX_LISTED,
            "note": "missing_aired is the only gap. Episodes under still_to_come have not aired \
                     and must never be described as missing, wanted or absent. Air dates come \
                     from TMDB and are often a day out.",
            "say": say,
        }),
        detail,
    )
    .with_sources(sources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Date;
    use time::macros::date;
    use uuid::Uuid;

    fn season(number: i32, total: Option<i32>) -> Season {
        Season {
            id: Uuid::now_v7(),
            season_identifier: number.to_string(),
            season_number: number,
            title: None,
            is_special: number == 0,
            metadata_episode_count: total,
        }
    }

    fn counts(episodes: i64, aired: i64, held_aired: i64, to_come: i64) -> SeasonCounts {
        SeasonCounts {
            episodes,
            held: held_aired,
            aired,
            to_come,
            undated: episodes - aired - to_come,
            held_aired,
            held_unaired: 0,
        }
    }

    fn gap(number: i32, air_date: Date) -> SeasonEpisode {
        SeasonEpisode {
            episode_number: number,
            title: None,
            air_date: Some(air_date),
            held: false,
            standing: Standing::MissingAired,
        }
    }

    /// The live failure, as a sentence: 8 episodes, 6 aired, 6 held, 2 to come.
    /// "22 episodes available" and "you have 4" both came out of counting a
    /// truncated list, so the say line has to state each number outright.
    #[test]
    fn the_say_line_separates_not_aired_from_missing() {
        let say = say_line("someShow", &season(3, Some(8)), counts(8, 6, 6, 2), &[]);
        assert!(say.contains("has 8 episodes"), "{say}");
        assert!(say.contains("6 episodes have aired"), "{say}");
        assert!(say.contains("you have 6 of 6"), "{say}");
        assert!(say.contains("Nothing that has aired is missing"), "{say}");
        assert!(say.contains("The other 2 have not aired yet"), "{say}");
        assert!(say.contains("not missing"), "{say}");
        assert!(!say.contains("2 missing"), "{say}");
    }

    #[test]
    fn a_real_gap_is_named_and_kept_apart_from_the_unaired_ones() {
        let s = season(3, Some(8));
        let missing = listed(
            &s,
            &[gap(2, date!(2026 - 01 - 07)), gap(5, date!(2026 - 01 - 28))],
            Standing::MissingAired,
        );
        let say = say_line("someShow", &s, counts(8, 6, 4, 2), &missing);
        assert!(say.contains("you have 4 of 6"), "{say}");
        assert!(
            say.contains("2 episodes that have aired are missing"),
            "{say}"
        );
        assert!(say.contains("S03E02, S03E05"), "{say}");
        assert!(say.contains("The other 2 have not aired yet"), "{say}");
    }

    #[test]
    fn a_season_nobody_has_seen_yet_says_so_rather_than_reporting_gaps() {
        let say = say_line("someShow", &season(4, Some(10)), counts(10, 0, 0, 10), &[]);
        assert!(say.contains("None of them have aired yet"), "{say}");
        assert!(!say.contains("missing:"), "{say}");
    }

    /// A season shorter than the show's own listing means episodes we have no
    /// row for at all, which is not the same as episodes we do not hold.
    #[test]
    fn a_short_episode_list_is_flagged_rather_than_counted_as_gaps() {
        let say = say_line("someShow", &season(3, Some(8)), counts(6, 6, 6, 0), &[]);
        assert!(say.contains("listing says 8"), "{say}");
        assert!(say.contains("may be incomplete"), "{say}");
    }

    #[test]
    fn specials_are_named_as_specials_not_as_season_zero() {
        let say = say_line("someShow", &season(0, None), counts(3, 3, 1, 0), &[]);
        assert!(say.starts_with("The specials of someShow"), "{say}");
    }

    #[test]
    fn a_season_number_the_model_invented_out_of_range_is_ignored() {
        assert_eq!(arg_i32(&json!({"season": 3}), "season"), Some(3));
        assert_eq!(arg_i32(&json!({"season": "3"}), "season"), Some(3));
        assert_eq!(arg_i32(&json!({"season": 0}), "season"), Some(0));
        assert_eq!(arg_i32(&json!({"season": -1}), "season"), None);
        assert_eq!(arg_i32(&json!({}), "season"), None);
    }
}
