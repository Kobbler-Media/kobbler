//! Rendering a vetted cast: each chapter read by its approved voices, muxed
//! into a chaptered m4b and imported as the book's audiobook, then rendered
//! again chapter by chapter as the cast changes. The cast is the vetting rig's,
//! with its voices approved through the vetting routes.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::book_cast_vetting_test::{Rig, rig, voices};
use crate::common::*;
use api::jobs::handlers::{BookCastApproveHandler, BookCastRenderHandler};
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, Request, Respond, ResponseTemplate};

struct Refuse(Arc<AtomicBool>);

impl Respond for Refuse {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        if self.0.load(Ordering::SeqCst) {
            ResponseTemplate::new(500)
        } else {
            ResponseTemplate::new(200).set_body_raw(tone_wav(200), "audio/wav")
        }
    }
}

pub(crate) async fn approve_all(rig: &Rig, except: &[&str]) {
    loop {
        let cast = rig.cast().await;
        let next = cast["characters"].as_array().unwrap().iter().find_map(|c| {
            let name = c["name"].as_str().unwrap();
            if except.contains(&name) {
                return None;
            }
            voices(c)
                .into_iter()
                .find(|v| v["approved_at"].is_null())
                .map(|v| {
                    (
                        c["id"].as_str().unwrap().to_string(),
                        v["from_chapter"].as_i64().unwrap(),
                    )
                })
        });
        let Some((id, from)) = next else { break };
        let resp = rig
            .admin()
            .get(&rig.url(&format!("/characters/{id}/auditions?from_chapter={from}")))
            .await;
        resp.assert_ok();
        let take = resp.json::<Value>()["lines"][0].clone();
        rig.admin()
            .post_json(
                &rig.url(&format!("/characters/{id}/voices/{from}/approval")),
                &json!({"segment_index": take["segment_index"], "span_start": take["span_start"], "seed": 3}),
            )
            .await
            .assert_ok();
    }
}

pub(crate) fn work_dir(rig: &Rig) -> PathBuf {
    PathBuf::from(rig.server.storage_root())
        .join("book_casts")
        .join(rig.book_id.to_string())
}

pub(crate) async fn id_of(rig: &Rig, name: &str) -> String {
    rig.cast().await["characters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn rendered_since(rig: &Rig, before: usize) -> Vec<String> {
    rig.speech_requests()
        .await
        .split_off(before)
        .iter()
        .map(|b| b["input"].as_str().unwrap().to_string())
        .collect()
}

pub(crate) async fn render(rig: &Rig) {
    rig.server
        .process_pending_jobs(&BookCastRenderHandler::new(rig.server.app_state.clone()))
        .await;
}

pub(crate) async fn start(rig: &Rig) -> TestResponse {
    rig.admin().post(&rig.url("/render")).await
}

async fn preset_of(rig: &Rig, name: &str) -> String {
    let cast = rig.cast().await;
    let c = cast["characters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap()
        .clone();
    format!(
        "render-{}-{}",
        rig.book_id,
        voices(&c)[0]["preset_id"].as_str().unwrap()
    )
}

fn spoken_by<'a>(asked: &'a [Value], text: &str) -> &'a Value {
    asked
        .iter()
        .find(|b| b["input"].as_str().unwrap().contains(text))
        .unwrap_or_else(|| panic!("nobody read {text:?}"))
}

#[tokio::test]
async fn an_approved_cast_renders_to_a_chaptered_audiobook_of_the_book() {
    let rig = rig().await;
    rig.seed().await;
    approve_all(&rig, &[]).await;
    let before = rig.speech_requests().await.len();

    start(&rig).await.assert_status(StatusCode::ACCEPTED);
    assert_eq!(rig.cast().await["state"], "rendering");
    start(&rig).await.assert_status(StatusCode::CONFLICT);
    render(&rig).await;

    let asked = rig.speech_requests().await.split_off(before);
    let (ann, narrator, lizzy) = (
        preset_of(&rig, "Ann").await,
        preset_of(&rig, "Narrator").await,
        preset_of(&rig, "Lizzy").await,
    );
    let read: Vec<(&str, Option<&str>)> = [
        "One more thing before we go,",
        "Hello there, my old friend,",
        "Halt, who goes there?",
        "The rain kept on falling",
        "Good morning to you all,",
        "I am Sadeas",
    ]
    .iter()
    .map(|t| {
        let b = spoken_by(&asked, t);
        assert_eq!(b["response_format"], "wav");
        (b["voice"].as_str().unwrap(), b["instructions"].as_str())
    })
    .collect();
    assert_eq!(
        read,
        [
            (ann.as_str(), Some("Delivery: whispering.")),
            (ann.as_str(), None),
            (narrator.as_str(), None),
            (narrator.as_str(), None),
            (lizzy.as_str(), None),
            (narrator.as_str(), None),
        ],
        "each passage is read by its own voice, a quote with its cue; generic \
         speakers and quotes nobody was given are the narrator's"
    );
    assert_eq!(
        spoken_by(&asked, "Hello there, my old friend,")["input"],
        "Hello there, my old friend,",
        "a quote is read without its quote marks"
    );

    let cast = rig.cast().await;
    assert_eq!(cast["state"], "done", "{:?}", cast["error"]);
    assert_eq!(
        cast["render"]["chapters"],
        json!({"total": 2, "done": 2, "stale": 0, "missing": 0})
    );
    assert_eq!(cast["render"]["unapproved"], json!([]));
    assert_eq!(
        cast["render"]["audiobook"]["book_id"],
        rig.book_id.to_string()
    );

    let (file_id, format, file_path): (Uuid, String, String) =
        sqlx::query_as("SELECT id, format, file_path FROM audiobook_files WHERE book_id = $1")
            .bind(rig.book_id)
            .fetch_one(&rig.server.pool)
            .await
            .unwrap();
    assert_eq!(format, "M4B");
    assert_eq!(cast["render"]["audiobook"]["file_id"], file_id.to_string());
    assert!(std::path::Path::new(&file_path).exists());
    let chapters: Vec<(String, f64, f64)> = sqlx::query_as(
        "SELECT title, start_seconds, end_seconds FROM audiobook_chapters
         WHERE audiobook_file_id = $1 ORDER BY chapter_index",
    )
    .bind(file_id)
    .fetch_all(&rig.server.pool)
    .await
    .unwrap();
    let titles: Vec<&str> = chapters.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(titles, ["One", "Two"]);
    assert!(chapters[0].2 > 0.0 && (chapters[1].1 - chapters[0].2).abs() < 0.01);
    let work = work_dir(&rig);
    assert!(
        work.join("chapter-0000.aac").exists() && work.join("chapter-0001.aac").exists(),
        "each chapter's audio is kept to render only what changes"
    );
    assert!(
        !std::fs::read_dir(&work).unwrap().any(|e| e
            .unwrap()
            .path()
            .extension()
            .is_some_and(|x| x == "m4b")),
        "the muxed book goes once imported"
    );

    start(&rig).await.assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn rendering_waits_for_every_voice_to_be_approved() {
    let rig = rig().await;
    rig.seed().await;
    approve_all(&rig, &["Ann", "Tom"]).await;

    let resp = start(&rig).await;
    resp.assert_status(StatusCode::CONFLICT);
    let missing: Vec<(String, Value, Value)> = resp.json::<Value>()["unapproved"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| {
            (
                u["name"].as_str().unwrap().to_string(),
                u["from_chapter"].clone(),
                u["speakers"].clone(),
            )
        })
        .collect();
    assert_eq!(
        missing,
        [
            ("Ann".to_string(), json!(0), json!(["Ann"])),
            ("Ann".to_string(), json!(1), json!(["Ann"])),
            ("Pooled male child".to_string(), Value::Null, json!(["Tom"])),
        ],
        "an arc step is a voice of its own, and a pool is named by its preset"
    );
    let cast = rig.cast().await;
    assert_eq!(cast["state"], "casting");
    assert_eq!(cast["render"]["unapproved"].as_array().unwrap().len(), 3);
    assert_eq!(cast["render"]["chapters"]["done"], 0);
    let queued: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM job_status WHERE job_type = 'book_cast_render'")
            .fetch_one(&rig.server.pool)
            .await
            .unwrap();
    assert_eq!(queued, 0);
}

#[tokio::test]
async fn a_failed_render_resumes_at_the_first_unfinished_chapter() {
    let rig = rig().await;
    rig.seed().await;
    approve_all(&rig, &[]).await;
    let refusing = Arc::new(AtomicBool::new(true));
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .and(body_string_contains("road wound north"))
        .respond_with(Refuse(refusing.clone()))
        .with_priority(1)
        .mount(&rig.tts)
        .await;

    start(&rig).await.assert_status(StatusCode::ACCEPTED);
    render(&rig).await;
    let cast = rig.cast().await;
    assert_eq!(cast["state"], "failed");
    assert!(
        cast["error"].as_str().unwrap().contains("Two"),
        "{}",
        cast["error"]
    );
    assert_eq!(
        cast["render"]["chapters"],
        json!({"total": 2, "done": 1, "stale": 0, "missing": 1})
    );
    assert!(cast["render"]["audiobook"].is_null());
    assert!(
        cast["render"]["eta_seconds"].as_i64().unwrap() > 0,
        "the chapter rendered so far times the one left: {}",
        cast["render"]
    );

    refusing.store(false, Ordering::SeqCst);
    let before = rig.speech_requests().await.len();
    start(&rig).await.assert_status(StatusCode::ACCEPTED);
    render(&rig).await;
    let asked = rig.speech_requests().await.split_off(before);
    let inputs: Vec<&str> = asked.iter().map(|b| b["input"].as_str().unwrap()).collect();
    assert!(
        !inputs
            .iter()
            .any(|i| i.contains("rain kept on falling") || i.contains("Hello there")),
        "the finished chapter is not read again: {inputs:?}"
    );
    assert!(inputs.iter().any(|i| i.contains("road wound north")));
    let cast = rig.cast().await;
    assert_eq!(cast["state"], "done", "{:?}", cast["error"]);
    let chapters: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audiobook_chapters c JOIN audiobook_files f ON f.id = c.audiobook_file_id
         WHERE f.book_id = $1",
    )
    .bind(rig.book_id)
    .fetch_one(&rig.server.pool)
    .await
    .unwrap();
    assert_eq!(chapters, 2, "the resumed render still muxes every chapter");
}

#[tokio::test]
async fn render_requires_an_admin_and_a_cast() {
    let rig = rig().await;
    let url = rig.url("/render");
    rig.server
        .api_client()
        .post(&url)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    let viewer = create_user_client(&rig.server, "viewer@example.com", "viewer").await;
    viewer.post(&url).await.assert_status(StatusCode::FORBIDDEN);
    rig.admin()
        .post(&url.replace(&rig.book_id.to_string(), &Uuid::now_v7().to_string()))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    api::repositories::book_casts::set_state(
        &rig.server.pool,
        rig.book_id,
        api::repositories::book_casts::CastState::Bible,
        None,
    )
    .await
    .unwrap();
    rig.admin()
        .post(&url)
        .await
        .assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_change_renders_again_only_the_chapters_it_touches() {
    let rig = rig().await;
    rig.seed().await;
    approve_all(&rig, &[]).await;
    start(&rig).await.assert_status(StatusCode::ACCEPTED);
    render(&rig).await;
    let first = rig.cast().await["render"]["audiobook"]["file_id"].clone();
    assert!(first.is_string());

    let morning = rig.line("Good morning to you all,").await;
    let (segment, span) = (&morning["segment_index"], &morning["span_start"]);
    rig.admin()
        .put_json(
            &rig.url(&format!("/lines/{segment}/{span}")),
            &json!({"character_id": null}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let cast = rig.cast().await;
    assert_eq!(
        cast["render"]["chapters"],
        json!({"total": 2, "done": 1, "stale": 1, "missing": 0})
    );
    assert_eq!(
        cast["render"]["stale"],
        json!([{"chapter_index": 1, "attribution": true, "voices": []}]),
        "a line given to someone else changes its chapter's attribution"
    );
    assert_eq!(cast["state"], "done", "an edit does not undo the render");

    let before = rig.speech_requests().await.len();
    start(&rig).await.assert_status(StatusCode::ACCEPTED);
    render(&rig).await;
    let read = rendered_since(&rig, before).await;
    assert!(read.iter().any(|t| t.contains("road wound north")));
    assert!(
        !read.iter().any(|t| t.contains("rain kept on falling")),
        "the untouched chapter is not read again: {read:?}"
    );
    let cast = rig.cast().await;
    assert_eq!(cast["state"], "done", "{:?}", cast["error"]);
    assert_eq!(
        cast["render"]["chapters"],
        json!({"total": 2, "done": 2, "stale": 0, "missing": 0})
    );
    assert_eq!(
        cast["render"]["audiobook"]["file_id"], first,
        "the cast's audiobook is replaced, not added to"
    );
    let files: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audiobook_files WHERE book_id = $1")
        .bind(rig.book_id)
        .fetch_one(&rig.server.pool)
        .await
        .unwrap();
    assert_eq!(files, 1);

    let ann = id_of(&rig, "Ann").await;
    let approval = rig.url(&format!("/characters/{ann}/voices/0/approval"));
    let take = rig.line("Hello there, my old friend,").await;
    rig.admin()
        .post_json(
            &approval,
            &json!({"segment_index": take["segment_index"], "span_start": take["span_start"], "seed": 42}),
        )
        .await
        .assert_ok();
    let cast = rig.cast().await;
    assert_eq!(
        cast["render"]["stale"],
        json!([{"chapter_index": 0, "attribution": false, "voices": [{"character_id": ann, "name": "Ann"}]}]),
        "a new take of Ann's first voice touches only the chapter it reads in"
    );
    let before = rig.speech_requests().await.len();
    start(&rig).await.assert_status(StatusCode::ACCEPTED);
    render(&rig).await;
    let read = rendered_since(&rig, before).await;
    assert!(read.iter().any(|t| t.contains("rain kept on falling")));
    assert!(
        !read.iter().any(|t| t.contains("road wound north")),
        "{read:?}"
    );
    start(&rig).await.assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn approve_all_takes_the_default_take_of_every_voice_left() {
    let rig = rig().await;
    rig.seed().await;
    let ann = id_of(&rig, "Ann").await;
    let take = rig.line("Hello there, my old friend,").await;
    rig.admin()
        .post_json(
            &rig.url(&format!("/characters/{ann}/voices/0/approval")),
            &json!({"segment_index": take["segment_index"], "span_start": take["span_start"], "seed": 3}),
        )
        .await
        .assert_ok();
    let before = rig.speech_requests().await.len();

    let url = rig.url("/voices/approve-all");
    rig.admin()
        .post(&url)
        .await
        .assert_status(StatusCode::ACCEPTED);
    rig.admin()
        .post(&url)
        .await
        .assert_status(StatusCode::CONFLICT);
    assert!(rig.cast().await["approving"]["status"].is_string());
    rig.server
        .process_pending_jobs(&BookCastApproveHandler::new(rig.server.app_state.clone()))
        .await;

    let cast = rig.cast().await;
    assert_eq!(cast["render"]["unapproved"], json!([]));
    assert!(cast["approving"].is_null());
    let mints = rig.speech_requests().await.split_off(before);
    assert_eq!(
        mints.len(),
        4,
        "Ann's change, the narrator and the two pools: each pool once however many share it"
    );
    assert!(
        mints
            .iter()
            .all(|m| m["seed"] == 0 && m["response_format"] == "wav" && m.get("voice").is_none()),
        "{mints:?}"
    );
    let ann_first = rig.cast().await["characters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Ann")
        .map(|c| voices(c)[0]["approved_at"].clone())
        .unwrap();
    assert!(ann_first.is_string());
    start(&rig).await.assert_status(StatusCode::ACCEPTED);

    let viewer = create_user_client(&rig.server, "viewer@example.com", "viewer").await;
    viewer.post(&url).await.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn redo_everything_wipes_the_cast_its_voices_and_its_audio() {
    let rig = rig().await;
    rig.seed().await;
    approve_all(&rig, &[]).await;
    start(&rig).await.assert_status(StatusCode::ACCEPTED);
    render(&rig).await;
    let audiobook = rig.cast().await["render"]["audiobook"].clone();
    let presets: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM user_voice_presets WHERE cast_book_id = $1")
            .bind(rig.book_id)
            .fetch_all(&rig.server.pool)
            .await
            .unwrap();
    assert!(!presets.is_empty());
    let clips = PathBuf::from(rig.server.storage_root())
        .join("voices")
        .join(rig.admin_id.to_string());
    assert!(
        presets
            .iter()
            .any(|p| clips.join(format!("{p}.wav")).exists())
    );

    rig.admin()
        .post_json(&rig.url(""), &json!({"fresh": true}))
        .await
        .assert_status(StatusCode::ACCEPTED);

    let cast = rig.cast().await;
    assert_eq!(cast["state"], "chapters");
    assert_eq!(cast["characters"], json!([]));
    let left: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM user_voice_presets WHERE cast_book_id = $1 OR id = ANY($2)",
    )
    .bind(rig.book_id)
    .bind(&presets)
    .fetch_one(&rig.server.pool)
    .await
    .unwrap();
    assert_eq!(left, 0);
    assert!(
        presets
            .iter()
            .all(|p| !clips.join(format!("{p}.wav")).exists())
    );
    let forgotten = rig
        .tts
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "DELETE" && r.url.path().contains("preset"))
        .count();
    assert!(forgotten >= 4, "every approved clone leaves the engine");
    assert!(!work_dir(&rig).exists(), "the rendered chapters go");
    assert_eq!(
        cast["render"]["audiobook"], audiobook,
        "the imported audiobook stays in the library, still the one a new render replaces"
    );
}

#[tokio::test]
async fn analysis_cannot_be_redone_under_a_running_render() {
    let rig = rig().await;
    rig.seed().await;
    approve_all(&rig, &[]).await;
    start(&rig).await.assert_status(StatusCode::ACCEPTED);
    for body in [json!({"fresh": true}), json!({"rerun": "bible"})] {
        rig.admin()
            .post_json(&rig.url(""), &body)
            .await
            .assert_status(StatusCode::CONFLICT);
    }
}
