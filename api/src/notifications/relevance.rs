//! Who actually cares that this arrived.
//!
//! The global agents (Pushover, Plex, web push) fan out to everyone and stay
//! that way. Piped into a chat, that is a firehose, and a firehose gets muted in
//! week one — so the chat agent asks this module first and says nothing about
//! anything nobody here asked for.
//!
//! Every query below is `user_id`-scoped by construction. That is the whole
//! point: the removed `MovieDocument.watched` read a per-user table with no
//! `user_id` and no `ORDER BY` and was therefore wrong for everyone. Do not
//! reintroduce that shape here.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use super::NotificationTrigger;
use super::prefs;
use crate::repositories::chat_identities::ChatIdentity;
use crate::repositories::chat_notification_prefs;

/// The thing a notification is about. Sports events are `tv_episodes` rows with
/// `content_category = 'sport'`, so [`Entity::Episode`] covers them too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Entity {
    Book(Uuid),
    Movie(Uuid),
    Show(Uuid),
    Season(Uuid),
    Episode(Uuid),
}

impl Entity {
    pub fn id(&self) -> Uuid {
        match self {
            Entity::Book(id)
            | Entity::Movie(id)
            | Entity::Show(id)
            | Entity::Season(id)
            | Entity::Episode(id) => *id,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Entity::Book(_) => "book",
            Entity::Movie(_) => "movie",
            Entity::Show(_) => "show",
            Entity::Season(_) => "season",
            Entity::Episode(_) => "episode",
        }
    }
}

/// Work out what kind of thing a bare notification id is.
///
/// `notifications::resolve_entity` cannot be reused for this: it reports a
/// one-episode season pack as an `"episode"` while still carrying the *season*
/// id, which would send every lookup below to the wrong table.
pub async fn classify(pool: &PgPool, id: Uuid) -> Result<Option<Entity>, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        SELECT
            EXISTS(SELECT 1 FROM books       WHERE id = $1) AS "is_book!",
            EXISTS(SELECT 1 FROM movies      WHERE id = $1) AS "is_movie!",
            EXISTS(SELECT 1 FROM tv_episodes WHERE id = $1) AS "is_episode!",
            EXISTS(SELECT 1 FROM tv_seasons  WHERE id = $1) AS "is_season!",
            EXISTS(SELECT 1 FROM tv_shows    WHERE id = $1) AS "is_show!"
        "#,
        id,
    )
    .fetch_one(pool)
    .await?;

    Ok(if row.is_book {
        Some(Entity::Book(id))
    } else if row.is_movie {
        Some(Entity::Movie(id))
    } else if row.is_episode {
        Some(Entity::Episode(id))
    } else if row.is_season {
        Some(Entity::Season(id))
    } else if row.is_show {
        Some(Entity::Show(id))
    } else {
        None
    })
}

/// Users with a signal on `entity`, sorted and deduplicated.
///
/// A user qualifies if they have watched any of it, put it on their watchlist,
/// asked for it, or are reading it. Everything else is silence.
///
/// Watch state and the watchlist resolve *up* — one episode watched or the show
/// listed makes every episode of that show relevant. "They asked for it" does
/// not: an attempt row names the exact thing that was requested, and widening it
/// to the show would make one season-24 request an opt-in to the whole series.
pub async fn interested_users(pool: &PgPool, entity: &Entity) -> Result<Vec<Uuid>, sqlx::Error> {
    match entity {
        Entity::Book(id) => {
            sqlx::query_scalar!(
                r#"
                SELECT u.user_id AS "user_id!"
                FROM (
                    SELECT r.user_id FROM reading_states r WHERE r.book_id = $1
                    UNION
                    SELECT wa.user_id FROM wanted_attempts wa
                     WHERE wa.book_id = $1 AND wa.user_id IS NOT NULL
                ) u
                ORDER BY u.user_id
                "#,
                id,
            )
            .fetch_all(pool)
            .await
        }
        Entity::Movie(id) => {
            sqlx::query_scalar!(
                r#"
                SELECT u.user_id AS "user_id!"
                FROM (
                    SELECT mws.user_id FROM movie_watch_states mws WHERE mws.movie_id = $1
                    UNION
                    SELECT wi.user_id FROM watchlist_items wi
                     WHERE wi.media_type = 'movie' AND wi.media_id = $1
                    UNION
                    SELECT wa.user_id FROM wanted_attempts wa
                     WHERE wa.movie_id = $1 AND wa.user_id IS NOT NULL
                ) u
                ORDER BY u.user_id
                "#,
                id,
            )
            .fetch_all(pool)
            .await
        }
        Entity::Show(id) => {
            sqlx::query_scalar!(
                r#"
                SELECT u.user_id AS "user_id!"
                FROM (
                    SELECT tws.user_id FROM tv_watch_states tws
                      JOIN tv_episodes e ON e.id = tws.episode_id
                      JOIN tv_seasons s ON s.id = e.season_id
                     WHERE s.show_id = $1
                    UNION
                    SELECT wi.user_id FROM watchlist_items wi
                     WHERE wi.media_type = 'show' AND wi.media_id = $1
                    UNION
                    SELECT wa.user_id FROM wanted_attempts wa
                      JOIN tv_episodes e ON e.id = wa.episode_id
                      JOIN tv_seasons s ON s.id = e.season_id
                     WHERE s.show_id = $1 AND wa.user_id IS NOT NULL
                ) u
                ORDER BY u.user_id
                "#,
                id,
            )
            .fetch_all(pool)
            .await
        }
        Entity::Season(id) => {
            sqlx::query_scalar!(
                r#"
                SELECT u.user_id AS "user_id!"
                FROM (
                    SELECT tws.user_id FROM tv_watch_states tws
                      JOIN tv_episodes e ON e.id = tws.episode_id
                      JOIN tv_seasons s ON s.id = e.season_id
                     WHERE s.show_id = (SELECT show_id FROM tv_seasons WHERE id = $1)
                    UNION
                    SELECT wi.user_id FROM watchlist_items wi
                     WHERE wi.media_type = 'show'
                       AND wi.media_id = (SELECT show_id FROM tv_seasons WHERE id = $1)
                    UNION
                    SELECT wa.user_id FROM wanted_attempts wa
                      JOIN tv_episodes e ON e.id = wa.episode_id
                     WHERE e.season_id = $1 AND wa.user_id IS NOT NULL
                ) u
                ORDER BY u.user_id
                "#,
                id,
            )
            .fetch_all(pool)
            .await
        }
        Entity::Episode(id) => {
            sqlx::query_scalar!(
                r#"
                SELECT u.user_id AS "user_id!"
                FROM (
                    SELECT tws.user_id FROM tv_watch_states tws
                      JOIN tv_episodes e2 ON e2.id = tws.episode_id
                      JOIN tv_seasons s2 ON s2.id = e2.season_id
                     WHERE s2.show_id = (
                         SELECT s.show_id FROM tv_episodes e
                           JOIN tv_seasons s ON s.id = e.season_id
                          WHERE e.id = $1
                     )
                    UNION
                    SELECT wi.user_id FROM watchlist_items wi
                     WHERE wi.media_type = 'show'
                       AND wi.media_id = (
                           SELECT s.show_id FROM tv_episodes e
                             JOIN tv_seasons s ON s.id = e.season_id
                            WHERE e.id = $1
                       )
                    UNION
                    SELECT wa.user_id FROM wanted_attempts wa
                     WHERE wa.episode_id = $1 AND wa.user_id IS NOT NULL
                ) u
                ORDER BY u.user_id
                "#,
                id,
            )
            .fetch_all(pool)
            .await
        }
    }
}

/// One chat identity that should hear about an event, with the user it belongs
/// to. A user with two identities gets two entries — the transport decides
/// which of its own it delivers to.
#[derive(Debug, Clone)]
pub struct ChatRecipient {
    pub user_id: Uuid,
    pub identity: ChatIdentity,
}

/// Relevance, then each interested user's preferences, then their chat
/// identities. Nothing here touches the global agents' fan-out.
pub async fn chat_recipients(
    pool: &PgPool,
    entity: &Entity,
    trigger: NotificationTrigger,
    now: OffsetDateTime,
) -> Result<Vec<ChatRecipient>, sqlx::Error> {
    let interested = interested_users(pool, entity).await?;
    if interested.is_empty() {
        return Ok(Vec::new());
    }

    let prefs_by_user = chat_notification_prefs::get_many(pool, &interested)
        .await
        .map_err(|e| match e {
            crate::repositories::RepositoryError::Database(db) => db,
            other => sqlx::Error::Protocol(other.to_string()),
        })?;

    // Quiet hours are wall-clock times in each person's own zone, so the local
    // clock is per user, not one server-wide reading.
    let zones = crate::repositories::users::get_timezones(pool, &interested)
        .await
        .map_err(|e| match e {
            crate::repositories::RepositoryError::Database(db) => db,
            other => sqlx::Error::Protocol(other.to_string()),
        })?;

    let allowed: Vec<Uuid> = interested
        .into_iter()
        .filter(|user_id| match prefs_by_user.get(user_id) {
            Some(prefs) => {
                let zone = prefs::Zone::for_user(zones.get(user_id).map(String::as_str));
                prefs::should_notify(prefs, trigger, now, zone.local_time(now))
            }
            None => false,
        })
        .collect();

    if allowed.is_empty() {
        return Ok(Vec::new());
    }

    let rows = sqlx::query_as!(
        ChatIdentity,
        r#"
        SELECT id, transport, address, external_id, user_id, max_role, enabled,
               created_at, last_seen_at
        FROM chat_identities
        WHERE enabled = TRUE AND user_id = ANY($1)
        ORDER BY user_id, created_at
        "#,
        &allowed,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|identity| ChatRecipient {
            user_id: identity.user_id,
            identity,
        })
        .collect())
}

/// The same thing starting from a bare notification entity id, which is what
/// `translate_event` hands out.
pub async fn chat_recipients_for_id(
    pool: &PgPool,
    entity_id: Uuid,
    trigger: NotificationTrigger,
    now: OffsetDateTime,
) -> Result<Vec<ChatRecipient>, sqlx::Error> {
    match classify(pool, entity_id).await? {
        Some(entity) => chat_recipients(pool, &entity, trigger, now).await,
        None => Ok(Vec::new()),
    }
}

/// What the chat notification agent should actually do with one event.
#[derive(Debug, Clone)]
pub enum ChatDelivery {
    /// Nobody has bound a chat identity on this transport, so the agent keeps
    /// its original behaviour and posts to the single configured space. This is
    /// an *install-level* property, checked once per event: the moment the first
    /// identity is bound the agent becomes relevance-scoped for everyone, so the
    /// fallback can never quietly turn back into "tell the whole space".
    Unbound,
    /// Deliver to exactly these identities and nobody else. Empty is silence —
    /// which is the normal outcome for something nobody here cares about.
    Scoped(Vec<ChatRecipient>),
}

/// True once anyone at all has a usable identity on `transport`.
pub async fn any_identity_bound(pool: &PgPool, transport: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM chat_identities WHERE transport = $1 AND enabled = TRUE
        ) AS "bound!"
        "#,
        transport,
    )
    .fetch_one(pool)
    .await
}

/// The whole decision for one event, in one call: fall back, or scope.
///
/// An entity that cannot be classified resolves to `Scoped(vec![])` rather than
/// the fallback — if we cannot say who asked for a thing, broadcasting it to the
/// shared space is exactly the firehose this exists to prevent.
pub async fn chat_delivery(
    pool: &PgPool,
    transport: &str,
    entity_id: Option<Uuid>,
    trigger: NotificationTrigger,
    now: OffsetDateTime,
) -> Result<ChatDelivery, sqlx::Error> {
    if !any_identity_bound(pool, transport).await? {
        return Ok(ChatDelivery::Unbound);
    }

    let Some(entity_id) = entity_id else {
        return Ok(ChatDelivery::Scoped(Vec::new()));
    };

    let mut recipients = chat_recipients_for_id(pool, entity_id, trigger, now).await?;
    recipients.retain(|r| r.identity.transport == transport);
    Ok(ChatDelivery::Scoped(recipients))
}
