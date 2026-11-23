use serde::{Deserialize, Deserializer, de::Error};

pub(crate) fn is_plain(key: &str) -> bool {
    (1..=128).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn plain(key: String) -> Result<String, String> {
    if is_plain(&key) {
        Ok(key)
    } else {
        Err(format!("release_key {key:?} is not a release key"))
    }
}

pub fn required<'de, D: Deserializer<'de>>(de: D) -> Result<String, D::Error> {
    plain(String::deserialize(de)?).map_err(D::Error::custom)
}

pub fn optional<'de, D: Deserializer<'de>>(de: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(de)?
        .map(plain)
        .transpose()
        .map_err(D::Error::custom)
}
