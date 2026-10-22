//! Formula 1 calendars from Jolpica, the Ergast API's successor: circuit,
//! its town and country, and the start of every session of a weekend.

use serde::{Deserialize, Serialize};

use super::VideoProviderError;

const BASE_URL: &str = "https://api.jolpi.ca/ergast/f1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct F1Race {
    #[serde(rename = "raceName")]
    pub race_name: String,
    #[serde(rename = "Circuit")]
    pub circuit: F1Circuit,
    pub date: String,
    #[serde(default)]
    pub time: Option<String>,
    #[serde(rename = "FirstPractice", default)]
    pub first_practice: Option<F1Session>,
    #[serde(rename = "SecondPractice", default)]
    pub second_practice: Option<F1Session>,
    #[serde(rename = "ThirdPractice", default)]
    pub third_practice: Option<F1Session>,
    #[serde(rename = "Qualifying", default)]
    pub qualifying: Option<F1Session>,
    #[serde(rename = "Sprint", default)]
    pub sprint: Option<F1Session>,
    #[serde(rename = "SprintQualifying", alias = "SprintShootout", default)]
    pub sprint_qualifying: Option<F1Session>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct F1Circuit {
    #[serde(rename = "circuitName")]
    pub name: String,
    #[serde(rename = "Location")]
    pub location: F1Location,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct F1Location {
    #[serde(default)]
    pub locality: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct F1Session {
    pub date: String,
    #[serde(default)]
    pub time: Option<String>,
}

#[derive(Deserialize)]
struct Response {
    #[serde(rename = "MRData")]
    data: MrData,
}

#[derive(Deserialize)]
struct MrData {
    #[serde(rename = "RaceTable")]
    race_table: RaceTable,
}

#[derive(Deserialize)]
struct RaceTable {
    #[serde(rename = "Races", default)]
    races: Vec<F1Race>,
}

fn parse_races(body: &str) -> Result<Vec<F1Race>, VideoProviderError> {
    serde_json::from_str::<Response>(body)
        .map(|r| r.data.race_table.races)
        .map_err(|e| VideoProviderError::Other(format!("Jolpica response unreadable: {e}")))
}

pub async fn fetch_races(
    client: &reqwest::Client,
    year: u32,
) -> Result<Vec<F1Race>, VideoProviderError> {
    let url = format!("{BASE_URL}/{year}/races/?limit=100");
    let resp = client.get(&url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(VideoProviderError::Other(format!(
            "Jolpica returned HTTP {status} for {url}"
        )));
    }
    parse_races(&resp.text().await?)
}

impl F1Race {
    fn sessions(&self) -> impl Iterator<Item = &F1Session> {
        [
            &self.first_practice,
            &self.second_practice,
            &self.third_practice,
            &self.qualifying,
            &self.sprint,
            &self.sprint_qualifying,
        ]
        .into_iter()
        .flatten()
    }

    pub fn session_start(&self, session: &str) -> Option<String> {
        let s = session.to_lowercase();
        let (date, time) =
            if s.contains("sprint") && (s.contains("qualifying") || s.contains("shootout")) {
                self.sprint_qualifying
                    .as_ref()
                    .map(|x| (&x.date, &x.time))?
            } else if s.contains("sprint") {
                self.sprint.as_ref().map(|x| (&x.date, &x.time))?
            } else if s.contains("qualifying") {
                self.qualifying.as_ref().map(|x| (&x.date, &x.time))?
            } else if s.contains("practice") || s.starts_with("fp") {
                let practice = match s.chars().rev().find(char::is_ascii_digit) {
                    Some('2') => &self.second_practice,
                    Some('3') => &self.third_practice,
                    _ => &self.first_practice,
                };
                practice.as_ref().map(|x| (&x.date, &x.time))?
            } else if s.contains("race") || s.contains("grand prix") {
                (&self.date, &self.time)
            } else {
                return None;
            };
        time.as_ref().map(|t| format!("{date}T{t}"))
    }
}

pub fn race_on(races: &[F1Race], date: time::Date) -> Option<&F1Race> {
    let day = date.to_string();
    races.iter().find(|race| {
        let first = race
            .sessions()
            .map(|s| s.date.as_str())
            .chain([race.date.as_str()])
            .min()
            .unwrap_or(race.date.as_str());
        first <= day.as_str() && day.as_str() <= race.date.as_str()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RACES: &str = r#"{"MRData":{"RaceTable":{"season":"2026","Races":[
        {"season":"2026","round":"4","raceName":"Miami Grand Prix",
         "Circuit":{"circuitId":"miami","circuitName":"Miami International Autodrome",
                    "Location":{"lat":"25.9","long":"-80.2","locality":"Miami","country":"USA"}},
         "date":"2026-05-03","time":"20:00:00Z",
         "FirstPractice":{"date":"2026-05-01","time":"16:00:00Z"},
         "Qualifying":{"date":"2026-05-02","time":"20:00:00Z"},
         "Sprint":{"date":"2026-05-02","time":"16:00:00Z"},
         "SprintQualifying":{"date":"2026-05-01","time":"20:30:00Z"}},
        {"season":"2026","round":"5","raceName":"Canadian Grand Prix",
         "Circuit":{"circuitId":"villeneuve","circuitName":"Circuit Gilles Villeneuve",
                    "Location":{"locality":"Montreal","country":"Canada"}},
         "date":"2026-05-24",
         "FirstPractice":{"date":"2026-05-22","time":"17:30:00Z"},
         "SecondPractice":{"date":"2026-05-22","time":"21:00:00Z"}}
    ]}}}"#;

    fn date(s: &str) -> time::Date {
        time::Date::parse(s, &time::format_description::well_known::Iso8601::DATE).unwrap()
    }

    #[test]
    fn races_are_matched_to_their_weekend_by_date() {
        let races = parse_races(RACES).unwrap();
        assert_eq!(races[0].circuit.location.locality.as_deref(), Some("Miami"));
        for day in ["2026-05-01", "2026-05-02", "2026-05-03"] {
            assert_eq!(
                race_on(&races, date(day)).unwrap().race_name,
                "Miami Grand Prix"
            );
        }
        assert_eq!(
            race_on(&races, date("2026-05-22")).unwrap().race_name,
            "Canadian Grand Prix"
        );
        assert!(race_on(&races, date("2026-05-10")).is_none());
    }

    #[test]
    fn sessions_map_to_their_start() {
        let races = parse_races(RACES).unwrap();
        let miami = &races[0];
        for (session, start) in [
            ("Race", Some("2026-05-03T20:00:00Z")),
            ("Qualifying", Some("2026-05-02T20:00:00Z")),
            ("Sprint", Some("2026-05-02T16:00:00Z")),
            ("Sprint Race", Some("2026-05-02T16:00:00Z")),
            ("Sprint Qualifying", Some("2026-05-01T20:30:00Z")),
            ("Sprint Shootout", Some("2026-05-01T20:30:00Z")),
            ("Free Practice 1", Some("2026-05-01T16:00:00Z")),
            ("Free Practice 2", None),
            ("Warm Up", None),
        ] {
            assert_eq!(miami.session_start(session).as_deref(), start, "{session}");
        }
        assert_eq!(races[1].session_start("Race"), None);
        assert_eq!(
            races[1].session_start("Practice 2").as_deref(),
            Some("2026-05-22T21:00:00Z")
        );
    }

    #[test]
    fn an_unreadable_response_is_an_error() {
        assert!(parse_races("{}").is_err());
    }
}
