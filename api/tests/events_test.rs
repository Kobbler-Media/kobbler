use crate::common::*;
use api::sse_client::{SseEvent, SseStream};
use futures_util::StreamExt;
use reqwest::StatusCode;
use std::time::Duration;
use uuid::Uuid;

/// Connect to the unified SSE events stream with admin auth.
async fn connect_events(server: &TestServer) -> SseStream {
    let url = format!("{}/api/v1/admin/events", server.api_url);
    let client = reqwest::Client::new();
    let request = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", server.admin_token));
    SseStream::connect(request)
        .await
        .expect("Failed to connect SSE")
}

/// Read the next named SSE event, skipping Open events.
/// Returns (event_name, data_string) or None on timeout.
async fn next_event(es: &mut SseStream, timeout_ms: u64) -> Option<(String, String)> {
    tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        loop {
            match es.next().await {
                Some(Ok(SseEvent::Open)) => continue,
                Some(Ok(SseEvent::Message { event, data })) => return Some((event, data)),
                _ => return None,
            }
        }
    })
    .await
    .ok()
    .flatten()
}

/// Wait for the SSE connection to open.
async fn wait_for_open(es: &mut SseStream) {
    tokio::time::timeout(Duration::from_millis(2000), async {
        loop {
            match es.next().await {
                Some(Ok(SseEvent::Open)) => return,
                _ => continue,
            }
        }
    })
    .await
    .expect("SSE connection did not open in time");
}

#[tokio::test]
async fn test_events_stream_requires_auth() {
    let server = TestServer::start_api_only().await;

    let resp = server.api_client().get("/api/v1/admin/events").await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_events_stream_connects() {
    let server = TestServer::start_api_only().await;

    let mut es = connect_events(&server).await;
    wait_for_open(&mut es).await;
    es.close();
}

#[tokio::test]
async fn test_events_stream_receives_library_events() {
    let server = TestServer::start_api_only().await;

    let mut es = connect_events(&server).await;
    wait_for_open(&mut es).await;

    // Broadcast a library_updated event
    let book_id = Uuid::new_v4();
    let _ = server
        .library_events
        .send(api::LibraryEvent::library_updated(Some(book_id)));

    let (event_name, data) = next_event(&mut es, 2000)
        .await
        .expect("Did not receive library_updated event");

    assert_eq!(event_name, "library_updated");
    let parsed: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(parsed["kind"], "library_updated");
    assert_eq!(parsed["book_id"], book_id.to_string());

    es.close();
}

#[tokio::test]
async fn test_events_stream_receives_job_events() {
    let server = TestServer::start_api_only().await;

    let mut es = connect_events(&server).await;
    wait_for_open(&mut es).await;

    // Broadcast a job event
    let job_id = Uuid::new_v4();
    let _ = server.job_events.send(api::jobs::JobEvent {
        job_id,
        job_type: "test_job".to_string(),
        status: "running".to_string(),
        progress: Some(50),
        error: None,
        result: None,
        description: Some("Test job".to_string()),
        entity_type: None,
        entity_id: None,
        display: None,
        created_at: time::OffsetDateTime::now_utc(),
        scheduled: false,
    });

    let (event_name, data) = next_event(&mut es, 2000)
        .await
        .expect("Did not receive job event");

    assert_eq!(event_name, "job");
    let parsed: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(parsed["job_id"], job_id.to_string());
    assert_eq!(parsed["job_type"], "test_job");
    assert_eq!(parsed["status"], "running");
    assert_eq!(parsed["progress"], 50);

    es.close();
}

#[tokio::test]
async fn test_events_stream_receives_ingest_queue_events() {
    let server = TestServer::start_api_only().await;

    let mut es = connect_events(&server).await;
    wait_for_open(&mut es).await;

    // Broadcast an ingest queue event
    let item_id = Uuid::new_v4();
    let _ = server.ingest_queue_events.send(api::IngestQueueEvent {
        item_id,
        status: "processing".to_string(),
    });

    let (event_name, data) = next_event(&mut es, 2000)
        .await
        .expect("Did not receive ingest_queue event");

    assert_eq!(event_name, "ingest_queue");
    let parsed: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(parsed["item_id"], item_id.to_string());
    assert_eq!(parsed["status"], "processing");

    es.close();
}

#[tokio::test]
async fn test_events_stream_receives_multiple_event_types() {
    let server = TestServer::start_api_only().await;

    let mut es = connect_events(&server).await;
    wait_for_open(&mut es).await;

    // Send events from all three channels
    let book_id = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let item_id = Uuid::new_v4();

    let _ = server
        .library_events
        .send(api::LibraryEvent::library_updated(Some(book_id)));
    let _ = server.job_events.send(api::jobs::JobEvent {
        job_id,
        job_type: "multi_test".to_string(),
        status: "completed".to_string(),
        progress: None,
        error: None,
        result: None,
        description: None,
        entity_type: None,
        entity_id: None,
        display: None,
        created_at: time::OffsetDateTime::now_utc(),
        scheduled: false,
    });
    let _ = server.ingest_queue_events.send(api::IngestQueueEvent {
        item_id,
        status: "done".to_string(),
    });

    // Collect all three events (order may vary due to stream merging)
    let mut received = Vec::new();
    for _ in 0..3 {
        if let Some(event) = next_event(&mut es, 2000).await {
            received.push(event);
        }
    }

    assert_eq!(
        received.len(),
        3,
        "Expected 3 events, got {}",
        received.len()
    );

    let event_names: Vec<&str> = received.iter().map(|(name, _)| name.as_str()).collect();
    assert!(
        event_names.contains(&"library_updated"),
        "Missing library_updated"
    );
    assert!(event_names.contains(&"job"), "Missing job");
    assert!(
        event_names.contains(&"ingest_queue"),
        "Missing ingest_queue"
    );

    es.close();
}

#[tokio::test]
async fn test_events_stream_library_event_kinds() {
    let server = TestServer::start_api_only().await;

    let mut es = connect_events(&server).await;
    wait_for_open(&mut es).await;

    // Test that different LibraryEvent variants produce correct SSE event names
    let author_id = Uuid::new_v4();
    let series_id = Uuid::new_v4();

    let _ = server
        .library_events
        .send(api::LibraryEvent::author_updated(author_id));
    let _ = server
        .library_events
        .send(api::LibraryEvent::series_updated(series_id));

    let (name1, data1) = next_event(&mut es, 2000)
        .await
        .expect("Did not receive author_updated event");
    let (name2, data2) = next_event(&mut es, 2000)
        .await
        .expect("Did not receive series_updated event");

    assert_eq!(name1, "author_updated");
    let parsed1: serde_json::Value = serde_json::from_str(&data1).unwrap();
    assert_eq!(parsed1["author_id"], author_id.to_string());

    assert_eq!(name2, "series_updated");
    let parsed2: serde_json::Value = serde_json::from_str(&data2).unwrap();
    assert_eq!(parsed2["series_id"], series_id.to_string());

    es.close();
}
