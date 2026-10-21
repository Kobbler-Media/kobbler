//! Integration tests for the "voice preset from audiobook" flow:
//! POST /api/v1/admin/tts/presets/{preset_id}/clip-from-audiobook

use crate::common::*;
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;

/// Write a minimal 16-bit PCM mono WAV. Values are i16 little-endian samples.
fn write_wav(path: &Path, sample_rate: u32, samples: &[i16]) {
    let bytes_per_sample = 2u32;
    let data_bytes = samples.len() as u32 * bytes_per_sample;
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(b"RIFF").unwrap();
    f.write_all(&(36u32 + data_bytes).to_le_bytes()).unwrap();
    f.write_all(b"WAVE").unwrap();
    f.write_all(b"fmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&sample_rate.to_le_bytes()).unwrap();
    f.write_all(&(sample_rate * bytes_per_sample).to_le_bytes())
        .unwrap();
    f.write_all(&(bytes_per_sample as u16).to_le_bytes())
        .unwrap();
    f.write_all(&16u16.to_le_bytes()).unwrap();
    f.write_all(b"data").unwrap();
    f.write_all(&data_bytes.to_le_bytes()).unwrap();
    for &s in samples {
        f.write_all(&s.to_le_bytes()).unwrap();
    }
}

fn synth_60s_with_tone(sample_rate: u32) -> Vec<i16> {
    let total = sample_rate as usize * 60;
    let mut samples = vec![0i16; total];
    let tone_start = sample_rate as usize * 28;
    let tone_end = sample_rate as usize * 33;
    let freq = 440.0;
    let amp = 20_000.0;
    for (i, sample) in samples
        .iter_mut()
        .enumerate()
        .take(tone_end)
        .skip(tone_start)
    {
        let t = i as f64 / sample_rate as f64;
        *sample = ((2.0 * std::f64::consts::PI * freq * t).sin() * amp) as i16;
    }
    samples
}

#[tokio::test]
async fn clip_from_audiobook_writes_wav_and_updates_preset() {
    let server = TestServer::start_api_only().await;
    let user_client = create_user_client(&server, "voiceclip@test.com", "viewer").await;

    // Look up the user's id directly from DB via email (no helper exposes it).
    let user_id: uuid::Uuid = sqlx::query_scalar!(
        "SELECT id FROM users WHERE email = $1",
        "voiceclip@test.com"
    )
    .fetch_one(&server.pool)
    .await
    .unwrap();

    // Write a synthetic audiobook WAV under the test storage_root.
    let storage = server.storage_root();
    let book_dir = std::path::PathBuf::from(&storage).join("audiobooks/test");
    std::fs::create_dir_all(&book_dir).unwrap();
    let wav_path = book_dir.join("ch01.wav");
    write_wav(&wav_path, 16_000, &synth_60s_with_tone(16_000));

    // Insert book + audiobook_files row referencing the absolute path.
    let book_id = create_book(&server, "Test Audiobook").await;
    let wav_path_str = wav_path.to_string_lossy().to_string();
    let wav_size = std::fs::metadata(&wav_path).unwrap().len() as i64;
    sqlx::query!(
        "INSERT INTO audiobook_files (book_id, file_index, format, file_path, file_size, duration_seconds)
         VALUES ($1, 0, 'WAV', $2, $3, 60.0)",
        book_id,
        wav_path_str,
        wav_size,
    )
    .execute(&server.pool)
    .await
    .unwrap();

    // Create a voice preset for this user.
    let resp = user_client
        .post_json(
            "/api/v1/admin/tts/presets",
            &json!({"name": "From audiobook test"}),
        )
        .await;
    resp.assert_success();
    let preset: Value = resp.json();
    let preset_id = preset["id"].as_str().unwrap().to_string();

    // Cut a 5-second window starting at offset 10s.
    let resp = user_client
        .post_json(
            &format!("/api/v1/admin/tts/presets/{preset_id}/clip-from-audiobook"),
            &json!({
                "audiobook_id": book_id.to_string(),
                "chapter_index": 0,
                "offset_seconds": 10.0,
                "duration_seconds": 5.0,
            }),
        )
        .await;
    resp.assert_ok();
    let updated: Value = resp.json();
    assert!(
        updated["clip_filename"].as_str().is_some(),
        "preset should have clip_filename set, got: {updated:?}"
    );

    // Verify the WAV exists at the expected path with a sane size for ~5s of mono 16kHz PCM.
    let clip_path = std::path::PathBuf::from(&storage)
        .join("voices")
        .join(user_id.to_string())
        .join(format!("{preset_id}.wav"));
    assert!(
        clip_path.is_file(),
        "clip wav should exist at {clip_path:?}"
    );
    let len = std::fs::metadata(&clip_path).unwrap().len();
    assert!(
        (40_000..400_000).contains(&len),
        "clip size {len} bytes outside expected range for ~5s mono 16kHz PCM"
    );
}

#[tokio::test]
async fn suggest_audiobook_clip_returns_offset_in_tone_region() {
    let server = TestServer::start_api_only().await;
    let user_client = create_user_client(&server, "voicesuggest@test.com", "viewer").await;

    let storage = server.storage_root();
    let book_dir = std::path::PathBuf::from(&storage).join("audiobooks/suggest");
    std::fs::create_dir_all(&book_dir).unwrap();
    let wav_path = book_dir.join("ch01.wav");
    write_wav(&wav_path, 16_000, &synth_60s_with_tone(16_000));

    let book_id = create_book(&server, "Suggest Test").await;
    let wav_path_str = wav_path.to_string_lossy().to_string();
    let wav_size = std::fs::metadata(&wav_path).unwrap().len() as i64;
    sqlx::query!(
        "INSERT INTO audiobook_files (book_id, file_index, format, file_path, file_size, duration_seconds)
         VALUES ($1, 0, 'WAV', $2, $3, 60.0)",
        book_id,
        wav_path_str,
        wav_size,
    )
    .execute(&server.pool)
    .await
    .unwrap();

    let resp = user_client
        .get(&format!(
            "/api/v1/admin/tts/audiobook-clip/suggest?audiobook_id={book_id}&chapter_index=0&duration_seconds=5"
        ))
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    let offset = body["offset_seconds"].as_f64().unwrap();
    assert!(
        (25.0..=33.0).contains(&offset),
        "expected offset near tone region (28-33s), got {offset}"
    );
    assert_eq!(body["chapter_index"].as_i64().unwrap(), 0);
    assert_eq!(body["duration_seconds"].as_f64().unwrap(), 5.0);
}
