//! API wrapper functions for Amazon auth and STK APIs.

use crate::error::{Error, Result};
use crate::models::{
    DeviceInfo, GetOwnedDevicesResponse, GetUploadUrlResponse, OwnedDevice, SendToKindleResponse,
    TokenExchangeResponse,
};
use crate::signer::Signer;
use reqwest::Client as HttpClient;
use serde_json::{Value, json};
use std::path::Path;
use tokio::io::AsyncReadExt;
use tracing::debug;

/// Client info sent with STK API requests.
const DEFAULT_CLIENT_INFO: &str = r#"{
    "appName": "ShellExtension",
    "appVersion": "1.1.1.253",
    "os": "MacOSX_10.14.6_x64",
    "osArchitecture": "x64"
}"#;

/// Amazon OAuth2 client ID.
const CLIENT_ID: &str = "658490dfb190e494030082836775981fa23be0c2425441860352ba0f55915b43002d";

/// Exchange authorization code for access token.
pub async fn token_exchange(authorization_code: &str, code_verifier: &str) -> Result<String> {
    let client = HttpClient::new();

    let body = json!({
        "app_name": "Unknown",
        "client_domain": "DeviceLegacy",
        "client_id": CLIENT_ID,
        "code_algorithm": "SHA-256",
        "code_verifier": code_verifier,
        "requested_token_type": "access_token",
        "source_token": authorization_code,
        "source_token_type": "authorization_code",
    });

    debug!(code = %authorization_code, "Exchanging authorization code for access token");

    let response = client
        .post("https://api.amazon.com/auth/token")
        .header("Accept-Language", "en-US")
        .header("x-amzn-identity-auth-domain", "api.amazon.com")
        .header("Content-Type", "application/json")
        .header("User-Agent", "Mozilla/5.0")
        .json(&body)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(Error::Api {
            status: status.as_u16(),
            message: body,
        });
    }

    let data: TokenExchangeResponse = response.json().await?;
    Ok(data.access_token)
}

/// Register a device with Amazon using an access token.
///
/// Returns device info including the private key and ADP token needed for API requests.
pub async fn register_device_with_token(access_token: &str) -> Result<DeviceInfo> {
    let client = HttpClient::new();

    // XML body for device registration
    let body = format!(
        r#"<?xml version='1.0' encoding='UTF-8'?>
<request><parameters><deviceType>A1K6D1WRW0MALS</deviceType><deviceSerialNumber>ZYSQ37GQ5JQDAIKDZ3WYH6I74MJCVEGG</deviceSerialNumber><pid>D21NN3GG</pid><authToken>{}</authToken><authTokenType>AccessToken</authTokenType><softwareVersion>253</softwareVersion><os_version>MacOSX_10.14.6_x64</os_version><device_model>Maxs MacBook Pro</device_model></parameters></request>"#,
        access_token
    );

    debug!("Registering device with Amazon");

    let response = client
        .post("https://firs-ta-g7g.amazon.com/FirsProxy/registerDeviceWithToken")
        .header("Content-Type", "text/xml")
        .header("Expect", "")
        .header("Accept-Language", "en-US,*")
        .header("User-Agent", "Mozilla/5.0")
        .body(body)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(Error::Api {
            status: status.as_u16(),
            message: body,
        });
    }

    let xml = response.text().await?;
    let device_info = DeviceInfo::from_xml(&xml)?;
    Ok(device_info)
}

/// Get list of Kindle devices owned by the user.
pub async fn get_list_of_owned_devices(signer: &Signer) -> Result<Vec<OwnedDevice>> {
    let response: GetOwnedDevicesResponse =
        stk_request(signer, "/GetListOfOwnedDevices", json!({})).await?;
    Ok(response.owned_devices.into_iter().map(Into::into).collect())
}

/// Get a pre-signed URL for uploading a file.
pub async fn get_upload_url(signer: &Signer, file_size: u64) -> Result<GetUploadUrlResponse> {
    stk_request(signer, "/GetUploadUrl", json!({ "fileSize": file_size })).await
}

/// Upload a file to the pre-signed S3 URL.
pub async fn upload_file(upload_url: &str, file_path: &Path) -> Result<()> {
    let client = HttpClient::new();

    // Read file
    let mut file = tokio::fs::File::open(file_path).await?;
    let metadata = file.metadata().await?;
    let file_size = metadata.len();

    let mut contents = Vec::with_capacity(file_size as usize);
    file.read_to_end(&mut contents).await?;

    debug!(url = %upload_url, size = file_size, "Uploading file to S3");

    let response = client
        .put(upload_url)
        .header("Accept-Encoding", "gzip, deflate")
        .header("Accept-Language", "en-US,*")
        .header("Content-Length", file_size.to_string())
        .header("User-Agent", "Mozilla/5.0")
        .body(contents)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(Error::Api {
            status: status.as_u16(),
            message: format!("Upload failed: {}", body),
        });
    }

    Ok(())
}

/// Send an uploaded file to Kindle devices.
pub async fn send_to_kindle(
    signer: &Signer,
    stk_token: &str,
    target_device_serial_numbers: &[&str],
    author: &str,
    title: &str,
    format: &str,
) -> Result<SendToKindleResponse> {
    let body = json!({
        "DocumentMetadata": {
            "author": author,
            "crc32": 0,
            "inputFormat": format,
            "title": title,
        },
        "archive": true,
        "deliveryMechanism": "WIFI",
        "outputFormat": "MOBI",
        "stkToken": stk_token,
        "targetDevices": target_device_serial_numbers,
    });

    stk_request(signer, "/SendToKindle", body).await
}

/// Make a signed request to the STK service.
async fn stk_request<T>(signer: &Signer, path: &str, body: Value) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let client = HttpClient::new();

    // Merge body with ClientInfo
    let client_info: Value = serde_json::from_str(DEFAULT_CLIENT_INFO)?;
    let mut full_body = body;
    if let Some(obj) = full_body.as_object_mut() {
        obj.insert("ClientInfo".to_string(), client_info);
    }

    let data = serde_json::to_string_pretty(&full_body)?;
    let digest = signer.digest_header_for_request("POST", path, &data)?;

    debug!(path = %path, "Making STK API request");

    let response = client
        .post(format!("https://stkservice.amazon.com{}", path))
        .header("Accept", "application/json")
        .header("Accept-Encoding", "gzip, deflate")
        .header("Content-Type", "application/json")
        .header("X-ADP-Request-Digest", digest)
        .header("X-ADP-Authentication-Token", &signer.adp_token)
        .header("Accept-Language", "en-US,*")
        .header("User-Agent", "Mozilla/5.0")
        .body(data)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(Error::Api {
            status: status.as_u16(),
            message: body,
        });
    }

    let result: T = response.json().await?;
    Ok(result)
}

/// Logout the device (revoke credentials).
#[allow(dead_code)]
pub async fn logout(signer: &Signer) -> Result<()> {
    let client = HttpClient::new();
    let path = "/FirsProxy/disownFiona?contentDeleted=false";
    let digest = signer.digest_header_for_request("GET", path, "")?;

    let response = client
        .get(format!("https://firs-ta-g7g.amazon.com{}", path))
        .header("Content-Type", "text/xml")
        .header("X-ADP-Request-Digest", digest)
        .header("X-ADP-Authentication-Token", &signer.adp_token)
        .header("Accept-Language", "en-US,*")
        .header("User-Agent", "Mozilla/5.0")
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(Error::Api {
            status: status.as_u16(),
            message: body,
        });
    }

    Ok(())
}
