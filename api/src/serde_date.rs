//! Custom serde helpers for `time::Date` as ISO 8601 `"YYYY-MM-DD"` strings.
//!
//! The `time` crate's default serde representation uses ordinal `[year, day_of_year]`
//! arrays, which is not useful for JSON APIs. These modules provide human-readable
//! ISO date formatting instead.

use serde::{self, Deserialize, Deserializer, Serializer};
use time::Date;
use time::format_description::well_known::Iso8601;

/// Serialize/deserialize `time::Date` as `"YYYY-MM-DD"`.
pub mod date {
    use super::*;

    pub fn serialize<S>(date: &Date, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = date
            .format(&Iso8601::DATE)
            .map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&s)
    }

    #[allow(dead_code)]
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Date, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Date::parse(&s, &Iso8601::DATE).map_err(serde::de::Error::custom)
    }
}

/// Serialize/deserialize `Option<time::Date>` as `"YYYY-MM-DD"` or `null`.
pub mod option_date {
    use super::*;

    pub fn serialize<S>(date: &Option<Date>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match date {
            Some(d) => {
                let s = d
                    .format(&Iso8601::DATE)
                    .map_err(serde::ser::Error::custom)?;
                serializer.serialize_str(&s)
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Date>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<String> = Option::deserialize(deserializer)?;
        match opt {
            Some(s) => {
                let d = Date::parse(&s, &Iso8601::DATE).map_err(serde::de::Error::custom)?;
                Ok(Some(d))
            }
            None => Ok(None),
        }
    }
}
