//! `resolve_media` — one entry point across books, audiobooks, movies, TV and
//! sports.
//!
//! Three steps, identical for every media type:
//!
//! 1. **Library first**, over Meili's multi-search, because she means her copy.
//! 2. **Metadata providers second**, dispatched to whichever of the two stacks
//!    the media type implies and collapsed with the existing `merge` modules.
//! 3. **Ambiguity is a question, never a guess.** Downloading the wrong show is
//!    the failure that ends the trial.
//!
//! Media type is an argument, not a fork in the tool list. Nothing below tells
//! the caller that books and video are different subsystems.

pub mod build;
pub mod descend;
pub mod fixture;
pub mod format;
pub mod index;
pub mod phrasing;
pub mod providers;
pub mod ranking;
pub mod refs;
pub mod store;
pub mod text;
pub mod types;

use std::collections::HashSet;

use time::OffsetDateTime;
use uuid::Uuid;

use build::{ScoreCtx, Scored};
use index::{IndexKind, IndexQuery, LibraryIndex};
use phrasing::Relative;
use providers::MetadataLookup;
use ranking::{AMBIGUITY_MARGIN, MAX_CANDIDATES, MIN_PLAUSIBLE};
use store::ResolveStore;

pub use refs::{MediaKind, MediaRef, MediaRefParseError, ProviderRef, ProviderScope};
pub use types::{
    Candidate, CandidateSource, Candidates, FormatSkew, MediaTypeHint, ProviderPayload, Resolution,
    ResolveError, ResolveRequest, WantFormat,
};

/// Hits per index in the first pass. Wide enough to see the rivals, narrow
/// enough that ranking stays cheap.
const POOL_LIMIT: usize = 10;
/// The synopsis pass reads a bounded slice of the library rather than the whole
/// of it — descriptions are not searchable in Meili, only stored.
const DESCRIPTION_POOL_LIMIT: usize = 60;
/// A description has to genuinely answer the query, not share a word with it.
const MIN_DESCRIPTION_COVERAGE: f64 = 0.75;
/// Candidates of one kind offered before the next kind gets a turn. Two is a
/// choice; three is a list of films with a show hidden under it.
const PER_KIND: usize = 2;

pub struct ResolveContext<'a> {
    pub index: &'a dyn LibraryIndex,
    pub providers: &'a dyn MetadataLookup,
    pub store: &'a dyn ResolveStore,
    /// Watch state and reading history are this person's, never the household's.
    pub user_id: Uuid,
    /// Injected so "last night's episode" is testable.
    pub now: OffsetDateTime,
}

/// Resolve a vague phrase to candidates carrying stable refs.
pub async fn resolve_media(
    ctx: &ResolveContext<'_>,
    req: &ResolveRequest,
) -> Result<Candidates, ResolveError> {
    let parsed = phrasing::parse(&req.query);
    let season = req.season.or(parsed.season);
    let episode = req.episode.or(parsed.episode);
    let explicit_hint = req.media_type;
    let effective_hint = explicit_hint.or(parsed.media_type);
    let relative = parsed.relative;
    let exclude_watched = req.exclude_watched || parsed.exclude_watched;
    let explicit_format = req.format.or(parsed.format);
    let descending = season.is_some()
        || episode.is_some()
        || relative.is_some()
        || parsed.all_seasons
        || matches!(
            explicit_hint,
            Some(MediaTypeHint::Episode | MediaTypeHint::Season)
        );

    let title_tokens = parsed.title_tokens.clone();
    let title = title_tokens.join(" ");
    let mut sctx = ScoreCtx {
        query: &title,
        query_tokens: &title_tokens,
        year: parsed.year,
        soft_hint: parsed.media_type,
        author: req.author.as_deref(),
        relative,
        format: explicit_format,
        descending,
    };

    let plan = Plan {
        season,
        episode,
        relative,
        explicit_hint,
        effective_hint,
        exclude_watched,
        explicit_format,
        descending,
        all_seasons: parsed.all_seasons,
    };

    // Nothing was named, so the referent has to come from the conversation.
    if title_tokens.is_empty() {
        let Some(context) = req.context.clone() else {
            return Ok(Candidates::needs_context(context_question(relative)));
        };
        let mut out = descend_from_ref(ctx, &sctx, &plan, &context).await?;
        finish(ctx, &plan, &mut out).await?;
        return Ok(out);
    }

    let mut pool = library_pool(ctx, &sctx, &plan).await?;
    build::finalize_scores(&mut pool);
    retain_plausible(&mut pool);

    if pool.is_empty() {
        pool = descriptive_pool(ctx, &sctx, &plan).await?;
        build::finalize_scores(&mut pool);
        retain_plausible(&mut pool);
    }

    // The providers are asked for the words as spoken, not the token soup the
    // ranker scores with.
    //
    // `title_tokens` has the intent words taken out, which is right for scoring
    // against the library — "that baking show" is about baking. It is wrong to
    // send to a search API. The list contains "last", "new" and "first" because
    // they carry intent in "last night's episode", so a title that owns one of
    // those words arrives at TMDB gutted: "Last Seen (2026)" is searched for as
    // `seen`, and TMDB answers with *Seen on Trip* and *Seen With the Heart*.
    // The year is already split out by `phrasing`, which matters just as much —
    // TMDB returns nothing at all for a query with "(2026)" in it and wants the
    // year in its own field.
    //
    // Spoken first, stripped second, because neither survives alone: TMDB finds
    // nothing for "download dune", and finds the wrong film for "out". Every
    // rung of this ladder is a query that failed, so one more is in keeping.
    if pool.is_empty() {
        pool = provider_pool(ctx, &sctx, &plan, &parsed.title).await;
        build::finalize_scores(&mut pool);
        retain_plausible(&mut pool);
    }

    if pool.is_empty() && parsed.title != title {
        pool = provider_pool(ctx, &sctx, &plan, &title).await;
        build::finalize_scores(&mut pool);
        retain_plausible(&mut pool);
    }

    // Last: the words that name the thing come first and the reason for asking
    // comes after it — "the latest episode of X, it has audio descriptions I
    // don't want". Meili matches on every term, so that tail turns a resolvable
    // phrase into nothing at all. Retry on the longest leading run that finds
    // something, and score against that run rather than the whole sentence.
    //
    // After the providers, not before them: "dune messiah" narrows to a "dune"
    // that is already on the shelf, and a sequel nobody owns must still reach
    // the provider that has it.
    let narrowed;
    if pool.is_empty()
        && let Some(hit) = narrowed_pool(ctx, &sctx, &plan, &title_tokens).await?
    {
        narrowed = hit;
        pool = narrowed.pool;
        sctx.query = &narrowed.query;
        sctx.query_tokens = &narrowed.tokens;
    }

    // The providers only ever got the phrase as spoken. A show they call
    // "Ludwig" is "Ludwig with David Mitchell" out loud, so the same trailing
    // drop is run against them once the library has said everything it will.
    let narrowed_provider;
    if pool.is_empty()
        && let Some(hit) = narrowed_provider_pool(ctx, &sctx, &plan, &title_tokens).await
    {
        narrowed_provider = hit;
        pool = narrowed_provider.pool;
        sctx.query = &narrowed_provider.query;
        sctx.query_tokens = &narrowed_provider.tokens;
    }

    // A media type the caller passed is a preference, not a wall.
    //
    // `media_type` is an argument the model fills in, and "download Last Seen
    // (2026)" says nothing about which shelf the thing is on. It guessed
    // `movie`, and the TV show of that name could not be reached at all — two
    // unrelated films were offered instead, and the right answer was not on the
    // list to be picked. So when the hinted shelf comes back with nothing, or
    // with nothing it can choose between, the other shelves are searched too.
    //
    // The guess stops voting once it has failed: `soft_hint` carries only what
    // she actually said ("the film", "that book"), so a type she named still
    // sorts its own kind first and a type the model invented no longer does.
    //
    // Only when nothing behind the wall actually answers to the name. Two films
    // called Dune are a real question about films, and a book called Dune does
    // not make it a better one — "the film" was the answer to that. It is the
    // pool with no exact match in it at all that says the wall is in the way.
    //
    // And never while descending: "season 3 of X", "last night's episode" and
    // the like are on the TV rails by construction, and widening there would go
    // looking for a book.
    if plan.effective_hint.is_some()
        && !plan.descending
        && nothing_named_it(&pool)
        && (pool.is_empty() || ambiguous_count(&pool) > 1)
    {
        let mut wider = widened_pool(ctx, &sctx, &plan, &parsed.title, &title).await?;
        let known: std::collections::HashSet<MediaRef> =
            pool.iter().map(|s| s.candidate.media_ref.clone()).collect();
        wider.retain(|s| !known.contains(&s.candidate.media_ref));
        if !wider.is_empty() {
            pool.append(&mut wider);
            build::finalize_scores(&mut pool);
            retain_plausible(&mut pool);
        }
    }

    if pool.is_empty() {
        return Ok(Candidates::not_found(format!(
            "nothing in the library or at any provider matches \"{}\"",
            req.query.trim()
        )));
    }

    if exclude_watched {
        subtract_watched_movies(ctx, &mut pool).await?;
        if pool.is_empty() {
            return Ok(Candidates::not_found(
                "everything matching is already watched",
            ));
        }
    }

    // A person in the request is a disambiguator the title search cannot use:
    // "Ludwig with David Mitchell" answers to three shows, and only one of
    // them has him in it. Worth a credits call only when the names are tied.
    let mut person_note: Option<String> = None;
    if let Some(person) = parsed.person.as_deref()
        && ambiguous_count(&pool) > 1
        && let Some(winner) = person_match(ctx, &pool, person).await
    {
        let winning_ref = winner.candidate.media_ref.clone();
        pool.retain(|s| s.candidate.media_ref == winning_ref);
        person_note = Some(format!("matched because {person} is in it"));
    }

    let rivals = ambiguous_count(&pool);
    if rivals > 1 {
        let mut candidates = spread(pool, rivals);
        let question = which_question(&candidates);
        let mut out = Candidates {
            resolution: Resolution::Ambiguous,
            question: Some(question),
            candidates: std::mem::take(&mut candidates),
            format: None,
            format_question: None,
            note: None,
            pick_qualifier: pick_qualifier(&plan),
            seasons: None,
        };
        finish(ctx, &plan, &mut out).await?;
        return Ok(out);
    }

    let subject = pool.remove(0);
    let mut out = descend_from_scored(ctx, &sctx, &plan, subject).await?;
    if let Some(note) = person_note {
        out.note.get_or_insert(note);
    }
    finish(ctx, &plan, &mut out).await?;
    Ok(out)
}

/// Everything the resolution ladder needs that is not the text itself.
#[derive(Clone, Copy)]
struct Plan {
    season: Option<i32>,
    episode: Option<i32>,
    relative: Option<Relative>,
    explicit_hint: Option<MediaTypeHint>,
    effective_hint: Option<MediaTypeHint>,
    exclude_watched: bool,
    explicit_format: Option<WantFormat>,
    descending: bool,
    /// "all seasons" — every season of a show, not one named one.
    all_seasons: bool,
}

// =============================================================================
// Pools
// =============================================================================

fn index_allowed(kind: IndexKind, plan: &Plan) -> bool {
    let Some(hint) = plan.explicit_hint else {
        return true;
    };
    match hint {
        MediaTypeHint::Book | MediaTypeHint::Audiobook => matches!(
            kind,
            IndexKind::Books | IndexKind::Authors | IndexKind::Series
        ),
        MediaTypeHint::Movie => kind == IndexKind::Movies,
        MediaTypeHint::Show => kind == IndexKind::TvShows,
        MediaTypeHint::Season => matches!(kind, IndexKind::TvShows | IndexKind::TvSeasons),
        MediaTypeHint::Episode => matches!(
            kind,
            IndexKind::TvShows | IndexKind::TvSeasons | IndexKind::TvEpisodes
        ),
        MediaTypeHint::Author => kind == IndexKind::Authors,
        MediaTypeHint::Series => matches!(kind, IndexKind::Series | IndexKind::Books),
        MediaTypeHint::Sports => matches!(kind, IndexKind::TvShows | IndexKind::TvEpisodes),
    }
}

/// An explicit `media_type` is a filter on the answer. It is relaxed for the
/// containers a descent starts from: "the new Sanderson" with `media_type=book`
/// still has to be allowed to find the author first.
fn allows_subject(plan: &Plan, kind: MediaKind) -> bool {
    match plan.explicit_hint {
        None => true,
        Some(hint) => {
            hint.accepts(kind)
                || (plan.descending
                    && matches!(
                        kind,
                        MediaKind::Show
                            | MediaKind::Season
                            | MediaKind::Author
                            | MediaKind::Series
                            | MediaKind::SportsEvent
                    ))
        }
    }
}

fn convert(sctx: &ScoreCtx<'_>, plan: &Plan, hits: index::IndexHits) -> Vec<Scored> {
    let mut out = Vec::new();
    for doc in &hits.books {
        out.extend(build::from_book_doc(sctx, doc));
    }
    for doc in &hits.movies {
        out.extend(build::from_movie_doc(sctx, doc));
    }
    for doc in &hits.shows {
        out.extend(build::from_show_doc(sctx, doc));
    }
    for doc in &hits.seasons {
        out.extend(build::from_season_doc(sctx, doc));
    }
    for doc in &hits.episodes {
        out.extend(build::from_episode_doc(sctx, doc));
    }
    for doc in &hits.authors {
        out.extend(build::from_author_doc(sctx, doc));
    }
    for doc in &hits.series {
        out.extend(build::from_series_doc(sctx, doc));
    }
    out.retain(|s| allows_subject(plan, s.candidate.media_ref.kind()));
    out.retain(|s| names_the_season(plan, s));
    out
}

/// A season number is the request, not a preference.
///
/// Seasons of one show are identical to the ranker — same title, same library
/// bonus — so they land within [`AMBIGUITY_MARGIN`] of each other and of the
/// show. Left in the pool, a request for season 25 becomes a question about
/// seasons 11 and 26, and picking one of those queues it. A document for a
/// season nobody named does not answer the question badly; it does not answer
/// it at all.
fn names_the_season(plan: &Plan, scored: &Scored) -> bool {
    let Some(wanted) = plan.season else {
        return true;
    };
    match scored.candidate.media_ref.kind() {
        MediaKind::Season | MediaKind::Episode | MediaKind::SportsEvent => {
            scored.season_sort == Some(wanted)
        }
        _ => true,
    }
}

async fn library_pool(
    ctx: &ResolveContext<'_>,
    sctx: &ScoreCtx<'_>,
    plan: &Plan,
) -> Result<Vec<Scored>, ResolveError> {
    let queries: Vec<IndexQuery> = [
        IndexKind::Books,
        IndexKind::Movies,
        IndexKind::TvShows,
        IndexKind::TvSeasons,
        IndexKind::TvEpisodes,
        IndexKind::Authors,
        IndexKind::Series,
    ]
    .into_iter()
    .filter(|k| index_allowed(*k, plan))
    .map(|k| IndexQuery::new(k, sctx.query, POOL_LIMIT))
    .collect();

    let hits = ctx.index.multi_search(&queries).await?;
    Ok(convert(sctx, plan, hits))
}

/// A pool found on a leading run of the query's tokens, with the run that
/// found it.
struct Narrowed {
    query: String,
    tokens: Vec<String>,
    pool: Vec<Scored>,
}

/// Drop trailing tokens one at a time until the library has something to say.
///
/// Only reached once every other pool has come back empty, so the cost is paid
/// on queries that would otherwise have answered "nothing matches that".
async fn narrowed_pool(
    ctx: &ResolveContext<'_>,
    sctx: &ScoreCtx<'_>,
    plan: &Plan,
    title_tokens: &[String],
) -> Result<Option<Narrowed>, ResolveError> {
    for len in (1..title_tokens.len()).rev() {
        let tokens = title_tokens[..len].to_vec();
        let query = tokens.join(" ");
        let trial = ScoreCtx {
            query: &query,
            query_tokens: &tokens,
            ..*sctx
        };
        let mut pool = library_pool(ctx, &trial, plan).await?;
        build::finalize_scores(&mut pool);
        retain_plausible(&mut pool);
        if !pool.is_empty() {
            return Ok(Some(Narrowed {
                query,
                tokens,
                pool,
            }));
        }
    }
    Ok(None)
}

/// Drop trailing tokens one at a time until a provider has something to say.
///
/// The providers only ever get the full phrase as spoken, and
/// [`narrowed_pool`] runs its ladder against the library. A show they call
/// "Ludwig" is "Ludwig with David Mitchell" out loud, so without this rung the
/// answer is "nothing matches that". Only reached once the library has come
/// back empty at every rung, so the cost is paid on queries that would
/// otherwise have answered "nothing matches that".
async fn narrowed_provider_pool(
    ctx: &ResolveContext<'_>,
    sctx: &ScoreCtx<'_>,
    plan: &Plan,
    title_tokens: &[String],
) -> Option<Narrowed> {
    for len in (1..title_tokens.len()).rev() {
        let tokens = title_tokens[..len].to_vec();
        let query = tokens.join(" ");
        let trial = ScoreCtx {
            query: &query,
            query_tokens: &tokens,
            ..*sctx
        };
        let mut pool = provider_pool(ctx, &trial, plan, &query).await;
        build::finalize_scores(&mut pool);
        retain_plausible(&mut pool);
        if !pool.is_empty() {
            return Some(Narrowed {
                query,
                tokens,
                pool,
            });
        }
    }
    None
}

async fn descriptive_pool(
    ctx: &ResolveContext<'_>,
    sctx: &ScoreCtx<'_>,
    plan: &Plan,
) -> Result<Vec<Scored>, ResolveError> {
    let queries: Vec<IndexQuery> = [IndexKind::Books, IndexKind::Movies, IndexKind::TvShows]
        .into_iter()
        .filter(|k| index_allowed(*k, plan))
        .map(|k| IndexQuery::new(k, "", DESCRIPTION_POOL_LIMIT))
        .collect();

    let hits = ctx.index.multi_search(&queries).await?;
    let mut pool = convert(sctx, plan, hits);
    for scored in &mut pool {
        scored.signals.description_hit =
            build::description_coverage(sctx.query_tokens, scored.candidate.description.as_deref());
    }
    pool.retain(|s| s.signals.description_hit >= MIN_DESCRIPTION_COVERAGE);
    Ok(pool)
}

/// Whether anything in the pool is actually called what she called it.
///
/// A partial match is how a wrong shelf looks from the inside: asked for a film
/// called "Last Seen", TMDB offers *Last Seen Alive* and *Last Seen, Long Time
/// Ago*, both plausible, neither named. An exact one means the shelf answered.
fn nothing_named_it(pool: &[Scored]) -> bool {
    !pool.iter().any(|s| s.signals.exact_title > 0.0)
}

/// The ladder again with the media-type wall down, for when the hinted shelf
/// could not answer. Library before providers, the same order and for the same
/// reason: a thing on the shelf beats a thing at TMDB.
async fn widened_pool(
    ctx: &ResolveContext<'_>,
    sctx: &ScoreCtx<'_>,
    plan: &Plan,
    spoken: &str,
    stripped: &str,
) -> Result<Vec<Scored>, ResolveError> {
    let plan = Plan {
        explicit_hint: None,
        effective_hint: None,
        ..*plan
    };

    let mut pool = library_pool(ctx, sctx, &plan).await?;
    build::finalize_scores(&mut pool);
    retain_plausible(&mut pool);

    if pool.is_empty() {
        pool = provider_pool(ctx, sctx, &plan, spoken).await;
        build::finalize_scores(&mut pool);
        retain_plausible(&mut pool);
    }

    if pool.is_empty() && spoken != stripped {
        pool = provider_pool(ctx, sctx, &plan, stripped).await;
        build::finalize_scores(&mut pool);
        retain_plausible(&mut pool);
    }

    Ok(pool)
}

/// Ask the providers for `title`. The ranker still scores what comes back
/// against `sctx`, so the words asked for and the words scored with are allowed
/// to differ — see the call site for why they must.
async fn provider_pool(
    ctx: &ResolveContext<'_>,
    sctx: &ScoreCtx<'_>,
    plan: &Plan,
    title: &str,
) -> Vec<Scored> {
    if title.is_empty() {
        return Vec::new();
    }
    let (want_books, want_movies, want_shows) = match plan.effective_hint {
        Some(MediaTypeHint::Book | MediaTypeHint::Audiobook | MediaTypeHint::Author) => {
            (true, false, false)
        }
        Some(MediaTypeHint::Series) => (true, false, false),
        Some(MediaTypeHint::Movie) => (false, true, false),
        Some(
            MediaTypeHint::Show
            | MediaTypeHint::Season
            | MediaTypeHint::Episode
            | MediaTypeHint::Sports,
        ) => (false, false, true),
        None => {
            let video_only = plan.season.is_some() || plan.episode.is_some();
            (!video_only, !video_only, true)
        }
    };

    let mut pool = Vec::new();
    if want_books {
        for hit in ctx.providers.books(title, sctx.author).await {
            pool.push(build::from_provider_hit(sctx, &hit, MediaKind::Book));
        }
    }
    if want_movies {
        for hit in ctx.providers.movies(title, sctx.year).await {
            pool.push(build::from_provider_hit(sctx, &hit, MediaKind::Movie));
        }
    }
    if want_shows {
        for hit in ctx.providers.shows(title, sctx.year).await {
            pool.push(build::from_provider_hit(sctx, &hit, MediaKind::Show));
        }
    }
    pool.retain(|s| allows_subject(plan, s.candidate.media_ref.kind()));
    pool
}

fn retain_plausible(pool: &mut Vec<Scored>) {
    pool.retain(|s| s.candidate.score >= MIN_PLAUSIBLE && build::names_the_subject(&s.signals));
}

/// The tied candidates, a couple of each kind.
///
/// With the media-type wall down a close-run question can be a film, a show and
/// a book at once, and taking the top five by score alone can mean five films
/// and no hint that the show exists — which is how "Last Seen" went missing
/// behind two films nobody asked for. Two of a kind is enough to say "it could
/// be either" without turning an answer into a catalogue, and whatever room is
/// left over is filled by score, so a genuine five-way tie between films still
/// reads as one.
///
/// Order is never touched: the list comes out best-first either way.
fn spread(pool: Vec<Scored>, rivals: usize) -> Vec<Candidate> {
    let head: Vec<Candidate> = pool.into_iter().take(rivals).map(|s| s.candidate).collect();
    let room = rivals.min(MAX_CANDIDATES);

    let mut chosen: Vec<usize> = Vec::new();
    let mut per_kind: std::collections::HashMap<MediaKind, usize> =
        std::collections::HashMap::new();
    for (i, candidate) in head.iter().enumerate() {
        if chosen.len() == room {
            break;
        }
        let taken = per_kind.entry(candidate.kind()).or_default();
        if *taken < PER_KIND {
            *taken += 1;
            chosen.push(i);
        }
    }
    // One kind crowded out the rest and there is room going spare.
    for i in 0..head.len() {
        if chosen.len() == room {
            break;
        }
        if !chosen.contains(&i) {
            chosen.push(i);
        }
    }
    chosen.sort_unstable();

    head.into_iter()
        .enumerate()
        .filter(|(i, _)| chosen.contains(i))
        .map(|(_, candidate)| candidate)
        .collect()
}

/// How many candidates are close enough to the top that picking one would be a
/// guess./// How many candidates are close enough to the top that picking one would be a
/// guess.
fn ambiguous_count(pool: &[Scored]) -> usize {
    let Some(top) = pool.first() else { return 0 };
    pool.iter()
        .take_while(|s| top.candidate.score - s.candidate.score < AMBIGUITY_MARGIN)
        .count()
}

/// The one candidate whose cast or crew actually contains the person named.
///
/// A tie the title cannot break is broken by the person the caller named —
/// but only a clean win: nobody matching, or two, leaves the question standing.
async fn person_match(ctx: &ResolveContext<'_>, pool: &[Scored], person: &str) -> Option<Scored> {
    let mut matched: Vec<Scored> = Vec::new();
    let mut checked = 0;
    for scored in pool.iter() {
        let Some(p) = scored.candidate.media_ref.provider_ref() else {
            continue;
        };
        if scored.candidate.media_ref.kind() != MediaKind::Show {
            continue;
        }
        checked += 1;
        if checked > 4 {
            break;
        }
        let names = ctx.providers.tv_credits(&p.provider, &p.id).await;
        if names.iter().any(|n| person_in_name(person, n)) {
            matched.push(scored.clone());
        }
    }
    match matched.len() {
        1 => matched.pop(),
        _ => None,
    }
}

/// "David Mitchell" is in "David R. Mitchell": every token of the person, in
/// order, in the name. One token is never enough to identify anyone.
fn person_in_name(person: &str, name: &str) -> bool {
    let person = text::tokens(person);
    let name = text::tokens(name);
    if person.len() < 2 {
        return false;
    }
    let mut start = 0;
    for w in &person {
        match name[start..].iter().position(|h| h == w) {
            Some(i) => start += i + 1,
            None => return false,
        }
    }
    true
}

// =============================================================================
// Descent
// =============================================================================

async fn descend_from_ref(
    ctx: &ResolveContext<'_>,
    sctx: &ScoreCtx<'_>,
    plan: &Plan,
    context: &MediaRef,
) -> Result<Candidates, ResolveError> {
    let Some(id) = context.local_id() else {
        return Ok(Candidates::needs_context(context_question(plan.relative)));
    };
    match context.kind() {
        MediaKind::Show => descend_show(ctx, sctx, plan, id).await,
        MediaKind::Season => {
            let hits =
                ctx.index
                    .multi_search(&[IndexQuery::new(IndexKind::TvSeasons, "", 1)
                        .filtered(format!("id = \"{id}\""))])
                    .await?;
            let Some(season) = hits.seasons.first() else {
                return Ok(Candidates::not_found(
                    "that season is no longer in the library",
                ));
            };
            let Ok(show_id) = Uuid::parse_str(&season.show_id) else {
                return Ok(Candidates::not_found("that season has no show"));
            };
            descend_show(ctx, sctx, plan, show_id).await
        }
        MediaKind::Episode | MediaKind::SportsEvent => {
            let Some(episode) = descend::episode_by_id(ctx.index, id).await? else {
                return Ok(Candidates::not_found(
                    "that episode is no longer in the library",
                ));
            };
            descend_episode_context(ctx, sctx, plan, &episode).await
        }
        _ => Ok(Candidates::needs_context(context_question(plan.relative))),
    }
}

async fn descend_from_scored(
    ctx: &ResolveContext<'_>,
    sctx: &ScoreCtx<'_>,
    plan: &Plan,
    subject: Scored,
) -> Result<Candidates, ResolveError> {
    if let Some(provider) = subject.candidate.media_ref.clone().provider_ref() {
        return Ok(provider_single(ctx, subject, provider.clone(), plan).await);
    }

    match subject.candidate.media_ref.kind() {
        MediaKind::Show => {
            let Some(show_id) = subject.local_id() else {
                return Ok(single(subject));
            };
            if plan.season.is_none() && plan.episode.is_none() && plan.relative.is_none() {
                return Ok(single(subject));
            }
            descend_show(ctx, sctx, plan, show_id).await
        }
        MediaKind::Season => {
            // `names_the_season` should have taken a season that is not the one
            // named out of the pool. If one reaches here anyway, the only useful
            // thing about it is its show: this is the site that answered "season
            // 25" with a season 11 nobody asked for.
            if let Some(wanted) = plan.season
                && subject.season_sort != Some(wanted)
            {
                return match subject.show_id {
                    Some(show_id) => descend_show(ctx, sctx, plan, show_id).await,
                    None => Ok(Candidates::not_found(format!(
                        "there is no season {wanted} in the library"
                    ))),
                };
            }
            let (Some(season_id), Some(episode)) = (subject.season_id, plan.episode) else {
                return Ok(single(subject));
            };
            let episodes = descend::episodes_of_season(ctx.index, season_id).await?;
            let found = match plan.season {
                Some(season) => {
                    let wanted = descend::episode_code(season, episode);
                    episodes.iter().find(|e| e.episode_code == wanted)
                }
                None => episodes.iter().find(|e| e.episode_sort_order == episode),
            };
            match found {
                Some(doc) => Ok(single_doc(sctx, doc)),
                None => Ok(Candidates::not_found(format!(
                    "there is no episode {episode} in that season"
                ))),
            }
        }
        MediaKind::Episode | MediaKind::SportsEvent => {
            if plan.relative.is_none() {
                return Ok(single(subject));
            }
            let Some(id) = subject.local_id() else {
                return Ok(single(subject));
            };
            let Some(doc) = descend::episode_by_id(ctx.index, id).await? else {
                return Ok(single(subject));
            };
            descend_episode_context(ctx, sctx, plan, &doc).await
        }
        MediaKind::Author => {
            if !plan.descending && plan.effective_hint != Some(MediaTypeHint::Book) {
                return Ok(single(subject));
            }
            let author_id = subject.author_ids.first().copied();
            descend_books(ctx, sctx, plan, author_id, None, subject).await
        }
        MediaKind::Series => {
            if !plan.descending && plan.effective_hint != Some(MediaTypeHint::Book) {
                return Ok(single(subject));
            }
            let series_id = subject.series_id;
            descend_books(ctx, sctx, plan, None, series_id, subject).await
        }
        _ => Ok(single(subject)),
    }
}

async fn descend_show(
    ctx: &ResolveContext<'_>,
    sctx: &ScoreCtx<'_>,
    plan: &Plan,
    show_id: Uuid,
) -> Result<Candidates, ResolveError> {
    if let (Some(season), Some(episode)) = (plan.season, plan.episode) {
        return match descend::find_episode(ctx.index, show_id, season, episode).await? {
            Some(doc) => Ok(single_doc(sctx, &doc)),
            None => Ok(Candidates::not_found(format!(
                "there is no {} in the library",
                descend::episode_code(season, episode)
            ))),
        };
    }
    if let Some(season) = plan.season {
        return match descend::find_season(ctx.index, show_id, season).await? {
            Some(doc) => Ok(single_season(sctx, &doc)),
            None => season_from_provider(ctx, sctx, plan, show_id, season).await,
        };
    }
    let Some(relative) = plan.relative else {
        return Ok(Candidates::not_found("nothing to resolve for that show"));
    };
    let watched = watched_episodes(ctx, plan, show_id).await?;
    let outcome =
        descend::relative_episode(ctx.index, show_id, relative, ctx.now, &watched).await?;
    match outcome.episode {
        Some(doc) => {
            let mut out = single_doc(sctx, &doc);
            out.note = outcome.note;
            Ok(out)
        }
        None => Ok(Candidates::not_found(relative_miss(relative))),
    }
}

/// A season the library does not hold, for a show it does.
///
/// The provider pools only run when the library pool came back *empty*, so a
/// show already on the shelf could never reach them — which made "season 25 of
/// a show we have" the one shape with no route to the metadata provider that
/// has it, and no way to want it. The show's own title is the query, not the
/// caller's phrasing, because the show is already identified.
async fn season_from_provider(
    ctx: &ResolveContext<'_>,
    sctx: &ScoreCtx<'_>,
    plan: &Plan,
    show_id: Uuid,
    season: i32,
) -> Result<Candidates, ResolveError> {
    let missing = || {
        Candidates::not_found(format!(
            "there is no season {season} in the library, and no provider has one for that show"
        ))
    };
    let Some(show) = descend::show_by_id(ctx.index, show_id).await? else {
        return Ok(missing());
    };
    let tokens = text::strict_content_tokens(&show.title);
    let trial = ScoreCtx {
        query: &show.title,
        query_tokens: &tokens,
        year: show.year,
        ..*sctx
    };
    let hits = ctx.providers.shows(&show.title, show.year).await;
    let mut pool: Vec<Scored> = hits
        .iter()
        .map(|hit| build::from_provider_hit(&trial, hit, MediaKind::Show))
        .collect();
    build::finalize_scores(&mut pool);
    retain_plausible(&mut pool);
    // Two provider shows this close is not a season we can ask about — the
    // question would be about the show, which the caller already named.
    if pool.is_empty() || ambiguous_count(&pool) > 1 {
        return Ok(missing());
    }
    let subject = pool.remove(0);
    let Some(provider) = subject.candidate.media_ref.provider_ref().cloned() else {
        return Ok(missing());
    };
    Ok(provider_single(ctx, subject, provider, plan).await)
}

async fn descend_episode_context(
    ctx: &ResolveContext<'_>,
    sctx: &ScoreCtx<'_>,
    plan: &Plan,
    episode: &crate::search::TvEpisodeDocument,
) -> Result<Candidates, ResolveError> {
    let Ok(show_id) = Uuid::parse_str(&episode.show_id) else {
        return Ok(Candidates::not_found("that episode has no show"));
    };
    match plan.relative {
        Some(Relative::Next) => {
            let Ok(episode_id) = Uuid::parse_str(&episode.id) else {
                return Ok(Candidates::not_found("that episode has no id"));
            };
            match descend::next_after(ctx.index, show_id, episode_id).await? {
                Some(doc) => Ok(single_doc(sctx, &doc)),
                None => Ok(Candidates::not_found(
                    "that was the last one — nothing after it yet",
                )),
            }
        }
        Some(_) | None => descend_show(ctx, sctx, plan, show_id).await,
    }
}

async fn descend_books(
    ctx: &ResolveContext<'_>,
    sctx: &ScoreCtx<'_>,
    plan: &Plan,
    author_id: Option<Uuid>,
    series_id: Option<Uuid>,
    fallback: Scored,
) -> Result<Candidates, ResolveError> {
    if author_id.is_none() && series_id.is_none() {
        return Ok(single(fallback));
    }
    let briefs = ctx
        .store
        .newest_books(author_id, series_id, MAX_CANDIDATES as i64)
        .await?;
    if briefs.is_empty() {
        return Ok(single(fallback));
    }
    let mut pool: Vec<Scored> = briefs
        .iter()
        .map(|b| build::from_book_brief(sctx, b))
        .collect();
    // Ordering already came from the store; recency is what "the new one"
    // means and the pool is small enough for it to be decisive.
    for (i, scored) in pool.iter_mut().enumerate() {
        scored.recency_key = Some(-(i as f64));
    }
    if plan.relative == Some(Relative::First) {
        for scored in pool.iter_mut() {
            scored.recency_key = scored.recency_key.map(|k| -k);
        }
    }
    build::finalize_scores(&mut pool);
    Ok(single(pool.remove(0)))
}

/// A provider answer carries its own seasons when the provider can say, so
/// the model can answer "which seasons exist?" from the tool rather than from
/// memory. A season not in the list is not out, and asking for it fails here,
/// at resolve time, rather than at queue time.
async fn provider_single(
    ctx: &ResolveContext<'_>,
    subject: Scored,
    provider: ProviderRef,
    plan: &Plan,
) -> Candidates {
    let mut out = single(provider_scope(subject, provider.clone(), plan));
    let Some(candidate) = out.candidates.first() else {
        return out;
    };
    let attach = matches!(
        provider.scope,
        ProviderScope::Whole | ProviderScope::All | ProviderScope::Season(_)
    );
    if !attach {
        return out;
    }
    let seasons = ctx
        .providers
        .tv_seasons(&provider.provider, &provider.id)
        .await;
    if seasons.is_empty() {
        return out;
    }
    // The requested season can arrive as the provider's scope (the phrase named it)
    // or the caller added it — the person-tiebreak path asks the provider for the
    // title alone, so the season sits in the plan and the scope says Whole.
    let requested = match provider.scope {
        ProviderScope::Season(n) => Some(n),
        ProviderScope::Episode { season, .. } => Some(season),
        _ => plan.season.map(|s| s.max(0) as u32),
    };
    if let Some(n) = requested
        && !seasons.iter().any(|s| s.season == n)
    {
        let have = seasons
            .iter()
            .map(|s| s.season.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Candidates::not_found(format!(
            "no season {n} in {} — it has season {have}",
            candidate.title
        ));
    }
    out.seasons = Some(seasons);
    out
}

fn provider_scope(mut subject: Scored, provider: ProviderRef, plan: &Plan) -> Scored {
    let media_ref = match (plan.season, plan.episode) {
        (Some(season), Some(episode)) => MediaRef::provider_episode(
            provider.provider.clone(),
            provider.id.clone(),
            season.max(0) as u32,
            episode.max(0) as u32,
        ),
        (Some(season), None) => MediaRef::provider_season(
            provider.provider.clone(),
            provider.id.clone(),
            season.max(0) as u32,
        ),
        // "All seasons" names no single season, so the ref takes the
        // all-seasons scope: confirmation then queues every season instead of
        // sitting on a whole-show ref that cannot be queued.
        (None, None) if plan.all_seasons => {
            MediaRef::provider_all(provider.provider.clone(), provider.id.clone())
        }
        _ => subject.candidate.media_ref.clone(),
    };
    subject.candidate.media_ref = media_ref;
    subject
}

// =============================================================================
// Finishing
// =============================================================================

async fn finish(
    ctx: &ResolveContext<'_>,
    plan: &Plan,
    out: &mut Candidates,
) -> Result<(), ResolveError> {
    out.candidates.truncate(MAX_CANDIDATES);
    label_sports(ctx, &mut out.candidates).await?;
    let bookish = out
        .candidates
        .iter()
        .any(|c| c.media_ref.kind() == MediaKind::Book);
    if bookish {
        let skew = ctx.store.format_skew(ctx.user_id).await?;
        let (format, question) = format::resolve_format(plan.explicit_format, skew);
        out.format = format;
        out.format_question = question;
    }
    Ok(())
}

/// Sports ride the TV rails; `content_category` is the only thing that tells
/// them apart, and it is not in any search document.
async fn label_sports(
    ctx: &ResolveContext<'_>,
    candidates: &mut [Candidate],
) -> Result<(), ResolveError> {
    let show_ids: Vec<Uuid> = candidates.iter().filter_map(|c| c.show_id).collect();
    if show_ids.is_empty() {
        return Ok(());
    }
    let categories = ctx.store.content_categories(&show_ids).await?;
    for candidate in candidates.iter_mut() {
        let Some(show_id) = candidate.show_id else {
            continue;
        };
        if categories.get(&show_id).map(String::as_str) != Some("sport") {
            continue;
        }
        candidate.is_sports = true;
        if candidate.media_ref.kind() == MediaKind::Episode
            && let Some(id) = candidate.media_ref.local_id()
        {
            candidate.media_ref = MediaRef::sports_event(id);
        }
    }
    Ok(())
}

async fn watched_episodes(
    ctx: &ResolveContext<'_>,
    plan: &Plan,
    show_id: Uuid,
) -> Result<HashSet<Uuid>, ResolveError> {
    if !plan.exclude_watched {
        return Ok(HashSet::new());
    }
    let episodes = descend::all_episodes_in_order(ctx.index, show_id).await?;
    let ids: Vec<Uuid> = episodes
        .iter()
        .filter_map(|e| Uuid::parse_str(&e.id).ok())
        .collect();
    ctx.store.watched_episode_ids(ctx.user_id, &ids).await
}

async fn subtract_watched_movies(
    ctx: &ResolveContext<'_>,
    pool: &mut Vec<Scored>,
) -> Result<(), ResolveError> {
    let movie_ids: Vec<Uuid> = pool
        .iter()
        .filter(|s| s.candidate.media_ref.kind() == MediaKind::Movie)
        .filter_map(|s| s.local_id())
        .collect();
    if movie_ids.is_empty() {
        return Ok(());
    }
    let watched = ctx.store.watched_movie_ids(ctx.user_id, &movie_ids).await?;
    pool.retain(|s| match s.local_id() {
        Some(id) if s.candidate.media_ref.kind() == MediaKind::Movie => !watched.contains(&id),
        _ => true,
    });
    Ok(())
}

// =============================================================================
// Small helpers
// =============================================================================

fn single(mut scored: Scored) -> Candidates {
    scored.candidate.score = ranking::rank_score(&scored.signals);
    Candidates {
        resolution: Resolution::Single,
        candidates: vec![scored.candidate],
        question: None,
        format: None,
        format_question: None,
        note: None,
        pick_qualifier: None,
        seasons: None,
    }
}

/// The part of the request a tap on a candidate cannot carry on its own.
fn pick_qualifier(plan: &Plan) -> Option<String> {
    match (plan.season, plan.episode) {
        (Some(season), Some(episode)) => Some(descend::episode_code(season, episode)),
        (Some(season), None) => Some(format!("season {season}")),
        _ => None,
    }
}

fn single_doc(sctx: &ScoreCtx<'_>, doc: &crate::search::TvEpisodeDocument) -> Candidates {
    match build::from_episode_doc(sctx, doc) {
        Some(scored) => single(scored),
        None => Candidates::not_found("that episode has no id"),
    }
}

fn single_season(sctx: &ScoreCtx<'_>, doc: &crate::search::TvSeasonDocument) -> Candidates {
    match build::from_season_doc(sctx, doc) {
        Some(scored) => single(scored),
        None => Candidates::not_found("that season has no id"),
    }
}

fn context_question(relative: Option<Relative>) -> String {
    match relative {
        Some(Relative::Latest) => "The latest what? Which show or author do you mean?",
        Some(Relative::Next) => "The next what? Which show do you mean?",
        Some(Relative::LastNight) => "Last night's what? Which show do you mean?",
        Some(Relative::First) => "The first what? Which show do you mean?",
        None => "Which one do you mean?",
    }
    .to_string()
}

fn relative_miss(relative: Relative) -> String {
    match relative {
        Relative::Latest => "nothing has aired for that yet",
        Relative::Next => "nothing is scheduled for that yet",
        Relative::LastNight => "nothing aired for that last night",
        Relative::First => "there are no episodes for that yet",
    }
    .to_string()
}

fn which_question(candidates: &[Candidate]) -> String {
    let options: Vec<String> = candidates
        .iter()
        .map(|c| match &c.subtitle {
            Some(sub) => format!("{} ({sub})", c.title),
            None => c.title.clone(),
        })
        .collect();
    format!("Which one — {}?", options.join(", or "))
}

/// Sports label a show's episodes without a special case beyond the label.
/// Kept public so `want_it` can ask the same question of a bare ref.
pub fn is_sports_category(category: &str) -> bool {
    category == "sport"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_media_type_filters_but_relaxes_for_containers() {
        let plan = |hint, descending| Plan {
            season: None,
            episode: None,
            relative: None,
            explicit_hint: hint,
            effective_hint: hint,
            exclude_watched: false,
            explicit_format: None,
            descending,
            all_seasons: false,
        };
        let book_only = plan(Some(MediaTypeHint::Book), false);
        assert!(allows_subject(&book_only, MediaKind::Book));
        assert!(!allows_subject(&book_only, MediaKind::Movie));
        assert!(!allows_subject(&book_only, MediaKind::Author));

        let descending = plan(Some(MediaTypeHint::Book), true);
        assert!(allows_subject(&descending, MediaKind::Author));
        assert!(!allows_subject(&descending, MediaKind::Movie));

        let anything = plan(None, false);
        assert!(allows_subject(&anything, MediaKind::Movie));
    }

    #[test]
    fn all_seasons_stamps_the_all_scope_on_a_provider_show() {
        let plan = Plan {
            season: None,
            episode: None,
            relative: None,
            explicit_hint: None,
            effective_hint: None,
            exclude_watched: false,
            explicit_format: None,
            descending: true,
            all_seasons: true,
        };
        let hit = providers::ProviderHit {
            provider: "tmdb".to_string(),
            id: "95396".to_string(),
            title: "Deadloch".to_string(),
            year: Some(2023),
            authors: vec![],
            description: None,
            rating: None,
            payload: ProviderPayload::default(),
        };
        let tokens = vec!["deadloch".to_string()];
        let scored = build::from_provider_hit(
            &ScoreCtx {
                query: "deadloch",
                query_tokens: &tokens,
                year: None,
                soft_hint: None,
                author: None,
                relative: None,
                format: None,
                descending: true,
            },
            &hit,
            MediaKind::Show,
        );
        let out = provider_scope(
            scored,
            ProviderRef {
                provider: "tmdb".to_string(),
                id: "95396".to_string(),
                kind: MediaKind::Show,
                scope: ProviderScope::Whole,
            },
            &plan,
        );
        assert_eq!(
            out.candidate.media_ref.to_string(),
            "provider:tmdb:95396:all"
        );
    }

    #[test]
    fn ambiguity_counts_only_the_close_ones() {
        fn scored(score: f64) -> Scored {
            let mut s = build::from_author_doc(
                &ScoreCtx {
                    query: "",
                    query_tokens: &[],
                    year: None,
                    soft_hint: None,
                    author: None,
                    relative: None,
                    format: None,
                    descending: false,
                },
                &crate::search::AuthorDocument {
                    id: Uuid::new_v4().to_string(),
                    name: "x".to_string(),
                    image_url: None,
                    book_count: 0,
                    is_monitored: false,
                },
            )
            .unwrap();
            s.candidate.score = score;
            s
        }
        assert_eq!(ambiguous_count(&[]), 0);
        assert_eq!(ambiguous_count(&[scored(0.9)]), 1);
        assert_eq!(ambiguous_count(&[scored(0.9), scored(0.88)]), 2);
        assert_eq!(ambiguous_count(&[scored(0.9), scored(0.5)]), 1);
    }

    #[test]
    fn person_in_name_matches_in_order_and_needs_two_tokens() {
        assert!(person_in_name("David Mitchell", "David Mitchell"));
        assert!(person_in_name("David Mitchell", "David R. Mitchell"));
        assert!(!person_in_name("David Mitchell", "Mitchell David"));
        assert!(!person_in_name("David", "David R. Mitchell"));
        assert!(!person_in_name("David Mitchell", "Davina Mitchell"));
    }
}
