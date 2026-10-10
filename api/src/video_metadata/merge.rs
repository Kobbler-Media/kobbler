use super::types::{EpisodeDetails, MovieResult, SeasonDetails, TvShowResult, VideoIdentifiers};
use std::collections::HashSet;

/// Merge multiple movie search results into one, using priority-based field selection.
/// First result has highest priority for scalar fields. Identifiers and genres are unioned.
pub fn merge_movie_results(results: &[MovieResult]) -> Option<MovieResult> {
    if results.is_empty() {
        return None;
    }

    let identifiers =
        merge_identifiers(&results.iter().map(|r| &r.identifiers).collect::<Vec<_>>());
    let genres = merge_genres(
        &results
            .iter()
            .map(|r| r.genres.as_slice())
            .collect::<Vec<_>>(),
    );

    Some(MovieResult {
        provider: "merged".to_string(),
        provider_id: results[0].provider_id.clone(),
        title: first_non_empty_str(results.iter().map(|r| &r.title)).unwrap_or_default(),
        year: first_some(results.iter().map(|r| r.year)),
        description: first_some(results.iter().map(|r| r.description.as_ref())).cloned(),
        poster_url: first_some(results.iter().map(|r| r.poster_url.as_ref())).cloned(),
        backdrop_url: first_some(results.iter().map(|r| r.backdrop_url.as_ref())).cloned(),
        rating: first_some(results.iter().map(|r| r.rating)),
        rating_count: first_some(results.iter().map(|r| r.rating_count)),
        identifiers,
        genres,
        network: first_some(results.iter().map(|r| r.network.as_ref())).cloned(),
    })
}

/// Merge multiple TV show search results into one.
pub fn merge_tv_results(results: &[TvShowResult]) -> Option<TvShowResult> {
    if results.is_empty() {
        return None;
    }

    let identifiers =
        merge_identifiers(&results.iter().map(|r| &r.identifiers).collect::<Vec<_>>());
    let genres = merge_genres(
        &results
            .iter()
            .map(|r| r.genres.as_slice())
            .collect::<Vec<_>>(),
    );

    Some(TvShowResult {
        provider: "merged".to_string(),
        provider_id: results[0].provider_id.clone(),
        title: first_non_empty_str(results.iter().map(|r| &r.title)).unwrap_or_default(),
        year: first_some(results.iter().map(|r| r.year)),
        status: first_some(results.iter().map(|r| r.status.as_ref())).cloned(),
        description: first_some(results.iter().map(|r| r.description.as_ref())).cloned(),
        poster_url: first_some(results.iter().map(|r| r.poster_url.as_ref())).cloned(),
        backdrop_url: first_some(results.iter().map(|r| r.backdrop_url.as_ref())).cloned(),
        network: first_some(results.iter().map(|r| r.network.as_ref())).cloned(),
        rating: first_some(results.iter().map(|r| r.rating)),
        rating_count: first_some(results.iter().map(|r| r.rating_count)),
        identifiers,
        genres,
    })
}

/// Merge season payloads from several providers, first one winning every
/// scalar it actually has.
///
/// The episode list is keyed on `episode_number` and the primary provider's
/// ordering is preserved; a later provider only ever fills gaps. That is what
/// keeps a TMDB result — which has no air time at all — from blanking the
/// `air_time` TVMaze supplied.
pub fn merge_season_details(results: &[SeasonDetails]) -> SeasonDetails {
    let primary = results
        .first()
        .expect("merge_season_details needs at least one result");

    let mut episodes: Vec<EpisodeDetails> = primary.episodes.clone();
    for other in &results[1..] {
        for ep in &other.episodes {
            match episodes
                .iter_mut()
                .find(|e| e.episode_number == ep.episode_number)
            {
                Some(existing) => fill_episode_gaps(existing, ep),
                None => episodes.push(ep.clone()),
            }
        }
    }

    SeasonDetails {
        provider: primary.provider.clone(),
        show_id: primary.show_id.clone(),
        season_number: primary.season_number,
        title: first_some(results.iter().map(|r| r.title.as_ref())).cloned(),
        air_date: first_some(results.iter().map(|r| r.air_date.as_ref())).cloned(),
        description: first_some(results.iter().map(|r| r.description.as_ref())).cloned(),
        poster_url: first_some(results.iter().map(|r| r.poster_url.as_ref())).cloned(),
        episodes,
    }
}

fn fill_episode_gaps(target: &mut EpisodeDetails, other: &EpisodeDetails) {
    if target.title.is_none() {
        target.title = other.title.clone();
    }
    if target.air_date.is_none() {
        target.air_date = other.air_date.clone();
    }
    if target.air_time.is_none() {
        target.air_time = other.air_time.clone();
    }
    if target.runtime.is_none() {
        target.runtime = other.runtime;
    }
    if target.still_url.is_none() {
        target.still_url = other.still_url.clone();
    }
    if target.description.is_none() {
        target.description = other.description.clone();
    }
    if target.country.is_none() {
        target.country = other.country.clone();
    }
    if target.credits.is_none() {
        target.credits = other.credits.clone();
    }
}

fn merge_identifiers(all: &[&VideoIdentifiers]) -> VideoIdentifiers {
    VideoIdentifiers {
        tmdb_id: first_some(all.iter().map(|i| i.tmdb_id.as_ref())).cloned(),
        imdb_id: first_some(all.iter().map(|i| i.imdb_id.as_ref())).cloned(),
        tvmaze_id: first_some(all.iter().map(|i| i.tvmaze_id.as_ref())).cloned(),
        tvdb_id: first_some(all.iter().map(|i| i.tvdb_id.as_ref())).cloned(),
    }
}

fn merge_genres(all: &[&[String]]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for genres in all {
        for genre in *genres {
            let key = genre.to_lowercase();
            if seen.insert(key) {
                result.push(genre.clone());
            }
        }
    }
    result
}

fn first_some<T: Copy>(mut iter: impl Iterator<Item = Option<T>>) -> Option<T> {
    iter.find_map(|v| v)
}

fn first_non_empty_str<'a>(mut iter: impl Iterator<Item = &'a String>) -> Option<String> {
    iter.find(|s| !s.is_empty()).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn movie(provider: &str) -> MovieResult {
        MovieResult {
            provider: provider.to_string(),
            provider_id: format!("{provider}-123"),
            title: String::new(),
            year: None,
            description: None,
            poster_url: None,
            backdrop_url: None,
            rating: None,
            rating_count: None,
            identifiers: VideoIdentifiers::default(),
            genres: vec![],
            network: None,
        }
    }

    fn tv(provider: &str) -> TvShowResult {
        TvShowResult {
            provider: provider.to_string(),
            provider_id: format!("{provider}-456"),
            title: String::new(),
            year: None,
            status: None,
            description: None,
            poster_url: None,
            backdrop_url: None,
            network: None,
            rating: None,
            rating_count: None,
            identifiers: VideoIdentifiers::default(),
            genres: vec![],
        }
    }

    #[test]
    fn empty_returns_none() {
        assert!(merge_movie_results(&[]).is_none());
        assert!(merge_tv_results(&[]).is_none());
    }

    #[test]
    fn scalar_fields_use_first_non_none() {
        let mut a = movie("tmdb");
        a.title = "The Matrix".to_string();
        a.year = None;
        a.rating = Some(4.35);

        let mut b = movie("omdb");
        b.title = "The Matrix (1999)".to_string();
        b.year = Some(1999);
        b.rating = Some(4.5);
        b.rating_count = Some(50000);

        let merged = merge_movie_results(&[a, b]).unwrap();
        assert_eq!(merged.provider, "merged");
        assert_eq!(merged.title, "The Matrix");
        assert_eq!(merged.year, Some(1999));
        assert_eq!(merged.rating, Some(4.35));
        assert_eq!(merged.rating_count, Some(50000));
    }

    #[test]
    fn identifiers_are_unioned() {
        let mut a = movie("tmdb");
        a.identifiers.tmdb_id = Some("550".to_string());

        let mut b = movie("omdb");
        b.identifiers.imdb_id = Some("tt0137523".to_string());

        let merged = merge_movie_results(&[a, b]).unwrap();
        assert_eq!(merged.identifiers.tmdb_id.as_deref(), Some("550"));
        assert_eq!(merged.identifiers.imdb_id.as_deref(), Some("tt0137523"));
    }

    #[test]
    fn genres_dedup_case_insensitive() {
        let mut a = movie("tmdb");
        a.genres = vec!["Action".to_string(), "Sci-Fi".to_string()];

        let mut b = movie("omdb");
        b.genres = vec!["action".to_string(), "Drama".to_string()];

        let merged = merge_movie_results(&[a, b]).unwrap();
        assert_eq!(merged.genres, vec!["Action", "Sci-Fi", "Drama"]);
    }

    fn ep(number: u32, air_date: Option<&str>, air_time: Option<&str>) -> EpisodeDetails {
        EpisodeDetails {
            episode_number: number,
            title: None,
            air_date: air_date.map(str::to_string),
            air_time: air_time.map(str::to_string),
            runtime: None,
            still_url: None,
            description: None,
            provider_id: None,
            country: None,
            credits: None,
        }
    }

    fn season(provider: &str, episodes: Vec<EpisodeDetails>) -> SeasonDetails {
        SeasonDetails {
            provider: provider.to_string(),
            show_id: "1396".to_string(),
            season_number: 24,
            title: None,
            air_date: None,
            description: None,
            poster_url: None,
            episodes,
        }
    }

    #[test]
    fn tmdb_first_does_not_blank_a_tvmaze_airstamp() {
        let tmdb = season("tmdb", vec![ep(7, Some("2026-09-04"), None)]);
        let tvmaze = season(
            "tvmaze",
            vec![ep(7, Some("2026-09-04"), Some("2026-09-04T21:00:00-04:00"))],
        );

        let merged = merge_season_details(&[tmdb, tvmaze]);
        assert_eq!(merged.provider, "tmdb");
        assert_eq!(merged.episodes.len(), 1);
        assert_eq!(
            merged.episodes[0].air_time.as_deref(),
            Some("2026-09-04T21:00:00-04:00")
        );
    }

    #[test]
    fn day_only_stays_distinguishable_from_day_and_time() {
        let tmdb = season("tmdb", vec![ep(7, Some("2026-09-04"), None)]);

        let alone = merge_season_details(std::slice::from_ref(&tmdb));
        assert_eq!(alone.episodes[0].air_date.as_deref(), Some("2026-09-04"));
        assert_eq!(
            alone.episodes[0].air_time, None,
            "no provider knew a time, so none may be invented"
        );

        let with_tvmaze = merge_season_details(&[
            tmdb,
            season("tvmaze", vec![ep(7, Some("2026-09-04"), None)]),
        ]);
        assert_eq!(with_tvmaze.episodes[0].air_time, None);
    }

    #[test]
    fn secondary_only_fills_gaps_and_adds_missing_episodes() {
        let primary = season(
            "tmdb",
            vec![ep(1, Some("2026-09-04"), Some("2026-09-04T20:00:00Z"))],
        );
        let secondary = season(
            "tvmaze",
            vec![
                ep(1, Some("1999-01-01"), Some("1999-01-01T00:00:00Z")),
                ep(2, Some("2026-09-11"), Some("2026-09-11T21:00:00-04:00")),
            ],
        );

        let merged = merge_season_details(&[primary, secondary]);
        assert_eq!(merged.episodes.len(), 2);
        assert_eq!(merged.episodes[0].air_date.as_deref(), Some("2026-09-04"));
        assert_eq!(
            merged.episodes[0].air_time.as_deref(),
            Some("2026-09-04T20:00:00Z")
        );
        assert_eq!(merged.episodes[1].episode_number, 2);
        assert_eq!(
            merged.episodes[1].air_time.as_deref(),
            Some("2026-09-11T21:00:00-04:00")
        );
    }

    #[test]
    fn tv_merge_works() {
        let mut a = tv("tmdb");
        a.title = "Breaking Bad".to_string();
        a.identifiers.tmdb_id = Some("1396".to_string());
        a.genres = vec!["Drama".to_string()];

        let mut b = tv("tvmaze");
        b.title = "Breaking Bad".to_string();
        b.network = Some("AMC".to_string());
        b.identifiers.tvmaze_id = Some("169".to_string());
        b.genres = vec!["drama".to_string(), "Crime".to_string()];
        b.rating_count = Some(10000);

        let merged = merge_tv_results(&[a, b]).unwrap();
        assert_eq!(merged.provider, "merged");
        assert_eq!(merged.network.as_deref(), Some("AMC"));
        assert_eq!(merged.identifiers.tmdb_id.as_deref(), Some("1396"));
        assert_eq!(merged.identifiers.tvmaze_id.as_deref(), Some("169"));
        assert_eq!(merged.genres, vec!["Drama", "Crime"]);
        assert_eq!(merged.rating_count, Some(10000));
    }
}
