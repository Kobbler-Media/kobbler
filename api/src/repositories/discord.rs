//! Discord agent state: per-show release threads and the entity→message map.
//!
//! Each show thread also carries the id of its channel card (migration 0180) and the
//! item that card is showing (migration 0181).
//!
//! Both tables are keyed by the notification agent id, so a Discord agent and,
//! say, a Pushover agent never share rows. See migration 0179.

use super::Result;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

// =============================================================================
// Per-show release threads
// =============================================================================

/// The thread id Discord created for this (agent, show), if one exists.
pub async fn get_thread<'e, E>(exec: E, agent_id: Uuid, show_id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT thread_id
        FROM discord_show_threads
        WHERE agent_id = $1 AND show_id = $2
        "#,
        agent_id,
        show_id
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| r.thread_id))
}

/// Record (or refresh) the thread id for this (agent, show).
pub async fn upsert_thread<'e, E>(
    exec: E,
    agent_id: Uuid,
    show_id: Uuid,
    thread_id: &str,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO discord_show_threads (agent_id, show_id, thread_id)
        VALUES ($1, $2, $3)
        ON CONFLICT (agent_id, show_id) DO UPDATE SET thread_id = EXCLUDED.thread_id
        "#,
        agent_id,
        show_id,
        thread_id
    )
    .execute(exec)
    .await?;

    Ok(())
}

/// The channel card currently advertising this (agent, show)'s thread, if any, and
/// the item it is showing.
pub async fn get_card<'e, E>(
    exec: E,
    agent_id: Uuid,
    show_id: Uuid,
) -> Result<Option<(String, Option<Uuid>)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT card_message_id, card_entity_id
        FROM discord_show_threads
        WHERE agent_id = $1 AND show_id = $2
        "#,
        agent_id,
        show_id
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.and_then(|r| r.card_message_id.map(|id| (id, r.card_entity_id))))
}

/// Record the channel card that now advertises this (agent, show)'s thread, and the
/// item it shows.
pub async fn set_card<'e, E>(
    exec: E,
    agent_id: Uuid,
    show_id: Uuid,
    card_message_id: &str,
    entity_id: Uuid,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        UPDATE discord_show_threads
        SET card_message_id = $3, card_entity_id = $4
        WHERE agent_id = $1 AND show_id = $2
        "#,
        agent_id,
        show_id,
        card_message_id,
        entity_id
    )
    .execute(exec)
    .await?;

    Ok(())
}

// =============================================================================
// Entity → message map
// =============================================================================

/// The Discord message that last announced this entity, and the channel it lives in.
pub async fn get_message<'e, E>(
    exec: E,
    agent_id: Uuid,
    entity_id: Uuid,
) -> Result<Option<(String, String)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT message_id, channel_id
        FROM discord_message_map
        WHERE agent_id = $1 AND entity_id = $2
        "#,
        agent_id,
        entity_id
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| (r.message_id, r.channel_id)))
}

/// Record (or refresh) the message that announced this entity.
pub async fn upsert_message<'e, E>(
    exec: E,
    agent_id: Uuid,
    entity_id: Uuid,
    message_id: &str,
    channel_id: &str,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO discord_message_map (agent_id, entity_id, message_id, channel_id)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (agent_id, entity_id)
        DO UPDATE SET message_id = EXCLUDED.message_id, channel_id = EXCLUDED.channel_id
        "#,
        agent_id,
        entity_id,
        message_id,
        channel_id
    )
    .execute(exec)
    .await?;

    Ok(())
}
