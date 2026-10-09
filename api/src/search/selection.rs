//! Startup selection of the search backend.

use tracing::warn;

use super::meili::MeiliState;

/// Which search backend a given configuration asks for.
///
/// Meilisearch is optional: an empty `meili_url` means "not configured", and an
/// unusable one degrades to the SQL fallback rather than taking the process down.
pub enum SearchBackendChoice {
    Meili(MeiliState),
    SqlFallback,
}

impl SearchBackendChoice {
    pub fn from_config(meili_url: &str, meili_master_key: &str) -> Self {
        let url = meili_url.trim();
        if url.is_empty() {
            return Self::SqlFallback;
        }

        match MeiliState::new(url, meili_master_key) {
            Ok(state) => Self::Meili(state),
            Err(err) => {
                warn!("{err}. Search will use the SQL fallback backend");
                Self::SqlFallback
            }
        }
    }
}
