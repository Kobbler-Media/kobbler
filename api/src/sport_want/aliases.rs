use nats_jobs::{SportEventMeta, SportIdentifiers};

use super::gazetteer::{self, fold};

/// Words that describe what a venue is rather than which one it is. Splitting a
/// venue's name on them leaves the words releases name it by: "Circuito de
/// Jerez – Ángel Nieto" gives "Jerez", "Misano World Circuit Marco Simoncelli"
/// gives "Misano".
const VENUE_FILLER: &[&str] = &[
    "autodrom",
    "autodromo",
    "automotodrom",
    "circuit",
    "circuito",
    "grand",
    "internacional",
    "international",
    "internazionale",
    "motorland",
    "park",
    "prix",
    "street",
    "world",
];

const PARTICLES: &[&str] = &[
    "and", "da", "de", "del", "della", "des", "di", "do", "du", "e", "la", "le", "of", "the", "von",
];

const VENUE_NICKNAMES: &[(&str, &[&str])] = &[("circuit of the americas", &["COTA"])];

const MIN_CHUNK_CHARS: usize = 3;

#[derive(Debug, Default)]
pub struct Gathered {
    pub venues: Vec<String>,
    pub cities: Vec<String>,
    pub places: Vec<String>,
    pub country_iso: Option<String>,
    pub event_aliases: Vec<String>,
    pub league_aliases: Vec<String>,
    pub session_start: Option<String>,
    pub ids: SportIdentifiers,
}

impl Gathered {
    pub fn country_from(&mut self, value: Option<&str>) {
        if self.country_iso.is_some() {
            return;
        }
        let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
            return;
        };
        let found = if value.len() == 2 {
            gazetteer::country(value)
        } else {
            gazetteer::country_by_name(value)
        };
        self.country_iso = found.map(|c| c.iso2.to_string());
    }
}

/// Some feeds shout ("GRAND PRIX OF SAN MARINO").
pub fn tidy(name: &str) -> Option<String> {
    let collapsed = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    let shouting = collapsed.chars().filter(|c| c.is_alphabetic()).count() > 3
        && !collapsed.chars().any(char::is_lowercase);
    if !shouting {
        return Some(collapsed);
    }
    Some(
        collapsed
            .split(' ')
            .map(|word| {
                let lower = word.to_lowercase();
                let mut chars = lower.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().chain(chars).collect())
                    .unwrap_or_default()
            })
            .collect::<Vec<String>>()
            .join(" "),
    )
}

fn chunks(name: &str, splitters: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut flush = |current: &mut Vec<&str>| {
        let chunk = current.join(" ");
        if chunk.chars().count() >= MIN_CHUNK_CHARS && !PARTICLES.contains(&fold(&chunk).as_str()) {
            out.push(chunk);
        }
        current.clear();
    };
    for piece in name.split(['-', '–', '—', ',', '/', '(', ')']) {
        for word in piece.split_whitespace() {
            let folded = fold(word);
            if splitters.contains(&folded.as_str()) || PARTICLES.contains(&folded.as_str()) {
                flush(&mut current);
            } else {
                current.push(word);
            }
        }
        flush(&mut current);
    }
    out
}

pub fn venue_names(venue: &str) -> Vec<String> {
    let mut names = vec![venue.to_string()];
    let folded = fold(venue);
    for (full, nicknames) in VENUE_NICKNAMES {
        if folded == *full {
            names.extend(nicknames.iter().map(|n| n.to_string()));
        }
    }
    names.extend(
        chunks(venue, VENUE_FILLER)
            .into_iter()
            .filter(|c| c != venue),
    );
    names
}

/// "Montmeló, Barcelona" is a town and its province; "Jerez de la Frontera" is
/// known as Jerez.
pub fn city_names(city: &str) -> Vec<String> {
    city.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .flat_map(|part| {
            std::iter::once(part.to_string())
                .chain(chunks(part, &[]).into_iter().filter(move |c| c != part))
        })
        .collect()
}

#[derive(Default)]
struct AliasList {
    seen: std::collections::HashSet<String>,
    names: Vec<String>,
}

impl AliasList {
    fn push(&mut self, name: &str) {
        let name = name.trim();
        if !name.is_empty() && self.seen.insert(fold(name)) {
            self.names.push(name.to_string());
        }
    }

    fn into_option(self) -> Option<Vec<String>> {
        (!self.names.is_empty()).then_some(self.names)
    }
}

pub fn finish(
    gathered: Gathered,
    location: Option<&str>,
    league_name: &str,
    class: Option<&str>,
) -> SportEventMeta {
    let country = gathered.country_iso.as_deref().and_then(gazetteer::country);

    let mut location_aliases = AliasList::default();
    if let Some(location) = location {
        location_aliases.push(location);
    }
    for venue in &gathered.venues {
        for name in venue_names(venue) {
            location_aliases.push(&name);
        }
    }
    let mut city_parts = Vec::new();
    for city in &gathered.cities {
        for name in city_names(city) {
            location_aliases.push(&name);
            city_parts.push(name);
        }
    }
    for place in &gathered.places {
        location_aliases.push(place);
    }
    let region = country.and_then(|c| {
        city_parts
            .iter()
            .find_map(|part| gazetteer::region(part, c.iso2))
    });
    if let Some(region) = region {
        location_aliases.push(region);
    }
    if let Some(country) = country {
        for name in gazetteer::country_aliases(country) {
            location_aliases.push(name);
        }
    }

    let mut event_aliases = AliasList::default();
    for name in &gathered.event_aliases {
        event_aliases.push(name);
    }

    let mut league_aliases = AliasList::default();
    league_aliases.seen.insert(fold(league_name.trim()));
    for name in &gathered.league_aliases {
        league_aliases.push(name);
    }

    let ids = gathered.ids;
    let has_ids =
        ids.thesportsdb_id.is_some() || ids.sportarr_id.is_some() || ids.official_id.is_some();

    SportEventMeta {
        venue: gathered.venues.first().cloned(),
        city: gathered
            .cities
            .first()
            .and_then(|c| c.split(',').next())
            .map(|c| c.trim().to_string()),
        region: region.map(String::from),
        country: country.map(|c| c.name.to_string()),
        country_iso: country.map(|c| c.iso2.to_string()),
        location_aliases: location_aliases.into_option(),
        event_aliases: event_aliases.into_option(),
        league_aliases: league_aliases.into_option(),
        teams: None,
        session_start: gathered.session_start,
        class: class.map(String::from),
        identifiers: has_ids.then_some(ids),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn venues_yield_the_word_releases_use() {
        let cases: &[(&str, &[&str])] = &[
            ("Circuit of the Americas", &["COTA", "Americas"]),
            ("Circuito de Jerez – Ángel Nieto", &["Jerez", "Ángel Nieto"]),
            (
                "Misano World Circuit Marco Simoncelli",
                &["Misano", "Marco Simoncelli"],
            ),
            ("Phillip Island Grand Prix Circuit", &["Phillip Island"]),
            ("Algarve International Circuit", &["Algarve"]),
            ("Autódromo Internacional do Algarve", &["Algarve"]),
            ("TT Circuit Assen", &["Assen"]),
            ("Donington Park", &["Donington"]),
            ("Balaton Park Circuit", &["Balaton"]),
            ("Autodrom Most", &["Most"]),
            ("MotorLand Aragón", &["Aragón"]),
            (
                "Circuit de Barcelona-Catalunya",
                &["Barcelona", "Catalunya"],
            ),
            ("Autodromo Internazionale del Mugello", &["Mugello"]),
            ("Cremona Circuit", &["Cremona"]),
        ];
        for (venue, wants) in cases {
            let names = venue_names(venue);
            assert_eq!(names[0], *venue);
            for want in *wants {
                assert!(names.iter().any(|n| n == want), "{want} not in {names:?}");
            }
        }
        assert_eq!(venue_names("Sachsenring"), vec!["Sachsenring"]);
    }

    #[test]
    fn cities_split_into_town_and_province() {
        assert_eq!(
            city_names("Montmeló, Barcelona"),
            vec!["Montmeló", "Barcelona"]
        );
        assert_eq!(
            city_names("Jerez de la Frontera"),
            vec!["Jerez de la Frontera", "Jerez", "Frontera"]
        );
        assert_eq!(city_names("Teruel,Aragon"), vec!["Teruel", "Aragon"]);
    }

    #[test]
    fn tidy_quietens_shouting_and_keeps_codes() {
        assert_eq!(
            tidy("GRAND PRIX OF SAN MARINO ").as_deref(),
            Some("Grand Prix Of San Marino")
        );
        assert_eq!(tidy("USA").as_deref(), Some("USA"));
        assert_eq!(
            tidy("Red Bull Grand Prix of The United States").as_deref(),
            Some("Red Bull Grand Prix of The United States")
        );
        assert_eq!(tidy("  "), None);
    }

    #[test]
    fn finish_copes_with_a_blank_league_name() {
        let gathered = Gathered {
            league_aliases: vec!["WSBK".into()],
            ..Default::default()
        };
        assert_eq!(
            finish(gathered, None, " ", None).league_aliases,
            Some(vec!["WSBK".into()])
        );
        assert_eq!(
            finish(Gathered::default(), None, "", None).league_aliases,
            None
        );
    }

    #[test]
    fn finish_drops_the_league_name_from_its_own_aliases() {
        let gathered = Gathered {
            league_aliases: vec![
                "SBK".into(),
                "WSB".into(),
                "WSBK".into(),
                "World Superbike".into(),
            ],
            ..Default::default()
        };
        let meta = finish(gathered, None, "WSBK", Some("SBK"));
        assert_eq!(
            meta.league_aliases,
            Some(vec!["SBK".into(), "WSB".into(), "World Superbike".into()])
        );
        assert_eq!(meta.location_aliases, None);
        assert!(meta.identifiers.is_none());
    }
}
