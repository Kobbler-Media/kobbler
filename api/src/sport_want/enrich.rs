use async_trait::async_trait;
use nats_jobs::SportTeam;

use super::aliases::{Gathered, tidy};
use crate::video_metadata::jolpica::{self, F1Race};
use crate::video_metadata::pulselive::{self, MotoGpEvent};
use crate::video_metadata::thesportsdb;
use crate::video_metadata::types::{EpisodeDetails, SeasonDetails};
use crate::video_metadata::{VideoProviderError, VideoProviderRegistry};

pub struct EventRef<'a> {
    pub league_id: Option<&'a str>,
    pub class: Option<&'static str>,
    pub season: Option<u32>,
    pub air_date: Option<time::Date>,
    pub provider_event_id: Option<&'a str>,
    pub sort_order: i32,
    pub title: Option<&'a str>,
    pub session: Option<&'a str>,
}

/// One source of what an event is known as. Sportarr covers every league;
/// the others add what only a series' own organiser publishes. None of them
/// is required: a failure leaves the want with whatever the rest found.
#[async_trait]
pub trait SportEnricher: Sync {
    fn name(&self) -> &'static str;
    fn covers(&self, event: &EventRef<'_>) -> bool;
    async fn gather(
        &self,
        registry: &VideoProviderRegistry,
        event: &EventRef<'_>,
        into: &mut Gathered,
    ) -> Result<(), VideoProviderError>;
}

pub const ENRICHERS: &[&dyn SportEnricher] = &[&Sportarr, &Pulselive, &Jolpica];

const MOTOGP_CLASSES: &[&str] = &["MotoGP", "Moto2", "Moto3"];

pub fn class_of(league_name: &str) -> Option<&'static str> {
    let key: String = league_name
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    let class = match key.as_str() {
        "motogp" => "MotoGP",
        "moto2" => "Moto2",
        "moto3" => "Moto3",
        "motoe" => "MotoE",
        "sbk"
        | "wsbk"
        | "worldsbk"
        | "worldsuperbike"
        | "worldsuperbikes"
        | "superbikeworldchampionship" => "SBK",
        "wssp" | "worldssp" | "worldsupersport" | "supersportworldchampionship" => "WorldSSP",
        "f1" | "formula1" | "formulaone" => "F1",
        "f2" | "formula2" => "F2",
        "f3" | "formula3" => "F3",
        "formulae" => "Formula E",
        _ => return None,
    };
    Some(class)
}

pub struct Sportarr;

#[async_trait]
impl SportEnricher for Sportarr {
    fn name(&self) -> &'static str {
        "sportarr"
    }

    fn covers(&self, event: &EventRef<'_>) -> bool {
        event.league_id.is_some()
    }

    async fn gather(
        &self,
        registry: &VideoProviderRegistry,
        event: &EventRef<'_>,
        into: &mut Gathered,
    ) -> Result<(), VideoProviderError> {
        let Some(league_id) = event.league_id else {
            return Ok(());
        };
        if let Some(names) = registry.get_league_alternate_names(league_id).await {
            into.league_aliases.extend(names);
        }
        if let Some(season) = event.season {
            let details = registry
                .get_season(thesportsdb::INFO.id, league_id, season)
                .await?;
            gather_from_season(&details, event, into);
        }
        for (name, id) in std::mem::take(&mut into.team_refs) {
            let looked_up = match id {
                Some(id) => registry.get_sport_team(&id).await.flatten(),
                None => None,
            };
            into.teams.push(looked_up.unwrap_or(SportTeam {
                name,
                short: None,
                aliases: Vec::new(),
            }));
        }
        Ok(())
    }
}

fn round_of(episode_number: i64) -> i64 {
    episode_number / 1000
}

fn find_event<'s>(season: &'s SeasonDetails, event: &EventRef<'_>) -> Option<&'s EpisodeDetails> {
    let air_date = event.air_date.map(|d| d.to_string());
    event
        .provider_event_id
        .and_then(|id| {
            season
                .episodes
                .iter()
                .find(|e| e.provider_id.as_deref() == Some(id))
        })
        .or_else(|| {
            season.episodes.iter().find(|e| {
                e.title.is_some() && e.title.as_deref() == event.title && e.air_date == air_date
            })
        })
}

/// Venue and city are missing from some rows of a round and spelled
/// differently on others ("Montmelo", "Montmeló, Barcelona"), so every row of
/// the round contributes, this session's own first.
pub fn gather_from_season(season: &SeasonDetails, event: &EventRef<'_>, into: &mut Gathered) {
    let found = find_event(season, event);
    let round = found
        .map(|e| round_of(e.episode_number as i64))
        .unwrap_or_else(|| round_of(event.sort_order as i64));
    let mut rows: Vec<&EpisodeDetails> = found.into_iter().collect();
    if round > 0 {
        rows.extend(
            season
                .episodes
                .iter()
                .filter(|e| round_of(e.episode_number as i64) == round)
                .filter(|e| found.is_none_or(|f| !std::ptr::eq(*e, f))),
        );
    }

    for row in &rows {
        if let Some(sport) = &row.sport {
            into.venues.extend(sport.venue.iter().cloned());
            into.cities.extend(sport.city.iter().cloned());
        }
        into.country_from(row.country.as_deref());
    }

    let Some(found) = found else {
        return;
    };
    into.session_start = into.session_start.take().or(found.air_time.clone());
    if let Some(sport) = &found.sport {
        into.event_aliases
            .extend(sport.alternate_name.iter().chain(&sport.filename).cloned());
        into.ids.thesportsdb_id = sport.thesportsdb_id.clone();
        into.status = sport.status.clone();
        for (name, id) in [
            (&sport.home_team, &sport.home_team_id),
            (&sport.away_team, &sport.away_team_id),
        ] {
            if let Some(name) = name {
                into.team_refs.push((name.clone(), id.clone()));
            }
        }
    }
    if let Some(id) = found.provider_id.as_deref() {
        if id.starts_with("ev-") {
            into.ids.sportarr_id = Some(id.to_string());
        } else if into.ids.thesportsdb_id.is_none() {
            into.ids.thesportsdb_id = Some(id.to_string());
        }
    }
}

pub struct Pulselive;

#[async_trait]
impl SportEnricher for Pulselive {
    fn name(&self) -> &'static str {
        "pulselive"
    }

    fn covers(&self, event: &EventRef<'_>) -> bool {
        event.class.is_some_and(|c| MOTOGP_CLASSES.contains(&c))
            && event.season.is_some()
            && event.air_date.is_some()
    }

    async fn gather(
        &self,
        registry: &VideoProviderRegistry,
        event: &EventRef<'_>,
        into: &mut Gathered,
    ) -> Result<(), VideoProviderError> {
        let (Some(season), Some(date)) = (event.season, event.air_date) else {
            return Ok(());
        };
        let events = registry.get_motogp_events(season).await.unwrap_or_default();
        if let Some(found) = pulselive::event_on(&events, date) {
            gather_from_motogp(found, into);
        }
        Ok(())
    }
}

pub fn gather_from_motogp(event: &MotoGpEvent, into: &mut Gathered) {
    if let Some(circuit) = &event.circuit {
        into.venues.extend(circuit.name.as_deref().and_then(tidy));
        into.cities.extend(circuit.place.as_deref().and_then(tidy));
    }
    into.places
        .extend(event.additional_name.as_deref().and_then(tidy));
    if let Some(country) = &event.country {
        into.country_from(country.iso.as_deref());
        into.places.extend(country.name.as_deref().and_then(tidy));
    }
    into.event_aliases.extend(
        [&event.sponsored_name, &event.name]
            .into_iter()
            .filter_map(|n| n.as_deref().and_then(tidy)),
    );
    into.ids.official_id = Some(event.id.clone());
}

/// Events Sportarr reports as postponed or cancelled ("Match Postponed",
/// "cancelled") are not wanted: nothing will be released for them on that date.
pub fn called_off(status: Option<&str>) -> bool {
    status.is_some_and(|s| {
        let s = s.to_lowercase();
        s.contains("postpon") || s.contains("cancel")
    })
}

pub struct Jolpica;

#[async_trait]
impl SportEnricher for Jolpica {
    fn name(&self) -> &'static str {
        "jolpica"
    }

    fn covers(&self, event: &EventRef<'_>) -> bool {
        event.class == Some("F1") && event.season.is_some() && event.air_date.is_some()
    }

    async fn gather(
        &self,
        registry: &VideoProviderRegistry,
        event: &EventRef<'_>,
        into: &mut Gathered,
    ) -> Result<(), VideoProviderError> {
        let (Some(season), Some(date)) = (event.season, event.air_date) else {
            return Ok(());
        };
        let races = registry.get_f1_races(season).await.unwrap_or_default();
        if let Some(race) = jolpica::race_on(&races, date) {
            gather_from_f1(race, event.session, into);
        }
        Ok(())
    }
}

pub fn gather_from_f1(race: &F1Race, session: Option<&str>, into: &mut Gathered) {
    into.venues.push(race.circuit.name.clone());
    into.cities
        .extend(race.circuit.location.locality.iter().cloned());
    into.country_from(race.circuit.location.country.as_deref());
    into.event_aliases.push(race.race_name.clone());
    if into.session_start.is_none() {
        into.session_start = session.and_then(|s| race.session_start(s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leagues_name_their_class() {
        for (league, class) in [
            ("MotoGP", Some("MotoGP")),
            ("Moto GP", Some("MotoGP")),
            ("Moto2", Some("Moto2")),
            ("SBK", Some("SBK")),
            ("WSBK", Some("SBK")),
            ("World Superbike", Some("SBK")),
            ("Formula 1", Some("F1")),
            ("English Premier League", None),
        ] {
            assert_eq!(class_of(league), class, "{league}");
        }
    }

    #[test]
    fn pulselive_covers_only_the_motogp_classes() {
        let event = |class| EventRef {
            league_id: Some("4407"),
            class,
            season: Some(2026),
            air_date: time::Date::from_calendar_date(2026, time::Month::March, 29).ok(),
            provider_event_id: None,
            sort_order: 3000,
            title: None,
            session: None,
        };
        assert!(Pulselive.covers(&event(Some("MotoGP"))));
        assert!(Pulselive.covers(&event(Some("Moto3"))));
        assert!(!Pulselive.covers(&event(Some("SBK"))));
        assert!(!Pulselive.covers(&event(None)));
        assert!(Sportarr.covers(&event(Some("SBK"))));
    }

    #[test]
    fn postponed_and_cancelled_events_are_called_off() {
        for (status, off) in [
            (Some("postponed"), true),
            (Some("Match Postponed"), true),
            (Some("cancelled"), true),
            (Some("Canceled"), true),
            (Some("scheduled"), false),
            (Some("completed"), false),
            (None, false),
        ] {
            assert_eq!(called_off(status), off, "{status:?}");
        }
    }

    #[test]
    fn jolpica_covers_only_formula_one() {
        let event = |class| EventRef {
            league_id: None,
            class,
            season: Some(2026),
            air_date: time::Date::from_calendar_date(2026, time::Month::May, 2).ok(),
            provider_event_id: None,
            sort_order: 4000,
            title: None,
            session: Some("Qualifying"),
        };
        assert!(Jolpica.covers(&event(Some("F1"))));
        assert!(!Jolpica.covers(&event(Some("F2"))));
        assert!(!Jolpica.covers(&event(Some("MotoGP"))));
    }
}
