//! Integration test for /api/v1/admin/tts/status preset-name enrichment.
//!
//! Voice clips are registered with the chatterbox TTS service under the synthetic
//! id `preset-<uuid>`. Chatterbox doesn't know the user's friendly preset name —
//! that lives in `user_voice_presets`. The status handler must look up
//! `preset-<uuid>` ids against the caller's presets and substitute the friendly
//! name into the response so all voice-selection UIs show the right label.

use crate::common::*;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn tts_status_replaces_preset_uuid_with_friendly_name() {
    let mock_tts = MockServer::start().await;
    let server = TestServer::start_api_only().await;
    let user_client = create_user_client(&server, "voicestatus@test.com", "viewer").await;

    let resp = user_client
        .post_json(
            "/api/v1/admin/tts/presets",
            &json!({"name": "My Cool Voice"}),
        )
        .await;
    resp.assert_success();
    let preset: Value = resp.json();
    let preset_id = preset["id"].as_str().unwrap().to_string();
    let synthetic_id = format!("preset-{preset_id}");

    Mock::given(method("GET"))
        .and(path("/v1/audio/voices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "voices": [
                {"id": "default", "name": "Default (built-in)"},
                {"id": synthetic_id.clone(), "name": synthetic_id.clone()}
            ]
        })))
        .mount(&mock_tts)
        .await;

    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "tts_enabled": true,
                "tts_service_url": mock_tts.uri(),
                "tts_service_type": "chatterbox"
            }),
        )
        .await
        .assert_ok();

    let resp = user_client.get("/api/v1/admin/tts/status").await;
    resp.assert_ok();
    let body: Value = resp.json();
    let voices = body["voices"]
        .as_array()
        .expect("voices field should be an array");

    let preset_voice = voices
        .iter()
        .find(|v| v["id"].as_str() == Some(synthetic_id.as_str()))
        .unwrap_or_else(|| panic!("preset voice missing from response: {body}"));
    assert_eq!(
        preset_voice["name"].as_str(),
        Some("My Cool Voice"),
        "preset voice name should be replaced with the user-defined name; got: {preset_voice}"
    );

    let default_voice = voices
        .iter()
        .find(|v| v["id"].as_str() == Some("default"))
        .expect("default voice missing");
    assert_eq!(
        default_voice["name"].as_str(),
        Some("Default (built-in)"),
        "non-preset voices should be untouched"
    );
}

#[tokio::test]
async fn tts_status_leaves_other_users_preset_voices_with_synthetic_id() {
    // A preset-<uuid> voice that doesn't belong to the requesting user (or doesn't
    // exist in our DB at all) should be returned as-is. The handler enriches names
    // it knows about; it doesn't filter or guess.
    let mock_tts = MockServer::start().await;
    let server = TestServer::start_api_only().await;
    let user_client = create_user_client(&server, "voiceother@test.com", "viewer").await;

    let stranger_id = uuid::Uuid::new_v4();
    let stranger_synthetic = format!("preset-{stranger_id}");

    Mock::given(method("GET"))
        .and(path("/v1/audio/voices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "voices": [
                {"id": stranger_synthetic.clone(), "name": stranger_synthetic.clone()}
            ]
        })))
        .mount(&mock_tts)
        .await;

    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "tts_enabled": true,
                "tts_service_url": mock_tts.uri(),
                "tts_service_type": "chatterbox"
            }),
        )
        .await
        .assert_ok();

    let resp = user_client.get("/api/v1/admin/tts/status").await;
    resp.assert_ok();
    let body: Value = resp.json();
    let voices = body["voices"].as_array().unwrap();
    let stranger = voices
        .iter()
        .find(|v| v["id"].as_str() == Some(stranger_synthetic.as_str()))
        .expect("stranger voice should still appear in response");
    assert_eq!(stranger["name"].as_str(), Some(stranger_synthetic.as_str()));
}

#[tokio::test]
async fn tts_status_parses_legacy_voice_map_shape() {
    // The engine's legacy shape keys voices by model id with a flat string
    // array. The status handler flattens that into the TtsVoice shape and
    // surfaces engine="breeze" + supports_emotion=true so the voice page can
    // gate engine-specific UI sections.
    let mock_tts = MockServer::start().await;
    let server = TestServer::start_api_only().await;
    let user_client = create_user_client(&server, "voicebreeze@test.com", "viewer").await;

    Mock::given(method("GET"))
        .and(path("/v1/audio/voices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "breeze-tts-2": ["default", "alice", "bob"]
        })))
        .mount(&mock_tts)
        .await;

    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "tts_enabled": true,
                "tts_service_url": mock_tts.uri(),
                "tts_service_type": "breeze"
            }),
        )
        .await
        .assert_ok();

    let resp = user_client.get("/api/v1/admin/tts/status").await;
    resp.assert_ok();
    let body: Value = resp.json();

    assert_eq!(
        body["engine"].as_str(),
        Some("breeze"),
        "engine label = breeze"
    );
    assert_eq!(
        body["supports_emotion"].as_bool(),
        Some(true),
        "breeze supports voice cloning + temperature"
    );
    assert_eq!(
        body["supports_alignment"].as_bool(),
        Some(true),
        "breeze is the aligner-capable engine — the reader gates on this"
    );

    let voices = body["voices"].as_array().expect("voices should be array");
    let names: Vec<&str> = voices.iter().filter_map(|v| v["name"].as_str()).collect();
    assert!(names.contains(&"alice"));
    assert!(names.contains(&"bob"));

    // In the legacy shape a voice has id == name (no separate id/name upstream).
    let alice = voices
        .iter()
        .find(|v| v["id"].as_str() == Some("alice"))
        .unwrap();
    assert_eq!(alice["name"].as_str(), Some("alice"));
}

/// The live engine answers with BOTH shapes at once, listing the same ids; the
/// richer `voices` array just carries more per voice. The array must win, and
/// `default` must survive it — that entry is a real selectable value meaning
/// "the model's own narrator", not a pseudo-entry to filter out.
#[tokio::test]
async fn tts_status_prefers_the_rich_voice_array_over_the_legacy_map() {
    let mock_tts = MockServer::start().await;
    let server = TestServer::start_api_only().await;
    let user_client = create_user_client(&server, "voicebothshapes@test.com", "viewer").await;

    Mock::given(method("GET"))
        .and(path("/v1/audio/voices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "breeze-tts-2": ["default", "gaiman"],
            "model_id": "breeze-tts-2",
            "voices": [
                {"id": "default", "builtin": true},
                {"id": "gaiman", "builtin": false, "frames": 150, "has_ref_text": true}
            ]
        })))
        .mount(&mock_tts)
        .await;

    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({
                "tts_enabled": true,
                "tts_service_url": mock_tts.uri(),
                "tts_service_type": "breeze"
            }),
        )
        .await
        .assert_ok();

    let resp = user_client.get("/api/v1/admin/tts/status").await;
    resp.assert_ok();
    let body: Value = resp.json();
    let voices = body["voices"].as_array().expect("voices should be array");
    let names: Vec<&str> = voices.iter().filter_map(|v| v["id"].as_str()).collect();
    assert_eq!(
        names,
        ["default", "gaiman"],
        "the rich array is authoritative and `default` is a real voice: {body}"
    );
}
