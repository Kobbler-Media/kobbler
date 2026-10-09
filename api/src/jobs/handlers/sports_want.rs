//! Sports want job handler - enqueues want_request jobs to Kobload via NATS.
//!
//! Similar to tv_want but constructs MediaWant::SportEvent for sport-specific
//! search term generation (e.g., "MotoGP 2025 R01 Thailand Race").

use crate::LibraryEvent;
use crate::file_link_mode::FileLinkMode;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::{tv_episode_files, tv_episodes, tv_seasons, tv_shows};
use crate::runtime_config::RuntimeConfigManager;
use crate::video_metadata::thesportsdb::parse_sport_event_details;
use async_trait::async_trait;
use nats_jobs::Publisher;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use super::want_helpers::build_excluded_releases;

/// Payload for sports_want jobs.
#[derive(Debug, Deserialize, Serialize)]
pub struct SportsWantPayload {
    pub show_id: Uuid,
    pub season_id: Uuid,
    pub episode_ids: Option<Vec<Uuid>>,
    /// Who asked, when a person did. Schedulers leave this unset.
    #[serde(default)]
    pub user_id: Option<Uuid>,
    /// A regrab: the resulting file joins whatever is already there instead of
    /// replacing it. Everything else — schedulers, RSS, the UI — leaves this false.
    #[serde(default)]
    pub additional_copy: bool,
    /// A person asked for this event again right now. Kobload reopens an
    /// already-imported event instead of skipping it, and still picks under the
    /// quality profile.
    #[serde(default)]
    pub manual: bool,
}

/// Handler for wanting sport events via Kobload.
pub struct SportsWantHandler {
    pool: PgPool,
    runtime_config: RuntimeConfigManager,
    nats_publisher: Publisher,
    library_events: broadcast::Sender<LibraryEvent>,
}

impl SportsWantHandler {
    pub fn new(
        pool: PgPool,
        runtime_config: RuntimeConfigManager,
        nats_publisher: Publisher,
        library_events: broadcast::Sender<LibraryEvent>,
    ) -> Self {
        Self {
            pool,
            runtime_config,
            nats_publisher,
            library_events,
        }
    }
}

#[async_trait]
impl JobHandler for SportsWantHandler {
    fn job_type(&self) -> &'static str {
        "sports_want"
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        let payload: SportsWantPayload = serde_json::from_value(payload)
            .map_err(|e| JobError::ExecutionFailed(format!("Invalid payload: {}", e)))?;

        info!(
            job_id = %job_id,
            show_id = %payload.show_id,
            season_id = %payload.season_id,
            "Processing sports_want job"
        );

        let runtime = self.runtime_config.get().await;

        let callback_url = match &runtime.kobload_ingest_url {
            Some(url) if !url.trim().is_empty() => url.clone(),
            _ => {
                info!(show_id = %payload.show_id, "Kobload ingest URL not configured, skipping sports_want");
                return Ok(json!({
                    "show_id": payload.show_id,
                    "season_id": payload.season_id,
                    "status": "skipped",
                    "reason": "ingest_url_not_configured"
                }));
            }
        };

        let min_score = runtime.wanted_min_score.unwrap_or(0.65);

        let show = tv_shows::get(&self.pool, payload.show_id)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to fetch league: {}", e)))?
            .ok_or_else(|| JobError::ExecutionFailed("League not found".to_string()))?;

        let season = tv_seasons::get(&self.pool, payload.season_id)
            .await
            .map_err(|e| JobError::ExecutionFailed(format!("Failed to fetch season: {}", e)))?
            .ok_or_else(|| JobError::ExecutionFailed("Season not found".to_string()))?;

        // Extract search overrides from sport_metadata
        let search_title = show
            .sport_metadata
            .as_ref()
            .and_then(|m| m.get("search_title"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let location_aliases: std::collections::HashMap<String, String> = show
            .sport_metadata
            .as_ref()
            .and_then(|m| m.get("location_aliases"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let league_name = search_title.unwrap_or_else(|| show.title.clone());

        let mut affected_episode_ids = Vec::new();

        // Get episodes to want
        let episodes_to_want = if let Some(ref episode_ids) = payload.episode_ids {
            let mut eps = Vec::new();
            for ep_id in episode_ids {
                let ep = tv_episodes::get(&self.pool, *ep_id)
                    .await
                    .map_err(|e| {
                        JobError::ExecutionFailed(format!("Failed to fetch event: {}", e))
                    })?
                    .ok_or_else(|| {
                        JobError::ExecutionFailed(format!("Event not found: {}", ep_id))
                    })?;
                eps.push(ep);
            }
            eps
        } else {
            tv_episodes::list_missing(&self.pool, payload.season_id)
                .await
                .map_err(|e| {
                    JobError::ExecutionFailed(format!("Failed to list missing events: {}", e))
                })?
        };

        if episodes_to_want.is_empty() {
            info!(
                season_id = %payload.season_id,
                "No events to want, nothing to do"
            );
            return Ok(json!({
                "show_id": payload.show_id,
                "season_id": payload.season_id,
                "status": "skipped",
                "reason": "no_events_to_want"
            }));
        }

        let mut failed_episode_ids = Vec::new();

        for episode in &episodes_to_want {
            nats_jobs::check_cancelled(&cancel)?;
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

            // Normalize common country names to release-friendly short forms
            if let Some(ref loc) = details.location {
                details.location = Some(normalize_country_for_releases(loc).to_string());
            }

            // Apply user-configured location aliases (overrides built-in normalization)
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

            // Fetch library quality for this episode
            let (in_library, library_video_height) = if episode.is_in_library {
                let quality =
                    tv_episode_files::get_library_quality_batch(&self.pool, &[episode.id])
                        .await
                        .unwrap_or_default();
                match quality.first() {
                    Some(q) => (Some(true), q.video_height),
                    None => (Some(true), None),
                }
            } else {
                (Some(false), None)
            };

            let media_want = nats_jobs::MediaWant::SportEvent {
                league_name: league_name.clone(),
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
            };

            // Read rejected releases for this episode
            let rejected = tv_episodes::get_rejected_releases(&self.pool, episode.id)
                .await
                .unwrap_or(json!([]));
            let excluded_releases = build_excluded_releases(&rejected);

            // Allow quality upgrades for ~30 days after the event aired (mirrors tv_want):
            // a 720p grab can be replaced when a 1080p release lands later. The have/quality
            // signal is already sent via in_library + library_video_height above.
            let upgrade_allowed = episode.air_date.is_none_or(|d| {
                let cutoff = time::OffsetDateTime::now_utc().date() - time::Duration::days(30);
                d >= cutoff
            });

            let want_request = nats_jobs::WantRequest {
                source_system: "kobbler".to_string(),
                source_id: episode.id.to_string(),
                callback_url: callback_url.clone(),
                auth_token: None,
                query: None,
                patterns: Vec::new(),
                match_targets: Vec::new(),
                match_mode: "fuzzy".to_string(),
                min_score,
                early_exit_score: 0.85,
                media_type: Some("tv".to_string()),
                auto_download: true,
                limit: 100,
                metadata: Some(crate::file_link_mode::tag_metadata(
                    json!({
                        "source_type": "sport_event",
                        "title": format!(
                            "{} {} - {}",
                            show.title,
                            season.season_identifier,
                            episode.title.as_deref().unwrap_or(&episode.episode_identifier)
                        ),
                        "show_id": payload.show_id,
                        "season_id": payload.season_id,
                        "episode_id": episode.id,
                        "episode_identifier": episode.episode_identifier,
                        "user_id": payload.user_id,
                    }),
                    FileLinkMode::from_additional_copy(payload.additional_copy),
                )),
                media_want: Some(media_want),
                upgrade_allowed: Some(upgrade_allowed),
                manual: payload.manual,
                excluded_releases,
                search_not_before: None,
            };

            let job_payload = match serde_json::to_value(&want_request) {
                Ok(v) => v,
                Err(e) => {
                    warn!(
                        episode_id = %episode.id,
                        error = %e,
                        "Failed to serialize want_request, skipping"
                    );
                    failed_episode_ids.push(episode.id);
                    continue;
                }
            };

            match self
                .nats_publisher
                .enqueue_job("kobload", "want_request", job_payload)
                .await
            {
                Ok(_) => {
                    affected_episode_ids.push(episode.id);
                }
                Err(e) => {
                    warn!(
                        episode_id = %episode.id,
                        error = %e,
                        "Failed to enqueue want_request, continuing"
                    );
                    failed_episode_ids.push(episode.id);
                }
            }
        }

        // If ALL enqueues failed, return retryable error so NATS retries the whole job
        if affected_episode_ids.is_empty() && !episodes_to_want.is_empty() {
            return Err(JobError::retryable(format!(
                "All {} enqueue attempts failed",
                episodes_to_want.len(),
            )));
        }

        // If some failed, re-enqueue a new sports_want job with just the failed IDs
        if !failed_episode_ids.is_empty() {
            let retry_payload = json!({
                "show_id": payload.show_id,
                "season_id": payload.season_id,
                "episode_ids": failed_episode_ids,
                "user_id": payload.user_id,
                "additional_copy": payload.additional_copy,
            });
            match self
                .nats_publisher
                .enqueue_job("kobbler", "sports_want", retry_payload)
                .await
            {
                Ok(_) => {
                    info!(
                        failed_count = failed_episode_ids.len(),
                        "Re-enqueued sports_want for failed episodes"
                    );
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        failed_count = failed_episode_ids.len(),
                        "Failed to re-enqueue sports_want for failed episodes"
                    );
                }
            }
        }

        // Update want_status to "searching"
        if !affected_episode_ids.is_empty()
            && let Err(e) =
                tv_episodes::set_want_status_batch(&self.pool, &affected_episode_ids, "searching")
                    .await
        {
            warn!(error = %e, "Failed to set event want_status to searching");
        }

        let _ = self
            .library_events
            .send(LibraryEvent::tv_show_updated(payload.show_id));

        info!(
            show_id = %payload.show_id,
            season_id = %payload.season_id,
            events_enqueued = affected_episode_ids.len(),
            events_failed = failed_episode_ids.len(),
            "Sports want request(s) enqueued to Kobload"
        );

        Ok(json!({
            "show_id": payload.show_id,
            "season_id": payload.season_id,
            "status": "enqueued",
            "events_enqueued": affected_episode_ids.len(),
            "events_failed": failed_episode_ids.len(),
        }))
    }
}

/// Normalize common country names from providers to release-friendly short forms.
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

/// Whether a provider `strCountry` value looks like an ISO-3166 alpha-2 code
/// rather than a place name.
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

    /// Full country names already usable as search terms are passed through, and
    /// the few that scene releases spell differently are normalised.
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
        // A good value is untouched.
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
