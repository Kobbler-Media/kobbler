//! Chat bot conversation store (`chat_sessions` / `chat_messages`).

use serde_json::Value;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, sqlx::Error>;

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: Uuid,
    pub transport: String,
    pub space: Option<String>,
    pub thread: Option<String>,
    pub user_id: Option<Uuid>,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone)]
pub struct MessageRow {
    pub id: Uuid,
    pub role: String,
    pub content: String,
    pub steps: Value,
    pub status: String,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone)]
pub struct NewSession<'a> {
    pub transport: &'a str,
    pub space: Option<&'a str>,
    pub thread: Option<&'a str>,
    pub user_id: Option<Uuid>,
}

pub async fn create_session(db: &PgPool, new: NewSession<'_>) -> Result<SessionRow> {
    let id = Uuid::now_v7();
    let row = sqlx::query!(
        "INSERT INTO chat_sessions (id, transport, space, thread, user_id) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING id, transport, space, thread, user_id, updated_at",
        id,
        new.transport,
        new.space,
        new.thread,
        new.user_id,
    )
    .fetch_one(db)
    .await?;
    Ok(SessionRow {
        id: row.id,
        transport: row.transport,
        space: row.space,
        thread: row.thread,
        user_id: row.user_id,
        updated_at: row.updated_at,
    })
}

/// The open session at a transport-native address (Google Chat's space+thread).
pub async fn find_open_by_address(
    db: &PgPool,
    transport: &str,
    space: &str,
    thread: &str,
) -> Result<Option<SessionRow>> {
    let row = sqlx::query!(
        "SELECT id, transport, space, thread, user_id, updated_at FROM chat_sessions \
         WHERE transport = $1 AND space = $2 AND thread = $3 AND closed_at IS NULL",
        transport,
        space,
        thread,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.map(|r| SessionRow {
        id: r.id,
        transport: r.transport,
        space: r.space,
        thread: r.thread,
        user_id: r.user_id,
        updated_at: r.updated_at,
    }))
}

/// The most recent open session for a user on a transport with no address of
/// its own. `ORDER BY updated_at DESC, id DESC` because two sessions created in
/// the same transaction share a timestamp and an arbitrary winner would make
/// the reuse decision nondeterministic.
pub async fn find_open_for_user(
    db: &PgPool,
    transport: &str,
    user_id: Uuid,
    not_before: OffsetDateTime,
) -> Result<Option<SessionRow>> {
    let row = sqlx::query!(
        "SELECT id, transport, space, thread, user_id, updated_at FROM chat_sessions \
         WHERE transport = $1 AND user_id = $2 AND closed_at IS NULL AND updated_at >= $3 \
         ORDER BY updated_at DESC, id DESC LIMIT 1",
        transport,
        user_id,
        not_before,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.map(|r| SessionRow {
        id: r.id,
        transport: r.transport,
        space: r.space,
        thread: r.thread,
        user_id: r.user_id,
        updated_at: r.updated_at,
    }))
}

pub async fn close_session(db: &PgPool, id: Uuid) -> Result<()> {
    sqlx::query!(
        "UPDATE chat_sessions SET closed_at = NOW() WHERE id = $1 AND closed_at IS NULL",
        id
    )
    .execute(db)
    .await?;
    Ok(())
}

pub async fn touch_session(db: &PgPool, id: Uuid) -> Result<()> {
    sqlx::query!(
        "UPDATE chat_sessions SET updated_at = NOW() WHERE id = $1",
        id
    )
    .execute(db)
    .await?;
    Ok(())
}

pub async fn append_message(
    db: &PgPool,
    session_id: Uuid,
    role: &str,
    content: &str,
    status: &str,
) -> Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO chat_messages (id, session_id, role, content, status) \
         VALUES ($1, $2, $3, $4, $5)",
        id,
        session_id,
        role,
        content,
        status,
    )
    .execute(db)
    .await?;
    Ok(id)
}

pub async fn finish_message(
    db: &PgPool,
    id: Uuid,
    content: &str,
    steps: &Value,
    status: &str,
) -> Result<()> {
    sqlx::query!(
        "UPDATE chat_messages SET content = $2, steps = $3, status = $4 WHERE id = $1",
        id,
        content,
        steps,
        status,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Oldest first — the order the loop replays them in.
pub async fn messages(db: &PgPool, session_id: Uuid, limit: i64) -> Result<Vec<MessageRow>> {
    let rows = sqlx::query!(
        "SELECT id, role, content, steps, status, created_at FROM chat_messages \
         WHERE session_id = $1 ORDER BY created_at DESC, id DESC LIMIT $2",
        session_id,
        limit,
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .rev()
        .map(|r| MessageRow {
            id: r.id,
            role: r.role,
            content: r.content,
            steps: r.steps,
            status: r.status,
            created_at: r.created_at,
        })
        .collect())
}

pub async fn count_messages_with_role(db: &PgPool, session_id: Uuid, role: &str) -> Result<i64> {
    let count = sqlx::query_scalar!(
        r#"SELECT COUNT(*) as "count!" FROM chat_messages WHERE session_id = $1 AND role = $2"#,
        session_id,
        role,
    )
    .fetch_one(db)
    .await?;
    Ok(count)
}
