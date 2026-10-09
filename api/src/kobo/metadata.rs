//! Book metadata building for Kobo sync.

use isolang::Language;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use tracing::warn;
use uuid::Uuid;

use crate::kobo::types::format_rfc3339;
use crate::repositories::{authors, book_files, books};

/// Input for building book metadata.
pub struct MetadataInput {
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
    pub page_count: Option<i32>,
    pub rating: Option<f64>,
}

/// Build Kobo metadata JSON for a book (fetches files/authors from DB).
pub async fn build_metadata(pool: &PgPool, base_url: &str, input: MetadataInput) -> Value {
    let book_id = input.book_id;

    let files = book_files::list_format_and_size(pool, book_id)
        .await
        .unwrap_or_else(|e| {
            warn!(book_id = %book_id, error = %e, "Failed to fetch book files");
            Vec::new()
        });

    let author_names = authors::get_book_author_names(pool, book_id)
        .await
        .unwrap_or_else(|e| {
            warn!(book_id = %book_id, error = %e, "Failed to fetch book authors");
            Vec::new()
        });

    build_metadata_from_data(base_url, input, &files, &author_names)
}

/// Build Kobo metadata JSON from pre-fetched data (no DB access).
pub fn build_metadata_from_data(
    base_url: &str,
    input: MetadataInput,
    files: &[book_files::FileFormatInfo],
    author_names: &[String],
) -> Value {
    let MetadataInput {
        book_id,
        title,
        description,
        language,
        publisher,
        pubdate,
        series_id,
        series_name,
        series_index,
        identifiers,
        page_count,
        rating,
    } = input;

    let mut download_urls = Vec::new();
    let has_kepub = files.iter().any(|f| f.format.to_uppercase() == "KEPUB");

    // Pick content_revision from preferred file (KEPUB > EPUB) for RevisionId
    let content_revision = files
        .iter()
        .find(|f| {
            let fmt = f.format.to_uppercase();
            if has_kepub {
                fmt == "KEPUB"
            } else {
                fmt == "EPUB"
            }
        })
        .map(|f| f.content_revision);

    for file in files.iter() {
        let format = file.format.to_uppercase();
        if has_kepub && format != "KEPUB" {
            continue;
        }
        if !has_kepub && format != "EPUB" {
            continue;
        }

        let size = file.size_bytes;
        let url = format!(
            "{}/download/{}/{}",
            base_url,
            book_id,
            format.to_lowercase()
        );
        if format == "EPUB" {
            download_urls.push(json!({
                "Format": "EPUB3",
                "Size": size,
                "Url": url,
                "Platform": "Generic"
            }));
            download_urls.push(json!({
                "Format": "EPUB",
                "Size": size,
                "Url": url,
                "Platform": "Generic"
            }));
        } else {
            download_urls.push(json!({
                "Format": format,
                "Size": size,
                "Url": url,
                "Platform": "Generic"
            }));
        }
    }

    let contributor_roles: Vec<Value> = author_names
        .iter()
        .map(|name| json!({ "Name": name, "Role": "Author" }))
        .collect();
    let author_names: Vec<&str> = author_names.iter().map(|s| s.as_str()).collect();

    if download_urls.is_empty() {
        warn!(book_id = %book_id, "No Kobo-compatible formats available for download");
    }

    if title.trim().is_empty() {
        warn!(book_id = %book_id, "Book title is empty");
    }

    // Build ExternalIds from stored identifiers
    let mut external_ids: Vec<Value> = Vec::new();
    if let Some(ref ids) = identifiers
        && let Some(isbn) = ids.get("isbn").and_then(|v| v.as_str())
    {
        external_ids.push(json!({"Id": isbn, "Type": "ISBN"}));
    }

    let mut metadata = json!({
        "Categories": ["00000000-0000-0000-0000-000000000001"],
        "CoverImageId": book_id,
        "CrossRevisionId": book_id,
        "CurrentDisplayPrice": {"CurrencyCode": "USD", "TotalAmount": 0},
        "CurrentLoveDisplayPrice": {"TotalAmount": 0},
        "Description": description,
        "DownloadUrls": download_urls,
        "EntitlementId": book_id,
        "ExternalIds": external_ids,
        "Genre": "00000000-0000-0000-0000-000000000001",
        "IsEligibleForKoboLove": false,
        "IsInternetArchive": false,
        "IsPreOrder": false,
        "IsSocialEnabled": true,
        "Language": normalize_language(language),
        "Contributors": author_names,
        "ContributorRoles": contributor_roles,
        "PhoneticPronunciations": {},
        "PublicationDate": pubdate.map(format_rfc3339),
        "Publisher": {"Imprint": "", "Name": publisher},
        "RevisionId": content_revision.unwrap_or(book_id),
        "Title": title,
        "WorkId": book_id,
    });

    if let Some(pages) = page_count {
        metadata["NumberOfPages"] = json!(pages);
    }
    if let Some(r) = rating {
        metadata["Rating"] = json!(r);
    }

    if let (Some(id), Some(name)) = (series_id, series_name) {
        let number_float = series_index.unwrap_or(1.0);
        let number = series_index.map(|f| f as i32).unwrap_or(1);
        metadata["Series"] = json!({
            "Name": name,
            "Number": number,
            "NumberFloat": number_float,
            "Id": id
        });
    }

    metadata
}

/// Normalize language code to ISO 639-1 two-letter code.
fn normalize_language(language: Option<String>) -> String {
    let raw = language.unwrap_or_else(|| "en".to_string());
    let code = raw.trim().to_lowercase();
    if code.len() == 2 {
        return code;
    }
    if code.len() == 3
        && let Some(lang) = Language::from_639_3(&code)
        && let Some(two) = lang.to_639_1()
    {
        return two.to_string();
    }
    "en".to_string()
}

/// Fetch metadata for a book from the database.
pub async fn fetch_book_metadata(pool: &PgPool, book_id: Uuid) -> Option<MetadataInput> {
    let info = books::get_for_kobo_sync(pool, book_id)
        .await
        .ok()
        .flatten()?;

    Some(MetadataInput {
        book_id: info.book_id,
        title: info.title,
        description: info.description,
        language: info.language,
        publisher: info.publisher,
        pubdate: info.pubdate,
        series_id: info.series_id,
        series_name: info.series_name,
        series_index: info.series_index,
        identifiers: info.identifiers,
        page_count: info.page_count,
        rating: info.rating,
    })
}
