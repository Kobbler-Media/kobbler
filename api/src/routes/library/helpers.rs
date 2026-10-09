//! Internal helper functions for library routes.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use time::{Date, Month, OffsetDateTime, Time, UtcOffset, format_description::well_known::Rfc3339};
use uuid::Uuid;

use sqlx::PgPool;

use crate::AppState;
use crate::repositories::{PaginationDirection, activity_log, authors, ingest_events};

// Re-export from kobo::types to avoid duplication
pub use crate::kobo::types::max_dt;

// =============================================================================
// Date Parsing Helper
// =============================================================================

/// Parse a date string in various formats:
/// - RFC3339 (2024-01-15T00:00:00Z)
/// - ISO date (2024-01-15)
/// - Year-month (2024-01)
/// - Year only (2024)
pub fn parse_flexible_date(s: &str) -> Option<OffsetDateTime> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Try RFC3339 first
    if let Ok(dt) = OffsetDateTime::parse(trimmed, &Rfc3339) {
        return Some(dt);
    }

    // Try ISO date (YYYY-MM-DD)
    if trimmed.len() == 10
        && trimmed.chars().nth(4) == Some('-')
        && let Ok(date) = Date::parse(
            trimmed,
            &time::format_description::parse_borrowed::<2>("[year]-[month]-[day]").ok()?,
        )
    {
        return Some(date.with_time(Time::MIDNIGHT).assume_offset(UtcOffset::UTC));
    }

    // Try year-month (YYYY-MM)
    if trimmed.len() == 7 && trimmed.chars().nth(4) == Some('-') {
        let parts: Vec<&str> = trimmed.split('-').collect();
        if parts.len() == 2
            && let (Ok(year), Ok(month)) = (parts[0].parse::<i32>(), parts[1].parse::<u8>())
            && let Ok(month) = Month::try_from(month)
            && let Ok(date) = Date::from_calendar_date(year, month, 1)
        {
            return Some(date.with_time(Time::MIDNIGHT).assume_offset(UtcOffset::UTC));
        }
    }

    // Try year only (YYYY)
    if trimmed.len() == 4
        && let Ok(year) = trimmed.parse::<i32>()
        && let Ok(date) = Date::from_calendar_date(year, Month::January, 1)
    {
        return Some(date.with_time(Time::MIDNIGHT).assume_offset(UtcOffset::UTC));
    }

    None
}

// =============================================================================
// Search Query Parser
// =============================================================================

/// A parsed search term
#[derive(Debug, Clone)]
pub enum SearchTerm {
    /// Include results matching this pattern
    Include(String),
    /// Exclude results matching this pattern
    Exclude(String),
}

/// Parse a search query into terms
/// Supports:
/// - `!term` - exclude matches
/// - `"exact phrase"` - exact phrase (still uses ILIKE but as single unit)
/// - Multiple terms - ANDed together
pub fn parse_search_query(query: &str) -> Vec<SearchTerm> {
    let mut terms = Vec::new();
    let mut current_term = String::new();
    let mut is_exclude = false;
    let mut in_quotes = false;

    for c in query.chars() {
        match c {
            '"' => {
                if in_quotes {
                    // End of quoted section
                    in_quotes = false;
                    if !current_term.is_empty() {
                        let term = std::mem::take(&mut current_term);
                        if is_exclude {
                            terms.push(SearchTerm::Exclude(term));
                        } else {
                            terms.push(SearchTerm::Include(term));
                        }
                        is_exclude = false;
                    }
                } else {
                    // Start of quoted section
                    in_quotes = true;
                }
            }
            '!' if !in_quotes && current_term.is_empty() => {
                is_exclude = true;
            }
            ' ' | '\t' if !in_quotes => {
                // End of term
                if !current_term.is_empty() {
                    let term = std::mem::take(&mut current_term);
                    if is_exclude {
                        terms.push(SearchTerm::Exclude(term));
                    } else {
                        terms.push(SearchTerm::Include(term));
                    }
                    is_exclude = false;
                }
            }
            _ => {
                current_term.push(c);
            }
        }
    }

    // Don't forget the last term
    if !current_term.is_empty() {
        if is_exclude {
            terms.push(SearchTerm::Exclude(current_term));
        } else {
            terms.push(SearchTerm::Include(current_term));
        }
    }

    terms
}

/// Build SQL search conditions from parsed terms
/// Returns (condition_sql, bindings) where bindings are the ILIKE patterns or UUIDs
#[allow(dead_code)] // Used in tests
pub fn build_search_conditions(terms: &[SearchTerm]) -> (String, Vec<String>) {
    if terms.is_empty() {
        return (String::new(), Vec::new());
    }

    let mut conditions = Vec::new();
    let mut bindings = Vec::new();

    for term in terms {
        let (raw_term, is_exclude) = match term {
            SearchTerm::Include(t) => (t.trim(), false),
            SearchTerm::Exclude(t) => (t.trim(), true),
        };

        if raw_term.is_empty() {
            continue;
        }

        // Check if term looks like a UUID - match book ID, series ID, or author ID
        if let Ok(uuid) = uuid::Uuid::parse_str(raw_term) {
            let param_idx = bindings.len() + 1;
            bindings.push(uuid.to_string());

            // Cast the text parameter to UUID for comparison with UUID columns
            let uuid_match = format!(
                "(b.id = ${p}::uuid OR EXISTS (
                    SELECT 1 FROM book_series bs_uuid
                    WHERE bs_uuid.book_id = b.id AND bs_uuid.series_id = ${p}::uuid
                ) OR EXISTS (
                    SELECT 1 FROM book_authors ba_uuid
                    WHERE ba_uuid.book_id = b.id AND ba_uuid.author_id = ${p}::uuid
                ))",
                p = param_idx
            );

            let condition = if is_exclude {
                format!("NOT {}", uuid_match)
            } else {
                uuid_match
            };
            conditions.push(condition);
            continue;
        }

        // Regular text search pattern
        let pattern = format!("%{}%", raw_term);

        if pattern.len() <= 2 {
            // Skip empty patterns (just %%)
            continue;
        }

        let param_idx = bindings.len() + 1;
        bindings.push(pattern);

        // Build condition that matches title, series name, OR author
        // Use COALESCE for series name to handle NULLs (books without series)
        let condition = if is_exclude {
            format!(
                "NOT (b.title ILIKE ${p} OR COALESCE(s.name ILIKE ${p}, FALSE) OR EXISTS (
                    SELECT 1 FROM book_authors ba_exc
                    JOIN authors a_exc ON a_exc.id = ba_exc.author_id
                    WHERE ba_exc.book_id = b.id AND a_exc.name ILIKE ${p}
                ))",
                p = param_idx
            )
        } else {
            format!(
                "(b.title ILIKE ${p} OR COALESCE(s.name ILIKE ${p}, FALSE) OR EXISTS (
                    SELECT 1 FROM book_authors ba_inc
                    JOIN authors a_inc ON a_inc.id = ba_inc.author_id
                    WHERE ba_inc.book_id = b.id AND a_inc.name ILIKE ${p}
                ))",
                p = param_idx
            )
        };

        conditions.push(condition);
    }

    if conditions.is_empty() {
        return (String::new(), Vec::new());
    }

    // All conditions are ANDed together
    let sql = conditions.join(" AND ");
    (sql, bindings)
}

// =============================================================================
// Cursor Helpers
// =============================================================================

#[derive(Serialize, Deserialize)]
pub struct Cursor {
    pub id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub ts: OffsetDateTime,
}

#[derive(Serialize, Deserialize)]
pub struct NameCursor {
    pub id: Uuid,
    pub name: String,
}

pub fn decode_cursor(cursor: &str) -> Option<Cursor> {
    let bytes = URL_SAFE_NO_PAD.decode(cursor).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn encode_cursor(cursor: &Cursor) -> String {
    let json = serde_json::to_vec(cursor).unwrap_or_default();
    URL_SAFE_NO_PAD.encode(&json)
}

pub fn decode_name_cursor(cursor: &str) -> Option<NameCursor> {
    let bytes = URL_SAFE_NO_PAD.decode(cursor).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn encode_name_cursor(cursor: &NameCursor) -> String {
    let json = serde_json::to_vec(cursor).unwrap_or_default();
    URL_SAFE_NO_PAD.encode(&json)
}

/// Trim a `limit + 1` fetch down to the window and report what lies on either side.
///
/// Returns `(items, has_previous, has_next)`. `rows` must already be in ascending
/// display order, so the sentinel row sits at the front for a backward fetch and at
/// the back for a forward one — trimming the wrong end drops a row the caller has
/// never seen and leaves a hole in the list.
pub fn paginated_window<T>(
    rows: Vec<T>,
    limit: i64,
    direction: PaginationDirection,
    is_first_page: bool,
) -> (Vec<T>, bool, bool) {
    let overflow = rows.len() as i64 > limit;
    let limit = limit.max(0) as usize;
    match direction {
        PaginationDirection::Forward => (
            rows.into_iter().take(limit).collect(),
            !is_first_page,
            overflow,
        ),
        PaginationDirection::Backward => {
            let skip = rows.len().saturating_sub(limit);
            (
                rows.into_iter().skip(skip).collect(),
                overflow,
                !is_first_page,
            )
        }
    }
}

// =============================================================================
// Utility Functions
// =============================================================================

pub fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

pub async fn fetch_author_ids(state: &AppState, book_id: Uuid) -> Vec<Uuid> {
    authors::get_book_author_ids(&state.pool, book_id)
        .await
        .unwrap_or_default()
}

/// Build the cover URL for a book, but only if its images JSONB indicates a cover is available.
///
/// Checks for "cover" (with a usable image) OR "extracted" (the cover endpoint's fallback chain
/// serves extracted covers too). Returns `None` for books with no cover at all, avoiding 404s.
pub fn book_cover_url(images: &crate::images::ImageMap, book_id: Uuid) -> Option<String> {
    if images.get("cover").is_some_and(|e| e.has_image()) || images.contains_key("extracted") {
        Some(format!("/api/v1/library/books/{}/cover", book_id))
    } else {
        None
    }
}

/// Build the extracted cover URL for a book if its images JSONB has an "extracted" key.
pub fn extracted_cover_url(images: &crate::images::ImageMap, book_id: Uuid) -> Option<String> {
    if images.contains_key("extracted") {
        Some(format!("/api/v1/library/books/{}/extracted-cover", book_id))
    } else {
        None
    }
}

pub async fn record_event(
    state: &AppState,
    source: &str,
    book_id: Option<Uuid>,
    status: &str,
    message: Option<String>,
    device_id: Option<Uuid>,
) {
    if let Err(e) = ingest_events::create(
        &state.pool,
        source,
        book_id,
        status,
        message.as_deref(),
        device_id,
    )
    .await
    {
        tracing::error!(error = %e, source = %source, "failed to record event");
    }
}

/// Record a user-facing activity log entry.
///
/// Takes `&PgPool` instead of `&AppState` so it can be called from job handlers
/// that don't have access to AppState.
#[allow(clippy::too_many_arguments)]
pub async fn record_activity(
    pool: &PgPool,
    event_kind: &str,
    entity_type: Option<&str>,
    entity_id: Option<Uuid>,
    title: &str,
    subtitle: Option<&str>,
    image_url: Option<&str>,
    url: Option<&str>,
    user_id: Option<Uuid>,
) {
    let input = activity_log::CreateActivity {
        event_kind,
        entity_type,
        entity_id,
        title,
        subtitle,
        image_url,
        url,
        user_id,
    };
    if let Err(e) = activity_log::create(pool, &input).await {
        tracing::error!(error = %e, event_kind = %event_kind, "failed to record activity");
    }
}

/// Whoever asked for this entity over an authenticated path, if anyone did.
///
/// Scheduler- and importer-driven attempts leave `wanted_attempts.user_id` NULL, so `None`
/// genuinely means "nobody asked" rather than "we lost track".
pub async fn requester_of(pool: &PgPool, entity_id: Uuid) -> Option<Uuid> {
    crate::repositories::wanted_attempts::latest_requester(pool, entity_id)
        .await
        .unwrap_or(None)
}

/// Compute an image URL for the given entity type and ID.
fn entity_image_url(entity_type: &str, entity_id: Uuid) -> Option<String> {
    match entity_type {
        "book" => Some(format!("/api/v1/library/books/{entity_id}/cover")),
        "movie" => Some(format!("/api/v1/admin/movies/{entity_id}/poster")),
        "show" => Some(format!("/api/v1/admin/tv/shows/{entity_id}/poster")),
        _ => None,
    }
}

/// Record activity for an entity, resolving its name and URL automatically.
pub async fn record_activity_for_entity(
    pool: &PgPool,
    event_kind: &str,
    entity_id: Uuid,
    user_id: Option<Uuid>,
) {
    use crate::notifications::resolve_entity;

    if let Some(resolved) = resolve_entity(pool, entity_id).await {
        let image_url = entity_image_url(resolved.entity_type, entity_id);
        record_activity(
            pool,
            event_kind,
            Some(resolved.entity_type),
            Some(entity_id),
            &resolved.name,
            None,
            image_url.as_deref(),
            Some(&resolved.url),
            user_id,
        )
        .await;
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;

    #[test]
    fn test_parse_simple_term() {
        let terms = parse_search_query("hello");
        assert_eq!(terms.len(), 1);
        assert!(matches!(&terms[0], SearchTerm::Include(s) if s == "hello"));
    }

    #[test]
    fn test_parse_multiple_terms() {
        let terms = parse_search_query("hello world");
        assert_eq!(terms.len(), 2);
        assert!(matches!(&terms[0], SearchTerm::Include(s) if s == "hello"));
        assert!(matches!(&terms[1], SearchTerm::Include(s) if s == "world"));
    }

    #[test]
    fn test_parse_negation() {
        let terms = parse_search_query("will !wight");
        assert_eq!(terms.len(), 2);
        assert!(matches!(&terms[0], SearchTerm::Include(s) if s == "will"));
        assert!(matches!(&terms[1], SearchTerm::Exclude(s) if s == "wight"));
    }

    #[test]
    fn test_parse_quoted_phrase() {
        let terms = parse_search_query("\"cradle book\"");
        assert_eq!(terms.len(), 1);
        assert!(matches!(&terms[0], SearchTerm::Include(s) if s == "cradle book"));
    }

    #[test]
    fn test_parse_negated_quoted_phrase() {
        let terms = parse_search_query("!\"will wight\"");
        assert_eq!(terms.len(), 1);
        assert!(matches!(&terms[0], SearchTerm::Exclude(s) if s == "will wight"));
    }

    #[test]
    fn test_parse_complex_query() {
        let terms = parse_search_query("fantasy !\"young adult\" brandon");
        assert_eq!(terms.len(), 3);
        assert!(matches!(&terms[0], SearchTerm::Include(s) if s == "fantasy"));
        assert!(matches!(&terms[1], SearchTerm::Exclude(s) if s == "young adult"));
        assert!(matches!(&terms[2], SearchTerm::Include(s) if s == "brandon"));
    }

    #[test]
    fn test_build_conditions_empty() {
        let (sql, bindings) = build_search_conditions(&[]);
        assert!(sql.is_empty());
        assert!(bindings.is_empty());
    }

    #[test]
    fn test_build_conditions_single_include() {
        let terms = vec![SearchTerm::Include("test".to_string())];
        let (sql, bindings) = build_search_conditions(&terms);
        assert!(sql.contains("b.title ILIKE $1"));
        assert!(!sql.contains("NOT"));
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0], "%test%");
    }

    #[test]
    fn test_build_conditions_single_exclude() {
        let terms = vec![SearchTerm::Exclude("test".to_string())];
        let (sql, bindings) = build_search_conditions(&terms);
        assert!(sql.contains("NOT"));
        assert!(sql.contains("b.title ILIKE $1"));
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0], "%test%");
    }

    #[test]
    fn test_build_conditions_multiple() {
        let terms = vec![
            SearchTerm::Include("will".to_string()),
            SearchTerm::Exclude("wight".to_string()),
        ];
        let (sql, bindings) = build_search_conditions(&terms);
        assert!(sql.contains("$1"));
        assert!(sql.contains("$2"));
        assert!(sql.contains(" AND "));
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0], "%will%");
        assert_eq!(bindings[1], "%wight%");
    }

    #[test]
    fn test_build_conditions_uuid_matches_series_and_author() {
        let uuid = "12345678-1234-1234-1234-123456789abc";
        let terms = vec![SearchTerm::Include(uuid.to_string())];
        let (sql, bindings) = build_search_conditions(&terms);

        // Should match book ID, series ID (via book_series), and author ID with ::uuid cast
        assert!(sql.contains("b.id = $1::uuid"), "Should match book ID");
        assert!(
            sql.contains("bs_uuid.series_id = $1::uuid"),
            "Should match series ID via book_series"
        );
        assert!(
            sql.contains("ba_uuid.author_id = $1::uuid"),
            "Should match author ID via book_authors"
        );
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0], uuid);
    }

    #[test]
    fn test_build_conditions_uuid_exclude() {
        let uuid = "12345678-1234-1234-1234-123456789abc";
        let terms = vec![SearchTerm::Exclude(uuid.to_string())];
        let (sql, bindings) = build_search_conditions(&terms);

        // Should wrap all UUID conditions in NOT with ::uuid cast
        assert!(sql.starts_with("NOT ("), "Should start with NOT");
        assert!(sql.contains("b.id = $1::uuid"), "Should match book ID");
        assert!(
            sql.contains("bs_uuid.series_id = $1::uuid"),
            "Should match series ID via book_series"
        );
        assert!(
            sql.contains("ba_uuid.author_id = $1::uuid"),
            "Should match author ID via book_authors"
        );
        assert_eq!(bindings.len(), 1);
    }

    #[test]
    fn test_build_conditions_includes_series_name() {
        let terms = vec![SearchTerm::Include("cradle".to_string())];
        let (sql, bindings) = build_search_conditions(&terms);

        // Should search title, series name (with COALESCE for NULL handling), and author name
        assert!(sql.contains("b.title ILIKE $1"), "Should search title");
        assert!(
            sql.contains("COALESCE(s.name ILIKE $1, FALSE)"),
            "Should search series name with NULL handling"
        );
        assert!(
            sql.contains("a_inc.name ILIKE $1"),
            "Should search author name"
        );
        assert_eq!(bindings[0], "%cradle%");
    }
}
