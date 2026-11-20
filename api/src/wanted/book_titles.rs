use sqlx::PgPool;
use uuid::Uuid;

use super::alternate_titles::{Article, titles_besides};
use crate::metadata::ProviderRegistry;
use crate::metadata::hardcover::BookTitles;

pub struct BookRef<'a> {
    pub title: &'a str,
    pub series: Option<&'a str>,
    pub position: Option<f64>,
    pub hardcover_id: Option<&'a str>,
}

/// Whether a missing Hardcover lookup may go to the API. The bulk watch sync
/// covers every wanted book at once, so it reads only what is already cached.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lookup {
    Fetch,
    CacheOnly,
}

fn position_label(position: f64) -> String {
    if position.fract() == 0.0 {
        format!("{}", position as i64)
    } else {
        position.to_string()
    }
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn with_subtitle(title: &str, subtitle: Option<&str>) -> Option<String> {
    subtitle
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("{}: {s}", title.trim()))
}

/// Every name a book may be released under. The bare series name is never
/// one of them, even when book 1 shares it ("Dune"): it names the whole series,
/// so "Dune 1" and "Dune Book 1" stand for book 1 instead.
pub fn assemble(book: &BookRef<'_>, hardcover: Option<&BookTitles>) -> Option<Vec<String>> {
    let mut names = Vec::new();
    if let Some(hc) = hardcover {
        names.push(hc.title.clone());
        names.extend(with_subtitle(&hc.title, hc.subtitle.as_deref()));
        for edition in &hc.editions {
            names.push(edition.title.clone());
            names.extend(with_subtitle(&edition.title, edition.subtitle.as_deref()));
        }
    }
    if let Some((main, _)) = book.title.split_once(':') {
        names.push(main.to_string());
    }
    if let Some(series) = book.series.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(position) = book.position {
            let n = position_label(position);
            names.push(format!("{series} {n}"));
            names.push(format!("{series} Book {n}"));
        }
        match book.title.split_once(':') {
            Some((main, rest)) if same(main, series) => names.push(rest.trim().to_string()),
            _ if !same(book.title, series) => {
                names.push(format!("{series}: {}", book.title.trim()))
            }
            _ => {}
        }
        names.retain(|n| !same(n, series) || same(book.title, series));
    }
    names.retain(|n| !n.trim().is_empty());
    titles_besides(book.title, names, Article::DropOnly)
}

pub struct BookNames {
    pub alternates: Option<Vec<String>>,
    pub others: Option<Vec<String>>,
}

/// The names a book is released under, and the titles of its author's other
/// books (Hardcover's list for the author, plus the library's) with none of
/// those names among them.
pub async fn book_names(
    registry: &ProviderRegistry,
    pool: &PgPool,
    book_id: Uuid,
    book: &BookRef<'_>,
    lookup: Lookup,
) -> BookNames {
    let hardcover = match book.hardcover_id {
        Some(id) => registry.get_book_titles(id, lookup == Lookup::Fetch).await,
        None => None,
    };
    let alternates = assemble(book, hardcover.as_ref());
    let mut others = Vec::new();
    for author_id in hardcover.iter().flat_map(|hc| &hc.author_ids) {
        others.extend(
            registry
                .get_author_titles(*author_id, lookup == Lookup::Fetch)
                .await
                .unwrap_or_default(),
        );
    }
    match crate::repositories::books::titles_by_same_authors(pool, book_id).await {
        Ok(titles) => others.extend(titles),
        Err(e) => tracing::warn!(error = %e, %book_id, "Failed to read the author's other books"),
    }
    BookNames {
        others: others_besides(book.title, &alternates, hardcover.as_ref(), others),
        alternates,
    }
}

fn others_besides(
    title: &str,
    alternates: &Option<Vec<String>>,
    hardcover: Option<&BookTitles>,
    others: Vec<String>,
) -> Option<Vec<String>> {
    let own: Vec<&str> = std::iter::once(title)
        .chain(alternates.iter().flatten().map(String::as_str))
        .chain(hardcover.map(|hc| hc.title.as_str()))
        .chain(
            hardcover
                .into_iter()
                .flat_map(|hc| hc.editions.iter().map(|e| e.title.as_str())),
        )
        .collect();
    let mut kept: Vec<String> = Vec::new();
    for other in others {
        let other = other.trim();
        // "Dune: Deluxe Edition" is an edition Hardcover lists as a book.
        let main = other.split_once(':').map_or(other, |(main, _)| main);
        if other.is_empty()
            || own.iter().any(|o| same(o, other) || same(o, main))
            || kept.iter().any(|k| same(k, other))
        {
            continue;
        }
        kept.push(other.to_string());
    }
    (!kept.is_empty()).then_some(kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::hardcover::EditionTitle;

    fn book<'a>(title: &'a str, series: Option<&'a str>, position: Option<f64>) -> BookRef<'a> {
        BookRef {
            title,
            series,
            position,
            hardcover_id: None,
        }
    }

    fn has(names: &Option<Vec<String>>, wanted: &[&str], absent: &[&str]) {
        let names = names.as_deref().unwrap_or_default();
        for w in wanted {
            assert!(names.iter().any(|n| n == w), "{w} not in {names:?}");
        }
        for a in absent {
            assert!(
                !names.iter().any(|n| n.eq_ignore_ascii_case(a)),
                "{a} in {names:?}"
            );
        }
    }

    #[test]
    fn series_books_carry_series_and_position() {
        let names = assemble(&book("The Final Empire", Some("Mistborn"), Some(1.0)), None);
        has(
            &names,
            &[
                "Mistborn 1",
                "Mistborn Book 1",
                "Mistborn: The Final Empire",
                "Final Empire",
            ],
            &["Mistborn", "The Final Empire"],
        );
        let names = assemble(
            &book("Edgedancer", Some("The Stormlight Archive"), Some(2.5)),
            None,
        );
        has(
            &names,
            &["The Stormlight Archive 2.5"],
            &["The Stormlight Archive"],
        );
    }

    /// Book 1 named after its series: the title is the series name, so the
    /// numbered forms stand in for it and later books never borrow it.
    #[test]
    fn book_one_named_after_its_series() {
        let first = assemble(&book("Dune", Some("Dune"), Some(1.0)), None);
        has(&first, &["Dune 1", "Dune Book 1"], &["Dune: Dune"]);
        let second = assemble(&book("Dune Messiah", Some("Dune"), Some(2.0)), None);
        has(
            &second,
            &["Dune 2", "Dune Book 2", "Dune: Dune Messiah"],
            &["Dune"],
        );
        let prefixed = assemble(
            &book("Mistborn: The Final Empire", Some("Mistborn"), Some(1.0)),
            None,
        );
        has(
            &prefixed,
            &["The Final Empire", "Mistborn 1"],
            &["Mistborn"],
        );
    }

    #[test]
    fn hardcover_edition_titles_and_subtitles_are_included() {
        let hc = BookTitles {
            title: "Babel".into(),
            subtitle: Some("An Arcane History".into()),
            editions: vec![
                EditionTitle {
                    title: "Babel, or the Necessity of Violence".into(),
                    subtitle: None,
                },
                EditionTitle {
                    title: "Babel".into(),
                    subtitle: Some("Or the Necessity of Violence: An Arcane History".into()),
                },
            ],
            author_ids: Vec::new(),
        };
        let names = assemble(&book("Babel", None, None), Some(&hc));
        has(
            &names,
            &[
                "Babel: An Arcane History",
                "Babel, or the Necessity of Violence",
                "Babel: Or the Necessity of Violence: An Arcane History",
            ],
            &["Babel"],
        );
    }

    #[test]
    fn a_subtitled_title_is_also_known_by_its_main_part() {
        has(
            &assemble(
                &book("Sapiens: A Brief History of Humankind", None, None),
                None,
            ),
            &["Sapiens"],
            &[],
        );
        assert_eq!(assemble(&book("Sapiens", None, None), None), None);
    }
}
