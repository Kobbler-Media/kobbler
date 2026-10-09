//! Bridge between Kobload SSE events and Kobbler event broadcasts.
//!
//! This module subscribes to Kobload's `/api/v1/downloads/stream` SSE endpoint
//! and broadcasts events to connected SSE clients via the unified event stream.

use std::time::Duration;

use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::broadcast;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::events::LibraryEvent;
use crate::runtime_config::RuntimeConfigManager;
use crate::sse_client::{SseError, SseEvent, SseStream};

/// Events from Kobload's SSE stream
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum KobloadStreamEvent {
    Snapshot {
        downloads: Vec<serde_json::Value>,
    },
    Created {
        download: serde_json::Value,
    },
    Updated {
        download: serde_json::Value,
    },
    StreamWarmReady {
        info_hash: String,
        file_id: Option<i32>,
        debrid_provider_id: String,
        debrid_provider: String,
    },
}

/// Run the Kobload bridge, subscribing to Kobload SSE and broadcasting to clients.
///
/// This function runs forever, reconnecting if the SSE connection drops.
pub async fn run_kobload_bridge(
    runtime_config: RuntimeConfigManager,
    ws_broadcast: broadcast::Sender<LibraryEvent>,
) {
    loop {
        // Get current config
        let config = runtime_config.get().await;
        let Some(base_url) = config.kobload_base_url.filter(|s| !s.is_empty()) else {
            // No Kobload configured, wait and retry
            sleep(Duration::from_secs(30)).await;
            continue;
        };
        let Some(api_key) = config.kobload_api_key.filter(|s| !s.is_empty()) else {
            // No API key configured, wait and retry
            sleep(Duration::from_secs(30)).await;
            continue;
        };

        let url = format!("{}/api/v1/downloads/stream", base_url.trim_end_matches('/'));

        info!(url = %url, "Connecting to Kobload SSE stream");

        // Create SSE client with auth header
        let client = crate::http::builder()
            .timeout(Duration::MAX)
            .build()
            .expect("Failed to create SSE HTTP client");
        let request = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Accept", "text/event-stream");

        let mut es = match SseStream::connect(request).await {
            Ok(es) => es,
            Err(SseError::Transport(e)) => {
                warn!(error = %e, "Failed to connect to Kobload SSE, will retry");
                sleep(Duration::from_secs(5)).await;
                continue;
            }
            Err(e) => {
                warn!(error = %e, "Failed to connect to Kobload SSE, will retry");
                sleep(Duration::from_secs(5)).await;
                continue;
            }
        };

        while let Some(event) = es.next().await {
            match event {
                Ok(SseEvent::Open) => {
                    debug!("Kobload SSE connection opened");
                }
                Ok(SseEvent::Message { ref data, .. }) => {
                    if let Err(e) = handle_sse_message(data, &ws_broadcast) {
                        debug!(error = %e, "Failed to parse Kobload SSE message");
                    }
                }
                Err(SseError::StreamEnded) => {
                    info!("Kobload SSE stream ended, will reconnect");
                    es.close();
                    break;
                }
                Err(SseError::Transport(ref e)) if e.is_timeout() || e.is_connect() => {
                    warn!(error = %e, "Kobload SSE connection lost, will reconnect");
                    es.close();
                    break;
                }
                Err(err) => {
                    warn!(error = %err, "Kobload SSE error, will reconnect");
                    es.close();
                    break;
                }
            }
        }

        // Wait before reconnecting
        sleep(Duration::from_secs(5)).await;
    }
}

fn handle_sse_message(
    data: &str,
    ws_broadcast: &broadcast::Sender<LibraryEvent>,
) -> Result<(), serde_json::Error> {
    let event: KobloadStreamEvent = serde_json::from_str(data)?;

    let (event_type, count) = match &event {
        KobloadStreamEvent::Snapshot { downloads } => ("snapshot", downloads.len()),
        KobloadStreamEvent::Created { .. } => ("created", 1),
        KobloadStreamEvent::Updated { .. } => ("updated", 1),
        KobloadStreamEvent::StreamWarmReady { .. } => ("stream_warm_ready", 1),
    };

    info!(
        event_type = %event_type,
        count = %count,
        receivers = %ws_broadcast.receiver_count(),
        "Received Kobload SSE event, broadcasting to clients"
    );

    let library_event = match event {
        KobloadStreamEvent::Snapshot { downloads } => {
            LibraryEvent::kobload_download_snapshot(downloads)
        }
        KobloadStreamEvent::Created { download } => {
            LibraryEvent::kobload_download_created(download)
        }
        KobloadStreamEvent::Updated { download } => {
            LibraryEvent::kobload_download_updated(download)
        }
        KobloadStreamEvent::StreamWarmReady {
            info_hash,
            file_id,
            debrid_provider_id,
            debrid_provider,
        } => {
            LibraryEvent::stream_warm_ready(info_hash, file_id, debrid_provider_id, debrid_provider)
        }
    };

    // Broadcast to all connected SSE clients (ignore send errors - no receivers)
    let _ = ws_broadcast.send(library_event);

    Ok(())
}
