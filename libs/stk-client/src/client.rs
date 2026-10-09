//! Client for Amazon Send-to-Kindle service.

use crate::api;
use crate::error::{Error, Result};
use crate::models::{DeviceInfo, OwnedDevice};
use crate::signer::Signer;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::info;

/// Credentials version for serialization format.
const CREDENTIALS_VERSION: i32 = 1;

/// Client for Amazon Send-to-Kindle service.
///
/// Supports listing devices and sending files to specific devices.
#[derive(Debug)]
pub struct Client {
    device_info: DeviceInfo,
    signer: Signer,
}

impl Client {
    /// Create a new client from device info.
    pub fn new(device_info: DeviceInfo) -> Self {
        // Safe to unwrap because DeviceInfo comes from a successful registration
        let signer =
            Signer::from_device_info(&device_info).expect("DeviceInfo should contain valid key");
        Self {
            device_info,
            signer,
        }
    }

    /// Get the list of Kindle devices owned by the user.
    pub async fn get_owned_devices(&self) -> Result<Vec<OwnedDevice>> {
        api::get_list_of_owned_devices(&self.signer).await
    }

    /// Send a file to the specified Kindle devices.
    ///
    /// # Arguments
    ///
    /// * `file_path` - Path to the ebook file
    /// * `target_device_serial_numbers` - Device serial numbers to send to
    /// * `author` - Book author
    /// * `title` - Book title
    /// * `format` - File format (e.g., "EPUB", "PDF")
    ///
    /// # Returns
    ///
    /// SKU identifier assigned by Amazon.
    pub async fn send_file(
        &self,
        file_path: &Path,
        target_device_serial_numbers: &[&str],
        author: &str,
        title: &str,
        format: &str,
    ) -> Result<String> {
        // Get file size
        let metadata = tokio::fs::metadata(file_path).await?;
        let file_size = metadata.len();

        info!(
            path = %file_path.display(),
            size = file_size,
            devices = ?target_device_serial_numbers,
            "Sending file to Kindle"
        );

        // Get upload URL
        let upload = api::get_upload_url(&self.signer, file_size).await?;

        // Upload file to S3
        api::upload_file(&upload.upload_url, file_path).await?;

        // Send to Kindle
        let response = api::send_to_kindle(
            &self.signer,
            &upload.stk_token,
            target_device_serial_numbers,
            author,
            title,
            format,
        )
        .await?;

        Ok(response.sku)
    }

    /// Serialize the client credentials to a JSON string.
    ///
    /// The credentials can be restored with `Client::loads()`.
    pub fn dumps(&self) -> String {
        let creds = Credentials {
            version: CREDENTIALS_VERSION,
            device_info: self.device_info.clone(),
        };
        serde_json::to_string(&creds).expect("Credentials should serialize")
    }

    /// Deserialize a client from a JSON string.
    ///
    /// # Errors
    ///
    /// Returns an error if the credentials are invalid or have an unsupported version.
    pub fn loads(s: &str) -> Result<Self> {
        let creds: Credentials =
            serde_json::from_str(s).map_err(|e| Error::Credentials(e.to_string()))?;

        if creds.version != CREDENTIALS_VERSION {
            return Err(Error::Credentials(format!(
                "Unsupported credentials version: {}",
                creds.version
            )));
        }

        Ok(Self::new(creds.device_info))
    }
}

/// Serialized credentials format.
#[derive(Debug, Serialize, Deserialize)]
struct Credentials {
    version: i32,
    device_info: DeviceInfo,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credentials_round_trip() {
        // Create a fake DeviceInfo for testing
        let device_info = DeviceInfo {
            device_private_key:
                "-----BEGIN RSA PRIVATE KEY-----\ntest\n-----END RSA PRIVATE KEY-----".to_string(),
            adp_token: "test_token".to_string(),
            device_type: "test_type".to_string(),
            given_name: "Test".to_string(),
            name: "Test User".to_string(),
            account_pool: "Amazon".to_string(),
            user_directed_id: "test_id".to_string(),
            user_device_name: "Test Device".to_string(),
            home_region: Some("NA".to_string()),
        };

        let creds = Credentials {
            version: CREDENTIALS_VERSION,
            device_info,
        };

        let json = serde_json::to_string(&creds).unwrap();
        let parsed: Credentials = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.version, CREDENTIALS_VERSION);
        assert_eq!(parsed.device_info.given_name, "Test");
    }
}
