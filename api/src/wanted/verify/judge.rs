use kob_normalize::titles_similar;

use super::Expected;
use crate::streaming::language::{display_name, same_language};

#[derive(Debug, Default)]
pub struct Found {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub year: Option<i32>,
    pub language: Option<String>,
    /// Whether a wanted author's surname appears in the opening of the text or
    /// narration. Only read when the metadata names nobody.
    pub mentions_author: Option<bool>,
}

#[derive(Debug, PartialEq)]
pub enum Verdict {
    Matches,
    WrongBook(String),
}

pub(super) fn name_tokens(name: &str) -> Vec<String> {
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

/// A name's last word, which survives "Bob Mortimer", "B. Mortimer" and
/// "Mortimer, Bob" alike (the last as "mortimer" via the comma).
pub(super) fn surname(author: &str) -> Option<String> {
    name_tokens(author.split(',').next().unwrap_or(author))
        .pop()
        .filter(|s| s.len() >= 3)
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
    if !expected.authors.is_empty() {
        let wanted = expected.authors.join(", ");
        if !found.authors.is_empty() {
            let any_match = found
                .authors
                .iter()
                .any(|f| expected.authors.iter().any(|w| same_author(w, f)));
            if !any_match {
                return Verdict::WrongBook(format!(
                    "the file is by {}, not {wanted}",
                    found.authors.join(", ")
                ));
            }
            author_confirmed = true;
        } else if found.mentions_author == Some(false) {
            return Verdict::WrongBook(format!("{wanted} appears nowhere in the file's opening"));
        } else {
            author_confirmed = found.mentions_author == Some(true);
        }
    }

    if let (Some(found), Some(wanted)) = (found.language.as_deref(), expected.language.as_deref())
        && let Some(name) = display_name(Some(found))
        && !same_language(found, wanted)
    {
        return Verdict::WrongBook(format!("the file is in {name}, not {wanted}"));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn michael() -> Expected {
        Expected {
            title: "Michael".into(),
            alternates: vec![],
            others: vec!["The Satsuma Complex".into(), "The Hotel Avocado".into()],
            authors: vec!["Bob Mortimer".into()],
            year: Some(2025),
            language: Some("english".into()),
        }
    }

    fn found(title: Option<&str>, authors: &[&str], year: Option<i32>) -> Found {
        Found {
            title: title.map(String::from),
            authors: authors.iter().map(|a| a.to_string()).collect(),
            year,
            ..Found::default()
        }
    }

    #[test]
    fn judges_files_against_the_wanted_book() {
        let mentions = |m| Found {
            mentions_author: Some(m),
            ..found(Some("Michael"), &[], None)
        };
        let language = |l: &str| Found {
            language: Some(l.into()),
            ..found(Some("Michael"), &["Bob Mortimer"], None)
        };
        let cases = [
            (
                "the Danish novel",
                found(Some("Mumien"), &["Michael Katz Krefeld"], Some(2026)),
                false,
            ),
            (
                "the book",
                found(Some("Michael"), &["Bob Mortimer"], Some(2025)),
                true,
            ),
            (
                "surname first",
                found(Some("Michael: A Novel"), &["Mortimer, Bob"], None),
                true,
            ),
            (
                "one of several authors",
                found(Some("Michael"), &["A Ghost", "Bob Mortimer"], None),
                true,
            ),
            ("author in the text", mentions(true), true),
            ("author nowhere", mentions(false), false),
            (
                "another of the author's books",
                found(Some("The Satsuma Complex"), &["Bob Mortimer"], None),
                false,
            ),
            (
                "dated before publication",
                found(Some("Michael"), &["Bob Mortimer"], Some(2001)),
                false,
            ),
            (
                "a later reprint",
                found(Some("Michael"), &["Bob Mortimer"], Some(2031)),
                true,
            ),
            (
                "other title, nobody named",
                found(Some("Mumien"), &[], None),
                false,
            ),
            (
                "right title, nobody named",
                found(Some("Michael"), &[], None),
                true,
            ),
            ("nothing recorded", found(None, &[], None), true),
            ("in Danish", language("da"), false),
            ("in English", language("en-GB"), true),
            ("in English, as a code", language("eng"), true),
            ("unreadable language", language("xx-nonsense"), true),
        ];
        for (name, file, matches) in cases {
            let verdict = judge(&michael(), &file);
            assert_eq!(verdict == Verdict::Matches, matches, "{name}: {verdict:?}");
        }
    }

    #[test]
    fn any_of_several_wanted_authors_will_do() {
        let expected = Expected {
            authors: vec!["Terry Pratchett".into(), "Neil Gaiman".into()],
            ..michael()
        };
        assert_eq!(
            judge(&expected, &found(Some("Michael"), &["Neil Gaiman"], None)),
            Verdict::Matches
        );
    }
}
