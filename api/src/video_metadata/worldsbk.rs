//! WorldSBK's own results API, the pulselive platform behind worldsbk.com.
//! Unauthenticated and undocumented, JSON:API shaped: the sponsored round
//! names, circuits, their towns and every session's start, for every class
//! on the weekend.

use serde::{Deserialize, Serialize};

use super::VideoProviderError;

pub const BASE_URL: &str = "https://api.wsbk.pulselive.com";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SbkRound {
    pub id: String,
    pub code: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub place: Option<String>,
    pub country: Option<String>,
    pub start_date: String,
    pub end_date: String,
    pub circuit: Option<SbkCircuit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SbkCircuit {
    pub name: Option<String>,
    pub description: Option<String>,
    pub locality: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SbkSession {
    pub id: String,
    pub category: String,
    pub short_name: Option<String>,
    pub name: Option<String>,
    pub start: Option<String>,
}

#[derive(Deserialize)]
struct Document<A> {
    data: Vec<Resource<A>>,
    #[serde(default)]
    included: Vec<Resource<serde_json::Value>>,
}

#[derive(Deserialize)]
struct Resource<A> {
    #[serde(rename = "type")]
    kind: String,
    id: String,
    attributes: A,
    #[serde(default)]
    relationships: Relationships,
}

#[derive(Deserialize, Default)]
struct Relationships {
    circuit: Option<Link>,
    category: Option<Link>,
}

#[derive(Deserialize)]
struct Link {
    data: Option<Reference>,
}

#[derive(Deserialize)]
struct Reference {
    id: String,
}

impl Relationships {
    fn id(link: &Option<Link>) -> Option<&str> {
        link.as_ref()?.data.as_ref().map(|r| r.id.as_str())
    }
}

#[derive(Deserialize)]
struct RoundAttributes {
    source_id: String,
    name: Option<String>,
    description: Option<String>,
    brief_description: Option<String>,
    country_iso: Option<String>,
    start_date: String,
    end_date: String,
}

#[derive(Deserialize)]
struct CircuitAttributes {
    name: Option<String>,
    description: Option<String>,
    locality: Option<String>,
}

#[derive(Deserialize)]
struct SessionAttributes {
    short_name: Option<String>,
    brief_description: Option<String>,
    start_date_utc: Option<String>,
}

fn unreadable(e: serde_json::Error) -> VideoProviderError {
    VideoProviderError::Other(format!("WorldSBK response unreadable: {e}"))
}

fn text(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|v| !v.is_empty())
}

pub fn parse_rounds(body: &str) -> Result<Vec<SbkRound>, VideoProviderError> {
    let doc: Document<RoundAttributes> = serde_json::from_str(body).map_err(unreadable)?;
    let circuit = |id: &str| {
        let found = doc
            .included
            .iter()
            .find(|r| r.kind == "circuits" && r.id == id)?;
        let a: CircuitAttributes = serde_json::from_value(found.attributes.clone()).ok()?;
        Some(SbkCircuit {
            name: text(a.name),
            description: text(a.description),
            locality: text(a.locality),
        })
    };
    Ok(doc
        .data
        .iter()
        .map(|r| {
            let a = &r.attributes;
            SbkRound {
                id: r.id.clone(),
                code: a.source_id.clone(),
                name: text(a.name.clone()),
                description: text(a.description.clone()),
                place: text(a.brief_description.clone()),
                country: text(a.country_iso.clone()),
                start_date: a.start_date.clone(),
                end_date: a.end_date.clone(),
                circuit: Relationships::id(&r.relationships.circuit).and_then(circuit),
            }
        })
        .collect())
}

pub fn parse_sessions(body: &str) -> Result<Vec<SbkSession>, VideoProviderError> {
    let doc: Document<SessionAttributes> = serde_json::from_str(body).map_err(unreadable)?;
    Ok(doc
        .data
        .into_iter()
        .filter_map(|r| {
            let category = Relationships::id(&r.relationships.category)?.to_string();
            let a = r.attributes;
            Some(SbkSession {
                id: r.id,
                category,
                short_name: text(a.short_name),
                name: text(a.brief_description),
                start: a.start_date_utc,
            })
        })
        .collect())
}

/// A season or round the API does not know comes back as an empty list.
pub async fn fetch_rounds(
    client: &reqwest::Client,
    base_url: &str,
    year: u32,
) -> Result<Vec<SbkRound>, VideoProviderError> {
    let url = format!("{base_url}/wsbk-events/v1/seasons/{year}/rounds");
    parse_rounds(&crate::source_cache::get_text(client, "WorldSBK", &url).await?)
}

pub async fn fetch_sessions(
    client: &reqwest::Client,
    base_url: &str,
    year: u32,
    round_code: &str,
) -> Result<Vec<SbkSession>, VideoProviderError> {
    let url = format!("{base_url}/wsbk-events/v1/seasons/{year}/rounds/{round_code}/sessions");
    parse_sessions(&crate::source_cache::get_text(client, "WorldSBK", &url).await?)
}

pub fn round_on(rounds: &[SbkRound], date: time::Date) -> Option<&SbkRound> {
    let day = date.to_string();
    let day = day.as_str();
    rounds.iter().find(|r| {
        r.start_date.get(..10).is_some_and(|s| s <= day)
            && r.end_date.get(..10).is_some_and(|e| day <= e)
    })
}

pub fn category_of(class: &str) -> Option<&'static str> {
    match class {
        "SBK" => Some("SBK"),
        "WorldSSP" => Some("SSP"),
        _ => None,
    }
}

fn session_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

/// A red-flagged race is listed twice under one name, so the earliest start wins.
pub fn session_named<'s>(
    sessions: &'s [SbkSession],
    category: &str,
    session: &str,
) -> Option<&'s SbkSession> {
    let wanted = session_key(session);
    sessions
        .iter()
        .filter(|s| s.category == category)
        .filter(|s| {
            [&s.name, &s.short_name]
                .into_iter()
                .flatten()
                .any(|n| session_key(n) == wanted)
        })
        .min_by(|a, b| a.start.cmp(&b.start))
}

/// The FIM's three-letter codes WorldSBK uses differ from ISO 3166 alpha-3
/// for some hosts ("POR", "NED"), which would otherwise resolve to nothing.
pub fn country_code(code: &str) -> &str {
    match code {
        "GER" => "DE",
        "INA" => "ID",
        "MAS" => "MY",
        "NED" => "NL",
        "POR" => "PT",
        "RSA" => "ZA",
        "SUI" => "CH",
        other => other,
    }
}

#[cfg(test)]
pub(crate) const TEST_ROUNDS_2026: &str =
    include_str!("../../tests/fixtures/worldsbk/rounds_2026.json");
#[cfg(test)]
pub(crate) const TEST_SESSIONS_2026_JER: &str =
    include_str!("../../tests/fixtures/worldsbk/sessions_2026_jer.json");

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: time::Month, d: u8) -> time::Date {
        time::Date::from_calendar_date(y, m, d).unwrap()
    }

    #[test]
    fn rounds_carry_their_circuit() {
        let rounds = parse_rounds(TEST_ROUNDS_2026).unwrap();
        let jerez = rounds.iter().find(|r| r.code == "JER").unwrap();
        assert_eq!(jerez.id, "2026-JER");
        assert_eq!(jerez.description.as_deref(), Some("Pirelli Spanish Round"));
        assert_eq!(jerez.place.as_deref(), Some("Jerez"));
        assert_eq!(jerez.country.as_deref(), Some("ESP"));
        assert_eq!(
            jerez.circuit,
            Some(SbkCircuit {
                name: Some("Jerez".into()),
                description: Some("Circuito de Jerez - Ángel Nieto, Andalucía".into()),
                locality: Some("Jerez de la Frontera".into()),
            })
        );
    }

    #[test]
    fn rounds_are_matched_by_date() {
        let rounds = parse_rounds(TEST_ROUNDS_2026).unwrap();
        let on = |d| round_on(&rounds, d).map(|r| r.code.as_str());
        assert_eq!(on(date(2026, time::Month::October, 16)), Some("JER"));
        assert_eq!(on(date(2026, time::Month::October, 18)), Some("JER"));
        assert_eq!(on(date(2026, time::Month::May, 30)), Some("ARA"));
        assert_eq!(on(date(2026, time::Month::October, 19)), None);
    }

    #[test]
    fn sessions_are_found_by_either_name_within_their_class() {
        let sessions = parse_sessions(TEST_SESSIONS_2026_JER).unwrap();
        let start = |category, session| {
            session_named(&sessions, category, session).and_then(|s| s.start.as_deref())
        };
        assert_eq!(
            start("SBK", "Superpole Race"),
            Some("2026-10-18T09:10:00+00:00")
        );
        assert_eq!(start("SBK", "Race 2"), Some("2026-10-18T13:30:00+00:00"));
        assert_eq!(start("SBK", "FP2"), Some("2026-10-16T13:00:00+00:00"));
        assert_eq!(start("SBK", "Superpole"), Some("2026-10-17T09:15:00+00:00"));
        assert_eq!(start("SSP", "Race 2"), Some("2026-10-18T12:00:00+00:00"));
        assert_eq!(start("SSP", "Superpole Race"), None);
    }

    #[test]
    fn empty_answers_are_no_rounds_or_sessions() {
        assert_eq!(parse_rounds(r#"{"data":[]}"#).unwrap(), Vec::new());
        assert_eq!(parse_sessions(r#"{"data":[]}"#).unwrap(), Vec::new());
        assert!(parse_rounds(r#"{"errors":[]}"#).is_err());
    }

    #[test]
    fn fim_country_codes_become_iso() {
        assert_eq!(country_code("POR"), "PT");
        assert_eq!(country_code("NED"), "NL");
        assert_eq!(country_code("ESP"), "ESP");
    }
}
