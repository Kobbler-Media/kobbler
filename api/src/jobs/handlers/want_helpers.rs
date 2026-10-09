//! Shared helpers for want job handlers.

use nats_jobs::ExcludedRelease;

/// Convert a `rejected_releases` JSONB array into `Option<Vec<ExcludedRelease>>`.
///
/// Each entry is expected to have optional `release_title` and/or `info_hash` fields.
/// Returns `None` if the array is empty (avoids sending an empty vec to kobload).
pub fn build_excluded_releases(rejected: &serde_json::Value) -> Option<Vec<ExcludedRelease>> {
    let arr = rejected.as_array()?;
    if arr.is_empty() {
        return None;
    }
    let releases: Vec<ExcludedRelease> = arr
        .iter()
        .filter_map(|entry| {
            let release_title = entry
                .get("release_title")
                .and_then(|v| v.as_str())
                .map(String::from);
            let info_hash = entry
                .get("info_hash")
                .and_then(|v| v.as_str())
                .map(String::from);
            if release_title.is_some() || info_hash.is_some() {
                Some(ExcludedRelease {
                    release_title,
                    info_hash,
                })
            } else {
                None
            }
        })
        .collect();
    if releases.is_empty() {
        None
    } else {
        Some(releases)
    }
}
