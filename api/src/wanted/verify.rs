//! Before a downloaded file is filed as a wanted book, check that the file
//! itself agrees: the release name chose it, but only the book's own metadata
//! (or, for an ebook that names no author, its text) says what it is.

use std::path::Path;

use kob_normalize::titles_similar;
use sqlx::PgPool;
use uuid::Uuid;

use super::book_titles::{BookRef, Lookup, book_names};
use crate::metadata::ProviderRegistry;
use crate::repositories::books;

pub struct Expected {
    pub title: String,
    pub alternates: Vec<String>,
    pub others: Vec<String>,
    pub author: Option<String>,
    pub year: Option<i32>,
}

impl Expected {
    /// Names come from the cache the want filled; an import never waits on
    /// Hardcover.
    pub async fn for_book(pool: &PgPool, book_id: Uuid) -> Option<Self> {
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
            title: meta.title,
            alternates: names.alternates.unwrap_or_default(),
            others: names.others.unwrap_or_default(),
            author: meta.author,
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

#[derive(Debug, Default)]
pub struct Found {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub year: Option<i32>,
    /// Whether the wanted author's surname appears in the text. Only read for
    /// ebooks whose metadata names nobody.
    pub mentions_author: Option<bool>,
}

#[derive(Debug, PartialEq)]
pub enum Verdict {
    Matches,
    WrongBook(String),
}

fn name_tokens(name: &str) -> Vec<String> {
    kob_normalize::canonical(&name.replace('.', " "))
        .split_whitespace()
        .filter(|t| t.len() >= 2)
        .map(String::from)
        .collect()
}

fn same_author(wanted: &str, found: &str) -> bool {
    let found = name_tokens(found);
    name_tokens(wanted).iter().any(|t| found.contains(t))
}

fn surname(author: &str) -> Option<String> {
    name_tokens(author.split(',').next().unwrap_or(author))
        .pop()
        .filter(|s| s.len() >= 3)
}

fn year_of(date: &str) -> Option<i32> {
    date.trim()
        .get(..4)
        .and_then(|y| y.parse().ok())
        .filter(|y| (1450..=2200).contains(y))
}

pub fn judge(expected: &Expected, found: &Found) -> Verdict {
    let is_wanted = |title: &str| {
        std::iter::once(&expected.title)
            .chain(&expected.alternates)
            .any(|w| titles_similar(w, title) || titles_similar(title, w))
    };
    if let Some(title) = found.title.as_deref()
        && !is_wanted(title)
        && let Some(other) = expected.others.iter().find(|o| titles_similar(o, title))
    {
        return Verdict::WrongBook(format!("the file is {other}, not {}", expected.title));
    }

    let mut author_confirmed = false;
    if let Some(author) = expected.author.as_deref() {
        if !found.authors.is_empty() {
            if !found.authors.iter().any(|f| same_author(author, f)) {
                return Verdict::WrongBook(format!(
                    "the file is by {}, not {author}",
                    found.authors.join(", ")
                ));
            }
            author_confirmed = true;
        } else if found.mentions_author == Some(false) {
            return Verdict::WrongBook(format!("{author} appears nowhere in the file"));
        } else {
            author_confirmed = found.mentions_author == Some(true);
        }
    }

    if let (Some(found_year), Some(year)) = (found.year, expected.year)
        && found_year < year - 1
    {
        return Verdict::WrongBook(format!(
            "the file dates from {found_year}, before {} was published in {year}",
            expected.title
        ));
    }

    if !author_confirmed
        && let Some(title) = found.title.as_deref()
        && !is_wanted(title)
    {
        return Verdict::WrongBook(format!("the file is titled {title}"));
    }
    Verdict::Matches
}

const AUDIO_EXTENSIONS: &[&str] = &["m4b", "m4a", "mp3", "flac", "ogg", "opus", "aac", "wav"];

/// What the file says about itself, or `None` when it can't be read. An
/// unreadable file is not evidence either way.
pub async fn inspect(path: &Path, author: Option<&str>) -> Option<Found> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    if AUDIO_EXTENSIONS.contains(&ext.as_str()) {
        let probe = crate::audio::probe::probe_audio(path).await.ok()?;
        return Some(Found {
            title: probe.album.or(probe.title),
            authors: probe.author.into_iter().collect(),
            year: probe.year.as_deref().and_then(year_of),
            mentions_author: None,
        });
    }

    let owned = path.to_path_buf();
    let meta = tokio::task::spawn_blocking(move || kob_ebook::read_metadata(&owned))
        .await
        .ok()?
        .ok()?;
    let authors: Vec<String> = meta
        .authors
        .into_iter()
        .filter(|a| !a.trim().is_empty() && !a.eq_ignore_ascii_case("unknown"))
        .collect();
    let mentions_author = match (authors.is_empty(), author.and_then(surname)) {
        (true, Some(surname)) if matches!(ext.as_str(), "epub" | "kepub") => {
            let owned = path.to_path_buf();
            tokio::task::spawn_blocking(move || {
                crate::ebook::extract::extract_epub_text(&owned)
                    .ok()
                    .map(|segments| {
                        segments
                            .iter()
                            .any(|s| name_tokens(&s.text).contains(&surname))
                    })
            })
            .await
            .ok()
            .flatten()
        }
        _ => None,
    };
    Some(Found {
        title: meta.title.filter(|t| !t.trim().is_empty()),
        authors,
        year: meta.pubdate.as_deref().and_then(year_of),
        mentions_author,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn michael() -> Expected {
        Expected {
            title: "Michael".into(),
            alternates: vec![],
            others: vec!["The Satsuma Complex".into(), "The Hotel Avocado".into()],
            author: Some("Bob Mortimer".into()),
            year: Some(2025),
        }
    }

    fn found(
        title: Option<&str>,
        authors: &[&str],
        year: Option<i32>,
        mentions: Option<bool>,
    ) -> Found {
        Found {
            title: title.map(String::from),
            authors: authors.iter().map(|a| a.to_string()).collect(),
            year,
            mentions_author: mentions,
        }
    }

    #[test]
    fn judges_files_against_the_wanted_book() {
        let cases = [
            (
                found(Some("Mumien"), &["Michael Katz Krefeld"], Some(2026), None),
                false,
            ),
            (
                found(Some("Michael"), &["Bob Mortimer"], Some(2025), None),
                true,
            ),
            (
                found(Some("Michael: A Novel"), &["Mortimer, Bob"], None, None),
                true,
            ),
            (found(Some("Michael"), &[], None, Some(true)), true),
            (found(Some("Michael"), &[], None, Some(false)), false),
            (
                found(Some("The Satsuma Complex"), &["Bob Mortimer"], None, None),
                false,
            ),
            (
                found(Some("Michael"), &["Bob Mortimer"], Some(2001), None),
                false,
            ),
            (
                found(Some("Michael"), &["Bob Mortimer"], Some(2031), None),
                true,
            ),
            (found(Some("Mumien"), &[], None, None), false),
            (found(Some("Michael"), &[], None, None), true),
            (found(None, &[], None, None), true),
        ];
        for (file, matches) in cases {
            let verdict = judge(&michael(), &file);
            assert_eq!(
                verdict == Verdict::Matches,
                matches,
                "{file:?}: {verdict:?}"
            );
        }
    }
}
