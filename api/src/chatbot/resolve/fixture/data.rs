//! A small, deterministic library: two same-titled films, a reboot pair, a
//! title that collides across media types, a show with a describable set, a
//! sports league, and an author whose newest book is not the one with an
//! audiobook.

use std::collections::HashMap;

use time::{Date, Month, OffsetDateTime};
use uuid::Uuid;

use crate::search::{
    AuthorDocument, BookDocument, MovieDocument, SeriesDocument, TvEpisodeDocument,
    TvSeasonDocument, TvShowDocument,
};

use crate::chatbot::resolve::index::IndexHits;

/// The clock the fixtures are built around, so "last night" is a fact.
pub fn now() -> OffsetDateTime {
    date(2026, Month::August, 24)
        .with_hms(20, 0, 0)
        .unwrap()
        .assume_utc()
}

fn date(year: i32, month: Month, day: u8) -> Date {
    Date::from_calendar_date(year, month, day).expect("fixture date")
}

fn iso(d: Date) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())
}

fn ts(d: Date) -> i64 {
    d.midnight().assume_utc().unix_timestamp()
}

macro_rules! fixture_ids {
    ($($name:ident = $n:literal),* $(,)?) => {
        $(pub const $name: Uuid = Uuid::from_u128($n);)*

        /// Resolve a `@NAME` placeholder from the phrasing table.
        pub fn id_by_name(name: &str) -> Option<Uuid> {
            match name {
                $(stringify!($name) => Some($name),)*
                _ => None,
            }
        }
    };
}

fixture_ids! {
    SHOW_SOMESHOW = 1,
    SEASON_SOMESHOW_01 = 2,
    SEASON_SOMESHOW_24 = 3,
    EP_SOMESHOW_S01E01 = 10,
    EP_SOMESHOW_S01E02 = 11,
    EP_SOMESHOW_S24E01 = 20,
    EP_SOMESHOW_S24E02 = 21,
    EP_SOMESHOW_S24E03 = 22,
    EP_SOMESHOW_S24E04 = 23,
    EP_SOMESHOW_S24E05 = 24,
    EP_SOMESHOW_S24E06 = 25,
    EP_SOMESHOW_S24E07 = 26,
    EP_SOMESHOW_S24E08 = 27,
    EP_SOMESHOW_S24E09 = 28,
    SHOW_BAKEOFF = 30,
    SEASON_BAKEOFF_12 = 31,
    EP_BAKEOFF_S12E01 = 32,
    EP_BAKEOFF_S12E02 = 33,
    SHOW_BSG_1978 = 40,
    SHOW_BSG_2004 = 41,
    SHOW_F1 = 50,
    SEASON_F1_2026 = 51,
    EVENT_SPA = 52,
    EVENT_MONACO = 53,
    MOVIE_DUNE_2021 = 60,
    MOVIE_DUNE_1984 = 61,
    MOVIE_GREAT_ESCAPE = 62,
    MOVIE_ARRIVAL = 63,
    BOOK_DUNE = 70,
    BOOK_WAY_OF_KINGS = 71,
    BOOK_RHYTHM_OF_WAR = 72,
    AUTHOR_SANDERSON = 80,
    AUTHOR_HERBERT = 81,
    SERIES_STORMLIGHT = 90,
}

/// Replace every `@FIXTURE_NAME` in a ref with the fixture's UUID.
pub fn expand(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('@') {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 1..];
        let end = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(tail.len());
        let name = &tail[..end];
        match id_by_name(name) {
            Some(uuid) => out.push_str(&uuid.to_string()),
            None => panic!("unknown fixture id @{name}"),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

#[allow(clippy::too_many_arguments)]
fn show(
    uuid: Uuid,
    title: &str,
    year: i32,
    genres: &[&str],
    network: &str,
    total: i32,
    available: i32,
    rating: Option<f64>,
    description: &str,
) -> TvShowDocument {
    TvShowDocument {
        id: uuid.to_string(),
        title: title.to_string(),
        year: Some(year),
        genres: genres.iter().map(|g| (*g).to_string()).collect(),
        poster_url: None,
        is_monitored: true,
        status: Some("continuing".to_string()),
        network: Some(network.to_string()),
        first_letter: crate::search::compute_first_letter(title),
        total_episodes: total,
        available_episode_count: available,
        rating,
        content_rating: None,
        runtime: Some(45),
        description: Some(description.to_string()),
        vectors: None,
    }
}

fn season(
    uuid: Uuid,
    show_id: Uuid,
    show_title: &str,
    number: i32,
    episodes: i32,
) -> TvSeasonDocument {
    TvSeasonDocument {
        id: uuid.to_string(),
        show_id: show_id.to_string(),
        show_title: show_title.to_string(),
        display_title: format!("{show_title} Season {number}"),
        season_title: None,
        season_identifier: number.to_string(),
        season_code: format!("S{number:02}"),
        poster_url: None,
        episode_count: episodes,
        is_monitored: true,
        is_special: false,
    }
}

#[allow(clippy::too_many_arguments)]
fn episode(
    uuid: Uuid,
    show_id: Uuid,
    season_id: Uuid,
    show_title: &str,
    season_number: i32,
    episode_number: i32,
    title: &str,
    air: Option<Date>,
) -> TvEpisodeDocument {
    let code = format!("S{season_number:02}E{episode_number:02}");
    TvEpisodeDocument {
        id: uuid.to_string(),
        show_id: show_id.to_string(),
        season_id: season_id.to_string(),
        show_title: show_title.to_string(),
        display_title: format!("{show_title} {code} {title}"),
        episode_title: Some(title.to_string()),
        episode_code: code,
        still_url: None,
        is_in_library: air.map(|d| ts(d) < now().unix_timestamp()).unwrap_or(false),
        want_status: "none".to_string(),
        air_date: air.map(iso),
        air_date_ts: air.map(ts),
        season_sort_order: season_number,
        episode_sort_order: episode_number,
        runtime: Some(45),
        description: Some(format!("{title}.")),
    }
}

#[allow(clippy::too_many_arguments)]
fn movie(
    uuid: Uuid,
    title: &str,
    year: i32,
    directors: &[&str],
    genres: &[&str],
    rating: f64,
    description: &str,
) -> MovieDocument {
    MovieDocument {
        id: uuid.to_string(),
        title: title.to_string(),
        year: Some(year),
        directors: directors.iter().map(|d| (*d).to_string()).collect(),
        genres: genres.iter().map(|g| (*g).to_string()).collect(),
        poster_url: None,
        is_in_library: true,
        want_status: "none".to_string(),
        first_letter: crate::search::compute_first_letter(title),
        rating: Some(rating),
        runtime: Some(120),
        content_rating: None,
        description: Some(description.to_string()),
        vectors: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn book(
    uuid: Uuid,
    title: &str,
    authors: &[(Uuid, &str)],
    series: Option<(Uuid, &str)>,
    has_ebook: bool,
    has_audiobook: bool,
    description: &str,
) -> BookDocument {
    BookDocument {
        id: uuid.to_string(),
        title: title.to_string(),
        authors: authors.iter().map(|(_, n)| (*n).to_string()).collect(),
        author_ids: authors.iter().map(|(i, _)| i.to_string()).collect(),
        series_name: series.map(|(_, n)| n.to_string()),
        series_id: series.map(|(i, _)| i.to_string()),
        series_index: None,
        genres: vec!["Fantasy".to_string()],
        cover_url: None,
        is_in_library: true,
        want_status: "none".to_string(),
        first_letter: crate::search::compute_first_letter(title),
        ingest_status: None,
        has_audiobook,
        has_ebook,
        narrator: None,
        all_series_names: series.map(|(_, n)| n.to_string()),
        description: Some(description.to_string()),
        vectors: None,
    }
}

const SOMESHOW: &str = "someShow";
const BAKEOFF: &str = "The Great British Bake Off";
const BSG: &str = "Battlestar Galactica";
const F1: &str = "Formula 1";

/// The whole corpus, as the search indexes would hold it.
pub fn corpus() -> IndexHits {
    let shows = vec![
        show(
            SHOW_SOMESHOW,
            SOMESHOW,
            2001,
            &["Drama"],
            "KOB",
            11,
            10,
            Some(7.4),
            "A long-running drama about a family and the business they refuse to sell.",
        ),
        show(
            SHOW_BAKEOFF,
            BAKEOFF,
            2010,
            &["Reality"],
            "Channel 4",
            2,
            2,
            Some(8.0),
            "Amateur bakers gather in a tent in the English countryside for a baking competition, one challenge each week.",
        ),
        show(
            SHOW_BSG_1978,
            BSG,
            1978,
            &["Science Fiction"],
            "ABC",
            0,
            0,
            None,
            "A ragtag fleet flees the Cylons in search of Earth.",
        ),
        show(
            SHOW_BSG_2004,
            BSG,
            2004,
            &["Science Fiction"],
            "Sci Fi",
            0,
            0,
            None,
            "The reimagined fleet flees the Cylons in search of Earth.",
        ),
        show(
            SHOW_F1,
            F1,
            1950,
            &["Sport"],
            "FOM",
            2,
            2,
            None,
            "Motor racing's world championship, round by round.",
        ),
    ];

    let seasons = vec![
        season(SEASON_SOMESHOW_01, SHOW_SOMESHOW, SOMESHOW, 1, 2),
        season(SEASON_SOMESHOW_24, SHOW_SOMESHOW, SOMESHOW, 24, 9),
        season(SEASON_BAKEOFF_12, SHOW_BAKEOFF, BAKEOFF, 12, 2),
        season(SEASON_F1_2026, SHOW_F1, F1, 2026, 2),
    ];

    let s24 = |uuid: Uuid, n: i32, title: &str, air: Date| {
        episode(
            uuid,
            SHOW_SOMESHOW,
            SEASON_SOMESHOW_24,
            SOMESHOW,
            24,
            n,
            title,
            Some(air),
        )
    };

    let episodes = vec![
        episode(
            EP_SOMESHOW_S01E01,
            SHOW_SOMESHOW,
            SEASON_SOMESHOW_01,
            SOMESHOW,
            1,
            1,
            "Pilot",
            Some(date(2001, Month::September, 10)),
        ),
        episode(
            EP_SOMESHOW_S01E02,
            SHOW_SOMESHOW,
            SEASON_SOMESHOW_01,
            SOMESHOW,
            1,
            2,
            "Groundwork",
            Some(date(2001, Month::September, 17)),
        ),
        s24(
            EP_SOMESHOW_S24E01,
            1,
            "Opening Offer",
            date(2026, Month::June, 28),
        ),
        s24(EP_SOMESHOW_S24E02, 2, "Counter", date(2026, Month::July, 5)),
        s24(EP_SOMESHOW_S24E03, 3, "Escrow", date(2026, Month::July, 12)),
        s24(
            EP_SOMESHOW_S24E04,
            4,
            "Diligence",
            date(2026, Month::July, 19),
        ),
        s24(
            EP_SOMESHOW_S24E05,
            5,
            "Handshake",
            date(2026, Month::July, 26),
        ),
        s24(
            EP_SOMESHOW_S24E06,
            6,
            "Clawback",
            date(2026, Month::August, 9),
        ),
        s24(
            EP_SOMESHOW_S24E07,
            7,
            "The Draft",
            date(2026, Month::August, 16),
        ),
        s24(
            EP_SOMESHOW_S24E08,
            8,
            "Signing Day",
            date(2026, Month::August, 23),
        ),
        s24(
            EP_SOMESHOW_S24E09,
            9,
            "Closing",
            date(2026, Month::August, 29),
        ),
        episode(
            EP_BAKEOFF_S12E01,
            SHOW_BAKEOFF,
            SEASON_BAKEOFF_12,
            BAKEOFF,
            12,
            1,
            "Cake Week",
            Some(date(2026, Month::July, 8)),
        ),
        episode(
            EP_BAKEOFF_S12E02,
            SHOW_BAKEOFF,
            SEASON_BAKEOFF_12,
            BAKEOFF,
            12,
            2,
            "Biscuit Week",
            Some(date(2026, Month::July, 15)),
        ),
        episode(
            EVENT_SPA,
            SHOW_F1,
            SEASON_F1_2026,
            F1,
            2026,
            7,
            "Belgian Grand Prix",
            Some(date(2026, Month::August, 6)),
        ),
        episode(
            EVENT_MONACO,
            SHOW_F1,
            SEASON_F1_2026,
            F1,
            2026,
            8,
            "Monaco Grand Prix",
            Some(date(2026, Month::August, 20)),
        ),
    ];

    let movies = vec![
        movie(
            MOVIE_DUNE_2021,
            "Dune",
            2021,
            &["Denis Villeneuve"],
            &["Science Fiction"],
            7.8,
            "A noble family is drawn into a war over the galaxy's most valuable resource.",
        ),
        movie(
            MOVIE_DUNE_1984,
            "Dune",
            1984,
            &["David Lynch"],
            &["Science Fiction"],
            6.3,
            "A duke's son leads desert warriors against the galactic emperor.",
        ),
        movie(
            MOVIE_GREAT_ESCAPE,
            "The Great Escape",
            1963,
            &["John Sturges"],
            &["War"],
            8.2,
            "Allied prisoners plan a mass breakout from a German prison camp.",
        ),
        movie(
            MOVIE_ARRIVAL,
            "Arrival",
            2016,
            &["Denis Villeneuve"],
            &["Science Fiction"],
            7.6,
            "A linguist is recruited to work out what twelve visiting craft want.",
        ),
    ];

    let sanderson = (AUTHOR_SANDERSON, "Brandon Sanderson");
    let stormlight = (SERIES_STORMLIGHT, "The Stormlight Archive");

    let books = vec![
        book(
            BOOK_DUNE,
            "Dune",
            &[(AUTHOR_HERBERT, "Frank Herbert")],
            None,
            true,
            true,
            "A noble family is drawn into a war over the galaxy's most valuable resource.",
        ),
        book(
            BOOK_WAY_OF_KINGS,
            "The Way of Kings",
            &[sanderson],
            Some(stormlight),
            true,
            true,
            "Roshar's storms and the war on the Shattered Plains.",
        ),
        book(
            BOOK_RHYTHM_OF_WAR,
            "Rhythm of War",
            &[sanderson],
            Some(stormlight),
            true,
            false,
            "The war for Roshar turns on a siege at Urithiru.",
        ),
    ];

    let authors = vec![
        AuthorDocument {
            id: AUTHOR_SANDERSON.to_string(),
            name: "Brandon Sanderson".to_string(),
            image_url: None,
            book_count: 2,
            is_monitored: true,
        },
        AuthorDocument {
            id: AUTHOR_HERBERT.to_string(),
            name: "Frank Herbert".to_string(),
            image_url: None,
            book_count: 1,
            is_monitored: false,
        },
    ];

    let series = vec![SeriesDocument {
        id: SERIES_STORMLIGHT.to_string(),
        name: "The Stormlight Archive".to_string(),
        book_count: 2,
        is_monitored: true,
    }];

    IndexHits {
        books,
        movies,
        shows,
        seasons,
        episodes,
        authors,
        series,
    }
}

/// `tv_shows.content_category`, which is not in any search document.
pub fn content_categories() -> HashMap<Uuid, String> {
    HashMap::from([
        (SHOW_SOMESHOW, "tv".to_string()),
        (SHOW_BAKEOFF, "tv".to_string()),
        (SHOW_BSG_1978, "tv".to_string()),
        (SHOW_BSG_2004, "tv".to_string()),
        (SHOW_F1, "sport".to_string()),
    ])
}
