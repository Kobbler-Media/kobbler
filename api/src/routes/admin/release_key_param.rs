use serde::{Deserialize, Deserializer, de::Error};

fn plain(key: String) -> Result<String, String> {
    let ok = (1..=128).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
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
