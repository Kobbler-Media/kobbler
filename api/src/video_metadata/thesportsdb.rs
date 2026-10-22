use async_trait::async_trait;
use chrono::Datelike;
use serde::Deserialize;

use super::types::{
    EpisodeDetails, MovieResult, SeasonSummary, SportEventSource, TvShowResult, VideoCapability,
    VideoCapabilitySet, VideoConfigField, VideoIdentifiers, VideoProviderInfo,
};
use super::{
    MovieDetails, MovieQuery, SeasonDetails, TvQuery, TvShowDetails, VideoMetadataProvider,
    VideoProviderError, VideoProviderFactory,
};

// ============================================================================
// Provider Info
// ============================================================================

static CONFIG_FIELDS: &[VideoConfigField] = &[];

static CAPABILITIES: &[VideoCapability] = &[
    VideoCapability::TvSearch,
    VideoCapability::TvDetails,
    VideoCapability::SeasonDetails,
    VideoCapability::ImageFetch,
];

pub static INFO: VideoProviderInfo = VideoProviderInfo {
    id: crate::http::provider_ids::THESPORTSDB,
    display_name: "TheSportsDB",
    description: "Free sports metadata. No API key required. Leagues, seasons, events.",
    website: "https://www.thesportsdb.com",
    capabilities: VideoCapabilitySet::new(CAPABILITIES),
    config_fields: CONFIG_FIELDS,
    requests_per_minute: 60, // Free tier, be respectful
};

const BASE_URL: &str = "https://www.thesportsdb.com/api/v1/json/3";

/// Sportarr proxy API — wraps TheSportsDB with server-side caching and no
/// result-count caps. Used as primary source; falls back to direct TheSportsDB.
const SPORTARR_BASE_URL: &str = "https://sportarr.net/api/v2/json";

/// Sportarr's own league records, the only place its alternate names are
/// published without an API key.
const SPORTARR_PUBLIC_URL: &str = "https://sportarr.net/api/public/v1";

/// How many years of seasons to generate when enriching a league.
const LOOKBACK_YEARS: u32 = 3;

// ============================================================================
// Factory
// ============================================================================

pub struct TheSportsDbFactory;

impl VideoProviderFactory for TheSportsDbFactory {
    fn info(&self) -> &'static VideoProviderInfo {
        &INFO
    }

    fn create(
        &self,
        client: reqwest::Client,
        _config: &serde_json::Value,
    ) -> Box<dyn VideoMetadataProvider> {
        Box::new(TheSportsDbProvider { client })
    }
}

// ============================================================================
// API Response Types
// ============================================================================

#[derive(Debug, Deserialize)]
struct LeagueSearchResponse {
    #[serde(default)]
    countries: Vec<ApiLeague>,
}

#[derive(Debug, Deserialize)]
struct LeagueLookupResponse {
    #[serde(default)]
    leagues: Vec<ApiLeague>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ApiLeague {
    #[serde(rename = "idLeague")]
    id_league: String,
    #[serde(rename = "strLeague")]
    str_league: Option<String>,
    #[serde(rename = "strSport")]
    str_sport: Option<String>,
    #[serde(rename = "strCountry")]
    str_country: Option<String>,
    #[serde(rename = "strDescriptionEN")]
    str_description_en: Option<String>,
    #[serde(rename = "strBadge")]
    str_badge: Option<String>,
    #[serde(rename = "strBanner")]
    str_banner: Option<String>,
    #[serde(rename = "strPoster")]
    str_poster: Option<String>,
    #[serde(rename = "strFanart1")]
    str_fanart1: Option<String>,
    #[serde(rename = "intFormedYear")]
    int_formed_year: Option<String>,
    #[serde(rename = "strCurrentSeason")]
    str_current_season: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EventsResponse {
    #[serde(default)]
    events: Option<Vec<ApiEvent>>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ApiEvent {
    #[serde(rename = "idEvent")]
    id_event: String,
    #[serde(rename = "strEvent")]
    str_event: Option<String>,
    #[serde(rename = "intRound")]
    int_round: Option<String>,
    #[serde(rename = "dateEvent")]
    date_event: Option<String>,
    #[serde(rename = "strVenue")]
    str_venue: Option<String>,
    #[serde(rename = "strCity")]
    str_city: Option<String>,
    #[serde(rename = "strCountry")]
    str_country: Option<String>,
    #[serde(rename = "strThumb")]
    str_thumb: Option<String>,
    #[serde(rename = "strPoster")]
    str_poster: Option<String>,
    #[serde(rename = "strDescriptionEN")]
    str_description_en: Option<String>,
    #[serde(rename = "strSeason")]
    str_season: Option<String>,
    #[serde(rename = "strTimestamp", default)]
    str_timestamp: Option<String>,
    #[serde(rename = "strFilename", default)]
    str_filename: Option<String>,
    #[serde(rename = "strEventAlternate", default)]
    str_event_alternate: Option<String>,
    #[serde(rename = "tsdbId", default)]
    tsdb_id: Option<String>,
    #[serde(rename = "strStatus", default)]
    str_status: Option<String>,
    #[serde(rename = "strHomeTeam", default)]
    str_home_team: Option<String>,
    #[serde(rename = "idHomeTeam", default)]
    id_home_team: Option<String>,
    #[serde(rename = "strAwayTeam", default)]
    str_away_team: Option<String>,
    #[serde(rename = "idAwayTeam", default)]
    id_away_team: Option<String>,
}

// ============================================================================
// Sportarr API Response Types (wraps TheSportsDB data)
// ============================================================================

#[derive(Debug, Default, Deserialize)]
struct SportarrResponse<T: Default> {
    #[serde(default)]
    data: Option<T>,
}

#[derive(Debug, Default, Deserialize)]
struct SportarrSearchData {
    #[serde(default)]
    search: Vec<ApiLeague>,
}

#[derive(Debug, Default, Deserialize)]
struct SportarrLookupData {
    #[serde(default)]
    lookup: Vec<ApiLeague>,
}

#[derive(Debug, Default, Deserialize)]
struct SportarrScheduleData {
    #[serde(default)]
    schedule: Vec<ApiEvent>,
}

// ============================================================================
// Provider
// ============================================================================

struct TheSportsDbProvider {
    client: reqwest::Client,
}

impl TheSportsDbProvider {
    /// Send a GET request and check the HTTP status before deserializing.
    /// Returns `RateLimited` on 429, propagates other error statuses.
    async fn checked_get<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
    ) -> Result<T, VideoProviderError> {
        let resp = self.client.get(url).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            tracing::warn!("TheSportsDB rate limited (429) on {url}");
            return Err(VideoProviderError::RateLimited);
        }
        if !status.is_success() {
            return Err(VideoProviderError::Other(format!(
                "TheSportsDB returned HTTP {status} for {url}"
            )));
        }
        resp.json().await.map_err(VideoProviderError::from)
    }

    /// Like `checked_get` but returns a default value on deserialization failure
    /// (TheSportsDB sometimes returns empty/null bodies for valid "no results" responses).
    async fn checked_get_or<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        default: T,
    ) -> Result<T, VideoProviderError> {
        let resp = self.client.get(url).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            tracing::warn!("TheSportsDB rate limited (429) on {url}");
            return Err(VideoProviderError::RateLimited);
        }
        if !status.is_success() {
            return Err(VideoProviderError::Other(format!(
                "TheSportsDB returned HTTP {status} for {url}"
            )));
        }
        Ok(resp.json().await.unwrap_or(default))
    }

    /// Search for leagues by name. Tries Sportarr API first, falls back to
    /// direct TheSportsDB.
    async fn search_leagues(&self, query: &str) -> Result<Vec<ApiLeague>, VideoProviderError> {
        // 1. Try Sportarr (no rate limit, server-side cache)
        let sportarr_url = format!(
            "{SPORTARR_BASE_URL}/search/league/{}",
            urlencoding::encode(query)
        );
        match self
            .checked_get::<SportarrResponse<SportarrSearchData>>(&sportarr_url)
            .await
        {
            Ok(resp) => {
                let results = resp.data.map(|d| d.search).unwrap_or_default();
                if !results.is_empty() {
                    return Ok(results);
                }
            }
            Err(e) => {
                tracing::debug!("Sportarr search failed, falling back to TheSportsDB: {e}");
            }
        }

        // 2. Fallback: TheSportsDB direct — try sport-based search first
        let url = format!("{BASE_URL}/search_all_leagues.php?s={query}");
        let resp: LeagueSearchResponse = self
            .checked_get_or(
                &url,
                LeagueSearchResponse {
                    countries: Vec::new(),
                },
            )
            .await?;

        if !resp.countries.is_empty() {
            return Ok(resp.countries);
        }

        // 3. Fallback: direct league name search
        let url = format!("{BASE_URL}/search_all_leagues.php?l={query}");
        let resp: LeagueSearchResponse = self
            .checked_get_or(
                &url,
                LeagueSearchResponse {
                    countries: Vec::new(),
                },
            )
            .await?;

        Ok(resp.countries)
    }

    /// Get league details by ID. Tries Sportarr first, falls back to TheSportsDB.
    async fn get_league(&self, league_id: &str) -> Result<ApiLeague, VideoProviderError> {
        // 1. Try Sportarr
        let sportarr_url = format!("{SPORTARR_BASE_URL}/lookup/league/{league_id}");
        match self
            .checked_get::<SportarrResponse<SportarrLookupData>>(&sportarr_url)
            .await
        {
            Ok(resp) => {
                if let Some(league) = resp.data.and_then(|d| d.lookup.into_iter().next()) {
                    return Ok(league);
                }
            }
            Err(e) => {
                tracing::debug!("Sportarr lookup failed, falling back to TheSportsDB: {e}");
            }
        }

        // 2. Fallback: TheSportsDB direct
        let url = format!("{BASE_URL}/lookupleague.php?id={league_id}");
        let resp: LeagueLookupResponse = self.checked_get(&url).await?;

        resp.leagues
            .into_iter()
            .next()
            .ok_or(VideoProviderError::NotFound)
    }

    /// Get events for a single round in a season.
    async fn get_round_events(
        &self,
        league_id: &str,
        round: u32,
        season: &str,
    ) -> Result<Vec<ApiEvent>, VideoProviderError> {
        let url = format!("{BASE_URL}/eventsround.php?id={league_id}&r={round}&s={season}");
        let resp: EventsResponse = self
            .checked_get_or(&url, EventsResponse { events: None })
            .await?;

        Ok(resp.events.unwrap_or_default())
    }

    /// Get all events for a specific season.
    ///
    /// Strategy:
    /// 1. Sportarr API — single call, no result cap, server-cached (30 days)
    /// 2. Quick freshness check via TheSportsDB `eventsseason.php` (1 call,
    ///    capped at 15 but enough to detect new events)
    /// 3. If TheSportsDB shows more unique rounds than Sportarr returned,
    ///    fall back to direct per-round fetching for the missing data
    async fn get_season_events(
        &self,
        league_id: &str,
        season: &str,
    ) -> Result<Vec<ApiEvent>, VideoProviderError> {
        // 1. Try Sportarr — single call, returns all events
        let sportarr_url = format!("{SPORTARR_BASE_URL}/schedule/league/{league_id}/{season}");
        let sportarr_events = match self
            .checked_get::<SportarrResponse<SportarrScheduleData>>(&sportarr_url)
            .await
        {
            Ok(resp) => {
                let events = resp.data.map(|d| d.schedule).unwrap_or_default();
                if !events.is_empty() {
                    tracing::debug!(
                        "Sportarr returned {} events for league {} season {}",
                        events.len(),
                        league_id,
                        season,
                    );
                    Some(events)
                } else {
                    None
                }
            }
            Err(e) => {
                tracing::debug!(
                    "Sportarr schedule failed for league {} season {}: {e}",
                    league_id,
                    season,
                );
                None
            }
        };

        // 2. Quick freshness check against TheSportsDB (1 API call).
        //    The free tier caps at 15 results, but we only need to detect
        //    whether new rounds exist beyond what Sportarr returned.
        let scout_url = format!("{BASE_URL}/eventsseason.php?id={league_id}&s={season}");
        let scout_events = match self
            .checked_get_or::<EventsResponse>(&scout_url, EventsResponse { events: None })
            .await
        {
            Ok(resp) => resp.events.unwrap_or_default(),
            Err(e) => {
                tracing::debug!("TheSportsDB freshness check failed: {e}");
                Vec::new()
            }
        };

        if let Some(sportarr_events) = sportarr_events {
            // Compare max round numbers to detect if TheSportsDB has newer data
            let sportarr_max_round = sportarr_events
                .iter()
                .filter_map(|e| e.int_round.as_deref()?.parse::<u32>().ok())
                .max()
                .unwrap_or(0);
            let scout_max_round = scout_events
                .iter()
                .filter_map(|e| e.int_round.as_deref()?.parse::<u32>().ok())
                .max()
                .unwrap_or(0);

            if scout_max_round <= sportarr_max_round {
                // Sportarr is up to date (or fresher)
                return Ok(sportarr_events);
            }

            // TheSportsDB has newer rounds — merge. Fetch only the new rounds
            // to avoid burning rate limit on data we already have.
            tracing::info!(
                "TheSportsDB has newer data (round {scout_max_round}) than \
                 Sportarr cache (round {sportarr_max_round}) for league \
                 {league_id} season {season}, fetching new rounds"
            );
            let mut all_events = sportarr_events;
            let mut seen: std::collections::HashSet<String> =
                all_events.iter().map(|e| e.id_event.clone()).collect();

            let mut consecutive_empty = 0u32;
            for round in (sportarr_max_round + 1)..=(scout_max_round + 5) {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let events = self.get_round_events(league_id, round, season).await?;
                if events.is_empty() {
                    consecutive_empty += 1;
                    if consecutive_empty >= 3 {
                        break;
                    }
                } else {
                    consecutive_empty = 0;
                    for e in events {
                        if seen.insert(e.id_event.clone()) {
                            all_events.push(e);
                        }
                    }
                }
            }

            return Ok(all_events);
        }

        // 3. No Sportarr data — full TheSportsDB direct fallback
        if scout_events.is_empty() {
            return Ok(Vec::new());
        }
        self.get_season_events_direct(league_id, season).await
    }

    /// TheSportsDB direct fallback for season events.
    ///
    /// Uses `eventsseason.php` as a scout (1 call, capped at 15 results on
    /// the free tier). If under the cap we have everything; otherwise, fetch
    /// per-round using `eventsround.php`.
    async fn get_season_events_direct(
        &self,
        league_id: &str,
        season: &str,
    ) -> Result<Vec<ApiEvent>, VideoProviderError> {
        let scout_url = format!("{BASE_URL}/eventsseason.php?id={league_id}&s={season}");
        let scout: EventsResponse = self
            .checked_get_or(&scout_url, EventsResponse { events: None })
            .await?;
        let scout_events = scout.events.unwrap_or_default();

        if scout_events.is_empty() {
            return Ok(Vec::new());
        }

        // If under the 15-result cap, we have the complete season
        if scout_events.len() < 15 {
            return Ok(scout_events);
        }

        // Over the cap — need per-round fetching. Use the scout to find
        // which rounds exist so we don't blindly iterate 1..100.
        let mut known_rounds: std::collections::BTreeSet<u32> = scout_events
            .iter()
            .filter_map(|e| e.int_round.as_deref()?.parse::<u32>().ok())
            .collect();

        let max_round = known_rounds.iter().next_back().copied().unwrap_or(1);

        // The scout may not have shown all rounds (it caps at 15 events, not
        // 15 rounds). Fill in gaps: include all rounds from 1..=max_round.
        for r in 1..=max_round {
            known_rounds.insert(r);
        }

        // Fetch each round individually, then probe beyond max_round
        let mut all_events = Vec::new();

        for &round in &known_rounds {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let events = self.get_round_events(league_id, round, season).await?;
            all_events.extend(events);
        }

        // Probe beyond the scout's max round for additional rounds
        let mut consecutive_empty = 0u32;
        for round in (max_round + 1)..=(max_round + 20) {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let events = self.get_round_events(league_id, round, season).await?;
            if events.is_empty() {
                consecutive_empty += 1;
                if consecutive_empty >= 3 {
                    break;
                }
            } else {
                consecutive_empty = 0;
                all_events.extend(events);
            }
        }

        // Also check round 500 (pre-season/testing for some leagues)
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let preseason = self.get_round_events(league_id, 500, season).await?;
        all_events.extend(preseason);

        // Deduplicate by event ID (scout events may overlap with round fetches)
        let mut seen = std::collections::HashSet::new();
        all_events.retain(|e| seen.insert(e.id_event.clone()));

        Ok(all_events)
    }

    /// Group events by round and extract location.
    /// Prefers the structured `str_country` field over parsing event names.
    #[cfg(test)]
    fn group_events_by_round(events: &[ApiEvent]) -> Vec<RoundGroup<'_>> {
        use std::collections::BTreeMap;

        let mut round_map: BTreeMap<u32, Vec<&ApiEvent>> = BTreeMap::new();

        for event in events {
            let round = event
                .int_round
                .as_deref()
                .and_then(|r| r.parse::<u32>().ok())
                .unwrap_or(0);
            round_map.entry(round).or_default().push(event);
        }

        round_map
            .into_iter()
            .map(|(round, events)| {
                // Prefer structured str_country field, fall back to event name parsing
                let location = events
                    .first()
                    .and_then(|e| e.str_country.as_deref())
                    .filter(|c| !c.is_empty())
                    .map(String::from)
                    .unwrap_or_else(|| {
                        extract_location(events.first().and_then(|e| e.str_event.as_deref()))
                    });

                RoundGroup {
                    round,
                    location,
                    events,
                }
            })
            .collect()
    }
}

#[cfg(test)]
struct RoundGroup<'a> {
    round: u32,
    location: String,
    events: Vec<&'a ApiEvent>,
}

fn episodes_from_events(events: &[ApiEvent]) -> Vec<EpisodeDetails> {
    let mut sorted_events: Vec<&ApiEvent> = events.iter().collect();
    sorted_events.sort_by(|a, b| {
        let date_a = a.date_event.as_deref().unwrap_or("");
        let date_b = b.date_event.as_deref().unwrap_or("");
        let name_a = a.str_event.as_deref().unwrap_or("");
        let name_b = b.str_event.as_deref().unwrap_or("");
        date_a
            .cmp(date_b)
            .then_with(|| {
                session_sort_index_from_name(name_a).cmp(&session_sort_index_from_name(name_b))
            })
            .then_with(|| name_a.cmp(name_b))
    });

    let mut round_seq: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();

    sorted_events
        .into_iter()
        .map(|event| {
            let round = event
                .int_round
                .as_deref()
                .and_then(|r| r.parse::<u32>().ok())
                .unwrap_or(0);

            let seq = round_seq.entry(round).or_insert(0);
            let episode_number = round * 1000 + *seq;
            *seq += 1;

            EpisodeDetails {
                episode_number,
                title: event.str_event.clone(),
                air_date: event.date_event.clone(),
                air_time: session_start(event.str_timestamp.as_deref()),
                runtime: None,
                still_url: event
                    .str_thumb
                    .as_deref()
                    .or(event.str_poster.as_deref())
                    .map(String::from),
                description: event.str_description_en.clone(),
                provider_id: Some(event.id_event.clone()),
                country: event.str_country.clone(),
                credits: None,
                sport: Some(SportEventSource {
                    venue: non_empty(&event.str_venue),
                    city: non_empty(&event.str_city),
                    alternate_name: non_empty(&event.str_event_alternate),
                    filename: non_empty(&event.str_filename),
                    thesportsdb_id: non_empty(&event.tsdb_id),
                    status: non_empty(&event.str_status),
                    home_team: non_empty(&event.str_home_team),
                    home_team_id: non_empty(&event.id_home_team),
                    away_team: non_empty(&event.str_away_team),
                    away_team_id: non_empty(&event.id_away_team),
                }),
            }
        })
        .collect()
}

fn non_empty(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(String::from)
}

/// Sessions of older seasons are all stamped 00:00:00 UTC, which is the date
/// with no time rather than a midnight start.
fn session_start(timestamp: Option<&str>) -> Option<String> {
    let parsed = chrono::DateTime::parse_from_rfc3339(timestamp?.trim()).ok()?;
    let utc = parsed.with_timezone(&chrono::Utc);
    (utc.time() != chrono::NaiveTime::MIN).then(|| parsed.to_rfc3339())
}

fn parse_league_alternate_names(body: &str) -> Vec<String> {
    #[derive(Deserialize)]
    struct PublicLeague {
        #[serde(rename = "alternateNames", default)]
        alternate_names: Vec<String>,
    }
    serde_json::from_str::<PublicLeague>(body)
        .map(|l| {
            l.alternate_names
                .into_iter()
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

async fn sportarr_get(
    client: &reqwest::Client,
    url: &str,
) -> Result<Option<String>, VideoProviderError> {
    let resp = client
        .get(url)
        .timeout(crate::source_cache::LOOKUP_TIMEOUT)
        .send()
        .await?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(VideoProviderError::Other(format!(
            "Sportarr returned HTTP {status} for {url}"
        )));
    }
    Ok(Some(resp.text().await?))
}

fn parse_league_short_id(body: &str) -> Option<String> {
    let resp: SportarrResponse<SportarrLookupData> = serde_json::from_str(body).ok()?;
    resp.data?.lookup.into_iter().next().map(|l| l.id_league)
}

/// The names release groups also file a league under ("WSBK", "World
/// Superbike"). A TheSportsDB id is resolved to Sportarr's id first; a league
/// Sportarr does not know has none.
pub async fn fetch_league_alternate_names(
    client: &reqwest::Client,
    urls: &SportarrUrls,
    league_id: &str,
) -> Result<Vec<String>, VideoProviderError> {
    let short_id = if league_id.starts_with("lg-") {
        league_id.to_string()
    } else {
        let url = format!(
            "{}/lookup/league/{}",
            urls.v2,
            urlencoding::encode(league_id)
        );
        match sportarr_get(client, &url)
            .await?
            .as_deref()
            .and_then(parse_league_short_id)
        {
            Some(id) => id,
            None => return Ok(Vec::new()),
        }
    };
    let url = format!("{}/leagues/{}", urls.public, urlencoding::encode(&short_id));
    Ok(sportarr_get(client, &url)
        .await?
        .map(|body| parse_league_alternate_names(&body))
        .unwrap_or_default())
}

pub struct SportarrUrls {
    pub v2: String,
    pub public: String,
}

impl Default for SportarrUrls {
    fn default() -> Self {
        Self {
            v2: SPORTARR_BASE_URL.to_string(),
            public: SPORTARR_PUBLIC_URL.to_string(),
        }
    }
}

fn parse_team(body: &str) -> Option<nats_jobs::SportTeam> {
    #[derive(Deserialize)]
    struct Team {
        #[serde(rename = "strTeam")]
        name: Option<String>,
        #[serde(rename = "strTeamShort", default)]
        short: Option<String>,
        #[serde(rename = "strAlternate", default)]
        alternate: Option<String>,
    }
    #[derive(Deserialize)]
    struct Lookup {
        #[serde(default)]
        data: Vec<Team>,
    }
    let team = serde_json::from_str::<Lookup>(body)
        .ok()?
        .data
        .into_iter()
        .next()?;
    let name = non_empty(&team.name)?;
    Some(nats_jobs::SportTeam {
        short: non_empty(&team.short),
        aliases: team
            .alternate
            .iter()
            .flat_map(|a| a.split(','))
            .map(str::trim)
            .filter(|a| !a.is_empty() && !a.eq_ignore_ascii_case(&name))
            .map(String::from)
            .collect(),
        name,
    })
}

/// A team's short name and nicknames ("ARS", "Gunners") from Sportarr. A
/// team Sportarr does not know is `None`, which is an answer.
pub async fn fetch_team(
    client: &reqwest::Client,
    urls: &SportarrUrls,
    team_id: &str,
) -> Result<Option<nats_jobs::SportTeam>, VideoProviderError> {
    let url = format!("{}/lookup/team/{}", urls.v2, urlencoding::encode(team_id));
    Ok(sportarr_get(client, &url)
        .await?
        .as_deref()
        .and_then(parse_team))
}

/// Trim and collapse runs of whitespace to single spaces.
/// TheSportsDB titles often have trailing spaces ("United States Grand Prix Moto2 ")
/// and double spaces ("Austrian Grand Prix  Moto2").
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Extract location from event name by removing known suffixes/prefixes.
/// Used as fallback when `str_country` is not available.
///
/// "Thailand Sprint Race" → "Thailand"
/// "Grand Prix of Qatar" → "Qatar"
/// "Thailand Grand Prix Moto3" → "Thailand" (strips class suffix first)
/// Title sponsors TheSportsDB puts in front of WorldSBK round names
/// ("Pirelli Spanish Round", "Prosecco DOC UK Round"). Longest first.
const ROUND_SPONSORS: &[&str] = &[
    "Prosecco DOC",
    "Liqui Moly",
    "Estrella Galicia",
    "Pirelli",
    "Motul",
    "Acerbis",
    "Tissot",
    "Dunlop",
];

/// Normalize a location extracted from an event title.
/// Strips leading articles and maps demonyms to country names.
///
/// WorldSBK titles are "{Sponsor} {Demonym} Round[ Superpole] - {Session}", so
/// after the session is gone the sponsor, "Round", "Superpole" and the dash are
/// left over. Through 2023 the Superpole Race was "{Round}[ Race] (Superpole)".
/// Kept, they made every SBK want look for a place no release is named after
/// ("Pirelli Spanish Round" vs `WSBK.2026.Round12.Spain.Jerez`) and nothing
/// was grabbed from round 5 on.
fn normalize_location(location: &str) -> String {
    // Strip leading articles
    let mut location = location
        .strip_prefix("the ")
        .or_else(|| location.strip_prefix("The "))
        .unwrap_or(location)
        .trim();

    loop {
        let before = location;
        location = location.trim_end_matches(['-', ' ']);
        if location.ends_with(')')
            && let Some(open) = location.rfind('(')
        {
            location = &location[..open];
        }
        for suffix in [" Superpole", " Round", " Race"] {
            if let Some(stripped) = location.strip_suffix(suffix) {
                location = stripped;
            }
        }
        if location == before {
            break;
        }
    }
    for sponsor in ROUND_SPONSORS {
        if let Some(rest) = location.strip_prefix(sponsor)
            && rest.starts_with(' ')
        {
            location = rest.trim();
            break;
        }
    }

    if let Some(country) = demonym_country(location) {
        return country.to_string();
    }
    // An unlisted sponsor in front of a demonym ("Acme Spanish") still names
    // the country.
    if let Some((_, last)) = location.rsplit_once(' ')
        && let Some(country) = demonym_country(last)
    {
        return country.to_string();
    }
    location.to_string()
}

/// Map demonyms to country names (common in TheSportsDB event titles).
fn demonym_country(word: &str) -> Option<&'static str> {
    let country = match word {
        "Brazilian" => "Brazil",
        "Hungarian" => "Hungary",
        "British" => "Britain",
        "Spanish" => "Spain",
        "French" => "France",
        "Italian" => "Italy",
        "Dutch" => "Netherlands",
        "German" => "Germany",
        "Australian" => "Australia",
        "Austrian" => "Austria",
        "Japanese" => "Japan",
        "Chinese" => "China",
        "Indian" => "India",
        "Portuguese" => "Portugal",
        "Canadian" => "Canada",
        "Mexican" => "Mexico",
        "Belgian" => "Belgium",
        _ => return None,
    };
    Some(country)
}

pub fn extract_location(event_name: Option<&str>) -> String {
    let name = match event_name {
        Some(n) if !n.trim().is_empty() => n,
        _ => return "Unknown".to_string(),
    };

    // Normalize whitespace: trim and collapse multiple spaces (TheSportsDB data
    // often has trailing spaces and double spaces, e.g. "Austrian Grand Prix  Moto2")
    let mut cleaned = collapse_whitespace(name);

    // Step 1: Strip motorsport class suffixes (TheSportsDB appends these for sub-categories)
    let class_suffixes = [
        " Moto3", " Moto2", " MotoGP", " MotoE", " WSBK", " SBK", " WSS", " SSP",
    ];
    for suffix in &class_suffixes {
        if let Some(stripped) = cleaned.strip_suffix(suffix) {
            cleaned = stripped.trim().to_string();
            break;
        }
    }

    // Step 2: Remove session suffixes (longest-first to avoid partial matches,
    // e.g. " Race" must come after " Race 1" / " Race 2" / " Superpole Race")
    let session_suffixes = [
        " Sprint Qualifying",
        " Sprint Shootout",
        " Superpole Race",
        " Sprint Race",
        " Sprint",
        " E-Prix",
        " ePrix",
        " GP",
        " Grand Prix",
        " Gran Premio",
        " Free Practice 1",
        " Free Practice 2",
        " Free Practice 3",
        " Free Practice 4",
        " Free Practice",
        " Qualifying 1",
        " Qualifying 2",
        " Qualifying",
        " Practice",
        " Race 1",
        " Race 2",
        " Race",
        " Superpole",
        " Warm Up",
    ];

    for suffix in &session_suffixes {
        if let Some(stripped) = cleaned.strip_suffix(suffix) {
            let stripped = stripped.trim();
            if !stripped.is_empty() {
                return normalize_location(stripped);
            }
        }
    }

    // Step 3: Remove "Grand Prix of " / "Gran Premio de " prefix patterns.
    // "Gran Premio di " must come before "Gran Premio d'" to avoid greedy matching
    // (e.g., "Gran Premio di San Marino..." should not match "Gran Premio d").
    let gp_prefixes = [
        "Grand Prix of ",
        "Grand Prix de ",
        "Grande Prémio de ",
        "Gran Premio di ",
        "Gran Premio de ",
        "Gran Premio d'",
        "Gran Premi de ",
        "Motorrad Grand Prix von ",
        "Motorrad Grand Prix ",
        "Pertamina Grand Prix of ",
        "TT ",
    ];

    for prefix in &gp_prefixes {
        if let Some(stripped) = cleaned.strip_prefix(prefix) {
            let stripped = stripped.trim();
            if !stripped.is_empty() {
                // For "Gran Premio di San Marino e della Riviera di Rimini",
                // take only the first meaningful part before " e della"
                let location = if let Some(idx) = stripped.find(" e della ") {
                    &stripped[..idx]
                } else {
                    stripped
                };
                return normalize_location(location);
            }
        }
    }

    // Fallback: use the cleaned name
    normalize_location(&cleaned)
}

/// Normalize session names from TheSportsDB event titles to release-friendly names.
///
/// TheSportsDB uses locale-specific event names ("Grand Prix", "Gran Premio",
/// "E-Prix") while release filenames use standardized English session types
/// ("Race", "Qualifying", "Sprint", etc.).
///
/// Based on conventions from Sportarr's NormalizeMotorsportSession:
/// - "Grand Prix" / "GP" / locale variants → "Race"
/// - "E-Prix" / "ePrix" (Formula E) → "Race"
/// - "Sprint Shootout" → "Sprint Qualifying"
fn normalize_session_name(session: &str) -> String {
    match session {
        "Grand Prix" | "GP" | "Gran Premio" | "Grande Prémio" | "Gran Premi" | "E-Prix"
        | "ePrix" | "E Prix" => "Race".to_string(),
        "Sprint Shootout" => "Sprint Qualifying".to_string(),
        _ => session.to_string(),
    }
}

/// Extract the session/event-type portion from a TheSportsDB event name,
/// given the location. E.g., "Thailand Qualifying 1" with location "Thailand"
/// returns "Qualifying 1". Falls back to the full event name if location
/// isn't found as a prefix.
pub fn extract_session_from_event(event_name: &str, location: &str) -> String {
    // Normalize whitespace first (TheSportsDB has trailing spaces, double spaces)
    let mut cleaned = collapse_whitespace(event_name);

    // Strip class suffixes (Moto3, MotoGP, etc.)
    let class_suffixes = [
        " Moto3", " Moto2", " MotoGP", " MotoE", " WSBK", " SBK", " WSS", " SSP",
    ];
    for suffix in &class_suffixes {
        if let Some(stripped) = cleaned.strip_suffix(suffix) {
            cleaned = stripped.trim().to_string();
            break;
        }
    }

    // Try to strip location prefix (word-boundary: next char must be space or end)
    if let Some(rest) = cleaned.strip_prefix(location)
        && (rest.is_empty() || rest.starts_with(' '))
    {
        // "San Marino - Sprint Race": the location no longer carries the dash.
        let rest = rest.trim().trim_start_matches('-').trim();
        if !rest.is_empty() {
            return normalize_session_name(rest);
        }
    }

    // Try "Grand Prix of Location Session" → "Session"
    // or just return the event name minus known location patterns
    let gp_patterns = [
        format!("Grand Prix of {} ", location),
        format!("Grand Prix de {} ", location),
        format!("Gran Premio de {} ", location),
    ];
    for pat in &gp_patterns {
        if let Some(rest) = cleaned.strip_prefix(pat.as_str()) {
            let rest = rest.trim();
            if !rest.is_empty() {
                return normalize_session_name(rest);
            }
        }
    }

    // If the entire cleaned title is a GP-format name (no trailing session),
    // the event IS the race itself. This catches patterns like
    // "Grand Prix of the Americas", "Gran Premio di San Marino e della Riviera di Rimini".
    let gp_name_prefixes = [
        "Grand Prix of ",
        "Grand Prix de ",
        "Grande Prémio de ",
        "Gran Premio di ",
        "Gran Premio de ",
        "Gran Premio d'",
        "Gran Premi de ",
        "Motorrad Grand Prix ",
        "Pertamina Grand Prix of ",
    ];
    for prefix in &gp_name_prefixes {
        if cleaned.starts_with(prefix) {
            return "Race".to_string();
        }
    }

    // Suffix-based extraction: match known session patterns from the end of the
    // string. Handles series like SBK where TheSportsDB titles are
    // "{Sponsor} {Demonym} Round {Session}" and the location prefix doesn't
    // match because it's a demonym ("Portuguese") vs country ("Portugal").
    // Ordered longest-first to avoid partial matches.
    let cleaned_lower = cleaned.to_lowercase();
    let session_suffixes_lower: &[(&str, &str)] = &[
        ("sprint qualifying", "Sprint Qualifying"),
        ("superpole race", "Superpole Race"),
        ("sprint race", "Sprint Race"),
        ("free practice 1", "Free Practice 1"),
        ("free practice 2", "Free Practice 2"),
        ("free practice 3", "Free Practice 3"),
        ("free practice 4", "Free Practice 4"),
        ("free practice", "Free Practice"),
        ("qualifying 1", "Qualifying 1"),
        ("qualifying 2", "Qualifying 2"),
        ("qualifying", "Qualifying"),
        ("warm up", "Warm Up"),
        ("race 1", "Race 1"),
        ("race 2", "Race 2"),
        ("grand prix", "Race"),
        ("superpole", "Superpole"),
        ("sprint", "Sprint"),
        ("race", "Race"),
        ("practice", "Practice"),
    ];
    for (suffix, normalized) in session_suffixes_lower {
        if cleaned_lower.ends_with(suffix) {
            return normalized.to_string();
        }
    }

    normalize_session_name(&cleaned)
}

/// Sessions recognised as motorsport weekend sessions. Only these get a natural-key
/// `episode_identifier`; anything else (team-sport leagues etc.) keeps the provider-id
/// form so we don't reshape non-motorsport events.
const RECOGNIZED_SESSIONS: &[&str] = &[
    "Race",
    "Race 1",
    "Race 2",
    "Sprint",
    "Sprint Race",
    "Sprint Qualifying",
    "Sprint Shootout",
    "Superpole",
    "Superpole Race",
    "Qualifying",
    "Qualifying 1",
    "Qualifying 2",
    "Practice",
    "Free Practice",
    "Free Practice 1",
    "Free Practice 2",
    "Free Practice 3",
    "Free Practice 4",
    "Warm Up",
];

/// Build a stable `episode_identifier` for a sport event.
///
/// For recognised motorsport sessions we derive a natural key from the season round
/// (`episode_number / 1000`) and the session, e.g. `evt-r5-free-practice-1`. Two upstream
/// TheSportsDB event IDs for the same real session therefore collapse to one row via the
/// `(season_id, episode_identifier)` UNIQUE constraint — the root fix for duplicate
/// sport-event downloads. Non-motorsport events fall back to the provider-id form.
///
/// Mirrors the session/location/round logic of `parse_sport_event_details` (the want
/// side) so ingestion and want agree on what a session is.
pub fn sport_episode_identifier(
    episode_number: i32,
    title: Option<&str>,
    provider_id: Option<&str>,
) -> String {
    if let Some(title) = title {
        let round = episode_number / 1000;
        let location = extract_location(Some(title));
        if round > 0 && location != "Unknown" {
            let session = extract_session_from_event(title, &location);
            if RECOGNIZED_SESSIONS.contains(&session.as_str()) {
                let slug = session.to_lowercase().replace(' ', "-");
                return format!("evt-r{round}-{slug}");
            }
        }
    }
    match provider_id {
        Some(pid) => format!("evt-{pid}"),
        None => format!(
            "ep-{}-{}",
            episode_number,
            title.unwrap_or("event").to_lowercase().replace(' ', "-")
        ),
    }
}

/// Stable sort index for session types within a round.
/// Used by `session_sort_index_from_name` to sort events chronologically.
pub fn session_sort_index(session: &str) -> u32 {
    match session {
        "Practice" => 0,
        "Qualifying" => 1,
        "Sprint" => 2,
        "Race" => 3,
        _ => 4,
    }
}

/// Determine a sort index from a raw event name for ordering within a round.
/// Lower = earlier in the weekend. Uses keyword matching on the event name.
fn session_sort_index_from_name(event_name: &str) -> u32 {
    let lower = event_name.to_lowercase();
    if lower.contains("practice") || lower.contains("warm") {
        0
    } else if lower.contains("qualifying") || lower.contains("qual") {
        1
    } else if lower.contains("sprint") {
        2
    } else {
        3 // Race / main event / team match / anything else
    }
}

#[async_trait]
impl VideoMetadataProvider for TheSportsDbProvider {
    fn info(&self) -> &'static VideoProviderInfo {
        &INFO
    }

    async fn search_movies(
        &self,
        _query: &MovieQuery,
    ) -> Result<Vec<MovieResult>, VideoProviderError> {
        // Sports-only provider
        Ok(Vec::new())
    }

    async fn get_movie(&self, _provider_id: &str) -> Result<MovieDetails, VideoProviderError> {
        Err(VideoProviderError::NotSupported)
    }

    async fn search_tv(&self, query: &TvQuery) -> Result<Vec<TvShowResult>, VideoProviderError> {
        let leagues = self.search_leagues(&query.title).await?;

        // Also filter results by the query string to improve relevance
        let query_lower = query.title.to_lowercase();
        let results: Vec<TvShowResult> = leagues
            .into_iter()
            .filter(|l| {
                l.str_league
                    .as_deref()
                    .map(|n| n.to_lowercase().contains(&query_lower))
                    .unwrap_or(false)
            })
            .map(|l| {
                let year = l
                    .int_formed_year
                    .as_deref()
                    .and_then(|y| y.parse::<u16>().ok());

                TvShowResult {
                    provider: "thesportsdb".to_string(),
                    provider_id: l.id_league.clone(),
                    title: l.str_league.unwrap_or_default(),
                    year,
                    status: Some("continuing".to_string()),
                    description: l.str_description_en,
                    poster_url: l.str_poster.or(l.str_badge),
                    backdrop_url: None,
                    network: l.str_sport,
                    rating: None,
                    rating_count: None,
                    identifiers: VideoIdentifiers {
                        tmdb_id: None,
                        imdb_id: None,
                        tvmaze_id: None,
                        tvdb_id: None,
                    },
                    genres: l.str_country.map(|c| vec![c]).unwrap_or_default(),
                }
            })
            .collect();

        Ok(results)
    }

    async fn get_tv_show(&self, provider_id: &str) -> Result<TvShowDetails, VideoProviderError> {
        let league = self.get_league(provider_id).await?;

        // Build season list: recent LOOKBACK_YEARS from current year.
        // strCurrentSeason can be "2025" or "2024-2025" (cross-year leagues).
        let current_year = chrono::Utc::now().year() as u32;
        let api_end_year = league.str_current_season.as_deref().and_then(|y| {
            // Handle both "2025" and "2024-2025" formats
            y.split('-')
                .next_back()
                .and_then(|part| part.parse::<u32>().ok())
        });
        let end_year = api_end_year.unwrap_or(current_year).max(current_year);
        let start_year = end_year.saturating_sub(LOOKBACK_YEARS);

        let mut season_years: Vec<u32> = (start_year..=end_year).collect();
        season_years.sort_unstable();
        season_years.dedup();
        // Most recent seasons first — enrichment processes them in order,
        // so recent/relevant seasons get populated before older ones.
        season_years.reverse();

        let season_summaries: Vec<SeasonSummary> = season_years
            .iter()
            .map(|&y| SeasonSummary {
                season_number: y,
                episode_count: None,
                air_date: Some(format!("{y}-01-01")),
                poster_url: None,
                title: Some(y.to_string()),
            })
            .collect();

        let year = league
            .int_formed_year
            .as_deref()
            .and_then(|y| y.parse::<u16>().ok());

        Ok(TvShowDetails {
            provider: "thesportsdb".to_string(),
            provider_id: league.id_league.clone(),
            title: league.str_league.unwrap_or_default(),
            year,
            status: Some("continuing".to_string()),
            description: league.str_description_en,
            poster_url: league.str_poster.or(league.str_badge.clone()),
            backdrop_url: league.str_fanart1,
            logo_url: league.str_badge,
            identifiers: VideoIdentifiers::default(),
            genres: league.str_country.map(|c| vec![c]).unwrap_or_default(),
            rating: None,
            rating_count: None,
            network: league.str_sport,
            content_rating: None,
            first_aired: year.map(|y| format!("{}-01-01", y)),
            runtime: None,
            seasons: season_summaries,
            cast: Vec::new(),
            crew: Vec::new(),
            images: Vec::new(),
        })
    }

    async fn get_season(
        &self,
        show_id: &str,
        season: u32,
    ) -> Result<SeasonDetails, VideoProviderError> {
        let season_str = season.to_string();
        let events = self.get_season_events(show_id, &season_str).await?;

        if events.is_empty() {
            return Err(VideoProviderError::NotFound);
        }

        let episodes = episodes_from_events(&events);

        Ok(SeasonDetails {
            provider: "thesportsdb".to_string(),
            show_id: show_id.to_string(),
            season_number: season,
            title: Some(season_str),
            air_date: episodes.first().and_then(|e| e.air_date.clone()),
            description: None,
            poster_url: None,
            episodes,
        })
    }

    async fn fetch_image(&self, url: &str) -> Result<Vec<u8>, VideoProviderError> {
        let bytes = self.client.get(url).send().await?.bytes().await?;
        Ok(bytes.to_vec())
    }
}

/// Parsed sport event details for constructing MediaWant.
pub struct SportEventDetails {
    pub round: Option<u32>,
    pub location: Option<String>,
    pub session_type: Option<String>,
    pub event_title: Option<String>,
}

/// The session to put in a want, which can be more specific than the one the
/// episode identifier is built from.
///
/// WorldSBK's Superpole Race is titled "{Round} Superpole - Race", and the dash
/// keeps it from reading as "Superpole Race": the session comes out as "Race".
/// SBK has no other plain "Race", so the identifier (`evt-r12-race`) is still
/// unique and stays as it is: changing it would fork every existing row. The
/// want has to say "Superpole Race", though, or it never matches the
/// `...Superpole.Race...` release.
///
/// The bracketed form ("Motul French Round  (Superpole)") yields no session
/// the identifier recognises, so those rows keep the provider-id form and only
/// the want learns which race it is.
///
/// F1 numbers its practice without "Free" ("Australian Grand Prix - Practice
/// 1"). Those rows keep the provider-id form too, and the want says "Free
/// Practice 1", as releases and the motorcycle series do.
fn want_session(title: &str, session: String) -> String {
    let title = title.to_lowercase();
    if (session == "Race" && title.contains("superpole")) || title.contains("(superpole)") {
        return "Superpole Race".to_string();
    }
    match session.strip_prefix("Practice ") {
        Some(n @ ("1" | "2" | "3" | "4")) => format!("Free Practice {n}"),
        _ => session,
    }
}

/// Parse sport event details from episode title, identifier, and sort order.
///
/// Supports both new format (evt-{id} identifier, raw TheSportsDB title)
/// and legacy format (R01-race identifier, "R01 Location - Session" title).
pub fn parse_sport_event_details(
    title: Option<&str>,
    identifier: &str,
    sort_order: i32,
) -> SportEventDetails {
    // New format: evt-{id} identifier with raw event title
    if identifier.starts_with("evt-") {
        let event_title = title.map(String::from);
        let round = if sort_order > 0 {
            Some((sort_order / 1000) as u32)
        } else {
            None
        };

        if let Some((grand_prix, session)) = title.and_then(|t| t.split_once(" Grand Prix - ")) {
            return SportEventDetails {
                round,
                location: Some(normalize_location(grand_prix.trim())),
                session_type: Some(want_session(
                    title.unwrap_or_default(),
                    normalize_session_name(session.trim()),
                )),
                event_title,
            };
        }

        // Try to extract location and session from motorsport event names
        if let Some(title_str) = title {
            let location = extract_location(Some(title_str));
            if location != "Unknown" {
                let session =
                    want_session(title_str, extract_session_from_event(title_str, &location));
                return SportEventDetails {
                    round,
                    location: Some(location),
                    session_type: Some(session),
                    event_title,
                };
            }
        }

        // Non-motorsport: just use event_title
        return SportEventDetails {
            round,
            location: None,
            session_type: None,
            event_title,
        };
    }

    // Legacy format: "R01 Thailand - Race" title, "R01-race" identifier
    if let Some(title_str) = title
        && let Some(rest) = title_str.strip_prefix('R')
    {
        let parts: Vec<&str> = rest.splitn(2, ' ').collect();
        if parts.len() == 2 {
            let round = parts[0].parse::<u32>().unwrap_or(1);
            if let Some((location, session)) = parts[1].rsplit_once(" - ") {
                return SportEventDetails {
                    round: Some(round),
                    location: Some(location.to_string()),
                    session_type: Some(session.to_string()),
                    event_title: title.map(String::from),
                };
            }
        }
    }

    if let Some(rest) = identifier.strip_prefix('R') {
        let parts: Vec<&str> = rest.splitn(2, '-').collect();
        if parts.len() == 2 {
            let round = parts[0].parse::<u32>().unwrap_or(1);
            let session = parts[1].to_string();
            return SportEventDetails {
                round: Some(round),
                location: Some("Unknown".to_string()),
                session_type: Some(session),
                event_title: title.map(String::from),
            };
        }
    }

    SportEventDetails {
        round: Some(1),
        location: Some("Unknown".to_string()),
        session_type: Some("Race".to_string()),
        event_title: title.map(String::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // extract_location
    // =========================================================================

    #[test]
    fn test_extract_location_sprint() {
        assert_eq!(extract_location(Some("Thailand Sprint Race")), "Thailand");
    }

    #[test]
    fn test_extract_location_gp() {
        assert_eq!(extract_location(Some("Thailand GP")), "Thailand");
    }

    #[test]
    fn test_extract_location_grand_prix_of() {
        assert_eq!(extract_location(Some("Grand Prix of Qatar")), "Qatar");
    }

    #[test]
    fn test_extract_location_gran_premio() {
        assert_eq!(extract_location(Some("Gran Premio de España")), "España");
    }

    #[test]
    fn test_extract_location_none() {
        assert_eq!(extract_location(None), "Unknown");
    }

    #[test]
    fn test_extract_location_moto3_grand_prix() {
        assert_eq!(
            extract_location(Some("Thailand Grand Prix Moto3")),
            "Thailand"
        );
    }

    #[test]
    fn test_extract_location_moto2_grand_prix() {
        assert_eq!(
            extract_location(Some("Argentina Grand Prix Moto2")),
            "Argentina"
        );
    }

    #[test]
    fn test_extract_location_moto3_gran_premio() {
        assert_eq!(
            extract_location(Some("Gran Premio de España Moto3")),
            "España"
        );
    }

    #[test]
    fn test_extract_location_moto3_tt() {
        assert_eq!(extract_location(Some("TT Assen Moto3")), "Assen");
    }

    #[test]
    fn test_extract_location_moto3_motorrad() {
        assert_eq!(
            extract_location(Some("Motorrad Grand Prix Deutschland Moto3")),
            "Deutschland"
        );
    }

    #[test]
    fn test_extract_location_grand_prix_of_the() {
        assert_eq!(
            extract_location(Some("Grand Prix of the Americas Moto3")),
            "Americas"
        );
    }

    #[test]
    fn test_extract_location_demonym_brazilian() {
        assert_eq!(
            extract_location(Some("Brazilian Grand Prix Moto3")),
            "Brazil"
        );
    }

    #[test]
    fn test_extract_location_demonym_hungarian() {
        assert_eq!(
            extract_location(Some("Hungarian Grand Prix Moto3")),
            "Hungary"
        );
    }

    #[test]
    fn test_extract_location_demonym_british() {
        assert_eq!(
            extract_location(Some("British Grand Prix Moto3")),
            "Britain"
        );
    }

    #[test]
    fn test_extract_location_san_marino_italian() {
        assert_eq!(
            extract_location(Some(
                "Gran Premio di San Marino e della Riviera di Rimini Moto3"
            )),
            "San Marino"
        );
    }

    // =========================================================================
    // extract_session_from_event
    // =========================================================================

    #[test]
    fn test_extract_session_from_event() {
        assert_eq!(
            extract_session_from_event("Thailand Qualifying 1", "Thailand"),
            "Qualifying 1"
        );
        assert_eq!(
            extract_session_from_event("Thailand Sprint Race", "Thailand"),
            "Sprint Race"
        );
        assert_eq!(
            extract_session_from_event("Thailand GP", "Thailand"),
            "Race"
        );
    }

    #[test]
    fn test_extract_session_strips_class_suffix() {
        // "Grand Prix" is normalized to "Race" for release matching
        assert_eq!(
            extract_session_from_event("Thailand Grand Prix Moto3", "Thailand"),
            "Race"
        );
    }

    #[test]
    fn test_extract_session_qualifying_not_normalized() {
        assert_eq!(
            extract_session_from_event("Thailand Qualifying 1 Moto3", "Thailand"),
            "Qualifying 1"
        );
    }

    #[test]
    fn test_extract_session_sprint_race_not_normalized() {
        assert_eq!(
            extract_session_from_event("Thailand Sprint Race Moto3", "Thailand"),
            "Sprint Race"
        );
    }

    #[test]
    fn test_extract_session_gp_full_name_is_race() {
        // When the entire event name IS the GP (no trailing session), session = Race
        assert_eq!(
            extract_session_from_event("Grand Prix of the Americas Moto3", "Americas"),
            "Race"
        );
        assert_eq!(
            extract_session_from_event("Grand Prix of Qatar Moto3", "Qatar"),
            "Race"
        );
        assert_eq!(
            extract_session_from_event(
                "Gran Premio di San Marino e della Riviera di Rimini Moto3",
                "San Marino"
            ),
            "Race"
        );
    }

    // =========================================================================
    // SBK / WSBK event naming (suffix-based extraction)
    // =========================================================================

    #[test]
    fn test_extract_location_sbk_without_sponsor() {
        // Without the sponsor prefix, stripping " Race 2" + " SBK" leaves the
        // demonym "Portuguese" which normalizes to "Portugal". In practice
        // str_country from TheSportsDB provides the location directly.
        assert_eq!(extract_location(Some("Portuguese Race 2 SBK")), "Portugal");
    }

    #[test]
    fn test_extract_session_sbk_race_2() {
        // Location is "Portugal" (from str_country) but title uses "Portuguese"
        assert_eq!(
            extract_session_from_event("Pirelli Portuguese Round Race 2 SBK", "Portugal"),
            "Race 2"
        );
    }

    #[test]
    fn test_extract_session_sbk_race_1() {
        assert_eq!(
            extract_session_from_event("Pirelli Portuguese Round Race 1 SBK", "Portugal"),
            "Race 1"
        );
    }

    #[test]
    fn test_extract_session_sbk_superpole_race() {
        assert_eq!(
            extract_session_from_event("Pirelli Portuguese Round Superpole Race SBK", "Portugal"),
            "Superpole Race"
        );
    }

    #[test]
    fn test_extract_session_sbk_superpole() {
        assert_eq!(
            extract_session_from_event("Pirelli Portuguese Round Superpole SBK", "Portugal"),
            "Superpole"
        );
    }

    #[test]
    fn test_extract_session_sbk_free_practice() {
        assert_eq!(
            extract_session_from_event("Pirelli Portuguese Round Free Practice 1 SBK", "Portugal"),
            "Free Practice 1"
        );
    }

    /// TheSportsDB's 2026 WorldSBK titles, from round 5 on with no country to
    /// fall back to. The sponsor, "Round", "Superpole" and the dash all ended
    /// up in the location, and every want from those rounds missed its release
    /// (`WSBK.2026.Round12.Spain.Jerez.Race.One`).
    #[test]
    fn sbk_round_titles_give_the_place_releases_use() {
        let cases = [
            ("Pirelli Spanish Round - Race 1", "Spain"),
            ("Pirelli Spanish Round Superpole - Race", "Spain"),
            ("Pirelli Portuguese Round - Free Practice 1", "Portugal"),
            ("Pirelli Portuguese Round Superpole", "Portugal"),
            ("Pirelli Dutch Superpole - Race", "Netherlands"),
            ("Motul Hungarian Round - Race 2", "Hungary"),
            ("Czech Round - Race 1", "Czech"),
            ("Aragon Round Superpole - Race", "Aragon"),
            (
                "Pirelli Emilia Romagna Round - Superpole - Race",
                "Emilia Romagna",
            ),
            ("Prosecco DOC UK Round - Race 2", "UK"),
            ("Acerbis French Round - Race 1", "France"),
            ("Italian Round - Race 1", "Italy"),
            ("Italian Superpole - Race", "Italy"),
            ("Tissot Estoril Round - Race 2", "Estoril"),
            ("Australian - Warm Up", "Australia"),
            ("Australian Superpole", "Australia"),
            // An unlisted sponsor still leaves the demonym to map.
            ("Acme Spanish Round - Race 1", "Spain"),
            // The pre-2026 form.
            ("Pirelli Portuguese Round Superpole Race SBK", "Portugal"),
        ];
        for (title, want) in cases {
            assert_eq!(extract_location(Some(title)), want, "location for {title}");
        }
    }

    #[test]
    fn sbk_wants_name_the_superpole_race() {
        let race = |title: &str, ident: &str, ord: i32| {
            let d = parse_sport_event_details(Some(title), ident, ord);
            (d.location, d.session_type, d.round)
        };
        assert_eq!(
            race(
                "Pirelli Spanish Round Superpole - Race",
                "evt-r12-race",
                12002
            ),
            (
                Some("Spain".into()),
                Some("Superpole Race".into()),
                Some(12)
            )
        );
        assert_eq!(
            race("Pirelli Spanish Round - Race 1", "evt-r12-race-1", 12000),
            (Some("Spain".into()), Some("Race 1".into()), Some(12))
        );
        assert_eq!(
            race(
                "Pirelli Portuguese Round Superpole",
                "evt-r2-superpole",
                2004
            ),
            (Some("Portugal".into()), Some("Superpole".into()), Some(2))
        );
        // MotoGP has no Superpole; its races stay "Race".
        assert_eq!(
            race("Thailand Race", "evt-r1-race", 1003).1,
            Some("Race".into())
        );
    }

    /// Existing rows are keyed on these identifiers; the location cleanup must
    /// not move any of them, or a season refresh inserts a second row per event.
    #[test]
    fn sbk_identifiers_are_unchanged_by_the_location_cleanup() {
        let cases = [
            ("Pirelli Spanish Round - Race 1", 12000, "evt-r12-race-1"),
            ("Pirelli Spanish Round - Race 2", 12001, "evt-r12-race-2"),
            (
                "Pirelli Spanish Round Superpole - Race",
                12002,
                "evt-r12-race",
            ),
            (
                "Pirelli Portuguese Round Superpole",
                2004,
                "evt-r2-superpole",
            ),
            (
                "Pirelli Portuguese Round - Free Practice 1",
                2000,
                "evt-r2-free-practice-1",
            ),
            ("Australian Superpole - Race", 1007, "evt-r1-race"),
            ("Australian - Warm Up", 1005, "evt-r1-warm-up"),
            ("San Marino - Sprint Race", 14005, "evt-r14-sprint-race"),
            ("Spain - Free Practice 1", 4000, "evt-r4-free-practice-1"),
        ];
        for (title, ord, want) in cases {
            assert_eq!(
                sport_episode_identifier(ord, Some(title), Some("123")),
                want,
                "identifier for {title}"
            );
        }
    }

    // =========================================================================
    // TheSportsDB dirty data (trailing spaces, double spaces)
    // =========================================================================

    #[test]
    fn test_extract_location_trailing_space() {
        // "United States Grand Prix Moto2 " — trailing space from TheSportsDB
        assert_eq!(
            extract_location(Some("United States Grand Prix Moto2 ")),
            "United States"
        );
    }

    #[test]
    fn test_extract_location_double_space() {
        // "Austrian Grand Prix  Moto2" — double space
        assert_eq!(
            extract_location(Some("Austrian Grand Prix  Moto2")),
            "Austria"
        );
    }

    #[test]
    fn test_extract_session_trailing_space() {
        assert_eq!(
            extract_session_from_event("United States Grand Prix Moto2 ", "United States"),
            "Race"
        );
    }

    #[test]
    fn test_extract_session_double_space() {
        assert_eq!(
            extract_session_from_event("Austrian Grand Prix  Moto2", "Austria"),
            "Race"
        );
    }

    // =========================================================================
    // session_sort_index
    // =========================================================================

    #[test]
    fn test_session_sort_order() {
        assert!(session_sort_index("Practice") < session_sort_index("Qualifying"));
        assert!(session_sort_index("Qualifying") < session_sort_index("Sprint"));
        assert!(session_sort_index("Sprint") < session_sort_index("Race"));
    }

    // =========================================================================
    // group_events_by_round — prefers str_country over event name
    // =========================================================================

    fn make_event(name: &str, round: u32, country: Option<&str>) -> ApiEvent {
        make_event_with_id(name, round, country, "1")
    }

    fn make_event_with_id(name: &str, round: u32, country: Option<&str>, id: &str) -> ApiEvent {
        ApiEvent {
            id_event: id.to_string(),
            str_event: Some(name.to_string()),
            int_round: Some(round.to_string()),
            date_event: Some("2026-03-01".to_string()),
            str_venue: None,
            str_city: None,
            str_country: country.map(String::from),
            str_thumb: None,
            str_poster: None,
            str_description_en: None,
            str_season: None,
            str_timestamp: None,
            str_filename: None,
            str_event_alternate: None,
            tsdb_id: None,
            str_status: None,
            str_home_team: None,
            id_home_team: None,
            str_away_team: None,
            id_away_team: None,
        }
    }

    #[test]
    fn test_group_uses_str_country() {
        let events = vec![make_event("Thailand Grand Prix Moto3", 1, Some("Thailand"))];
        let groups = TheSportsDbProvider::group_events_by_round(&events);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].location, "Thailand");
    }

    #[test]
    fn test_group_falls_back_to_event_name() {
        let events = vec![make_event("Thailand Sprint Race", 1, None)];
        let groups = TheSportsDbProvider::group_events_by_round(&events);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].location, "Thailand");
    }

    // =========================================================================
    // 1:1 event mapping — each API event = one episode
    // =========================================================================

    #[test]
    fn test_moto3_single_event_creates_one_episode() {
        let events = vec![make_event("Thailand Grand Prix Moto3", 1, Some("Thailand"))];
        let groups = TheSportsDbProvider::group_events_by_round(&events);
        assert_eq!(groups[0].events.len(), 1);
    }

    #[test]
    fn test_motogp_multi_event_preserves_all_events() {
        // MotoGP: 4 events per round — all 4 should become episodes
        let events = vec![
            make_event_with_id("Thailand Qualifying 1", 1, Some("Thailand"), "100"),
            make_event_with_id("Thailand Qualifying 2", 1, Some("Thailand"), "101"),
            make_event_with_id("Thailand Sprint Race", 1, Some("Thailand"), "102"),
            make_event_with_id("Thailand GP", 1, Some("Thailand"), "103"),
        ];
        let groups = TheSportsDbProvider::group_events_by_round(&events);
        // All 4 events in one round group
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].events.len(), 4);
    }

    #[test]
    fn test_team_sport_events_preserved() {
        // Premier League: each match is a separate event
        let events = vec![
            make_event_with_id("Arsenal vs Chelsea", 1, Some("England"), "200"),
            make_event_with_id("Liverpool vs Man City", 1, Some("England"), "201"),
            make_event_with_id("Tottenham vs Brighton", 1, Some("England"), "202"),
        ];
        let groups = TheSportsDbProvider::group_events_by_round(&events);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].events.len(), 3);
    }

    #[test]
    fn test_episode_numbering_uses_round_times_1000() {
        // Round 3 events should get episode_numbers 3000, 3001, etc.
        let events = vec![
            make_event_with_id("Event A", 3, None, "300"),
            make_event_with_id("Event B", 3, None, "301"),
        ];

        // We can't call get_season directly (it's async + needs HTTP),
        // but we can verify the grouping logic
        let groups = TheSportsDbProvider::group_events_by_round(&events);
        assert_eq!(groups[0].round, 3);
        assert_eq!(groups[0].events.len(), 2);
        // episode_number would be 3*1000+0=3000, 3*1000+1=3001
    }

    // =========================================================================
    // sport_episode_identifier (natural-key dedup)
    // =========================================================================

    #[test]
    fn test_identifier_two_provider_ids_same_session_collapse() {
        // The bug: the same real session arrives under two TheSportsDB event IDs.
        // Both must produce the SAME natural-key identifier so the UNIQUE constraint
        // collapses them to one episode row.
        let a = sport_episode_identifier(
            5000,
            Some("Hungary Free Practice 1 Moto3"),
            Some("ev-567724"),
        );
        let b =
            sport_episode_identifier(5001, Some("Hungary Free Practice 1 Moto3"), Some("2417416"));
        assert_eq!(a, "evt-r5-free-practice-1");
        assert_eq!(
            a, b,
            "two provider ids for one session must share an identifier"
        );
    }

    #[test]
    fn test_identifier_distinguishes_sessions_in_round() {
        let race = sport_episode_identifier(5003, Some("Hungary Grand Prix Moto3"), Some("p1"));
        let fp1 = sport_episode_identifier(5000, Some("Hungary Free Practice 1 Moto3"), Some("p2"));
        assert_eq!(race, "evt-r5-race");
        assert_ne!(race, fp1, "different sessions in a round stay distinct");
    }

    #[test]
    fn test_identifier_falls_back_for_non_motorsport() {
        // No recognisable session/location -> keep the provider-id form so team-sport
        // leagues are left untouched.
        let id = sport_episode_identifier(12, Some("Arsenal vs Chelsea"), Some("999"));
        assert_eq!(id, "evt-999");
    }

    #[test]
    fn test_identifier_falls_back_without_round() {
        // round 0 (no int_round) -> provider-id form, never evt-r0-*.
        let id = sport_episode_identifier(0, Some("Hungary Race Moto3"), Some("777"));
        assert_eq!(id, "evt-777");
    }

    /// WorldSBK titled its Superpole Race "{Round}  (Superpole)" or
    /// "{Round} Race (Superpole)" through 2023. The bracket kept the sponsor,
    /// "Round" and the demonym in the location.
    #[test]
    fn sbk_parenthesised_superpole_titles_give_the_place() {
        let cases = [
            ("Motul French Round  (Superpole)", "France"),
            ("Pirelli Dutch Round  (Superpole)", "Netherlands"),
            (
                "Grand Ridge Brewery Australian Round  (Superpole)",
                "Australia",
            ),
            ("Aragon Round Race (Superpole)", "Aragon"),
            ("Emilia Romagna Round Race (Superpole)", "Emilia Romagna"),
            ("Catalunya Round  (Superpole)", "Catalunya"),
        ];
        for (title, want) in cases {
            assert_eq!(extract_location(Some(title)), want, "location for {title}");
        }
    }

    /// TheSportsDB's F1 titles as Sportarr serves them for 2026: the Grand
    /// Prix, a dash, then the session, with practice numbered but not "Free".
    #[test]
    fn f1_wants_name_the_place_and_session() {
        let cases = [
            (
                "Australian Grand Prix - Practice 1",
                1000,
                "Australia",
                "Free Practice 1",
            ),
            (
                "Australian Grand Prix - Practice 3",
                1002,
                "Australia",
                "Free Practice 3",
            ),
            (
                "Australian Grand Prix - Qualifying",
                1003,
                "Australia",
                "Qualifying",
            ),
            ("Australian Grand Prix - Race", 1004, "Australia", "Race"),
            (
                "Miami Grand Prix - Sprint Qualifying",
                4001,
                "Miami",
                "Sprint Qualifying",
            ),
            ("Miami Grand Prix - Sprint", 4002, "Miami", "Sprint"),
            (
                "Barcelona-Catalunya Grand Prix - Practice 2",
                7001,
                "Barcelona-Catalunya",
                "Free Practice 2",
            ),
            ("British Grand Prix - Race", 9004, "Britain", "Race"),
        ];
        for (title, ord, location, session) in cases {
            let d = parse_sport_event_details(Some(title), "evt-123", ord);
            assert_eq!(
                (d.location.as_deref(), d.session_type.as_deref()),
                (Some(location), Some(session)),
                "{title}"
            );
        }
    }

    /// F1 rows are already stored under these identifiers; reading the
    /// titles better must not move them onto other keys.
    #[test]
    fn f1_identifiers_are_unchanged() {
        for (title, ord, identifier) in [
            ("Australian Grand Prix - Practice 1", 1000, "evt-123"),
            (
                "Australian Grand Prix - Qualifying",
                1003,
                "evt-r1-qualifying",
            ),
            ("Australian Grand Prix - Race", 1004, "evt-r1-race"),
            ("Miami Grand Prix - Sprint", 4002, "evt-r4-sprint"),
        ] {
            assert_eq!(
                sport_episode_identifier(ord, Some(title), Some("123")),
                identifier,
                "{title}"
            );
        }
    }

    #[test]
    fn sbk_parenthesised_superpole_wants_name_the_superpole_race() {
        for (title, ord) in [
            ("Motul French Round  (Superpole)", 8002),
            ("Aragon Round Race (Superpole)", 1001),
        ] {
            let d = parse_sport_event_details(Some(title), "evt-123", ord);
            assert_eq!(d.session_type.as_deref(), Some("Superpole Race"), "{title}");
        }
    }

    /// Those rows were stored under the provider-id form; a fix to the location
    /// must not move them onto a natural key and fork every one.
    #[test]
    fn sbk_parenthesised_superpole_identifiers_are_unchanged() {
        for (title, ord) in [
            ("Motul French Round  (Superpole)", 8002),
            ("Aragon Round Race (Superpole)", 1001),
        ] {
            assert_eq!(
                sport_episode_identifier(ord, Some(title), Some("123")),
                "evt-123",
                "identifier for {title}"
            );
        }
    }

    const AMERICAS_SCHEDULE: &str = r#"{"data":{"schedule":[
        {"idEvent":"ev-2561852","tsdbId":"2568439","strEvent":"USA - Free Practice 1","intRound":"3",
         "dateEvent":"2026-03-27","strTimestamp":"2026-03-27T15:45:00+00:00",
         "strVenue":"Circuit of the Americas","strCity":"Austin","strCountry":"US",
         "strFilename":"MotoGP 2026-03-27 USA Free Practice 1","strEventAlternate":null},
        {"idEvent":"ev-361462","tsdbId":"2424575","strEvent":"USA - Race","intRound":"3",
         "dateEvent":"2026-03-29","strTimestamp":"2026-03-29T19:00:00+00:00",
         "strVenue":null,"strCity":null,"strCountry":null,
         "strFilename":"MotoGP 2026-03-29 USA Race","strEventAlternate":"Grand Prix of the Americas"},
        {"idEvent":"ev-1","strEvent":"Grand Ridge Brewery Australian Round - Race 1","intRound":"1",
         "dateEvent":"2024-02-24","strTimestamp":"2024-02-24T00:00:00+00:00",
         "strVenue":"Phillip Island Grand Prix Circuit","strCity":"Phillip Island","strCountry":"AU"}
    ]}}"#;

    #[test]
    fn sportarr_rows_carry_venue_city_and_start_into_episodes() {
        let resp: SportarrResponse<SportarrScheduleData> =
            serde_json::from_str(AMERICAS_SCHEDULE).unwrap();
        let events = resp.data.unwrap().schedule;
        let episodes = episodes_from_events(&events);
        let by_id = |id: &str| {
            episodes
                .iter()
                .find(|e| e.provider_id.as_deref() == Some(id))
                .unwrap()
        };

        let fp1 = by_id("ev-2561852");
        assert_eq!(fp1.air_time.as_deref(), Some("2026-03-27T15:45:00+00:00"));
        assert_eq!(
            fp1.sport,
            Some(SportEventSource {
                venue: Some("Circuit of the Americas".into()),
                city: Some("Austin".into()),
                alternate_name: None,
                filename: Some("MotoGP 2026-03-27 USA Free Practice 1".into()),
                thesportsdb_id: Some("2568439".into()),
                ..Default::default()
            })
        );

        let race = by_id("ev-361462");
        let sport = race.sport.as_ref().unwrap();
        assert_eq!(sport.venue, None);
        assert_eq!(
            sport.alternate_name.as_deref(),
            Some("Grand Prix of the Americas")
        );
        assert_eq!(race.air_time.as_deref(), Some("2026-03-29T19:00:00+00:00"));
    }

    /// Older seasons stamp every session at midnight UTC: a date, not a time.
    #[test]
    fn sportarr_midnight_timestamps_are_not_a_start_time() {
        let resp: SportarrResponse<SportarrScheduleData> =
            serde_json::from_str(AMERICAS_SCHEDULE).unwrap();
        let episodes = episodes_from_events(&resp.data.unwrap().schedule);
        let round1 = episodes
            .iter()
            .find(|e| e.provider_id.as_deref() == Some("ev-1"))
            .unwrap();
        assert_eq!(round1.air_time, None);
    }

    #[test]
    fn sportarr_league_alternate_names_are_read() {
        let body = r#"{"id":"1e1c7821","shortId":"lg-000090","name":"SBK","tsdbId":"4454",
            "alternateNames":["SBK","WSB","WSBK","World Superbike"]}"#;
        assert_eq!(
            parse_league_alternate_names(body),
            vec!["SBK", "WSB", "WSBK", "World Superbike"]
        );
        assert!(parse_league_alternate_names(r#"{"alternateNames":[]}"#).is_empty());
        assert!(parse_league_alternate_names(r#"{"detail":"nope"}"#).is_empty());
    }

    #[test]
    fn sportarr_rows_carry_status_and_teams() {
        let body = r#"{"data":{"schedule":[{"idEvent":"ev-a","strEvent":"Arsenal vs Coventry City",
            "dateEvent":"2026-08-22","strStatus":"postponed","strHomeTeam":"Arsenal",
            "idHomeTeam":"tm-000006","strAwayTeam":"Coventry City","idAwayTeam":"tm-000032"}]}}"#;
        let resp: SportarrResponse<SportarrScheduleData> = serde_json::from_str(body).unwrap();
        let episodes = episodes_from_events(&resp.data.unwrap().schedule);
        let sport = episodes[0].sport.as_ref().unwrap();
        assert_eq!(sport.status.as_deref(), Some("postponed"));
        assert_eq!(sport.home_team.as_deref(), Some("Arsenal"));
        assert_eq!(sport.home_team_id.as_deref(), Some("tm-000006"));
        assert_eq!(sport.away_team.as_deref(), Some("Coventry City"));
        assert_eq!(sport.away_team_id.as_deref(), Some("tm-000032"));
    }

    #[test]
    fn sportarr_team_lookup_gives_short_name_and_nicknames() {
        let body = r#"{"data":[{"idTeam":"tm-000006","strTeam":"Arsenal","strTeamShort":"ARS",
            "strAlternate":"Gunners, The Arsenal,Arsenal"}]}"#;
        let team = parse_team(body).unwrap();
        assert_eq!(team.name, "Arsenal");
        assert_eq!(team.short.as_deref(), Some("ARS"));
        assert_eq!(team.aliases, vec!["Gunners", "The Arsenal"]);
        let bare = parse_team(
            r#"{"data":[{"strTeam":"Fulham","strTeamShort":null,"strAlternate":null}]}"#,
        )
        .unwrap();
        assert_eq!((bare.short, bare.aliases.len()), (None, 0));
        assert!(parse_team(r#"{"data":[]}"#).is_none());
    }
}
