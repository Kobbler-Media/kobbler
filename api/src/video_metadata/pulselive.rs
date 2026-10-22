//! MotoGP's own results API (pulselive). Unauthenticated and undocumented; it
//! is the only source of the sponsored event names and circuit names for
//! MotoGP, Moto2 and Moto3, which Sportarr has no alternate names for.

use serde::{Deserialize, Serialize};

use super::VideoProviderError;

const BASE_URL: &str = "https://api.motogp.pulselive.com/motogp/v1/results";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MotoGpEvent {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub sponsored_name: Option<String>,
    #[serde(default)]
    pub additional_name: Option<String>,
    #[serde(default)]
    pub circuit: Option<MotoGpCircuit>,
    #[serde(default)]
    pub country: Option<MotoGpCountry>,
    pub date_start: String,
    pub date_end: String,
    #[serde(default)]
    pub test: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MotoGpCircuit {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub place: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MotoGpCountry {
    #[serde(default)]
    pub iso: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Deserialize)]
struct Season {
    id: String,
    year: u32,
}

async fn get<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> Result<T, VideoProviderError> {
    let resp = client.get(url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(VideoProviderError::Other(format!(
            "pulselive returned HTTP {status} for {url}"
        )));
    }
    resp.json().await.map_err(VideoProviderError::from)
}

pub async fn fetch_events(
    client: &reqwest::Client,
    year: u32,
) -> Result<Vec<MotoGpEvent>, VideoProviderError> {
    let seasons: Vec<Season> = get(client, &format!("{BASE_URL}/seasons")).await?;
    let season = seasons
        .into_iter()
        .find(|s| s.year == year)
        .ok_or(VideoProviderError::NotFound)?;
    get(
        client,
        &format!("{BASE_URL}/events?seasonUuid={}", season.id),
    )
    .await
}

/// The race weekend running on `date`. Tests share circuits and dates with
/// nothing a want is built for, so they never match.
pub fn event_on(events: &[MotoGpEvent], date: time::Date) -> Option<&MotoGpEvent> {
    let day = date.to_string();
    events.iter().find(|e| {
        !e.test && e.date_start.as_str() <= day.as_str() && day.as_str() <= e.date_end.as_str()
    })
}

#[cfg(test)]
pub(crate) const TEST_EVENTS_2026: &str = r#"[
    {"id":"782c929d-faaf-44e2-9d6f-0fde033855be","name":"GRAND PRIX OF THE UNITED STATES",
     "sponsored_name":"Red Bull Grand Prix of The United States","additional_name":"USA",
     "short_name":"USA","circuit":{"id":"5b97","name":"Circuit Of The Americas","place":"Austin","nation":"USA"},
     "country":{"iso":"US","name":"United States of America","region_iso":""},
     "date_start":"2026-03-27","date_end":"2026-03-29","test":false,"status":"FINISHED"},
    {"id":"t1","name":"BURIRAM TEST","sponsored_name":"BURIRAM TEST","additional_name":"BURIRAM TEST",
     "circuit":{"name":"Chang International Circuit","place":"Buriram"},
     "country":{"iso":"TH","name":"Thailand"},
     "date_start":"2026-02-21","date_end":"2026-02-22","test":true},
    {"id":"th","name":"GRAND PRIX OF THAILAND","sponsored_name":"PT Grand Prix of Thailand",
     "additional_name":"THAILAND","circuit":{"name":"Chang International Circuit","place":"Buriram"},
     "country":{"iso":"TH","name":"Thailand"},
     "date_start":"2026-02-27","date_end":"2026-03-01","test":false}
]"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn date(s: &str) -> time::Date {
        time::Date::parse(s, &time::format_description::well_known::Iso8601::DATE).unwrap()
    }

    #[test]
    fn events_parse_from_the_results_api() {
        let events: Vec<MotoGpEvent> = serde_json::from_str(TEST_EVENTS_2026).unwrap();
        let usa = &events[0];
        assert_eq!(
            usa.sponsored_name.as_deref(),
            Some("Red Bull Grand Prix of The United States")
        );
        assert_eq!(
            usa.circuit.as_ref().unwrap().name.as_deref(),
            Some("Circuit Of The Americas")
        );
        assert_eq!(usa.country.as_ref().unwrap().iso.as_deref(), Some("US"));
        assert!(events[1].test);
    }

    #[test]
    fn events_are_matched_by_date_and_tests_are_skipped() {
        let events: Vec<MotoGpEvent> = serde_json::from_str(TEST_EVENTS_2026).unwrap();
        assert_eq!(
            event_on(&events, date("2026-03-29")).unwrap().id,
            events[0].id
        );
        assert_eq!(
            event_on(&events, date("2026-03-27")).unwrap().id,
            events[0].id
        );
        assert_eq!(event_on(&events, date("2026-02-21")), None);
        assert_eq!(event_on(&events, date("2026-03-01")).unwrap().id, "th");
        assert_eq!(event_on(&events, date("2026-03-10")), None);
    }
}
