//! Per-user chat notification preferences.
//!
//! A user with no row has the defaults, so nothing has to create one before the
//! first notification: read paths fall back to [`ChatNotificationPrefs::defaults`]
//! and only an actual preference change writes.

use std::collections::HashMap;

use sqlx::{Executor, Postgres};
use time::{OffsetDateTime, Time};
use uuid::Uuid;

use super::Result;
use crate::notifications::NotificationTrigger;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatNotificationPrefs {
    pub user_id: Uuid,
    pub enabled: bool,
    pub triggers: Vec<NotificationTrigger>,
    pub muted_until: Option<OffsetDateTime>,
    pub quiet_from: Option<Time>,
    pub quiet_to: Option<Time>,
}

impl ChatNotificationPrefs {
    pub fn defaults(user_id: Uuid) -> Self {
        Self {
            user_id,
            enabled: true,
            triggers: vec![NotificationTrigger::OnImport],
            muted_until: None,
            quiet_from: None,
            quiet_to: None,
        }
    }

    /// The window to stay silent in, or `None` for "say things when they happen".
    ///
    /// Quiet hours are opt-in: unset means off. They used to default to 22:00-07:00,
    /// which is silent about the thing that makes it dangerous — the window is judged
    /// in the *server's* zone for anyone who has not set their own, so a household at
    /// UTC+10 was muted 08:00-17:00 local and notified all night, and the messages
    /// arrived hours late rather than never, which is the hardest version to notice.
    /// A messaging app has its own do-not-disturb; this does not need to guess.
    ///
    /// A zero-width window (`from == to`) still reads as off — it is how the tool
    /// records "turn quiet hours off" against a row that already had some.
    ///
    /// Migration 0165's comment describes the old default. It has been applied, and
    /// editing an applied migration changes its checksum and panics the API at boot,
    /// so the comment stays wrong there and right here.
    pub fn quiet_window(&self) -> Option<(Time, Time)> {
        let (from, to) = (self.quiet_from?, self.quiet_to?);
        if from == to { None } else { Some((from, to)) }
    }
}

fn decode_triggers(raw: serde_json::Value) -> Vec<NotificationTrigger> {
    serde_json::from_value(raw).unwrap_or_else(|_| vec![NotificationTrigger::OnImport])
}

fn encode_triggers(triggers: &[NotificationTrigger]) -> serde_json::Value {
    serde_json::to_value(triggers).unwrap_or_else(|_| serde_json::json!(["on_import"]))
}

pub async fn get<'e, E>(exec: E, user_id: Uuid) -> Result<ChatNotificationPrefs>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT enabled, triggers, muted_until, quiet_from, quiet_to
        FROM chat_notification_prefs
        WHERE user_id = $1
        "#,
        user_id,
    )
    .fetch_optional(exec)
    .await?;

    Ok(match row {
        Some(row) => ChatNotificationPrefs {
            user_id,
            enabled: row.enabled,
            triggers: decode_triggers(row.triggers),
            muted_until: row.muted_until,
            quiet_from: row.quiet_from,
            quiet_to: row.quiet_to,
        },
        None => ChatNotificationPrefs::defaults(user_id),
    })
}

/// Every id in `user_ids` comes back, with defaults filled in for the ones that
/// have no row — one query, never one per user.
pub async fn get_many<'e, E>(
    exec: E,
    user_ids: &[Uuid],
) -> Result<HashMap<Uuid, ChatNotificationPrefs>>
where
    E: Executor<'e, Database = Postgres>,
{
    let mut out: HashMap<Uuid, ChatNotificationPrefs> = user_ids
        .iter()
        .map(|id| (*id, ChatNotificationPrefs::defaults(*id)))
        .collect();

    if user_ids.is_empty() {
        return Ok(out);
    }

    let rows = sqlx::query!(
        r#"
        SELECT user_id, enabled, triggers, muted_until, quiet_from, quiet_to
        FROM chat_notification_prefs
        WHERE user_id = ANY($1)
        "#,
        user_ids,
    )
    .fetch_all(exec)
    .await?;

    for row in rows {
        out.insert(
            row.user_id,
            ChatNotificationPrefs {
                user_id: row.user_id,
                enabled: row.enabled,
                triggers: decode_triggers(row.triggers),
                muted_until: row.muted_until,
                quiet_from: row.quiet_from,
                quiet_to: row.quiet_to,
            },
        );
    }

    Ok(out)
}

pub async fn upsert<'e, E>(exec: E, prefs: &ChatNotificationPrefs) -> Result<ChatNotificationPrefs>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        INSERT INTO chat_notification_prefs
            (user_id, enabled, triggers, muted_until, quiet_from, quiet_to)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (user_id) DO UPDATE SET
            enabled     = EXCLUDED.enabled,
            triggers    = EXCLUDED.triggers,
            muted_until = EXCLUDED.muted_until,
            quiet_from  = EXCLUDED.quiet_from,
            quiet_to    = EXCLUDED.quiet_to
        RETURNING user_id, enabled, triggers, muted_until, quiet_from, quiet_to
        "#,
        prefs.user_id,
        prefs.enabled,
        encode_triggers(&prefs.triggers),
        prefs.muted_until,
        prefs.quiet_from,
        prefs.quiet_to,
    )
    .fetch_one(exec)
    .await?;

    Ok(ChatNotificationPrefs {
        user_id: row.user_id,
        enabled: row.enabled,
        triggers: decode_triggers(row.triggers),
        muted_until: row.muted_until,
        quiet_from: row.quiet_from,
        quiet_to: row.quiet_to,
    })
}

pub async fn delete<'e, E>(exec: E, user_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let res = sqlx::query!(
        "DELETE FROM chat_notification_prefs WHERE user_id = $1",
        user_id
    )
    .execute(exec)
    .await?;
    Ok(res.rows_affected() > 0)
}
