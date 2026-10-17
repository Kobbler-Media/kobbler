//! Users repository operations.

use std::collections::HashMap;

use super::Result;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

/// Create a new user.
pub async fn create<'e, E>(exec: E, email: &str, password_hash: &str, role: &str) -> Result<Uuid>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO users (id, email, password_hash, role) VALUES ($1, $2, $3, $4)",
        id,
        email,
        password_hash,
        role,
    )
    .execute(exec)
    .await?;
    Ok(id)
}

/// Get user count.
pub async fn count<'e, E>(exec: E) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!(r#"SELECT COUNT(*) as "count!" FROM users"#)
        .fetch_one(exec)
        .await?;
    Ok(row)
}

/// Check if any users exist.
pub async fn any_exist<'e, E>(exec: E) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let count = sqlx::query_scalar!(r#"SELECT COUNT(*) as "count!" FROM users"#)
        .fetch_one(exec)
        .await?;
    Ok(count > 0)
}

/// Check if a user exists by ID.
pub async fn exists<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!("SELECT id FROM users WHERE id = $1", id)
        .fetch_optional(exec)
        .await?;
    Ok(row.is_some())
}

/// Check if email exists for another user (for update validation).
pub async fn email_exists_for_other<'e, E>(exec: E, email: &str, exclude_id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        "SELECT id FROM users WHERE email = $1 AND id != $2",
        email,
        exclude_id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.is_some())
}

/// Get user ID, password hash, and role by email (for login).
pub async fn get_by_email_for_login<'e, E>(
    exec: E,
    email: &str,
) -> Result<Option<(Uuid, String, String)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        "SELECT id, password_hash, role FROM users WHERE email = $1",
        email,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| (r.id, r.password_hash, r.role)))
}

/// Get user by ID.
pub async fn get_by_id<'e, E>(exec: E, id: Uuid) -> Result<Option<(Uuid, String, String)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!("SELECT id, email, role FROM users WHERE id = $1", id)
        .fetch_optional(exec)
        .await?;
    Ok(row.map(|r| (r.id, r.email, r.role)))
}

/// The name a user chose to be shown as, if they have set one.
pub async fn get_display_name<'e, E>(exec: E, id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!("SELECT display_name FROM users WHERE id = $1", id)
        .fetch_optional(exec)
        .await?;
    Ok(row.flatten())
}

/// Set or clear a user's display name. `None` clears it, falling the client back to
/// deriving something from the email address.
pub async fn update_display_name<'e, E>(exec: E, id: Uuid, name: Option<&str>) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("UPDATE users SET display_name = $2 WHERE id = $1", id, name,)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// A user's IANA timezone name, if they have set one. `None` means "use the
/// server's zone" — see [`crate::notifications::prefs::Zone`].
pub async fn get_timezone<'e, E>(exec: E, id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!("SELECT timezone FROM users WHERE id = $1", id)
        .fetch_optional(exec)
        .await?;
    Ok(row.flatten())
}

/// Every id that has a timezone set, in one query — the notification fan-out
/// judges quiet hours for a whole set of users at once and must never go per-row.
pub async fn get_timezones<'e, E>(exec: E, ids: &[Uuid]) -> Result<HashMap<Uuid, String>>
where
    E: Executor<'e, Database = Postgres>,
{
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query!(
        r#"SELECT id, timezone as "timezone!" FROM users WHERE id = ANY($1) AND timezone IS NOT NULL"#,
        ids,
    )
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.timezone)).collect())
}

/// Set or clear a user's timezone. Callers must have validated the name as an
/// IANA zone first; the column is plain text.
pub async fn update_timezone<'e, E>(exec: E, id: Uuid, timezone: Option<&str>) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("UPDATE users SET timezone = $2 WHERE id = $1", id, timezone)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Update user email.
pub async fn update_email<'e, E>(exec: E, id: Uuid, email: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("UPDATE users SET email = $2 WHERE id = $1", id, email,)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Update user password hash.
pub async fn update_password<'e, E>(exec: E, id: Uuid, password_hash: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE users SET password_hash = $2 WHERE id = $1",
        id,
        password_hash,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// User summary for listing.
pub struct UserSummary {
    pub id: Uuid,
    pub email: String,
    pub role: String,
    pub display_name: Option<String>,
    pub timezone: Option<String>,
    pub is_kids: bool,
}

/// List all users.
pub async fn list_all<'e, E>(exec: E) -> Result<Vec<UserSummary>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        "SELECT id, email, role, display_name, timezone, is_kids FROM users ORDER BY created_at DESC",
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| UserSummary {
            id: r.id,
            email: r.email,
            role: r.role,
            display_name: r.display_name,
            timezone: r.timezone,
            is_kids: r.is_kids,
        })
        .collect())
}

/// Count users with admin role.
pub async fn count_admins<'e, E>(exec: E) -> Result<i64>
where
    E: Executor<'e, Database = Postgres>,
{
    let count =
        sqlx::query_scalar!(r#"SELECT COUNT(*) as "count!" FROM users WHERE role = 'admin'"#,)
            .fetch_one(exec)
            .await?;
    Ok(count)
}

/// Update a user's role.
pub async fn set_kids<'e, E>(exec: E, user_id: Uuid, is_kids: bool) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE users SET is_kids = $1 WHERE id = $2",
        is_kids,
        user_id
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn update_role<'e, E>(exec: E, user_id: Uuid, role: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("UPDATE users SET role = $1 WHERE id = $2", role, user_id,)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Update user avatar.
pub async fn update_avatar<'e, E>(
    exec: E,
    id: Uuid,
    avatar: &[u8],
    content_type: &str,
) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE users SET avatar = $2, avatar_content_type = $3 WHERE id = $1",
        id,
        avatar,
        content_type,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Get user avatar.
pub async fn get_avatar<'e, E>(exec: E, id: Uuid) -> Result<Option<(Vec<u8>, String)>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"SELECT avatar, avatar_content_type as "avatar_content_type?" FROM users WHERE id = $1 AND avatar IS NOT NULL"#,
        id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.and_then(|r| {
        let avatar = r.avatar?;
        let ct = r
            .avatar_content_type
            .unwrap_or_else(|| "image/jpeg".to_string());
        Some((avatar, ct))
    }))
}

/// Delete user avatar.
pub async fn delete_avatar<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE users SET avatar = NULL, avatar_content_type = NULL WHERE id = $1",
        id,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Delete a user by ID. Returns true if a row was deleted.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("DELETE FROM users WHERE id = $1", id)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Check if user has an avatar.
pub async fn has_avatar<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND avatar IS NOT NULL) as "exists!""#,
        id,
    )
    .fetch_one(exec)
    .await?;
    Ok(row)
}

/// Get user's password hash by ID (for self-service password change).
pub async fn get_password_hash<'e, E>(exec: E, id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!("SELECT password_hash FROM users WHERE id = $1", id,)
        .fetch_optional(exec)
        .await?;
    Ok(row)
}

/// Set the profile PIN hash for a user.
pub async fn set_profile_pin<'e, E>(exec: E, id: Uuid, pin_hash: &str) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE users SET profile_pin_hash = $2 WHERE id = $1",
        id,
        pin_hash,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Clear the profile PIN for a user.
pub async fn clear_profile_pin<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!("UPDATE users SET profile_pin_hash = NULL WHERE id = $1", id,)
        .execute(exec)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Get the profile PIN hash for a user, if set.
pub async fn get_profile_pin_hash<'e, E>(exec: E, id: Uuid) -> Result<Option<String>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!(
        r#"SELECT profile_pin_hash as "profile_pin_hash?" FROM users WHERE id = $1"#,
        id,
    )
    .fetch_optional(exec)
    .await?;
    Ok(row.flatten())
}

/// Check whether the user has a profile PIN set.
pub async fn has_profile_pin<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND profile_pin_hash IS NOT NULL) as "exists!""#,
        id,
    )
    .fetch_one(exec)
    .await?;
    Ok(row)
}
