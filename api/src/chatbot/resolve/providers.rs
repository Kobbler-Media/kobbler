//! Step two of the ladder: things that are not in the library yet.
//!
//! Two parallel stacks — `metadata` for books, `video_metadata` for everything
//! else — collapsed into one hit type so the seam never reaches the model.
//! Provider disagreement is folded with the existing `merge` modules rather
//! than shown as four near-identical rows.

use async_trait::async_trait;

use crate::metadata::similarity::{fuzzy_match_title, strip_punctuation};
use crate::metadata::{MetadataQuery, ProviderRegistry, merge as book_merge};
use crate::video_metadata::{MovieQuery, TvQuery, VideoProviderRegistry, merge as video_merge};

use super::types::{ProviderPayload, ShowSeason};

/// One collapsed provider result. `provider`/`id` name the provider the ref
/// points at; the display fields come from the merge across all of them.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderHit {
    pub provider: String,
    pub id: String,
    pub title: String,
    pub year: Option<i32>,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub rating: Option<f64>,
    pub payload: ProviderPayload,
}

#[async_trait]
pub trait MetadataLookup: Send + Sync {
    async fn books(&self, title: &str, author: Option<&str>) -> Vec<ProviderHit>;
    async fn movies(&self, title: &str, year: Option<i32>) -> Vec<ProviderHit>;
    /// `year` is the one the caller parsed out of the phrase. TMDB takes it in
    /// its own field and will not find anything with it left in the query.
    async fn shows(&self, title: &str, year: Option<i32>) -> Vec<ProviderHit>;
    /// The cast and crew names the provider has on file for a show, for
    /// matching a person the caller named ("with David Mitchell"). Empty when
    /// the provider cannot say.
    async fn tv_credits(&self, provider: &str, id: &str) -> Vec<String>;
    /// The seasons the provider has on file for a show, for answering "is
    /// season 2 out?" without guessing. Empty when the provider cannot say.
    async fn tv_seasons(&self, provider: &str, id: &str) -> Vec<ShowSeason>;
}

/// Providers are off. Used by callers that only want the library.
pub struct NoProviders;

#[async_trait]
impl MetadataLookup for NoProviders {
    async fn books(&self, _title: &str, _author: Option<&str>) -> Vec<ProviderHit> {
        Vec::new()
    }
    async fn movies(&self, _title: &str, _year: Option<i32>) -> Vec<ProviderHit> {
        Vec::new()
    }
    async fn shows(&self, _title: &str, _year: Option<i32>) -> Vec<ProviderHit> {
        Vec::new()
    }

    async fn tv_credits(&self, _provider: &str, _id: &str) -> Vec<String> {
        Vec::new()
    }
    async fn tv_seasons(&self, _provider: &str, _id: &str) -> Vec<ShowSeason> {
        Vec::new()
    }
}

/// Provider order for the ref itself. `want_it` posts to
/// `/admin/discover/want-episode` with a `{provider}_id` identifier key and to
/// `/admin/wanted` with book identifiers, so the ref must name a provider those
/// paths understand — and prefer the richest one when several agree.
const BOOK_REF_PRIORITY: &[&str] = &["hardcover", "googlebooks", "openlibrary", "audnexus"];
const VIDEO_REF_PRIORITY: &[&str] = &["tmdb", "tvdb", "tvmaze", "omdb", "cinemeta", "thesportsdb"];

fn rank_provider(priority: &[&str], provider: &str) -> usize {
    priority
        .iter()
        .position(|p| *p == provider)
        .unwrap_or(priority.len())
}

fn same_work(a_title: &str, a_year: Option<i32>, b_title: &str, b_year: Option<i32>) -> bool {
    if let (Some(a), Some(b)) = (a_year, b_year)
        && (a - b).abs() > 1
    {
        return false;
    }
    let a = strip_punctuation(&a_title.to_lowercase());
    let b = strip_punctuation(&b_title.to_lowercase());
    fuzzy_match_title(&a, &b)
}

/// Group provider results that describe the same work, so one thing is one
/// candidate.
fn group<T, F, G>(results: Vec<T>, title: F, year: G) -> Vec<Vec<T>>
where
    F: Fn(&T) -> String,
    G: Fn(&T) -> Option<i32>,
{
    let mut groups: Vec<Vec<T>> = Vec::new();
    for item in results {
        let (t, y) = (title(&item), year(&item));
        match groups
            .iter_mut()
            .find(|g| same_work(&title(&g[0]), year(&g[0]), &t, y))
        {
            Some(g) => g.push(item),
            None => groups.push(vec![item]),
        }
    }
    groups
}

pub struct RegistryLookup<'a> {
    pub books: &'a ProviderRegistry,
    pub video: &'a VideoProviderRegistry,
}

#[async_trait]
impl MetadataLookup for RegistryLookup<'_> {
    async fn books(&self, title: &str, author: Option<&str>) -> Vec<ProviderHit> {
        let query = match author {
            Some(a) => MetadataQuery::by_title_author(title, a),
            None => MetadataQuery::by_title(title),
        };
        let outcome = self.books.search(&query, None).await;
        let mut hits = Vec::new();
        for mut g in group(
            outcome.results,
            |r| r.title.clone(),
            |r| {
                r.publish_date
                    .as_deref()
                    .and_then(|d| d.get(..4))
                    .and_then(|y| y.parse().ok())
            },
        ) {
            g.sort_by_key(|r| rank_provider(BOOK_REF_PRIORITY, &r.provider));
            let Some(head) = g.first().cloned() else {
                continue;
            };
            let Some(external_id) = head.external_id.clone() else {
                continue;
            };
            let Some(merged) = book_merge::merge_by_priority(&g) else {
                continue;
            };
            let year = merged
                .publish_date
                .as_deref()
                .and_then(|d| d.get(..4))
                .and_then(|y| y.parse().ok());
            hits.push(ProviderHit {
                provider: head.provider.clone(),
                id: external_id.clone(),
                title: merged.title.clone(),
                year,
                authors: merged.authors.clone(),
                description: merged.description.clone(),
                rating: merged.rating,
                payload: ProviderPayload {
                    provider: head.provider,
                    provider_id: external_id,
                    title: merged.title,
                    year,
                    authors: merged.authors,
                    description: merged.description,
                    poster_url: merged.cover_url,
                    network: None,
                    genres: merged.subjects,
                    rating: merged.rating,
                    rating_count: merged.rating_count,
                    identifiers: merged.identifiers,
                },
            });
        }
        hits
    }

    async fn movies(&self, title: &str, year: Option<i32>) -> Vec<ProviderHit> {
        let query = MovieQuery {
            title: title.to_string(),
            year: year.and_then(|y| u16::try_from(y).ok()),
        };
        let outcome = self.video.search_movies(&query, None).await;
        let mut hits = Vec::new();
        for mut g in group(
            outcome.results,
            |r| r.title.clone(),
            |r| r.year.map(i32::from),
        ) {
            g.sort_by_key(|r| rank_provider(VIDEO_REF_PRIORITY, &r.provider));
            let Some(head) = g.first().cloned() else {
                continue;
            };
            let Some(merged) = video_merge::merge_movie_results(&g) else {
                continue;
            };
            hits.push(ProviderHit {
                provider: head.provider.clone(),
                id: head.provider_id.clone(),
                title: merged.title.clone(),
                year: merged.year.map(i32::from),
                authors: Vec::new(),
                description: merged.description.clone(),
                rating: merged.rating,
                payload: ProviderPayload {
                    provider: head.provider,
                    provider_id: head.provider_id,
                    title: merged.title,
                    year: merged.year.map(i32::from),
                    authors: Vec::new(),
                    description: merged.description,
                    poster_url: merged.poster_url,
                    network: merged.network,
                    genres: merged.genres,
                    rating: merged.rating,
                    rating_count: merged.rating_count,
                    identifiers: serde_json::to_value(&merged.identifiers)
                        .unwrap_or(serde_json::Value::Null),
                },
            });
        }
        hits
    }

    async fn shows(&self, title: &str, year: Option<i32>) -> Vec<ProviderHit> {
        let query = TvQuery {
            title: title.to_string(),
            year,
        };
        let outcome = self.video.search_tv(&query, None).await;
        let mut hits = Vec::new();
        for mut g in group(
            outcome.results,
            |r| r.title.clone(),
            |r| r.year.map(i32::from),
        ) {
            g.sort_by_key(|r| rank_provider(VIDEO_REF_PRIORITY, &r.provider));
            let Some(head) = g.first().cloned() else {
                continue;
            };
            let Some(merged) = video_merge::merge_tv_results(&g) else {
                continue;
            };
            hits.push(ProviderHit {
                provider: head.provider.clone(),
                id: head.provider_id.clone(),
                title: merged.title.clone(),
                year: merged.year.map(i32::from),
                authors: Vec::new(),
                description: merged.description.clone(),
                rating: merged.rating,
                payload: ProviderPayload {
                    provider: head.provider,
                    provider_id: head.provider_id,
                    title: merged.title,
                    year: merged.year.map(i32::from),
                    authors: Vec::new(),
                    description: merged.description,
                    poster_url: merged.poster_url,
                    network: merged.network,
                    genres: merged.genres,
                    rating: merged.rating,
                    rating_count: merged.rating_count,
                    identifiers: serde_json::to_value(&merged.identifiers)
                        .unwrap_or(serde_json::Value::Null),
                },
            });
        }
        hits
    }

    async fn tv_credits(&self, provider: &str, id: &str) -> Vec<String> {
        let Ok(details) = self.video.get_tv_show(provider, id).await else {
            return Vec::new();
        };
        let mut names: Vec<String> = details.cast.iter().map(|c| c.name.clone()).collect();
        names.extend(details.crew.iter().map(|c| c.name.clone()));
        names
    }
    async fn tv_seasons(&self, provider: &str, id: &str) -> Vec<ShowSeason> {
        let Ok(details) = self.video.get_tv_show(provider, id).await else {
            return Vec::new();
        };
        let mut seasons = details
            .seasons
            .iter()
            .map(|s| ShowSeason {
                season: s.season_number,
                episodes: s.episode_count,
                air_date: s.air_date.clone(),
            })
            .collect::<Vec<_>>();
        seasons.sort_by_key(|s| s.season);
        seasons
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_work_folds_provider_disagreement() {
        assert!(same_work("Dune", Some(2021), "Dune", Some(2021)));
        assert!(same_work("Dune", Some(2021), "Dune", None));
        // A reboot is not the same work.
        assert!(!same_work("Dune", Some(1984), "Dune", Some(2021)));
        assert!(!same_work("Dune", None, "Arrival", None));
    }

    #[test]
    fn grouping_collapses_agreeing_providers() {
        let rows = vec![
            ("tmdb", "Dune", Some(2021)),
            ("omdb", "Dune", Some(2021)),
            ("tmdb", "Dune", Some(1984)),
        ];
        let groups = group(rows, |r| r.1.to_string(), |r| r.2);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].len(), 2);
    }

    #[test]
    fn ref_provider_priority_prefers_the_stack_the_endpoints_speak() {
        assert!(
            rank_provider(BOOK_REF_PRIORITY, "hardcover")
                < rank_provider(BOOK_REF_PRIORITY, "openlibrary")
        );
        assert!(
            rank_provider(VIDEO_REF_PRIORITY, "tmdb") < rank_provider(VIDEO_REF_PRIORITY, "tvdb")
        );
        assert_eq!(
            rank_provider(VIDEO_REF_PRIORITY, "unknown"),
            VIDEO_REF_PRIORITY.len()
        );
    }
}
