//! Priority-based metadata merging for user-selected results.
//!
//! When users select multiple provider results, this module handles merging
//! them by priority (first selected = highest priority).

use crate::metadata::MetadataResult;
use serde_json::{Map, Value};

/// Merge multiple metadata results by priority.
///
/// - results[0] is highest priority
/// - For most fields: use first non-null value from priority order
/// - For identifiers: aggregate ALL identifiers from ALL providers
/// - For subjects: union from all sources
pub fn merge_by_priority(results: &[MetadataResult]) -> Option<MetadataResult> {
    if results.is_empty() {
        return None;
    }

    if results.len() == 1 {
        return Some(results[0].clone());
    }

    // Start with the highest priority result
    let mut merged = results[0].clone();
    merged.provider = "merged".to_string();

    let mut all_identifiers: Map<String, Value> = Map::new();
    for result in results {
        all_identifiers.extend(result.all_identifiers());
    }
    merged.identifiers = Value::Object(all_identifiers);

    // For other fields, use first non-null from priority order
    for result in results.iter().skip(1) {
        if merged.title.is_empty() && !result.title.is_empty() {
            merged.title = result.title.clone();
        }
        if merged.authors.is_empty() && !result.authors.is_empty() {
            merged.authors = result.authors.clone();
        }
        if merged.description.is_none() && result.description.is_some() {
            merged.description = result.description.clone();
        }
        if merged.cover_url.is_none() && result.cover_url.is_some() {
            merged.cover_url = result.cover_url.clone();
        }
        if merged.page_count.is_none() && result.page_count.is_some() {
            merged.page_count = result.page_count;
        }
        if merged.rating.is_none() && result.rating.is_some() {
            merged.rating = result.rating;
        }
        if merged.rating_count.is_none() && result.rating_count.is_some() {
            merged.rating_count = result.rating_count;
        }
        if merged.isbn.is_none() && result.isbn.is_some() {
            merged.isbn = result.isbn.clone();
        }
        if merged.language.is_none() && result.language.is_some() {
            merged.language = result.language.clone();
        }
        if merged.publisher.is_none() && result.publisher.is_some() {
            merged.publisher = result.publisher.clone();
        }
        if merged.publish_date.is_none() && result.publish_date.is_some() {
            merged.publish_date = result.publish_date.clone();
        }
        if merged.series.is_none() && result.series.is_some() {
            merged.series = result.series.clone();
        }
        if merged.series_index.is_none() && result.series_index.is_some() {
            merged.series_index = result.series_index;
        }
        if merged.series_provider_id.is_none() && result.series_provider_id.is_some() {
            merged.series_provider_id = result.series_provider_id.clone();
        }
        // Series list: union from all sources (deduplicate by name, case-insensitive)
        for si in &result.series_list {
            if !merged
                .series_list
                .iter()
                .any(|s| s.name.to_lowercase() == si.name.to_lowercase())
            {
                merged.series_list.push(si.clone());
            }
        }
        if merged.narrator.is_none() && result.narrator.is_some() {
            merged.narrator = result.narrator.clone();
        }
        if merged.duration_seconds.is_none() && result.duration_seconds.is_some() {
            merged.duration_seconds = result.duration_seconds;
        }
        if merged.chapters.is_empty() && !result.chapters.is_empty() {
            merged.chapters = result.chapters.clone();
        }
        if merged.asin.is_none() && result.asin.is_some() {
            merged.asin = result.asin.clone();
        }

        // Subjects: union from all sources (case-insensitive dedup)
        for subject in &result.subjects {
            if !merged
                .subjects
                .iter()
                .any(|s| s.to_lowercase() == subject.to_lowercase())
            {
                merged.subjects.push(subject.clone());
            }
        }
    }

    Some(merged)
}

/// Merge existing identifiers with new identifiers (never replace, always add).
///
/// This ensures we never lose identifiers when updating metadata.
pub fn merge_identifiers(existing: Option<&Value>, new: &Value) -> Value {
    let mut merged: Map<String, Value> = existing
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    if let Some(new_obj) = new.as_object() {
        for (key, value) in new_obj {
            merged.insert(key.clone(), value.clone());
        }
    }

    Value::Object(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_result(provider: &str) -> MetadataResult {
        let mut r = MetadataResult::new(provider, String::new());
        r.external_id = Some(format!("{}_id", provider));
        r
    }

    #[test]
    fn test_empty_results() {
        assert!(merge_by_priority(&[]).is_none());
    }

    #[test]
    fn test_single_result() {
        let result = make_result("google");
        let merged = merge_by_priority(std::slice::from_ref(&result)).unwrap();
        assert_eq!(merged.provider, "google");
    }

    #[test]
    fn test_identifiers_aggregated() {
        let mut r1 = make_result("google");
        r1.isbn = Some("1234567890123".to_string());

        let mut r2 = make_result("hardcover");
        r2.isbn = Some("0123456789".to_string());

        let merged = merge_by_priority(&[r1, r2]).unwrap();
        assert_eq!(merged.provider, "merged");

        let ids = merged.identifiers.as_object().unwrap();
        assert!(ids.contains_key("google"));
        assert!(ids.contains_key("hardcover"));
        // First result's ISBN13 is used
        assert!(ids.contains_key("isbn13"));
    }

    #[test]
    fn test_subjects_unioned() {
        let mut r1 = make_result("google");
        r1.subjects = vec!["Fiction".to_string(), "Fantasy".to_string()];

        let mut r2 = make_result("hardcover");
        r2.subjects = vec!["fantasy".to_string(), "Science Fiction".to_string()]; // "fantasy" is duplicate (case insensitive)

        let merged = merge_by_priority(&[r1, r2]).unwrap();
        assert_eq!(merged.subjects.len(), 3);
        assert!(merged.subjects.iter().any(|s| s == "Fiction"));
        assert!(merged.subjects.iter().any(|s| s == "Fantasy"));
        assert!(merged.subjects.iter().any(|s| s == "Science Fiction"));
    }

    #[test]
    fn test_priority_ordering() {
        let mut r1 = make_result("google");
        r1.title = "Google Title".to_string();
        r1.description = None;

        let mut r2 = make_result("hardcover");
        r2.title = "Hardcover Title".to_string();
        r2.description = Some("Hardcover description".to_string());

        let merged = merge_by_priority(&[r1, r2]).unwrap();
        // Title from first (higher priority)
        assert_eq!(merged.title, "Google Title");
        // Description from second (first was None)
        assert_eq!(
            merged.description,
            Some("Hardcover description".to_string())
        );
    }
}
