//! Device lookup and context.

use sqlx::PgPool;

use crate::repositories::devices;

// Re-export DeviceContext from repository
pub use devices::DeviceContext;

/// Error returned when device lookup fails.
#[derive(Debug)]
pub enum DeviceLookupError {
    NotFound,
    Database(String),
}

impl std::fmt::Display for DeviceLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceLookupError::NotFound => write!(f, "device not found"),
            DeviceLookupError::Database(e) => write!(f, "database error: {}", e),
        }
    }
}

/// Look up a device by its token.
pub async fn lookup_device(pool: &PgPool, token: &str) -> Result<DeviceContext, DeviceLookupError> {
    devices::get_context_by_token(pool, token)
        .await
        .map_err(|e| DeviceLookupError::Database(e.to_string()))?
        .ok_or(DeviceLookupError::NotFound)
}
