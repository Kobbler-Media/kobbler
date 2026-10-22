//! The "For You" shelf: TMDB titles related to what a user has watched, minus what they
//! have already seen. Plain ranking, no model. TMDB's related lists are fetched per seed
//! title by the discover sync and cached, so a page load only reads the cache.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::jobs::handlers::discover_sync::{
    DiscoverItem, movie_to_discover_item, tv_to_discover_item,
};
use crate::repositories::{self, metadata_cache};
use crate::routes::admin::discover::{DiscoverItemWithStatus, attach_status};
use crate::video_metadata::tmdb::{TmdbProvider, TmdbRelation};

pub const RELATED_PROVIDER: &str = "tmdb_related";

/// Related lists change slowly; a week keeps TMDB traffic to new seeds.
const RELATED_TTL_SECS: i64 = 7 * 24 * 3600;
const SEED_COUNT: usize = 12;
const SHELF_LEN: usize = 20;
/// Fewer picks than this reads as noise rather than taste, so the shelf is hidden.
const MIN_SHELF: usize = 6;
const SIMILAR_WEIGHT: f64 = 0.4;

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub media_type: String,
    pub tmdb_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelatedTitles {
    pub recommendations: Vec<DiscoverItem>,
    pub similar: Vec<DiscoverItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Seed {
    pub media_type: String,
    pub tmdb_id: Option<i64>,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct Pick {
    pub item: DiscoverItem,
    pub because: Vec<Seed>,
}

#[derive(Debug, Serialize)]
pub struct ForYouItem {
    #[serde(flatten)]
    pub item: DiscoverItemWithStatus,
    pub because: Vec<String>,
    pub because_items: Vec<Seed>,
}

fn related_key(media_type: &str, tmdb_id: &str) -> String {
    format!("{media_type}:{tmdb_id}")
}

/// Every title the user has started, most recent first: library watch state plus the
/// history of their Stremio account (or the instance-wide one). Titles the user excluded
/// are left out, and a watch imported without a date sorts last rather than as new.
pub async fn history(pool: &PgPool, user_id: Uuid) -> sqlx::Result<Vec<HistoryEntry>> {
    let rows = sqlx::query!(
        r#"
        WITH acct AS (
            SELECT id FROM stremio_accounts
            WHERE user_id = $1 OR user_id IS NULL
            ORDER BY user_id NULLS LAST
            LIMIT 1
        ),
        started AS (
            SELECT 'movie' AS media_type, m.identifiers->>'tmdb_id' AS tmdb_id, m.title,
                   CASE WHEN ws.source = 'local' THEN COALESCE(ws.watched_at, ws.updated_at)
                        ELSE ws.watched_at END AS at
            FROM movie_watch_states ws
            JOIN movies m ON m.id = ws.movie_id
            WHERE ws.user_id = $1 AND (ws.watched OR ws.position_seconds > 0)
            UNION ALL
            SELECT 'tv', s.identifiers->>'tmdb_id', s.title,
                   CASE WHEN ws.source = 'local' THEN COALESCE(ws.watched_at, ws.updated_at)
                        ELSE ws.watched_at END
            FROM tv_watch_states ws
            JOIN tv_episodes e ON e.id = ws.episode_id
            JOIN tv_seasons se ON se.id = e.season_id
            JOIN tv_shows s ON s.id = se.show_id
            WHERE ws.user_id = $1 AND (ws.watched OR ws.position_seconds > 0)
            UNION ALL
            SELECT h.media_type, h.tmdb_id, COALESCE(h.title, h.name), h.last_watched
            FROM stremio_history_items h
            JOIN acct ON acct.id = h.account_id
        )
        SELECT media_type AS "media_type!",
               tmdb_id AS "tmdb_id!",
               (array_agg(title ORDER BY at DESC NULLS LAST))[1] AS "title!"
        FROM started
        WHERE tmdb_id IS NOT NULL AND tmdb_id <> ''
          AND NOT EXISTS (
              SELECT 1 FROM watch_history_exclusions x
              WHERE x.user_id = $1 AND x.media_type = started.media_type
                AND x.tmdb_id = started.tmdb_id
          )
        GROUP BY media_type, tmdb_id
        ORDER BY MAX(at) DESC NULLS LAST, media_type, tmdb_id
        "#,
        user_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| HistoryEntry {
            media_type: r.media_type,
            tmdb_id: r.tmdb_id,
            title: r.title,
        })
        .collect())
}

/// Fetches related lists for every user's seeds that the cache does not already hold.
pub async fn refresh_related(pool: &PgPool, tmdb: &TmdbProvider) -> (usize, Vec<String>) {
    let mut errors = Vec::new();
    let users = match sqlx::query_scalar!("SELECT id FROM users")
        .fetch_all(pool)
        .await
    {
        Ok(users) => users,
        Err(e) => return (0, vec![format!("Failed to list users for For You: {e}")]),
    };

    let mut seeds: Vec<HistoryEntry> = Vec::new();
    let mut queued = HashSet::new();
    for user_id in users {
        match history(pool, user_id).await {
            Ok(entries) => {
                for entry in entries.into_iter().take(SEED_COUNT) {
                    if queued.insert(related_key(&entry.media_type, &entry.tmdb_id)) {
                        seeds.push(entry);
                    }
                }
            }
            Err(e) => errors.push(format!("Failed to read watch history for {user_id}: {e}")),
        }
    }

    let expires = OffsetDateTime::now_utc() + time::Duration::seconds(RELATED_TTL_SECS);
    let mut fetched = 0;
    for seed in seeds {
        let key = related_key(&seed.media_type, &seed.tmdb_id);
        if matches!(
            metadata_cache::get(pool, RELATED_PROVIDER, &key).await,
            Ok(Some(_))
        ) {
            continue;
        }
        let related = match fetch_related(tmdb, &seed).await {
            Ok(related) => related,
            Err(e) => {
                errors.push(format!("TMDB related titles for {}: {e}", seed.title));
                continue;
            }
        };
        match metadata_cache::set(pool, RELATED_PROVIDER, &key, &json!(related), expires).await {
            Ok(()) => fetched += 1,
            Err(e) => errors.push(format!("Failed to cache related titles for {key}: {e}")),
        }
    }
    (fetched, errors)
}

async fn fetch_related(
    tmdb: &TmdbProvider,
    seed: &HistoryEntry,
) -> Result<RelatedTitles, crate::video_metadata::VideoProviderError> {
    let id = seed.tmdb_id.as_str();
    Ok(match seed.media_type.as_str() {
        "movie" => RelatedTitles {
            recommendations: tmdb
                .related_movies(id, TmdbRelation::Recommendations)
                .await?
                .into_iter()
                .map(movie_to_discover_item)
                .collect(),
            similar: tmdb
                .related_movies(id, TmdbRelation::Similar)
                .await?
                .into_iter()
                .map(movie_to_discover_item)
                .collect(),
        },
        _ => RelatedTitles {
            recommendations: tmdb
                .related_tv(id, TmdbRelation::Recommendations)
                .await?
                .into_iter()
                .map(tv_to_discover_item)
                .collect(),
            similar: tmdb
                .related_tv(id, TmdbRelation::Similar)
                .await?
                .into_iter()
                .map(tv_to_discover_item)
                .collect(),
        },
    })
}

/// Recent seeds outweigh old ones, a seed's top recommendations outweigh its tail, and a
/// title several seeds point at climbs above any one seed's favourite.
pub fn rank(
    seeds: &[(HistoryEntry, RelatedTitles)],
    seen: &HashSet<(String, String)>,
) -> Vec<Pick> {
    struct Candidate {
        item: DiscoverItem,
        score: f64,
        by_seed: Vec<(usize, f64)>,
    }

    let mut candidates: HashMap<(String, String), Candidate> = HashMap::new();
    let seed_count = seeds.len().max(1) as f64;
    for (i, (_, related)) in seeds.iter().enumerate() {
        let seed_weight = 1.0 - 0.5 * i as f64 / seed_count;
        for (list, list_weight) in [
            (&related.recommendations, 1.0),
            (&related.similar, SIMILAR_WEIGHT),
        ] {
            let len = list.len().max(1) as f64;
            for (pos, item) in list.iter().enumerate() {
                let key = (item.media_type.clone(), item.provider_id.clone());
                if seen.contains(&key) || item.poster_url.is_none() {
                    continue;
                }
                let weight = seed_weight * list_weight * (1.0 - 0.5 * pos as f64 / len);
                let candidate = candidates.entry(key).or_insert_with(|| Candidate {
                    item: item.clone(),
                    score: 0.0,
                    by_seed: Vec::new(),
                });
                candidate.score += weight;
                match candidate.by_seed.iter_mut().find(|(s, _)| *s == i) {
                    Some((_, w)) => *w += weight,
                    None => candidate.by_seed.push((i, weight)),
                }
            }
        }
    }

    let mut ranked: Vec<Candidate> = candidates.into_values().collect();
    ranked.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.item.media_type.cmp(&b.item.media_type))
            .then_with(|| a.item.provider_id.cmp(&b.item.provider_id))
    });
    ranked
        .into_iter()
        .map(|mut c| {
            c.by_seed.sort_by(|a, b| b.1.total_cmp(&a.1));
            Pick {
                item: c.item,
                because: c
                    .by_seed
                    .into_iter()
                    .map(|(i, _)| {
                        let seed = &seeds[i].0;
                        Seed {
                            media_type: seed.media_type.clone(),
                            tmdb_id: seed.tmdb_id.parse().ok(),
                            title: seed.title.clone(),
                        }
                    })
                    .collect(),
            }
        })
        .collect()
}

/// The shelf as stored, with TMDB art URLs still raw so the sync can warm them.
pub async fn picks(pool: &PgPool, user_id: Uuid, is_kids: bool) -> repositories::Result<Vec<Pick>> {
    let history = history(pool, user_id).await?;
    if history.is_empty() {
        return Ok(Vec::new());
    }
    let mut seen: HashSet<(String, String)> = history
        .iter()
        .map(|h| (h.media_type.clone(), h.tmdb_id.clone()))
        .collect();
    seen.extend(repositories::for_you_dismissals::list(pool, user_id).await?);
    let seeds: Vec<HistoryEntry> = history.into_iter().take(SEED_COUNT).collect();
    let keys: Vec<String> = seeds
        .iter()
        .map(|s| related_key(&s.media_type, &s.tmdb_id))
        .collect();
    let mut cached: HashMap<String, RelatedTitles> =
        metadata_cache::get_many(pool, RELATED_PROVIDER, &keys)
            .await?
            .into_iter()
            .filter_map(|(key, value)| Some((key, serde_json::from_value(value).ok()?)))
            .collect();
    let seeds: Vec<(HistoryEntry, RelatedTitles)> = seeds
        .into_iter()
        .zip(keys)
        .filter_map(|(seed, key)| Some((seed, cached.remove(&key)?)))
        .collect();

    let mut shelf = rank(&seeds, &seen);
    if is_kids {
        let titles: Vec<(String, String)> = shelf
            .iter()
            .map(|p| (p.item.media_type.clone(), p.item.provider_id.clone()))
            .collect();
        let allowed = crate::kids::discover_titles_allowed(pool, &titles).await?;
        shelf
            .retain(|p| allowed.contains(&(p.item.media_type.clone(), p.item.provider_id.clone())));
    }
    shelf.truncate(SHELF_LEN);
    if shelf.len() < MIN_SHELF {
        shelf.clear();
    }
    Ok(shelf)
}

pub async fn for_you(pool: &PgPool, user_id: Uuid, is_kids: bool) -> Vec<ForYouItem> {
    let picks = match picks(pool, user_id, is_kids).await {
        Ok(picks) => picks,
        Err(e) => {
            tracing::warn!("Failed to build For You: {e}");
            return Vec::new();
        }
    };
    let (items, because): (Vec<DiscoverItem>, Vec<Vec<Seed>>) =
        picks.into_iter().map(|p| (p.item, p.because)).unzip();
    attach_status(pool, items)
        .await
        .into_iter()
        .zip(because)
        .map(|(item, because_items)| ForYouItem {
            item,
            because: because_items.iter().map(|s| s.title.clone()).collect(),
            because_items,
        })
        .collect()
}

/// Every user's shelf art, for the sync to keep on disk.
pub async fn art_sources(pool: &PgPool) -> Vec<String> {
    let users = match sqlx::query!("SELECT id, is_kids FROM users")
        .fetch_all(pool)
        .await
    {
        Ok(users) => users,
        Err(e) => {
            tracing::warn!("Failed to list users for For You art: {e}");
            return Vec::new();
        }
    };
    let mut srcs = Vec::new();
    for user in users {
        if let Ok(picks) = picks(pool, user.id, user.is_kids).await {
            srcs.extend(
                picks
                    .into_iter()
                    .flat_map(|p| [p.item.backdrop_url, p.item.poster_url])
                    .flatten(),
            );
        }
    }
    srcs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(media_type: &str, id: &str) -> DiscoverItem {
        DiscoverItem {
            provider: "tmdb".into(),
            provider_id: id.into(),
            media_type: media_type.into(),
            title: format!("Title {id}"),
            year: None,
            description: None,
            poster_url: Some(format!("https://image.tmdb.org/t/p/w500/{id}.jpg")),
            backdrop_url: None,
            genres: vec![],
            rating: None,
            rating_count: None,
            authors: vec![],
            identifiers: json!({ "tmdb_id": id }),
        }
    }

    fn seed(title: &str, recs: &[&str], similar: &[&str]) -> (HistoryEntry, RelatedTitles) {
        (
            HistoryEntry {
                media_type: "tv".into(),
                tmdb_id: title.into(),
                title: title.into(),
            },
            RelatedTitles {
                recommendations: recs.iter().map(|id| item("tv", id)).collect(),
                similar: similar.iter().map(|id| item("tv", id)).collect(),
            },
        )
    }

    fn ids(picks: &[Pick]) -> Vec<&str> {
        picks.iter().map(|p| p.item.provider_id.as_str()).collect()
    }

    #[test]
    fn titles_several_seeds_agree_on_lead_the_shelf() {
        let seeds = [
            seed("A", &["1", "2", "3"], &[]),
            seed("B", &["3", "4"], &[]),
        ];
        let picks = rank(&seeds, &HashSet::new());
        assert_eq!(ids(&picks)[0], "3");
        assert_eq!(picks[0].because.len(), 2);
    }

    #[test]
    fn seen_titles_and_posterless_ones_are_dropped() {
        let mut seeds = [seed("A", &["1", "2", "3"], &[])];
        seeds[0].1.recommendations[2].poster_url = None;
        let seen = HashSet::from([("tv".to_string(), "1".to_string())]);
        assert_eq!(ids(&rank(&seeds, &seen)), vec!["2"]);
    }

    #[test]
    fn recommendations_outrank_similar_and_recent_seeds_outrank_old() {
        let seeds = [seed("Recent", &["r"], &["s"]), seed("Old", &["o"], &[])];
        assert_eq!(ids(&rank(&seeds, &HashSet::new())), vec!["r", "o", "s"]);
    }

    #[test]
    fn because_names_the_strongest_seed_first() {
        let seeds = [seed("Weak", &[], &["x"]), seed("Strong", &["x"], &[])];
        let picks = rank(&seeds, &HashSet::new());
        let titles: Vec<&str> = picks[0].because.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, vec!["Strong", "Weak"]);
    }
}
