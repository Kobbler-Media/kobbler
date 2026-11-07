//! Who a rerun of the cast pass finds again. A character the analysis names
//! anew is the one already cast under that name or one of its aliases, so it
//! keeps that character's identity and with it everything vetted on it. The
//! cast as it stands decides names: a merge holds because the absorbed names
//! are the survivor's aliases, and a split holds because a name another known
//! character owns is never handed to someone else.

use std::iter::once;

use uuid::Uuid;

use super::chapters::same_name;
use crate::repositories::book_casts::NewCharacter;

pub struct Known {
    pub id: Uuid,
    pub name: String,
    pub aliases: Vec<String>,
    pub generic: bool,
    pub narrator: bool,
}

pub struct Placed {
    pub id: Option<Uuid>,
    pub character: NewCharacter,
}

fn owns(k: &Known, name: &str) -> bool {
    once(&k.name).chain(&k.aliases).any(|n| same_name(n, name))
}

fn owner(known: &[Known], c: &NewCharacter) -> Option<usize> {
    known
        .iter()
        .position(|k| same_name(&k.name, &c.name))
        .or_else(|| {
            known
                .iter()
                .position(|k| k.aliases.iter().any(|a| same_name(a, &c.name)))
        })
        .or_else(|| {
            known
                .iter()
                .position(|k| c.aliases.iter().any(|a| owns(k, a)))
        })
}

/// A known character no new one claims is gone, unless it is the narrator or
/// a name it owns was claimed by someone else's character.
pub fn place(known: &[Known], fresh: Vec<NewCharacter>) -> Vec<Placed> {
    let mut found: Vec<Option<NewCharacter>> = known.iter().map(|_| None).collect();
    let mut claimed = vec![false; known.len()];
    let mut added = Vec::new();
    for c in fresh {
        let Some(i) = owner(known, &c) else {
            added.push(Placed {
                id: None,
                character: c,
            });
            continue;
        };
        let k = &known[i];
        let entry = found[i].get_or_insert_with(|| NewCharacter {
            name: k.name.clone(),
            aliases: k.aliases.clone(),
            generic: c.generic,
            bible: c.bible.clone(),
        });
        for name in once(c.name).chain(c.aliases) {
            if let Some(j) = (0..known.len()).find(|&j| j != i && owns(&known[j], &name)) {
                claimed[j] = true;
                continue;
            }
            // Lines resolve by exact (case-blind) spelling, so a variant
            // same_name would match is still worth keeping as an alias.
            let spelled = |n: &String| n.eq_ignore_ascii_case(&name);
            if !spelled(&entry.name) && !entry.aliases.iter().any(spelled) {
                entry.aliases.push(name);
            }
        }
    }
    let mut out: Vec<Placed> = known
        .iter()
        .zip(found)
        .zip(claimed)
        .filter_map(|((k, found), claimed)| {
            let character = found.or_else(|| {
                (k.narrator || claimed).then(|| NewCharacter {
                    name: k.name.clone(),
                    aliases: k.aliases.clone(),
                    generic: k.generic,
                    bible: serde_json::json!({}),
                })
            })?;
            Some(Placed {
                id: Some(k.id),
                character,
            })
        })
        .collect();
    out.extend(added);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn known(name: &str, aliases: &[&str]) -> Known {
        Known {
            id: Uuid::now_v7(),
            name: name.into(),
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
            generic: false,
            narrator: name == "Narrator",
        }
    }

    fn fresh(name: &str, aliases: &[&str]) -> NewCharacter {
        NewCharacter {
            name: name.into(),
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
            generic: false,
            bible: json!({}),
        }
    }

    #[test]
    fn a_rerun_finds_the_cast_again_by_name() {
        type Cast<'a> = &'a [(&'a str, &'a [&'a str])];
        type Want<'a> = &'a [(Option<&'a str>, &'a str, &'a [&'a str])];
        let cases: &[(&str, Cast, Cast, Want)] = &[
            (
                "a character named again is the same one, taking any new alias",
                &[("Ann", &[])],
                &[("ann", &["Lizzy"])],
                &[(Some("Ann"), "Ann", &["Lizzy"])],
            ),
            (
                "a merge holds when the analysis casts the two apart again",
                &[("Ann", &["Lizzy"])],
                &[("Ann", &[]), ("Lizzy", &["Elizabeth"])],
                &[(Some("Ann"), "Ann", &["Lizzy", "Elizabeth"])],
            ),
            (
                "a split holds when the analysis merges the two again",
                &[("Ann", &[]), ("Elizabeth", &["Lizzy"])],
                &[("Ann", &["Lizzy"])],
                &[
                    (Some("Ann"), "Ann", &[]),
                    (Some("Elizabeth"), "Elizabeth", &["Lizzy"]),
                ],
            ),
            (
                "a character the analysis no longer finds is gone; the narrator never is",
                &[("Bob", &[]), ("Narrator", &[])],
                &[("Zed", &[])],
                &[(Some("Narrator"), "Narrator", &[]), (None, "Zed", &[])],
            ),
        ];
        for (why, before, after, want) in cases {
            let before: Vec<Known> = before.iter().map(|(n, a)| known(n, a)).collect();
            let after = after.iter().map(|(n, a)| fresh(n, a)).collect();
            let got: Vec<(Option<String>, String, Vec<String>)> = place(&before, after)
                .into_iter()
                .map(|p| {
                    let was =
                        p.id.map(|id| before.iter().find(|k| k.id == id).unwrap().name.clone());
                    (was, p.character.name, p.character.aliases)
                })
                .collect();
            let want: Vec<(Option<String>, String, Vec<String>)> = want
                .iter()
                .map(|(was, n, a)| {
                    (
                        was.map(str::to_string),
                        n.to_string(),
                        a.iter().map(|s| s.to_string()).collect(),
                    )
                })
                .collect();
            assert_eq!(got, want, "{why}");
        }
    }
}
