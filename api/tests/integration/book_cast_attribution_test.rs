//! Vetting who says what: each name a character goes by with the lines pass 1
//! gave it, a name moved to someone else, the lines nobody speaks yet, and
//! quotes that are not speech at all.

use crate::book_cast_vetting_test::{Rig, rig, voices};
use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

impl Rig {
    fn line_url(&self, text: &str) -> String {
        let (segment, span) = self.lines[text];
        self.url(&format!("/lines/{segment}/{span}"))
    }

    async fn move_name(&self, from: &str, body: Value) -> TestResponse {
        self.admin()
            .post_json(&self.url(&format!("/characters/{from}/names/move")), &body)
            .await
    }

    async fn unattributed(&self) -> Vec<Value> {
        let resp = self.admin().get(&self.url("/lines/unattributed")).await;
        resp.assert_ok();
        resp.json::<Value>()["lines"].as_array().unwrap().clone()
    }
}

fn names(c: &Value) -> Vec<(String, i64)> {
    c["names"]
        .as_array()
        .unwrap_or_else(|| panic!("no names on {c}"))
        .iter()
        .map(|n| {
            (
                n["name"].as_str().unwrap().to_string(),
                n["lines"].as_i64().unwrap(),
            )
        })
        .collect()
}

fn named(pairs: &[(&str, i64)]) -> Vec<(String, i64)> {
    pairs.iter().map(|(n, c)| (n.to_string(), *c)).collect()
}

#[tokio::test]
async fn a_name_shows_its_lines_and_moves_with_them_to_another_character() {
    let rig = rig().await;
    rig.seed().await;
    let (ann, tom, lizzy, narrator) = (
        rig.id("Ann").await,
        rig.id("Tom").await,
        rig.id("Lizzy").await,
        rig.id("Narrator").await,
    );
    let admin = rig.admin();
    admin
        .post_json(
            &rig.url(&format!("/characters/{ann}/merge")),
            &json!({"absorb": lizzy}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    admin
        .patch_json(
            &rig.url(&format!("/characters/{ann}")),
            &json!({"aliases": ["Lizzy", "Brightness"]}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    admin
        .put_json(&rig.line_url("Bye now,"), &json!({"character_id": ann}))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    assert_eq!(
        names(&rig.character("Ann").await),
        named(&[("Ann", 6), ("Lizzy", 1), ("Brightness", 0)]),
        "the canonical name first, then each alias, with the character's lines under it"
    );
    assert_eq!(names(&rig.character("Tom").await), named(&[("Tom", 0)]));

    let resp = rig.move_name(&ann, json!({"name": "lizzy"})).await;
    resp.assert_status(StatusCode::CREATED);
    let moved_to = resp.json::<Value>()["id"].clone();
    let new = rig.character("Lizzy").await;
    assert_eq!(new["id"], moved_to);
    assert_eq!(names(&new), named(&[("Lizzy", 1)]));
    assert_eq!(new["line_count"], 1);
    assert_eq!(new["pooled"], true);
    assert!(
        !voices(&new).is_empty(),
        "the new character is given a voice"
    );
    let ann_now = rig.character("Ann").await;
    assert_eq!(ann_now["aliases"], json!(["Brightness"]));
    assert_eq!(ann_now["line_count"], 6);
    let morning = rig.line("Good morning to you all,").await;
    assert_eq!(morning["character_id"], moved_to);
    assert_eq!(morning["speaker"], "Lizzy");

    let resp = rig
        .move_name(&ann, json!({"name": "Brightness", "character_id": tom}))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["id"], tom);
    assert_eq!(rig.character("Tom").await["aliases"], json!(["Brightness"]));
    assert_eq!(rig.character("Ann").await["aliases"], json!([]));

    for (from, body, status, why) in [
        (
            &ann,
            json!({"name": "Nobody"}),
            StatusCode::BAD_REQUEST,
            "not one of its names",
        ),
        (
            &ann,
            json!({"name": "ann"}),
            StatusCode::BAD_REQUEST,
            "the canonical name: rename first",
        ),
        (
            &tom,
            json!({"name": "Brightness", "character_id": tom}),
            StatusCode::BAD_REQUEST,
            "to itself",
        ),
        (
            &tom,
            json!({"name": "Brightness", "character_id": narrator}),
            StatusCode::BAD_REQUEST,
            "to the narrator",
        ),
        (
            &narrator,
            json!({"name": "Narrator"}),
            StatusCode::BAD_REQUEST,
            "from the narrator",
        ),
        (
            &tom,
            json!({"name": "Brightness", "new_name": "Ann"}),
            StatusCode::CONFLICT,
            "a new character by a name already cast",
        ),
        (
            &tom,
            json!({"name": "Brightness", "character_id": Uuid::now_v7()}),
            StatusCode::NOT_FOUND,
            "to nobody",
        ),
    ] {
        assert_eq!(rig.move_name(from, body).await.status, status, "{why}");
    }

    admin
        .patch_json(
            &rig.url(&format!("/characters/{ann}")),
            &json!({"name": "Annie"}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let resp = rig
        .move_name(&ann, json!({"name": "Ann", "new_name": " Other Ann "}))
        .await;
    resp.assert_status(StatusCode::CREATED);
    let other = rig.character("Other Ann").await;
    assert_eq!(other["aliases"], json!(["Ann"]));
    assert_eq!(other["line_count"], 5);
    assert_eq!(
        rig.character("Annie").await["line_count"],
        1,
        "a line the admin gave Ann by hand stays where they put it"
    );
    assert_eq!(rig.line("Bye now,").await["character_id"], ann);

    admin
        .patch_json(
            &rig.url(&format!("/characters/{}", other["id"].as_str().unwrap())),
            &json!({"name": "Ann"}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert_eq!(
        names(&rig.character("Ann").await),
        named(&[("Ann", 5), ("Other Ann", 0)]),
        "a character can still be renamed to one of its names"
    );
}

#[tokio::test]
async fn lines_nobody_speaks_are_listed_until_given_a_speaker_or_narration() {
    let rig = rig().await;
    rig.seed().await;
    let tom = rig.id("Tom").await;
    let admin = rig.admin();
    let (segment, span) = rig.lines["I am Sadeas, and I came for you,"];

    let unattributed = rig.unattributed().await;
    assert_eq!(unattributed.len(), 1, "{unattributed:?}");
    let sadeas = &unattributed[0];
    assert_eq!(
        (
            &sadeas["chapter_index"],
            &sadeas["chapter_title"],
            &sadeas["segment_index"],
            &sadeas["span_start"],
            &sadeas["speaker"],
            &sadeas["cue"],
        ),
        (
            &json!(1),
            &json!("Two"),
            &json!(segment),
            &json!(span),
            &json!("Sadeas"),
            &Value::Null
        )
    );
    assert_eq!(sadeas["text"], "I am Sadeas, and I came for you,");
    assert_eq!(
        sadeas["context"],
        json!({
            "before": "“Good morning to you all,” said Lizzy.",
            "pre": "",
            "quote": "“I am Sadeas, and I came for you,”",
            "post": " said Sadeas.",
            "after": "The road wound north through the hills and into the dark.",
        })
    );
    let cast = rig.cast().await;
    assert_eq!(
        (&cast["lines"], &cast["unresolved"]),
        (&json!(9), &json!(1))
    );

    let halt = rig.line_url("Halt, who goes there?");
    admin
        .put_json(&halt, &json!({"not_speech": true}))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let line = rig.line("Halt, who goes there?").await;
    assert_eq!(
        (
            &line["not_speech"],
            &line["character_id"],
            &line["speaker"],
            &line["source"]
        ),
        (&json!(true), &Value::Null, &Value::Null, &json!("user"))
    );
    assert_eq!(rig.character("Soldiers").await["line_count"], 0);
    let cast = rig.cast().await;
    assert_eq!(
        (&cast["lines"], &cast["unresolved"]),
        (&json!(8), &json!(1)),
        "a quote nobody says is neither a line nor waiting for a speaker"
    );
    assert_eq!(rig.unattributed().await.len(), 1);
    admin
        .put_json(&halt, &json!({"not_speech": true, "character_id": tom}))
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    admin
        .put_json(&halt, &json!({"character_id": tom}))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let line = rig.line("Halt, who goes there?").await;
    assert_eq!(
        (&line["not_speech"], &line["character_id"]),
        (&json!(false), &json!(tom))
    );
    assert_eq!(rig.character("Tom").await["line_count"], 2);

    admin
        .put_json(
            &rig.line_url("I am Sadeas, and I came for you,"),
            &json!({"character_id": null}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert_eq!(rig.unattributed().await, Vec::<Value>::new());
    assert_eq!(
        rig.cast().await["unresolved"],
        0,
        "made narration by hand, a line has been decided"
    );

    let url = rig.url("/lines/unattributed");
    rig.server
        .api_client()
        .get(&url)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    create_user_client(&rig.server, "viewer@example.com", "viewer")
        .await
        .get(&url)
        .await
        .assert_status(StatusCode::FORBIDDEN);
    admin
        .get(&url.replace(&rig.book_id.to_string(), &Uuid::now_v7().to_string()))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_rerun_that_finds_a_line_is_not_speech_takes_it_from_its_speaker() {
    use api::repositories::book_casts::{self, CastLine};
    let rig = rig().await;
    let pool = &rig.server.pool;
    let (segment, span) = rig.lines["One more thing before we go,"];
    let chapter = book_casts::chapters(pool, rig.book_id)
        .await
        .unwrap()
        .remove(0);
    let rerun: Vec<CastLine> = book_casts::lines(pool, rig.book_id, Some(0))
        .await
        .unwrap()
        .into_iter()
        .map(|l| {
            if (l.segment_index, l.span_start) == (segment, span) {
                CastLine {
                    speaker: None,
                    cue: None,
                    not_speech: true,
                    ..l
                }
            } else {
                l
            }
        })
        .collect();
    let run = book_casts::Run::of(&book_casts::get(pool, rig.book_id).await.unwrap().unwrap());
    book_casts::save_chapter(pool, &run, &chapter, "test", &rerun)
        .await
        .unwrap();
    let line = rig.line("One more thing before we go,").await;
    assert_eq!(
        (&line["not_speech"], &line["character_id"], &line["speaker"]),
        (&json!(true), &Value::Null, &Value::Null)
    );
}

#[tokio::test]
async fn a_chapter_line_reads_in_its_paragraph_between_its_neighbours_in_the_chapter() {
    let rig = rig().await;
    let cases = [
        (
            "Bye now,",
            json!({
                "before": "One",
                "pre": "“Hello there, my old friend,” said Ann. ",
                "quote": "“Bye now,”",
                "post": " said Tom.",
                "after": "“One more thing before we go,” said Ann. “And another,” said Ann.",
            }),
            "the chapter heading leads its first paragraph",
        ),
        (
            "Good morning to you all,",
            json!({
                "before": "Two",
                "pre": "",
                "quote": "“Good morning to you all,”",
                "post": " said Lizzy.",
                "after": "“I am Sadeas, and I came for you,” said Sadeas.",
            }),
            "the last chapter's closing paragraph is not this one's context",
        ),
        (
            "Halt, who goes there?",
            json!({
                "before": "“We should leave before the rain,” said Ann. “Yes we should,” said Ann.",
                "pre": "",
                "quote": "“Halt, who goes there?”",
                "post": " said soldier.",
                "after": "The rain kept on falling over the grey town all night long.",
            }),
            "narration around a line is context too",
        ),
    ];
    for (text, want, why) in cases {
        assert_eq!(rig.line(text).await["context"], want, "{why}");
    }
}
