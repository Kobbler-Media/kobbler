//! What the code, not the model, decides about who is who. Beefy at book scale
//! names a lead "Shallan Davar" without listing the "Shallan" pass 1 used two
//! thousand times, casts one person twice, hangs titles like "Brightness" on
//! several people, files a lead under a generic "general", and drops speakers
//! altogether; each rule here undoes one of those.

use super::chapters::{is_nobody, same_name};
use crate::repositories::book_casts::NewCharacter;

/// A name pass 1 gave a line, and whether pass 1 profiled it as a generic role.
pub struct Speaker {
    pub name: String,
    pub generic: bool,
}

pub fn tidy(characters: Vec<NewCharacter>, speakers: &[Speaker]) -> Vec<NewCharacter> {
    let mut out: Vec<NewCharacter> = Vec::new();
    for mut c in characters {
        c.name = c.name.trim().to_string();
        if is_nobody(&c.name) {
            continue;
        }
        match out
            .iter_mut()
            .find(|k| k.generic == c.generic && (claims(k, &c.name) || claims(&c, &k.name)))
        {
            Some(k) => {
                let name = c.name;
                add_aliases(k, std::iter::once(name).chain(c.aliases));
            }
            None => {
                let aliases = std::mem::take(&mut c.aliases);
                add_aliases(&mut c, aliases);
                out.push(c);
            }
        }
    }

    for c in out.iter_mut().filter(|c| c.generic) {
        c.aliases
            .retain(|a| !speakers.iter().any(|s| !s.generic && same_name(&s.name, a)));
    }

    let claimants =
        |name: &str, out: &[NewCharacter]| out.iter().filter(|c| claims(c, name)).count();
    let shared: Vec<String> = out
        .iter()
        .flat_map(|c| c.aliases.iter())
        .filter(|a| claimants(a, &out) > 1)
        .cloned()
        .collect();
    for c in &mut out {
        c.aliases
            .retain(|a| !shared.iter().any(|s| same_name(s, a)));
    }

    for speaker in speakers {
        let name = speaker.name.trim();
        if is_nobody(name) || shared.iter().any(|s| same_name(s, name)) {
            continue;
        }
        if let Some(c) = out.iter_mut().find(|c| claims(c, name)) {
            // The repository resolves lines by exact (case-blind) spelling.
            if !c.name.eq_ignore_ascii_case(name)
                && !c.aliases.iter().any(|a| a.eq_ignore_ascii_case(name))
            {
                c.aliases.push(name.to_string());
            }
            continue;
        }
        let words = |c: &&mut NewCharacter| {
            !c.generic && c.name.split_whitespace().any(|w| same_name(w, name))
        };
        let matches = out.iter_mut().filter(words).count();
        match matches {
            0 => out.push(NewCharacter {
                name: name.to_string(),
                aliases: Vec::new(),
                generic: speaker.generic,
                bible: serde_json::json!({}),
            }),
            1 => {
                if let Some(c) = out.iter_mut().find(words) {
                    c.aliases.push(name.to_string());
                }
            }
            _ => {}
        }
    }
    out
}

fn claims(c: &NewCharacter, name: &str) -> bool {
    same_name(&c.name, name) || c.aliases.iter().any(|a| same_name(a, name))
}

fn add_aliases(c: &mut NewCharacter, aliases: impl IntoIterator<Item = String>) {
    for alias in aliases {
        let alias = alias.trim();
        if !is_nobody(alias) && !claims(c, alias) {
            c.aliases.push(alias.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn generic(name: &str) -> bool {
        name.starts_with(char::is_lowercase)
    }

    fn character(name: &str, aliases: &[&str]) -> NewCharacter {
        NewCharacter {
            name: name.into(),
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
            generic: generic(name),
            bible: json!({}),
        }
    }

    #[test]
    fn tidies_names_the_model_left_loose() {
        type Cast<'a> = &'a [(&'a str, &'a [&'a str])];
        let cases: &[(&str, Cast, &[&str], Cast)] = &[
            (
                "a lead keeps the first name pass 1 used",
                &[("Shallan Davar", &["Veil"])],
                &["Shallan", "Veil"],
                &[("Shallan Davar", &["Veil", "Shallan"])],
            ),
            (
                "a person cast twice merges, whichever side names the other",
                &[
                    ("Shallan Davar", &[]),
                    ("Shallan Kholin", &["Shallan Davar"]),
                    ("Davar", &[]),
                    ("Lady Davar", &["Davar"]),
                ],
                &[],
                &[
                    ("Shallan Davar", &["Shallan Kholin"]),
                    ("Davar", &["Lady Davar"]),
                ],
            ),
            (
                "a title on several people belongs to none of them",
                &[
                    ("Jasnah Kholin", &["Brightness", "Jasnah"]),
                    ("Navani Kholin", &["Brightness"]),
                ],
                &["Brightness"],
                &[("Jasnah Kholin", &["Jasnah"]), ("Navani Kholin", &[])],
            ),
            (
                "a surname several people share is not given to one, and a speaker the \
                 model left out becomes someone",
                &[("Dalinar Kholin", &[]), ("Adolin Kholin", &[])],
                &["Kholin", "Elhokar"],
                &[
                    ("Dalinar Kholin", &[]),
                    ("Adolin Kholin", &[]),
                    ("Elhokar", &[]),
                ],
            ),
            (
                "any word of one name will do, but never for a generic role",
                &[("King Elhokar", &[]), ("guard", &[])],
                &["Elhokar", "guard captain", "King"],
                &[
                    ("King Elhokar", &["Elhokar", "King"]),
                    ("guard", &[]),
                    ("guard captain", &[]),
                ],
            ),
            (
                "the model's placeholder answers are nobody",
                &[
                    ("unknown", &["The Knight"]),
                    ("Syl", &["not_speech", "", "Syl", "Sylphrena"]),
                ],
                &["unknown"],
                &[("Syl", &["Sylphrena"])],
            ),
            (
                "a person filed under a generic role is taken back out",
                &[("general", &["Eshonai", "Resolve"])],
                &["Eshonai", "general"],
                &[("general", &["Resolve"]), ("Eshonai", &[])],
            ),
            (
                "a person and a generic role sharing a name do not merge",
                &[("Eshonai", &["General"]), ("general", &[])],
                &["Eshonai", "general"],
                &[("Eshonai", &[]), ("general", &[])],
            ),
            (
                "a name written back with another apostrophe is the same name",
                &[("Talenel\u{2019}Elin", &["Talenel' Elin"])],
                &["Talenel'Elin"],
                &[("Talenel\u{2019}Elin", &["Talenel'Elin"])],
            ),
        ];
        for (why, given, speakers, want) in cases {
            let given = given.iter().map(|(n, a)| character(n, a)).collect();
            let speakers: Vec<Speaker> = speakers
                .iter()
                .map(|s| Speaker {
                    name: s.to_string(),
                    generic: generic(s),
                })
                .collect();
            let got: Vec<(String, Vec<String>)> = tidy(given, &speakers)
                .into_iter()
                .map(|c| (c.name, c.aliases))
                .collect();
            let want: Vec<(String, Vec<String>)> = want
                .iter()
                .map(|(n, a)| (n.to_string(), a.iter().map(|s| s.to_string()).collect()))
                .collect();
            assert_eq!(got, want, "{why}");
        }
    }
}
