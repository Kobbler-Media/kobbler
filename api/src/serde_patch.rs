//! PATCH bodies need "leave it" and "clear it" to differ: a missing field is `None`,
//! an explicit `null` is `Some(None)`. Pair with `#[serde(default)]`.

use serde::{Deserialize, Deserializer};

pub fn double_option<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::deserialize(de).map(Some)
}
