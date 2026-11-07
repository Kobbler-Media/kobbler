//! A read-aloud cast to a receiver: one render, heard as one progressive AAC
//! stream by a receiver holding only a playback token, and followed by the
//! phone through the same timeline the synthesize route sends, minus the audio.

use crate::common::*;
use base64::Engine;
use reqwest::StatusCode;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sse(events: &[Value]) -> String {
    events.iter().map(|e| format!("data: {e}\n\n")).collect()
}

/// The aligning engine behind koblem: `deltas` of 500 ms each per passage.
async fn paragraph_engine(deltas: usize) -> MockServer {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/audio/voices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "voices": [{"id": "kobbler-narrator", "name": "Narrator"}]
        })))
        .mount(&mock)
        .await;
    let b64 = base64::engine::general_purpose::STANDARD.encode(tone_pcm(500));
    let mut events = vec![json!({
        "type": "speech.audio.chunk", "index": 0, "chunks_total": 1,
        "word_offset": 0, "word_count": 4, "audio_offset_ms": 0, "text": "One two three four."
    })];
    events.extend((0..deltas).map(|_| json!({"type": "speech.audio.delta", "audio": b64})));
    events.push(json!({"type": "speech.audio.done"}));
    Mock::given(method("POST"))
        .and(path("/api/v1/tts/generate"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse(&events), "text/event-stream"))
        .mount(&mock)
        .await;
    mock
}

/// A buffered engine that answers every sentence with `ms` of audio.
async fn sentence_engine(ms: usize) -> MockServer {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(tone_wav(ms), "audio/wav"))
        .mount(&mock)
        .await;
    mock
}

async fn configure(server: &TestServer, engine: &MockServer, aligning: bool) {
    let mut patch = json!({
        "tts_enabled": true,
        "tts_service_url": engine.uri(),
        "tts_service_type": if aligning { "breeze" } else { "kokoro" },
        "route_gpu_via_koblem": false,
    });
    if aligning {
        patch["koblem_service_url"] = json!(engine.uri());
        patch["koblem_api_key"] = json!("koblem-key");
    }
    server
        .admin_client()
        .patch_json("/api/v1/admin/config", &patch)
        .await
        .assert_ok();
}

async fn start_cast(client: &TestClient, text: &str) -> String {
    let resp = client
        .post_json("/api/v1/admin/tts/cast", &json!({"text": text}))
        .await;
    resp.assert_ok();
    resp.json::<Value>()["id"].as_str().unwrap().to_string()
}

async fn playback_token(client: &TestClient) -> String {
    let resp = client
        .post_json("/api/v1/admin/playback/tokens", &json!({}))
        .await;
    resp.assert_ok();
    resp.json::<Value>()["token"].as_str().unwrap().to_string()
}

fn lines(body: &str) -> Vec<Value> {
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn decoded_ms(aac: &[u8]) -> usize {
    assert_eq!(&aac[..2], &[0xFF, 0xF1], "not an ADTS AAC stream");
    let pcm = kob_ffmpeg::pcm_stream::decode_to_pcm(aac.to_vec(), TONE_RATE as i32).unwrap();
    pcm.len() / 2 * 1000 / TONE_RATE
}

fn assert_about(actual: usize, expected: usize) {
    assert!(
        actual + 30 >= expected && actual <= expected + 120,
        "decoded {actual} ms, expected about {expected} ms"
    );
}

async fn listen(server: &TestServer, id: &str, token: &str, from_ms: Option<u64>) -> TestResponse {
    let from = from_ms
        .map(|ms| format!("&from_ms={ms}"))
        .unwrap_or_default();
    server
        .api_client()
        .get(&format!(
            "/api/v1/admin/tts/cast/{id}/audio?token={token}{from}"
        ))
        .await
}

#[tokio::test]
async fn a_cast_read_is_one_aac_stream_of_the_whole_passage() {
    let engine = paragraph_engine(3).await;
    let server = TestServer::start_api_only().await;
    configure(&server, &engine, true).await;
    let reader = create_user_client(&server, "castpara@test.com", "viewer").await;

    let id = start_cast(&reader, "One two three four.").await;
    let token = playback_token(&reader).await;
    let audio = listen(&server, &id, &token, None).await;

    audio.assert_ok();
    assert_eq!(audio.headers["content-type"], "audio/aac");
    assert!(audio.headers.get("content-length").is_none());
    assert_about(decoded_ms(audio.bytes()), 1500);
}

#[tokio::test]
async fn the_phone_follows_the_cast_on_the_timeline_without_the_audio() {
    let engine = paragraph_engine(2).await;
    let server = TestServer::start_api_only().await;
    configure(&server, &engine, true).await;
    let reader = create_user_client(&server, "castevents@test.com", "viewer").await;

    let id = start_cast(&reader, "One two three four.").await;
    let resp = reader
        .get(&format!("/api/v1/admin/tts/cast/{id}/events"))
        .await;
    resp.assert_ok();
    let events = lines(&resp.text());
    let types: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();

    assert_eq!(types, ["paragraph_start", "chunk", "done"]);
    assert_eq!(events[0]["word_offset"], json!(0));
    assert_eq!(events[0]["audio_offset_ms"], json!(0));
    assert_eq!(events[1]["word_count"], json!(4));
}

#[tokio::test]
async fn a_cast_from_a_buffered_engine_places_each_sentence_on_the_stream() {
    let engine = sentence_engine(700).await;
    let server = TestServer::start_api_only().await;
    configure(&server, &engine, false).await;
    let reader = create_user_client(&server, "castsentence@test.com", "viewer").await;

    let id = start_cast(&reader, "The first sentence. The second one.").await;
    let token = playback_token(&reader).await;
    let audio = listen(&server, &id, &token, None).await;
    audio.assert_ok();
    assert_about(decoded_ms(audio.bytes()), 1400);

    let events = lines(
        &reader
            .get(&format!("/api/v1/admin/tts/cast/{id}/events"))
            .await
            .text(),
    );
    let sentences: Vec<&Value> = events.iter().filter(|e| e["type"] == "sentence").collect();
    assert_eq!(sentences.len(), 2);
    assert_eq!(sentences[0]["audio_offset_ms"], json!(0));
    assert_eq!(sentences[1]["audio_offset_ms"], json!(700));
    assert_eq!(sentences[1]["duration_ms"], json!(700));
    assert_eq!(sentences[1]["text"], json!("The second one."));
    assert!(sentences.iter().all(|s| s.get("audio_b64").is_none()));
    assert_eq!(events.last().unwrap()["type"], json!("done"));
}

/// A receiver whose stream dropped mid-read reopens it where it was.
#[tokio::test]
async fn a_receiver_picks_the_stream_up_part_way() {
    let engine = paragraph_engine(4).await;
    let server = TestServer::start_api_only().await;
    configure(&server, &engine, true).await;
    let reader = create_user_client(&server, "castresume@test.com", "viewer").await;

    let id = start_cast(&reader, "One two three four.").await;
    let token = playback_token(&reader).await;
    let audio = listen(&server, &id, &token, Some(1500)).await;

    audio.assert_ok();
    assert_about(decoded_ms(audio.bytes()), 500);
}

/// Both entry points share one resolution path, so the phone's own read must
/// still carry its audio after the cast route was split out of it.
#[tokio::test]
async fn reading_on_the_phone_still_streams_the_audio_inline() {
    for aligning in [true, false] {
        let engine = if aligning {
            paragraph_engine(2).await
        } else {
            sentence_engine(300).await
        };
        let server = TestServer::start_api_only().await;
        configure(&server, &engine, aligning).await;
        let reader = create_user_client(&server, "phoneread@test.com", "viewer").await;

        let resp = reader
            .post_json(
                "/api/v1/admin/tts/synthesize",
                &json!({"text": "One two three four."}),
            )
            .await;
        resp.assert_ok();
        let events = lines(&resp.text());
        let field = if aligning { "pcm_b64" } else { "audio_b64" };
        assert!(
            events.iter().any(|e| e.get(field).is_some()),
            "no {field} in {events:?}"
        );
        assert_eq!(events.last().unwrap()["type"], json!("done"));
    }
}

#[tokio::test]
async fn a_cast_is_heard_only_with_its_owners_credentials() {
    let engine = paragraph_engine(1).await;
    let server = TestServer::start_api_only().await;
    configure(&server, &engine, true).await;
    let reader = create_user_client(&server, "castowner@test.com", "viewer").await;
    let stranger = create_user_client(&server, "caststranger@test.com", "viewer").await;
    let id = start_cast(&reader, "One two three four.").await;

    server
        .api_client()
        .post_json("/api/v1/admin/tts/cast", &json!({"text": "Hello."}))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    server
        .api_client()
        .get(&format!("/api/v1/admin/tts/cast/{id}/audio"))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    let theirs = playback_token(&stranger).await;
    listen(&server, &id, &theirs, None)
        .await
        .assert_status(StatusCode::NOT_FOUND);
    stranger
        .get(&format!("/api/v1/admin/tts/cast/{id}/events"))
        .await
        .assert_status(StatusCode::NOT_FOUND);

    let mine = playback_token(&reader).await;
    server
        .api_client()
        .get(&format!("/api/v1/admin/tts/cast/{id}/events?token={mine}"))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    listen(&server, &uuid::Uuid::new_v4().to_string(), &mine, None)
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_cast_is_refused_what_a_read_would_be() {
    let engine = paragraph_engine(1).await;
    let server = TestServer::start_api_only().await;
    let reader = create_user_client(&server, "castrefused@test.com", "viewer").await;

    reader
        .post_json("/api/v1/admin/tts/cast", &json!({"text": "Hello."}))
        .await
        .assert_status(StatusCode::SERVICE_UNAVAILABLE);

    configure(&server, &engine, true).await;
    reader
        .post_json("/api/v1/admin/tts/cast", &json!({"text": ""}))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}
