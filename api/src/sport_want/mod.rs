//! The `SportEvent` want for one event, as every path that asks Kobload for a
//! sport event builds it: the want job, release search, and stream search.
//!
//! Beyond the search terms, the want carries every name the event's place,
//! league and session go by, so Kobload can accept a release named after any
//! of them.

mod aliases;
mod enrich;
mod gazetteer;
mod location;

use tracing::warn;

use crate::repositories::tv_episodes::EpisodeRow;
use crate::repositories::tv_seasons::SeasonRow;
use crate::repositories::tv_shows::TvShowRow;
use crate::video_metadata::VideoProviderRegistry;
use aliases::Gathered;
use enrich::{ENRICHERS, EventRef};

pub struct EventWant<'a> {
    pub show: &'a TvShowRow,
    pub season: &'a SeasonRow,
    pub episode: &'a EpisodeRow,
    pub in_library: Option<bool>,
    pub library_video_height: Option<i32>,
}

pub struct SportWant {
    pub want: nats_jobs::MediaWant,
    pub called_off: bool,
}

pub async fn event_want(registry: &VideoProviderRegistry, want: EventWant<'_>) -> SportWant {
    let EventWant {
        show,
        season,
        episode,
        in_library,
        library_video_height,
    } = want;
    let league_name = location::league_name(show);
    let details = location::event_details(show, episode);
    let class = enrich::class_of(&league_name).or_else(|| enrich::class_of(&show.title));

    let event = EventRef {
        league_id: show.identifiers.get("thesportsdb").and_then(|v| v.as_str()),
        class,
        season: season.season_identifier.trim().parse().ok(),
        air_date: episode.air_date,
        provider_event_id: episode
            .identifiers
            .get("thesportsdb_id")
            .and_then(|v| v.as_str()),
        sort_order: episode.episode_sort_order,
        title: episode.title.as_deref(),
        session: details.session_type.as_deref(),
    };

    let mut gathered = Gathered::default();
    gathered.league_aliases.push(show.title.clone());
    gathered
        .league_aliases
        .extend(show.alternate_titles.iter().flatten().cloned());
    for enricher in ENRICHERS {
        if !enricher.covers(&event) {
            continue;
        }
        if let Err(e) = enricher.gather(registry, &event, &mut gathered).await {
            warn!(
                episode_id = %episode.id,
                source = enricher.name(),
                error = %e,
                "Sport want enrichment unavailable, sending what the other sources found"
            );
        }
    }
    gathered.country_from(details.location.as_deref());
    let called_off = enrich::called_off(gathered.status.as_deref());

    let meta = aliases::finish(gathered, details.location.as_deref(), &league_name, class);

    let want = nats_jobs::MediaWant::SportEvent {
        league_name,
        season: season.season_identifier.clone(),
        round: details.round,
        location: details.location,
        session_type: details.session_type,
        event_title: details.event_title,
        event_date: episode.air_date.map(|d| d.to_string()),
        quality_profile_id: show.quality_profile_id,
        quality_overrides: show.preferred_languages.as_ref().map(|langs| {
            nats_jobs::QualityOverrides {
                preferred_languages: Some(langs.clone()),
                ..Default::default()
            }
        }),
        in_library,
        library_video_height,
        meta: Box::new(meta),
    };
    SportWant { want, called_off }
}

#[cfg(test)]
mod tests {
    use super::aliases::{Gathered, finish};
    use super::enrich::{EventRef, gather_from_motogp, gather_from_season};
    use crate::video_metadata::pulselive::{self, MotoGpEvent, TEST_EVENTS_2026};
    use crate::video_metadata::types::{EpisodeDetails, SeasonDetails, SportEventSource};

    fn session(
        number: u32,
        id: &str,
        title: &str,
        date: &str,
        venue: Option<&str>,
        city: Option<&str>,
        country: Option<&str>,
    ) -> EpisodeDetails {
        EpisodeDetails {
            episode_number: number,
            title: Some(title.into()),
            air_date: Some(date.into()),
            air_time: None,
            runtime: None,
            still_url: None,
            description: None,
            provider_id: Some(id.into()),
            country: country.map(String::from),
            credits: None,
            sport: Some(SportEventSource {
                venue: venue.map(String::from),
                city: city.map(String::from),
                ..Default::default()
            }),
        }
    }

    fn season(episodes: Vec<EpisodeDetails>) -> SeasonDetails {
        SeasonDetails {
            provider: "thesportsdb".into(),
            show_id: "4407".into(),
            season_number: 2026,
            title: None,
            air_date: None,
            description: None,
            poster_url: None,
            episodes,
        }
    }

    fn date(y: i32, m: time::Month, d: u8) -> Option<time::Date> {
        time::Date::from_calendar_date(y, m, d).ok()
    }

    fn assert_has(list: &Option<Vec<String>>, wants: &[&str]) {
        let list = list.as_deref().unwrap_or_default();
        for want in wants {
            assert!(
                list.iter().any(|a| a.eq_ignore_ascii_case(want)),
                "{want} missing from {list:?}"
            );
        }
    }

    #[test]
    fn americas_gp_race_carries_its_place_names_sponsor_start_and_class() {
        let mut race = session(
            3005,
            "ev-361462",
            "USA - Race",
            "2026-03-29",
            None,
            None,
            None,
        );
        race.air_time = Some("2026-03-29T19:00:00+00:00".into());
        race.sport.as_mut().unwrap().thesportsdb_id = Some("2424575".into());
        let schedule = season(vec![
            session(
                3000,
                "ev-2561852",
                "USA - Free Practice 1",
                "2026-03-27",
                Some("Circuit of the Americas"),
                Some("Austin"),
                Some("US"),
            ),
            race,
            session(
                4000,
                "ev-9",
                "Spain - Free Practice 1",
                "2026-04-24",
                Some("Circuito de Jerez – Ángel Nieto"),
                Some("Jerez de la Frontera"),
                Some("ES"),
            ),
        ]);
        let event = EventRef {
            league_id: Some("4407"),
            class: Some("MotoGP"),
            season: Some(2026),
            air_date: date(2026, time::Month::March, 29),
            provider_event_id: Some("ev-361462"),
            sort_order: 3005,
            title: Some("USA - Race"),
            session: Some("Race"),
        };

        let mut gathered = Gathered::default();
        gather_from_season(&schedule, &event, &mut gathered);
        let motogp: Vec<MotoGpEvent> = serde_json::from_str(TEST_EVENTS_2026).unwrap();
        gather_from_motogp(
            pulselive::event_on(&motogp, event.air_date.unwrap()).unwrap(),
            &mut gathered,
        );
        let meta = finish(gathered, Some("USA"), "MotoGP", Some("MotoGP"));

        assert_has(
            &meta.location_aliases,
            &[
                "COTA",
                "Circuit of the Americas",
                "Austin",
                "Texas",
                "USA",
                "US",
                "United States",
                "American",
                "Americas",
            ],
        );
        assert_has(
            &meta.event_aliases,
            &["Red Bull Grand Prix of The United States"],
        );
        assert_eq!(
            meta.session_start.as_deref(),
            Some("2026-03-29T19:00:00+00:00")
        );
        assert_eq!(meta.class.as_deref(), Some("MotoGP"));
        assert_eq!(meta.venue.as_deref(), Some("Circuit of the Americas"));
        assert_eq!(meta.city.as_deref(), Some("Austin"));
        assert_eq!(meta.region.as_deref(), Some("Texas"));
        assert_eq!(meta.country.as_deref(), Some("United States"));
        assert_eq!(meta.country_iso.as_deref(), Some("US"));
        let ids = meta.identifiers.unwrap();
        assert_eq!(ids.sportarr_id.as_deref(), Some("ev-361462"));
        assert_eq!(ids.thesportsdb_id.as_deref(), Some("2424575"));
        assert_eq!(
            ids.official_id.as_deref(),
            Some("782c929d-faaf-44e2-9d6f-0fde033855be")
        );
        let aliases = meta.location_aliases.unwrap();
        assert!(
            !aliases.iter().any(|a| a.contains("Jerez")),
            "another round's venue leaked in: {aliases:?}"
        );
    }

    /// Sportarr's venue gives the circuit word WorldSBK releases are named
    /// after (`WSBK.2026.Round12.Spain.Jerez`), which the title never has.
    #[test]
    fn sbk_round_carries_its_circuit_word() {
        let schedule = season(vec![
            session(
                12000,
                "ev-1",
                "Pirelli Spanish Round - Race 1",
                "2026-10-17",
                Some("Circuito de Jerez – Ángel Nieto"),
                Some("Jerez de la Frontera"),
                Some("ES"),
            ),
            session(
                12002,
                "ev-2",
                "Pirelli Spanish Round Superpole - Race",
                "2026-10-18",
                None,
                None,
                None,
            ),
        ]);
        let event = EventRef {
            league_id: Some("4454"),
            class: Some("SBK"),
            season: Some(2026),
            air_date: date(2026, time::Month::October, 18),
            provider_event_id: Some("ev-2"),
            sort_order: 12002,
            title: Some("Pirelli Spanish Round Superpole - Race"),
            session: Some("Superpole Race"),
        };
        let mut gathered = Gathered::default();
        gather_from_season(&schedule, &event, &mut gathered);
        gathered.league_aliases = vec![
            "SBK".into(),
            "WSB".into(),
            "WSBK".into(),
            "World Superbike".into(),
        ];
        let meta = finish(gathered, Some("Spain"), "SBK", Some("SBK"));

        assert_has(
            &meta.location_aliases,
            &[
                "Spain",
                "Jerez",
                "Circuito de Jerez – Ángel Nieto",
                "Spanish",
                "ES",
            ],
        );
        assert_eq!(
            meta.league_aliases,
            Some(vec!["WSB".into(), "WSBK".into(), "World Superbike".into()])
        );
        assert_eq!(meta.region.as_deref(), Some("Andalusia"));
    }

    #[test]
    fn an_unlisted_session_borrows_its_rounds_venue() {
        let schedule = season(vec![session(
            12000,
            "ev-1",
            "Pirelli Spanish Round - Race 1",
            "2026-10-17",
            Some("Circuito de Jerez – Ángel Nieto"),
            None,
            Some("ES"),
        )]);
        let event = EventRef {
            league_id: Some("4454"),
            class: Some("SBK"),
            season: Some(2026),
            air_date: None,
            provider_event_id: Some("123"),
            sort_order: 12004,
            title: Some("Pirelli Spanish Round - Race 2"),
            session: Some("Race 2"),
        };
        let mut gathered = Gathered::default();
        gather_from_season(&schedule, &event, &mut gathered);
        let meta = finish(gathered, Some("Spain"), "SBK", Some("SBK"));
        assert_has(&meta.location_aliases, &["Jerez"]);
        assert_eq!(meta.session_start, None);
        assert!(meta.identifiers.is_none());
    }
}
