use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;

#[tokio::test]
async fn test_get_reading_state_creates_if_not_exists() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "readstate@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create and entitle a book
    let book_id = create_book(&server, "Reading State Book").await;
    entitle_book(&server, book_id).await;

    // Get reading state (should auto-create)
    let resp = server
        .kobo_client()
        .kobo_get_state(&device_token, &book_id.to_string())
        .await;
    resp.assert_ok();

    let states: Vec<Value> = resp.json();
    assert_eq!(states.len(), 1);

    let state = &states[0];
    assert_eq!(
        state["EntitlementId"].as_str().unwrap(),
        book_id.to_string()
    );
    assert_eq!(
        state["StatusInfo"]["Status"].as_str().unwrap(),
        "ReadyToRead"
    );
}

#[tokio::test]
async fn test_update_reading_state_progress() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "progress@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Progress Book").await;
    entitle_book(&server, book_id).await;

    // Update reading progress
    let resp = server
        .kobo_client()
        .kobo_update_state(
            &device_token,
            &book_id.to_string(),
            &serde_json::json!({
                "ReadingStates": [{
                    "CurrentBookmark": {
                        "ProgressPercent": 50.0,
                        "ContentSourceProgressPercent": 50.0,
                        "Location": {
                            "Value": "chapter-5",
                            "Type": "Location",
                            "Source": "Kobo"
                        }
                    }
                }]
            }),
        )
        .await;
    resp.assert_ok();

    let result: Value = resp.json();
    assert_eq!(result["RequestResult"].as_str().unwrap(), "Success");
    assert!(
        result["UpdateResults"][0]["CurrentBookmarkResult"]["Result"]
            .as_str()
            .unwrap()
            == "Success"
    );

    // Verify the state was saved
    let resp = server
        .kobo_client()
        .kobo_get_state(&device_token, &book_id.to_string())
        .await;
    resp.assert_ok();

    let states: Vec<Value> = resp.json();
    let state = &states[0];
    assert_eq!(
        state["CurrentBookmark"]["ProgressPercent"]
            .as_f64()
            .unwrap(),
        50.0
    );
}

#[tokio::test]
async fn test_update_reading_state_status() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "status@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Status Book").await;
    entitle_book(&server, book_id).await;

    // Mark book as reading
    let resp = server
        .kobo_client()
        .kobo_update_state(
            &device_token,
            &book_id.to_string(),
            &serde_json::json!({
                "ReadingStates": [{
                    "StatusInfo": {
                        "Status": "Reading"
                    }
                }]
            }),
        )
        .await;
    resp.assert_ok();

    let result: Value = resp.json();
    assert_eq!(result["RequestResult"].as_str().unwrap(), "Success");

    // Verify the status was updated
    let resp = server
        .kobo_client()
        .kobo_get_state(&device_token, &book_id.to_string())
        .await;
    resp.assert_ok();

    let states: Vec<Value> = resp.json();
    let state = &states[0];
    assert_eq!(state["StatusInfo"]["Status"].as_str().unwrap(), "Reading");
}

#[tokio::test]
async fn test_update_reading_state_statistics() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "statistics@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Statistics Book").await;
    entitle_book(&server, book_id).await;

    // Update reading statistics
    let resp = server
        .kobo_client()
        .kobo_update_state(
            &device_token,
            &book_id.to_string(),
            &serde_json::json!({
                "ReadingStates": [{
                    "Statistics": {
                        "SpentReadingMinutes": 120,
                        "RemainingTimeMinutes": 60
                    }
                }]
            }),
        )
        .await;
    resp.assert_ok();

    let result: Value = resp.json();
    assert_eq!(result["RequestResult"].as_str().unwrap(), "Success");
    assert!(
        result["UpdateResults"][0]["StatisticsResult"]["Result"]
            .as_str()
            .unwrap()
            == "Success"
    );

    // Verify the statistics were saved
    let resp = server
        .kobo_client()
        .kobo_get_state(&device_token, &book_id.to_string())
        .await;
    resp.assert_ok();

    let states: Vec<Value> = resp.json();
    let state = &states[0];
    assert_eq!(
        state["Statistics"]["SpentReadingMinutes"].as_i64().unwrap(),
        120
    );
    assert_eq!(
        state["Statistics"]["RemainingTimeMinutes"]
            .as_i64()
            .unwrap(),
        60
    );
}

#[tokio::test]
async fn test_reading_state_works_for_unentitled_book() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "nobook@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    // Create a book but DON'T entitle it - book exists in DB
    // Reading state is still created because the book exists
    let book_id = create_book(&server, "No Entitlement Book").await;

    // Get reading state - works because book exists in books table
    let resp = server
        .kobo_client()
        .kobo_get_state(&device_token, &book_id.to_string())
        .await;
    resp.assert_ok();

    // Reading state is auto-created even without entitlement
    let states: Vec<Value> = resp.json();
    assert_eq!(states.len(), 1);
    assert_eq!(
        states[0]["StatusInfo"]["Status"].as_str().unwrap(),
        "ReadyToRead"
    );
}

#[tokio::test]
async fn test_reading_state_requires_valid_device() {
    let server = TestServer::start().await;

    let book_id = uuid::Uuid::new_v4();

    // Invalid device token should fail
    let resp = server
        .kobo_client()
        .kobo_get_state("invalid-token", &book_id.to_string())
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_reading_state_scoped_to_device() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "devicescope@test.com").await;
    let (_, device1_token) = create_device(&server, user_id).await;
    let (_, device2_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Scoped Book").await;
    entitle_book(&server, book_id).await;

    // Update reading state on device 1
    server
        .kobo_client()
        .kobo_update_state(
            &device1_token,
            &book_id.to_string(),
            &serde_json::json!({
                "ReadingStates": [{
                    "CurrentBookmark": {
                        "ProgressPercent": 75.0
                    },
                    "StatusInfo": {
                        "Status": "Reading"
                    }
                }]
            }),
        )
        .await
        .assert_ok();

    // Device 1 should see the progress
    let resp = server
        .kobo_client()
        .kobo_get_state(&device1_token, &book_id.to_string())
        .await;
    let states: Vec<Value> = resp.json();
    assert_eq!(
        states[0]["CurrentBookmark"]["ProgressPercent"]
            .as_f64()
            .unwrap(),
        75.0
    );
    assert_eq!(
        states[0]["StatusInfo"]["Status"].as_str().unwrap(),
        "Reading"
    );

    // Device 2 should see device 1's progress (cross-device sync)
    let resp = server
        .kobo_client()
        .kobo_get_state(&device2_token, &book_id.to_string())
        .await;
    let states: Vec<Value> = resp.json();
    assert_eq!(
        states[0]["StatusInfo"]["Status"].as_str().unwrap(),
        "Reading"
    );
    assert_eq!(
        states[0]["CurrentBookmark"]["ProgressPercent"]
            .as_f64()
            .unwrap(),
        75.0
    );
}

/// Regression: Kobo Nickel diffs GET responses against its local memory of the
/// last PUT. Any field Nickel didn't write (LastModified rewritten to NOW(),
/// `17` reserialized as `17.0`, injected `TimesStartedReading`) trips the
/// "continue reading from another device?" prompt. The GET response for the
/// PUT-ing device must be byte-identical to the bytes it sent.
#[tokio::test]
async fn test_put_state_echoes_client_payload_verbatim() {
    let server = TestServer::start().await;
    let user_id = create_user(&server, "echo@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;
    let book_id = create_book(&server, "Echo Book").await;
    entitle_book(&server, book_id).await;

    // Integer literal (not 17.0) and explicit LastModified — the exact fields
    // Nickel inspects after a GET.
    let put_body = serde_json::json!({
        "ReadingStates": [{
            "CurrentBookmark": {
                "ProgressPercent": 17,
                "LastModified": "2026-04-14T14:16:31Z",
                "Location": {
                    "Value": "kobo.1.1",
                    "Type": "KoboSpan",
                    "Source": "OEBPS/9780765326355_ch16.html"
                }
            },
            "Statistics": {
                "LastModified": "2026-04-14T14:16:31Z",
                "SpentReadingMinutes": 165,
                "RemainingTimeMinutes": 1316
            },
            "StatusInfo": {
                "LastModified": "2026-04-14T14:16:31Z",
                "Status": "Reading"
            }
        }]
    });

    server
        .kobo_client()
        .kobo_update_state(&device_token, &book_id.to_string(), &put_body)
        .await
        .assert_ok();

    let resp = server
        .kobo_client()
        .kobo_get_state(&device_token, &book_id.to_string())
        .await;
    resp.assert_ok();
    let states: Vec<Value> = resp.json();
    let state = &states[0];

    // Integer stays integer — no .0 reserialization.
    assert!(
        state["CurrentBookmark"]["ProgressPercent"].is_i64(),
        "ProgressPercent must round-trip as integer, got {:?}",
        state["CurrentBookmark"]["ProgressPercent"]
    );
    assert_eq!(
        state["CurrentBookmark"]["ProgressPercent"]
            .as_i64()
            .unwrap(),
        17
    );

    // LastModified must match the exact string Nickel sent, not a μs-precision NOW().
    assert_eq!(
        state["CurrentBookmark"]["LastModified"].as_str().unwrap(),
        "2026-04-14T14:16:31Z"
    );
    assert_eq!(
        state["Statistics"]["LastModified"].as_str().unwrap(),
        "2026-04-14T14:16:31Z"
    );
    assert_eq!(
        state["StatusInfo"]["LastModified"].as_str().unwrap(),
        "2026-04-14T14:16:31Z"
    );

    // No injected TimesStartedReading that the PUT didn't send.
    assert!(
        state["StatusInfo"].get("TimesStartedReading").is_none(),
        "server injected TimesStartedReading that the client never sent"
    );
}

/// A second PUT with the same progress but newer LastModified timestamps must
/// still update the stored client_payload so the echo tracks the latest bytes.
/// Otherwise Nickel would see its own-but-stale timestamps and still prompt.
#[tokio::test]
async fn test_noop_put_refreshes_timestamps_in_echo() {
    let server = TestServer::start().await;
    let user_id = create_user(&server, "noop@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;
    let book_id = create_book(&server, "Noop Book").await;
    entitle_book(&server, book_id).await;

    let put1 = serde_json::json!({
        "ReadingStates": [{
            "CurrentBookmark": {
                "ProgressPercent": 17,
                "LastModified": "2026-04-14T14:16:31Z"
            }
        }]
    });
    server
        .kobo_client()
        .kobo_update_state(&device_token, &book_id.to_string(), &put1)
        .await
        .assert_ok();

    let put2 = serde_json::json!({
        "ReadingStates": [{
            "CurrentBookmark": {
                "ProgressPercent": 17,
                "LastModified": "2026-04-14T15:00:00Z"
            }
        }]
    });
    server
        .kobo_client()
        .kobo_update_state(&device_token, &book_id.to_string(), &put2)
        .await
        .assert_ok();

    let resp = server
        .kobo_client()
        .kobo_get_state(&device_token, &book_id.to_string())
        .await;
    let states: Vec<Value> = resp.json();
    assert_eq!(
        states[0]["CurrentBookmark"]["LastModified"]
            .as_str()
            .unwrap(),
        "2026-04-14T15:00:00Z"
    );
}

/// Within-epsilon cross-device drift must not be served to the other device —
/// otherwise rounding noise between Kobo's position-based % and Hardcover's
/// page-based % trips Nickel's prompt for nothing.
#[tokio::test]
async fn test_sub_tolerance_cross_device_no_prompt() {
    let server = TestServer::start().await;
    let user_id = create_user(&server, "subtol@test.com").await;
    let (_, device1_token) = create_device(&server, user_id).await;
    let (_, device2_token) = create_device(&server, user_id).await;
    let book_id = create_book(&server, "SubTol Book").await;
    entitle_book(&server, book_id).await;

    // Device A @ 17% with a distinctive LastModified we can fingerprint the echo by.
    server
        .kobo_client()
        .kobo_update_state(
            &device1_token,
            &book_id.to_string(),
            &serde_json::json!({
                "ReadingStates": [{
                    "CurrentBookmark": {
                        "ProgressPercent": 17,
                        "LastModified": "2026-04-14T10:00:00Z"
                    }
                }]
            }),
        )
        .await
        .assert_ok();

    // Device B @ 17.1% — under the 0.3% epsilon.
    server
        .kobo_client()
        .kobo_update_state(
            &device2_token,
            &book_id.to_string(),
            &serde_json::json!({
                "ReadingStates": [{
                    "CurrentBookmark": {
                        "ProgressPercent": 17.1,
                        "LastModified": "2026-04-14T11:00:00Z"
                    }
                }]
            }),
        )
        .await
        .assert_ok();

    // Device A GETs — should still see its own bytes (17 integer, its own LastModified).
    let resp = server
        .kobo_client()
        .kobo_get_state(&device1_token, &book_id.to_string())
        .await;
    let states: Vec<Value> = resp.json();
    assert!(states[0]["CurrentBookmark"]["ProgressPercent"].is_i64());
    assert_eq!(
        states[0]["CurrentBookmark"]["ProgressPercent"]
            .as_i64()
            .unwrap(),
        17
    );
    assert_eq!(
        states[0]["CurrentBookmark"]["LastModified"]
            .as_str()
            .unwrap(),
        "2026-04-14T10:00:00Z"
    );
}

/// A meaningful cross-device jump (> 0.3%) must be served to the other device
/// so Nickel's legitimate "resume from another device?" flow still fires.
#[tokio::test]
async fn test_meaningful_cross_device_serves_other_bytes() {
    let server = TestServer::start().await;
    let user_id = create_user(&server, "crossdev@test.com").await;
    let (_, device1_token) = create_device(&server, user_id).await;
    let (_, device2_token) = create_device(&server, user_id).await;
    let book_id = create_book(&server, "CrossDev Book").await;
    entitle_book(&server, book_id).await;

    server
        .kobo_client()
        .kobo_update_state(
            &device1_token,
            &book_id.to_string(),
            &serde_json::json!({
                "ReadingStates": [{
                    "CurrentBookmark": {
                        "ProgressPercent": 17,
                        "LastModified": "2026-04-14T10:00:00Z"
                    },
                    "StatusInfo": {
                        "Status": "Reading",
                        "LastModified": "2026-04-14T10:00:00Z"
                    }
                }]
            }),
        )
        .await
        .assert_ok();

    // Device B @ 40% — well over epsilon, a real read-ahead.
    server
        .kobo_client()
        .kobo_update_state(
            &device2_token,
            &book_id.to_string(),
            &serde_json::json!({
                "ReadingStates": [{
                    "CurrentBookmark": {
                        "ProgressPercent": 40,
                        "LastModified": "2026-04-14T11:00:00Z"
                    },
                    "StatusInfo": {
                        "Status": "Reading",
                        "LastModified": "2026-04-14T11:00:00Z"
                    }
                }]
            }),
        )
        .await
        .assert_ok();

    // Device A GETs — should see device B's bytes (40% + B's LastModified).
    let resp = server
        .kobo_client()
        .kobo_get_state(&device1_token, &book_id.to_string())
        .await;
    let states: Vec<Value> = resp.json();
    assert_eq!(
        states[0]["CurrentBookmark"]["ProgressPercent"]
            .as_i64()
            .unwrap(),
        40
    );
    assert_eq!(
        states[0]["CurrentBookmark"]["LastModified"]
            .as_str()
            .unwrap(),
        "2026-04-14T11:00:00Z"
    );
}

/// Partial PUTs (bookmark-only after a full PUT) must preserve unrelated
/// sub-objects in the stored client_payload — whole-sub-object replacement,
/// not top-level clobber.
#[tokio::test]
async fn test_partial_put_merges_sub_objects() {
    let server = TestServer::start().await;
    let user_id = create_user(&server, "partial@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;
    let book_id = create_book(&server, "Partial Book").await;
    entitle_book(&server, book_id).await;

    // Full PUT first.
    server
        .kobo_client()
        .kobo_update_state(
            &device_token,
            &book_id.to_string(),
            &serde_json::json!({
                "ReadingStates": [{
                    "CurrentBookmark": {
                        "ProgressPercent": 17,
                        "LastModified": "2026-04-14T10:00:00Z"
                    },
                    "Statistics": {
                        "SpentReadingMinutes": 165,
                        "RemainingTimeMinutes": 1316,
                        "LastModified": "2026-04-14T10:00:00Z"
                    },
                    "StatusInfo": {
                        "Status": "Reading",
                        "LastModified": "2026-04-14T10:00:00Z"
                    }
                }]
            }),
        )
        .await
        .assert_ok();

    // Bookmark-only PUT — should NOT drop Statistics/StatusInfo from the echo.
    server
        .kobo_client()
        .kobo_update_state(
            &device_token,
            &book_id.to_string(),
            &serde_json::json!({
                "ReadingStates": [{
                    "CurrentBookmark": {
                        "ProgressPercent": 25,
                        "LastModified": "2026-04-14T11:00:00Z"
                    }
                }]
            }),
        )
        .await
        .assert_ok();

    let resp = server
        .kobo_client()
        .kobo_get_state(&device_token, &book_id.to_string())
        .await;
    let states: Vec<Value> = resp.json();
    assert_eq!(
        states[0]["CurrentBookmark"]["ProgressPercent"]
            .as_i64()
            .unwrap(),
        25
    );
    assert_eq!(
        states[0]["Statistics"]["SpentReadingMinutes"]
            .as_i64()
            .unwrap(),
        165
    );
    assert_eq!(
        states[0]["StatusInfo"]["Status"].as_str().unwrap(),
        "Reading"
    );
}

#[tokio::test]
async fn test_mark_book_finished() {
    let server = TestServer::start().await;

    let user_id = create_user(&server, "finished@test.com").await;
    let (_, device_token) = create_device(&server, user_id).await;

    let book_id = create_book(&server, "Finished Book").await;
    entitle_book(&server, book_id).await;

    // Mark book as finished
    let resp = server
        .kobo_client()
        .kobo_update_state(
            &device_token,
            &book_id.to_string(),
            &serde_json::json!({
                "ReadingStates": [{
                    "CurrentBookmark": {
                        "ProgressPercent": 100.0,
                        "ContentSourceProgressPercent": 100.0
                    },
                    "StatusInfo": {
                        "Status": "Finished"
                    }
                }]
            }),
        )
        .await;
    resp.assert_ok();

    // Verify
    let resp = server
        .kobo_client()
        .kobo_get_state(&device_token, &book_id.to_string())
        .await;
    let states: Vec<Value> = resp.json();
    assert_eq!(
        states[0]["StatusInfo"]["Status"].as_str().unwrap(),
        "Finished"
    );
    assert_eq!(
        states[0]["CurrentBookmark"]["ProgressPercent"]
            .as_f64()
            .unwrap(),
        100.0
    );
}
