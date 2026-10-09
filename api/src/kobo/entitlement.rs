//! Book entitlement building for Kobo sync.

use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::kobo::types::format_rfc3339;
use crate::repositories::{books, entitlements, series, shelves};

/// Build Kobo entitlement JSON for a book.
pub fn build_entitlement(
    book_id: Uuid,
    is_removed: bool,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
) -> Value {
    json!({
        "Accessibility": "Full",
        "ActivePeriod": {"From": format_rfc3339(created_at)},
        "Created": format_rfc3339(created_at),
        "CrossRevisionId": book_id,
        "Id": book_id,
        "IsRemoved": is_removed,
        "IsHiddenFromArchive": false,
        "IsLocked": false,
        "LastModified": format_rfc3339(updated_at),
        "OriginCategory": "Imported",
        "RevisionId": book_id,
        "Status": "Active",
    })
}

/// Mark a book as removed (soft delete).
pub async fn remove_book_entitlement(pool: &PgPool, book_id: Uuid) -> Result<(), sqlx::Error> {
    entitlements::mark_removed_by_book(pool, book_id)
        .await
        .map_err(|e| match e {
            crate::repositories::RepositoryError::Database(e) => e,
            e => sqlx::Error::Protocol(e.to_string()),
        })?;
    Ok(())
}

/// Check if a book exists in our database.
pub async fn book_exists(pool: &PgPool, book_id: Uuid) -> bool {
    books::exists(pool, book_id).await.unwrap_or(false)
}

/// Check if a tag/shelf exists in our database.
pub async fn tag_exists(pool: &PgPool, tag_id: Uuid) -> bool {
    shelves::exists(pool, tag_id).await.unwrap_or(false)
}

/// Check if a series exists in our database.
pub async fn series_exists(pool: &PgPool, series_id: Uuid) -> bool {
    series::exists(pool, series_id).await.unwrap_or(false)
}
