//! Database logger for proxy requests.

use std::path::PathBuf;

use kobo_proxy::{RequestLogEntry, RequestLogger, async_trait, header_serde};
use sqlx::PgPool;
use uuid::Uuid;

use crate::blob_storage::BlobStorage;
use crate::kobo_handler::entity_scan;
use crate::repositories::{devices, proxy_logs as proxy_logs_repo};
use crate::runtime_config::RuntimeConfigManager;

/// Logger that writes to the proxy_logs table.
pub struct DatabaseLogger {
    pool: PgPool,
    blob_storage: BlobStorage,
    runtime_config: RuntimeConfigManager,
    library_events: tokio::sync::broadcast::Sender<crate::events::LibraryEvent>,
}

impl DatabaseLogger {
    pub fn new(
        pool: PgPool,
        blob_storage: BlobStorage,
        runtime_config: RuntimeConfigManager,
        library_events: tokio::sync::broadcast::Sender<crate::events::LibraryEvent>,
    ) -> Self {
        Self {
            pool,
            blob_storage,
            runtime_config,
            library_events,
        }
    }
}

#[async_trait]
impl RequestLogger for DatabaseLogger {
    async fn log_start(&self, entry: &RequestLogEntry) {
        let rt_config = self.runtime_config.get().await;

        // Skip logging if disabled entirely
        if !rt_config.proxy_logging_enabled {
            return;
        }

        // Parse device token to get device_id
        let device_id: Option<Uuid> = if let Some(ref token) = entry.device_token {
            devices::get_id_by_token(&self.pool, token)
                .await
                .ok()
                .flatten()
        } else {
            None
        };

        let request_headers = Some(header_serde::to_json_value(&entry.request_headers));

        // Insert minimal record for inflight request
        let input = proxy_logs_repo::ProxyLogInitialInput {
            id: entry.id,
            device_id,
            method: &entry.method,
            path: &entry.path,
            query: entry.query.as_deref(),
            request_headers,
            decision: &entry.decision,
        };
        if let Err(e) = proxy_logs_repo::create_initial(&self.pool, &input).await {
            tracing::warn!(error = %e, "Failed to log proxy request start");
        }
    }

    async fn log(&self, entry: RequestLogEntry) {
        let rt_config = self.runtime_config.get().await;

        // Skip logging if disabled entirely
        if !rt_config.proxy_logging_enabled {
            // Clean up any temp files that were created
            if let Some(ref path) = entry.request_blob_temp_path {
                let _ = tokio::fs::remove_file(path).await;
            }
            if let Some(ref path) = entry.response_blob_temp_path {
                let _ = tokio::fs::remove_file(path).await;
            }
            return;
        }

        let response_headers = Some(header_serde::to_json_value(&entry.response_headers));
        let upstream_headers = entry
            .upstream_headers
            .as_ref()
            .map(|h| header_serde::to_json_value(h));

        // For intercept file downloads, use response_headers as original_response_headers
        // (since there's no upstream to modify from). For passthrough/modify, use the actual original.
        let original_response_headers = if entry.intercept_source_path.is_some() {
            Some(header_serde::to_json_value(&entry.response_headers))
        } else {
            entry
                .original_response_headers
                .as_ref()
                .map(|h| header_serde::to_json_value(h))
        };

        // Move temp blob files to permanent storage (if blob capture enabled)
        let mut request_blob_path: Option<String> = None;
        let mut response_blob_path: Option<String> = None;

        if rt_config.blob_capture_enabled {
            // For intercept file downloads, use the source file path directly (no duplicate storage)
            if let Some(ref source_path) = entry.intercept_source_path {
                response_blob_path = Some(source_path.clone());
            }

            // Move request blob temp file to permanent location
            if let Some(ref temp_path) = entry.request_blob_temp_path
                && let Some(perm_path) = self
                    .blob_storage
                    .move_temp_to_request(temp_path, entry.id)
                    .await
            {
                request_blob_path = Some(perm_path.to_string_lossy().to_string());
            }

            // Move response blob temp file to permanent location (only if no intercept source path)
            if entry.intercept_source_path.is_none()
                && let Some(ref temp_path) = entry.response_blob_temp_path
                && let Some(perm_path) = self
                    .blob_storage
                    .move_temp_to_response(temp_path, entry.id)
                    .await
            {
                response_blob_path = Some(perm_path.to_string_lossy().to_string());
            }
        } else {
            // Blob capture disabled - clean up any temp files
            if let Some(ref path) = entry.request_blob_temp_path {
                let _ = tokio::fs::remove_file(path).await;
            }
            if let Some(ref path) = entry.response_blob_temp_path {
                let _ = tokio::fs::remove_file(path).await;
            }
        }

        // Determine response body for logging
        // If we have actual body text, use it; otherwise show placeholder for binary
        let response_body = if entry.response_body.is_some() {
            entry.response_body.clone()
        } else if entry.response_body_size > 0 && response_blob_path.is_none() {
            // Binary content but blob capture is off - show placeholder
            Some(format!("[Binary: {} bytes]", entry.response_body_size))
        } else {
            None
        };

        // Same for request body
        let request_body = if entry.request_body.is_some() {
            entry.request_body.clone()
        } else if entry.request_body_size > 0 && request_blob_path.is_none() {
            Some(format!("[Binary: {} bytes]", entry.request_body_size))
        } else {
            None
        };

        // Same for upstream body
        let upstream_body = if entry.upstream_body.is_some() {
            entry.upstream_body.clone()
        } else if entry.upstream_body_size > 0 {
            Some(format!("[Binary: {} bytes]", entry.upstream_body_size))
        } else {
            None
        };

        // Scan for entity references and detect leaks
        let (entity_refs, has_entity_leak) = entity_scan::scan_and_resolve(
            &self.pool,
            &entry.decision,
            &entry.path,
            request_body.as_deref(),
            response_body.as_deref(),
            upstream_body.as_deref(),
        )
        .await;

        let entity_refs_json =
            serde_json::to_value(&entity_refs).unwrap_or(serde_json::Value::Array(vec![]));

        // Merge alert sources: classify-time alert takes priority, then entity scan leak
        let alert_message = if entry.alert_message.is_some() {
            entry.alert_message.clone()
        } else if has_entity_leak {
            Some("Local entity UUID sent to Kobo in non-intercepted request".to_string())
        } else {
            None
        };

        let has_alert = alert_message.is_some();

        if has_alert {
            tracing::warn!(
                path = %entry.path,
                decision = %entry.decision,
                alert = alert_message.as_deref().unwrap_or(""),
                "Proxy alert"
            );
        }

        // Update the existing record with completion data
        let complete_input = proxy_logs_repo::ProxyLogCompleteInput {
            request_body: request_body.as_deref(),
            request_body_size: entry.request_body_size as i64,
            upstream_status: entry.upstream_status.map(|s| s as i16),
            upstream_headers,
            upstream_body: upstream_body.as_deref(),
            upstream_body_size: entry.upstream_body_size as i64,
            response_status: entry.status as i16,
            response_headers,
            response_body: response_body.as_deref(),
            response_body_size: entry.response_body_size as i64,
            upstream_url: entry.upstream_url.as_deref(),
            upstream_duration_ms: entry.upstream_duration_ms.map(|v| v as i32),
            total_duration_ms: entry.total_duration_ms as i32,
            severity: &entry.severity,
            notes: entry.notes.as_deref(),
            request_blob_path: request_blob_path.as_deref(),
            response_blob_path: response_blob_path.as_deref(),
            original_response_headers,
            log_status: &entry.log_status,
            entity_refs: entity_refs_json,
            has_alert,
            alert_message: alert_message.as_deref(),
        };
        match proxy_logs_repo::complete(&self.pool, entry.id, &complete_input).await {
            Ok(_) => {
                // Notify SSE clients about the new log
                let _ = self
                    .library_events
                    .send(crate::events::LibraryEvent::proxy_log_created(entry.id));
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to update proxy log");
            }
        }
    }

    fn blob_temp_dir(&self) -> Option<PathBuf> {
        // Always return temp dir. Whether we actually keep the blobs is decided
        // at log time based on blob_capture_enabled. Temp files are cleaned up
        // in log() if blob capture is disabled.
        Some(self.blob_storage.temp_dir().clone())
    }
}
