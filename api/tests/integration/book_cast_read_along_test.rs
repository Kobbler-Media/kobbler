//! Read-along with a full cast: the page the reader sends is found in the book
//! and read by the cast's approved voices, each line with its cue. The cast is
//! the vetting rig's, with its voices approved through the vetting routes.

use crate::book_cast_render_test::approve_all;
use crate::book_cast_vetting_test::{Rig, rig, voices};
use crate::common::*;
use serde_json::{Value, json};

const PAGE: &str = "\u{201c}Hello there, my old friend,\u{201d} said Ann. \u{201c}Bye now,\u{201d} said Tom.\n\n\
    \u{201c}One more thing before we go,\u{201d} said Ann. \u{201c}And another,\u{201d} said Ann.\n\n\
    \u{201c}Halt, who goes there?\u{201d} said soldier.\n\n\
    The rain kept on falling over the grey";

async fn read(rig: &Rig, text: &str, book: bool) -> Vec<(String, String, Option<String>)> {
    let before = rig.speech_requests().await.len();
    let mut body = json!({"text": text});
    if book {
        body["book_id"] = json!(rig.book_id);
    }
    let email = format!("reader-{}@test.com", uuid::Uuid::now_v7());
    let reader = create_user_client(&rig.server, &email, "viewer").await;
    let resp = reader
        .post_json("/api/v1/admin/tts/synthesize", &body)
        .await;
    resp.assert_ok();
    assert!(resp.text().trim_end().ends_with(r#"{"type":"done"}"#));
    rig.speech_requests()
        .await
        .split_off(before)
        .into_iter()
        // The narrator the server mints for a reader with no voice of their own.
        .filter(|b| b["voice"].as_str().is_some_and(|v| !v.is_empty()))
        .map(|b: Value| {
            (
                b["input"].as_str().unwrap().to_string(),
                b["voice"].as_str().unwrap_or_default().to_string(),
                b["instructions"].as_str().map(str::to_string),
            )
        })
        .collect()
}

async fn preset(rig: &Rig, name: &str) -> String {
    let c = rig.character(name).await;
    format!("preset-{}", voices(&c)[0]["preset_id"].as_str().unwrap())
}

fn by<'a>(read: &'a [(String, String, Option<String>)], text: &str) -> (&'a str, Option<&'a str>) {
    let (_, voice, cue) = read
        .iter()
        .find(|(t, _, _)| t.contains(text))
        .unwrap_or_else(|| panic!("nobody read {text:?} in {read:?}"));
    (voice.as_str(), cue.as_deref())
}

#[tokio::test]
async fn a_page_of_a_cast_book_is_read_by_its_approved_voices() {
    let rig = rig().await;
    rig.seed().await;
    approve_all(&rig, &["Tom"]).await;
    let (ann, narrator) = (preset(&rig, "Ann").await, preset(&rig, "Narrator").await);

    let heard = read(&rig, PAGE, true).await;
    let shape: Vec<(&str, (&str, Option<&str>))> = [
        "Hello there, my old friend,",
        "said Ann.",
        "Bye now,",
        "One more thing before we go,",
        "Halt, who goes there?",
        "The rain kept on falling",
    ]
    .into_iter()
    .map(|t| (t, by(&heard, t)))
    .collect();
    assert_eq!(
        shape,
        [
            ("Hello there, my old friend,", (ann.as_str(), None)),
            ("said Ann.", (narrator.as_str(), None)),
            ("Bye now,", (narrator.as_str(), None)),
            (
                "One more thing before we go,",
                (ann.as_str(), Some("Delivery: whispering."))
            ),
            ("Halt, who goes there?", (narrator.as_str(), None)),
            ("The rain kept on falling", (narrator.as_str(), None)),
        ],
        "a line is read by its speaker with its cue, the narration by the narrator; a voice \
         not approved yet (Tom's) and a generic speaker read as the narrator"
    );
    assert!(
        heard
            .iter()
            .any(|(t, _, _)| t == "Hello there, my old friend,"),
        "a quote is read without its marks"
    );
    let words: usize = heard
        .iter()
        .map(|(t, _, _)| t.split_ascii_whitespace().count())
        .sum();
    assert_eq!(
        words,
        PAGE.split_ascii_whitespace().count(),
        "every word of the page is read once, so the highlight keeps its place"
    );
}

#[tokio::test]
async fn a_page_is_read_by_the_readers_own_voice_without_an_approved_cast() {
    let rig = rig().await;
    rig.seed().await;
    let one_voice = |heard: &[(String, String, Option<String>)]| {
        let mut voices: Vec<&str> = heard.iter().map(|(_, v, _)| v.as_str()).collect();
        voices.dedup();
        assert_eq!(voices.len(), 1, "{heard:?}");
        assert!(heard.iter().all(|(_, _, cue)| cue.is_none()), "{heard:?}");
    };

    one_voice(&read(&rig, PAGE, true).await);

    approve_all(&rig, &[]).await;
    one_voice(&read(&rig, PAGE, false).await);
    one_voice(
        &read(
            &rig,
            "\u{201c}Nothing in this book is said like this,\u{201d} said Ann.",
            true,
        )
        .await,
    );
}
