use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

const FALLBACK_REGION: &str = "US";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseDates {
    pub premiere: Option<String>,
    pub theatrical_limited: Option<String>,
    pub theatrical: Option<String>,
    pub digital: Option<String>,
    pub physical: Option<String>,
    pub tv: Option<String>,
}

pub type ReleaseDatesByRegion = BTreeMap<String, ReleaseDates>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegionalReleaseDates {
    pub region: String,
    #[serde(flatten)]
    pub dates: ReleaseDates,
}

impl ReleaseDates {
    fn slot(&mut self, release_type: u8) -> Option<&mut Option<String>> {
        match release_type {
            1 => Some(&mut self.premiere),
            2 => Some(&mut self.theatrical_limited),
            3 => Some(&mut self.theatrical),
            4 => Some(&mut self.digital),
            5 => Some(&mut self.physical),
            6 => Some(&mut self.tv),
            _ => None,
        }
    }

    fn earliest(&self) -> Option<&str> {
        [
            &self.premiere,
            &self.theatrical_limited,
            &self.theatrical,
            &self.digital,
            &self.physical,
            &self.tv,
        ]
        .into_iter()
        .filter_map(Option::as_deref)
        .min()
    }
}

fn iso_date(s: &str) -> Option<&str> {
    let date = s.get(..10)?;
    time::Date::parse(
        date,
        time::macros::format_description!("[year]-[month]-[day]"),
    )
    .ok()
    .map(|_| date)
}

pub fn by_region<'a>(
    entries: impl IntoIterator<Item = (&'a str, u8, &'a str)>,
) -> ReleaseDatesByRegion {
    let mut out = ReleaseDatesByRegion::new();
    for (region, release_type, date) in entries {
        let Some(date) = iso_date(date) else { continue };
        if ReleaseDates::default().slot(release_type).is_none() {
            continue;
        }
        let dates = out.entry(region.to_ascii_uppercase()).or_default();
        if let Some(slot) = dates.slot(release_type)
            && slot.as_deref().is_none_or(|current| date < current)
        {
            *slot = Some(date.to_string());
        }
    }
    out
}

pub fn select(by_region: &ReleaseDatesByRegion, region: &str) -> Option<RegionalReleaseDates> {
    let (region, dates) = by_region
        .get_key_value(region.to_ascii_uppercase().as_str())
        .or_else(|| by_region.get_key_value(FALLBACK_REGION))
        .or_else(|| {
            by_region
                .iter()
                .filter_map(|(r, d)| d.earliest().map(|e| (e, r, d)))
                .min_by_key(|&(earliest, ..)| earliest)
                .map(|(_, r, d)| (r, d))
        })?;
    Some(RegionalReleaseDates {
        region: region.clone(),
        dates: dates.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dates(pairs: &[(u8, &str)]) -> ReleaseDates {
        let map = by_region(pairs.iter().map(|&(t, d)| ("XX", t, d)));
        map.get("XX").cloned().unwrap_or_default()
    }

    #[test]
    fn maps_each_tmdb_type_to_its_field() {
        let date = || Some("2024-03-05".to_string());
        let cases = [
            (
                1,
                ReleaseDates {
                    premiere: date(),
                    ..Default::default()
                },
            ),
            (
                2,
                ReleaseDates {
                    theatrical_limited: date(),
                    ..Default::default()
                },
            ),
            (
                3,
                ReleaseDates {
                    theatrical: date(),
                    ..Default::default()
                },
            ),
            (
                4,
                ReleaseDates {
                    digital: date(),
                    ..Default::default()
                },
            ),
            (
                5,
                ReleaseDates {
                    physical: date(),
                    ..Default::default()
                },
            ),
            (
                6,
                ReleaseDates {
                    tv: date(),
                    ..Default::default()
                },
            ),
        ];
        for (release_type, want) in cases {
            let got = dates(&[(release_type, "2024-03-05T00:00:00.000Z")]);
            assert_eq!(got, want, "type {release_type}");
        }
    }

    #[test]
    fn ignores_unknown_types_and_malformed_dates() {
        let cases: &[(u8, &str)] = &[(0, "2024-01-01"), (7, "2024-01-01"), (3, "soon"), (3, "")];
        for &(release_type, date) in cases {
            assert!(
                by_region([("US", release_type, date)]).is_empty(),
                "({release_type}, {date:?})"
            );
        }
    }

    #[test]
    fn keeps_the_earliest_date_per_type() {
        let d = dates(&[
            (3, "2024-02-09T00:00:00.000Z"),
            (3, "2024-02-01T00:00:00.000Z"),
            (3, "2024-03-01T00:00:00.000Z"),
            (4, "2024-04-10T00:00:00.000Z"),
        ]);
        assert_eq!(d.theatrical.as_deref(), Some("2024-02-01"));
        assert_eq!(d.digital.as_deref(), Some("2024-04-10"));
    }

    #[test]
    fn region_selection_falls_back_to_us_then_earliest_anywhere() {
        let au_us_gb = by_region([
            ("AU", 3, "2024-02-01"),
            ("US", 3, "2024-01-15"),
            ("GB", 1, "2023-12-01"),
        ]);
        let gb_fr = by_region([("FR", 3, "2024-01-20"), ("GB", 1, "2023-12-01")]);
        let fr_de_tie = by_region([("FR", 3, "2024-01-20"), ("DE", 4, "2024-01-20")]);
        let cases: &[(&ReleaseDatesByRegion, &str, Option<&str>)] = &[
            (&au_us_gb, "AU", Some("AU")),
            (&au_us_gb, "au", Some("AU")),
            (&au_us_gb, "NZ", Some("US")),
            (&gb_fr, "NZ", Some("GB")),
            (&gb_fr, "US", Some("GB")),
            (&fr_de_tie, "NZ", Some("DE")),
            (&ReleaseDatesByRegion::new(), "US", None),
        ];
        for &(map, region, want) in cases {
            let got = select(map, region);
            assert_eq!(got.as_ref().map(|r| r.region.as_str()), want, "{region}");
            if let Some(got) = got {
                assert_eq!(&got.dates, &map[&got.region]);
            }
        }
    }

    #[test]
    fn serializes_to_the_api_shape() {
        let selected = select(&by_region([("AU", 3, "2024-02-01")]), "AU");
        assert_eq!(
            serde_json::to_value(selected).unwrap(),
            serde_json::json!({
                "region": "AU", "premiere": null, "theatrical_limited": null,
                "theatrical": "2024-02-01", "digital": null, "physical": null, "tv": null
            })
        );
    }
}
