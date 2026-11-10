//! Edits made while a cast renders: nothing is frozen, and the render reads
//! every chapter with the voices it started with, whatever the edit does to the
//! clips and the engine's clones meanwhile.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::book_cast_render_test::{approve_all, id_of, render, start};
use crate::book_cast_vetting_test::{Rig, rig};
use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use wiremock::matchers::path_regex;
use wiremock::{Mock, Request, Respond, ResponseTemplate};

/// A TTS engine that keeps its clones: a voice it does not hold cannot speak,
/// and each upload under a name is a new version of that voice.
#[derive(Clone)]
struct Engine {
    slow_on: &'static str,
    state: Arc<Mutex<EngineState>>,
}

#[derive(Default)]
struct EngineState {
    versions: HashMap<String, u32>,
    held: HashSet<String>,
    spoken: Vec<(String, u32)>,
}

fn field(body: &[u8], name: &str) -> Option<String> {
    let body = String::from_utf8_lossy(body);
    let marker = format!("name=\"{name}\"\r\n\r\n");
    let start = body.find(&marker)? + marker.len();
    let len = body[start..].find("\r\n")?;
    Some(body[start..start + len].to_string())
}

fn cloned(voice: &str) -> bool {
    voice.starts_with("preset-") || voice.starts_with("render-")
}

impl Respond for Engine {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let mut state = self.state.lock().unwrap();
        let path = req.url.path();
        match req.method.as_str() {
            "GET" => {
                let voices: Vec<Value> = state.held.iter().map(|v| json!({"id": v})).collect();
                ResponseTemplate::new(200).set_body_json(json!({"voices": voices}))
            }
            "DELETE" => {
                let name = percent_encoding::percent_decode_str(
                    path.trim_start_matches("/v1/audio/voices/"),
                )
                .decode_utf8_lossy()
                .to_string();
                state.held.remove(&name);
                ResponseTemplate::new(200)
            }
            _ if path == "/v1/audio/voices" => {
                let name = field(&req.body, "name").expect("an upload names its voice");
                *state.versions.entry(name.clone()).or_default() += 1;
                state.held.insert(name);
                ResponseTemplate::new(200).set_body_json(json!({}))
            }
            _ => {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                if let Some(voice) = body["voice"].as_str().filter(|v| cloned(v)) {
                    if !state.held.contains(voice) {
                        return ResponseTemplate::new(404).set_body_string("no such voice");
                    }
                    let version = state.versions[voice];
                    state.spoken.push((voice.to_string(), version));
                }
                let audio = ResponseTemplate::new(200).set_body_raw(tone_wav(200), "audio/wav");
                if body["input"]
                    .as_str()
                    .is_some_and(|i| i.contains(self.slow_on))
                {
                    return audio.set_delay(Duration::from_millis(1500));
                }
                audio
            }
        }
    }
}

impl Engine {
    async fn mount(rig: &Rig, slow_on: &'static str) -> Self {
        let engine = Engine {
            slow_on,
            state: Arc::default(),
        };
        Mock::given(path_regex("^/v1/audio/(voices|speech)"))
            .respond_with(engine.clone())
            .with_priority(1)
            .mount(&rig.tts)
            .await;
        engine
    }
}

#[derive(Debug, Clone, Copy)]
enum Edit {
    Unapprove,
    NewTake,
    Describe,
    DropPool,
}

async fn apply(rig: &Rig, edit: Edit) {
    let narrator = id_of(rig, "Narrator").await;
    let step = rig.url(&format!("/characters/{narrator}/voices/0"));
    match edit {
        Edit::Unapprove => {
            rig.admin()
                .delete(&format!("{step}/approval"))
                .await
                .assert_ok();
        }
        Edit::NewTake => {
            let take = rig
                .admin()
                .get(&rig.url(&format!("/characters/{narrator}/auditions")))
                .await
                .json::<Value>()["lines"][1]
                .clone();
            rig.admin()
                .post_json(
                    &format!("{step}/approval"),
                    &json!({"segment_index": take["segment_index"], "span_start": take["span_start"], "seed": 9}),
                )
                .await
                .assert_ok();
        }
        Edit::Describe => {
            rig.admin()
                .put_json(&step, &json!({"description": "A dry, quick narrator."}))
                .await
                .assert_ok();
        }
        Edit::DropPool => {
            let lizzy = id_of(rig, "Lizzy").await;
            rig.admin()
                .patch_json(
                    &rig.url(&format!("/characters/{lizzy}")),
                    &json!({"generic": true}),
                )
                .await
                .assert_status(StatusCode::NO_CONTENT);
        }
    }
}

const SLOW_LINE: &str = "The rain kept on falling";

async fn edit_mid_render(edit: Edit) -> Vec<String> {
    let rig = rig().await;
    let engine = Engine::mount(&rig, SLOW_LINE).await;
    rig.seed().await;
    approve_all(&rig, &[]).await;
    let before = rig.speech_requests().await.len();
    start(&rig).await.assert_status(StatusCode::ACCEPTED);

    let slow_line_asked = async {
        while !rig
            .speech_requests()
            .await
            .split_off(before)
            .iter()
            .any(|b| b["input"].as_str().unwrap().contains(SLOW_LINE))
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    let rendering = render(&rig);
    tokio::pin!(rendering);
    let edited = tokio::time::timeout(Duration::from_secs(120), async {
        tokio::select! {
            () = &mut rendering => false,
            () = slow_line_asked => {
                tokio::join!(&mut rendering, apply(&rig, edit));
                true
            }
        }
    })
    .await;

    let mut problems = Vec::new();
    let cast = rig.cast().await;
    match edited {
        Ok(true) => {}
        Ok(false) => problems.push(format!(
            "{edit:?}: the render ended {} before reading {SLOW_LINE:?}: {}",
            cast["state"], cast["error"]
        )),
        Err(_) => problems.push(format!(
            "{edit:?}: the render was still going after two minutes, {}",
            cast["state"]
        )),
    }
    if cast["state"] != "done" {
        problems.push(format!(
            "{edit:?}: the render ended {}: {}",
            cast["state"], cast["error"]
        ));
    }
    let state = engine.state.lock().unwrap();
    let mut first: HashMap<&str, u32> = HashMap::new();
    for (voice, version) in &state.spoken {
        if *first.entry(voice.as_str()).or_insert(*version) != *version {
            problems.push(format!(
                "{edit:?}: {voice} was read with one take and then another"
            ));
        }
    }
    if state.held.iter().any(|v| v.starts_with("render-")) {
        problems.push(format!(
            "{edit:?}: the render's own clones outlive it: {:?}",
            state.held
        ));
    }
    problems
}

#[tokio::test]
async fn an_edit_mid_render_leaves_the_render_the_voices_it_started_with() {
    let problems: Vec<String> = futures::future::join_all(
        [
            Edit::Unapprove,
            Edit::NewTake,
            Edit::Describe,
            Edit::DropPool,
        ]
        .map(edit_mid_render),
    )
    .await
    .concat();
    assert!(problems.is_empty(), "{problems:#?}");
}
