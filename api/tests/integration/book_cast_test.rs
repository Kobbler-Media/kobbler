//! Full-cast analysis end to end: koblem's LLM attributes each chapter's
//! quotes, says who is who across the book and rechecks the unknown lines,
//! then designs the voices, and the routes read the cast back.

use crate::common::*;
use api::jobs::handlers::{
    BookCastBibleHandler, BookCastChaptersHandler, BookCastVoicesHandler, EbookExtractHandler,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Semaphore;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

fn sse(content: &str) -> ResponseTemplate {
    let chunk = json!({"choices": [{"delta": {"content": content}}]});
    ResponseTemplate::new(200).set_body_raw(
        format!("data: {chunk}\n\ndata: [DONE]\n\n"),
        "text/event-stream",
    )
}

fn user_message(req: &Request) -> String {
    let body: Value = serde_json::from_slice(&req.body).unwrap();
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "user")
        .and_then(|m| m["content"].as_str())
        .unwrap()
        .to_string()
}

/// The model as the tests need it. In a chapter each `[Qn]` goes to the name
/// after the next "said", "asked" is a speaker it cannot tell, anything else is
/// not speech, and a lowercase name is a generic role. A request holding
/// `fail_on` answers 500 while `failures` lasts, as an engine falling over
/// mid-book would, and one holding `slow_on` takes a while to answer. The cast
/// and voice requests are answered by `cast` and `voices`.
struct Resident {
    fail_on: &'static str,
    failures: Arc<AtomicUsize>,
    slow_on: Option<&'static str>,
}

const SLOW: Duration = Duration::from_millis(1500);

impl Respond for Resident {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let text = user_message(req);
        let reply = self.reply(&text);
        if self.slow_on.is_some_and(|s| text.contains(s)) {
            return reply.set_delay(SLOW);
        }
        reply
    }
}

fn profile(speaker: &str) -> (&'static str, &'static str) {
    match speaker {
        "Tom" => ("male", "a child of ten"),
        "Meg" => ("female", "elderly"),
        _ => ("", ""),
    }
}

impl Resident {
    fn reply(&self, text: &str) -> ResponseTemplate {
        if text.contains(self.fail_on)
            && self
                .failures
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
        {
            return ResponseTemplate::new(500).set_body_string("engine fell over");
        }
        if text.contains("## Voice design for ") {
            return voices(text);
        }
        if text.contains("First-pass names") {
            return groups();
        }
        if text.contains("Lines to recheck") {
            return sse(&json!({"lines": [{"id": 0, "speaker": "Ann Smith"}]}).to_string());
        }
        let mut lines = Vec::new();
        let mut characters = Vec::new();
        for part in text.split("[Q").skip(1) {
            let (n, rest) = part.split_once(']').unwrap();
            let speaker = match rest.split_once("said ") {
                Some((_, after)) => after.split(|c: char| !c.is_alphanumeric()).next().unwrap(),
                None if rest.contains("asked") => "unknown",
                None => "not_speech",
            };
            lines.push(json!({"q": n.parse::<usize>().unwrap(), "speaker": speaker, "cue": ""}));
            let (gender, age) = profile(speaker);
            characters.push(json!({
                "name": speaker, "aliases": [], "gender": gender, "age": age,
                "role": format!("{speaker}'s role"), "attitude": "", "relationships": "",
                "speech": format!("{speaker} talks"),
                "generic": speaker.starts_with(char::is_lowercase),
            }));
        }
        sse(&json!({"lines": lines, "characters": characters}).to_string())
    }
}

/// Loose the way the model is at book scale: the lead is grouped twice and
/// never given the first name pass 1 used, and "unknown" turns up as a person.
fn groups() -> ResponseTemplate {
    sse(&json!({
        "people": [
            {"name": "Ann Smith", "names": ["Lizzy"], "generic": false},
            {"name": "Smith", "names": ["Ann Smith"], "generic": false},
            {"name": "Soldiers", "names": ["soldier"], "generic": true},
            {"name": "unknown", "names": ["The Knight"], "generic": false},
        ],
    })
    .to_string())
}

fn voices(text: &str) -> ResponseTemplate {
    let characters: Vec<Value> = text
        .split("## Voice design for ")
        .skip(1)
        .map(|part| {
            let name = part.lines().next().unwrap();
            // As the model did with "Talenel’Elin": a name that matches nothing asked.
            json!({"name": format!("{name} (the lead)"), "gender": "female", "age": "30s",
                   "personality": "Steady.", "accent": "Yorkshire",
                   "voice": format!("{name}'s voice")})
        })
        .collect();
    sse(&json!({"characters": characters}).to_string())
}

const NO_WAIT: &[Duration] = &[Duration::ZERO, Duration::ZERO];

struct Rig {
    server: TestServer,
    koblem: MockServer,
    book_id: Uuid,
}

const BOOK: &[(&str, &[&str])] = &[
    (
        "One",
        &[
            "“Hi,” said Ann. “Bye,” said Tom.",
            "“One,” said Ann. “Two,” said Ann.",
            "“Halt!” said soldier. “Who goes there?” someone asked.",
            "The rain kept on.",
        ],
    ),
    (
        "Two",
        &[
            "“Morning,” said Lizzy, and the sign read “CLOSED”.",
            "“Papers,” said guard.",
        ],
    ),
];

async fn rig(resident: Resident) -> Rig {
    rig_of(resident, BOOK).await
}

async fn rig_of(resident: Resident, book: &[(&str, &[&str])]) -> Rig {
    let server = TestServer::start_api_only().await;
    let koblem = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(resident)
        .mount(&koblem)
        .await;
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({"koblem_service_url": koblem.uri(), "koblem_api_key": "koblem-key"}),
        )
        .await
        .assert_ok();

    let book_id = create_book(&server, "Cast Test").await;
    upload_epub(
        &server,
        book_id,
        epub_with(
            &book
                .iter()
                .map(|(title, paragraphs)| chapter_xhtml(title, paragraphs))
                .collect::<Vec<_>>(),
        ),
    )
    .await;
    api::jobs::enqueue_ebook_extract(&server.pool, &server.nats_publisher, book_id, "Cast Test")
        .await;
    server
        .process_pending_jobs(&EbookExtractHandler::new(
            server.pool.clone(),
            server.storage_root(),
            server.nats_publisher.clone(),
            server.search_backend_arc(),
            Arc::new(Semaphore::new(1)),
        ))
        .await;
    Rig {
        server,
        koblem,
        book_id,
    }
}

impl Rig {
    async fn start(&self, body: Value) -> TestResponse {
        self.server
            .admin_client()
            .post_json(&format!("/api/v1/admin/books/{}/cast", self.book_id), &body)
            .await
    }

    async fn cast(&self) -> Value {
        let resp = self
            .server
            .admin_client()
            .get(&format!("/api/v1/admin/books/{}/cast", self.book_id))
            .await;
        resp.assert_ok();
        resp.json()
    }

    async fn chapter(&self, n: i32) -> Vec<Value> {
        let resp = self
            .server
            .admin_client()
            .get(&format!(
                "/api/v1/admin/books/{}/cast/chapters/{n}",
                self.book_id
            ))
            .await;
        resp.assert_ok();
        resp.json::<Value>()["lines"].as_array().unwrap().clone()
    }

    async fn run_chapters(&self) {
        self.server
            .process_pending_jobs(
                &BookCastChaptersHandler::new(
                    self.server.pool.clone(),
                    self.server.runtime_config.clone(),
                    self.server.nats_publisher.clone(),
                )
                .with_retry(NO_WAIT),
            )
            .await;
    }

    async fn run_bible(&self) {
        self.server
            .process_pending_jobs(
                &BookCastBibleHandler::new(
                    self.server.pool.clone(),
                    self.server.runtime_config.clone(),
                    self.server.nats_publisher.clone(),
                )
                .with_retry(NO_WAIT),
            )
            .await;
    }

    async fn run_voices(&self) {
        self.server
            .process_pending_jobs(
                &BookCastVoicesHandler::new(
                    self.server.pool.clone(),
                    self.server.runtime_config.clone(),
                    self.server.storage_root(),
                )
                .with_retry(NO_WAIT),
            )
            .await;
    }

    fn url(&self, rest: &str) -> String {
        format!("/api/v1/admin/books/{}/cast{rest}", self.book_id)
    }

    async fn speaks(&self) {
        Mock::given(method("POST"))
            .and(path("/v1/audio/speech"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(tone_wav(200), "audio/wav"))
            .mount(&self.koblem)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/audio/voices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"voices": []})))
            .mount(&self.koblem)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/voices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&self.koblem)
            .await;
        self.server
            .admin_client()
            .patch_json(
                "/api/v1/admin/config",
                &json!({"tts_enabled": true, "tts_service_url": self.koblem.uri(),
                        "tts_service_type": "breeze", "route_gpu_via_koblem": false}),
            )
            .await
            .assert_ok();
    }

    async fn character(&self, name: &str) -> Value {
        self.cast().await["characters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("no character {name}"))
    }

    async fn requests(&self, to: &str) -> Vec<Request> {
        self.koblem
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.url.path() == to)
            .collect()
    }

    async fn asked(&self, about: &str) -> String {
        self.requests("/v1/chat/completions")
            .await
            .iter()
            .map(user_message)
            .rfind(|m| m.contains(about))
            .unwrap_or_else(|| panic!("the model was never asked about {about}"))
    }
}

fn speakers(lines: &[Value]) -> Vec<(String, Option<String>)> {
    lines
        .iter()
        .map(|l| {
            (
                l["text"].as_str().unwrap().to_string(),
                l["speaker"].as_str().map(str::to_string),
            )
        })
        .collect()
}

fn never_failing() -> Resident {
    Resident {
        fail_on: "Lizzy",
        failures: Arc::default(),
        slow_on: None,
    }
}

#[tokio::test]
async fn analysis_attributes_casts_then_voices_the_book() {
    let rig = rig(never_failing()).await;
    rig.start(json!({}))
        .await
        .assert_status(StatusCode::ACCEPTED);
    rig.run_chapters().await;

    assert_eq!(
        speakers(&rig.chapter(0).await),
        [
            ("Hi,".to_string(), Some("Ann".to_string())),
            ("Bye,".to_string(), Some("Tom".to_string())),
            ("One,".to_string(), Some("Ann".to_string())),
            ("Two,".to_string(), Some("Ann".to_string())),
            ("Halt!".to_string(), Some("soldier".to_string())),
            ("Who goes there?".to_string(), None),
        ]
    );
    let cast = rig.cast().await;
    assert_eq!(cast["state"], "bible");
    assert_eq!(cast["chapters"], json!({"total": 2, "done": 2}));

    rig.run_bible().await;

    let asked = rig.asked("First-pass names").await;
    assert!(
        asked.contains("\n- Ann (3 lines; chapter 1; role: Ann's role)\n"),
        "each first-pass name is one short line: {asked}"
    );
    assert!(asked.contains("\n- soldier (1 line; chapter 1; generic; role: soldier's role)\n"));
    assert!(
        !asked.contains("Who goes there?"),
        "the lines to recheck are asked apart"
    );
    let asked = rig.asked("Lines to recheck").await;
    assert!(asked.contains("[0] chapter 1: “Who goes there?”"));
    assert!(
        asked.contains("Names: Ann Smith, Soldiers, Tom, guard\n"),
        "a line is given to someone in the cast as tidied: {asked}"
    );
    assert_eq!(rig.cast().await["state"], "voices");

    rig.start(json!({}))
        .await
        .assert_status(StatusCode::CONFLICT);
    rig.run_voices().await;
    let asked = rig.asked("## Voice design for").await;
    assert!(
        asked.contains(
            "## Voice design for Ann Smith\n\
             - chapter 1: role: Ann's role; speech: Ann talks\n\
             - chapter 2: role: Lizzy's role; speech: Lizzy talks\n"
        ),
        "a voice is designed from the chapter profiles under any of the character's names, \
         each said once: {asked}"
    );
    assert!(
        !asked.contains("for Tom") && !asked.contains("for Soldiers"),
        "pooled and generic speakers get no voice of their own"
    );
    assert!(
        asked.contains(
            "## Voice design for Narrator\nThe opening of the book:\nOne\n“Hi,” said Ann."
        ),
        "the narrator is designed from how the book reads: {asked}"
    );

    let cast = rig.cast().await;
    assert_eq!(cast["state"], "casting");
    assert_eq!(cast["error"], Value::Null);
    let mut summary: Vec<(&str, i64, bool, bool)> = cast["characters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["name"].as_str().unwrap(),
                c["line_count"].as_i64().unwrap(),
                c["pooled"].as_bool().unwrap(),
                c["generic"].as_bool().unwrap(),
            )
        })
        .collect();
    summary.sort();
    assert_eq!(
        summary,
        [
            ("Ann Smith", 5, false, false),
            ("Narrator", 0, false, false),
            ("Soldiers", 1, false, true),
            ("Tom", 1, true, false),
            ("guard", 1, false, true),
        ],
        "Ann gets the lines under her first name and her duplicate merges in; a generic \
         speaker is narrated, so never pooled; one the model left out stays generic; \
         \"unknown\" is nobody"
    );
    let characters = cast["characters"].as_array().unwrap();
    let narrator = characters.iter().find(|c| c["narrator"] == true).unwrap();
    assert_eq!(narrator["bible"]["voice"], "Narrator's voice");
    assert!(
        characters
            .iter()
            .filter(|c| c["generic"] == false)
            .all(|c| !c["voices"].as_array().unwrap().is_empty()),
        "reaching casting seeds a voice for every speaking part and the narrator: {characters:?}"
    );
    let ann = characters
        .iter()
        .find(|c| c["name"] == "Ann Smith")
        .unwrap();
    assert_eq!(ann["aliases"], json!(["Lizzy", "Smith", "Ann"]));
    assert_eq!(
        ann["bible"],
        json!({
            "gender": "female", "age": "30s", "personality": "Steady.",
            "accent": "Yorkshire", "voice": "Ann Smith's voice",
        }),
        "the voice pass writes the bible; a voice change is the admin's call, not the model's"
    );
    assert_eq!(
        ann["samples"],
        json!(["Who goes there?", "Morning,", "One,"]),
        "longest lines first"
    );
    assert_eq!(cast["unresolved"], 0);
    assert_eq!(
        speakers(&rig.chapter(0).await)[5],
        ("Who goes there?".to_string(), Some("Ann Smith".to_string())),
        "the cast pass named the speaker pass 1 could not"
    );
    assert_eq!(
        speakers(&rig.chapter(1).await),
        [
            ("Morning,".to_string(), Some("Lizzy".to_string())),
            ("Papers,".to_string(), Some("guard".to_string())),
        ],
        "the quoted sign is not speech"
    );
}

#[tokio::test]
async fn an_engine_fault_is_retried_in_place_then_resumed_after_the_last_saved_chapter() {
    let failures = Arc::new(AtomicUsize::new(NO_WAIT.len()));
    let rig = rig(Resident {
        fail_on: "Lizzy",
        failures: failures.clone(),
        slow_on: None,
    })
    .await;
    rig.start(json!({}))
        .await
        .assert_status(StatusCode::ACCEPTED);
    rig.run_chapters().await;
    let cast = rig.cast().await;
    assert_eq!(
        cast["state"], "bible",
        "a fault that clears within the retries"
    );
    assert_eq!(cast["error"], Value::Null);

    failures.store(NO_WAIT.len() + 1, Ordering::SeqCst);
    rig.start(json!({"fresh": true}))
        .await
        .assert_status(StatusCode::ACCEPTED);
    let cast = rig.cast().await;
    assert_eq!(cast["chapters"]["done"], 0, "fresh starts over");
    rig.run_chapters().await;
    let cast = rig.cast().await;
    assert_eq!(
        cast["state"], "chapters",
        "a fault that outlasts the retries"
    );
    assert_eq!(cast["chapters"]["done"], 1);
    assert!(cast["error"].as_str().unwrap().contains("500"));
    assert!(
        cast["eta"]["analysis_seconds"].as_i64().is_some(),
        "the chapter attributed so far times the rest: {}",
        cast["eta"]
    );
    let before = rig.requests("/v1/chat/completions").await.len();

    rig.start(json!({}))
        .await
        .assert_status(StatusCode::ACCEPTED);
    rig.run_chapters().await;
    assert_eq!(
        rig.requests("/v1/chat/completions").await.len(),
        before + 1,
        "only chapter two is asked again"
    );
    assert_eq!(rig.cast().await["state"], "bible");
}

#[tokio::test]
async fn cast_routes_require_an_admin_and_extracted_text() {
    let server = TestServer::start_api_only().await;
    let book_id = create_book(&server, "No Text").await;
    let cast = format!("/api/v1/admin/books/{book_id}/cast");

    server
        .api_client()
        .get(&cast)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    let user = create_user_client(&server, "reader@example.com", "viewer").await;
    user.get(&cast).await.assert_status(StatusCode::FORBIDDEN);
    user.post_json(&cast, &json!({}))
        .await
        .assert_status(StatusCode::FORBIDDEN);

    server
        .admin_client()
        .get(&cast)
        .await
        .assert_status(StatusCode::NOT_FOUND);
    server
        .admin_client()
        .post_json(&cast, &json!({}))
        .await
        .assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_rerun_of_the_analysis_keeps_what_the_admin_did() {
    let rig = rig(never_failing()).await;
    rig.speaks().await;
    rig.start(json!({}))
        .await
        .assert_status(StatusCode::ACCEPTED);
    rig.start(json!({"rerun": "chapters"}))
        .await
        .assert_status(StatusCode::CONFLICT);
    rig.run_chapters().await;
    rig.run_bible().await;
    rig.run_voices().await;
    assert_eq!(rig.cast().await["state"], "casting");
    let admin = rig.server.admin_client();

    let ann = rig.character("Ann Smith").await;
    let ann_id = ann["id"].as_str().unwrap().to_string();
    let take = admin
        .get(&rig.url(&format!("/characters/{ann_id}/auditions")))
        .await
        .json::<Value>()["lines"][0]
        .clone();
    admin
        .post_json(
            &rig.url(&format!("/characters/{ann_id}/voices/0/approval")),
            &json!({"segment_index": take["segment_index"], "span_start": take["span_start"], "seed": 3}),
        )
        .await
        .assert_ok();
    admin
        .post_json(
            &rig.url(&format!("/characters/{ann_id}/voices")),
            &json!({"from_chapter": 1, "change": "hoarse", "description": "Ann, hoarse"}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    admin
        .patch_json(
            &rig.url(&format!("/characters/{ann_id}")),
            &json!({"bible": {"personality": "Brave."}}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let narrator = rig.character("Narrator").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    admin
        .put_json(
            &rig.url(&format!("/characters/{narrator}/voices/0")),
            &json!({"description": "A dry narrator."}),
        )
        .await
        .assert_ok();
    let soldiers = rig.character("Soldiers").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let guard = rig.character("guard").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    admin
        .post_json(
            &rig.url(&format!("/characters/{soldiers}/merge")),
            &json!({"absorb": guard}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let mine = admin
        .post_json("/api/v1/admin/tts/presets", &json!({"name": "My clone"}))
        .await
        .json::<Value>()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let tom = rig.character("Tom").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    admin
        .put_json(
            &rig.url(&format!("/characters/{tom}/voices/0")),
            &json!({"preset_id": mine}),
        )
        .await
        .assert_ok();
    let halt = rig
        .chapter(0)
        .await
        .into_iter()
        .find(|l| l["text"] == "Halt!")
        .unwrap();
    admin
        .put_json(
            &rig.url(&format!(
                "/lines/{}/{}",
                halt["segment_index"], halt["span_start"]
            )),
            &json!({"character_id": ann_id}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let approved = rig.character("Ann Smith").await["voices"][0].clone();
    assert!(approved["approved_at"].is_string());

    rig.start(json!({"rerun": "chapters"}))
        .await
        .assert_status(StatusCode::ACCEPTED);
    let cast = rig.cast().await;
    assert_eq!(
        (cast["state"].clone(), cast["chapters"]["done"].clone()),
        (json!("chapters"), json!(0))
    );
    rig.run_chapters().await;
    rig.run_bible().await;
    rig.run_voices().await;
    let cast = rig.cast().await;
    assert_eq!(cast["state"], "casting", "{:?}", cast["error"]);

    let ann = rig.character("Ann Smith").await;
    assert_eq!(
        ann["id"],
        ann_id.as_str(),
        "a character found again is the same one"
    );
    assert_eq!(
        ann["voices"][0], approved,
        "the approved voice keeps its preset, clone and approval"
    );
    assert_eq!(ann["voices"][1]["from_chapter"], 1);
    assert_eq!(ann["voices"][1]["description"], "Ann, hoarse");
    assert_eq!(ann["bible"]["personality"], "Brave.", "{}", ann["bible"]);
    assert_eq!(
        ann["bible"]["voice"], "Ann Smith's voice",
        "what the admin left alone is the model's again"
    );
    assert_eq!(
        rig.character("Narrator").await["voices"][0]["description"],
        "A dry narrator."
    );
    assert_eq!(rig.character("Tom").await["voices"][0]["preset_id"], mine);
    let soldiers_now = rig.character("Soldiers").await;
    assert_eq!(soldiers_now["id"], soldiers.as_str());
    assert!(
        soldiers_now["aliases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "guard"),
        "a merge holds: {}",
        soldiers_now["aliases"]
    );
    assert!(
        cast["characters"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["name"] != "guard")
    );
    let papers = rig
        .chapter(1)
        .await
        .into_iter()
        .find(|l| l["text"] == "Papers,")
        .unwrap();
    assert_eq!(papers["character_id"], soldiers.as_str());
    let halt = rig
        .chapter(0)
        .await
        .into_iter()
        .find(|l| l["text"] == "Halt!")
        .unwrap();
    assert_eq!(
        (halt["character_id"].as_str(), halt["source"].as_str()),
        (Some(ann_id.as_str()), Some("user"))
    );
}

impl Rig {
    async fn until_asked(&self, about: &str) {
        for _ in 0..200 {
            if self
                .requests("/v1/chat/completions")
                .await
                .iter()
                .any(|r| user_message(r).contains(about))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the model was never asked about {about}");
    }

    async fn count(&self, sql: &'static str) -> i64 {
        sqlx::query_scalar(sql)
            .bind(self.book_id)
            .fetch_one(&self.server.pool)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn a_redo_mid_analysis_leaves_the_new_run_alone() {
    struct Case {
        pass: &'static str,
        slow_on: &'static str,
    }
    let cases = [
        Case {
            pass: "chapters",
            slow_on: "Lizzy",
        },
        Case {
            pass: "voices",
            slow_on: "## Voice design for ",
        },
    ];
    for case in cases {
        let rig = rig(Resident {
            slow_on: Some(case.slow_on),
            ..never_failing()
        })
        .await;
        rig.start(json!({}))
            .await
            .assert_status(StatusCode::ACCEPTED);
        if case.pass == "voices" {
            rig.run_chapters().await;
            rig.run_bible().await;
        }
        let pass = async {
            match case.pass {
                "chapters" => rig.run_chapters().await,
                _ => rig.run_voices().await,
            }
        };
        let redo = async {
            rig.until_asked(case.slow_on).await;
            rig.start(json!({"fresh": true}))
                .await
                .assert_status(StatusCode::ACCEPTED);
        };
        tokio::join!(pass, redo);

        let cast = rig.cast().await;
        assert_eq!(
            (cast["state"].clone(), cast["chapters"]["done"].clone()),
            (json!("chapters"), json!(0)),
            "{}: the run the redo replaced neither writes to the new cast nor moves it on",
            case.pass
        );
        assert_eq!(cast["characters"], json!([]), "{}", case.pass);
        assert_eq!(
            rig.count("SELECT COUNT(*) FROM user_voice_presets WHERE cast_book_id = $1")
                .await,
            0,
            "{}",
            case.pass
        );
        assert_eq!(
            rig.count(
                "SELECT COUNT(*) FROM job_status WHERE entity_id = $1 AND status = 'pending'
                 AND job_type IN ('book_cast_bible', 'book_cast_voices')"
            )
            .await,
            0,
            "{}: no later pass is queued for the new cast",
            case.pass
        );
        rig.run_chapters().await;
        assert_eq!(rig.cast().await["chapters"]["done"], 2, "{}", case.pass);
        rig.start(json!({}))
            .await
            .assert_status(StatusCode::CONFLICT);
    }
}

#[tokio::test]
async fn pooled_speakers_share_a_voice_by_the_gender_and_age_their_chapters_gave_them() {
    let rig = rig_of(
        never_failing(),
        &[(
            "One",
            &[
                "“Hi,” said Ann. “Bye,” said Tom.",
                "“One,” said Ann. “Two,” said Ann. “Three,” said Ann.",
                "“Four,” said Ann. “Hush,” said Meg.",
            ],
        )],
    )
    .await;
    rig.start(json!({}))
        .await
        .assert_status(StatusCode::ACCEPTED);
    rig.run_chapters().await;
    rig.run_bible().await;
    rig.run_voices().await;
    let cast = rig.cast().await;
    assert_eq!(cast["state"], "casting", "{:?}", cast["error"]);
    let pools: Vec<(String, String)> = ["Tom", "Meg"]
        .iter()
        .map(|name| {
            let c = cast["characters"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["name"] == *name)
                .unwrap_or_else(|| panic!("no character {name}"));
            assert_eq!(c["pooled"], true, "{name}");
            (
                name.to_string(),
                c["voices"][0]["preset_name"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        pools,
        [
            ("Tom".to_string(), "Pooled male child".to_string()),
            ("Meg".to_string(), "Pooled female older".to_string()),
        ]
    );
}

#[tokio::test]
async fn cancelling_a_pass_stops_it_waiting_out_a_retry() {
    const LONG: &[Duration] = &[Duration::from_secs(600)];
    let rig = rig(Resident {
        fail_on: "First-pass names",
        failures: Arc::new(AtomicUsize::new(usize::MAX)),
        slow_on: None,
    })
    .await;
    rig.start(json!({}))
        .await
        .assert_status(StatusCode::ACCEPTED);
    rig.run_chapters().await;
    assert_eq!(rig.cast().await["state"], "bible");

    let handler = BookCastBibleHandler::new(
        rig.server.pool.clone(),
        rig.server.runtime_config.clone(),
        rig.server.nats_publisher.clone(),
    )
    .with_retry(LONG);
    let cancel = tokio_util::sync::CancellationToken::new();
    let stop = async {
        rig.until_asked("First-pass names").await;
        cancel.cancel();
    };
    let run = tokio::time::timeout(
        Duration::from_secs(10),
        api::jobs::worker::JobHandler::execute(
            &handler,
            Uuid::now_v7(),
            json!({"book_id": rig.book_id}),
            cancel.clone(),
        ),
    );
    let (ran, ()) = tokio::join!(run, stop);
    assert!(
        matches!(ran, Ok(Err(api::jobs::JobError::Cancelled))),
        "{ran:?}"
    );
}
