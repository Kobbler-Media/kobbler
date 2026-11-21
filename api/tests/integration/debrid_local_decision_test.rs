//! A release already on disk is decided exactly as a library file is: the client's
//! codecs count, not just its container.

use crate::common::debrid_rig::*;
use crate::common::debrid_upstream::UpstreamPlan;
use serde_json::{Value, json};

/// A two-second 64x64 mkv encoded with these codecs.
fn encoded(video: &str, audio: &str) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("film.mkv");
    let status = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc=size=64x64:rate=24:duration=2")
        .args(["-f", "lavfi", "-i", "sine=duration=2"])
        .args([
            "-c:v",
            video,
            "-x265-params",
            "log-level=error",
            "-c:a",
            audio,
        ])
        .arg(&out)
        .status()
        .expect("ffmpeg");
    assert!(status.success());
    std::fs::read(out).unwrap()
}

fn mkv_h264_aac_caps() -> Value {
    json!({ "video_codecs": ["h264"], "audio_codecs": ["aac"], "containers": ["mkv", "matroska"] })
}

#[tokio::test]
async fn a_release_on_disk_is_decided_by_the_clients_codecs() {
    let cases = [
        ("hevc video", "libx265", "aac", "video_transcode"),
        ("ac3 audio", "libx264", "ac3", "audio_transcode"),
        ("eac3 audio", "libx264", "eac3", "audio_transcode"),
        ("all supported", "libx264", "aac", "direct_play"),
    ];
    for (what, video, audio, expected) in cases {
        let rig = Rig::start(UpstreamPlan::new(encoded(video, audio)), "film.mkv").await;
        let movie = rig.movie().await;
        rig.put_on_disk("diskhash", movie).await;
        let resolves = rig.kobload_requests("/stream/resolve").await.len();

        let session = rig
            .resolve_with(&rig.resolve_body("diskhash", movie, mkv_h264_aac_caps()))
            .await;
        assert_eq!(
            rig.kobload_requests("/stream/resolve").await.len(),
            resolves,
            "{what}: played from disk without kobload"
        );
        assert_eq!(session["method"], expected, "{what}: {session}");
        let direct = expected == "direct_play";
        assert_eq!(
            session["direct_url"].is_string(),
            direct,
            "{what}: {session}"
        );
        assert_eq!(
            session["playlist_url"].is_string(),
            !direct,
            "{what}: {session}"
        );
        assert_eq!(
            stored_decision(&rig, session_id_of(&session)).0,
            expected,
            "{what}"
        );
    }
}
