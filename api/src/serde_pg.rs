//! Serde helpers for deserializing PostgreSQL JSONB values.
//!
//! PostgreSQL serializes timestamptz as ISO 8601 strings in JSONB,
//! and UUIDs as hyphenated strings. These helpers handle that format.

use serde::{Deserialize, Deserializer};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

/// Deserialize an `OffsetDateTime` from a PostgreSQL JSONB timestamp string.
pub fn timestamp<'de, D>(deserializer: D) -> Result<OffsetDateTime, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    OffsetDateTime::parse(&s, &Rfc3339).map_err(serde::de::Error::custom)
}

/// Deserialize an `Option<OffsetDateTime>` from a PostgreSQL JSONB timestamp string.
pub fn opt_timestamp<'de, D>(deserializer: D) -> Result<Option<OffsetDateTime>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(deserializer)?;
    match s {
        Some(s) => OffsetDateTime::parse(&s, &Rfc3339)
            .map(Some)
            .map_err(serde::de::Error::custom),
        None => Ok(None),
    }
}

/// Deserialize a `Uuid` from a PostgreSQL JSONB UUID string.
pub fn uuid<'de, D>(deserializer: D) -> Result<Uuid, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    s.parse().map_err(serde::de::Error::custom)
}
