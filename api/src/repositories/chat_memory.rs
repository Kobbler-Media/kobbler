//! Tier 2 of the conversation store, plus the retention and forget-everything
//! sweeps that cut across all three tiers.
//!
//! The privacy rule is the same one `chat_turn_logs` enforces: what was said
//! belongs to the person who said it. [`summaries_for_user`] takes the owner's
//! id, and [`user_metadata`] — the admin view — selects counts and timestamps
//! only. No caller can ask this module for another person's prose.

use super::Result;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

/// Raw turns are kept a month; the ~150-word summary that replaces them keeps a
/// year. Facts have no expiry — they go when their owner removes them.
pub const MESSAGE_RETENTION_DAYS: i32 = 30;
pub const SUMMARY_RETENTION_DAYS: i32 = 365;

/// A summary older than this is not context, it is noise.
pub const CARRY_FORWARD_DAYS: i64 = 14;

#[derive(Debug, Clone)]
pub struct IdleSession {
    pub id: Uuid,
    pub transport: String,
    pub user_id: Option<Uuid>,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: Uuid,
    pub transport: String,
    pub summary: String,
    pub created_at: OffsetDateTime,
    pub closed_at: Option<OffsetDateTime>,
}

/// What an admin may see about someone else's use of the bot: how much, how
/// recently, and nothing about what.
#[derive(Debug, Clone)]
pub struct UserChatMetadata {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub session_count: i64,
    pub message_count: i64,
    pub summary_count: i64,
    pub fact_count: i64,
    pub last_seen: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Forgotten {
    pub sessions: u64,
    pub messages: u64,
    pub facts: u64,
    pub turn_logs: u64,
}

/// Open sessions with nothing said for `idle_minutes`. Ordered oldest first so
/// a backlog drains in the order it went quiet.
pub async fn idle_sessions(
    pool: &PgPool,
    idle_minutes: i32,
    limit: i64,
) -> Result<Vec<IdleSession>> {
    let rows = sqlx::query_as!(
        IdleSession,
        r#"
        SELECT id, transport, user_id, updated_at
        FROM chat_sessions
        WHERE closed_at IS NULL
          AND updated_at < NOW() - make_interval(mins => $1)
        ORDER BY updated_at ASC
        LIMIT $2
        "#,
        idle_minutes,
        limit,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Close a session, with or without a summary. A session not worth a GPU call
/// closes with `None` and is simply gone from the open set.
pub async fn close_with_summary(
    pool: &PgPool,
    session_id: Uuid,
    summary: Option<&str>,
) -> Result<()> {
    sqlx::query!(
        "UPDATE chat_sessions SET closed_at = NOW(), summary = $2 WHERE id = $1",
        session_id,
        summary,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// The one line the next conversation in this space inherits.
///
/// Scoped to the session's own address: an addressed transport carries forward
/// within its space and thread, and a web session carries forward within its
/// user. Recency-bounded so "did that ever finish?" works the next morning and
/// stops being asked of something from last spring.
pub async fn carry_forward_summary(pool: &PgPool, session_id: Uuid) -> Result<Option<String>> {
    let summary = sqlx::query_scalar!(
        r#"
        SELECT prev.summary
        FROM chat_sessions cur
        JOIN chat_sessions prev
          ON prev.id <> cur.id
         AND prev.transport = cur.transport
         AND prev.summary IS NOT NULL
         AND (
              (cur.space IS NOT NULL
                 AND prev.space = cur.space
                 AND prev.thread IS NOT DISTINCT FROM cur.thread)
              OR (cur.space IS NULL
                 AND cur.user_id IS NOT NULL
                 AND prev.user_id = cur.user_id)
             )
        WHERE cur.id = $1
          AND COALESCE(prev.closed_at, prev.updated_at) > NOW() - make_interval(days => $2)
        ORDER BY COALESCE(prev.closed_at, prev.updated_at) DESC
        LIMIT 1
        "#,
        session_id,
        CARRY_FORWARD_DAYS as i32,
    )
    .fetch_optional(pool)
    .await?
    .flatten();
    Ok(summary)
}

/// Session summaries belonging to one person, newest first.
pub async fn summaries_for_user(
    pool: &PgPool,
    owner_id: Uuid,
    limit: i64,
) -> Result<Vec<SessionSummary>> {
    let rows = sqlx::query_as!(
        SessionSummary,
        r#"
        SELECT id, transport, summary as "summary!", created_at, closed_at
        FROM chat_sessions
        WHERE user_id = $1 AND summary IS NOT NULL
        ORDER BY COALESCE(closed_at, updated_at) DESC
        LIMIT $2
        "#,
        owner_id,
        limit,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Counts and timestamps per user. Deliberately selects no `summary`, no
/// `content` and no fact `value` — this is the query an admin's request runs,
/// so the content never leaves Postgres for a non-owner.
pub async fn user_metadata(pool: &PgPool) -> Result<Vec<UserChatMetadata>> {
    let rows = sqlx::query_as!(
        UserChatMetadata,
        r#"
        SELECT
            u.id AS user_id,
            u.email,
            u.display_name,
            (SELECT COUNT(*) FROM chat_sessions cs WHERE cs.user_id = u.id)
                AS "session_count!",
            (SELECT COUNT(*) FROM chat_messages cm
              JOIN chat_sessions cs ON cs.id = cm.session_id
             WHERE cs.user_id = u.id)
                AS "message_count!",
            (SELECT COUNT(*) FROM chat_sessions cs
             WHERE cs.user_id = u.id AND cs.summary IS NOT NULL)
                AS "summary_count!",
            (SELECT COUNT(*) FROM chat_profile_facts f WHERE f.user_id = u.id)
                AS "fact_count!",
            (SELECT MAX(cs.updated_at) FROM chat_sessions cs WHERE cs.user_id = u.id)
                AS last_seen
        FROM users u
        ORDER BY (SELECT MAX(cs.updated_at) FROM chat_sessions cs WHERE cs.user_id = u.id)
                 DESC NULLS LAST,
                 u.email ASC
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Raw turns, past the retention window. The session row and its summary stay.
pub async fn delete_messages_older_than_days(pool: &PgPool, days: i32) -> Result<u64> {
    let deleted = sqlx::query!(
        "DELETE FROM chat_messages WHERE created_at < NOW() - make_interval(days => $1)",
        days,
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok(deleted)
}

/// Whole sessions past the summary window. `chat_messages` cascades.
pub async fn delete_sessions_older_than_days(pool: &PgPool, days: i32) -> Result<u64> {
    let deleted = sqlx::query!(
        r#"
        DELETE FROM chat_sessions
        WHERE COALESCE(closed_at, updated_at) < NOW() - make_interval(days => $1)
        "#,
        days,
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok(deleted)
}

/// Everything the bot remembers about one person, gone. Their own action —
/// nobody should need an admin to wipe their conversations.
///
/// Turn logs are in here because they hold the message text verbatim; a forget
/// that left them behind would not be one.
pub async fn forget_everything(pool: &PgPool, user_id: Uuid) -> Result<Forgotten> {
    let mut tx = pool.begin().await?;

    let messages = sqlx::query!(
        r#"
        DELETE FROM chat_messages
        WHERE session_id IN (SELECT id FROM chat_sessions WHERE user_id = $1)
        "#,
        user_id,
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();

    let sessions = sqlx::query!("DELETE FROM chat_sessions WHERE user_id = $1", user_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();

    let facts = sqlx::query!("DELETE FROM chat_profile_facts WHERE user_id = $1", user_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();

    let turn_logs = sqlx::query!("DELETE FROM chat_turn_logs WHERE user_id = $1", user_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();

    tx.commit().await?;
    Ok(Forgotten {
        sessions,
        messages,
        facts,
        turn_logs,
    })
}

/// Household members, for "what should *we* watch". Matched on display name
/// first, then the local part of the email — the two things someone would
/// actually say out loud.
pub async fn find_user_by_name(pool: &PgPool, name: &str) -> Result<Option<Uuid>> {
    let id = sqlx::query_scalar!(
        r#"
        SELECT id FROM users
        WHERE LOWER(display_name) = LOWER($1)
           OR LOWER(split_part(email, '@', 1)) = LOWER($1)
           OR LOWER(email) = LOWER($1)
        ORDER BY (LOWER(display_name) = LOWER($1)) DESC, created_at ASC
        LIMIT 1
        "#,
        name,
    )
    .fetch_optional(pool)
    .await?;
    Ok(id)
}

/// Ids of things `viewers` have already seen. One query per media kind, scoped
/// by user — watch state is per-user and deliberately not in the search index,
/// so the subtraction happens here in Postgres and in Rust, never in Meili.
pub async fn watched_movie_ids(pool: &PgPool, viewers: &[Uuid]) -> Result<Vec<Uuid>> {
    let ids = sqlx::query_scalar!(
        r#"
        SELECT DISTINCT movie_id FROM movie_watch_states
        WHERE user_id = ANY($1) AND watched = TRUE
        "#,
        viewers,
    )
    .fetch_all(pool)
    .await?;
    Ok(ids)
}

/// A show counts as seen once any of its episodes has been watched by any of
/// `viewers`: recommending something they are part-way through as a fresh
/// suggestion is the same mistake as recommending one they finished.
pub async fn watched_show_ids(pool: &PgPool, viewers: &[Uuid]) -> Result<Vec<Uuid>> {
    let ids = sqlx::query_scalar!(
        r#"
        SELECT DISTINCT ts.show_id
        FROM tv_watch_states ws
        JOIN tv_episodes te ON te.id = ws.episode_id
        JOIN tv_seasons ts ON ts.id = te.season_id
        WHERE ws.user_id = ANY($1) AND ws.watched = TRUE
        "#,
        viewers,
    )
    .fetch_all(pool)
    .await?;
    Ok(ids)
}

pub async fn read_book_ids(pool: &PgPool, viewers: &[Uuid]) -> Result<Vec<Uuid>> {
    let ids = sqlx::query_scalar!(
        r#"
        SELECT DISTINCT book_id FROM user_book_data
        WHERE user_id = ANY($1)
          AND (finished_at IS NOT NULL OR reading_status IN ('reading', 'finished'))
        "#,
        viewers,
    )
    .fetch_all(pool)
    .await?;
    Ok(ids)
}

/// Air times for episodes the calendar returned. Present only where TVMaze
/// genuinely published one — a NULL means the day is all we know, and the
/// caller must say "Thursday" rather than inventing an hour.
pub async fn episode_air_times(
    pool: &PgPool,
    episode_ids: &[Uuid],
) -> Result<Vec<(Uuid, OffsetDateTime)>> {
    let rows = sqlx::query!(
        r#"
        SELECT id, air_time as "air_time!"
        FROM tv_episodes
        WHERE id = ANY($1) AND air_time IS NOT NULL
        "#,
        episode_ids,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.air_time)).collect())
}
