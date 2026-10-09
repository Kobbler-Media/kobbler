//! Watches that turn a download into a running commentary in a chat thread.
//!
//! A row is a promise to say something terminal about one entity in one conversation.
//! `expires_at` is what keeps that promise bounded: a stuck download gets one apology,
//! not a message next March.

use super::Result;
use crate::chatbot::transport::Target;
use sqlx::{Executor, Postgres};
use std::time::Duration;
use time::OffsetDateTime;
use uuid::Uuid;

pub const DEFAULT_TTL: Duration = Duration::from_secs(12 * 3600);

/// The `last_state` values that mean "this watch has a verdict"; a row carrying one
/// is finished and is only still here because the news has not landed yet.
pub const TERMINAL_STATES: &[&str] = &["done", "failed", "needs_review", "cancelled", "expired"];

pub fn is_terminal_state(state: Option<&str>) -> bool {
    state.is_some_and(|s| TERMINAL_STATES.contains(&s))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    Episode,
    Movie,
    Book,
    Wanted,
}

impl EntityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Episode => "episode",
            Self::Movie => "movie",
            Self::Book => "book",
            Self::Wanted => "wanted",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "episode" => Some(Self::Episode),
            "movie" => Some(Self::Movie),
            "book" => Some(Self::Book),
            "wanted" => Some(Self::Wanted),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatWatch {
    pub id: Uuid,
    pub transport: String,
    pub target: Target,
    pub message_id: Option<String>,
    pub entity_kind: EntityKind,
    pub entity_id: Uuid,
    pub label: String,
    pub user_id: Option<Uuid>,
    pub last_state: Option<String>,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

/// Registration input. `transport` is deliberately absent — it comes from
/// [`Target::transport`], so the two cannot disagree.
#[derive(Debug, Clone)]
pub struct NewWatch<'a> {
    pub target: &'a Target,
    /// The message the act tool already posted, if any. `None` means the first
    /// update posts instead of editing.
    pub message_id: Option<&'a str>,
    pub entity_kind: EntityKind,
    pub entity_id: Uuid,
    /// What to call the thing in every message about it.
    pub label: &'a str,
    /// Who asked, for attribution and notification preferences.
    pub user_id: Option<Uuid>,
    pub ttl: Duration,
}

struct Row {
    id: Uuid,
    transport: String,
    target: serde_json::Value,
    message_id: Option<String>,
    entity_kind: String,
    entity_id: Uuid,
    label: String,
    user_id: Option<Uuid>,
    last_state: Option<String>,
    created_at: OffsetDateTime,
    expires_at: OffsetDateTime,
}

impl Row {
    fn into_watch(self) -> Option<ChatWatch> {
        let target: Target = serde_json::from_value(self.target).ok()?;
        Some(ChatWatch {
            id: self.id,
            transport: self.transport,
            target,
            message_id: self.message_id,
            entity_kind: EntityKind::parse(&self.entity_kind)?,
            entity_id: self.entity_id,
            label: self.label,
            user_id: self.user_id,
            last_state: self.last_state,
            created_at: self.created_at,
            expires_at: self.expires_at,
        })
    }
}

/// Register (or refresh) a watch. Re-asking in the same thread updates the existing
/// row rather than making one download narrate itself twice.
pub async fn watch<'e, E>(exec: E, input: NewWatch<'_>) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let target = serde_json::to_value(input.target)
        .map_err(|e| super::RepositoryError::Validation(format!("bad chat target: {e}")))?;
    let ttl_secs = input.ttl.as_secs_f64().max(1.0);

    let created: Uuid = sqlx::query_scalar!(
        r#"
        INSERT INTO chat_watches (
            id, transport, target, message_id, entity_kind, entity_id, label, user_id, expires_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW() + ($9::double precision * INTERVAL '1 second'))
        ON CONFLICT (transport, (target ->> 'space'), COALESCE(target ->> 'thread', ''), entity_id)
        DO UPDATE SET
            message_id = COALESCE(EXCLUDED.message_id, chat_watches.message_id),
            label = EXCLUDED.label,
            entity_kind = EXCLUDED.entity_kind,
            user_id = COALESCE(EXCLUDED.user_id, chat_watches.user_id),
            expires_at = GREATEST(chat_watches.expires_at, EXCLUDED.expires_at),
            last_state = NULL,
            last_edit_at = NULL
        RETURNING id
        "#,
        id,
        input.target.transport,
        target,
        input.message_id,
        input.entity_kind.as_str(),
        input.entity_id,
        input.label,
        input.user_id,
        ttl_secs,
    )
    .fetch_one(exec)
    .await?;

    Ok(created)
}

/// Live watches for any of these entities. Expired rows are invisible here — the
/// sweep is what says goodbye to them.
pub async fn for_entities<'e, E>(exec: E, entity_ids: &[Uuid]) -> Result<Vec<ChatWatch>>
where
    E: Executor<'e, Database = Postgres>,
{
    if entity_ids.is_empty() {
        return Ok(Vec::new());
    }

    let rows = sqlx::query_as!(
        Row,
        r#"
        SELECT id, transport, target, message_id, entity_kind, entity_id, label,
               user_id, last_state, created_at, expires_at
        FROM chat_watches
        WHERE entity_id = ANY($1) AND expires_at > NOW()
        ORDER BY created_at, id
        "#,
        entity_ids,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows.into_iter().filter_map(Row::into_watch).collect())
}

/// Live watches on whatever a Kobload download is acquiring, resolved through the
/// attempt that grabbed it. One query: Kobload progress fires often enough that a
/// two-step lookup would double the traffic for events nobody is waiting on.
pub async fn for_download<'e, E>(exec: E, external_id: &str) -> Result<Vec<ChatWatch>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        Row,
        r#"
        SELECT id, transport, target, message_id, entity_kind, entity_id, label,
               user_id, last_state, created_at, expires_at
        FROM chat_watches
        WHERE expires_at > NOW()
          AND entity_id IN (
              SELECT COALESCE(book_id, movie_id, episode_id)
              FROM wanted_attempts
              WHERE external_id = $1
          )
        ORDER BY created_at, id
        "#,
        external_id,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows.into_iter().filter_map(Row::into_watch).collect())
}

/// What a claimed coalescing slot carries: the message this request is already
/// showing in the thread, if a sibling has posted one.
#[derive(Debug, Clone, Default)]
pub struct EditSlot {
    pub message_id: Option<String>,
}

/// Take the coalescing slot for this watch *and everything else in the thread
/// waiting on the same request*. `None` means someone spoke for it inside the
/// window and this update should be dropped.
///
/// The group is [`claim_terminal_group`]'s: same transport, same thread, same
/// label. It has to be, because the wall of identical messages a season used to
/// end with is the same wall it used to *start* with — thirteen episodes, each
/// its own row with its own `message_id` of `NULL`, every one of them posting
/// "Searching for season 11 of Law & Order…" the moment the pack was grabbed.
/// Claiming per row only spaced them out.
///
/// The slot hands back the group's message id so the winner edits what the
/// thread is already showing instead of posting beside it.
pub async fn claim_edit_group<'e, E>(
    exec: E,
    id: Uuid,
    min_interval: Duration,
) -> Result<Option<EditSlot>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        WITH mine AS (
            SELECT transport, target, label FROM chat_watches WHERE id = $1
        ),
        family AS (
            SELECT w.id, w.message_id, w.last_edit_at
            FROM chat_watches w, mine
            WHERE w.transport = mine.transport
              AND w.target = mine.target
              AND w.label = mine.label
              AND w.expires_at > NOW()
        ),
        claimed AS (
            UPDATE chat_watches
            SET last_edit_at = NOW()
            WHERE id IN (SELECT id FROM family)
              AND EXISTS (SELECT 1 FROM family WHERE id = $1)
              AND NOT EXISTS (
                  SELECT 1 FROM family
                  WHERE last_edit_at > NOW() - ($2::double precision * INTERVAL '1 second')
              )
            RETURNING id
        )
        SELECT
            (SELECT COUNT(*) FROM claimed) AS "claimed!",
            (SELECT message_id FROM family WHERE message_id IS NOT NULL ORDER BY id LIMIT 1)
                AS "message_id?"
        "#,
        id,
        min_interval.as_secs_f64(),
    )
    .fetch_one(exec)
    .await?;

    Ok(match row.claimed > 0 {
        true => Some(EditSlot {
            message_id: row.message_id,
        }),
        false => None,
    })
}

/// Record the state the thread has actually been told about, and the message id if
/// this update had to post a new one.
///
/// Written across the whole group, for the same reason the slot is claimed across
/// it: the siblings are watching one message, so they have to know both what it
/// says and where it is. A row already carrying a verdict is left alone — that is
/// news the sweep still owes someone, and progress must not overwrite it.
pub async fn record_state_group<'e, E>(
    exec: E,
    id: Uuid,
    state: &str,
    message_id: Option<&str>,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        WITH mine AS (
            SELECT transport, target, label FROM chat_watches WHERE id = $1
        )
        UPDATE chat_watches w
        SET last_state = $2,
            message_id = COALESCE($3, w.message_id)
        FROM mine
        WHERE w.transport = mine.transport
          AND w.target = mine.target
          AND w.label = mine.label
          AND (w.last_state IS NULL OR w.last_state <> ALL($4))
        "#,
        id,
        state,
        message_id,
        TERMINAL_STATES as &[&str],
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Delete a watch. Called once its final message has actually been delivered.
pub async fn take<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let taken = sqlx::query_scalar!("DELETE FROM chat_watches WHERE id = $1 RETURNING id", id)
        .fetch_optional(exec)
        .await?;
    Ok(taken.is_some())
}

/// Claim a verdict for one watch *and everything else in the thread waiting on the
/// same request*.
///
/// A season want registers a watch per episode, all carrying the season's label, so
/// a season that fails or is cancelled flips every one of them at once and each row
/// rendered the same sentence about the same season. Thirteen episodes, thirteen
/// identical messages.
///
/// The label is what makes them one request: same transport, same thread, same
/// words. The winner speaks and its siblings are deleted rather than marked — a row
/// left carrying a verdict is a row the retry sweep would deliver later.
///
/// The winner is claimed rather than deleted, because a missing terminal message is
/// indistinguishable from a broken bot: the row has to outlive the send attempt and
/// be retried. `last_state` records the verdict so a retry knows what to say, the
/// claim is conditional on the row not already carrying one — that is what stops a
/// duplicate completion event narrating twice — and `expires_at` bounds the retrying.
///
/// Two siblings arriving at a verdict at genuinely the same instant can still each
/// claim their own row before either delete lands, and both will speak. That is two
/// messages rather than thirteen, and the events come off one subscriber in order,
/// so it is a bound rather than a behaviour.
pub async fn claim_terminal_group<'e, E>(
    exec: E,
    id: Uuid,
    state: &str,
    retry_window: Duration,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        WITH mine AS (
            SELECT transport, target, label FROM chat_watches WHERE id = $1
        ),
        claimed AS (
            UPDATE chat_watches
            SET last_state = $2,
                last_edit_at = NOW(),
                expires_at = NOW() + ($3::double precision * INTERVAL '1 second')
            WHERE id = $1
              AND (last_state IS NULL OR last_state <> ALL($4))
            RETURNING id
        ),
        hushed AS (
            DELETE FROM chat_watches w
            USING mine
            WHERE w.id <> $1
              AND EXISTS (SELECT 1 FROM claimed)
              AND w.transport = mine.transport
              AND w.target = mine.target
              AND w.label = mine.label
              AND (w.last_state IS NULL OR w.last_state <> ALL($4))
            RETURNING w.id
        )
        SELECT
            (SELECT COUNT(*) FROM claimed) AS "claimed!",
            (SELECT COUNT(*) FROM hushed) AS "hushed!"
        "#,
        id,
        state,
        retry_window.as_secs_f64(),
        TERMINAL_STATES as &[&str],
    )
    .fetch_one(exec)
    .await?;

    Ok(row.claimed > 0)
}

/// Expired watches that never reached a verdict, claimed as `expired` so each one
/// can be apologised for. This is what stops a stuck download messaging next March.
///
/// One apology per request, not per row: a season's episodes are one download and
/// one message, and they expire together. The siblings are deleted rather than
/// left behind, because a row carrying a verdict nobody has heard is a row the
/// retry sweep would deliver later — the same rule [`claim_terminal_group`] keeps.
pub async fn claim_expired<'e, E>(
    exec: E,
    limit: i64,
    retry_window: Duration,
) -> Result<Vec<ChatWatch>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        Row,
        r#"
        WITH due AS (
            SELECT id, transport, target, label FROM chat_watches
            WHERE expires_at <= NOW()
              AND (last_state IS NULL OR last_state <> ALL($3))
            ORDER BY expires_at
            LIMIT $1
        ),
        speakers AS (
            SELECT DISTINCT ON (transport, target, label) id
            FROM due
            ORDER BY transport, target, label, id
        ),
        hushed AS (
            DELETE FROM chat_watches
            WHERE id IN (SELECT id FROM due)
              AND id NOT IN (SELECT id FROM speakers)
            RETURNING id
        )
        UPDATE chat_watches
        SET last_state = 'expired',
            last_edit_at = NOW(),
            expires_at = NOW() + ($2::double precision * INTERVAL '1 second')
        WHERE id IN (SELECT id FROM speakers)
        RETURNING id, transport, target, message_id, entity_kind, entity_id, label,
                  user_id, last_state, created_at, expires_at
        "#,
        limit,
        retry_window.as_secs_f64(),
        TERMINAL_STATES as &[&str],
    )
    .fetch_all(exec)
    .await?;

    Ok(rows.into_iter().filter_map(Row::into_watch).collect())
}

/// Watches whose verdict is decided but undelivered, due another attempt.
///
/// Taking the slot up front means a sweep that overlaps the previous one cannot
/// double-send, and it is the same conditional-`UPDATE` trick as
/// [`claim_edit_group`].
pub async fn claim_terminal_retries<'e, E>(
    exec: E,
    limit: i64,
    backoff: Duration,
) -> Result<Vec<ChatWatch>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        Row,
        r#"
        UPDATE chat_watches
        SET last_edit_at = NOW()
        WHERE id IN (
            SELECT id FROM chat_watches
            WHERE last_state = ANY($3)
              AND expires_at > NOW()
              AND (
                  last_edit_at IS NULL
                  OR last_edit_at <= NOW() - ($2::double precision * INTERVAL '1 second')
              )
            ORDER BY last_edit_at
            LIMIT $1
        )
        RETURNING id, transport, target, message_id, entity_kind, entity_id, label,
                  user_id, last_state, created_at, expires_at
        "#,
        limit,
        backoff.as_secs_f64(),
        TERMINAL_STATES as &[&str],
    )
    .fetch_all(exec)
    .await?;

    Ok(rows.into_iter().filter_map(Row::into_watch).collect())
}

/// Verdicts that outlived their retry window. Nothing more will be said about these —
/// the caller logs them, because a dropped terminal message is worth knowing about.
pub async fn abandon_undelivered<'e, E>(exec: E, limit: i64) -> Result<Vec<ChatWatch>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query_as!(
        Row,
        r#"
        DELETE FROM chat_watches
        WHERE id IN (
            SELECT id FROM chat_watches
            WHERE last_state = ANY($2) AND expires_at <= NOW()
            ORDER BY expires_at
            LIMIT $1
        )
        RETURNING id, transport, target, message_id, entity_kind, entity_id, label,
                  user_id, last_state, created_at, expires_at
        "#,
        limit,
        TERMINAL_STATES as &[&str],
    )
    .fetch_all(exec)
    .await?;

    Ok(rows.into_iter().filter_map(Row::into_watch).collect())
}
