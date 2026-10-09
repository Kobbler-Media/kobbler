//! Candidate ranking.
//!
//! "The new one" and "that baking show" are won or lost here, not in the
//! prompt. Every signal is named and weighted separately so a bad answer can be
//! traced to a signal rather than to a mood.

use super::text;

pub const W_EXACT_TITLE: f64 = 0.40;
pub const W_PREFIX: f64 = 0.10;
pub const W_TOKEN_OVERLAP: f64 = 0.18;
/// The query is a contiguous run inside the candidate's title and covers a
/// decent share of it: "sanderson" is "Brandon Sanderson", "bake off" is the
/// show rather than one of its episodes.
pub const W_PARTIAL_NAME: f64 = 0.15;
pub const W_FUZZY: f64 = 0.12;
pub const W_DESCRIPTION: f64 = 0.35;
pub const W_IN_LIBRARY: f64 = 0.10;
pub const W_YEAR: f64 = 0.18;
pub const W_MEDIA_TYPE_HINT: f64 = 0.10;
pub const W_AUTHOR: f64 = 0.12;
pub const W_POPULARITY: f64 = 0.04;
/// Recency is a tie-breaker normally and the whole answer when the caller said
/// "latest" / "new" / "first". The quiet weight is deliberately below
/// `AMBIGUITY_MARGIN`: two same-titled films are a question, not a coin flip on
/// which is newer.
pub const W_RECENCY: f64 = 0.03;
pub const W_RECENCY_REQUESTED: f64 = 0.35;
pub const W_AVAILABILITY: f64 = 0.03;
pub const W_FORMAT_AVAILABLE: f64 = 0.04;

/// Below this a candidate is not worth offering, and the resolver falls
/// through to the metadata providers. Low because the index queries run with
/// `matchingStrategy: all`, so everything that comes back already contains
/// every query term.
pub const MIN_PLAUSIBLE: f64 = 0.30;
/// The subject is what the words name. A book that matched only because its
/// author's name appeared in the query is not what was asked for.
pub const MIN_TITLE_OVERLAP: f64 = 0.34;
/// Two candidates this close are a question, not a guess.
pub const AMBIGUITY_MARGIN: f64 = 0.10;
/// A wide result set drowns a 12B model.
pub const MAX_CANDIDATES: usize = 5;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RankSignals {
    /// 1.0 for a normalized exact match, 0.75 when every token is equivalent
    /// but the spelling differs, 0 otherwise.
    pub exact_title: f64,
    pub prefix: bool,
    /// Dice coefficient over equivalent tokens.
    pub token_overlap: f64,
    /// The query sits inside the title as a contiguous run worth a real share
    /// of it.
    pub partial_name: bool,
    pub fuzzy: f64,
    /// Fraction of the query's tokens found in the synopsis.
    pub description_hit: f64,
    pub in_library: bool,
    /// `Some(false)` means the caller named a year and this is not it.
    pub year_match: Option<bool>,
    pub media_type_hint: Option<bool>,
    pub author_match: Option<bool>,
    /// Rating, normalized across the candidate pool.
    pub popularity: f64,
    /// Newness, normalized across the candidate pool.
    pub recency: f64,
    /// The caller asked for the newest/oldest, so recency is the answer rather
    /// than a tie-breaker.
    pub recency_requested: bool,
    /// How much of a show is actually on disk.
    pub availability: f64,
    /// The requested book format exists for this title.
    pub format_available: Option<bool>,
}

pub fn rank_score(s: &RankSignals) -> f64 {
    let mut score = 0.0;
    score += W_EXACT_TITLE * s.exact_title;
    if s.prefix {
        score += W_PREFIX;
    }
    score += W_TOKEN_OVERLAP * s.token_overlap;
    if s.partial_name {
        score += W_PARTIAL_NAME;
    }
    score += W_FUZZY * s.fuzzy;
    score += W_DESCRIPTION * s.description_hit;
    if s.in_library {
        score += W_IN_LIBRARY;
    }
    score += signed(s.year_match, W_YEAR);
    score += signed(s.media_type_hint, W_MEDIA_TYPE_HINT);
    score += signed(s.author_match, W_AUTHOR);
    score += W_POPULARITY * s.popularity;
    let recency_weight = if s.recency_requested {
        W_RECENCY_REQUESTED
    } else {
        W_RECENCY
    };
    score += recency_weight * s.recency;
    score += W_AVAILABILITY * s.availability;
    match s.format_available {
        Some(true) => score += W_FORMAT_AVAILABLE,
        Some(false) => score -= W_FORMAT_AVAILABLE / 2.0,
        None => {}
    }
    score.max(0.0)
}

fn signed(flag: Option<bool>, weight: f64) -> f64 {
    match flag {
        Some(true) => weight,
        Some(false) => -weight,
        None => 0.0,
    }
}

/// What a title match looks like, before weighting.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TitleSignals {
    pub exact: f64,
    pub prefix: bool,
    pub overlap: f64,
    pub partial_name: bool,
    pub fuzzy: f64,
}

impl TitleSignals {
    fn key(&self) -> (f64, f64, bool, f64) {
        (self.exact, self.overlap, self.partial_name, self.fuzzy)
    }

    /// A candidate the words did not actually name. Books that matched only
    /// through their author, episodes that matched only through their show.
    pub fn names_the_subject(&self) -> bool {
        self.exact > 0.0 || self.partial_name || self.overlap >= MIN_TITLE_OVERLAP
    }
}

/// How much of the candidate's title the query has to cover for a run inside
/// it to mean the caller named that thing.
const PARTIAL_NAME_SHARE: f64 = 0.4;

/// The query's tokens appear consecutively inside the candidate's, and take up
/// a real share of them.
fn contiguous_run(query: &[String], candidate: &[String]) -> bool {
    if query.is_empty() || candidate.len() < query.len() {
        return false;
    }
    if (query.len() as f64) / (candidate.len() as f64) < PARTIAL_NAME_SHARE {
        return false;
    }
    candidate.windows(query.len()).any(|window| {
        window
            .iter()
            .zip(query)
            .all(|(c, q)| text::token_equiv(c, q))
    })
}

/// Title signals, taking the best over a candidate's title variants — an
/// episode is findable by its own title as well as by "Show S02E05 Title".
///
/// Both sides are reduced to content tokens first, so a leading "The" on the
/// title does not turn an exact match into a partial one.
pub fn title_signals(query: &str, variants: &[String]) -> TitleSignals {
    let q_tokens = text::content_tokens(query);
    if q_tokens.is_empty() {
        return TitleSignals::default();
    }
    let q_norm = q_tokens.join(" ");
    let mut q_sorted = q_tokens.clone();
    q_sorted.sort();
    let q_joined = q_sorted.join(" ");

    let mut best = TitleSignals::default();
    for variant in variants {
        let v_tokens = text::content_tokens(variant);
        if v_tokens.is_empty() {
            continue;
        }
        let v_norm = v_tokens.join(" ");
        let overlap = text::dice(&q_tokens, &v_tokens);
        let exact = if v_norm == q_norm {
            1.0
        } else if v_tokens.len() == q_tokens.len() && (overlap - 1.0).abs() < 1e-9 {
            0.75
        } else {
            0.0
        };
        let mut v_sorted = v_tokens.clone();
        v_sorted.sort();
        let candidate = TitleSignals {
            exact,
            prefix: v_norm.starts_with(&q_norm),
            overlap,
            partial_name: contiguous_run(&q_tokens, &v_tokens),
            fuzzy: text::jaro(&q_joined, &v_sorted.join(" ")),
        };
        if candidate.key() > best.key() {
            best = candidate;
        }
    }
    best
}

/// Normalize a set of raw values to 0..1 by position, so a two-item pool gets a
/// decisive gap rather than a gap that depends on absolute magnitudes.
pub fn normalize_ranks(values: &[Option<f64>]) -> Vec<f64> {
    let mut present: Vec<f64> = values.iter().filter_map(|v| *v).collect();
    if present.is_empty() {
        return vec![0.0; values.len()];
    }
    present.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    present.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let span = present.len().saturating_sub(1) as f64;
    values
        .iter()
        .map(|v| match v {
            None => 0.0,
            Some(x) => {
                if span == 0.0 {
                    1.0
                } else {
                    let pos = present
                        .iter()
                        .position(|p| (*p - *x).abs() < 1e-9)
                        .unwrap_or(0);
                    pos as f64 / span
                }
            }
        })
        .collect()
}

/// Sort in place, best first. Stable on ties so index order is the tie-break.
pub fn sort_by_score<T>(items: &mut [T], score: impl Fn(&T) -> f64) {
    items.sort_by(|a, b| {
        score(b)
            .partial_cmp(&score(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> RankSignals {
        RankSignals::default()
    }

    #[test]
    fn exact_title_dominates_a_partial_match() {
        let exact = RankSignals {
            exact_title: 1.0,
            token_overlap: 1.0,
            fuzzy: 1.0,
            prefix: true,
            ..base()
        };
        let partial = RankSignals {
            token_overlap: 0.4,
            fuzzy: 0.6,
            ..base()
        };
        assert!(rank_score(&exact) > rank_score(&partial) + AMBIGUITY_MARGIN);
    }

    #[test]
    fn in_library_breaks_a_tie_with_a_provider_hit() {
        let mine = RankSignals {
            exact_title: 1.0,
            in_library: true,
            ..base()
        };
        let theirs = RankSignals {
            exact_title: 1.0,
            ..base()
        };
        assert!(rank_score(&mine) > rank_score(&theirs));
        assert!((rank_score(&mine) - rank_score(&theirs) - W_IN_LIBRARY).abs() < 1e-9);
    }

    #[test]
    fn a_named_year_that_does_not_match_is_a_penalty_not_a_shrug() {
        let right = RankSignals {
            exact_title: 1.0,
            year_match: Some(true),
            ..base()
        };
        let wrong = RankSignals {
            exact_title: 1.0,
            year_match: Some(false),
            ..base()
        };
        let unstated = RankSignals {
            exact_title: 1.0,
            ..base()
        };
        assert!(rank_score(&right) > rank_score(&unstated));
        assert!(rank_score(&unstated) > rank_score(&wrong));
        assert!(rank_score(&right) - rank_score(&wrong) > AMBIGUITY_MARGIN);
    }

    #[test]
    fn media_type_hint_and_author_cut_both_ways() {
        for (signal, weight) in [
            (
                |v| RankSignals {
                    media_type_hint: Some(v),
                    ..base()
                },
                W_MEDIA_TYPE_HINT,
            ),
            (
                |v| RankSignals {
                    author_match: Some(v),
                    ..base()
                },
                W_AUTHOR,
            ),
        ] as [(fn(bool) -> RankSignals, f64); 2]
        {
            let hit = rank_score(&signal(true));
            let miss = rank_score(&signal(false));
            assert!((hit - weight).abs() < 1e-9);
            assert_eq!(miss, 0.0, "score floors at zero");
        }
    }

    #[test]
    fn asking_for_the_latest_makes_recency_decisive() {
        let newest = RankSignals {
            token_overlap: 0.5,
            recency: 1.0,
            recency_requested: true,
            ..base()
        };
        let oldest = RankSignals {
            token_overlap: 0.5,
            recency: 0.0,
            recency_requested: true,
            ..base()
        };
        assert!(rank_score(&newest) - rank_score(&oldest) > AMBIGUITY_MARGIN);

        let quiet_new = RankSignals {
            recency_requested: false,
            ..newest.clone()
        };
        let quiet_old = RankSignals {
            recency_requested: false,
            ..oldest.clone()
        };
        assert!(rank_score(&quiet_new) - rank_score(&quiet_old) < AMBIGUITY_MARGIN);
    }

    #[test]
    fn description_carries_a_query_the_title_cannot() {
        let described = RankSignals {
            description_hit: 1.0,
            in_library: true,
            ..base()
        };
        assert!(rank_score(&described) > MIN_PLAUSIBLE);
    }

    #[test]
    fn title_signals_pick_the_best_variant() {
        let variants = vec![
            "Monaco Grand Prix".to_string(),
            "Formula 1 S2026E08 Monaco Grand Prix".to_string(),
        ];
        let t = title_signals("the monaco grand prix", &variants);
        assert_eq!(t.exact, 1.0);
        assert!((t.overlap - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_leading_article_does_not_cost_an_exact_match() {
        let t = title_signals("the great escape", &["The Great Escape".to_string()]);
        assert_eq!(t.exact, 1.0);
        assert!(t.prefix);
    }

    #[test]
    fn title_signals_tolerate_a_typo_without_calling_it_exact() {
        let t = title_signals("arival", &["Arrival".to_string()]);
        assert_eq!(t.exact, 0.75);
        assert!((t.overlap - 1.0).abs() < 1e-9);
        assert!(t.fuzzy > 0.9);
    }

    #[test]
    fn title_signals_do_not_confuse_two_different_titles() {
        let t = title_signals(
            "the great escape",
            &["The Great British Bake Off".to_string()],
        );
        assert_eq!(t.exact, 0.0);
        assert!(t.overlap < 0.4, "{}", t.overlap);
        assert!(!t.names_the_subject());
    }

    #[test]
    fn a_surname_names_the_author_but_not_their_books() {
        let author = title_signals("sanderson", &["Brandon Sanderson".to_string()]);
        assert!(author.partial_name);
        assert!(author.names_the_subject());

        let book = title_signals("sanderson", &["Rhythm of War".to_string()]);
        assert!(!book.names_the_subject());
    }

    #[test]
    fn a_show_outranks_its_own_episodes_on_the_shows_name() {
        let show = title_signals("bake off", &["The Great British Bake Off".to_string()]);
        let episode = title_signals(
            "bake off",
            &["The Great British Bake Off S12E01 Cake Week".to_string()],
        );
        assert!(show.partial_name);
        assert!(!episode.partial_name);
        assert!(show.overlap > episode.overlap);
    }

    #[test]
    fn normalize_ranks_spreads_a_two_item_pool() {
        let out = normalize_ranks(&[Some(2020.0), Some(2010.0)]);
        assert_eq!(out, vec![1.0, 0.0]);
        assert_eq!(normalize_ranks(&[Some(5.0), Some(5.0)]), vec![1.0, 1.0]);
        assert_eq!(normalize_ranks(&[None, Some(1.0)]), vec![0.0, 1.0]);
        assert_eq!(normalize_ranks(&[]), Vec::<f64>::new());
    }
}
