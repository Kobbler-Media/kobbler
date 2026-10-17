//! The household inbox: every profile hears about what arrives, except that a Kids profile
//! only hears about titles it could open.

use sqlx::PgPool;
use uuid::Uuid;

use crate::repositories::user_notifications::{self, CreateUserNotification};

pub struct InAppNotification {
    pub title: String,
    pub message: String,
    pub entity_type: Option<String>,
    pub entity_id: Option<Uuid>,
    pub entity_url: Option<String>,
}

pub async fn notify_household(pool: &PgPool, notification: &InAppNotification) -> sqlx::Result<()> {
    let users = sqlx::query_as::<_, (Uuid, bool)>("SELECT id, is_kids FROM users")
        .fetch_all(pool)
        .await?;
    let kids_may_see = if users.iter().any(|(_, is_kids)| *is_kids) {
        crate::kids::entity_allowed(
            pool,
            notification.entity_type.as_deref(),
            notification.entity_id,
        )
        .await?
    } else {
        true
    };
    for (user_id, is_kids) in users {
        if is_kids && !kids_may_see {
            continue;
        }
        if let Err(e) = user_notifications::create(
            pool,
            CreateUserNotification {
                user_id,
                title: notification.title.clone(),
                message: notification.message.clone(),
                entity_type: notification.entity_type.clone(),
                entity_id: notification.entity_id,
                entity_url: notification.entity_url.clone(),
            },
        )
        .await
        {
            tracing::warn!(error = %e, "Failed to create in-app notification");
        }
    }
    Ok(())
}
