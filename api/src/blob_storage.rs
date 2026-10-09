//! Blob storage for captured proxy request/response bodies.
//!
//! When blob capture is enabled, full request and response bodies are stored
//! on disk and referenced by log entry ID.

use std::path::{Path, PathBuf};
use tokio::fs;
use uuid::Uuid;

/// Manages blob storage for proxy log entries.
#[derive(Clone)]
pub struct BlobStorage {
    base_path: PathBuf,
    temp_path: PathBuf,
}

impl BlobStorage {
    /// Create a new blob storage at `{storage_root}/proxy_blobs/`.
    pub fn new(storage_root: &str) -> Self {
        Self {
            base_path: Path::new(storage_root).join("proxy_blobs"),
            temp_path: Path::new(storage_root).join("proxy_blobs_temp"),
        }
    }

    /// Ensure the blob directory exists.
    pub async fn ensure_dir(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.base_path).await?;
        fs::create_dir_all(&self.temp_path).await
    }

    /// Get the temp directory path for streaming blob capture.
    pub fn temp_dir(&self) -> &PathBuf {
        &self.temp_path
    }

    /// Get the path for a request blob.
    pub fn request_path(&self, log_id: Uuid) -> PathBuf {
        self.base_path.join(format!("{}_request.bin", log_id))
    }

    /// Get the path for a response blob.
    pub fn response_path(&self, log_id: Uuid) -> PathBuf {
        self.base_path.join(format!("{}_response.bin", log_id))
    }

    /// Write blob data to disk.
    pub async fn write(&self, path: &Path, data: &[u8]) -> std::io::Result<()> {
        // Ensure directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(path, data).await
    }

    /// Read blob data from disk.
    pub async fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        fs::read(path).await
    }

    /// Delete a blob file (ignores NotFound errors).
    pub async fn delete(&self, path: &Path) -> std::io::Result<()> {
        match fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Delete all blobs associated with a log entry.
    pub async fn delete_for_log(&self, log_id: Uuid) {
        let _ = self.delete(&self.request_path(log_id)).await;
        let _ = self.delete(&self.response_path(log_id)).await;
    }

    /// Delete blobs for multiple log entries.
    pub async fn delete_for_logs(&self, log_ids: &[Uuid]) {
        for log_id in log_ids {
            self.delete_for_log(*log_id).await;
        }
    }

    /// Move a temp file to the permanent request blob location.
    ///
    /// Returns the final path on success, or None if the move fails.
    pub async fn move_temp_to_request(&self, temp_path: &str, log_id: Uuid) -> Option<PathBuf> {
        let dest = self.request_path(log_id);
        self.move_temp_file(temp_path, &dest).await
    }

    /// Move a temp file to the permanent response blob location.
    ///
    /// Returns the final path on success, or None if the move fails.
    pub async fn move_temp_to_response(&self, temp_path: &str, log_id: Uuid) -> Option<PathBuf> {
        let dest = self.response_path(log_id);
        self.move_temp_file(temp_path, &dest).await
    }

    /// Move a temp file to a destination path.
    async fn move_temp_file(&self, temp_path: &str, dest: &Path) -> Option<PathBuf> {
        // Ensure destination directory exists
        if let Some(parent) = dest.parent()
            && fs::create_dir_all(parent).await.is_err()
        {
            return None;
        }

        // Try rename first (fast, works on same filesystem)
        if fs::rename(temp_path, dest).await.is_ok() {
            return Some(dest.to_path_buf());
        }

        // Fall back to copy + delete (cross-filesystem)
        if fs::copy(temp_path, dest).await.is_ok() {
            let _ = fs::remove_file(temp_path).await;
            return Some(dest.to_path_buf());
        }

        None
    }

    /// Clean up temp files in the temp directory.
    ///
    /// Call this on startup to clean up any orphaned temp files from crashes.
    pub async fn cleanup_temp(&self) {
        if let Ok(mut entries) = fs::read_dir(&self.temp_path).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.extension().map(|e| e == "tmp").unwrap_or(false) {
                    let _ = fs::remove_file(path).await;
                }
            }
        }
    }
}
