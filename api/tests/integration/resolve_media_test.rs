//! The phrasing table is the specification for `chatbot::resolve`.
//!
//! Expected refs are written with `@FIXTURE_NAME` placeholders that
//! `api::chatbot::resolve::fixture::data` expands to the fixture library's
//! UUIDs, so the table stays readable while asserting exact refs.
//!
//! Everything here runs against in-memory fixtures: no Meili, no Postgres, no
//! provider HTTP, no LLM.

use std::str::FromStr;

use api::chatbot::resolve::fixture::data;
use api::chatbot::resolve::fixture::{FixtureIndex, FixtureProviders, FixtureStore};
use api::chatbot::resolve::{
    MediaKind, MediaRef, MediaTypeHint, ProviderScope, Resolution, ResolveContext, ResolveRequest,
    WantFormat, resolve_media,
};
use uuid::Uuid;

#[derive(Debug, Clone, Copy)]
pub enum Expect {
    /// Resolves to exactly one ref.
    One(&'static str),
    /// Refuses to collapse: every listed ref must be among the candidates,
    /// and the resolution must be ambiguous.
    Ambiguous(&'static [&'static str]),
    /// The query is relative ("the new one") with nothing to be relative to.
    NeedsContext,
    /// Not in the library and not at any provider.
    NotFound,
}

#[derive(Debug, Clone, Copy)]
pub struct Case {
    pub name: &'static str,
    pub query: &'static str,
    /// Prior turn's resolved ref, as a `@FIXTURE_NAME` placeholder.
    pub context: Option<&'static str>,
    pub media_type: Option<&'static str>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub author: Option<&'static str>,
    pub format: Option<&'static str>,
    pub exclude_watched: bool,
    /// Episodes this user has already watched, as `@FIXTURE_NAME`s.
    pub watched: &'static [&'static str],
    /// The caller's ebook/audiobook history, for format inference.
    pub skew: Option<(i64, i64)>,
    pub expect: Expect,
    /// Expected resolved book format, when the case is about format.
    pub expect_format: Option<&'static str>,
    /// The case must come back with a format question rather than a guess.
    pub expect_format_question: bool,
    /// When the answer is a single provider show, how many seasons the
    /// resolution must carry back.
    pub expect_seasons: Option<usize>,
}

const fn case(name: &'static str, query: &'static str, expect: Expect) -> Case {
    Case {
        name,
        query,
        context: None,
        media_type: None,
        season: None,
        episode: None,
        author: None,
        format: None,
        exclude_watched: false,
        watched: &[],
        skew: None,
        expect,
        expect_format: None,
        expect_format_question: false,
        expect_seasons: None,
    }
}

pub const CASES: &[Case] = &[
    // ---- explicit season / episode syntax, three ways of saying it -------
    case(
        "season by number",
        "season 24 of someShow",
        Expect::One("season:@SEASON_SOMESHOW_24"),
    ),
    case(
        "padded episode code",
        "someShow S24E07",
        Expect::One("episode:@EP_SOMESHOW_S24E07"),
    ),
    case(
        "unpadded split code",
        "s24 e7 of someShow",
        Expect::One("episode:@EP_SOMESHOW_S24E07"),
    ),
    case(
        "spelled out season and episode",
        "someShow season 24 episode 7",
        Expect::One("episode:@EP_SOMESHOW_S24E07"),
    ),
    Case {
        season: Some(24),
        episode: Some(7),
        ..case(
            "numbers as arguments, not in the text",
            "someShow",
            Expect::One("episode:@EP_SOMESHOW_S24E07"),
        )
    },
    Case {
        season: Some(24),
        ..case(
            "season as an argument",
            "someShow",
            Expect::One("season:@SEASON_SOMESHOW_24"),
        )
    },
    case(
        "an episode number that does not exist",
        "someShow S24E44",
        Expect::NotFound,
    ),
    // A season we do not hold must never be answered with one we do. The show
    // scores below its own seasons whenever the query is not its exact title,
    // so the winning candidate is a season doc — and nothing downstream checked
    // that it was the season that was actually named.
    case(
        "a season we do not have is not swapped for one we do",
        "season 5 of bake off",
        Expect::NotFound,
    ),
    // The show is in the library, the season is not: the provider is the only
    // place that season can come from, and it was unreachable.
    Case {
        season: Some(12),
        ..case(
            "a missing season of a library show reaches the provider",
            "someShow",
            Expect::One("provider:tmdb:70001:s12"),
        )
    },
    // ---- relative time, with and without prior context -------------------
    Case {
        context: Some("show:@SHOW_SOMESHOW"),
        ..case(
            "the new one, with a show in context",
            "the new one",
            Expect::One("episode:@EP_SOMESHOW_S24E08"),
        )
    },
    case("the new one, cold", "the new one", Expect::NeedsContext),
    Case {
        context: Some("show:@SHOW_SOMESHOW"),
        ..case(
            "the latest episode, with a show in context",
            "the latest episode",
            Expect::One("episode:@EP_SOMESHOW_S24E08"),
        )
    },
    case(
        "the latest episode, show named",
        "the latest episode of someShow",
        Expect::One("episode:@EP_SOMESHOW_S24E08"),
    ),
    case(
        "the latest episode, cold",
        "the latest episode",
        Expect::NeedsContext,
    ),
    Case {
        context: Some("show:@SHOW_SOMESHOW"),
        ..case(
            "last night's episode",
            "last night's episode",
            Expect::One("episode:@EP_SOMESHOW_S24E08"),
        )
    },
    case(
        "last night's episode, show named",
        "last night's episode of someShow",
        Expect::One("episode:@EP_SOMESHOW_S24E08"),
    ),
    case(
        "last night's episode of a show that did not air last night",
        "last night's episode of the bake off",
        Expect::NotFound,
    ),
    case(
        "last night's episode, cold",
        "last night's episode",
        Expect::NeedsContext,
    ),
    Case {
        context: Some("episode:@EP_SOMESHOW_S24E07"),
        ..case(
            "the next one, with an episode in context",
            "the next one",
            Expect::One("episode:@EP_SOMESHOW_S24E08"),
        )
    },
    Case {
        context: Some("show:@SHOW_SOMESHOW"),
        ..case(
            "the next one, with a show in context, means the next unaired",
            "the next one",
            Expect::One("episode:@EP_SOMESHOW_S24E09"),
        )
    },
    case("the next one, cold", "the next one", Expect::NeedsContext),
    case(
        "when is the next one",
        "when's the next someShow",
        Expect::One("episode:@EP_SOMESHOW_S24E09"),
    ),
    case(
        "the first one",
        "the first episode of someShow",
        Expect::One("episode:@EP_SOMESHOW_S01E01"),
    ),
    Case {
        exclude_watched: true,
        watched: &["EP_SOMESHOW_S24E08"],
        context: Some("show:@SHOW_SOMESHOW"),
        ..case(
            "the latest one I have not seen",
            "the latest episode",
            Expect::One("episode:@EP_SOMESHOW_S24E07"),
        )
    },
    // ---- fuzzy and descriptive ------------------------------------------
    case(
        "that baking show",
        "that baking show",
        Expect::One("show:@SHOW_BAKEOFF"),
    ),
    case(
        "described by its set, not its title",
        "the one with the tents",
        Expect::One("show:@SHOW_BAKEOFF"),
    ),
    case(
        "bare show title",
        "someShow",
        Expect::One("show:@SHOW_SOMESHOW"),
    ),
    Case {
        media_type: Some("movie"),
        ..case(
            // `media_type` is an argument the model fills in, and it guesses.
            // A guess that finds nothing must not be the reason the right
            // answer was never offered — the shelf next to it still gets
            // searched. The two-Dunes case above is the other half of this:
            // a hint whose own shelf answers to the name still narrows.
            "a media type the model guessed wrong is not a wall",
            "someShow",
            Expect::One("show:@SHOW_SOMESHOW"),
        )
    },
    // ---- books, authors, series, format ---------------------------------
    Case {
        media_type: Some("book"),
        ..case(
            "the new Sanderson resolves to the newest Sanderson she owns",
            "the new Sanderson",
            Expect::One("book:@BOOK_RHYTHM_OF_WAR"),
        )
    },
    Case {
        expect_format: Some("audiobook"),
        ..case(
            "the new Sanderson audiobook is still the newest Sanderson",
            "the new Sanderson audiobook",
            Expect::One("book:@BOOK_RHYTHM_OF_WAR"),
        )
    },
    Case {
        expect_format: Some("audiobook"),
        ..case(
            "explicit format in the message",
            "the audiobook of the way of kings",
            Expect::One("book:@BOOK_WAY_OF_KINGS"),
        )
    },
    Case {
        expect_format: Some("ebook"),
        ..case(
            "explicit ebook",
            "the ebook of the way of kings",
            Expect::One("book:@BOOK_WAY_OF_KINGS"),
        )
    },
    Case {
        skew: Some((1, 9)),
        expect_format: Some("audiobook"),
        ..case(
            "no format said, but she listens",
            "the way of kings",
            Expect::One("book:@BOOK_WAY_OF_KINGS"),
        )
    },
    Case {
        skew: Some((9, 1)),
        expect_format: Some("ebook"),
        ..case(
            "no format said, but she reads",
            "the way of kings",
            Expect::One("book:@BOOK_WAY_OF_KINGS"),
        )
    },
    Case {
        skew: Some((5, 5)),
        expect_format_question: true,
        ..case(
            "a genuinely split history is the only case that gets asked",
            "the way of kings",
            Expect::One("book:@BOOK_WAY_OF_KINGS"),
        )
    },
    Case {
        author: Some("Brandon Sanderson"),
        media_type: Some("book"),
        ..case(
            "author as an argument",
            "the way of kings",
            Expect::One("book:@BOOK_WAY_OF_KINGS"),
        )
    },
    case(
        "author by name",
        "brandon sanderson",
        Expect::One("author:@AUTHOR_SANDERSON"),
    ),
    case(
        "series by name",
        "the stormlight archive",
        Expect::One("series:@SERIES_STORMLIGHT"),
    ),
    // ---- movies ---------------------------------------------------------
    case(
        "movie by loose title",
        "the great escape",
        Expect::One("movie:@MOVIE_GREAT_ESCAPE"),
    ),
    case(
        "movie by typo",
        "arival",
        Expect::One("movie:@MOVIE_ARRIVAL"),
    ),
    case(
        "movie by title and year",
        "dune 1984",
        Expect::One("movie:@MOVIE_DUNE_1984"),
    ),
    case(
        "movie by title and parenthesised year",
        "dune (2021)",
        Expect::One("movie:@MOVIE_DUNE_2021"),
    ),
    // ---- collisions that must be questions, not guesses ------------------
    case(
        "title collides across media types",
        "dune",
        Expect::Ambiguous(&[
            "book:@BOOK_DUNE",
            "movie:@MOVIE_DUNE_2021",
            "movie:@MOVIE_DUNE_1984",
        ]),
    ),
    Case {
        media_type: Some("movie"),
        ..case(
            "media type narrows it but two films still share the title",
            "dune",
            Expect::Ambiguous(&["movie:@MOVIE_DUNE_2021", "movie:@MOVIE_DUNE_1984"]),
        )
    },
    Case {
        media_type: Some("book"),
        ..case(
            "media type disambiguates entirely",
            "dune",
            Expect::One("book:@BOOK_DUNE"),
        )
    },
    case(
        "reboot: two shows share a name",
        "battlestar galactica",
        Expect::Ambiguous(&["show:@SHOW_BSG_2004", "show:@SHOW_BSG_1978"]),
    ),
    case(
        "reboot disambiguated by year",
        "battlestar galactica 2004",
        Expect::One("show:@SHOW_BSG_2004"),
    ),
    // ---- not in the library: fall through to the metadata providers ------
    Case {
        media_type: Some("book"),
        ..case(
            "book not in the library falls through to the book providers",
            "project hail mary",
            Expect::One("provider:hardcover:99001:book"),
        )
    },
    Case {
        media_type: Some("show"),
        ..case(
            "show not in the library falls through to the video providers",
            "severance",
            Expect::One("provider:tmdb:95396:show"),
        )
    },
    Case {
        media_type: Some("show"),
        ..case(
            // The provider is asked for the words as spoken. Asked for the
            // ranker's tokens instead it would be hunting for "seen", because
            // "last" is an intent word in "last night's episode".
            "a title made of intent words still reaches the providers",
            "Last Seen (2026)",
            Expect::One("provider:tmdb:70002:show"),
        )
    },
    case(
        "season of a show not in the library",
        "season 3 of severance",
        Expect::One("provider:tmdb:95396:s03"),
    ),
    case(
        "episode of a show not in the library",
        "severance s03e04",
        Expect::One("provider:tmdb:95396:s03e04"),
    ),
    Case {
        media_type: Some("movie"),
        ..case(
            "movie not in the library falls through to the video providers",
            "the brutalist",
            Expect::One("provider:tmdb:549509:movie"),
        )
    },
    // ---- same-named shows that only the providers know ----------------------
    // The providers are asked for the words as spoken and find nothing, so the
    // ladder drops to "ludwig"; the person named then breaks the tie the title
    // cannot.
    Case {
        expect_seasons: Some(2),
        ..case(
            "the person named breaks the tie between same-named shows",
            "Ludwig with David Mitchell",
            Expect::One("provider:tmdb:243360:show"),
        )
    },
    Case {
        season: Some(2),
        expect_seasons: Some(2),
        ..case(
            "a season of the show, named by its person",
            "Ludwig with David Mitchell",
            Expect::One("provider:tmdb:243360:s02"),
        )
    },
    // The provider says the show has seasons 1 and 2; a request for season 3
    // must fail here, at resolve time, instead of queueing something that does
    // not exist.
    Case {
        season: Some(3),
        ..case(
            "a season the provider does not have is not found",
            "Ludwig with David Mitchell",
            Expect::NotFound,
        )
    },
    case(
        "same-named shows with no person named stay a question",
        "Ludwig",
        Expect::Ambiguous(&["provider:tmdb:243360:show", "provider:tmdb:85899:show"]),
    ),
    case(
        "a 'with' that names no person breaks no tie",
        "Ludwig with the twins",
        Expect::Ambiguous(&["provider:tmdb:243360:show", "provider:tmdb:85899:show"]),
    ),
    // ---- sports ride the same rails as TV --------------------------------
    case(
        "sports event",
        "the monaco grand prix",
        Expect::One("sports_event:@EVENT_MONACO"),
    ),
    case("sports league", "formula 1", Expect::One("show:@SHOW_F1")),
    case(
        "latest sports event",
        "the latest formula 1",
        Expect::One("sports_event:@EVENT_MONACO"),
    ),
    // ---- the referent, with the rest of the sentence attached -------------
    case(
        "a complaint trailing the episode they meant",
        "the latest episode of someShow, the sound is out of sync",
        Expect::One("episode:@EP_SOMESHOW_S24E08"),
    ),
    case(
        "a complaint trailing the film they meant",
        "another copy of the great escape, the audio is broken",
        Expect::One("movie:@MOVIE_GREAT_ESCAPE"),
    ),
    case(
        "a reason trailing the show they meant",
        "the latest someShow, it has audio descriptions I do not want",
        Expect::One("episode:@EP_SOMESHOW_S24E08"),
    ),
    case(
        "trailing words never rescue a title that is not there",
        "qwertyuiop the sound is out of sync",
        Expect::NotFound,
    ),
    // ---- nothing anywhere -------------------------------------------------
    case(
        "nothing in the library and nothing at any provider",
        "qwertyuiop nonexistent thing",
        Expect::NotFound,
    ),
];

fn fixture_uuid(name: &str) -> Uuid {
    data::id_by_name(name).unwrap_or_else(|| panic!("unknown fixture id @{name}"))
}

fn media_ref(raw: &str) -> MediaRef {
    let expanded = data::expand(raw);
    MediaRef::from_str(&expanded).unwrap_or_else(|e| panic!("bad ref {expanded}: {e}"))
}

fn run(case: &Case) -> api::chatbot::resolve::Candidates {
    let index = FixtureIndex::new();
    let providers = FixtureProviders;
    let watched: Vec<Uuid> = case.watched.iter().map(|n| fixture_uuid(n)).collect();
    let mut store = FixtureStore::new().watched(&watched);
    if let Some((ebook, audiobook)) = case.skew {
        store = store.with_skew(ebook, audiobook);
    }
    let ctx = ResolveContext {
        index: &index,
        providers: &providers,
        store: &store,
        user_id: Uuid::from_u128(0xF00D),
        now: data::now(),
    };
    let request = ResolveRequest {
        query: case.query.to_string(),
        media_type: case
            .media_type
            .map(|m| MediaTypeHint::parse(m).unwrap_or_else(|| panic!("bad media type {m}"))),
        season: case.season,
        episode: case.episode,
        author: case.author.map(str::to_string),
        format: case
            .format
            .map(|f| WantFormat::parse(f).unwrap_or_else(|| panic!("bad format {f}"))),
        context: case.context.map(media_ref),
        exclude_watched: case.exclude_watched,
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(resolve_media(&ctx, &request))
        .expect("resolve")
}

fn describe(out: &api::chatbot::resolve::Candidates) -> String {
    format!(
        "{:?} {:?}",
        out.resolution,
        out.candidates
            .iter()
            .map(|c| format!("{} [{:.3}]", c.media_ref, c.score))
            .collect::<Vec<_>>()
    )
}

#[test]
fn phrasing_table() {
    let mut failures = Vec::new();
    for case in CASES {
        let out = run(case);
        let detail = describe(&out);
        match case.expect {
            Expect::One(expected) => {
                let expected = media_ref(expected);
                match out.single() {
                    Some(c) if c.media_ref == expected => {}
                    _ => failures.push(format!("{}: expected {expected}, got {detail}", case.name)),
                }
            }
            Expect::Ambiguous(expected) => {
                if out.resolution != Resolution::Ambiguous {
                    failures.push(format!("{}: expected ambiguity, got {detail}", case.name));
                } else {
                    let refs = out.refs();
                    for want in expected {
                        let want = media_ref(want);
                        if !refs.contains(&want) {
                            failures.push(format!(
                                "{}: {want} missing from candidates, got {detail}",
                                case.name
                            ));
                        }
                    }
                }
            }
            Expect::NeedsContext => {
                if out.resolution != Resolution::NeedsContext {
                    failures.push(format!("{}: expected a question, got {detail}", case.name));
                }
            }
            Expect::NotFound => {
                if out.resolution != Resolution::NotFound {
                    failures.push(format!("{}: expected not-found, got {detail}", case.name));
                }
            }
        }

        if let Some(expected) = case.expect_format {
            let expected = WantFormat::parse(expected).expect("bad expected format");
            if out.format != Some(expected) {
                failures.push(format!(
                    "{}: expected format {expected:?}, got {:?}",
                    case.name, out.format
                ));
            }
        }
        if case.expect_format_question && out.format_question.is_none() {
            failures.push(format!("{}: expected a format question", case.name));
        }
        if let Some(want) = case.expect_seasons {
            let got = out.seasons.as_ref().map(|s| s.len());
            if got != Some(want) {
                failures.push(format!(
                    "{}: expected {want} seasons in the answer, got {:?}",
                    case.name, got
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} phrasings failed:\n{}",
        failures.len(),
        CASES.len(),
        failures.join("\n")
    );
}

#[test]
fn the_table_is_broad_enough_to_be_a_spec() {
    assert!(CASES.len() >= 40, "the table is the spec; keep it broad");
}

#[test]
fn ambiguity_never_collapses_to_one() {
    for case in CASES {
        let Expect::Ambiguous(_) = case.expect else {
            continue;
        };
        let out = run(case);
        assert!(
            out.single().is_none(),
            "{}: ambiguous cases must not resolve to a single ref",
            case.name
        );
        assert!(
            out.candidates.len() >= 2,
            "{}: expected at least two candidates, got {}",
            case.name,
            out.candidates.len()
        );
        assert!(
            out.question.is_some(),
            "{}: an ambiguous result has to carry the question",
            case.name
        );
    }
}

#[test]
fn every_expected_ref_round_trips() {
    for case in CASES {
        let refs: Vec<&str> = match case.expect {
            Expect::One(r) => vec![r],
            Expect::Ambiguous(rs) => rs.to_vec(),
            _ => vec![],
        };
        for raw in refs {
            let expanded = data::expand(raw);
            let parsed = MediaRef::from_str(&expanded).expect(&expanded);
            assert_eq!(parsed.to_string(), expanded, "{} in {}", raw, case.name);
        }
    }
}

/// The live failure, reduced: a partially-indexed show.
///
/// Seasons 1 and 24 of someShow are in the index; the show document is not —
/// the state the search index was actually in when a request for season 25 came
/// back offering seasons 11 and 26 of a show, and then queued one of them.
///
/// The number the caller named is the whole request. A season document for some
/// *other* season is not a worse answer than the show, it is not an answer at
/// all, and it must not be returned, ranked or offered as a pick.
mod partial_index {
    use super::*;
    use api::chatbot::resolve::index::IndexHits;

    fn seasons_only() -> IndexHits {
        let mut corpus = data::corpus();
        corpus.shows.clear();
        corpus.episodes.clear();
        corpus
    }

    fn resolve(season: Option<i32>, query: &str) -> api::chatbot::resolve::Candidates {
        let index = FixtureIndex::with_corpus(seasons_only());
        let providers = FixtureProviders;
        let store = FixtureStore::new();
        let ctx = ResolveContext {
            index: &index,
            providers: &providers,
            store: &store,
            user_id: Uuid::from_u128(0xF00D),
            now: data::now(),
        };
        let request = ResolveRequest {
            query: query.to_string(),
            media_type: None,
            season,
            episode: None,
            author: None,
            format: None,
            context: None,
            exclude_watched: false,
        };
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(resolve_media(&ctx, &request))
            .expect("resolve")
    }

    /// The one that queued season 11.
    #[test]
    fn a_named_season_is_never_answered_with_a_different_one() {
        // Only seasons 1 and 24 are indexed, so any *library* season here is the
        // wrong season by construction; season 25 can only come from a provider.
        let out = resolve(Some(25), "someShow");
        for candidate in &out.candidates {
            if candidate.media_ref.kind() != MediaKind::Season {
                continue;
            }
            let scope = candidate.media_ref.provider_ref().map(|p| p.scope);
            assert_eq!(
                scope,
                Some(ProviderScope::Season(25)),
                "season 25 was asked for and {} came back: {}",
                candidate.media_ref,
                describe(&out),
            );
        }
    }

    /// The card itself: two seasons of one show, neither of them the one named.
    #[test]
    fn the_seasons_we_do_have_are_not_a_question_to_ask() {
        let out = resolve(Some(25), "someShow");
        assert_ne!(
            out.resolution,
            Resolution::Ambiguous,
            "a season nobody has is not a choice between the seasons they do: {}",
            describe(&out),
        );
    }

    /// Without the number there is nothing to descend to, so seasons of one show
    /// really are a fair question — this is the behaviour the fix must not eat.
    #[test]
    fn without_a_season_number_the_seasons_are_still_candidates() {
        let out = resolve(None, "someShow");
        assert!(
            !out.candidates.is_empty(),
            "the seasons are all that is indexed; they must still answer: {}",
            describe(&out),
        );
    }
}

/// A pick has to be able to say the whole request.
#[test]
fn an_ambiguous_show_carries_the_season_into_the_pick() {
    let out = run(&Case {
        season: Some(3),
        ..case(
            "two shows, one named season",
            "battlestar galactica",
            Expect::Ambiguous(&[]),
        )
    });
    assert_eq!(out.resolution, Resolution::Ambiguous, "{}", describe(&out));
    assert_eq!(
        out.pick_qualifier.as_deref(),
        Some("season 3"),
        "tapping a show must not drop the season: {}",
        describe(&out),
    );
}
