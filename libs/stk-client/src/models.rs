//! Data models for Amazon STK API responses.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Device authentication information returned from device registration.
///
/// Contains the private key and ADP token needed to sign API requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// PEM-encoded RSA private key for signing requests.
    pub device_private_key: String,

    /// Token for X-ADP-Authentication-Token header.
    pub adp_token: String,

    /// Opaque device type identifier.
    pub device_type: String,

    /// User's given (first) name.
    pub given_name: String,

    /// User's full name.
    pub name: String,

    /// Account pool (e.g., "Amazon").
    pub account_pool: String,

    /// Opaque user identifier.
    pub user_directed_id: String,

    /// User's device name.
    pub user_device_name: String,

    /// Home region (e.g., "NA").
    #[serde(default)]
    pub home_region: Option<String>,
}

impl DeviceInfo {
    /// Parse DeviceInfo from Amazon's XML registration response.
    pub fn from_xml(xml: &str) -> Result<Self, quick_xml::DeError> {
        // The XML format is:
        // <response>
        //   <device_private_key>...</device_private_key>
        //   <adp_token>...</adp_token>
        //   ...
        // </response>
        //
        // We'll parse it using quick-xml's deserializer.
        #[derive(Debug, Deserialize)]
        struct Response {
            device_private_key: String,
            adp_token: String,
            device_type: String,
            given_name: String,
            name: String,
            account_pool: String,
            user_directed_id: String,
            user_device_name: String,
            #[serde(default)]
            home_region: Option<String>,
        }

        let response: Response = quick_xml::de::from_str(xml)?;
        Ok(DeviceInfo {
            device_private_key: response.device_private_key,
            adp_token: response.adp_token,
            device_type: response.device_type,
            given_name: response.given_name,
            name: response.name,
            account_pool: response.account_pool,
            user_directed_id: response.user_directed_id,
            user_device_name: response.user_device_name,
            home_region: response.home_region,
        })
    }
}

/// A user's Kindle device that can receive files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnedDevice {
    /// Device capabilities (name → enabled).
    #[serde(default)]
    pub device_capabilities: HashMap<String, bool>,

    /// Human-readable device name.
    pub device_name: String,

    /// Unique device serial number.
    pub device_serial_number: String,
}

/// Response from GetListOfOwnedDevices endpoint.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub(crate) struct GetOwnedDevicesResponse {
    pub owned_devices: Vec<OwnedDeviceRaw>,
    pub status_code: i32,
}

/// Raw owned device as returned by API.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OwnedDeviceRaw {
    #[serde(default)]
    pub device_capabilities: HashMap<String, bool>,
    pub device_name: String,
    pub device_serial_number: String,
}

impl From<OwnedDeviceRaw> for OwnedDevice {
    fn from(raw: OwnedDeviceRaw) -> Self {
        OwnedDevice {
            device_capabilities: raw.device_capabilities,
            device_name: raw.device_name,
            device_serial_number: raw.device_serial_number,
        }
    }
}

/// Response from GetUploadUrl endpoint.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub(crate) struct GetUploadUrlResponse {
    pub expiry_time: i64,
    pub status_code: i32,
    pub stk_token: String,
    pub upload_url: String,
}

/// Response from SendToKindle endpoint.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub(crate) struct SendToKindleResponse {
    pub sku: String,
    pub status_code: i32,
}

/// Response from token exchange endpoint.
#[derive(Debug, Deserialize)]
pub(crate) struct TokenExchangeResponse {
    pub access_token: String,
}
