use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;
use uuid::Uuid;

#[tokio::test]
async fn test_create_tag() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "tagcreate@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create a tag
    let resp = server
        .kobo_client()
        .kobo_create_tag(
            &device_token,
            &serde_json::json!({
                "Name": "My Reading List"
            }),
        )
        .await;
    resp.assert_ok();

    // Response should be the tag ID as a string
    let tag_id: String = resp.json();
    assert!(!tag_id.is_empty());
    assert!(Uuid::parse_str(&tag_id).is_ok());
}

#[tokio::test]
async fn test_create_tag_with_items() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "tagitems@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create a book to add to the tag
    let book_id = create_book(&server, "Tag Item Book").await;
    entitle_book(&server, book_id).await;

    // Create a tag with the book
    let resp = server
        .kobo_client()
        .kobo_create_tag(
            &device_token,
            &serde_json::json!({
                "Name": "Books With Items",
                "Items": [{
                    "RevisionId": book_id.to_string(),
                    "Type": "ProductRevisionTagItem"
                }]
            }),
        )
        .await;
    resp.assert_ok();
}

#[tokio::test]
async fn test_new_tag_appears_in_sync() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "tagsync@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Initial sync (captures token)
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");

    // Create a tag
    let resp = server
        .kobo_client()
        .kobo_create_tag(
            &device_token,
            &serde_json::json!({
                "Name": "Sync Test Tag"
            }),
        )
        .await;
    resp.assert_ok();
    let tag_id = resp.json::<String>();

    // Tag should appear in next sync with token
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await
        .assert_ok()
        .json();

    let tag_entry = items.iter().find(|item| {
        item.get("NewTag")
            .and_then(|t| t.get("Tag"))
            .and_then(|t| t.get("Id"))
            .and_then(|id| id.as_str())
            .map(|id| id == tag_id)
            .unwrap_or(false)
    });
    assert!(
        tag_entry.is_some(),
        "New tag should appear in sync as NewTag"
    );

    let tag = tag_entry.unwrap()["NewTag"]["Tag"].clone();
    assert_eq!(tag["Name"].as_str().unwrap(), "Sync Test Tag");
}

#[tokio::test]
async fn test_rename_tag() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "tagrename@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create a tag
    let resp = server
        .kobo_client()
        .kobo_create_tag(
            &device_token,
            &serde_json::json!({
                "Name": "Original Name"
            }),
        )
        .await;
    resp.assert_ok();
    let tag_id = resp.json::<String>();

    // Rename the tag (no sync in between - tests the rename API works)
    let resp = server
        .kobo_client()
        .kobo_rename_tag(
            &device_token,
            &tag_id,
            &serde_json::json!({
                "Name": "New Name"
            }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Tag should appear in sync with new name
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    let tag_entry = items.iter().find(|item| {
        item.get("NewTag")
            .and_then(|t| t.get("Tag"))
            .and_then(|t| t.get("Id"))
            .and_then(|id| id.as_str())
            .map(|id| id == tag_id)
            .unwrap_or(false)
    });
    assert!(tag_entry.is_some(), "Tag should appear in sync");

    let tag = tag_entry.unwrap()["NewTag"]["Tag"].clone();
    assert_eq!(tag["Name"].as_str().unwrap(), "New Name");
}

#[tokio::test]
async fn test_delete_tag() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "tagdelete@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create a tag
    let resp = server
        .kobo_client()
        .kobo_create_tag(
            &device_token,
            &serde_json::json!({
                "Name": "To Be Deleted"
            }),
        )
        .await;
    resp.assert_ok();
    let tag_id = resp.json::<String>();

    // Sync to get initial state and token
    let resp = server.kobo_client().kobo_sync(&device_token).await;
    resp.assert_ok();
    let token = resp.get_sync_token().expect("should have sync token");

    // Delete the tag
    let resp = server
        .kobo_client()
        .kobo_delete_tag(&device_token, &tag_id)
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Deleted tag should appear in sync with token as DeletedTag
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync_with_token(&device_token, &token)
        .await
        .assert_ok()
        .json();

    let tag_entry = items.iter().find(|item| {
        item.get("DeletedTag")
            .and_then(|t| t.get("Tag"))
            .and_then(|t| t.get("Id"))
            .and_then(|id| id.as_str())
            .map(|id| id == tag_id)
            .unwrap_or(false)
    });
    assert!(
        tag_entry.is_some(),
        "Deleted tag should appear in sync as DeletedTag"
    );
}

#[tokio::test]
async fn test_add_items_to_tag() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "tagadd@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create a book
    let book_id = create_book(&server, "Tag Add Book").await;
    entitle_book(&server, book_id).await;

    // Create an empty tag
    let resp = server
        .kobo_client()
        .kobo_create_tag(
            &device_token,
            &serde_json::json!({
                "Name": "Empty Tag"
            }),
        )
        .await;
    resp.assert_ok();
    let tag_id = resp.json::<String>();

    // Add the book to the tag (no sync in between - tests the add API works)
    let resp = server
        .kobo_client()
        .kobo_add_tag_items(
            &device_token,
            &tag_id,
            &serde_json::json!({
                "Name": "Empty Tag",
                "Items": [{
                    "RevisionId": book_id.to_string(),
                    "Type": "ProductRevisionTagItem"
                }]
            }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Tag should appear in sync with the item
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    let tag_entry = items.iter().find(|item| {
        item.get("NewTag")
            .and_then(|t| t.get("Tag"))
            .and_then(|t| t.get("Id"))
            .and_then(|id| id.as_str())
            .map(|id| id == tag_id)
            .unwrap_or(false)
    });
    assert!(tag_entry.is_some(), "Tag should appear in sync");

    let tag = tag_entry.unwrap()["NewTag"]["Tag"].clone();
    let tag_items = tag["Items"].as_array().unwrap();
    assert_eq!(tag_items.len(), 1);
    assert_eq!(
        tag_items[0]["RevisionId"].as_str().unwrap(),
        book_id.to_string()
    );
}

#[tokio::test]
async fn test_remove_items_from_tag() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "tagremove@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create a book
    let book_id = create_book(&server, "Tag Remove Book").await;
    entitle_book(&server, book_id).await;

    // Create a tag with the book
    let resp = server
        .kobo_client()
        .kobo_create_tag(
            &device_token,
            &serde_json::json!({
                "Name": "Tag With Book",
                "Items": [{
                    "RevisionId": book_id.to_string(),
                    "Type": "ProductRevisionTagItem"
                }]
            }),
        )
        .await;
    resp.assert_ok();
    let tag_id = resp.json::<String>();

    // Remove the book from the tag (no sync in between - tests the remove API works)
    let resp = server
        .kobo_client()
        .kobo_remove_tag_items(
            &device_token,
            &tag_id,
            &serde_json::json!({
                "Name": "Tag With Book",
                "Items": [{
                    "RevisionId": book_id.to_string(),
                    "Type": "ProductRevisionTagItem"
                }]
            }),
        )
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Tag should appear in sync with no items
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device_token)
        .await
        .assert_ok()
        .json();

    let tag_entry = items.iter().find(|item| {
        item.get("NewTag")
            .and_then(|t| t.get("Tag"))
            .and_then(|t| t.get("Id"))
            .and_then(|id| id.as_str())
            .map(|id| id == tag_id)
            .unwrap_or(false)
    });
    assert!(tag_entry.is_some(), "Tag should appear in sync");

    let tag = tag_entry.unwrap()["NewTag"]["Tag"].clone();
    let tag_items = tag["Items"].as_array().unwrap();
    assert!(
        tag_items.is_empty(),
        "Tag should have no items after removal"
    );
}

#[tokio::test]
async fn test_tag_unauthorized() {
    let server = TestServer::start().await;

    // Try to create a tag with an invalid device token
    let resp = server
        .kobo_client()
        .kobo_create_tag(
            "invalid-token",
            &serde_json::json!({
                "Name": "Should Fail"
            }),
        )
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_tags_scoped_to_user() {
    let server = TestServer::start().await;

    // User 1 creates a tag
    let user1_id = create_user(&server, "taguser1@test.com").await;
    let (_, device1_token) = create_device(&server, user1_id).await;

    let resp = server
        .kobo_client()
        .kobo_create_tag(
            &device1_token,
            &serde_json::json!({
                "Name": "User1 Tag"
            }),
        )
        .await;
    resp.assert_ok();
    let tag_id = resp.json::<String>();

    // User 2 should not see user1's tag
    let user2_id = create_user(&server, "taguser2@test.com").await;
    let (_, device2_token) = create_device(&server, user2_id).await;

    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device2_token)
        .await
        .assert_ok()
        .json();

    let has_tag = items.iter().any(|item| {
        item.get("NewTag")
            .or_else(|| item.get("ChangedTag"))
            .and_then(|t| t.get("Tag"))
            .and_then(|t| t.get("Id"))
            .and_then(|id| id.as_str())
            .map(|id| id == tag_id)
            .unwrap_or(false)
    });
    assert!(!has_tag, "User2 should not see User1's tag");

    // User 1 should see their tag
    let items: Vec<Value> = server
        .kobo_client()
        .kobo_sync(&device1_token)
        .await
        .assert_ok()
        .json();

    let has_tag = items.iter().any(|item| {
        item.get("NewTag")
            .and_then(|t| t.get("Tag"))
            .and_then(|t| t.get("Id"))
            .and_then(|id| id.as_str())
            .map(|id| id == tag_id)
            .unwrap_or(false)
    });
    assert!(has_tag, "User1 should see their own tag");
}
