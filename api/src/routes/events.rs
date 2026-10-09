//! Unified SSE event stream for all real-time updates.
//!
//! Includes a batching/deduplication layer to reduce event volume during
//! bulk operations (scans, enrichment). High-volume events are buffered
//! and flushed on a 1-second interval with deduplication. Low-volume
//! events pass through immediately. An idle-flush mechanism preserves
//! snappy feel for individual user actions.

use crate::events::LibraryEvent;
use crate::jobs::JobEvent;
use crate::routes::AuthUser;
use crate::{AppState, IngestQueueEvent};
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use std::collections::HashMap;
use std::convert::Infallible;
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

/// Flush interval for batched events.
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

/// If no event has arrived in this long, the next event triggers an immediate flush.
/// This preserves instant feedback for one-off user actions.
const IDLE_THRESHOLD: Duration = Duration::from_millis(200);

fn serialize_sse(event_name: &str, event: &impl serde::Serialize) -> Option<Event> {
    serde_json::to_string(event)
        .ok()
        .map(|data| Event::default().event(event_name).data(data))
}

/// GET /api/v1/admin/events — unified SSE stream for library, job, and ingest queue events.
///
/// Events are batched and deduplicated to reduce wire volume during bulk operations:
/// - Library events: collapsed to one per kind per flush window
/// - Job events: deduplicated by job_id, keeping the latest state
/// - Ingest queue events: deduplicated by item_id, keeping the latest status
/// - Low-volume events (kobload, notifications, wanted status, proxy log): passed through immediately
pub async fn events_stream(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let mut library_rx = state.library_events.subscribe();
    let mut job_rx = state.job_events.subscribe();
    let mut ingest_rx = state.ingest_queue_events.subscribe();

    let stream = async_stream::stream! {
        let mut flush_interval = tokio::time::interval(FLUSH_INTERVAL);
        // Skip the first immediate tick
        flush_interval.tick().await;

        // Buffers for deduplication (keeps last event per key)
        let mut lib_buffer: HashMap<&'static str, LibraryEvent> = HashMap::new();
        let mut job_buffer: HashMap<Uuid, JobEvent> = HashMap::new();
        let mut ingest_buffer: HashMap<Uuid, IngestQueueEvent> = HashMap::new();
        let mut last_event_time = Instant::now();
        let mut was_idle = true;

        loop {
            tokio::select! {
                biased;

                // Flush on interval tick
                _ = flush_interval.tick() => {
                    // Flush library buffer: one event per kind (last one wins)
                    for (kind, event) in lib_buffer.drain() {
                        if let Some(sse) = serialize_sse(kind, &event) {
                            yield Ok(sse);
                        }
                    }

                    // Flush job buffer: one event per job_id (latest state)
                    for (_, event) in job_buffer.drain() {
                        if let Some(sse) = serialize_sse("job", &event) {
                            yield Ok(sse);
                        }
                    }

                    // Flush ingest buffer: one event per item_id (latest status)
                    for (_, event) in ingest_buffer.drain() {
                        if let Some(sse) = serialize_sse("ingest_queue", &event) {
                            yield Ok(sse);
                        }
                    }

                    // Mark as idle if no events arrived recently
                    if last_event_time.elapsed() > IDLE_THRESHOLD {
                        was_idle = true;
                    }
                }

                // Library events
                result = library_rx.recv() => {
                    match result {
                        Ok(event) => {
                            if event.addressed_to().is_some_and(|u| u != auth.user_id) {
                                continue;
                            }
                            let now = Instant::now();

                            if event.is_batchable() {
                                // If we were idle, flush immediately (snappy single-action feel)
                                if was_idle {
                                    let kind = event.kind();
                                    if let Some(sse) = serialize_sse(kind, &event) {
                                        yield Ok(sse);
                                    }
                                } else {
                                    lib_buffer.insert(event.kind(), event);
                                }
                            } else {
                                // Low-volume: pass through immediately
                                let kind = event.kind();
                                if let Some(sse) = serialize_sse(kind, &event) {
                                    yield Ok(sse);
                                }
                            }

                            last_event_time = now;
                            was_idle = false;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(lagged = n, "SSE library event stream lagged");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }

                // Job events
                result = job_rx.recv() => {
                    match result {
                        Ok(event) => {
                            let now = Instant::now();

                            if was_idle {
                                if let Some(sse) = serialize_sse("job", &event) {
                                    yield Ok(sse);
                                }
                            } else {
                                job_buffer.insert(event.job_id, event);
                            }

                            last_event_time = now;
                            was_idle = false;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(lagged = n, "SSE job event stream lagged");
                            // One-shot resync: clients treat this as "refetch the
                            // running count" since buffered job events were lost.
                            if let Some(sse) = serialize_sse("jobs_resync", &serde_json::json!({ "lagged": n })) {
                                yield Ok(sse);
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }

                // Ingest queue events
                result = ingest_rx.recv() => {
                    match result {
                        Ok(event) => {
                            let now = Instant::now();

                            if was_idle {
                                if let Some(sse) = serialize_sse("ingest_queue", &event) {
                                    yield Ok(sse);
                                }
                            } else {
                                ingest_buffer.insert(event.item_id, event);
                            }

                            last_event_time = now;
                            was_idle = false;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(lagged = n, "SSE ingest queue event stream lagged");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    };

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("ping"),
    )
}
