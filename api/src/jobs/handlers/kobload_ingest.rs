//! Job handler for asynchronous Kobload ingest processing.
//!
//! Receives ingest requests (via NATS subscription or HTTP 202), processes the
//! files, and sends the result back via NATS (`events.ingest.result`) or HTTP
//! (`/api/v1/downloads/{id}/imported` or `/api/v1/downloads/{id}/import-failed`).

use crate::AppState;
use crate::jobs::worker::JobHandler;
use crate::jobs::{JobError, JobResult};
use crate::repositories::deferred_ingests;
use crate::routes::ingest::{IngestOutcome, KobloadCallbackRequest, process_kobload_ingest};
use async_trait::async_trait;
use nats_jobs::{IngestResultEvent, IngestResultStatus};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

pub struct KobloadIngestHandler {
    state: AppState,
}

impl KobloadIngestHandler {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl JobHandler for KobloadIngestHandler {
    fn job_type(&self) -> &'static str {
        "kobload_ingest"
    }

    fn max_concurrent(&self) -> usize {
        2
    }

    async fn execute(
        &self,
        job_id: Uuid,
        payload: Value,
        cancel: CancellationToken,
    ) -> JobResult<Value> {
        nats_jobs::check_cancelled(&cancel)?;
        let request: KobloadCallbackRequest =
            serde_json::from_value(payload.get("request").cloned().unwrap_or(Value::Null))
                .map_err(|e| JobError::ExecutionFailed(format!("deserialize request: {e}")))?;

        let response_mode = payload
            .get("response_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("http")
            .to_string();

        let download_id = request.download_id;

        info!(
            job_id = %job_id,
            download_id = %download_id,
            response_mode = %response_mode,
            "Processing kobload ingest"
        );

        match process_kobload_ingest(&self.state, request, true, cancel).await {
            Ok(IngestOutcome::Imported) => {
                info!(download_id = %download_id, "Kobload ingest succeeded");
                // If this was a previously deferred ingest, clean up the record
                let _ =
                    deferred_ingests::delete_by_download_id(&self.state.pool, download_id).await;
                send_result(
                    &self.state,
                    download_id,
                    &response_mode,
                    IngestResultStatus::Imported,
                    None,
                )
                .await;
                Ok(serde_json::json!({
                    "download_id": download_id,
                    "status": "imported",
                }))
            }
            Ok(IngestOutcome::NotImported) => {
                // Kobload reported a non-completed status, so nothing was placed.
                // Stay silent rather than sending Imported: confirming an import
                // for a failed download stamps `import_confirmed_at` on its watch
                // match, which permanently suppresses the re-search for that item.
                info!(
                    download_id = %download_id,
                    "Kobload ingest acknowledged with nothing imported, not confirming import"
                );
                Ok(serde_json::json!({
                    "download_id": download_id,
                    "status": "not_imported",
                }))
            }
            Ok(IngestOutcome::AlreadyProcessed) => {
                info!(download_id = %download_id, "Kobload ingest already processed, skipping notification");
                Ok(serde_json::json!({
                    "download_id": download_id,
                    "status": "already_processed",
                }))
            }
            Ok(IngestOutcome::Deferred(media_root_id)) => {
                info!(
                    download_id = %download_id,
                    media_root_id = %media_root_id,
                    "Kobload ingest deferred — media root unavailable"
                );
                // Store the payload for automatic retry when the root recovers
                if let Err(e) = deferred_ingests::insert(
                    &self.state.pool,
                    download_id,
                    media_root_id,
                    payload,
                    &response_mode,
                )
                .await
                {
                    warn!(
                        download_id = %download_id,
                        error = %e,
                        "Failed to store deferred ingest"
                    );
                }
                // Notify kobload so it shows in download events
                send_result(
                    &self.state,
                    download_id,
                    &response_mode,
                    IngestResultStatus::Deferred,
                    Some("Media root temporarily unavailable".to_string()),
                )
                .await;
                Ok(serde_json::json!({
                    "download_id": download_id,
                    "status": "deferred",
                    "media_root_id": media_root_id,
                }))
            }
            Err(err) => {
                warn!(download_id = %download_id, error = %err, "Kobload ingest failed");
                send_result(
                    &self.state,
                    download_id,
                    &response_mode,
                    IngestResultStatus::Failed,
                    Some(err.clone()),
                )
                .await;
                // Return success to the job system — we handled the failure by notifying kobload
                Ok(serde_json::json!({
                    "download_id": download_id,
                    "status": "failed",
                    "error": err,
                }))
            }
        }
    }
}

/// Send the ingest result back to kobload via NATS or HTTP.
async fn send_result(
    state: &AppState,
    download_id: Uuid,
    response_mode: &str,
    status: IngestResultStatus,
    error_msg: Option<String>,
) {
    if response_mode == "nats" {
        let event = IngestResultEvent {
            download_id,
            status,
            error: error_msg,
        };
        let payload = match serde_json::to_value(&event) {
            Ok(p) => p,
            Err(e) => {
                error!("Failed to serialize ingest result event: {}", e);
                return;
            }
        };
        if let Err(e) = state
            .nats
            .publisher()
            .publish_event("ingest.result", payload)
            .await
        {
            error!(download_id = %download_id, error = %e, "Failed to publish ingest.result event");
        }
    } else {
        // HTTP fallback: call kobload's import endpoints
        let runtime = state.runtime_config.get().await;
        let kobload_base_url = match runtime.kobload_base_url {
            Some(ref url) if !url.trim().is_empty() => url.trim_end_matches('/').to_string(),
            _ => {
                warn!(download_id = %download_id, "kobload_base_url not configured, cannot send HTTP result");
                return;
            }
        };
        let api_key = match runtime.kobload_api_key {
            Some(ref key) if !key.trim().is_empty() => key.clone(),
            _ => {
                warn!(download_id = %download_id, "kobload_api_key not configured, cannot send HTTP result");
                return;
            }
        };

        let client = crate::http::new_client();

        match status {
            IngestResultStatus::Imported => {
                let url = format!(
                    "{}/api/v1/downloads/{}/imported",
                    kobload_base_url, download_id
                );
                if let Err(e) = client.post(&url).bearer_auth(&api_key).send().await {
                    warn!(download_id = %download_id, error = %e, "Failed to confirm import to Kobload via HTTP");
                }
            }
            IngestResultStatus::Failed => {
                let url = format!(
                    "{}/api/v1/downloads/{}/import-failed",
                    kobload_base_url, download_id
                );
                let body = serde_json::json!({ "error": error_msg });
                if let Err(e) = client
                    .post(&url)
                    .bearer_auth(&api_key)
                    .json(&body)
                    .send()
                    .await
                {
                    warn!(download_id = %download_id, error = %e, "Failed to send import-failed to Kobload via HTTP");
                }
            }
            IngestResultStatus::Deferred => {
                let url = format!(
                    "{}/api/v1/downloads/{}/import-deferred",
                    kobload_base_url, download_id
                );
                let body = serde_json::json!({ "error": error_msg });
                if let Err(e) = client
                    .post(&url)
                    .bearer_auth(&api_key)
                    .json(&body)
                    .send()
                    .await
                {
                    warn!(download_id = %download_id, error = %e, "Failed to send import-deferred to Kobload via HTTP");
                }
            }
        }
    }
}
