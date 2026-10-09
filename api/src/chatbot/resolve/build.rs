//! Search documents and provider hits, turned into scored candidates.

use uuid::Uuid;

use crate::search::{
    AuthorDocument, BookDocument, MovieDocument, SeriesDocument, TvEpisodeDocument,
    TvSeasonDocument, TvShowDocument,
};

use super::phrasing::Relative;
use super::providers::ProviderHit;
use super::ranking::{self, RankSignals};
use super::refs::{MediaKind, MediaRef};
use super::text;
use super::types::{Candidate, CandidateSource, MediaTypeHint, WantFormat};

/// A candidate plus everything the pool-level normalization still needs.
#[derive(Debug, Clone)]
pub struct Scored {
    pub candidate: Candidate,
    pub signals: RankSignals,
    pub rating: Option<f64>,
    /// Higher is newer. Negated when the caller asked for the first one.
    pub recency_key: Option<f64>,
    pub show_id: Option<Uuid>,
    pub season_id: Option<Uuid>,
    pub author_ids: Vec<Uuid>,
    pub series_id: Option<Uuid>,
    pub season_sort: Option<i32>,
    pub episode_sort: Option<i32>,
}

impl Scored {
    pub fn local_id(&self) -> Option<Uuid> {
        self.candidate.media_ref.local_id()
    }
}

#[derive(Clone, Copy)]
pub struct ScoreCtx<'a> {
    pub query: &'a str,
    pub query_tokens: &'a [String],
    pub year: Option<i32>,
    /// Only the hint parsed out of the text scores; an explicit `media_type`
    /// argument is a filter, not a nudge.
    pub soft_hint: Option<MediaTypeHint>,
    pub author: Option<&'a str>,
    pub relative: Option<Relative>,
    pub format: Option<WantFormat>,
    /// A season, episode or relative phrase is in play, so the subject is a
    /// container and the media-type hint describes the answer rather than the
    /// thing being searched for.
    pub descending: bool,
}

impl ScoreCtx<'_> {
    fn base(&self, kind: MediaKind, variants: &[String], in_library: bool) -> RankSignals {
        let title = ranking::title_signals(self.query, variants);
        RankSignals {
            exact_title: title.exact,
            prefix: title.prefix,
            token_overlap: title.overlap,
            partial_name: title.partial_name,
            fuzzy: title.fuzzy,
            in_library,
            media_type_hint: self.media_type_hint(kind),
            recency_requested: self.relative.is_some(),
            ..RankSignals::default()
        }
    }

    /// While descending, "episode" says what the answer should be, not what to
    /// look for — penalising the show would lose the only usable subject.
    fn media_type_hint(&self, kind: MediaKind) -> Option<bool> {
        if self.descending {
            return None;
        }
        self.soft_hint.map(|h| h.accepts(kind))
    }

    fn year_match(&self, doc_year: Option<i32>) -> Option<bool> {
        match (self.year, doc_year) {
            (Some(q), Some(d)) => Some(q == d),
            _ => None,
        }
    }

    /// An explicit author argument is a claim to be checked; an author name
    /// that merely appears in the query is a bonus when it hits and silence
    /// when it does not.
    fn author_match(&self, authors: &[String]) -> Option<bool> {
        if let Some(wanted) = self.author {
            let wanted = text::normalize(wanted);
            return Some(
                authors
                    .iter()
                    .any(|a| text::dice(&text::tokens(&wanted), &text::tokens(a)) > 0.6),
            );
        }
        if authors.is_empty() || self.query_tokens.is_empty() {
            return None;
        }
        let hit = authors.iter().any(|a| {
            let a_tokens = text::tokens(a);
            self.query_tokens
                .iter()
                .any(|q| q.len() > 3 && a_tokens.iter().any(|t| text::token_equiv(q, t)))
        });
        if hit { Some(true) } else { None }
    }

    fn recency(&self, key: Option<f64>) -> Option<f64> {
        match self.relative {
            Some(Relative::First) => key.map(|k| -k),
            _ => key,
        }
    }
}

fn parse_uuid(s: &str) -> Option<Uuid> {
    Uuid::parse_str(s).ok()
}

fn tokens_of(text_value: Option<&str>) -> Vec<String> {
    text_value.map(text::tokens).unwrap_or_default()
}

pub fn from_book_doc(ctx: &ScoreCtx<'_>, doc: &BookDocument) -> Option<Scored> {
    let id = parse_uuid(&doc.id)?;
    let variants = vec![doc.title.clone()];
    let mut signals = ctx.base(MediaKind::Book, &variants, doc.is_in_library);
    signals.author_match = ctx.author_match(&doc.authors);
    signals.format_available = format_available(ctx.format, doc.has_ebook, doc.has_audiobook);
    Some(Scored {
        candidate: Candidate {
            media_ref: MediaRef::book(id),
            title: doc.title.clone(),
            subtitle: subtitle_for_book(&doc.authors, doc.series_name.as_deref()),
            year: None,
            description: doc.description.clone(),
            in_library: true,
            show_id: None,
            is_sports: false,
            score: 0.0,
            source: CandidateSource::Library,
            provider_payload: None,
        },
        signals,
        rating: None,
        recency_key: None,
        show_id: None,
        season_id: None,
        author_ids: doc
            .author_ids
            .iter()
            .filter_map(|a| parse_uuid(a))
            .collect(),
        series_id: doc.series_id.as_deref().and_then(parse_uuid),
        season_sort: None,
        episode_sort: None,
    })
}

pub fn from_movie_doc(ctx: &ScoreCtx<'_>, doc: &MovieDocument) -> Option<Scored> {
    let id = parse_uuid(&doc.id)?;
    let variants = vec![doc.title.clone()];
    let mut signals = ctx.base(MediaKind::Movie, &variants, doc.is_in_library);
    signals.year_match = ctx.year_match(doc.year);
    Some(Scored {
        candidate: Candidate {
            media_ref: MediaRef::movie(id),
            title: doc.title.clone(),
            subtitle: doc.year.map(|y| y.to_string()),
            year: doc.year,
            description: doc.description.clone(),
            in_library: true,
            show_id: None,
            is_sports: false,
            score: 0.0,
            source: CandidateSource::Library,
            provider_payload: None,
        },
        signals,
        rating: doc.rating,
        recency_key: ctx.recency(doc.year.map(f64::from)),
        show_id: None,
        season_id: None,
        author_ids: Vec::new(),
        series_id: None,
        season_sort: None,
        episode_sort: None,
    })
}

pub fn from_show_doc(ctx: &ScoreCtx<'_>, doc: &TvShowDocument) -> Option<Scored> {
    let id = parse_uuid(&doc.id)?;
    let variants = vec![doc.title.clone()];
    let mut signals = ctx.base(MediaKind::Show, &variants, true);
    signals.year_match = ctx.year_match(doc.year);
    signals.availability = if doc.total_episodes > 0 {
        f64::from(doc.available_episode_count) / f64::from(doc.total_episodes)
    } else {
        0.0
    };
    Some(Scored {
        candidate: Candidate {
            media_ref: MediaRef::show(id),
            show_id: Some(id),
            title: doc.title.clone(),
            subtitle: show_subtitle(doc),
            year: doc.year,
            description: doc.description.clone(),
            in_library: true,
            is_sports: false,
            score: 0.0,
            source: CandidateSource::Library,
            provider_payload: None,
        },
        signals,
        rating: doc.rating,
        recency_key: ctx.recency(doc.year.map(f64::from)),
        show_id: Some(id),
        season_id: None,
        author_ids: Vec::new(),
        series_id: None,
        season_sort: None,
        episode_sort: None,
    })
}

pub fn from_season_doc(ctx: &ScoreCtx<'_>, doc: &TvSeasonDocument) -> Option<Scored> {
    let id = parse_uuid(&doc.id)?;
    let mut variants = vec![doc.display_title.clone()];
    if let Some(title) = &doc.season_title {
        variants.push(title.clone());
    }
    let signals = ctx.base(MediaKind::Season, &variants, true);
    // Which season this *is*, so a request that named a number can tell it from
    // its siblings — they are otherwise identical to the ranker.
    let season_sort = doc.season_identifier.parse().ok();
    Some(Scored {
        candidate: Candidate {
            media_ref: MediaRef::season(id),
            show_id: parse_uuid(&doc.show_id),
            title: doc.display_title.clone(),
            subtitle: Some(format!("{} episodes", doc.episode_count)),
            year: None,
            description: None,
            in_library: true,
            is_sports: false,
            score: 0.0,
            source: CandidateSource::Library,
            provider_payload: None,
        },
        signals,
        rating: None,
        recency_key: None,
        show_id: parse_uuid(&doc.show_id),
        season_id: Some(id),
        author_ids: Vec::new(),
        series_id: None,
        season_sort,
        episode_sort: None,
    })
}

pub fn from_episode_doc(ctx: &ScoreCtx<'_>, doc: &TvEpisodeDocument) -> Option<Scored> {
    let id = parse_uuid(&doc.id)?;
    let mut variants = vec![doc.display_title.clone()];
    if let Some(title) = &doc.episode_title {
        variants.push(title.clone());
    }
    let signals = ctx.base(MediaKind::Episode, &variants, doc.is_in_library);
    let recency = doc.air_date_ts.map(|ts| ts as f64).or_else(|| {
        Some(f64::from(doc.season_sort_order) * 10_000.0 + f64::from(doc.episode_sort_order))
    });
    Some(Scored {
        candidate: Candidate {
            media_ref: MediaRef::episode(id),
            show_id: parse_uuid(&doc.show_id),
            title: doc.display_title.clone(),
            subtitle: episode_subtitle(doc),
            year: None,
            description: doc.description.clone(),
            in_library: true,
            is_sports: false,
            score: 0.0,
            source: CandidateSource::Library,
            provider_payload: None,
        },
        signals,
        rating: None,
        recency_key: ctx.recency(recency),
        show_id: parse_uuid(&doc.show_id),
        season_id: parse_uuid(&doc.season_id),
        author_ids: Vec::new(),
        series_id: None,
        season_sort: Some(doc.season_sort_order),
        episode_sort: Some(doc.episode_sort_order),
    })
}

pub fn from_author_doc(ctx: &ScoreCtx<'_>, doc: &AuthorDocument) -> Option<Scored> {
    let id = parse_uuid(&doc.id)?;
    let variants = vec![doc.name.clone()];
    let signals = ctx.base(MediaKind::Author, &variants, true);
    Some(Scored {
        candidate: Candidate {
            media_ref: MediaRef::author(id),
            title: doc.name.clone(),
            subtitle: Some(format!("{} books", doc.book_count)),
            year: None,
            description: None,
            in_library: true,
            show_id: None,
            is_sports: false,
            score: 0.0,
            source: CandidateSource::Library,
            provider_payload: None,
        },
        signals,
        rating: None,
        recency_key: None,
        show_id: None,
        season_id: None,
        author_ids: vec![id],
        series_id: None,
        season_sort: None,
        episode_sort: None,
    })
}

pub fn from_series_doc(ctx: &ScoreCtx<'_>, doc: &SeriesDocument) -> Option<Scored> {
    let id = parse_uuid(&doc.id)?;
    let variants = vec![doc.name.clone()];
    let signals = ctx.base(MediaKind::Series, &variants, true);
    Some(Scored {
        candidate: Candidate {
            media_ref: MediaRef::series(id),
            title: doc.name.clone(),
            subtitle: Some(format!("{} books", doc.book_count)),
            year: None,
            description: None,
            in_library: true,
            show_id: None,
            is_sports: false,
            score: 0.0,
            source: CandidateSource::Library,
            provider_payload: None,
        },
        signals,
        rating: None,
        recency_key: None,
        show_id: None,
        season_id: None,
        author_ids: Vec::new(),
        series_id: Some(id),
        season_sort: None,
        episode_sort: None,
    })
}

pub fn from_book_brief(ctx: &ScoreCtx<'_>, brief: &super::store::BookBrief) -> Scored {
    let variants = vec![brief.title.clone()];
    let mut signals = ctx.base(MediaKind::Book, &variants, brief.is_in_library);
    signals.author_match = ctx.author_match(&brief.authors);
    signals.format_available = format_available(ctx.format, brief.has_ebook, brief.has_audiobook);
    Scored {
        candidate: Candidate {
            media_ref: MediaRef::book(brief.id),
            title: brief.title.clone(),
            subtitle: subtitle_for_book(&brief.authors, None),
            year: brief.year,
            description: brief.description.clone(),
            in_library: true,
            show_id: None,
            is_sports: false,
            score: 0.0,
            source: CandidateSource::Library,
            provider_payload: None,
        },
        signals,
        rating: None,
        recency_key: ctx.recency(brief.year.map(f64::from)),
        show_id: None,
        season_id: None,
        author_ids: brief.author_ids.clone(),
        series_id: None,
        season_sort: None,
        episode_sort: None,
    }
}

pub fn from_provider_hit(ctx: &ScoreCtx<'_>, hit: &ProviderHit, kind: MediaKind) -> Scored {
    let variants = vec![hit.title.clone()];
    let mut signals = ctx.base(kind, &variants, false);
    signals.year_match = ctx.year_match(hit.year);
    signals.author_match = ctx.author_match(&hit.authors);
    let media_ref = MediaRef::provider(hit.provider.clone(), hit.id.clone(), kind);
    Scored {
        candidate: Candidate {
            media_ref,
            title: hit.title.clone(),
            subtitle: hit
                .year
                .map(|y| y.to_string())
                .or_else(|| hit.authors.first().cloned()),
            year: hit.year,
            description: hit.description.clone(),
            in_library: false,
            show_id: None,
            is_sports: false,
            score: 0.0,
            source: CandidateSource::Provider(hit.provider.clone()),
            provider_payload: Some(hit.payload.clone()),
        },
        signals,
        rating: hit.rating,
        recency_key: ctx.recency(hit.year.map(f64::from)),
        show_id: None,
        season_id: None,
        author_ids: Vec::new(),
        series_id: None,
        season_sort: None,
        episode_sort: None,
    }
}

fn format_available(
    format: Option<WantFormat>,
    has_ebook: bool,
    has_audiobook: bool,
) -> Option<bool> {
    match format {
        Some(WantFormat::Audiobook) => Some(has_audiobook),
        Some(WantFormat::Ebook) => Some(has_ebook),
        Some(WantFormat::Both) => Some(has_ebook && has_audiobook),
        None => None,
    }
}

fn subtitle_for_book(authors: &[String], series: Option<&str>) -> Option<String> {
    let author = authors.first().cloned();
    match (author, series) {
        (Some(a), Some(s)) => Some(format!("{a} · {s}")),
        (Some(a), None) => Some(a),
        (None, Some(s)) => Some(s.to_string()),
        (None, None) => None,
    }
}

fn show_subtitle(doc: &TvShowDocument) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(year) = doc.year {
        parts.push(year.to_string());
    }
    if let Some(network) = &doc.network {
        parts.push(network.clone());
    }
    if doc.available_episode_count > 0 {
        parts.push(format!(
            "{} of {} episodes",
            doc.available_episode_count, doc.total_episodes
        ));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

fn episode_subtitle(doc: &TvEpisodeDocument) -> Option<String> {
    let mut parts = vec![doc.episode_code.clone()];
    if let Some(air_date) = &doc.air_date {
        parts.push(air_date.clone());
    }
    Some(parts.join(" · "))
}

/// The synopsis pass: Meili cannot search descriptions, so "the one with the
/// tents" is answered by re-reading a bounded pool in Rust.
pub fn description_coverage(query_tokens: &[String], description: Option<&str>) -> f64 {
    if query_tokens.is_empty() {
        return 0.0;
    }
    text::coverage(query_tokens, &tokens_of(description))
}

/// Whether the query actually named this candidate, as opposed to reaching it
/// through an author or a show name.
pub fn names_the_subject(signals: &RankSignals) -> bool {
    signals.exact_title > 0.0
        || signals.partial_name
        || signals.token_overlap >= ranking::MIN_TITLE_OVERLAP
        || signals.description_hit > 0.0
}

/// Fold pool-relative signals in and write the final score.
///
/// Popularity and recency are normalized **within each media kind**. Across
/// kinds they are not comparable — a show's first-aired year and an episode's
/// air date are different scales — and mixing them let an episode of the named
/// show come within a whisker of the show itself on "last night's episode of X".
pub fn finalize_scores(pool: &mut [Scored]) {
    let mut kinds: Vec<MediaKind> = pool.iter().map(|s| s.candidate.kind()).collect();
    kinds.sort_by_key(|k| k.as_str());
    kinds.dedup();

    for kind in kinds {
        let group: Vec<usize> = pool
            .iter()
            .enumerate()
            .filter(|(_, s)| s.candidate.kind() == kind)
            .map(|(i, _)| i)
            .collect();
        let ratings: Vec<Option<f64>> = group.iter().map(|i| pool[*i].rating).collect();
        let recencies: Vec<Option<f64>> = group.iter().map(|i| pool[*i].recency_key).collect();
        let popularity = ranking::normalize_ranks(&ratings);
        let recency = ranking::normalize_ranks(&recencies);
        for (slot, index) in group.into_iter().enumerate() {
            pool[index].signals.popularity = popularity[slot];
            pool[index].signals.recency = recency[slot];
        }
    }

    for scored in pool.iter_mut() {
        scored.candidate.score = ranking::rank_score(&scored.signals);
    }
    ranking::sort_by_score(pool, |s| s.candidate.score);
}
