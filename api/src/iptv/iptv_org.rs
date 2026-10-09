//! Fetch and cache the iptv-org channel database.
//!
//! Uses <https://iptv-org.github.io/api/channels.json> as a canonical source
//! for channel names, alt-names, countries, and languages. Builds a normalized
//! lookup map for fast matching during sync.

use std::collections::HashMap;

use serde::Deserialize;
use tracing::{info, warn};

use super::normalization;

const CHANNELS_URL: &str = "https://iptv-org.github.io/api/channels.json";

#[derive(Debug, Clone)]
pub struct IptvOrgChannel {
    pub id: String,
    pub name: String,
    pub alt_names: Vec<String>,
    pub country: String,
    pub language: Option<String>,
    pub categories: Vec<String>,
    pub logo: Option<String>,
}

/// Deserialization shape matching the iptv-org API.
#[derive(Deserialize)]
struct RawChannel {
    id: Option<String>,
    name: Option<String>,
    #[serde(default)]
    alt_names: Vec<String>,
    #[serde(default)]
    country: String,
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default)]
    categories: Vec<String>,
    logo: Option<String>,
}

pub struct IptvOrgDatabase {
    /// Multiple channels can share the same normalized name (e.g. "TNT Sports 2"
    /// exists in BR, CL, UK). Store all and pick best match by country.
    by_name: HashMap<String, Vec<IptvOrgChannel>>,
}

impl IptvOrgDatabase {
    /// Fetch the iptv-org channel database and build a normalized lookup map.
    ///
    /// If the fetch fails, returns an empty database rather than propagating the
    /// error — this allows sync to proceed without iptv-org matching.
    pub async fn fetch(http: &reqwest::Client) -> Self {
        match Self::fetch_inner(http).await {
            Ok(db) => {
                info!(
                    channels = db.by_name.len(),
                    "loaded iptv-org channel database"
                );
                db
            }
            Err(e) => {
                warn!(error = %e, "failed to fetch iptv-org database, using empty fallback");
                Self {
                    by_name: HashMap::new(),
                }
            }
        }
    }

    async fn fetch_inner(http: &reqwest::Client) -> anyhow::Result<Self> {
        let resp = http
            .get(CHANNELS_URL)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await?
            .error_for_status()?;
        let raw_channels: Vec<RawChannel> = resp.json().await?;

        let mut by_name: HashMap<String, Vec<IptvOrgChannel>> =
            HashMap::with_capacity(raw_channels.len() * 2);

        for raw in raw_channels {
            let Some(id) = raw.id else { continue };
            let Some(name) = raw.name else { continue };

            let language = if let Some(lang) = raw.languages.into_iter().next() {
                Some(lang)
            } else {
                normalization::country_to_language(&raw.country).map(String::from)
            };

            let channel = IptvOrgChannel {
                id,
                name: name.clone(),
                alt_names: raw.alt_names.clone(),
                country: raw.country,
                language,
                categories: raw.categories,
                logo: raw.logo.clone(),
            };

            // Index by normalized primary name
            let normalized = normalization::normalize_stream_name(&name);
            by_name
                .entry(normalized.normalized_name)
                .or_default()
                .push(channel.clone());

            // Index alt-names too
            for alt in &raw.alt_names {
                let norm_alt = normalization::normalize_stream_name(alt);
                by_name
                    .entry(norm_alt.normalized_name)
                    .or_default()
                    .push(channel.clone());
            }
        }

        Ok(Self { by_name })
    }

    /// Build from a JSON string (for testing with saved data).
    pub fn from_json(json: &str) -> Self {
        let raw_channels: Vec<RawChannel> = match serde_json::from_str(json) {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "failed to parse iptv-org JSON");
                return Self {
                    by_name: HashMap::new(),
                };
            }
        };

        let mut by_name: HashMap<String, Vec<IptvOrgChannel>> =
            HashMap::with_capacity(raw_channels.len() * 2);
        for raw in raw_channels {
            let Some(id) = raw.id else { continue };
            let Some(name) = raw.name else { continue };

            let language = if let Some(lang) = raw.languages.into_iter().next() {
                Some(lang)
            } else {
                normalization::country_to_language(&raw.country).map(String::from)
            };

            let channel = IptvOrgChannel {
                id,
                name: name.clone(),
                alt_names: raw.alt_names.clone(),
                country: raw.country,
                language,
                categories: raw.categories,
                logo: raw.logo.clone(),
            };

            let normalized = normalization::normalize_stream_name(&name);
            by_name
                .entry(normalized.normalized_name)
                .or_default()
                .push(channel.clone());

            for alt in &raw.alt_names {
                let norm_alt = normalization::normalize_stream_name(alt);
                by_name
                    .entry(norm_alt.normalized_name)
                    .or_default()
                    .push(channel.clone());
            }
        }

        info!(
            channels = by_name.len(),
            "loaded iptv-org database from JSON"
        );
        Self { by_name }
    }

    /// Look up a channel by normalized name, preferring a match for the given country.
    /// If country is provided, returns the entry matching that country (or first if no match).
    /// If country is None, returns the first entry.
    pub fn lookup(&self, normalized_name: &str, country: Option<&str>) -> Option<&IptvOrgChannel> {
        let entries = self.by_name.get(normalized_name)?;
        if let Some(country) = country {
            // Prefer exact country match (iptv-org uses 2-letter codes, but "UK" vs "GB")
            let country_upper = country.to_uppercase();
            let gb_match = country_upper == "GB"; // iptv-org uses "UK" for GB
            if let Some(ch) = entries.iter().find(|c| {
                c.country.eq_ignore_ascii_case(&country_upper)
                    || (gb_match && c.country.eq_ignore_ascii_case("UK"))
            }) {
                return Some(ch);
            }
        }
        entries.first()
    }

    /// Number of unique normalized names indexed.
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// Whether the database is empty.
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}
