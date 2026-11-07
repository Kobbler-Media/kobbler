use std::collections::{HashMap, HashSet};

use serde_json::Value;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::{DEFAULT_NARRATOR_VOICE, arc_steps, clips};
use crate::repositories::Result;
use crate::repositories::book_cast_vetting::{self as repo, CastVoice, VettingCharacter};
use crate::repositories::book_casts::Run;
use crate::runtime_config::RuntimeConfigManager;

#[derive(Debug, PartialEq)]
pub struct Want {
    pub character_id: Uuid,
    pub from_chapter: i32,
    pub preset_name: String,
    pub description: String,
    pub pooled: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub create: Vec<Want>,
    pub drop: Vec<(Uuid, i32)>,
}

fn word<'a>(bible: &Value, field: &str, buckets: &[(&[&str], &'a str)], other: &'a str) -> &'a str {
    let text = bible[field].as_str().unwrap_or("").to_lowercase();
    buckets
        .iter()
        .find(|(keys, _)| keys.iter().any(|k| text.contains(k)))
        .map_or(other, |(_, w)| w)
}

/// Pooled speakers share a voice per gender and age, so a crowd of bit parts
/// still sounds like a crowd rather than one person.
pub fn pool_bucket(bible: &Value) -> (&'static str, &'static str) {
    let gender = word(
        bible,
        "gender",
        &[
            (&["female", "woman", "girl"], "female"),
            (&["male", "man", "boy"], "male"),
        ],
        "neutral",
    );
    let age = word(
        bible,
        "age",
        &[
            (
                &["child", "kid", "infant", "young boy", "young girl"],
                "child",
            ),
            (&["teen", "adolescent", "youth", "young", "20s"], "young"),
            (&["old", "elder", "aged", "60", "70", "80", "90"], "older"),
        ],
        "adult",
    );
    (gender, age)
}

fn pool_description((gender, age): (&str, &str)) -> String {
    let who = match (gender, age) {
        ("female", "child") => "A young girl",
        ("male", "child") => "A young boy",
        (_, "child") => "A child",
        ("female", "young") => "A young woman",
        ("male", "young") => "A young man",
        ("female", "older") => "An older woman",
        ("male", "older") => "An older man",
        ("female", _) => "A woman",
        ("male", _) => "A man",
        (_, "young") => "A young adult",
        (_, "older") => "An older adult",
        _ => "An adult",
    };
    format!("{who} with a natural, everyday voice, a neutral accent and a moderate pace.")
}

/// A design read with no clone behind it is all the TTS has to go on, and the
/// model's voice line often leaves out who is speaking.
fn design_description(bible: &Value) -> Option<String> {
    let voice = bible["voice"]
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty())?;
    let field = |k: &str| bible[k].as_str().map(str::trim).unwrap_or_default();
    let gender = match field("gender").to_ascii_lowercase().as_str() {
        "male" => Some("Male".to_string()),
        "female" => Some("Female".to_string()),
        _ => None,
    };
    let age = Some(field("age"))
        .filter(|a| !a.is_empty() && !a.eq_ignore_ascii_case("unknown"))
        .map(str::to_string);
    let accent = Some(field("accent"))
        .filter(|a| !a.is_empty() && !a.eq_ignore_ascii_case("neutral"))
        .map(|a| format!("{a} accent"));
    let who: Vec<String> = [gender, age, accent].into_iter().flatten().collect();
    Some(if who.is_empty() {
        voice.to_string()
    } else {
        format!("{}. {voice}", who.join(", "))
    })
}

fn own_description(c: &VettingCharacter) -> String {
    if let Some(d) = design_description(&c.bible) {
        return d;
    }
    if c.narrator {
        return DEFAULT_NARRATOR_VOICE.to_string();
    }
    pool_description(pool_bucket(&c.bible))
}

fn wanted(c: &VettingCharacter) -> Vec<Want> {
    if c.generic && !c.narrator {
        return Vec::new();
    }
    if c.pooled && !c.narrator {
        let bucket = pool_bucket(&c.bible);
        return vec![Want {
            character_id: c.id,
            from_chapter: 0,
            preset_name: format!("Pooled {} {}", bucket.0, bucket.1),
            description: pool_description(bucket),
            pooled: true,
        }];
    }
    let base = own_description(c);
    let mut out = vec![Want {
        character_id: c.id,
        from_chapter: 0,
        preset_name: c.name.clone(),
        description: base.clone(),
        pooled: false,
    }];
    out.extend(arc_steps(&c.bible).into_iter().map(|(from, voice)| Want {
        character_id: c.id,
        from_chapter: from,
        preset_name: format!("{} from chapter {}", c.name, from + 1),
        description: if voice.trim().is_empty() {
            base.clone()
        } else {
            voice
        },
        pooled: false,
    }));
    out
}

/// The voices no one has touched: unapproved, on their own design, still
/// described as the bible would describe them. A rerun of the analysis lets
/// these go to be designed afresh and keeps the rest.
pub fn untouched(characters: &[VettingCharacter], voices: &[CastVoice]) -> Vec<(Uuid, i32)> {
    voices
        .iter()
        .filter(|v| v.approved_at.is_none() && v.preset_id == v.design_preset_id)
        .filter(|v| {
            characters
                .iter()
                .find(|c| c.id == v.character_id)
                .and_then(|c| {
                    wanted(c)
                        .into_iter()
                        .find(|w| w.from_chapter == v.from_chapter)
                })
                .is_none_or(|w| v.description.as_deref() == Some(w.description.as_str()))
        })
        .map(|v| (v.character_id, v.from_chapter))
        .collect()
}

pub fn plan(characters: &[VettingCharacter], voices: &[CastVoice]) -> Plan {
    let have: HashSet<(Uuid, i32)> = voices
        .iter()
        .map(|v| (v.character_id, v.from_chapter))
        .collect();
    let mut keep = HashSet::new();
    let mut plan = Plan::default();
    for want in characters.iter().flat_map(wanted) {
        keep.insert((want.character_id, want.from_chapter));
        if !have.contains(&(want.character_id, want.from_chapter)) {
            plan.create.push(want);
        }
    }
    plan.drop = have.difference(&keep).copied().collect();
    plan.drop.sort();
    plan
}

/// Brings the cast's voices in line with its characters: a narrator, a design
/// preset for each voice a speaking part needs, and nothing for the parts that
/// no longer need one. Voices already there, picked or approved, are left alone,
/// and the clones of the presets it let go are discarded.
pub async fn seed(
    pool: &PgPool,
    runtime_config: &RuntimeConfigManager,
    storage_root: &str,
    book_id: Uuid,
    owner_id: Uuid,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let dropped = apply(&mut tx, book_id, owner_id).await?;
    tx.commit().await?;
    discard(runtime_config, storage_root, owner_id, dropped).await;
    Ok(())
}

/// [`seed`] as the voice pass's last write: false, and nothing seeded, when the
/// run no longer holds the cast.
pub async fn seed_run(
    pool: &PgPool,
    runtime_config: &RuntimeConfigManager,
    storage_root: &str,
    run: &Run,
    owner_id: Uuid,
) -> Result<bool> {
    let mut tx = pool.begin().await?;
    if !run.hold(&mut tx).await? {
        return Ok(false);
    }
    let dropped = apply(&mut tx, run.book_id, owner_id).await?;
    tx.commit().await?;
    discard(runtime_config, storage_root, owner_id, dropped).await;
    Ok(true)
}

async fn discard(
    runtime_config: &RuntimeConfigManager,
    storage_root: &str,
    owner_id: Uuid,
    dropped: Vec<Uuid>,
) {
    for preset_id in dropped {
        clips::discard(storage_root, runtime_config, owner_id, preset_id).await;
    }
}

async fn apply(tx: &mut PgConnection, book_id: Uuid, owner_id: Uuid) -> Result<Vec<Uuid>> {
    repo::ensure_narrator(&mut *tx, book_id).await?;
    let characters = repo::characters(&mut *tx, book_id).await?;
    let voices = repo::voices(&mut *tx, book_id).await?;
    let plan = plan(&characters, &voices);
    for (character_id, from_chapter) in &plan.drop {
        repo::delete_voice(&mut *tx, *character_id, *from_chapter).await?;
    }
    // Before creating: a voice let go for redesign leaves its preset holding
    // the name the new design takes.
    let dropped = repo::delete_unused_cast_presets(&mut *tx, book_id).await?;
    let mut pools: HashMap<String, Uuid> = HashMap::new();
    for want in &plan.create {
        let preset_id = match pools.get(&want.preset_name) {
            Some(id) if want.pooled => *id,
            _ => {
                let existing = if want.pooled {
                    repo::cast_preset_named(&mut *tx, book_id, &want.preset_name).await?
                } else {
                    None
                };
                let id = match existing {
                    Some(id) => id,
                    None => {
                        repo::create_cast_preset(
                            &mut *tx,
                            owner_id,
                            book_id,
                            &want.preset_name,
                            &want.description,
                        )
                        .await?
                    }
                };
                if want.pooled {
                    pools.insert(want.preset_name.clone(), id);
                }
                id
            }
        };
        repo::insert_voice(&mut *tx, want.character_id, want.from_chapter, preset_id).await?;
    }
    Ok(dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_design_says_who_is_speaking_before_how() {
        let voice = "Soft-spoken and breathy.";
        let cases = [
            (
                json!({"gender": "female", "age": "30s", "accent": "Scottish", "voice": voice}),
                "Female, 30s, Scottish accent. Soft-spoken and breathy.",
            ),
            (
                json!({"gender": "Male", "age": "unknown", "accent": "neutral", "voice": voice}),
                "Male. Soft-spoken and breathy.",
            ),
            (
                json!({"gender": "non-binary", "age": "", "voice": voice}),
                "Soft-spoken and breathy.",
            ),
            (json!({"gender": "female", "voice": " "}), ""),
        ];
        for (bible, want) in cases {
            assert_eq!(
                design_description(&bible).unwrap_or_default(),
                want,
                "{bible}"
            );
        }
    }

    #[test]
    fn pools_by_gender_and_age() {
        let cases = [
            (
                json!({"gender": "female", "age": "30s"}),
                ("female", "adult"),
            ),
            (json!({"gender": "Male", "age": "child"}), ("male", "child")),
            (
                json!({"gender": "male", "age": "elderly"}),
                ("male", "older"),
            ),
            (
                json!({"gender": "", "age": "teenager"}),
                ("neutral", "young"),
            ),
            (json!({}), ("neutral", "adult")),
        ];
        for (bible, bucket) in cases {
            assert_eq!(pool_bucket(&bible), bucket, "{bible}");
        }
    }
}
