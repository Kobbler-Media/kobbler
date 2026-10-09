//! UUID extraction and batch entity resolution for proxy log enrichment.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

/// Maximum number of UUIDs to resolve in a single batch.
const MAX_BATCH_SIZE: usize = 500;

static UUID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
        .unwrap()
});

/// A resolved entity reference stored in proxy log JSONB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityRef {
    pub uuid: Uuid,
    pub entity_type: String,
    pub name: String,
    pub source: String,
}

/// A resolved entity from the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedEntity {
    pub entity_type: String,
    pub name: String,
}

/// Extract all UUIDs from a text string.
pub fn extract_uuids(text: &str) -> HashSet<Uuid> {
    UUID_RE
        .find_iter(text)
        .filter_map(|m| Uuid::parse_str(m.as_str()).ok())
        .collect()
}

/// Batch-resolve UUIDs against entity tables.
///
/// Returns a map of UUID -> ResolvedEntity for any UUIDs that match
/// books, shelves, series, authors, or devices.
pub async fn batch_resolve(pool: &PgPool, uuids: &[Uuid]) -> HashMap<Uuid, ResolvedEntity> {
    if uuids.is_empty() {
        return HashMap::new();
    }

    // Cap the batch size
    let uuids = if uuids.len() > MAX_BATCH_SIZE {
        &uuids[..MAX_BATCH_SIZE]
    } else {
        uuids
    };

    let rows = sqlx::query!(
        r#"
        WITH input_ids AS (SELECT unnest($1::uuid[]) AS id)
        SELECT i.id AS "id!", 'book' AS "entity_type!", b.title AS "name!"
          FROM input_ids i JOIN books b ON b.id = i.id
        UNION ALL
        SELECT i.id, 'shelf', s.name
          FROM input_ids i JOIN shelves s ON s.id = i.id
        UNION ALL
        SELECT i.id, 'series', s.name
          FROM input_ids i JOIN series s ON s.id = i.id
        UNION ALL
        SELECT i.id, 'author', a.name
          FROM input_ids i JOIN authors a ON a.id = i.id
        UNION ALL
        SELECT i.id, 'device', d.name
          FROM input_ids i JOIN devices d ON d.id = i.id
        "#,
        uuids,
    )
    .fetch_all(pool)
    .await;

    match rows {
        Ok(rows) => {
            let mut map = HashMap::new();
            for row in rows {
                map.insert(
                    row.id,
                    ResolvedEntity {
                        entity_type: row.entity_type,
                        name: row.name,
                    },
                );
            }
            map
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to batch-resolve entity UUIDs");
            HashMap::new()
        }
    }
}

/// Entity types that indicate a local leak when found in non-intercepted requests.
const LEAK_ENTITY_TYPES: &[&str] = &["book", "shelf", "series"];

/// Sources that represent outbound data (sent *to* Kobo).
/// Response/upstream bodies are irrelevant — they're data we built or Kobo sent back.
const OUTBOUND_SOURCES: &[&str] = &["path", "request_body"];

/// Check whether entity refs indicate a local entity leak.
///
/// A leak means local entities (book/shelf/series) appear in outbound sources
/// (path or request body) of a non-intercepted request.
fn is_leak(decision: &str, entity_refs: &[EntityRef]) -> bool {
    decision != "intercept"
        && entity_refs.iter().any(|r| {
            LEAK_ENTITY_TYPES.contains(&r.entity_type.as_str())
                && OUTBOUND_SOURCES.contains(&r.source.as_str())
        })
}

/// Scan text sources, resolve entities, and determine leak status.
///
/// Returns (entity_refs, has_entity_leak).
pub async fn scan_and_resolve(
    pool: &PgPool,
    decision: &str,
    path: &str,
    request_body: Option<&str>,
    response_body: Option<&str>,
    upstream_body: Option<&str>,
) -> (Vec<EntityRef>, bool) {
    // Extract UUIDs from each source, tracking provenance
    let mut uuid_sources: HashMap<Uuid, HashSet<&str>> = HashMap::new();

    for (text, source) in [
        (Some(path), "path"),
        (request_body, "request_body"),
        (response_body, "response_body"),
        (upstream_body, "upstream_body"),
    ] {
        if let Some(text) = text {
            for uuid in extract_uuids(text) {
                uuid_sources.entry(uuid).or_default().insert(source);
            }
        }
    }

    if uuid_sources.is_empty() {
        return (vec![], false);
    }

    // Batch-resolve all unique UUIDs
    let all_uuids: Vec<Uuid> = uuid_sources.keys().copied().collect();
    let resolved = batch_resolve(pool, &all_uuids).await;

    // Build entity refs
    let mut entity_refs = Vec::new();
    for (uuid, sources) in &uuid_sources {
        if let Some(entity) = resolved.get(uuid) {
            for source in sources {
                entity_refs.push(EntityRef {
                    uuid: *uuid,
                    entity_type: entity.entity_type.clone(),
                    name: entity.name.clone(),
                    source: (*source).to_string(),
                });
            }
        }
    }

    // Sort for deterministic output
    entity_refs.sort_by(|a, b| a.source.cmp(&b.source).then(a.uuid.cmp(&b.uuid)));

    let has_entity_leak = is_leak(decision, &entity_refs);

    (entity_refs, has_entity_leak)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_uuids_from_path() {
        let path = "/v1/library/12345678-1234-1234-1234-123456789abc/metadata";
        let uuids = extract_uuids(path);
        assert_eq!(uuids.len(), 1);
        assert!(uuids.contains(&Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap()));
    }

    #[test]
    fn test_extract_uuids_multiple() {
        let text = "/v1/library/tags/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee/items with 11111111-2222-3333-4444-555555555555";
        let uuids = extract_uuids(text);
        assert_eq!(uuids.len(), 2);
    }

    #[test]
    fn test_extract_uuids_none() {
        let text = "/v1/library/sync";
        let uuids = extract_uuids(text);
        assert!(uuids.is_empty());
    }

    #[test]
    fn test_extract_uuids_case_insensitive() {
        let text = "AABBCCDD-1122-3344-5566-778899AABBCC";
        let uuids = extract_uuids(text);
        assert_eq!(uuids.len(), 1);
    }

    #[test]
    fn test_extract_uuids_from_json_body() {
        let body = r#"{"RevisionId":"12345678-abcd-ef01-2345-6789abcdef01","Items":[{"RevisionId":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"}]}"#;
        let uuids = extract_uuids(body);
        assert_eq!(uuids.len(), 2);
    }

    #[test]
    fn test_extract_uuids_deduplicates() {
        let text = "12345678-1234-1234-1234-123456789abc appears twice: 12345678-1234-1234-1234-123456789abc";
        let uuids = extract_uuids(text);
        assert_eq!(uuids.len(), 1);
    }

    fn make_ref(entity_type: &str, source: &str) -> EntityRef {
        EntityRef {
            uuid: Uuid::nil(),
            entity_type: entity_type.to_string(),
            name: "test".to_string(),
            source: source.to_string(),
        }
    }

    #[test]
    fn test_is_leak_response_body_modify_not_leak() {
        let refs = vec![make_ref("book", "response_body")];
        assert!(!is_leak("modify", &refs));
    }

    #[test]
    fn test_is_leak_upstream_body_modify_not_leak() {
        let refs = vec![make_ref("book", "upstream_body")];
        assert!(!is_leak("modify", &refs));
    }

    #[test]
    fn test_is_leak_path_modify_is_leak() {
        let refs = vec![make_ref("book", "path")];
        assert!(is_leak("modify", &refs));
    }

    #[test]
    fn test_is_leak_request_body_passthrough_is_leak() {
        let refs = vec![make_ref("shelf", "request_body")];
        assert!(is_leak("passthrough", &refs));
    }

    #[test]
    fn test_is_leak_path_intercept_not_leak() {
        let refs = vec![make_ref("book", "path")];
        assert!(!is_leak("intercept", &refs));
    }

    #[test]
    fn test_is_leak_author_path_modify_not_leak() {
        let refs = vec![make_ref("author", "path")];
        assert!(!is_leak("modify", &refs));
    }
}
