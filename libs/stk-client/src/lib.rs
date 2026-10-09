//! Native Rust client for Amazon's Send-to-Kindle service.
//!
//! This crate provides a type-safe interface for sending ebooks to Kindle devices
//! using Amazon's STK (Send-to-Kindle) API, without the 10MB email attachment limit.
//!
//! # Example
//!
//! ```ignore
//! use stk_client::{OAuth2, Client};
//!
//! // Start OAuth flow
//! let oauth = OAuth2::new();
//! let signin_url = oauth.get_signin_url();
//!
//! // After user signs in and is redirected back
//! let client = oauth.create_client(&redirect_url).await?;
//!
//! // List devices
//! let devices = client.get_owned_devices().await?;
//!
//! // Send a file
//! let sku = client.send_file(
//!     &path,
//!     &["DEVICE_SERIAL"],
//!     "Author Name",
//!     "Book Title",
//!     "EPUB",
//! ).await?;
//! ```

mod api;
mod client;
mod error;
mod models;
mod oauth;
mod signer;

pub use client::Client;
pub use error::{Error, Result};
pub use models::{DeviceInfo, OwnedDevice};
pub use oauth::OAuth2;
