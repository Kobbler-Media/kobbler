//! Reading a character's lines in the book: every line they say, any run of
//! the book's paragraphs with who has each quote, the shared minor-part voices
//! set by hand, and the model asked who really says a line.

use crate::book_cast_vetting_test::{Rig, rig, voices};
use crate::common::*;
use api::jobs::handlers::BookCastSuggestHandler;
use reqwest::StatusCode;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

impl Rig {
    async fn set_pooled(&self, id: &str, pooled: bool) -> TestResponse {
        self.admin()
            .patch_json(
                &self.url(&format!("/characters/{id}")),
                &json!({"pooled": pooled}),
            )
            .await
    }

    fn key(&self, text: &str) -> Value {
        let (segment_index, span_start) = self.lines[text];
        json!({"segment_index": segment_index, "span_start": span_start})
    }
}

fn preset_names(c: &Value) -> Vec<Value> {
    voices(c).iter().map(|v| v["preset_name"].clone()).collect()
}

#[tokio::test]
async fn a_character_shares_the_minor_part_voices_or_leaves_them_by_hand() {
    let rig = rig().await;
    rig.seed().await;
    let (ann, lizzy, narrator) = (
        rig.id("Ann").await,
        rig.id("Lizzy").await,
        rig.id("Narrator").await,
    );

    rig.set_pooled(&ann, true)
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let a = rig.character("Ann").await;
    assert_eq!(a["pooled"], true, "five lines would give Ann her own voice");
    assert_eq!(preset_names(&a), vec![json!("Pooled female adult")]);

    let (segment, span) = rig.lines["Bye now,"];
    rig.admin()
        .put_json(
            &rig.url(&format!("/lines/{segment}/{span}")),
            &json!({"character_id": ann}),
        )
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let a = rig.character("Ann").await;
    assert_eq!(a["line_count"], 6);
    assert_eq!(a["pooled"], true, "a recount keeps the choice");

    rig.set_pooled(&lizzy, false)
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let l = rig.character("Lizzy").await;
    assert_eq!(l["pooled"], false, "one line would pool Lizzy");
    assert_eq!(preset_names(&l), vec![json!("Lizzy")]);

    rig.set_pooled(&narrator, true)
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    let user = create_user_client(&rig.server, "reader@test.local", "user").await;
    user.patch_json(
        &rig.url(&format!("/characters/{ann}")),
        &json!({"pooled": false}),
    )
    .await
    .assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_characters_lines_list_in_book_order_and_read_in_the_book_around_them() {
    let rig = rig().await;
    let (ann, tom) = (rig.id("Ann").await, rig.id("Tom").await);
    let admin = rig.admin();

    let resp = admin
        .get(&rig.url(&format!("/characters/{ann}/lines")))
        .await;
    resp.assert_ok();
    let lines = resp.json::<Value>()["lines"].as_array().unwrap().clone();
    let texts: Vec<&str> = lines.iter().map(|l| l["text"].as_str().unwrap()).collect();
    assert_eq!(
        texts,
        vec![
            "Hello there, my old friend,",
            "One more thing before we go,",
            "And another,",
            "We should leave before the rain,",
            "Yes we should,",
        ]
    );
    assert_eq!(lines[0]["chapter_title"], "One");
    assert_eq!(lines[1]["cue"], "whispering");

    let (hello, _) = rig.lines["Hello there, my old friend,"];
    let resp = admin
        .get(&rig.url(&format!("/text?from={}&to={}", hello - 1, hello + 1)))
        .await;
    resp.assert_ok();
    let window: Value = resp.json();
    let paragraphs: Vec<(&str, &Value)> = window["paragraphs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["text"].as_str().unwrap(), &p["chapter_index"]))
        .collect();
    assert_eq!(paragraphs.len(), 3);
    assert_eq!(paragraphs[0], ("One", &json!(0)), "the chapter heading");
    assert!(paragraphs[1].0.starts_with("“Hello there"));
    let who: Vec<&Value> = window["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| &l["character_id"])
        .collect();
    assert_eq!(
        who,
        vec![&json!(ann), &json!(tom), &json!(ann), &json!(ann)],
        "every quote in the window with who has it"
    );
    assert!(window["first"].as_i64().unwrap() <= i64::from(hello - 1));
    assert!(window["last"].as_i64().unwrap() > i64::from(hello + 1));

    for query in [
        format!("from={hello}&to={}", hello - 1),
        "from=0&to=200".to_string(),
    ] {
        admin
            .get(&rig.url(&format!("/text?{query}")))
            .await
            .assert_status(StatusCode::BAD_REQUEST);
    }
    admin
        .get(&rig.url(&format!("/characters/{}/lines", uuid::Uuid::now_v7())))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    let user = create_user_client(&rig.server, "reader@test.local", "user").await;
    for url in [
        rig.url(&format!("/characters/{ann}/lines")),
        rig.url("/text?from=0&to=1"),
    ] {
        user.get(&url).await.assert_status(StatusCode::FORBIDDEN);
    }
}

fn sse(content: &str) -> ResponseTemplate {
    let chunk = json!({"choices": [{"delta": {"content": content}}]});
    ResponseTemplate::new(200).set_body_raw(
        format!("data: {chunk}\n\ndata: [DONE]\n\n"),
        "text/event-stream",
    )
}

#[tokio::test]
async fn the_model_suggests_who_says_lines_read_in_their_scene_and_nothing_changes() {
    let rig = rig().await;
    rig.seed().await;
    let koblem = MockServer::start().await;
    let answer = json!({"lines": [
        {"l": 1, "speaker": "tom", "reason": " Tom answers himself. "},
        {"l": 2, "speaker": "Navani", "reason": "She gives her name."},
    ]});
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse(&answer.to_string()))
        .mount(&koblem)
        .await;
    rig.admin()
        .patch_json(
            "/api/v1/admin/config",
            &json!({"koblem_service_url": koblem.uri()}),
        )
        .await
        .assert_ok();
    let (ann, tom, narrator) = (
        rig.id("Ann").await,
        rig.id("Tom").await,
        rig.id("Narrator").await,
    );

    let body = json!({"lines": [
        rig.key("One more thing before we go,"),
        rig.key("Hello there, my old friend,"),
    ]});
    let resp = rig
        .admin()
        .post_json(&rig.url(&format!("/characters/{ann}/suggest")), &body)
        .await;
    resp.assert_status(StatusCode::ACCEPTED);
    let job_id = resp.json::<Value>()["job_id"].as_str().unwrap().to_string();
    rig.server
        .process_pending_jobs(&BookCastSuggestHandler::new(
            rig.server.pool.clone(),
            rig.server.runtime_config.clone(),
        ))
        .await;

    let asked = koblem.received_requests().await.unwrap();
    assert_eq!(asked.len(), 1, "one scene, one request");
    let sent: Value = serde_json::from_slice(&asked[0].body).unwrap();
    let prompt = sent["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "user")
        .and_then(|m| m["content"].as_str())
        .unwrap();
    assert!(prompt.starts_with("Lines filed under: Ann\n"), "{prompt}");
    for marked in [
        "[L1]“Hello there, my old friend,”",
        "{Tom}“Bye now,”",
        "[L2]“One more thing before we go,”",
        "{Ann}“And another,”",
    ] {
        assert!(prompt.contains(marked), "{marked} in {prompt}");
    }

    let job = rig
        .admin()
        .get(&format!("/api/v1/admin/jobs/{job_id}"))
        .await
        .json::<Value>();
    assert_eq!(job["status"], "completed", "{job}");
    let (hello, _) = rig.lines["Hello there, my old friend,"];
    let (more, _) = rig.lines["One more thing before we go,"];
    assert_eq!(
        job["result"]["suggestions"],
        json!([
            {"segment_index": hello, "span_start": 0, "speaker": "Tom",
             "character_id": tom, "reason": "Tom answers himself."},
            {"segment_index": more, "span_start": 0, "speaker": "Navani",
             "character_id": null, "reason": "She gives her name."},
        ]),
        "a cast name in any case finds the character; a new name stays a name"
    );
    assert_eq!(
        rig.character("Ann").await["line_count"],
        5,
        "suggestions change nothing"
    );

    rig.admin()
        .post_json(
            &rig.url(&format!("/characters/{narrator}/suggest")),
            &json!({}),
        )
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    let user = create_user_client(&rig.server, "reader@test.local", "user").await;
    user.post_json(&rig.url(&format!("/characters/{ann}/suggest")), &json!({}))
        .await
        .assert_status(StatusCode::FORBIDDEN);
}
