use isolang::Language;
use serde::Serialize;

/// Matroska and MP4 tag tracks in ISO 639-2/B, which differs from 639-2/T (and 639-3)
/// for exactly these, and isolang knows only the latter.
const BIBLIOGRAPHIC: &[(&str, &str)] = &[
    ("alb", "sqi"),
    ("arm", "hye"),
    ("baq", "eus"),
    ("bur", "mya"),
    ("chi", "zho"),
    ("cze", "ces"),
    ("dut", "nld"),
    ("fre", "fra"),
    ("geo", "kat"),
    ("ger", "deu"),
    ("gre", "ell"),
    ("ice", "isl"),
    ("mac", "mkd"),
    ("mao", "mri"),
    ("may", "msa"),
    ("per", "fas"),
    ("rum", "ron"),
    ("slo", "slk"),
    ("tib", "bod"),
    ("wel", "cym"),
];

/// Placeholders a muxer writes when it does not know; `unk` is not one in ISO 639-3 (it
/// is Enawené-Nawé) but is only ever meant as "unknown" in a track tag.
const UNTAGGED: &[&str] = &["und", "unk", "mis", "zxx", "qaa"];

/// Names people use that ISO spells differently ("Modern Greek").
const NAME_ALIASES: &[(&str, &str)] = &[("greek", "ell"), ("farsi", "fas")];

/// The English name of a track's language tag, in 639-1, 639-2/B or /T, or a locale
/// such as `pt-BR`. `None` for a missing, placeholder or unrecognised tag.
pub fn display_name(tag: Option<&str>) -> Option<&'static str> {
    from_code(tag?).map(|l| l.to_name())
}

/// Whether two tags or preferences, each a code in any form or an English name, are the
/// same language. Norwegian is one language whichever written standard a track names.
pub fn same_language(a: &str, b: &str) -> bool {
    match (parse(a), parse(b)) {
        (Some(a), Some(b)) => macrolanguage(a) == macrolanguage(b),
        _ => false,
    }
}

/// The words a track title might use for `preference`'s language: its English name and
/// its three-letter codes. Two-letter codes and the rest of ISO 639-3 are left out — as
/// words in a title they are mostly not languages ("AC3", "the").
pub fn title_words(preference: &str) -> Vec<String> {
    let Some(lang) = parse(preference) else {
        return vec![preference.trim().to_lowercase()];
    };
    let t = lang.to_639_3();
    vec![
        lang.to_name().to_lowercase(),
        t.to_string(),
        bibliographic(t).to_string(),
    ]
}

fn parse(tag: &str) -> Option<Language> {
    from_code(tag).or_else(|| from_name(tag))
}

fn from_code(tag: &str) -> Option<Language> {
    let tag = tag.trim().to_ascii_lowercase();
    let code = tag.split(['-', '_']).next()?;
    if UNTAGGED.contains(&code) {
        return None;
    }
    let code = BIBLIOGRAPHIC
        .iter()
        .find(|(b, _)| *b == code)
        .map_or(code, |(_, t)| t);
    match code.len() {
        2 => Language::from_639_1(code),
        3 => Language::from_639_3(code),
        _ => None,
    }
}

fn from_name(name: &str) -> Option<Language> {
    let name = name.trim().to_lowercase();
    if let Some((_, code)) = NAME_ALIASES.iter().find(|(n, _)| *n == name) {
        return Language::from_639_3(code);
    }
    let mut chars = name.chars();
    let capitalised: String = chars.next()?.to_uppercase().chain(chars).collect();
    Language::from_name(&capitalised)
}

fn macrolanguage(lang: Language) -> Language {
    match lang.to_639_3() {
        "nob" | "nno" => Language::Nor,
        _ => lang,
    }
}

#[derive(Debug, Serialize)]
pub struct Named<T> {
    #[serde(flatten)]
    pub track: T,
    pub language_name: Option<&'static str>,
}

pub fn named<T>(tracks: Vec<T>, language: impl Fn(&T) -> Option<&str>) -> Vec<Named<T>> {
    tracks
        .into_iter()
        .map(|track| Named {
            language_name: display_name(language(&track)),
            track,
        })
        .collect()
}

/// The 639-2/B code containers use for a 639-2/T one, or the code itself where they agree.
pub fn bibliographic(terminologic: &str) -> &str {
    BIBLIOGRAPHIC
        .iter()
        .find(|(_, t)| *t == terminologic)
        .map_or(terminologic, |(b, _)| b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_a_tag_in_any_of_its_forms() {
        for (tag, name) in [
            (Some("eng"), Some("English")),
            (Some("en"), Some("English")),
            (Some("ENG "), Some("English")),
            (Some("ger"), Some("German")),
            (Some("deu"), Some("German")),
            (Some("cze"), Some("Czech")),
            (Some("ces"), Some("Czech")),
            (Some("chi"), Some("Chinese")),
            (Some("nob"), Some("Norwegian Bokmål")),
            (Some("pt-BR"), Some("Portuguese")),
            (Some("und"), None),
            (Some("unk"), None),
            (Some("zxx"), None),
            (Some("xyz"), None),
            (Some(""), None),
            (None, None),
        ] {
            assert_eq!(display_name(tag), name, "{tag:?}");
        }
    }

    #[test]
    fn matches_a_language_however_it_is_written() {
        for (a, b, same) in [
            ("eng", "english", true),
            ("en", "English", true),
            ("ENG", "eng", true),
            ("ger", "german", true),
            ("deu", "de", true),
            ("fre", "fra", true),
            ("gre", "greek", true),
            ("cze", "czech", true),
            ("nob", "norwegian", true),
            ("nno", "no", true),
            ("pt-BR", "portuguese", true),
            ("eng", "french", false),
            ("und", "und", false),
            ("", "english", false),
        ] {
            assert_eq!(same_language(a, b), same, "{a} / {b}");
        }
    }

    #[test]
    fn a_title_names_a_language_by_its_name_or_three_letter_code() {
        assert_eq!(title_words("english"), ["english", "eng", "eng"]);
        assert_eq!(title_words("de"), ["german", "deu", "ger"]);
    }

    #[test]
    fn bibliographic_codes_round_trip() {
        assert_eq!(bibliographic("deu"), "ger");
        assert_eq!(bibliographic("eng"), "eng");
        for (b, t) in BIBLIOGRAPHIC {
            assert_eq!(bibliographic(t), *b);
            assert!(display_name(Some(b)).is_some(), "{b}");
        }
    }
}
