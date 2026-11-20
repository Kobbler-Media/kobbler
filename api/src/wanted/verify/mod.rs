//! Before a downloaded file is filed as a wanted book, check that the file
//! itself agrees: the release name chose it, but only the book's own metadata,
//! the opening of its text, or the opening of its narration says what it is.

mod inspect;
mod judge;

pub use inspect::{Inspector, inspect};
pub use judge::{Found, Verdict, judge};

use sqlx::PgPool;
use uuid::Uuid;

use super::book_media_want::want_language;
use super::book_titles::{BookRef, Lookup, book_names};
use crate::metadata::ProviderRegistry;
use crate::repositories::books;

pub struct Expected {
    pub title: String,
    pub alternates: Vec<String>,
    pub others: Vec<String>,
    pub authors: Vec<String>,
    pub year: Option<i32>,
    /// English name, as [`want_language`] gives it.
    pub language: Option<String>,
}

impl Expected {
    /// Names come from the cache the want filled; an import never waits on
    /// Hardcover.
    pub async fn for_book(pool: &PgPool, book_id: Uuid, household_language: &str) -> Option<Self> {
        let meta = books::get_wanted_metadata(pool, book_id).await.ok()??;
        let names = book_names(
            &ProviderRegistry::new(pool.clone()),
            pool,
            book_id,
            &BookRef {
                title: &meta.title,
                series: meta.series_name.as_deref(),
                position: meta.series_index,
                hardcover_id: meta.identifiers.get("hardcover").and_then(|v| v.as_str()),
            },
            Lookup::CacheOnly,
        )
        .await;
        Some(Self {
            year: meta.pubdate.map(|d| d.year()),
            language: want_language(meta.language.as_deref(), household_language),
            title: meta.title,
            alternates: names.alternates.unwrap_or_default(),
            others: names.others.unwrap_or_default(),
            // The library joins a book's authors with ", ".
            authors: meta
                .author
                .iter()
                .flat_map(|a| a.split(", "))
                .map(str::trim)
                .filter(|a| !a.is_empty())
                .map(String::from)
                .collect(),
        })
    }
}

/// The release brought the wrong book: never take it again, and look for the
/// right one.
pub async fn reject_release(
    pool: &PgPool,
    nats: &nats_jobs::Publisher,
    book_id: Uuid,
    release_title: Option<&str>,
    reason: &str,
) {
    let mut entry = serde_json::json!({ "reason": format!("wrong book: {reason}") });
    if let Some(title) = release_title {
        entry["release_title"] = title.into();
    }
    if let Err(e) = books::add_rejected_release(pool, book_id, entry).await {
        tracing::warn!(error = %e, %book_id, "Failed to record the rejected release");
    }
    let _ = books::set_want_status(pool, book_id, "wanted").await;
    if let Err(e) = crate::jobs::enqueue(
        pool,
        nats,
        "book_want",
        serde_json::json!({ "book_id": book_id }),
        nats_jobs::JobContext::for_entity("book", book_id, "Searching again after a wrong book"),
    )
    .await
    {
        tracing::warn!(error = %e, %book_id, "Failed to enqueue book_want after a wrong book");
    }
}
