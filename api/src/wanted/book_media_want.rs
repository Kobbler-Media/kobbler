use nats_jobs::{BookIdentifiers, MediaWant};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use super::book_titles::{BookNames, BookRef, Lookup, book_names};
use crate::metadata::ProviderRegistry;
use crate::routes::ingest::{AUDIOBOOK_EXTENSIONS, EBOOK_EXTENSIONS};

pub struct WantedBook<'a> {
    pub id: Uuid,
    pub title: &'a str,
    pub author: Option<&'a str>,
    pub narrator: Option<&'a str>,
    pub identifiers: &'a Value,
    pub series_name: Option<&'a str>,
    pub series_index: Option<f64>,
    pub pubdate: Option<time::OffsetDateTime>,
    pub language: Option<&'a str>,
}

/// What Kobload is told about a wanted book, in either format.
pub struct BookWant<'a> {
    book: &'a WantedBook<'a>,
    identifiers: BookIdentifiers,
    names: BookNames,
    language: Option<String>,
}

fn identifier(json: &Value, key: &str) -> Option<String> {
    json.get(key).and_then(|v| v.as_str()).map(String::from)
}

/// The book's own language, or the household's when the book names none.
/// English name, lowercase, as Kobload's language table spells it.
pub fn want_language(book_language: Option<&str>, household: &str) -> Option<String> {
    use crate::streaming::language::name_of;
    book_language
        .and_then(name_of)
        .or_else(|| name_of(household))
        .map(str::to_lowercase)
}

impl<'a> BookWant<'a> {
    /// `household_language` is the playback language preference, used when the
    /// book itself names none.
    pub async fn load(pool: &PgPool, book: &'a WantedBook<'a>, household_language: &str) -> Self {
        let identifiers = BookIdentifiers {
            isbn10: identifier(book.identifiers, "isbn10"),
            isbn13: identifier(book.identifiers, "isbn13"),
            hardcover_id: identifier(book.identifiers, "hardcover"),
            goodreads_id: identifier(book.identifiers, "goodreads"),
            openlibrary_id: identifier(book.identifiers, "openlibrary"),
        };
        let names = book_names(
            &ProviderRegistry::new(pool.clone()),
            pool,
            book.id,
            &BookRef {
                title: book.title,
                series: book.series_name,
                position: book.series_index,
                hardcover_id: identifiers.hardcover_id.as_deref(),
            },
            Lookup::Fetch,
        )
        .await;
        Self {
            book,
            identifiers,
            names,
            language: want_language(book.language, household_language),
        }
    }

    pub fn isbn(&self) -> Option<String> {
        self.identifiers
            .isbn13
            .clone()
            .or_else(|| self.identifiers.isbn10.clone())
    }

    pub fn media_want(&self, audiobook: bool) -> MediaWant {
        let book = self.book;
        let title = book.title.to_string();
        let author = book.author.map(String::from);
        let isbn = self.isbn();
        let identifiers = Some(self.identifiers.clone());
        let series = book.series_name.map(String::from);
        let series_position = book.series_index.map(|idx| idx.to_string());
        let release_date = book.pubdate.map(|dt| dt.date().to_string());
        let alternate_titles = self.names.alternates.clone();
        let other_titles = self.names.others.clone();
        let language = self.language.clone();
        if audiobook {
            MediaWant::Audiobook {
                title,
                author,
                narrator: book.narrator.map(String::from),
                isbn,
                format_preferences: AUDIOBOOK_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
                identifiers,
                series,
                series_position,
                release_date,
                alternate_titles,
                other_titles,
                language,
            }
        } else {
            MediaWant::Book {
                title,
                author,
                isbn,
                format_preferences: EBOOK_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
                identifiers,
                series,
                series_position,
                release_date,
                alternate_titles,
                other_titles,
                language,
            }
        }
    }
}
