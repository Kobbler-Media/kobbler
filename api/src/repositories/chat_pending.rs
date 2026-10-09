//! Pending write confirmations, stored on `chat_messages.steps`.
//!
//! A confirmation is conversation state, not a resource, so it lives on the turn
//! that offered it rather than in a table of its own: the act tool returns a
//! `confirm_token` in its observation, the loop persists that observation as a
//! step, and the follow-up call finds it here.
//!
//! Two properties this file exists to guarantee:
//!
//! * **once.** [`consume`] flips `confirmed` inside the stored step in the same
//!   statement that reads it, so a second call with the same token finds nothing.
//!   Two calls in one round are sequential and each commits before the next
//!   reads, which is the only concurrency this ever sees.
//! * **soon.** Only steps written inside `max_age` are visible, so "yes"
//!   tomorrow cannot grab what was offered last night.

use serde_json::Value;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, sqlx::Error>;

/// Take a pending confirmation, returning the observation that offered it.
///
/// `None` means: no such token in this conversation, it was already used, or it
/// is older than `max_age`. All three are the same answer to the caller — do not
/// act.
pub async fn consume(
    db: &PgPool,
    session_id: Uuid,
    token: &str,
    max_age: Duration,
) -> Result<Option<Value>> {
    let cutoff = OffsetDateTime::now_utc() - max_age;
    let row = sqlx::query_scalar!(
        r#"
        WITH found AS (
            SELECT m.id AS message_id, s.ord, s.val
            FROM chat_messages m
            CROSS JOIN LATERAL jsonb_array_elements(m.steps) WITH ORDINALITY AS s(val, ord)
            WHERE m.session_id = $1
              AND m.created_at > $3
              AND s.val -> 'observation' ->> 'confirm_token' = $2
              AND COALESCE(s.val -> 'observation' ->> 'confirmed', 'false') = 'false'
            ORDER BY m.created_at DESC, s.ord DESC
            LIMIT 1
        )
        UPDATE chat_messages m
        SET steps = jsonb_set(
                m.steps,
                ARRAY[(found.ord - 1)::text, 'observation', 'confirmed'],
                'true'::jsonb
            )
        FROM found
        WHERE m.id = found.message_id
        RETURNING (found.val -> 'observation') AS "observation!"
        "#,
        session_id,
        token,
        cutoff,
    )
    .fetch_optional(db)
    .await?;

    Ok(row)
}

/// The candidate a `resolve_media` in this conversation stamped with `media_ref`.
///
/// This is how an acquisition tool gets the provider payload for something not in
/// the library: the resolver already fetched title, year and identifiers, and the
/// loop persisted them on the step. Asking the model to carry that blob back would
/// mean trusting it to retype metadata, which is exactly how a row called "that
/// show" gets created.
pub async fn candidate(
    db: &PgPool,
    session_id: Uuid,
    media_ref: &str,
    max_age: Duration,
) -> Result<Option<Value>> {
    let cutoff = OffsetDateTime::now_utc() - max_age;
    let found = sqlx::query_scalar!(
        r#"
        SELECT c.val AS "candidate!"
        FROM chat_messages m
        CROSS JOIN LATERAL jsonb_array_elements(m.steps) AS s(val)
        CROSS JOIN LATERAL jsonb_array_elements(
            COALESCE(s.val -> 'observation' -> 'candidates', '[]'::jsonb)
        ) AS c(val)
        WHERE m.session_id = $1
          AND m.created_at > $3
          AND c.val ->> 'ref' = $2
        ORDER BY m.created_at DESC
        LIMIT 1
        "#,
        session_id,
        media_ref,
        cutoff,
    )
    .fetch_optional(db)
    .await?;

    Ok(found)
}

/// Where a token stands that [`consume`] just refused. `None` — the conversation
/// never issued it (the model invented it); `Some(true)` — it was issued and
/// spent; `Some(false)` — it was issued but has fallen out of the window.
///
/// Runtime query on purpose: this is a diagnostic sibling of the macro-checked
/// [`consume`], and wording churn here must not force a `.sqlx` re-prepare.
pub async fn refused_reason(db: &PgPool, session_id: Uuid, token: &str) -> Result<Option<bool>> {
    let row: Option<bool> = sqlx::query_scalar(
        r#"
        SELECT COALESCE(s.val -> 'observation' ->> 'confirmed', 'false') = 'true'
        FROM chat_messages m
        CROSS JOIN LATERAL jsonb_array_elements(m.steps) AS s(val)
        WHERE m.session_id = $1
          AND s.val -> 'observation' ->> 'confirm_token' = $2
        ORDER BY m.created_at DESC
        LIMIT 1
        "#,
    )
    .bind(session_id)
    .bind(token)
    .fetch_optional(db)
    .await?;
    Ok(row)
}
