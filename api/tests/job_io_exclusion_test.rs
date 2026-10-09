//! Tests for I/O-heavy job mutual exclusion between audiobook_transcribe and
//! media_extract.

use api::jobs::handlers::AudiobookTranscribePayload;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// AudiobookTranscribePayload.manual defaults to false when omitted.
#[test]
fn transcribe_payload_manual_defaults_false() {
    let payload: AudiobookTranscribePayload =
        serde_json::from_value(json!({"book_id": "00000000-0000-0000-0000-000000000001"})).unwrap();
    assert!(!payload.manual);
}

/// AudiobookTranscribePayload.manual can be set to true.
#[test]
fn transcribe_payload_manual_explicit_true() {
    let payload: AudiobookTranscribePayload = serde_json::from_value(
        json!({"book_id": "00000000-0000-0000-0000-000000000001", "manual": true}),
    )
    .unwrap();
    assert!(payload.manual);
}

/// A single-permit semaphore prevents two holders from overlapping.
/// This validates the mechanism used for transcribe/extract mutual exclusion.
#[tokio::test]
async fn io_heavy_semaphore_mutual_exclusion() {
    let sem = Arc::new(Semaphore::new(1));
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // First holder acquires the permit.
    let permit = sem.clone().acquire_owned().await.unwrap();

    // Second holder cannot acquire while first holds it.
    let sem2 = sem.clone();
    let flag2 = flag.clone();
    let handle = tokio::spawn(async move {
        // try_acquire should fail immediately
        assert!(sem2.try_acquire().is_err(), "semaphore should be held");
        flag2.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    handle.await.unwrap();
    assert!(flag.load(std::sync::atomic::Ordering::SeqCst));

    // After releasing, it becomes available.
    drop(permit);
    assert!(sem.try_acquire().is_ok());
}

/// Background transcription releases the semaphore between files, allowing
/// media_extract to interleave. Simulates the per-file acquire/release pattern.
#[tokio::test]
async fn background_transcribe_yields_between_files() {
    let sem = Arc::new(Semaphore::new(1));

    // Simulate background transcription: acquire, do work, release per file.
    {
        let permit = sem.clone().acquire_owned().await.unwrap();
        // Semaphore is held during "transcription"
        assert!(sem.try_acquire().is_err());
        drop(permit);
    }

    // Between files, the semaphore is available for media_extract.
    assert!(sem.try_acquire().is_ok());
}
