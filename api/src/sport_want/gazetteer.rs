use std::collections::HashMap;
use std::sync::LazyLock;

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

pub struct Country {
    pub iso2: &'static str,
    pub iso3: &'static str,
    pub name: &'static str,
}

/// Names release groups use for a country that GeoNames does not list.
const EXTRA_NAMES: &[(&str, &[&str])] = &[
    ("US", &["United States of America", "America"]),
    ("GB", &["UK", "Great Britain", "Britain"]),
    ("CZ", &["Czech Republic"]),
    ("NL", &["Netherlands", "Holland"]),
    ("AE", &["UAE"]),
    ("KR", &["Korea"]),
];

const DEMONYMS: &[(&str, &[&str])] = &[
    ("AE", &["Emirati"]),
    ("AR", &["Argentine", "Argentinian", "Argentinean"]),
    ("AT", &["Austrian"]),
    ("AU", &["Australian"]),
    ("AZ", &["Azerbaijani"]),
    ("BE", &["Belgian"]),
    ("BH", &["Bahraini"]),
    ("BR", &["Brazilian"]),
    ("CA", &["Canadian"]),
    ("CH", &["Swiss"]),
    ("CN", &["Chinese"]),
    ("CZ", &["Czech"]),
    ("DE", &["German"]),
    ("ES", &["Spanish"]),
    ("FI", &["Finnish"]),
    ("FR", &["French"]),
    ("GB", &["British"]),
    ("HR", &["Croatian"]),
    ("HU", &["Hungarian"]),
    ("ID", &["Indonesian"]),
    ("IN", &["Indian"]),
    ("IT", &["Italian"]),
    ("JP", &["Japanese"]),
    ("KR", &["Korean"]),
    ("KZ", &["Kazakh"]),
    ("MC", &["Monegasque"]),
    ("MX", &["Mexican"]),
    ("MY", &["Malaysian"]),
    ("NL", &["Dutch"]),
    ("PL", &["Polish"]),
    ("PT", &["Portuguese"]),
    ("QA", &["Qatari"]),
    ("SA", &["Saudi"]),
    ("SE", &["Swedish"]),
    ("SG", &["Singaporean"]),
    ("SI", &["Slovenian"]),
    ("SM", &["Sammarinese"]),
    ("TH", &["Thai"]),
    ("TR", &["Turkish"]),
    ("US", &["American"]),
    ("VN", &["Vietnamese"]),
    ("ZA", &["South African"]),
];

fn tsv_rows(data: &'static str) -> impl Iterator<Item = Vec<&'static str>> {
    data.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split('\t').collect())
}

static COUNTRIES: LazyLock<Vec<Country>> = LazyLock::new(|| {
    tsv_rows(include_str!("data/countries.tsv"))
        .filter_map(|r| match r[..] {
            [iso2, iso3, name] => Some(Country { iso2, iso3, name }),
            _ => None,
        })
        .collect()
});

static CITY_REGIONS: LazyLock<HashMap<(String, &'static str), &'static str>> =
    LazyLock::new(|| {
        tsv_rows(include_str!("data/cities.tsv"))
            .filter_map(|r| match r[..] {
                [city, iso2, region] => Some(((fold(city), iso2), region)),
                _ => None,
            })
            .collect()
    });

pub fn fold(s: &str) -> String {
    s.nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase()
}

pub fn country(iso2: &str) -> Option<&'static Country> {
    COUNTRIES
        .iter()
        .find(|c| c.iso2.eq_ignore_ascii_case(iso2.trim()))
}

pub fn country_by_name(name: &str) -> Option<&'static Country> {
    let wanted = fold(name.trim());
    COUNTRIES
        .iter()
        .find(|c| country_aliases(c).iter().any(|a| fold(a) == wanted))
}

pub fn region(city: &str, iso2: &str) -> Option<&'static str> {
    let iso2 = country(iso2)?.iso2;
    CITY_REGIONS.get(&(fold(city.trim()), iso2)).copied()
}

pub fn country_aliases(country: &Country) -> Vec<&'static str> {
    let listed = |table: &'static [(&'static str, &'static [&'static str])]| {
        table
            .iter()
            .filter(|(iso2, _)| *iso2 == country.iso2)
            .flat_map(|(_, names)| names.iter().copied())
    };
    std::iter::once(country.name)
        .chain(listed(EXTRA_NAMES))
        .chain([country.iso2, country.iso3])
        .chain(listed(DEMONYMS))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countries_carry_codes_names_and_demonyms() {
        let us = country("us").unwrap();
        assert_eq!((us.iso2, us.iso3, us.name), ("US", "USA", "United States"));
        let aliases = country_aliases(us);
        for want in ["United States", "USA", "US", "American", "America"] {
            assert!(aliases.contains(&want), "{want} missing from {aliases:?}");
        }
        assert!(country_aliases(country("GB").unwrap()).contains(&"UK"));
    }

    #[test]
    fn countries_are_found_by_any_of_their_names() {
        for (name, iso2) in [
            ("United Kingdom", "GB"),
            ("UK", "GB"),
            ("Czech Republic", "CZ"),
            ("Spanish", "ES"),
            ("ITA", "IT"),
            ("netherlands", "NL"),
        ] {
            assert_eq!(country_by_name(name).map(|c| c.iso2), Some(iso2), "{name}");
        }
        assert!(country_by_name("Catalonia").is_none());
    }

    #[test]
    fn cities_resolve_to_their_region_within_the_country() {
        assert_eq!(region("Austin", "US"), Some("Texas"));
        assert_eq!(region("Alcañiz", "ES"), Some("Aragon"));
        assert_eq!(region("Austin", "AU"), None);
        assert_eq!(region("Nowhere-on-Sea", "GB"), None);
    }
}
