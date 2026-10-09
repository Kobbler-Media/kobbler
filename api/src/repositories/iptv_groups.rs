//! IPTV channel groups repository operations.

use super::Result;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

// =============================================================================
// Types
// =============================================================================

#[derive(Debug)]
pub struct GroupRow {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub icon: Option<String>,
    pub display_order: i32,
    pub auto_assign_categories: Vec<String>,
    pub enabled: bool,
}

#[derive(Debug)]
pub struct GroupWithCount {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub icon: Option<String>,
    pub display_order: i32,
    pub auto_assign_categories: Vec<String>,
    pub enabled: bool,
    pub channel_count: i64,
}

#[derive(Debug)]
pub struct CreateGroup {
    pub name: String,
    pub slug: String,
    pub icon: Option<String>,
    pub display_order: Option<i32>,
    pub auto_assign_categories: Vec<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug)]
pub struct UpdateGroup {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub icon: Option<String>,
    pub display_order: Option<i32>,
    pub auto_assign_categories: Option<Vec<String>>,
    pub enabled: Option<bool>,
}

// =============================================================================
// Operations
// =============================================================================

/// List all groups with channel counts (channels matching on category).
pub async fn list_all_with_counts<'e, E>(exec: E) -> Result<Vec<GroupWithCount>>
where
    E: Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        SELECT g.id, g.name, g.slug, g.icon, g.display_order, g.auto_assign_categories,
               g.enabled,
               COUNT(DISTINCT c.id) AS "channel_count!"
        FROM iptv_channel_groups g
        LEFT JOIN iptv_channels c ON c.enabled AND c.category = ANY(g.auto_assign_categories)
        GROUP BY g.id
        ORDER BY g.display_order ASC, g.name ASC
        "#,
    )
    .fetch_all(exec)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| GroupWithCount {
            id: r.id,
            name: r.name,
            slug: r.slug,
            icon: r.icon,
            display_order: r.display_order,
            auto_assign_categories: r.auto_assign_categories,
            enabled: r.enabled,
            channel_count: r.channel_count,
        })
        .collect())
}

/// Create a new group.
pub async fn create<'e, E>(exec: E, input: CreateGroup) -> Result<GroupRow>
where
    E: Executor<'e, Database = Postgres>,
{
    let id = Uuid::now_v7();
    let r = sqlx::query!(
        r#"
        INSERT INTO iptv_channel_groups (id, name, slug, icon, display_order,
                                         auto_assign_categories, enabled)
        VALUES ($1, $2, $3, $4, COALESCE($5, 0), $6, COALESCE($7, true))
        RETURNING id, name, slug, icon, display_order, auto_assign_categories, enabled
        "#,
        id,
        input.name,
        input.slug,
        input.icon,
        input.display_order,
        &input.auto_assign_categories,
        input.enabled,
    )
    .fetch_one(exec)
    .await?;

    Ok(GroupRow {
        id: r.id,
        name: r.name,
        slug: r.slug,
        icon: r.icon,
        display_order: r.display_order,
        auto_assign_categories: r.auto_assign_categories,
        enabled: r.enabled,
    })
}

/// Update a group.
pub async fn update<'e, E>(exec: E, id: Uuid, input: UpdateGroup) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE iptv_channel_groups SET
            name = COALESCE($2, name),
            slug = COALESCE($3, slug),
            icon = COALESCE($4, icon),
            display_order = COALESCE($5, display_order),
            auto_assign_categories = COALESCE($6, auto_assign_categories),
            enabled = COALESCE($7, enabled)
        WHERE id = $1
        "#,
        id,
        input.name,
        input.slug,
        input.icon,
        input.display_order,
        input.auto_assign_categories.as_deref(),
        input.enabled,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete a group by ID.
pub async fn delete<'e, E>(exec: E, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        DELETE FROM iptv_channel_groups WHERE id = $1
        "#,
        id,
    )
    .execute(exec)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Get a group by slug.
pub async fn get_by_slug<'e, E>(exec: E, slug: &str) -> Result<Option<GroupRow>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query!(
        r#"
        SELECT id, name, slug, icon, display_order, auto_assign_categories, enabled
        FROM iptv_channel_groups
        WHERE slug = $1
        "#,
        slug,
    )
    .fetch_optional(exec)
    .await?;

    Ok(row.map(|r| GroupRow {
        id: r.id,
        name: r.name,
        slug: r.slug,
        icon: r.icon,
        display_order: r.display_order,
        auto_assign_categories: r.auto_assign_categories,
        enabled: r.enabled,
    }))
}
