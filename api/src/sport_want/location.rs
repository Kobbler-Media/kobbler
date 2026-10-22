use std::collections::HashMap;

use crate::repositories::tv_episodes::EpisodeRow;
use crate::repositories::tv_shows::TvShowRow;
use crate::video_metadata::thesportsdb::{SportEventDetails, parse_sport_event_details};

pub fn league_name(show: &TvShowRow) -> String {
    show.sport_metadata
        .as_ref()
        .and_then(|m| m.get("search_title"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| show.title.clone())
}

pub fn event_details(show: &TvShowRow, episode: &EpisodeRow) -> SportEventDetails {
    let location_aliases: HashMap<String, String> = show
        .sport_metadata
        .as_ref()
        .and_then(|m| m.get("location_aliases"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let mut details = parse_sport_event_details(
        episode.title.as_deref(),
        &episode.episode_identifier,
        episode.episode_sort_order,
    );

    // Drop a location that cannot help a search, BEFORE the country
    // fallback below consults it. Legacy title parsing leaves trailing
    // separators and sponsor text ("Prosecco DOC UK Round -"), and the
    // fallback paths emit the literal "Unknown"; either one poisons both
    // the search term and the match target, where a meaningless fragment
    // barely dents the fuzzy score and lets any same-league release
    // through — that is how a preseason Buriram test was grabbed four
    // times against an Aragon race want. Sanitising first is also what
    // lets a useless title parse fall through to the provider country
    // rather than blocking it.
    details.location = sanitize_location(details.location);

    // Use the provider's structured country only as a FALLBACK for a
    // title parse that produced nothing — never as an override.
    //
    // TheSportsDB's `strCountry` is the country that HOSTS the round,
    // which is not the name the round is known by and not the name
    // release groups use. Several rounds a season share one host
    // country, so letting it win collapses them onto each other:
    // San Marino (Misano) and Italy (Mugello) are both "IT", and
    // Spain/Catalonia/Aragon/Valencia are all "ES". A want built for
    // round 14 San Marino then matched `MotoGP.2026.Italy.Sprint` from
    // round 7 back in May and re-grabbed the whole Mugello weekend.
    //
    // The event title always carries the distinguishing name
    // ("San Marino - Sprint Race"), and that is what scene groups name
    // files after, so the title parse is the better source whenever it
    // yields anything. The country still helps where the title parse
    // gives up — it is a real place name, just a coarse one.
    if details.location.is_none()
        && let Some(country) = episode.country.as_deref().map(str::trim)
        && !country.is_empty()
    {
        if looks_like_country_code(country) {
            // An unrecognised code stays dropped: a bare two-letter
            // fragment ("MotoGP 2026 ES Sprint") matches no release.
            if let Some(expanded) = country_code_to_release_name(country) {
                details.location = Some(expanded.to_string());
            }
        } else {
            details.location = Some(country.to_string());
        }
    }

    if let Some(ref loc) = details.location {
        details.location = Some(normalize_country_for_releases(loc).to_string());
    }

    if let Some(ref loc) = details.location
        && let Some(alias) = location_aliases.get(loc)
    {
        details.location = Some(alias.clone());
    }

    // Re-sanitise: normalization and user aliases can reintroduce empty
    // or junk values. With no location the motorsport branch is skipped
    // entirely and matching falls back to targets that at least demand
    // the event title.
    details.location = sanitize_location(details.location);

    details
}

/// Releases use abbreviated names ("USA", "UK") while TheSportsDB uses full names.
fn normalize_country_for_releases(country: &str) -> &str {
    match country {
        "United States" | "United States of America" | "Americas" => "USA",
        "United Kingdom" | "Great Britain" => "UK",
        "The Netherlands" => "Netherlands",
        _ => country,
    }
}

/// Expand a two-letter ISO-3166 country code to the English name scene releases
/// use. TheSportsDB's `strCountry` is a code for motorsport events ("ES", "DE"),
/// and a code is useless as a search term — no release is named
/// "MotoGP.2026.ES.Sprint.Race". Returns `None` for anything that is not a code
/// we know, so callers can fall back to the title-parsed location.
///
/// Only the countries that host rounds are listed; extend as calendars change.
fn country_code_to_release_name(code: &str) -> Option<&'static str> {
    let name = match code.to_ascii_uppercase().as_str() {
        "AR" => "Argentina",
        "AT" => "Austria",
        "AU" => "Australia",
        "AE" => "UAE",
        "BE" => "Belgium",
        "BR" => "Brazil",
        "CZ" => "Czechia",
        "DE" => "Germany",
        "ES" => "Spain",
        "FI" => "Finland",
        "FR" => "France",
        "GB" | "UK" => "UK",
        "HU" => "Hungary",
        "ID" => "Indonesia",
        "IT" => "Italy",
        "JP" => "Japan",
        "MY" => "Malaysia",
        "NL" => "Netherlands",
        "PT" => "Portugal",
        "QA" => "Qatar",
        "SA" => "Saudi Arabia",
        "SG" => "Singapore",
        "TH" => "Thailand",
        "TR" => "Turkey",
        "US" | "USA" => "USA",
        "ZA" => "South Africa",
        _ => return None,
    };
    Some(name)
}

/// Strip trailing separators from a location and drop it entirely if what
/// remains cannot help a search (empty, or the literal "Unknown" that the
/// legacy parse paths emit).
fn sanitize_location(location: Option<String>) -> Option<String> {
    location.and_then(|loc| {
        let cleaned = loc.trim().trim_end_matches(['-', ',', ':']).trim();
        if cleaned.is_empty() || cleaned.eq_ignore_ascii_case("unknown") {
            None
        } else {
            Some(cleaned.to_string())
        }
    })
}

fn looks_like_country_code(value: &str) -> bool {
    let v = value.trim();
    v.len() == 2 && v.chars().all(|c| c.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TheSportsDB returns `strCountry` as an ISO code for motorsport. Letting it
    /// through unexpanded produced wants searching "MotoGP 2026 ES Sprint Race",
    /// which matches no release — every scene name uses the country in English.
    #[test]
    fn expands_iso_country_codes_to_release_names() {
        assert_eq!(country_code_to_release_name("ES"), Some("Spain"));
        assert_eq!(country_code_to_release_name("DE"), Some("Germany"));
        assert_eq!(country_code_to_release_name("CZ"), Some("Czechia"));
        // Case-insensitive, since provider casing is not guaranteed.
        assert_eq!(country_code_to_release_name("es"), Some("Spain"));
        // Both spellings of the UK collapse to the form releases use.
        assert_eq!(country_code_to_release_name("GB"), Some("UK"));
        assert_eq!(country_code_to_release_name("UK"), Some("UK"));
    }

    /// An unknown code must return None so the caller keeps the title-parsed
    /// location rather than searching for a two-letter fragment.
    #[test]
    fn leaves_unknown_codes_unexpanded() {
        assert_eq!(country_code_to_release_name("ZZ"), None);
        assert_eq!(country_code_to_release_name("XX"), None);
    }

    #[test]
    fn recognises_country_codes_by_shape() {
        assert!(looks_like_country_code("ES"));
        assert!(looks_like_country_code(" de "));
        // Real place names must not be mistaken for codes and dropped.
        assert!(!looks_like_country_code("Spain"));
        assert!(!looks_like_country_code("Aragon"));
        assert!(!looks_like_country_code("USA"));
        assert!(!looks_like_country_code(""));
        assert!(!looks_like_country_code("E1"));
    }

    #[test]
    fn normalises_full_country_names() {
        assert_eq!(normalize_country_for_releases("United Kingdom"), "UK");
        assert_eq!(normalize_country_for_releases("United States"), "USA");
        assert_eq!(normalize_country_for_releases("Czechia"), "Czechia");
    }

    #[test]
    fn sanitize_location_drops_unusable_values() {
        assert_eq!(sanitize_location(None), None);
        assert_eq!(sanitize_location(Some("Unknown".into())), None);
        assert_eq!(sanitize_location(Some("unknown".into())), None);
        assert_eq!(sanitize_location(Some("   ".into())), None);
        assert_eq!(sanitize_location(Some("-".into())), None);
        // Trailing separators left by the session-suffix strip are removed.
        assert_eq!(
            sanitize_location(Some("Netherlands GP -".into())),
            Some("Netherlands GP".into())
        );
        assert_eq!(
            sanitize_location(Some("San Marino -".into())),
            Some("San Marino".into())
        );
        assert_eq!(
            sanitize_location(Some("Aragon".into())),
            Some("Aragon".into())
        );
    }

    /// The regression this file's country-fallback ordering exists to prevent.
    ///
    /// TheSportsDB gives San Marino (Misano, round 14) and Italy (Mugello,
    /// round 7) the same `strCountry` of "IT". While the provider country
    /// overrode the title parse, both wants were built with location "Italy",
    /// so the round 14 want matched `MotoGP.2026.Italy.Sprint` — a round 7
    /// release from May — and re-grabbed the entire Mugello weekend in
    /// September, days before San Marino had even been run.
    #[test]
    fn title_parse_beats_provider_country_for_shared_host_countries() {
        // Both of these events carry country "IT" upstream.
        let san_marino = parse_sport_event_details(
            Some("San Marino - Sprint Race"),
            "evt-r14-sprint-race",
            14005,
        );
        let italy =
            parse_sport_event_details(Some("Italy - Sprint Race"), "evt-r7-sprint-race", 7005);

        // The distinguishing name has to survive, or the two rounds collapse.
        assert_eq!(
            sanitize_location(san_marino.location),
            Some("San Marino".into())
        );
        assert_eq!(sanitize_location(italy.location), Some("Italy".into()));
        assert_eq!(san_marino.round, Some(14));
        assert_eq!(italy.round, Some(7));
    }

    /// Spain hosts up to five rounds a season, all reported as "ES". The title
    /// is the only field that separates them.
    #[test]
    fn title_parse_separates_the_spanish_rounds() {
        let cases = [
            (
                "Spain - Free Practice 1",
                "evt-r4-free-practice-1",
                4000,
                "Spain",
                4,
            ),
            (
                "Catalonia - Free Practice 1",
                "evt-r6-free-practice-1",
                6000,
                "Catalonia",
                6,
            ),
            (
                "Aragón - Free Practice 1",
                "evt-r13-free-practice-1",
                13000,
                "Aragón",
                13,
            ),
            (
                "Valencia - Free Practice 1",
                "evt-r22-free-practice-1",
                22000,
                "Valencia",
                22,
            ),
        ];
        for (title, ident, ord, want_loc, want_round) in cases {
            let d = parse_sport_event_details(Some(title), ident, ord);
            assert_eq!(
                sanitize_location(d.location).as_deref(),
                Some(want_loc),
                "location for {title}"
            );
            assert_eq!(d.round, Some(want_round), "round for {title}");
        }
    }
}
