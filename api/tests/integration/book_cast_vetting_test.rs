//! Vetting a full cast: voices seeded from the designs, auditioned on each
//! character's own lines, approved as clones, and the attribution corrected by
//! hand. The analysis passes are not run; the cast is written as they leave it.

use std::collections::HashMap;
use std::sync::Arc;

use crate::common::*;
use api::jobs::handlers::EbookExtractHandler;
use api::repositories::book_casts::{self, CastChapter, CastLine, CastState};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use uuid::Uuid;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const CHAPTERS: &[(&str, &[&str])] = &[
    (
        "One",
        &[
            "“Hello there, my old friend,” said Ann. “Bye now,” said Tom.",
            "“One more thing before we go,” said Ann. “And another,” said Ann.",
            "“We should leave before the rain,” said Ann. “Yes we should,” said Ann.",
            "“Halt, who goes there?” said soldier.",
            "The rain kept on falling over the grey town all night long.",
        ],
    ),
    (
        "Two",
        &[
            "“Good morning to you all,” said Lizzy.",
            "“I am Sadeas, and I came for you,” said Sadeas.",
            "The road wound north through the hills and into the dark.",
        ],
    ),
];

const ANN_FROM_ONE_MORE: &str =
    "One more thing before we go, And another, We should leave before the rain, Yes we should,";

pub(crate) struct Rig {
    pub(crate) server: TestServer,
    pub(crate) tts: MockServer,
    pub(crate) book_id: Uuid,
    pub(crate) admin_id: Uuid,
    pub(crate) lines: HashMap<String, (i32, i32)>,
}

fn quotes(text: &str) -> Vec<(i32, i32, String, String)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(open) = text[from..].find('“').map(|i| i + from) {
        let close = text[open..].find('”').map(|i| i + open).unwrap();
        let end = close + '”'.len_utf8();
        let spoken = text[open + '“'.len_utf8()..close].to_string();
        let speaker = text[end..]
            .split("said ")
            .nth(1)
            .unwrap()
            .split(|c: char| !c.is_alphanumeric())
            .next()
            .unwrap()
            .to_string();
        out.push((open as i32, end as i32, spoken, speaker));
        from = end;
    }
    out
}

pub(crate) async fn rig() -> Rig {
    let server = TestServer::start_api_only().await;
    let tts = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(tone_wav(200), "audio/wav"))
        .mount(&tts)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/audio/voices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"voices": []})))
        .mount(&tts)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/voices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&tts)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex("^/v1/audio/voices/.+"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&tts)
        .await;
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({"tts_enabled": true, "tts_service_url": tts.uri(),
                    "tts_service_type": "breeze", "route_gpu_via_koblem": false}),
        )
        .await
        .assert_ok();

    let book_id = create_book(&server, "Vetting").await;
    upload_epub(
        &server,
        book_id,
        epub_with(
            &CHAPTERS
                .iter()
                .map(|(title, paragraphs)| chapter_xhtml(title, paragraphs))
                .collect::<Vec<_>>(),
        ),
    )
    .await;
    api::jobs::enqueue_ebook_extract(&server.pool, &server.nats_publisher, book_id, "Vetting")
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
    let admin_id = get_admin_user_id(&server).await;
    let file_id = api::repositories::ebook_segments::file_with_segments(&server.pool, book_id)
        .await
        .unwrap()
        .unwrap();
    book_casts::start(&server.pool, book_id, file_id, admin_id)
        .await
        .unwrap();

    let run = book_casts::Run::of(
        &book_casts::get(&server.pool, book_id)
            .await
            .unwrap()
            .unwrap(),
    );
    let segments = api::repositories::ebook_segments::get_for_file(&server.pool, file_id)
        .await
        .unwrap();
    let mut lines = HashMap::new();
    for (chapter_index, (title, _)) in CHAPTERS.iter().enumerate() {
        let own: Vec<_> = segments
            .iter()
            .filter(|s| s.chapter_title.as_deref() == Some(*title))
            .collect();
        let mut cast_lines = Vec::new();
        for s in &own {
            for (start, end, spoken, speaker) in quotes(&s.text) {
                lines.insert(spoken.clone(), (s.segment_index, start));
                cast_lines.push(CastLine {
                    segment_index: s.segment_index,
                    span_start: start,
                    span_end: end,
                    chapter_index: chapter_index as i32,
                    speaker: Some(speaker),
                    character_id: None,
                    cue: (spoken == "One more thing before we go,").then(|| "whispering".into()),
                    source: "gemma".into(),
                    not_speech: false,
                });
            }
        }
        let chapter = CastChapter {
            chapter_index: chapter_index as i32,
            title: Some(title.to_string()),
            first_segment: own.first().unwrap().segment_index,
            last_segment: own.last().unwrap().segment_index,
            notes: json!({}),
        };
        book_casts::save_chapter(&server.pool, &run, &chapter, "test", &cast_lines)
            .await
            .unwrap();
    }
    for (name, aliases, bible, line_count, pooled, generic) in [
        (
            "Ann",
            vec![],
            json!({"gender": "female", "age": "30s", "voice": "Ann's voice",
                   "arc": [{"from_chapter": 1, "change": "hoarse", "voice": "Ann, hoarse"}]}),
            5,
            false,
            false,
        ),
        (
            "Tom",
            vec![],
            json!({"gender": "male", "age": "child"}),
            1,
            true,
            false,
        ),
        (
            "Lizzy",
            vec![],
            json!({"gender": "female", "age": "30s"}),
            1,
            true,
            false,
        ),
        (
            "Soldiers",
            vec!["soldier".to_string()],
            json!({}),
            1,
            false,
            true,
        ),
    ] {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO book_cast_characters (id, book_id, name, aliases, bible, line_count, pooled, generic)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(id)
        .bind(book_id)
        .bind(name)
        .bind(&aliases)
        .bind(&bible)
        .bind(line_count)
        .bind(pooled)
        .bind(generic)
        .execute(&server.pool)
        .await
        .unwrap();
        let mut names = aliases;
        names.push(name.to_string());
        sqlx::query(
            "UPDATE book_cast_lines SET character_id = $2 WHERE book_id = $1 AND speaker = ANY($3)",
        )
        .bind(book_id)
        .bind(id)
        .bind(&names)
        .execute(&server.pool)
        .await
        .unwrap();
    }
    book_casts::set_state(&server.pool, book_id, CastState::Casting, None)
        .await
        .unwrap();
    Rig {
        server,
        tts,
        book_id,
        admin_id,
        lines,
    }
}

impl Rig {
    pub(crate) fn url(&self, rest: &str) -> String {
        format!("/api/v1/admin/books/{}/cast{rest}", self.book_id)
    }

    pub(crate) fn admin(&self) -> TestClient {
        self.server.admin_client()
    }

    pub(crate) async fn seed(&self) {
        api::book_cast_vetting::seed::seed(
            &self.server.pool,
            &self.server.runtime_config,
            &self.server.storage_root(),
            self.book_id,
            self.admin_id,
        )
        .await
        .unwrap();
    }

    pub(crate) async fn cast(&self) -> Value {
        let resp = self.admin().get(&self.url("")).await;
        resp.assert_ok();
        resp.json()
    }

    pub(crate) async fn character(&self, name: &str) -> Value {
        self.cast().await["characters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("no character {name}"))
    }

    pub(crate) async fn id(&self, name: &str) -> String {
        self.character(name).await["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    pub(crate) async fn chapter(&self, n: i32) -> Vec<Value> {
        let resp = self.admin().get(&self.url(&format!("/chapters/{n}"))).await;
        resp.assert_ok();
        resp.json::<Value>()["lines"].as_array().unwrap().clone()
    }

    pub(crate) async fn line(&self, text: &str) -> Value {
        let (segment_index, span_start) = self.lines[text];
        for n in 0..CHAPTERS.len() as i32 {
            if let Some(l) = self
                .chapter(n)
                .await
                .into_iter()
                .find(|l| l["segment_index"] == segment_index && l["span_start"] == span_start)
            {
                return l;
            }
        }
        panic!("no line {text}")
    }

    fn take(&self, text: &str) -> Value {
        let (segment_index, span_start) = self.lines[text];
        json!({"segment_index": segment_index, "span_start": span_start})
    }

    pub(crate) async fn speech_requests(&self) -> Vec<Value> {
        self.tts
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/v1/audio/speech")
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect()
    }

    async fn requests_to(&self, verb: &str, prefix: &str) -> Vec<Request> {
        self.tts
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.method.as_str() == verb && r.url.path().starts_with(prefix))
            .collect()
    }
}

pub(crate) fn voices(c: &Value) -> Vec<Value> {
    c["voices"].as_array().unwrap().clone()
}

fn with(mut base: Value, extra: Value) -> Value {
    for (k, v) in extra.as_object().unwrap() {
        base[k] = v.clone();
    }
    base
}

#[tokio::test]
async fn seeding_voices_every_speaking_part_and_the_narrator() {
    let rig = rig().await;
    rig.seed().await;
    let cast = rig.cast().await;

    let narrator = rig.character("Narrator").await;
    assert_eq!(narrator["narrator"], true);
    let narrator_voices = voices(&narrator);
    assert_eq!(narrator_voices.len(), 1);
    assert!(
        narrator_voices[0]["description"]
            .as_str()
            .unwrap()
            .contains("narrator"),
        "a narrator with no voice design of its own gets the default"
    );

    let ann = voices(&rig.character("Ann").await);
    let described: Vec<(i64, &str)> = ann
        .iter()
        .map(|v| {
            (
                v["from_chapter"].as_i64().unwrap(),
                v["description"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(described, [(0, "Ann's voice"), (1, "Ann, hoarse")]);
    assert!(ann.iter().all(|v| v["preset_id"] == v["design_preset_id"]
        && v["approved_at"].is_null()
        && v["has_clip"] == false));

    let tom = voices(&rig.character("Tom").await);
    let lizzy = voices(&rig.character("Lizzy").await);
    assert_eq!(tom[0]["preset_name"], "Pooled male child");
    assert_eq!(lizzy[0]["preset_name"], "Pooled female adult");
    assert!(voices(&rig.character("Soldiers").await).is_empty());

    let presets = rig.admin().get("/api/v1/admin/tts/presets").await;
    presets.assert_ok();
    assert_eq!(
        presets.json::<Vec<Value>>(),
        Vec::<Value>::new(),
        "a cast's presets stay off the owner's own voice list"
    );
    assert_eq!(cast["presets"], json!([]));
    assert_eq!(
        cast["unattached"],
        json!([{"speaker": "Sadeas", "lines": 1}])
    );

    rig.seed().await;
    assert_eq!(
        voices(&rig.character("Ann").await),
        ann,
        "seeding again changes nothing"
    );

    let soldier = rig.line("Halt, who goes there?").await;
    assert_eq!(
        soldier["voice"]["character_id"], narrator["id"],
        "a generic speaker is read by the narrator"
    );
    let hello = rig.line("Hello there, my old friend,").await;
    assert_eq!(hello["voice"]["preset_id"], ann[0]["preset_id"]);
}

#[tokio::test]
async fn a_narrator_written_by_the_analysis_keeps_its_voice_design() {
    let rig = rig().await;
    sqlx::query(
        "INSERT INTO book_cast_characters (id, book_id, name, bible, narrator) VALUES ($1, $2, 'Narrator', $3, TRUE)",
    )
    .bind(Uuid::now_v7())
    .bind(rig.book_id)
    .bind(json!({"voice": "A dry, wry baritone."}))
    .execute(&rig.server.pool)
    .await
    .unwrap();
    rig.seed().await;
    let narrators: Vec<Value> = rig.cast().await["characters"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["narrator"] == true)
        .cloned()
        .collect();
    assert_eq!(narrators.len(), 1);
    assert_eq!(
        voices(&narrators[0])[0]["description"],
        "A dry, wry baritone."
    );
}

#[tokio::test]
async fn auditions_render_own_lines_then_a_take_is_approved_as_a_clone() {
    let rig = rig().await;
    rig.seed().await;
    let ann = rig.id("Ann").await;
    let auditions = rig.url(&format!("/characters/{ann}/auditions"));

    let resp = rig.admin().get(&auditions).await;
    resp.assert_ok();
    let picked: Vec<String> = resp.json::<Value>()["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["text"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(picked.len(), 5, "{picked:?}");
    assert_eq!(
        picked[0], "We should leave before the rain,",
        "longest first"
    );

    let narrator = rig.id("Narrator").await;
    let resp = rig
        .admin()
        .get(&rig.url(&format!("/characters/{narrator}/auditions")))
        .await;
    resp.assert_ok();
    let mut narration: Vec<String> = resp.json::<Value>()["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["text"].as_str().unwrap().to_string())
        .collect();
    narration.sort();
    assert_eq!(
        narration,
        [
            "The rain kept on falling over the grey town all night long.",
            "The road wound north through the hills and into the dark.",
        ],
        "the narrator auditions on narration"
    );

    let cue_line = rig.take("One more thing before we go,");
    for with_cue in [false, true] {
        let resp = rig
            .admin()
            .post_json(
                &auditions,
                &with(cue_line.clone(), json!({"seed": 7, "with_cue": with_cue})),
            )
            .await;
        resp.assert_ok();
        assert_eq!(resp.headers["content-type"], "audio/mpeg");
    }
    let asked = rig.speech_requests().await;
    let shapes: Vec<(&str, &str, i64, bool)> = asked
        .iter()
        .map(|b| {
            (
                b["input"].as_str().unwrap(),
                b["instructions"].as_str().unwrap(),
                b["seed"].as_i64().unwrap(),
                b.get("voice").is_some(),
            )
        })
        .collect();
    assert_eq!(
        shapes,
        [
            (ANN_FROM_ONE_MORE, "Ann's voice", 7, false),
            (
                "One more thing before we go,",
                "Ann's voice Delivery: whispering.",
                7,
                false
            ),
        ],
        "a take reads on through the character's next lines, long enough to clone from; \
         with its cue it is the one line, the delivery being what is tried"
    );
    assert!(
        asked.iter().all(|b| b["cfg_scale"] == 4.0),
        "an instruction only lands with guidance: {asked:?}"
    );

    let approval = rig.url(&format!("/characters/{ann}/voices/0/approval"));
    rig.admin()
        .post_json(&approval, &json!({}))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    let resp = rig
        .admin()
        .post_json(&approval, &with(cue_line.clone(), json!({"seed": 7})))
        .await;
    resp.assert_ok();
    let approved = &resp.json::<Value>()["voices"][0];
    assert!(approved["approved_at"].is_string());
    assert_eq!(approved["has_clip"], true);
    let preset_id = approved["preset_id"].as_str().unwrap().to_string();
    let clone = format!("preset-{preset_id}");
    let forgotten = format!("/v1/audio/voices/{clone}").replace('-', "%2D");

    let mint = rig.speech_requests().await.pop().unwrap();
    assert_eq!(mint["response_format"], "wav");
    assert_eq!(mint["seed"], 7);
    assert_eq!(mint["instructions"], "Ann's voice");
    assert_eq!(
        mint["input"], ANN_FROM_ONE_MORE,
        "the clone is the take that was heard"
    );
    let uploads = rig.requests_to("POST", "/v1/audio/voices").await;
    let upload = String::from_utf8_lossy(&uploads.last().unwrap().body).to_string();
    assert!(upload.contains(&clone));
    assert!(
        upload.contains(ANN_FROM_ONE_MORE),
        "the take is the clone's ref text"
    );
    let clip = std::path::PathBuf::from(rig.server.storage_root())
        .join("voices")
        .join(rig.admin_id.to_string())
        .join(format!("{preset_id}.wav"));
    assert!(clip.exists());

    for with_cue in [false, true] {
        rig.admin()
            .post_json(
                &auditions,
                &with(cue_line.clone(), json!({"seed": 9, "with_cue": with_cue})),
            )
            .await
            .assert_ok();
    }
    let asked = rig.speech_requests().await;
    let on_clone: Vec<(&str, Option<&str>, Option<f64>)> = asked[asked.len() - 2..]
        .iter()
        .map(|b| {
            (
                b["voice"].as_str().unwrap(),
                b["instructions"].as_str(),
                b["cfg_scale"].as_f64(),
            )
        })
        .collect();
    assert_eq!(
        on_clone,
        [
            (clone.as_str(), None, None),
            (clone.as_str(), Some("Delivery: whispering."), Some(4.0))
        ],
        "an approved voice is its clone, with only the cue as an instruction"
    );

    let hello = rig.take("Hello there, my old friend,");
    let resp = rig
        .admin()
        .post_json(&approval, &with(hello, json!({"seed": 11})))
        .await;
    resp.assert_ok();
    assert!(resp.json::<Value>()["voices"][0]["approved_at"].is_string());
    let mint = rig.speech_requests().await.pop().unwrap();
    assert_eq!(
        (
            mint["input"].as_str(),
            mint["seed"].as_i64(),
            mint["voice"].as_str()
        ),
        (
            Some(
                "Hello there, my old friend, One more thing before we go, And another, \
                 We should leave before the rain, Yes we should,"
            ),
            Some(11),
            None
        ),
        "another take is minted from the description, not from the clone it replaces"
    );
    assert_eq!(rig.requests_to("DELETE", &forgotten).await.len(), 1);
    let uploads = rig.requests_to("POST", "/v1/audio/voices").await;
    assert!(
        String::from_utf8_lossy(&uploads.last().unwrap().body).contains("Hello there"),
        "the new take replaces the clone"
    );

    let resp = rig
        .admin()
        .put_json(
            &rig.url(&format!("/characters/{ann}/voices/0")),
            &json!({"description": "Something else"}),
        )
        .await;
    resp.assert_ok();
    let redescribed = &resp.json::<Value>()["voices"][0];
    assert_eq!(redescribed["description"], "Something else");
    assert!(
        redescribed["approved_at"].is_null(),
        "a new description needs approving again"
    );
    assert_eq!(redescribed["has_clip"], false);
    assert!(!clip.exists());
    assert_eq!(rig.requests_to("DELETE", &forgotten).await.len(), 2);

    rig.admin()
        .post_json(&approval, &with(cue_line.clone(), json!({"seed": 7})))
        .await
        .assert_ok();
    let resp = rig.admin().delete(&approval).await;
    resp.assert_ok();
    let unapproved = &resp.json::<Value>()["voices"][0];
    assert!(unapproved["approved_at"].is_null());
    assert_eq!(unapproved["has_clip"], false);
    assert!(!clip.exists());
    assert_eq!(rig.requests_to("DELETE", &forgotten).await.len(), 3);

    let soldiers = rig.id("Soldiers").await;
    rig.admin()
        .get(&rig.url(&format!("/characters/{soldiers}/auditions")))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    rig.admin()
        .post_json(&auditions, &json!({"segment_index": 9999, "span_start": 0}))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_voice_can_be_an_existing_preset_or_its_own_description() {
    let rig = rig().await;
    rig.seed().await;
    let ann = rig.id("Ann").await;
    let step = rig.url(&format!("/characters/{ann}/voices/0"));

    let resp = rig
        .admin()
        .put_json(&step, &json!({"description": "Bright and quick."}))
        .await;
    resp.assert_ok();
    assert_eq!(
        resp.json::<Value>()["voices"][0]["description"],
        "Bright and quick."
    );

    let mine = rig
        .admin()
        .post_json("/api/v1/admin/tts/presets", &json!({"name": "My clone"}))
        .await;
    mine.assert_success();
    let mine = mine.json::<Value>()["id"].as_str().unwrap().to_string();
    assert_eq!(rig.cast().await["presets"][0]["id"], mine);

    let resp = rig
        .admin()
        .put_json(&step, &json!({"preset_id": mine}))
        .await;
    resp.assert_ok();
    let picked = &resp.json::<Value>()["voices"][0];
    assert_eq!(picked["preset_id"], mine);
    assert_eq!(picked["preset_name"], "My clone");
    assert_ne!(picked["design_preset_id"], mine);

    let resp = rig
        .admin()
        .post_json(&format!("{step}/approval"), &json!({}))
        .await;
    resp.assert_ok();
    assert!(
        resp.json::<Value>()["voices"][0]["approved_at"].is_string(),
        "an existing preset is approved as it stands"
    );

    let resp = rig
        .admin()
        .put_json(&step, &json!({"preset_id": null}))
        .await;
    resp.assert_ok();
    let back = &resp.json::<Value>()["voices"][0];
    assert_eq!(back["preset_id"], back["design_preset_id"]);
    assert!(back["approved_at"].is_null());

    let resp = rig
        .admin()
        .post_json(
            &format!("{step}/approval"),
            &with(rig.take("Hello there, my old friend,"), json!({"seed": 5})),
        )
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["voices"][0]["has_clip"], true);
    rig.admin()
        .put_json(&step, &json!({"preset_id": mine}))
        .await
        .assert_ok();
    let resp = rig
        .admin()
        .put_json(&step, &json!({"preset_id": null}))
        .await;
    resp.assert_ok();
    let back = &resp.json::<Value>()["voices"][0];
    assert_eq!(
        (back["approved_at"].clone(), back["has_clip"].clone()),
        (Value::Null, json!(false)),
        "leaving an approved design for another preset lets its clone go"
    );

    let tom_pool = voices(&rig.character("Tom").await)[0]["preset_id"].clone();
    rig.admin()
        .put_json(&step, &json!({"preset_id": tom_pool}))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    rig.admin()
        .put_json(&step, &json!({"description": "  "}))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    rig.admin()
        .put_json(
            &rig.url(&format!("/characters/{ann}/voices/5")),
            &json!({"description": "x"}),
        )
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn corrections_move_lines_names_and_voices() {
    let rig = rig().await;
    rig.seed().await;
    let (ann, tom, lizzy) = (
        rig.id("Ann").await,
        rig.id("Tom").await,
        rig.id("Lizzy").await,
    );
    let admin = rig.admin();

    let attach = rig.url("/speakers/attach");
    let resp = admin
        .post_json(&attach, &json!({"speaker": "sadeas", "character_id": tom}))
        .await;
    resp.assert_ok();
    assert_eq!(resp.json::<Value>()["attached"], 1);
    let tom_now = rig.character("Tom").await;
    assert_eq!(tom_now["aliases"], json!(["sadeas"]));
    assert_eq!(tom_now["line_count"], 2);
    assert_eq!(rig.cast().await["unattached"], json!([]));
    let sadeas = rig.line("I am Sadeas, and I came for you,").await;
    assert_eq!(
        (sadeas["speaker"].as_str(), sadeas["source"].as_str()),
        (Some("Tom"), Some("user"))
    );
    admin
        .post_json(&attach, &json!({"speaker": "Sadeas", "character_id": tom}))
        .await
        .assert_status(StatusCode::NOT_FOUND);

    let (segment, span) = rig.lines["Bye now,"];
    let bye = rig.url(&format!("/lines/{segment}/{span}"));
    admin
        .put_json(&bye, &json!({"character_id": null}))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let line = rig.line("Bye now,").await;
    assert_eq!(line["character_id"], Value::Null);
    assert_eq!(line["speaker"], Value::Null);
    assert_eq!(line["source"], "user");
    assert_eq!(
        line["voice"]["character_id"].as_str(),
        Some(rig.id("Narrator").await.as_str())
    );

    let chapter = book_casts::chapters(&rig.server.pool, rig.book_id)
        .await
        .unwrap()
        .remove(0);
    let rerun: Vec<CastLine> = book_casts::lines(&rig.server.pool, rig.book_id, Some(0))
        .await
        .unwrap()
        .into_iter()
        .map(|l| {
            if (l.segment_index, l.span_start) == (segment, span) {
                CastLine {
                    speaker: Some("Tom".into()),
                    source: "gemma".into(),
                    ..l
                }
            } else {
                l
            }
        })
        .collect();
    let run = book_casts::Run::of(
        &book_casts::get(&rig.server.pool, rig.book_id)
            .await
            .unwrap()
            .unwrap(),
    );
    book_casts::save_chapter(&rig.server.pool, &run, &chapter, "test", &rerun)
        .await
        .unwrap();
    assert_eq!(
        rig.line("Bye now,").await["speaker"],
        Value::Null,
        "an analysis rerun never overwrites a line corrected by hand"
    );
    admin
        .put_json(&bye, &json!({"character_id": ann}))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert_eq!(rig.character("Ann").await["line_count"], 6);
    admin
        .put_json(&rig.url("/lines/9999/0"), &json!({"character_id": ann}))
        .await
        .assert_status(StatusCode::NOT_FOUND);

    admin
        .post_json(
            &rig.url(&format!("/characters/{ann}/merge")),
            &json!({"absorb": lizzy}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let ann_now = rig.character("Ann").await;
    assert_eq!(ann_now["aliases"], json!(["Lizzy"]));
    assert_eq!(ann_now["line_count"], 7);
    assert!(
        rig.cast().await["characters"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["name"] != "Lizzy")
    );
    let morning = rig.line("Good morning to you all,").await;
    assert_eq!(morning["character_id"], ann);
    assert_eq!(
        morning["voice"]["from_chapter"], 1,
        "a chapter-two line takes Ann's voice from her change on"
    );
    admin
        .post_json(
            &rig.url(&format!("/characters/{ann}/merge")),
            &json!({"absorb": ann}),
        )
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    let split = rig.url(&format!("/characters/{ann}/split"));
    let resp = admin
        .post_json(
            &split,
            &json!({"name": "Elizabeth", "aliases": ["Lizzy"],
                    "lines": [rig.take("Good morning to you all,")]}),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let elizabeth = rig.character("Elizabeth").await;
    assert_eq!(elizabeth["id"], resp.json::<Value>()["id"]);
    assert_eq!(elizabeth["line_count"], 1);
    assert_eq!(elizabeth["pooled"], true);
    assert_eq!(voices(&elizabeth)[0]["preset_name"], "Pooled neutral adult");
    assert_eq!(rig.character("Ann").await["aliases"], json!([]));
    admin
        .post_json(
            &split,
            &json!({"name": "Someone", "lines": [rig.take("I am Sadeas, and I came for you,")]}),
        )
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    admin
        .post_json(
            &split,
            &json!({"name": "Tom", "lines": [rig.take("And another,")]}),
        )
        .await
        .assert_status(StatusCode::CONFLICT);

    let character = rig.url(&format!("/characters/{ann}"));
    admin
        .patch_json(
            &character,
            &json!({"bible": {"personality": "Brave.", "gender": null}}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let bible = rig.character("Ann").await["bible"].clone();
    assert_eq!(bible["personality"], "Brave.");
    assert!(bible.get("gender").is_none());
    admin
        .patch_json(&character, &json!({"name": "Tom"}))
        .await
        .assert_status(StatusCode::CONFLICT);

    let changes = rig.url(&format!("/characters/{ann}/voices"));
    admin
        .post_json(
            &changes,
            &json!({"from_chapter": 3, "change": "aged", "description": "Ann, old"}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let steps: Vec<(i64, String)> = voices(&rig.character("Ann").await)
        .iter()
        .map(|v| {
            (
                v["from_chapter"].as_i64().unwrap(),
                v["description"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(steps[2], (3, "Ann, old".to_string()));
    admin
        .post_json(&changes, &json!({"from_chapter": 3}))
        .await
        .assert_status(StatusCode::CONFLICT);
    admin
        .post_json(&changes, &json!({"from_chapter": 0}))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    admin
        .delete(&format!("{changes}/3"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert_eq!(voices(&rig.character("Ann").await).len(), 2);
    admin
        .delete(&format!("{changes}/3"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    admin
        .delete(&format!("{changes}/0"))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    admin
        .post_json(
            &rig.url(&format!("/characters/{tom}/voices")),
            &json!({"from_chapter": 1}),
        )
        .await
        .assert_status(StatusCode::CONFLICT);

    admin
        .patch_json(
            &rig.url(&format!("/characters/{tom}")),
            &json!({"generic": true}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert!(voices(&rig.character("Tom").await).is_empty());
    let sadeas = rig.line("I am Sadeas, and I came for you,").await;
    assert_eq!(
        sadeas["voice"]["character_id"].as_str(),
        Some(rig.id("Narrator").await.as_str()),
        "a speaker made generic is read by the narrator"
    );
}

#[tokio::test]
async fn vetting_routes_require_an_admin_and_a_finished_analysis() {
    let rig = rig().await;
    let id = Uuid::now_v7();
    let routes: Vec<(&str, String, Value)> = vec![
        (
            "GET",
            rig.url(&format!("/characters/{id}/auditions")),
            json!({}),
        ),
        (
            "POST",
            rig.url(&format!("/characters/{id}/auditions")),
            json!({"segment_index": 0}),
        ),
        (
            "POST",
            rig.url(&format!("/characters/{id}/voices/0/approval")),
            json!({}),
        ),
        (
            "DELETE",
            rig.url(&format!("/characters/{id}/voices/0/approval")),
            json!({}),
        ),
        (
            "PUT",
            rig.url(&format!("/characters/{id}/voices/0")),
            json!({}),
        ),
        (
            "POST",
            rig.url(&format!("/characters/{id}/voices")),
            json!({"from_chapter": 1}),
        ),
        (
            "DELETE",
            rig.url(&format!("/characters/{id}/voices/1")),
            json!({}),
        ),
        ("PATCH", rig.url(&format!("/characters/{id}")), json!({})),
        (
            "POST",
            rig.url(&format!("/characters/{id}/merge")),
            json!({"absorb": id}),
        ),
        (
            "POST",
            rig.url(&format!("/characters/{id}/split")),
            json!({"name": "x"}),
        ),
        (
            "POST",
            rig.url(&format!("/characters/{id}/names/move")),
            json!({"name": "x"}),
        ),
        (
            "POST",
            rig.url("/speakers/attach"),
            json!({"speaker": "x", "character_id": id}),
        ),
        ("PUT", rig.url("/lines/0/0"), json!({"character_id": null})),
    ];
    let viewer = create_user_client(&rig.server, "viewer@example.com", "viewer").await;
    let anonymous = rig.server.api_client();
    async fn call(client: &TestClient, verb: &str, url: &str, body: &Value) -> StatusCode {
        match verb {
            "GET" => client.get(url).await,
            "POST" => client.post_json(url, body).await,
            "PUT" => client.put_json(url, body).await,
            "PATCH" => client.patch_json(url, body).await,
            _ => client.delete(url).await,
        }
        .status
    }
    book_casts::set_state(&rig.server.pool, rig.book_id, CastState::Bible, None)
        .await
        .unwrap();
    for (verb, url, body) in &routes {
        assert_eq!(
            call(&anonymous, verb, url, body).await,
            StatusCode::UNAUTHORIZED,
            "{verb} {url}"
        );
        assert_eq!(
            call(&viewer, verb, url, body).await,
            StatusCode::FORBIDDEN,
            "{verb} {url}"
        );
        assert_eq!(
            call(&rig.admin(), verb, url, body).await,
            StatusCode::CONFLICT,
            "{verb} {url}: analysis is still running"
        );
        let elsewhere = url.replace(&rig.book_id.to_string(), &id.to_string());
        assert_eq!(
            call(&rig.admin(), verb, &elsewhere, body).await,
            StatusCode::NOT_FOUND,
            "{verb} {elsewhere}"
        );
    }
}

#[tokio::test]
async fn nothing_is_frozen_while_the_cast_renders_or_after() {
    let rig = rig().await;
    rig.seed().await;
    let ann = rig.id("Ann").await;
    let (segment, span) = rig.lines["Bye now,"];
    for state in [CastState::Rendering, CastState::Done, CastState::Failed] {
        book_casts::set_state(&rig.server.pool, rig.book_id, state, None)
            .await
            .unwrap();
        rig.admin()
            .patch_json(
                &rig.url(&format!("/characters/{ann}")),
                &json!({"bible": {"personality": format!("{state:?}")}}),
            )
            .await
            .assert_status(StatusCode::NO_CONTENT);
        rig.admin()
            .put_json(
                &rig.url(&format!("/characters/{ann}/voices/0")),
                &json!({"description": format!("Ann while {state:?}")}),
            )
            .await
            .assert_ok();
        rig.admin()
            .put_json(
                &rig.url(&format!("/lines/{segment}/{span}")),
                &json!({"character_id": null}),
            )
            .await
            .assert_status(StatusCode::NO_CONTENT);
    }
    assert_eq!(
        voices(&rig.character("Ann").await)[0]["description"],
        "Ann while Failed"
    );
}

#[tokio::test]
async fn a_merge_keeps_an_edit_made_to_the_absorbed_after_it_was_read() {
    use api::repositories::book_cast_vetting as vetting;
    let rig = rig().await;
    let (ann, tom) = (
        Uuid::parse_str(&rig.id("Ann").await).unwrap(),
        Uuid::parse_str(&rig.id("Tom").await).unwrap(),
    );
    let pool = &rig.server.pool;
    let survivor = vetting::character(pool, rig.book_id, ann)
        .await
        .unwrap()
        .unwrap();
    let absorbed = vetting::character(pool, rig.book_id, tom)
        .await
        .unwrap()
        .unwrap();
    rig.admin()
        .patch_json(
            &rig.url(&format!("/characters/{tom}")),
            &json!({"bible": {"accent": "Scots"}, "aliases": ["Tommy"]}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);

    vetting::merge(
        pool,
        rig.book_id,
        survivor.id,
        absorbed.id,
        api::book_cast::POOL_BELOW,
    )
    .await
    .unwrap();

    let ann = rig.character("Ann").await;
    assert_eq!(ann["bible"]["accent"], "Scots", "{}", ann["bible"]);
    assert_eq!(ann["aliases"], json!(["Tom", "Tommy"]));
}
