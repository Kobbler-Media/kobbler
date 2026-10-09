//! Book recommendations for Kobo nextread endpoint.

use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use tracing::error;
use uuid::Uuid;

use crate::kobo::metadata::MetadataInput;
use crate::kobo::types::format_rfc3339;
use crate::repositories::books;

/// Reason why a book was recommended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecommendationReason {
    /// Next book in the same series
    NextInSeries,
    /// Another book by the same author
    SameAuthor,
}

/// A book recommendation candidate.
pub struct RecommendationCandidate {
    pub book_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<OffsetDateTime>,
    pub series_id: Option<Uuid>,
    pub series_name: Option<String>,
    pub series_index: Option<f64>,
    pub identifiers: Option<serde_json::Value>,
    pub authors: Vec<String>,
    pub reason: RecommendationReason,
    /// Reading progress for this book (0-100), None if never opened.
    pub progress_percent: Option<f64>,
}

impl From<&RecommendationCandidate> for MetadataInput {
    fn from(c: &RecommendationCandidate) -> Self {
        MetadataInput {
            book_id: c.book_id,
            title: c.title.clone(),
            description: c.description.clone(),
            language: c.language.clone(),
            publisher: c.publisher.clone(),
            pubdate: c.pubdate,
            series_id: c.series_id,
            series_name: c.series_name.clone(),
            series_index: c.series_index,
            identifiers: c.identifiers.clone(),
            page_count: None,
            rating: None,
        }
    }
}

/// Get book recommendations for a given book.
///
/// Returns recommendations in priority order:
/// 1. Next book(s) in the same series
/// 2. Other books by the same author(s)
///
/// Only returns books that are entitled to the user/device.
pub async fn get_recommendations(
    pool: &PgPool,
    book_id: Uuid,
    user_id: Uuid,
    device_id: Uuid,
    limit: i32,
) -> Vec<RecommendationCandidate> {
    let rows = match books::get_recommendations(pool, book_id, user_id, device_id, limit).await {
        Ok(rows) => rows,
        Err(e) => {
            error!(error = %e, book_id = %book_id, "Failed to fetch recommendations");
            return Vec::new();
        }
    };

    rows.into_iter()
        .map(|row| {
            let reason = if row.reason == "next_in_series" {
                RecommendationReason::NextInSeries
            } else {
                RecommendationReason::SameAuthor
            };

            RecommendationCandidate {
                book_id: row.book_id,
                title: row.title,
                description: row.description,
                language: row.language,
                publisher: row.publisher,
                pubdate: row.pubdate,
                series_id: row.series_id,
                series_name: row.series_name,
                series_index: row.series_index,
                identifiers: row.identifiers,
                authors: row.authors,
                reason,
                progress_percent: row.progress_percent,
            }
        })
        .collect()
}

/// Build a Kobo product recommendation JSON object.
///
/// This builds the format expected by the /v1/products/{id}/nextread endpoint.
pub fn build_product_recommendation(candidate: &RecommendationCandidate) -> Value {
    let contributor_roles: Vec<Value> = candidate
        .authors
        .iter()
        .map(|name| json!({"Name": name, "Role": "Author"}))
        .collect();

    let contributors = candidate.authors.join(", ");

    // Generate a slug from title (lowercase, spaces to dashes, alphanumeric only)
    let slug: String = candidate
        .title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    let mut product = json!({
        "Id": candidate.book_id,
        "Title": candidate.title,
        "Contributors": contributors,
        "ContributorRoles": contributor_roles,
        "Description": candidate.description.clone().unwrap_or_default(),
        "Language": candidate.language.clone().unwrap_or_else(|| "en".to_string()),
        "ImageId": candidate.book_id,
        "PublisherName": candidate.publisher.clone().unwrap_or_default(),
        "PublicationDate": candidate.pubdate.map(format_rfc3339),
        "WorkId": candidate.book_id,
        "CrossRevisionId": candidate.book_id,
        "Slug": slug,
        "IsFree": true,
        "IsPreOrder": false,
        "IsInternetArchive": false,
        "IsRecommendation": true,
        "HasPreview": false,
        "Price": {"Currency": "USD", "Price": 0},
        "PromoCodeAllowed": false,
        "EligibleForKoboLoveDiscount": false,
        "AgeVerificationRequired": false,
        "Rating": 0,
        "TotalRating": 0,
        "RedirectPreviewUrls": [],
        "AccessibilityDetails": {
            "IsFixedLayout": false,
            "IsTextToSpeechAllowed": false,
            "PrimaryContentType": "",
            "ContentTypes": [],
            "EPubAccessibilities": [],
            "HazardWarningTypes": [],
            "IsAccessible": false
        }
    });

    // Add series info if present
    if let (Some(series_id), Some(series_name)) = (&candidate.series_id, &candidate.series_name) {
        let number_float = candidate.series_index.unwrap_or(1.0);
        let number = candidate.series_index.map(|f| f as i32).unwrap_or(1);
        product["SeriesId"] = json!(series_id);
        product["SeriesName"] = json!(series_name);
        product["SeriesNumber"] = json!(number.to_string());
        product["SeriesNumberFloat"] = json!(number_float);
    }

    product
}
