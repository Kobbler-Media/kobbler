//! Tag/shelf operations for Kobo sync.

use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use tracing::error;
use uuid::Uuid;

use crate::kobo::types::{format_rfc3339, max_dt};
use crate::repositories::{shelf_items, shelves};

/// Input for tag item operations.
pub struct TagItemInput {
    pub revision_id: Uuid,
}

/// Result of syncing tags.
#[allow(dead_code)]
pub struct TagSyncResult {
    pub items: Vec<Value>,
    pub deleted_ids: Vec<Uuid>,
    pub last_modified: OffsetDateTime,
}

/// Create a new tag/shelf.
pub async fn create_tag(
    pool: &PgPool,
    user_id: Uuid,
    name: &str,
    items: Option<Vec<TagItemInput>>,
) -> Result<Uuid, String> {
    let shelf_id = shelves::create(pool, user_id, name)
        .await
        .map_err(|e| e.to_string())?;

    if let Some(items) = items {
        for item in items {
            if let Err(e) = shelf_items::add(pool, shelf_id, item.revision_id).await {
                error!(error = %e, shelf_id = %shelf_id, book_id = %item.revision_id, "failed to add item to tag");
            }
        }
    }

    Ok(shelf_id)
}

/// Rename a tag/shelf.
pub async fn rename_tag(
    pool: &PgPool,
    user_id: Uuid,
    tag_id: Uuid,
    name: &str,
) -> Result<(), String> {
    shelves::rename_for_user(pool, tag_id, user_id, name)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a tag/shelf.
pub async fn delete_tag(pool: &PgPool, user_id: Uuid, tag_id: Uuid) -> Result<(), String> {
    // Get name for archive
    let name = shelves::get_name(pool, tag_id, user_id)
        .await
        .ok()
        .flatten();

    // Archive the tag
    if let Some(ref name) = name
        && let Err(e) = shelves::archive(pool, tag_id, user_id, name).await
    {
        error!(error = %e, tag_id = %tag_id, "failed to archive tag before delete");
    }

    // Delete the tag
    shelves::delete_for_user(pool, tag_id, user_id)
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Add items to a tag/shelf.
pub async fn add_tag_items(
    pool: &PgPool,
    tag_id: Uuid,
    items: Vec<TagItemInput>,
) -> Result<(), String> {
    for item in items {
        if let Err(e) = shelf_items::add(pool, tag_id, item.revision_id).await {
            error!(error = %e, shelf_id = %tag_id, book_id = %item.revision_id, "failed to add item to tag");
        }
    }
    Ok(())
}

/// Remove items from a tag/shelf.
pub async fn remove_tag_items(
    pool: &PgPool,
    tag_id: Uuid,
    items: Vec<TagItemInput>,
) -> Result<(), String> {
    let mut removed_any = false;
    for item in items {
        match shelf_items::remove(pool, tag_id, item.revision_id).await {
            Ok(was_removed) => {
                if was_removed {
                    removed_any = true;
                }
            }
            Err(e) => {
                error!(error = %e, shelf_id = %tag_id, book_id = %item.revision_id, "failed to remove item from tag");
            }
        }
    }

    // Update shelf timestamp so sync picks up the removal
    if removed_any && let Err(e) = shelves::touch(pool, tag_id).await {
        error!(error = %e, shelf_id = %tag_id, "failed to update shelf timestamp after item removal");
    }

    Ok(())
}

/// Sync tags for a user since a given timestamp.
pub async fn sync_tags(pool: &PgPool, user_id: Uuid, since: OffsetDateTime) -> TagSyncResult {
    let shelf_rows = shelves::list_modified_since(pool, user_id, since)
        .await
        .unwrap_or_default();

    let mut results = Vec::new();
    let mut last_modified = since;

    for shelf in shelf_rows {
        let shelf_modified = shelf
            .item_last_modified
            .map(|dt| max_dt(dt, shelf.updated_at))
            .unwrap_or(shelf.updated_at);

        let book_ids = shelf_items::list_book_ids(pool, shelf.id)
            .await
            .unwrap_or_default();

        let items: Vec<Value> = book_ids
            .iter()
            .map(|book_id| {
                json!({
                    "RevisionId": book_id,
                    "Type": "ProductRevisionTagItem"
                })
            })
            .collect();

        let tag = json!({
            "Tag": {
                "Created": format_rfc3339(shelf.created_at),
                "Id": shelf.id,
                "Items": items,
                "LastModified": format_rfc3339(shelf_modified),
                "Name": shelf.name,
                "Type": "UserTag"
            }
        });

        let wrapper = if shelf.created_at > since {
            "NewTag"
        } else {
            "ChangedTag"
        };
        results.push(json!({ wrapper: tag }));
        last_modified = max_dt(last_modified, shelf_modified);
    }

    // Handle deleted tags
    let archive_rows = shelves::list_archives_since(pool, user_id, since)
        .await
        .unwrap_or_default();

    let mut deleted_ids = Vec::new();
    for archive in archive_rows {
        let mut tag_map = serde_json::Map::new();
        tag_map.insert("Id".to_string(), json!(archive.id));
        if let Some(name) = archive.name {
            tag_map.insert("Name".to_string(), json!(name));
        }
        tag_map.insert(
            "LastModified".to_string(),
            json!(format_rfc3339(archive.last_modified)),
        );
        tag_map.insert("Type".to_string(), json!("UserTag"));

        results.push(json!({
            "DeletedTag": {
                "Tag": Value::Object(tag_map)
            }
        }));
        deleted_ids.push(archive.id);
        last_modified = max_dt(last_modified, archive.last_modified);
    }

    TagSyncResult {
        items: results,
        deleted_ids,
        last_modified,
    }
}
