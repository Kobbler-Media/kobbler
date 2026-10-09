//! The tools that do things, and the confirmation that stands between the model
//! and a download.
//!
//! Three rules, in the order they matter:
//!
//! 1. **Ambiguity is a question, never a guess.** Downloading the wrong show is
//!    the failure that ends the trial, so an `Ambiguous` or `NeedsContext`
//!    resolution returns candidates and a question and touches nothing.
//! 2. **Read is free, write is confirmed.** Anything that spends disk —
//!    `want_it`, `grab_release`, `regrab` — answers the first call with a plain
//!    sentence naming the action and a token, and only acts when the token comes
//!    back. "When does X come out" and "can you get X" are one word apart, and
//!    erring toward downloading is the expensive direction.
//! 3. **Every action ends in a message.** Synchronous work says what it did;
//!    anything Kobload takes away registers a [`chat_watches`] watch so the
//!    thread hears the end of it. Silence reads as broken.
//!
//! Media type is never a fork in the tool list. It comes off the [`MediaRef`]
//! the resolver stamped, which is why the model never picks an endpoint.

use std::str::FromStr as _;
use std::time::Duration as StdDuration;

use axum::extract::{Json as JsonBody, Path, Query, State};
use axum::response::IntoResponse;
use serde_json::{Value, json};
use time::Duration;
use uuid::Uuid;

use crate::AppState;
use crate::chatbot::resolve::{MediaKind, MediaRef, ProviderScope};
use crate::file_link_mode::AcquireQuery;
use crate::repositories::{
    chat_pending, chat_watches, movie_files, tv_episode_files, tv_episodes, tv_seasons, watchlist,
};

use super::dispatch::Ctx;
use super::read::ToolOutcome;

/// How far back an acquisition tool will look for the `resolve_media` result
/// that produced its ref. A conversation can
/// wander between resolving something and deciding to get it.
const CANDIDATE_LOOKBACK: Duration = Duration::hours(2);

/// Releases handed to the model. A wide list drowns a 12B model and the picker
/// is deliberately off the critical path — `regrab` exists so nobody has to use
/// one.
const MAX_RELEASES: usize = 5;

/// Whether a regrab of this kind of thing keeps the copy already on disk.
///
/// Video is additive: `additional_copy` rides the queue request through
/// `file_link_mode::tag_metadata`, comes back on the ingest callback, and reaches
/// `link_episodes_with_mode` / `replace_other_files_for_movie` as
/// `FileLinkMode::AddCopy`. Books are not: `book_files` is `ON CONFLICT
/// (book_id, format) DO UPDATE`, so a second EPUB overwrites the first and no
/// flag exists to say otherwise.
///
/// The wording in [`regrab`] follows this, and must never claim the old file is
/// still there where it is not.
fn regrab_keeps_existing_file(kind: MediaKind) -> bool {
    matches!(
        kind,
        MediaKind::Movie | MediaKind::Episode | MediaKind::SportsEvent
    )
}

/// Cap on a handler response body we read back in-process.
const MAX_BODY: usize = 4 * 1024 * 1024;

// =============================================================================
// Argument plumbing
// =============================================================================

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn arg_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

fn arg_u32(args: &Value, key: &str) -> Option<u32> {
    args.get(key)
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        })?
        .try_into()
        .ok()
}

fn media_ref(args: &Value) -> Result<MediaRef, ToolOutcome> {
    let Some(raw) = arg_str(args, "ref") else {
        return Err(ToolOutcome::failed(
            "that needs a ref from resolve_media first — resolve what they meant, then act on it",
        ));
    };
    MediaRef::from_str(raw).map_err(|e| {
        ToolOutcome::failed(format!(
            "{raw} is not a ref I recognise ({e}) - season refs look like 'provider:tmdb:243360:s02', show refs like 'provider:tmdb:243360'. Call resolve_media and use the ref it returns; do not invent one."
        ))
    })
}

/// There is no confirmation step: an explicit request acts on the spot, and an
/// unclear one is asked about in plain text before the tool is called at all
/// (a "yes" then simply arrives as the next message). Kept as the single seam
/// every write tool passes through so a future guard has one place to live.
pub(super) fn gate(_ctx: &Ctx<'_>, _action: &str, _target: &str, args: &Value) -> Value {
    args.clone()
}

// =============================================================================
// Resolution — a ref, a label, and never a guess
// =============================================================================

struct Subject {
    media_ref: MediaRef,
    label: String,
    /// The candidate this ref came from, when `resolve_media` produced it in this
    /// conversation. Carries the provider payload for anything not in the library.
    candidate: Option<Value>,
}

impl Subject {
    /// Enough of a provider record to create the row. Comes off the resolver's
    /// own candidate, never off anything the model retyped.
    fn provider_payload(&self) -> Value {
        self.candidate
            .as_ref()
            .and_then(|c| c.get("provider_payload"))
            .cloned()
            .unwrap_or_else(|| json!({}))
    }
}

/// Turn the `ref` argument into something to act on, or into a question.
///
/// A bare title is accepted and resolved, because the model will occasionally
/// pass one; an ambiguous result comes back as candidates and a question, and
/// the caller must return it untouched.
async fn subject(ctx: &Ctx<'_>, args: &Value) -> Result<Subject, ToolOutcome> {
    let media_ref = compose_provider_season(media_ref(args)?, args)?;
    let mut candidate = chat_pending::candidate(
        &ctx.state.pool,
        ctx.session_id,
        &media_ref.to_string(),
        CANDIDATE_LOOKBACK,
    )
    .await
    .unwrap_or(None);
    // A season ref composed from a show ref was never in a resolution result of
    // its own; the show's candidate carries the title and the provider payload.
    if candidate.is_none()
        && let Some(p) = media_ref.provider_ref()
        && matches!(
            p.scope,
            ProviderScope::Season(_) | ProviderScope::Episode { .. }
        )
    {
        let parent = MediaRef::provider(p.provider.clone(), p.id.clone(), MediaKind::Show);
        candidate = chat_pending::candidate(
            &ctx.state.pool,
            ctx.session_id,
            &parent.to_string(),
            CANDIDATE_LOOKBACK,
        )
        .await
        .unwrap_or(None);
    }
    let label = label_for(ctx.state, &media_ref, candidate.as_ref()).await;
    Ok(Subject {
        media_ref,
        label,
        candidate,
    })
}

/// A provider show ref plus a season the user named is a season ref.
///
/// The resolver offers the show when several shows share the name and the
/// model picked the right one off the year. Without this composition the only
/// path to the season ref is for the model to invent the grammar, and it will
/// get it wrong.
fn compose_provider_season(media_ref: MediaRef, args: &Value) -> Result<MediaRef, ToolOutcome> {
    let Some(p) = media_ref.provider_ref() else {
        return Ok(media_ref);
    };
    if p.kind != MediaKind::Show || p.scope != ProviderScope::Whole {
        return Ok(media_ref);
    }
    let Some(season) = arg_u32(args, "season") else {
        return Ok(media_ref);
    };
    let composed = match arg_u32(args, "episode") {
        Some(episode) => {
            MediaRef::provider_episode(p.provider.clone(), p.id.clone(), season, episode)
        }
        None => MediaRef::provider_season(p.provider.clone(), p.id.clone(), season),
    };
    Ok(composed)
}

/// What to call this thing in every sentence about it.
///
/// A vague echo is a useless echo — "I'm about to get that show" gives her
/// nothing to say stop to — so the resolver's own title is preferred over
/// anything derived from the ref.
async fn label_for(state: &AppState, media_ref: &MediaRef, candidate: Option<&Value>) -> String {
    if let Some(id) = media_ref.local_id()
        && let Some(name) = local_label(state, media_ref.kind(), id).await
    {
        return name;
    }
    let named = candidate
        .and_then(|c| c.get("title"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty());
    if let Some(p) = media_ref.provider_ref() {
        // The year goes into the label: "I'm about to get Unexpected" lets a
        // wrong-year match through unnoticed, and the say sentence is the
        // user's one chance to catch it.
        let subtitle = candidate
            .and_then(|c| c.get("subtitle"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let what = match (named, subtitle) {
            (Some(t), Some(s)) => format!("{t} ({s})"),
            (Some(t), None) => t.to_string(),
            (None, _) => "it".to_string(),
        };
        return match p.scope {
            ProviderScope::Whole => what.to_string(),
            ProviderScope::All => format!("all of {what}"),
            ProviderScope::Season(s) => format!("season {s} of {what}"),
            ProviderScope::Episode { season, episode } => {
                format!("S{season:02}E{episode:02} of {what}")
            }
        };
    }
    named
        .map(str::to_string)
        .unwrap_or_else(|| media_ref.to_string())
}

async fn local_label(state: &AppState, kind: MediaKind, id: Uuid) -> Option<String> {
    let pool = &state.pool;
    match kind {
        MediaKind::Book => sqlx::query_scalar!("SELECT title FROM books WHERE id = $1", id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten(),
        MediaKind::Movie => sqlx::query_scalar!("SELECT title FROM movies WHERE id = $1", id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten(),
        MediaKind::Show => sqlx::query_scalar!("SELECT title FROM tv_shows WHERE id = $1", id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten(),
        MediaKind::Season => sqlx::query!(
            r#"
            SELECT sh.title AS "show!", se.season_identifier AS "season!"
            FROM tv_seasons se JOIN tv_shows sh ON sh.id = se.show_id
            WHERE se.id = $1
            "#,
            id
        )
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .map(|r| format!("season {} of {}", r.season, r.show)),
        MediaKind::Episode | MediaKind::SportsEvent => sqlx::query!(
            r#"
            SELECT sh.title AS "show!", se.season_identifier AS "season!",
                   ep.episode_identifier AS "episode!", ep.title AS "title?"
            FROM tv_episodes ep
            JOIN tv_seasons se ON se.id = ep.season_id
            JOIN tv_shows sh ON sh.id = se.show_id
            WHERE ep.id = $1
            "#,
            id
        )
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .map(|r| match r.title {
            Some(t) if !t.is_empty() => {
                format!("S{}E{} of {} (\"{t}\")", r.season, r.episode, r.show)
            }
            _ => format!("S{}E{} of {}", r.season, r.episode, r.show),
        }),
        MediaKind::Author => sqlx::query_scalar!("SELECT name FROM authors WHERE id = $1", id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten(),
        MediaKind::Series => sqlx::query_scalar!("SELECT name FROM series WHERE id = $1", id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten(),
    }
}

/// Sports fixtures are `tv_episodes` rows; the want and grab endpoints differ.
async fn is_sports(state: &AppState, episode_id: Uuid) -> bool {
    sqlx::query_scalar!(
        r#"
        SELECT COALESCE(sh.content_category, 'tv') AS "category!"
        FROM tv_episodes ep
        JOIN tv_seasons se ON se.id = ep.season_id
        JOIN tv_shows sh ON sh.id = se.show_id
        WHERE ep.id = $1
        "#,
        episode_id
    )
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
    .is_some_and(|c| c == "sport")
}

// =============================================================================
// Calling our own handlers
// =============================================================================

/// Run one of kobbler's axum handlers in-process and read its body.
///
/// Reusing the handler rather than a private copy is deliberate: attribution,
/// `wanted_attempts`, the want-status transitions, the NATS payloads and the
/// library events are all in there, and a second implementation is how they
/// drift. There is no HTTP round trip — this is a plain function call.
async fn body_of(response: axum::response::Response) -> (u16, Value) {
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), MAX_BODY)
        .await
        .unwrap_or_default();
    let value = serde_json::from_slice::<Value>(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).trim().to_string()));
    (status, value)
}

fn failed_call(what: &str, status: u16, body: &Value) -> ToolOutcome {
    let detail = body
        .get("error")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| match body {
            Value::String(s) if !s.is_empty() => s.clone(),
            _ => format!("status {status}"),
        });
    ToolOutcome::failed(format!("{what} did not work — {detail}"))
}

// =============================================================================
// chat_watches — the single reconciliation point
// =============================================================================

/// Register a watch so an asynchronous action still ends in a message.
///
/// **Every act tool that starts a download calls this and only this.** It is the
/// single point of contact with `repositories::chat_watches`, so a change to
/// that API is a change to this function body and nothing else.
///
/// A web session has no conversation address (`space` is NULL for the Command
/// Palette, which watches its own SSE stream), so there is nothing to register
/// and `false` is the honest answer rather than a failure.
///
/// `message_id` is deliberately `None`. A tool runs *during* the turn: the
/// confirmation sentence is text the model has not finished producing, and the
/// transport posts it only after the turn ends, so no transport message id
/// exists yet. `Turn::message_id` is the `chat_messages` row's UUID, not a
/// Google Chat message name, and storing it here would make every progress
/// update PATCH a resource that does not exist. `watches::send_with` handles
/// `None` by posting and recording the id it gets back, so the thread still
/// collapses to one evolving message from the first progress update onward.
/// Folding the confirmation itself into that chain needs the transport's posted
/// id threaded back into the loop, which is a change at the loop/transport seam,
/// not here.
async fn register_watch(
    ctx: &Ctx<'_>,
    entity_kind: chat_watches::EntityKind,
    entity_id: Uuid,
    label: &str,
) -> bool {
    let Ok(Some(session)) = session_address(ctx).await else {
        return false;
    };
    let target = crate::chatbot::transport::Target {
        transport: session.0,
        space: session.1,
        thread: session.2,
    };
    let input = chat_watches::NewWatch {
        target: &target,
        message_id: None,
        entity_kind,
        entity_id,
        label,
        user_id: Some(ctx.auth.user_id),
        ttl: StdDuration::from_secs(chat_watches::DEFAULT_TTL.as_secs()),
    };
    match chat_watches::watch(&ctx.state.pool, input).await {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(error = %e, entity_id = %entity_id, "chat watch registration failed");
            false
        }
    }
}

type Address = (String, String, Option<String>);

async fn session_address(ctx: &Ctx<'_>) -> Result<Option<Address>, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT transport, space, thread FROM chat_sessions WHERE id = $1"#,
        ctx.session_id
    )
    .fetch_optional(&ctx.state.pool)
    .await?;
    Ok(row.and_then(|r| r.space.map(|space| (r.transport, space, r.thread))))
}

/// Watches are per entity, so a season want registers one per episode it queued.
async fn watch_all(
    ctx: &Ctx<'_>,
    kind: chat_watches::EntityKind,
    ids: &[Uuid],
    label: &str,
) -> usize {
    let mut n = 0;
    for id in ids {
        if register_watch(ctx, kind, *id, label).await {
            n += 1;
        }
    }
    n
}

fn watch_kind(kind: MediaKind) -> chat_watches::EntityKind {
    match kind {
        MediaKind::Book => chat_watches::EntityKind::Book,
        MediaKind::Movie => chat_watches::EntityKind::Movie,
        _ => chat_watches::EntityKind::Episode,
    }
}

// =============================================================================
// want_it
// =============================================================================

/// A provider show with no season named is not one request, and no
/// confirmation can ever be spent on it: nothing can queue a whole show we do
/// not have. So the question is asked before any offer is minted, and the gate
/// only ever guards a ref that can be spent — a season, an episode, or all of
/// them.
fn provider_show_question(subject: &Subject) -> Option<ToolOutcome> {
    let p = subject.media_ref.provider_ref()?;
    if p.kind != MediaKind::Show || p.scope != ProviderScope::Whole {
        return None;
    }
    let named = subject.label.clone();
    Some(ToolOutcome::ok(
        json!({
            "ok": true,
            "needs_answer": true,
            "target": named,
            "note": "we do not have this show at all, so which season do they mean? \
                     Ask them, then call want_it again with the same ref and their answer \
                     as the season argument - \"<title> all seasons\" if they want every one. \
                     Do not pick one.",
        }),
        format!("{named}: which season?"),
    ))
}

/// `want_it` — get me this, whatever kind of thing it is.
///
/// The [`MediaRef`] decides which of the acquisition paths runs. The model has
/// never chosen one and must not start: it passes the ref the resolver stamped.
pub async fn want_it(ctx: &Ctx<'_>, args: &Value) -> ToolOutcome {
    let subject = match subject(ctx, args).await {
        Ok(s) => s,
        Err(outcome) => return outcome,
    };
    if let Some(question) = provider_show_question(&subject) {
        return question;
    }
    // "get someShow" and "get the new episode" are one sentence apart and three
    // hundred gigabytes apart, so the echo carries the size before she agrees.
    let announced = match scale_of(ctx, &subject.media_ref).await {
        Some(n) if n > 1 => format!("{} — {n} episodes you don't have yet", subject.label),
        _ => subject.label.clone(),
    };
    let args = gate(ctx, "get", &announced, args);
    perform_want(ctx, &subject, &args, AcquireQuery::default()).await
}

/// How many episodes a season or show want would actually queue. `None` for
/// anything that is one thing.
async fn scale_of(ctx: &Ctx<'_>, media_ref: &MediaRef) -> Option<usize> {
    let id = media_ref.local_id()?;
    let pool = &ctx.state.pool;
    match media_ref.kind() {
        MediaKind::Season => Some(tv_episodes::list_missing(pool, id).await.ok()?.len()),
        MediaKind::Show => {
            let seasons = tv_seasons::list_by_show(pool, id).await.ok()?;
            let mut n = 0;
            for season in seasons {
                n += tv_episodes::list_missing(pool, season.id)
                    .await
                    .map(|m| m.len())
                    .unwrap_or(0);
            }
            Some(n)
        }
        _ => None,
    }
}

async fn perform_want(
    ctx: &Ctx<'_>,
    subject: &Subject,
    args: &Value,
    acquire: AcquireQuery,
) -> ToolOutcome {
    let state = ctx.state;
    let auth = ctx.auth.clone();
    match &subject.media_ref {
        MediaRef::Local { kind, id } => match kind {
            MediaKind::Book => want_local_book(ctx, *id, &subject.label, acquire).await,
            MediaKind::Movie => {
                let response = crate::routes::admin::movies::want_movie(
                    State(state.clone()),
                    auth,
                    Path(*id),
                    Query(acquire),
                )
                .await
                .into_response();
                let (status, body) = body_of(response).await;
                if !(200..300).contains(&status) {
                    return failed_call("wanting that film", status, &body);
                }
                let watched =
                    register_watch(ctx, chat_watches::EntityKind::Movie, *id, &subject.label).await;
                done_one(&subject.label, *id, watched)
            }
            MediaKind::Episode | MediaKind::SportsEvent => {
                want_local_episode(ctx, *id, &subject.label, acquire).await
            }
            MediaKind::Season => want_local_season(ctx, *id, &subject.label).await,
            MediaKind::Show => want_whole_show(ctx, *id, &subject.label).await,
            MediaKind::Author => monitor(ctx, "author", *id, &subject.label).await,
            MediaKind::Series => monitor(ctx, "series", *id, &subject.label).await,
        },
        MediaRef::Provider(_) => want_from_provider(ctx, subject, args).await,
    }
}

fn done_one(label: &str, entity_id: Uuid, watched: bool) -> ToolOutcome {
    ToolOutcome::ok(
        json!({
            "ok": true,
            "done": true,
            "target": label,
            "entity_id": entity_id,
            "watching": watched,
            "say": format!("Right, {label} is on the way — I'll tell you when it lands."),
        }),
        format!("wanted {label}"),
    )
}

async fn want_local_book(
    ctx: &Ctx<'_>,
    id: Uuid,
    label: &str,
    acquire: AcquireQuery,
) -> ToolOutcome {
    let response = crate::routes::library::want_book(
        State(ctx.state.clone()),
        ctx.auth.clone(),
        Path(id),
        Query(acquire),
    )
    .await
    .into_response();
    let (status, body) = body_of(response).await;
    if !(200..300).contains(&status) {
        return failed_call("wanting that book", status, &body);
    }
    let watched = register_watch(ctx, chat_watches::EntityKind::Book, id, label).await;
    done_one(label, id, watched)
}

async fn want_local_episode(
    ctx: &Ctx<'_>,
    id: Uuid,
    label: &str,
    acquire: AcquireQuery,
) -> ToolOutcome {
    let sports = is_sports(ctx.state, id).await;
    let response = match sports {
        true => crate::routes::admin::sports::want_event(
            State(ctx.state.clone()),
            ctx.auth.clone(),
            Path(id),
            Query(acquire),
        )
        .await
        .into_response(),
        false => crate::routes::admin::tv::want_episode(
            State(ctx.state.clone()),
            ctx.auth.clone(),
            Path(id),
            Query(acquire),
        )
        .await
        .into_response(),
    };
    let (status, body) = body_of(response).await;
    if !(200..300).contains(&status) {
        return failed_call("wanting that episode", status, &body);
    }
    let watched = register_watch(ctx, chat_watches::EntityKind::Episode, id, label).await;
    done_one(label, id, watched)
}

/// A season want must say what is already here.
///
/// "You've already got 22 of those, grabbing the other 2" is the difference
/// between a bot that knows the library and one that silently re-queues.
async fn want_local_season(ctx: &Ctx<'_>, season_id: Uuid, label: &str) -> ToolOutcome {
    let pool = &ctx.state.pool;
    let all = match tv_episodes::list_for_season(pool, season_id).await {
        Ok(rows) => rows,
        Err(e) => return ToolOutcome::failed(format!("could not read that season: {e}")),
    };
    if all.is_empty() {
        return ToolOutcome::failed(format!("{label} has no episodes listed yet"));
    }
    let already_have = all.iter().filter(|e| e.is_in_library).count();
    let already_wanted = all
        .iter()
        .filter(|e| !e.is_in_library && e.want_status != "none")
        .count();
    let missing = match tv_episodes::list_missing(pool, season_id).await {
        Ok(rows) => rows,
        Err(e) => return ToolOutcome::failed(format!("could not read that season: {e}")),
    };

    if missing.is_empty() {
        return ToolOutcome::ok(
            json!({
                "ok": true,
                "done": false,
                "queued": false,
                "target": label,
                "total_episodes": all.len(),
                "already_have": already_have,
                "already_wanted": already_wanted,
                "newly_wanted": 0,
                "say": already_wanted_sentence(label, already_have, already_wanted),
            }),
            format!("{label}: nothing missing"),
        );
    }

    let response = crate::routes::admin::tv::want_season(
        State(ctx.state.clone()),
        ctx.auth.clone(),
        Path(season_id),
    )
    .await
    .into_response();
    let (status, body) = body_of(response).await;
    if !(200..300).contains(&status) {
        return failed_call("wanting that season", status, &body);
    }

    let episode_ids: Vec<Uuid> = missing.iter().map(|e| e.id).collect();
    let watched = watch_all(ctx, chat_watches::EntityKind::Episode, &episode_ids, label).await;
    season_outcome(
        label,
        all.len(),
        already_have,
        already_wanted,
        &episode_ids,
        watched,
    )
}

fn already_wanted_sentence(label: &str, already_have: usize, already_wanted: usize) -> String {
    match (already_have, already_wanted) {
        (0, n) => format!("You've already asked for all {n} of {label} — nothing new to do."),
        (n, 0) => format!("You've already got all {n} of {label} — nothing to grab."),
        (h, w) => format!(
            "You've already got {h} of {label} and asked for the other {w} — nothing new to do."
        ),
    }
}

fn season_outcome(
    label: &str,
    total: usize,
    already_have: usize,
    already_wanted: usize,
    queued: &[Uuid],
    watched: usize,
) -> ToolOutcome {
    let say = match already_have {
        0 => format!("Grabbing all {} of {label}.", queued.len()),
        n => format!(
            "You've already got {n} of those — grabbing the other {}.",
            queued.len()
        ),
    };
    ToolOutcome::ok(
        json!({
            "ok": true,
            "done": true,
            "queued": true,
            "target": label,
            "total_episodes": total,
            "already_have": already_have,
            "already_wanted": already_wanted,
            "newly_wanted": queued.len(),
            "watching": watched,
            "say": say,
        }),
        format!(
            "{label}: {} queued, {already_have} already here",
            queued.len()
        ),
    )
}

/// A whole show. Counts first, for the same reason a season reports them.
async fn want_whole_show(ctx: &Ctx<'_>, show_id: Uuid, label: &str) -> ToolOutcome {
    let pool = &ctx.state.pool;
    let seasons = match tv_seasons::list_by_show(pool, show_id).await {
        Ok(rows) => rows,
        Err(e) => return ToolOutcome::failed(format!("could not read that show: {e}")),
    };
    let mut total = 0usize;
    let mut already_have = 0usize;
    let mut already_wanted = 0usize;
    let mut missing_ids: Vec<Uuid> = Vec::new();
    for season in &seasons {
        let all = tv_episodes::list_for_season(pool, season.id)
            .await
            .unwrap_or_default();
        total += all.len();
        already_have += all.iter().filter(|e| e.is_in_library).count();
        already_wanted += all
            .iter()
            .filter(|e| !e.is_in_library && e.want_status != "none")
            .count();
        if let Ok(missing) = tv_episodes::list_missing(pool, season.id).await {
            missing_ids.extend(missing.into_iter().map(|e| e.id));
        }
    }

    if missing_ids.is_empty() {
        return ToolOutcome::ok(
            json!({
                "ok": true,
                "done": false,
                "queued": false,
                "target": label,
                "total_episodes": total,
                "already_have": already_have,
                "already_wanted": already_wanted,
                "newly_wanted": 0,
                "say": already_wanted_sentence(label, already_have, already_wanted),
            }),
            format!("{label}: nothing missing"),
        );
    }

    let response = crate::routes::admin::tv::want_all_missing(
        State(ctx.state.clone()),
        ctx.auth.clone(),
        Path(show_id),
    )
    .await
    .into_response();
    let (status, body) = body_of(response).await;
    if !(200..300).contains(&status) {
        return failed_call("wanting that show", status, &body);
    }
    let watched = watch_all(ctx, chat_watches::EntityKind::Episode, &missing_ids, label).await;
    season_outcome(
        label,
        total,
        already_have,
        already_wanted,
        &missing_ids,
        watched,
    )
}

/// "Get everything new by this" — the books analogue of monitoring a show, and
/// the same tool with a different ref rather than a tool of its own.
async fn monitor(ctx: &Ctx<'_>, what: &str, id: Uuid, label: &str) -> ToolOutcome {
    let body = json!({"is_monitored": true, "auto_want_mode": "new_only"});
    let payload = match serde_json::from_value(body) {
        Ok(p) => p,
        Err(e) => return ToolOutcome::failed(format!("could not set up monitoring: {e}")),
    };
    let response = match what {
        "author" => crate::routes::library::update_author_monitoring(
            State(ctx.state.clone()),
            ctx.auth.clone(),
            Path(id),
            JsonBody(payload),
        )
        .await
        .into_response(),
        _ => crate::routes::library::update_series_monitoring(
            State(ctx.state.clone()),
            ctx.auth.clone(),
            Path(id),
            JsonBody(payload),
        )
        .await
        .into_response(),
    };
    let (status, resp) = body_of(response).await;
    if !(200..300).contains(&status) {
        return failed_call("setting that up", status, &resp);
    }
    ToolOutcome::ok(
        json!({
            "ok": true,
            "done": true,
            "monitoring": what,
            "target": label,
            "entity_id": id,
            "say": format!("Done — anything new by {label} will come in on its own from now on."),
        }),
        format!("monitoring {label}"),
    )
}

/// Not in the library yet. The provider ref carries everything the create-then-want
/// handlers need, so there is no second round trip to the provider.
async fn want_from_provider(ctx: &Ctx<'_>, subject: &Subject, args: &Value) -> ToolOutcome {
    let Some(p) = subject.media_ref.provider_ref() else {
        return ToolOutcome::failed("that ref does not name a provider record");
    };
    let payload = match args.get("provider_payload") {
        Some(v) if v.is_object() => v.clone(),
        _ => subject.provider_payload(),
    };
    // Creating a row called "that show" is worse than not creating one, and the
    // only honest source of a title is the resolution this ref came from.
    let title = payload
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .or_else(|| {
            subject
                .candidate
                .as_ref()
                .and_then(|c| c.get("title"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|t| !t.is_empty())
        });

    // A show with no season named is not one request, it is several. `want_it`
    // asks this before the gate mints anything; this is the backstop for any
    // other caller.
    if let Some(question) = provider_show_question(subject) {
        return question;
    }

    let Some(title) = title.map(str::to_string) else {
        return ToolOutcome::failed(
            "I have lost track of what that ref was — run resolve_media again and use the ref it returns",
        );
    };

    match (p.kind, p.scope) {
        (MediaKind::Book, _) => want_new_book(ctx, &payload, &title, arg_str(args, "format")).await,
        (MediaKind::Movie, _) => want_new_movie(ctx, p, &payload, &title).await,
        (MediaKind::Show, ProviderScope::All) => want_all_seasons(ctx, p, &payload, &title).await,
        (MediaKind::Season, ProviderScope::Season(n)) => {
            want_discover(ctx, p, &payload, &title, n, None).await
        }
        (MediaKind::Episode, ProviderScope::Episode { season, episode }) => {
            want_discover(ctx, p, &payload, &title, season, Some(episode)).await
        }
        _ => ToolOutcome::failed(format!(
            "I do not know how to get {} from {} yet",
            p.kind.as_str(),
            p.provider
        )),
    }
}

async fn want_new_book(
    ctx: &Ctx<'_>,
    payload: &Value,
    title: &str,
    format: Option<&str>,
) -> ToolOutcome {
    let authors = payload
        .get("authors")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str);
    let body = json!({
        "title": title,
        "author": authors,
        "identifiers": payload.get("identifiers").cloned().unwrap_or(json!({})),
        "cover_url": payload.get("poster_url"),
        "want_format": format.unwrap_or("ebook"),
    });
    let request = match serde_json::from_value(body) {
        Ok(r) => r,
        Err(e) => return ToolOutcome::failed(format!("could not build that request: {e}")),
    };
    let response = crate::routes::wanted::create_wanted(
        State(ctx.state.clone()),
        ctx.auth.clone(),
        JsonBody(request),
    )
    .await
    .into_response();
    let (status, resp) = body_of(response).await;
    if !(200..300).contains(&status) {
        return failed_call("adding that book", status, &resp);
    }
    let book_id = resp
        .get("id")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok());
    let Some(book_id) = book_id else {
        return ToolOutcome::failed("the book was added but I could not read its id back");
    };
    want_local_book(ctx, book_id, title, AcquireQuery::default()).await
}

async fn want_new_movie(
    ctx: &Ctx<'_>,
    p: &crate::chatbot::resolve::ProviderRef,
    payload: &Value,
    title: &str,
) -> ToolOutcome {
    let mut identifiers = payload
        .get("identifiers")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Some(map) = identifiers.as_object_mut() {
        map.entry(format!("{}_id", p.provider))
            .or_insert_with(|| json!(p.id));
    }
    let body = json!({
        "title": title,
        "year": payload.get("year"),
        "description": payload.get("description"),
        "poster_url": payload.get("poster_url"),
        "identifiers": identifiers,
        "genres": payload.get("genres"),
        "rating": payload.get("rating"),
        "rating_count": payload.get("rating_count"),
    });
    let request = match serde_json::from_value(body) {
        Ok(r) => r,
        Err(e) => return ToolOutcome::failed(format!("could not build that request: {e}")),
    };
    let response = crate::routes::admin::movies::create_movie(
        State(ctx.state.clone()),
        ctx.auth.clone(),
        JsonBody(request),
    )
    .await
    .into_response();
    let (status, resp) = body_of(response).await;
    if !(200..300).contains(&status) {
        return failed_call("adding that film", status, &resp);
    }
    let Some(movie_id) = resp
        .get("id")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
    else {
        return ToolOutcome::failed("the film was added but I could not read its id back");
    };
    let response = crate::routes::admin::movies::want_movie(
        State(ctx.state.clone()),
        ctx.auth.clone(),
        Path(movie_id),
        Query(AcquireQuery::default()),
    )
    .await
    .into_response();
    let (status, resp) = body_of(response).await;
    if !(200..300).contains(&status) {
        return failed_call("wanting that film", status, &resp);
    }
    let watched = register_watch(ctx, chat_watches::EntityKind::Movie, movie_id, title).await;
    done_one(title, movie_id, watched)
}

/// A show we do not have: one season or one episode of it, through the discover
/// handlers that create the show and sync the season as a side effect.
async fn want_discover(
    ctx: &Ctx<'_>,
    p: &crate::chatbot::resolve::ProviderRef,
    payload: &Value,
    title: &str,
    season: u32,
    episode: Option<u32>,
) -> ToolOutcome {
    let mut spec = json!({
        "provider": p.provider,
        "provider_id": p.id,
        "season_number": season,
        "title": title,
        "year": payload.get("year"),
        "identifiers": payload.get("identifiers"),
        "description": payload.get("description"),
        "poster_url": payload.get("poster_url"),
        "genres": payload.get("genres"),
        "rating": payload.get("rating"),
        "rating_count": payload.get("rating_count"),
        "network": payload.get("network"),
    });
    if let (Some(map), Some(n)) = (spec.as_object_mut(), episode) {
        map.insert("episode_number".to_string(), json!(n));
    }

    let response = match episode {
        Some(_) => {
            let request = match serde_json::from_value(spec) {
                Ok(r) => r,
                Err(e) => return ToolOutcome::failed(format!("could not build that request: {e}")),
            };
            crate::routes::admin::discover::want_discover_episode(
                State(ctx.state.clone()),
                ctx.auth.clone(),
                JsonBody(request),
            )
            .await
            .into_response()
        }
        None => {
            let request = match serde_json::from_value(spec) {
                Ok(r) => r,
                Err(e) => return ToolOutcome::failed(format!("could not build that request: {e}")),
            };
            crate::routes::admin::discover::want_discover_season(
                State(ctx.state.clone()),
                ctx.auth.clone(),
                JsonBody(request),
            )
            .await
            .into_response()
        }
    };
    let (status, resp) = body_of(response).await;
    if !(200..300).contains(&status) {
        return failed_call("getting that", status, &resp);
    }

    match episode {
        Some(_) => {
            let Some(id) = resp
                .get("episode_id")
                .and_then(Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok())
            else {
                return ToolOutcome::failed(
                    "that was queued but I could not read the episode back",
                );
            };
            let label = format!("S{season:02}E{:02} of {title}", episode.unwrap_or_default());
            let watched = register_watch(ctx, chat_watches::EntityKind::Episode, id, &label).await;
            done_one(&label, id, watched)
        }
        None => {
            let label = format!("season {season} of {title}");
            let ids: Vec<Uuid> = resp
                .get("newly_wanted_episodes")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|e| e.get("episode_id").and_then(Value::as_str))
                        .filter_map(|s| Uuid::parse_str(s).ok())
                        .collect()
                })
                .unwrap_or_default();
            let watched = watch_all(ctx, chat_watches::EntityKind::Episode, &ids, &label).await;
            let usize_of = |k: &str| resp.get(k).and_then(Value::as_u64).unwrap_or(0) as usize;
            match usize_of("newly_wanted") {
                0 => ToolOutcome::ok(
                    json!({
                        "ok": true,
                        "done": false,
                        "queued": false,
                        "target": label,
                        "total_episodes": usize_of("total_episodes"),
                        "already_have": usize_of("already_have"),
                        "already_wanted": usize_of("already_wanted"),
                        "newly_wanted": 0,
                        "say": already_wanted_sentence(
                            &label,
                            usize_of("already_have"),
                            usize_of("already_wanted"),
                        ),
                    }),
                    format!("{label}: nothing missing"),
                ),
                _ => season_outcome(
                    &label,
                    usize_of("total_episodes"),
                    usize_of("already_have"),
                    usize_of("already_wanted"),
                    &ids,
                    watched,
                ),
            }
        }
    }
}

/// Every season of a show we do not have: one discover call per season, the
/// counts folded into one sentence. Season 0 is specials, not a season — it
/// waits for someone to ask for it by name.
async fn want_all_seasons(
    ctx: &Ctx<'_>,
    p: &crate::chatbot::resolve::ProviderRef,
    payload: &Value,
    title: &str,
) -> ToolOutcome {
    let registry = crate::video_metadata::VideoProviderRegistry::new(ctx.state.pool.clone());
    let details = match registry.get_tv_show(&p.provider, &p.id).await {
        Ok(d) => d,
        Err(e) => {
            return ToolOutcome::failed(format!("could not read the seasons of {title}: {e}"));
        }
    };
    let mut seasons: Vec<u32> = details
        .seasons
        .iter()
        .map(|s| s.season_number)
        .filter(|n| *n >= 1)
        .collect();
    seasons.sort_unstable();
    seasons.dedup();
    if seasons.is_empty() {
        return ToolOutcome::failed(format!("the provider lists no seasons for {title} yet"));
    }

    let mut total = 0usize;
    let mut already_have = 0usize;
    let mut already_wanted = 0usize;
    let mut episode_ids: Vec<Uuid> = Vec::new();
    let mut failures = 0usize;
    for n in &seasons {
        let out = want_discover(ctx, p, payload, title, *n, None).await;
        if !out.ok {
            failures += 1;
            continue;
        }
        let obs = &out.observation;
        let usize_of = |k: &str| obs.get(k).and_then(Value::as_u64).unwrap_or(0) as usize;
        total += usize_of("total_episodes");
        already_have += usize_of("already_have");
        already_wanted += usize_of("already_wanted");
        if let Some(list) = obs.get("newly_wanted_episodes").and_then(Value::as_array) {
            for e in list {
                if let Some(id) = e.get("episode_id").and_then(Value::as_str)
                    && let Ok(id) = Uuid::parse_str(id)
                {
                    episode_ids.push(id);
                }
            }
        }
    }
    if episode_ids.is_empty() && failures == seasons.len() {
        return ToolOutcome::failed(format!("could not queue any season of {title}"));
    }
    let watched = watch_all(ctx, chat_watches::EntityKind::Episode, &episode_ids, title).await;
    let mut outcome = season_outcome(
        title,
        total,
        already_have,
        already_wanted,
        &episode_ids,
        watched,
    );
    if failures > 0 {
        // Say so rather than let the model believe every season went in.
        outcome.observation["note"] = json!(format!(
            "{failures} of the {} seasons could not be synced",
            seasons.len()
        ));
        outcome.detail = format!(
            "{title}: {} queued, {failures} seasons skipped",
            episode_ids.len()
        );
    }
    outcome
}

// =============================================================================
// list_releases / grab_release / regrab
// =============================================================================

/// The releases for one thing, pre-truncated. The model gets five at most.
///
/// Five rows off an unknown number of results is how "there aren't any 1080p
/// versions available" gets said about a search that had them at row nine, so
/// the counts and the tally of what resolutions exist are over *everything* the
/// search returned, not over the window.
pub async fn list_releases(ctx: &Ctx<'_>, args: &Value) -> ToolOutcome {
    let subject = match subject(ctx, args).await {
        Ok(s) => s,
        Err(outcome) => return outcome,
    };
    let wanted = arg_str(args, "resolution")
        .map(str::to_ascii_lowercase)
        .filter(|r| r != "any");
    match releases_for(ctx, &subject.media_ref).await {
        Err(outcome) => outcome,
        Ok(found) => {
            let shown = trim_releases(&found, wanted.as_deref());
            let available = resolutions_available(&found);
            let matching = match wanted.as_deref() {
                Some(want) => found
                    .iter()
                    .filter(|r| resolution_of(r).as_deref() == Some(want))
                    .count(),
                None => found.len(),
            };
            ToolOutcome::ok(
                json!({
                    "ok": true,
                    "target": subject.label,
                    "total_found": found.len(),
                    "matching": matching,
                    "showing": shown.len(),
                    "filtered_to": wanted,
                    "resolutions_available": available,
                    "releases": shown,
                    "note": "release_index is the number to pass to grab_release. \
                             Grabbing one costs disk, so confirm before you do. \
                             Only resolutions_available says what exists — the rows \
                             are the best few of total_found, so a resolution missing \
                             from them is not missing from the search.",
                }),
                match shown.len() {
                    0 => format!("no releases for {}", subject.label),
                    n => format!("{n} of {} releases for {}", found.len(), subject.label),
                },
            )
        }
    }
}

async fn releases_for(ctx: &Ctx<'_>, media_ref: &MediaRef) -> Result<Vec<Value>, ToolOutcome> {
    let Some(id) = media_ref.local_id() else {
        return Err(ToolOutcome::failed(
            "we do not have that yet, so there is nothing to list releases for — want_it adds it first",
        ));
    };
    let state = ctx.state.clone();
    let auth = ctx.auth.clone();
    let response = match media_ref.kind() {
        MediaKind::Book => crate::routes::wanted::search_releases(
            State(state),
            auth,
            Path(id),
            Query(serde_json::from_value(json!({})).expect("empty query")),
        )
        .await
        .into_response(),
        MediaKind::Movie => crate::routes::admin::movies::search_movie_releases(
            State(state),
            auth,
            Path(id),
            Query(serde_json::from_value(json!({})).expect("empty query")),
        )
        .await
        .into_response(),
        MediaKind::Episode | MediaKind::SportsEvent => match is_sports(ctx.state, id).await {
            true => crate::routes::admin::sports::search_event_releases(
                State(state),
                auth,
                Path(id),
                Query(serde_json::from_value(json!({})).expect("empty query")),
            )
            .await
            .into_response(),
            false => crate::routes::admin::tv::search_episode_releases(
                State(state),
                auth,
                Path(id),
                Query(serde_json::from_value(json!({})).expect("empty query")),
            )
            .await
            .into_response(),
        },
        kind => {
            return Err(ToolOutcome::failed(format!(
                "there is no release list for a {} — pick the episode or the film itself",
                kind.as_str()
            )));
        }
    };
    let (status, body) = body_of(response).await;
    if !(200..300).contains(&status) {
        return Err(failed_call("searching for releases", status, &body));
    }
    Ok(body
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// Kobload's `SearchResult` nests torrent stats, so the flat `seeders` the web
/// UI reads is only there for the sources that have one.
fn seeders_of(release: &Value) -> Option<i64> {
    release
        .get("torrent")
        .and_then(|t| t.get("seeders"))
        .or_else(|| release.get("seeders"))
        .and_then(Value::as_i64)
}

/// Best first, so the tally reads as a preference order.
const RESOLUTIONS: [(&str, &[&str]); 4] = [
    ("2160p", &["2160p", "4320p", "4k", "uhd"]),
    ("1080p", &["1080p", "1080i"]),
    ("720p", &["720p"]),
    ("480p", &["480p", "sd"]),
];

/// Kobload does not parse a quality out of a release, so the release title is
/// the only place a resolution exists. Separators vary by group, so the title is
/// flattened to spaces first rather than split on any one of them.
fn resolution_of(release: &Value) -> Option<String> {
    let title = release.get("title").and_then(Value::as_str)?.to_lowercase();
    let words: Vec<&str> = title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    RESOLUTIONS
        .iter()
        .find(|(_, tags)| tags.iter().any(|tag| words.contains(tag)))
        .map(|(name, _)| (*name).to_string())
}

/// Every resolution present anywhere in the search, best first.
fn resolutions_available(found: &[Value]) -> Vec<String> {
    let present: Vec<String> = found.iter().filter_map(resolution_of).collect();
    RESOLUTIONS
        .iter()
        .map(|(name, _)| (*name).to_string())
        .filter(|name| present.contains(name))
        .collect()
}

/// The five best, with an optional resolution required, and the original index
/// kept so `grab_release` and the model are talking about the same row.
fn trim_releases(found: &[Value], resolution: Option<&str>) -> Vec<Value> {
    found
        .iter()
        .enumerate()
        .filter(|(_, r)| match resolution {
            Some(want) => resolution_of(r).as_deref() == Some(want),
            None => true,
        })
        .take(MAX_RELEASES)
        .map(|(i, r)| {
            json!({
                "release_index": i,
                "title": r.get("title"),
                "size": r.get("size"),
                "seeders": seeders_of(r),
                "resolution": resolution_of(r),
                "protocol": r.get("protocol"),
                "already_downloaded": r.get("previously_downloaded").is_some(),
                "previously_rejected": r.get("rejected_release").is_some(),
            })
        })
        .collect()
}

/// `grab_release` — take one specific release off a list the model was shown.
pub async fn grab_release(ctx: &Ctx<'_>, args: &Value) -> ToolOutcome {
    let subject = match subject(ctx, args).await {
        Ok(s) => s,
        Err(outcome) => return outcome,
    };
    let args = gate(ctx, "download a copy of", &subject.label, args);
    // Read after the gate, not before: the index the offer was made about wins
    // over one the confirming call forgot to repeat.
    let index = arg_u64(&args, "release_index").unwrap_or(0) as usize;
    let found = match releases_for(ctx, &subject.media_ref).await {
        Ok(f) => f,
        Err(outcome) => return outcome,
    };
    let Some(release) = found.get(index) else {
        return ToolOutcome::failed(format!(
            "there is no release {index} for {} any more — list them again",
            subject.label
        ));
    };
    queue(ctx, &subject, release, "chat").await
}

/// `regrab` — "another copy", which almost always means the one on disk is broken.
///
/// It picks nothing. It blocklists the release already on disk so Kobload cannot
/// hand the same file back, then re-wants the thing with `manual` set, which is
/// what reopens something already imported. Kobload does the choosing, under the
/// quality profile — the interactive `list_releases` + `grab_release` pair is the
/// escape hatch for someone who wants one specific copy.
pub async fn regrab(ctx: &Ctx<'_>, args: &Value) -> ToolOutcome {
    let subject = match subject(ctx, args).await {
        Ok(s) => s,
        Err(outcome) => return outcome,
    };
    let Some(entity_id) = subject.media_ref.local_id() else {
        return ToolOutcome::failed(
            "we do not have that yet, so there is no copy to replace — want_it gets it in the first place",
        );
    };
    let kind = subject.media_ref.kind();
    if !matches!(
        kind,
        MediaKind::Movie | MediaKind::Episode | MediaKind::SportsEvent | MediaKind::Book
    ) {
        return ToolOutcome::failed(format!(
            "a {} is not one thing I can replace — pick the episode, the film or the book itself",
            kind.as_str()
        ));
    }

    let args = gate(ctx, "download another copy of", &subject.label, args);

    let blocked = blocklist_current_release(ctx.state, kind, entity_id).await;
    let additive = regrab_keeps_existing_file(kind);
    let mut outcome = perform_want(
        ctx,
        &subject,
        &args,
        AcquireQuery {
            additional_copy: additive,
            manual: true,
        },
    )
    .await;

    if outcome.ok
        && let Some(obj) = outcome.observation.as_object_mut()
    {
        obj.insert("keeps_existing_file".to_string(), json!(additive));
        obj.insert("blocklisted_current_release".to_string(), json!(blocked));
        obj.insert(
            "say".to_string(),
            json!(match additive {
                true => format!(
                    "Looking for another copy of {} — I've ruled out the one you have so it \
                     can't come back, and it picks from what the quality settings allow. The \
                     copy you have stays where it is; tidy it up in the web UI when you're happy.",
                    subject.label
                ),
                false => format!(
                    "Looking for another copy of {} — I've ruled out the one you have so it \
                     can't come back, and it picks from what the quality settings allow. It \
                     replaces the copy you have when it lands.",
                    subject.label
                ),
            }),
        );
        outcome.detail = format!("re-wanted {}", subject.label);
    }
    outcome
}

/// Rule out the copy on disk before re-wanting, so Kobload's own pick cannot be
/// the file they just told us is broken.
///
/// Same `rejected_releases` blocklist the web UI's replace-file button writes,
/// which is what `build_excluded_releases` reads on the way back out to Kobload.
/// A successful ingest clears it, so it does not outlive the problem.
async fn blocklist_current_release(state: &AppState, kind: MediaKind, id: Uuid) -> bool {
    let metadata: Vec<Value> = match kind {
        MediaKind::Episode | MediaKind::SportsEvent => {
            tv_episode_files::list_by_episode(&state.pool, id)
                .await
                .unwrap_or_default()
                .into_iter()
                .filter_map(|f| f.source_metadata)
                .collect()
        }
        MediaKind::Movie => movie_files::list_by_movie(&state.pool, id)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|f| f.source_metadata)
            .collect(),
        MediaKind::Book => crate::repositories::book_files::list_with_kobload_source(&state.pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|f| f.book_id == id)
            .map(|f| f.source_metadata)
            .collect(),
        _ => Vec::new(),
    };

    let mut blocked = false;
    for source_metadata in &metadata {
        let entry = crate::routes::admin::downloads::build_rejection_entry(
            Some(source_metadata),
            Some("chat_regrab"),
        );
        // A bare reason identifies nothing, so there is nothing to exclude on.
        if entry.as_object().is_some_and(|o| o.len() < 2) {
            continue;
        }
        let wrote = match kind {
            MediaKind::Episode | MediaKind::SportsEvent => {
                tv_episodes::add_rejected_release(&state.pool, id, entry).await
            }
            MediaKind::Movie => {
                crate::repositories::movies::add_rejected_release(&state.pool, id, entry).await
            }
            MediaKind::Book => {
                crate::repositories::books::add_rejected_release(&state.pool, id, entry).await
            }
            _ => Ok(false),
        };
        blocked |= wrote.unwrap_or(false);
    }
    blocked
}

/// Hand one release to Kobload through the same handler the web UI uses.
///
/// Only `grab_release` gets here — someone naming one specific copy — so the
/// file replaces what is on the entity. "Another copy, keep this one" is
/// `regrab`, which goes out through the want path instead.
async fn queue(ctx: &Ctx<'_>, subject: &Subject, release: &Value, source: &str) -> ToolOutcome {
    let Some(id) = subject.media_ref.local_id() else {
        return ToolOutcome::failed("that is not something I can download directly");
    };
    let payload = json!({
        "source": source,
        "release": release,
    });
    let state = ctx.state.clone();
    let auth = ctx.auth.clone();
    let response = match subject.media_ref.kind() {
        MediaKind::Book => {
            let request = match serde_json::from_value(payload) {
                Ok(r) => r,
                Err(e) => return ToolOutcome::failed(format!("that release is malformed: {e}")),
            };
            crate::routes::wanted::queue_book_release(
                State(state),
                auth,
                Path(id),
                JsonBody(request),
            )
            .await
            .into_response()
        }
        MediaKind::Movie => {
            let request = match serde_json::from_value(payload) {
                Ok(r) => r,
                Err(e) => return ToolOutcome::failed(format!("that release is malformed: {e}")),
            };
            crate::routes::admin::movies::queue_movie_release(
                State(state),
                auth,
                Path(id),
                JsonBody(request),
            )
            .await
            .into_response()
        }
        MediaKind::Episode | MediaKind::SportsEvent => match is_sports(ctx.state, id).await {
            true => {
                let request = match serde_json::from_value(payload) {
                    Ok(r) => r,
                    Err(e) => {
                        return ToolOutcome::failed(format!("that release is malformed: {e}"));
                    }
                };
                crate::routes::admin::sports::queue_event_release(
                    State(state),
                    auth,
                    Path(id),
                    JsonBody(request),
                )
                .await
                .into_response()
            }
            false => {
                let request = match serde_json::from_value(payload) {
                    Ok(r) => r,
                    Err(e) => {
                        return ToolOutcome::failed(format!("that release is malformed: {e}"));
                    }
                };
                crate::routes::admin::tv::queue_episode_release(
                    State(state),
                    auth,
                    Path(id),
                    JsonBody(request),
                )
                .await
                .into_response()
            }
        },
        kind => {
            return ToolOutcome::failed(format!(
                "a {} is not something I can download in one go",
                kind.as_str()
            ));
        }
    };
    let (status, body) = body_of(response).await;
    if !(200..300).contains(&status) {
        return failed_call("starting that download", status, &body);
    }
    let watched = register_watch(
        ctx,
        watch_kind(subject.media_ref.kind()),
        id,
        &subject.label,
    )
    .await;
    let name = release.get("title").and_then(Value::as_str).unwrap_or("it");
    ToolOutcome::ok(
        json!({
            "ok": true,
            "done": true,
            "target": subject.label,
            "entity_id": id,
            "release": name,
            "watching": watched,
            "say": format!("Downloading {name} for {} — I'll say when it's here.", subject.label),
        }),
        format!("grabbed {name}"),
    )
}

// =============================================================================
// cancel_download
// =============================================================================

/// Stopping is the safe direction, so it does not ask twice. "Stop" meaning stop
/// is the whole point of the confirmation sentence everywhere else.
pub async fn cancel_download(ctx: &Ctx<'_>, args: &Value) -> ToolOutcome {
    let subject = match subject(ctx, args).await {
        Ok(s) => s,
        Err(outcome) => return outcome,
    };
    let Some(id) = subject.media_ref.local_id() else {
        return ToolOutcome::failed("nothing is downloading for that");
    };
    let active = sqlx::query_scalar!(
        r#"
        SELECT external_id AS "external_id!"
        FROM wanted_attempts
        WHERE (book_id = $1 OR movie_id = $1 OR episode_id = $1)
          AND external_id IS NOT NULL
          AND status NOT IN ('completed', 'failed', 'cancelled')
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
        id
    )
    .fetch_optional(&ctx.state.pool)
    .await;

    let external_id = match active {
        Ok(Some(v)) => v,
        Ok(None) => {
            return ToolOutcome::ok(
                json!({
                    "ok": true,
                    "done": false,
                    "target": subject.label,
                    "say": format!("Nothing is downloading for {} right now.", subject.label),
                }),
                format!("nothing downloading for {}", subject.label),
            );
        }
        Err(e) => return ToolOutcome::failed(format!("could not check the downloads: {e}")),
    };
    let Ok(download_id) = Uuid::parse_str(&external_id) else {
        return ToolOutcome::failed("that download has no id I can cancel with");
    };

    let response = crate::routes::admin::cancel_download(
        State(ctx.state.clone()),
        ctx.auth.clone(),
        Path(download_id),
    )
    .await
    .into_response();
    let (status, body) = body_of(response).await;
    if !(200..300).contains(&status) {
        return failed_call("cancelling that", status, &body);
    }
    ToolOutcome::ok(
        json!({
            "ok": true,
            "done": true,
            "target": subject.label,
            "say": format!("Stopped the download for {}. The files already on disk are untouched.", subject.label),
        }),
        format!("cancelled {}", subject.label),
    )
}

// =============================================================================
// add_to_watchlist
// =============================================================================

/// The safest verb there is: costs no disk, cannot go wrong, and is the natural
/// answer when something is not out yet.
pub async fn add_to_watchlist(ctx: &Ctx<'_>, args: &Value) -> ToolOutcome {
    let subject = match subject(ctx, args).await {
        Ok(s) => s,
        Err(outcome) => return outcome,
    };
    let (media_type, id) = match (&subject.media_ref, subject.media_ref.local_id()) {
        (_, None) => {
            return ToolOutcome::failed(
                "we do not have that yet, so it cannot go on the list — offer to get it instead",
            );
        }
        (r, Some(id)) => match r.kind() {
            MediaKind::Show => (watchlist::SHOW, id),
            MediaKind::Movie => (watchlist::MOVIE, id),
            MediaKind::Season | MediaKind::Episode => match show_of_episode(ctx.state, r).await {
                Some(show_id) => (watchlist::SHOW, show_id),
                None => return ToolOutcome::failed("I could not work out which show that is"),
            },
            kind => {
                return ToolOutcome::failed(format!(
                    "the watch list is for films and shows — a {} does not go on it",
                    kind.as_str()
                ));
            }
        },
    };

    if let Err(e) = watchlist::add(&ctx.state.pool, ctx.auth.user_id, media_type, id).await {
        return ToolOutcome::failed(format!("could not add that to your list: {e}"));
    }
    crate::routes::library::helpers::record_activity_for_entity(
        &ctx.state.pool,
        "watchlist_added",
        id,
        Some(ctx.auth.user_id),
    )
    .await;
    ToolOutcome::ok(
        json!({
            "ok": true,
            "done": true,
            "target": subject.label,
            "entity_id": id,
            "say": format!("Added {} to your list — it'll show up in Up Next.", subject.label),
        }),
        format!("watchlisted {}", subject.label),
    )
}

async fn show_of_episode(state: &AppState, media_ref: &MediaRef) -> Option<Uuid> {
    let id = media_ref.local_id()?;
    match media_ref.kind() {
        MediaKind::Season => tv_seasons::get(&state.pool, id)
            .await
            .ok()?
            .map(|s| s.show_id),
        _ => {
            let episode = tv_episodes::get(&state.pool, id).await.ok()??;
            tv_seasons::get(&state.pool, episode.season_id)
                .await
                .ok()?
                .map(|s| s.show_id)
        }
    }
}

// =============================================================================
// set_notification_prefs
// =============================================================================

/// The one act tool a `viewer` may call: muting yourself needs no privilege.
///
/// The wording, the relative durations and the quiet-hours arithmetic all live in
/// [`crate::notifications::prefs`], which is also what reads the row back when a
/// notification is about to fire. Two implementations of "is she muted" is how
/// the bot ends up talking at 2am.
pub async fn set_notification_prefs(ctx: &Ctx<'_>, args: &Value) -> ToolOutcome {
    use crate::notifications::prefs;

    let request: prefs::SetNotificationPrefs = match serde_json::from_value(args.clone()) {
        Ok(r) => r,
        Err(e) => {
            return ToolOutcome::failed(format!(
                "I did not follow that — {e}. It is mute, resume, enable_trigger, disable_trigger, set_quiet_hours, clear_quiet_hours or status."
            ));
        }
    };

    match prefs::apply(&ctx.state.pool, ctx.auth.user_id, &request).await {
        Ok(update) => ToolOutcome::ok(
            json!({
                "ok": true,
                "done": true,
                "enabled": update.prefs.enabled,
                "muted_until": update.prefs.muted_until.map(|t| t.unix_timestamp()),
                "triggers": update.prefs.triggers,
                "say": &update.summary,
            }),
            update.summary,
        ),
        Err(e) => ToolOutcome::failed(format!("could not change that: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A whole provider show cannot be queued, so it is a question — while the
    /// all-seasons ref and local shows are executable, so they are not.
    #[test]
    fn a_whole_provider_show_is_a_question() {
        let subject = Subject {
            media_ref: "provider:tmdb:95396:show".parse().unwrap(),
            label: "Deadloch".to_string(),
            candidate: None,
        };
        let q = provider_show_question(&subject).unwrap();
        assert!(q.ok);
        assert_eq!(q.observation["needs_answer"], json!(true));
        // No token is minted: there is nothing a confirm could spend.
        assert!(q.observation.get("confirm_token").is_none());
        assert!(
            q.observation["note"]
                .as_str()
                .unwrap()
                .contains("all seasons")
        );

        let all = Subject {
            media_ref: "provider:tmdb:95396:all".parse().unwrap(),
            label: "all of Deadloch".to_string(),
            candidate: None,
        };
        assert!(provider_show_question(&all).is_none());

        let local = Subject {
            media_ref: "show:3f2504e0-4f89-41d3-9a0c-0305e82c3301".parse().unwrap(),
            label: "Deadloch".to_string(),
            candidate: None,
        };
        assert!(provider_show_question(&local).is_none());
    }

    #[test]
    fn a_season_that_is_mostly_here_says_so_rather_than_re_queuing() {
        let ids = vec![Uuid::now_v7(), Uuid::now_v7()];
        let outcome = season_outcome("season 24 of someShow", 24, 22, 0, &ids, 2);
        let say = outcome.observation["say"].as_str().unwrap();
        assert!(say.contains("already got 22"), "{say}");
        assert!(say.contains("other 2"), "{say}");
        assert_eq!(outcome.observation["newly_wanted"], json!(2));

        let nothing = already_wanted_sentence("season 24 of someShow", 24, 0);
        assert!(nothing.contains("already got all 24"), "{nothing}");
    }

    #[test]
    fn a_release_list_is_trimmed_before_the_model_sees_it() {
        let found: Vec<Value> = (0..12)
            .map(|i| json!({"title": format!("r{i}"), "info_hash": format!("HASH{i}")}))
            .collect();
        let all = trim_releases(&found, None);
        assert_eq!(all.len(), MAX_RELEASES);
        assert_eq!(all[0]["release_index"], json!(0));
    }

    /// Kobload's search results have no `quality` field and keep seeders under
    /// `torrent`, so both used to reach the model as `null` on every row. All it
    /// had left was the release title.
    #[test]
    fn a_release_row_carries_the_two_things_a_copy_is_chosen_on() {
        let row = &trim_releases(
            &[json!({
                "title": "Some.Show.S04E04.2160p.ATVP.WEB-DL.DDP5.1.Atmos.DV.HDR.H.265-FLUX",
                "torrent": {"seeders": 42},
            })],
            None,
        )[0];
        assert_eq!(row["resolution"], json!("2160p"));
        assert_eq!(row["seeders"], json!(42));
    }

    #[test]
    fn resolutions_come_off_the_release_title() {
        for (title, expected) in [
            ("Some.Show.S04E04.2160p.WEB-DL-FLUX", Some("2160p")),
            ("Some.Show.S04E04.1080p.WEB-DL-NTb", Some("1080p")),
            ("Some.Show.S04E04.720p.HEVC.x265-MeGusta", Some("720p")),
            ("Some Show S04E04 4K UHD REMUX", Some("2160p")),
            ("Some.Show.S04E04.WEBRip.x264", None),
        ] {
            assert_eq!(
                resolution_of(&json!({"title": title})),
                expected.map(str::to_string),
                "{title}"
            );
        }
    }

    /// "There aren't any 1080p versions available" was said about a list of five
    /// rows off an unknown number of results. The tally is over everything the
    /// search returned, so absence is a fact rather than a guess.
    #[test]
    fn the_tally_is_over_every_result_not_the_five_that_are_shown() {
        let found: Vec<Value> = (0..12)
            .map(|i| {
                let res = if i == 11 { "1080p" } else { "2160p" };
                json!({"title": format!("Some.Show.S04E04.{res}.WEB-DL-{i}")})
            })
            .collect();
        assert_eq!(
            resolutions_available(&found),
            vec!["2160p".to_string(), "1080p".to_string()]
        );

        let shown = trim_releases(&found, Some("1080p"));
        assert_eq!(shown.len(), 1);
        assert_eq!(
            shown[0]["release_index"],
            json!(11),
            "a filtered row still indexes into the unfiltered search"
        );
    }

    /// Video carries `additional_copy` all the way to the link step; books have
    /// no such flag and overwrite on `(book_id, format)`. Promising a book's old
    /// file survived is the one lie that would cost her a file.
    #[test]
    fn regrab_only_promises_the_old_file_where_the_flag_actually_reaches() {
        for kind in [MediaKind::Movie, MediaKind::Episode, MediaKind::SportsEvent] {
            assert!(regrab_keeps_existing_file(kind), "{}", kind.as_str());
        }
        for kind in [
            MediaKind::Book,
            MediaKind::Show,
            MediaKind::Season,
            MediaKind::Author,
            MediaKind::Series,
        ] {
            assert!(!regrab_keeps_existing_file(kind), "{}", kind.as_str());
        }
    }
}
