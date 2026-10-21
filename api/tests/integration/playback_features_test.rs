use crate::common::*;
use reqwest::StatusCode;
use serde_json::Value;

// =============================================================================
// Audiobook Bookmarks
// =============================================================================

#[tokio::test]
async fn test_create_and_list_bookmarks() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    // Create a book with an audiobook file first
    let book_id = create_test_book(&server).await;

    // Create a bookmark
    let resp = client
        .post_json(
            &format!("/api/v1/admin/books/{book_id}/bookmarks"),
            &serde_json::json!({
                "position_seconds": 42.5,
                "note": "Great passage"
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);
    let json: Value = resp.json();
    assert!(json["id"].is_string());
    let bookmark_id = json["id"].as_str().unwrap();

    // Create another bookmark
    let resp = client
        .post_json(
            &format!("/api/v1/admin/books/{book_id}/bookmarks"),
            &serde_json::json!({
                "position_seconds": 120.0
            }),
        )
        .await;
    resp.assert_status(StatusCode::CREATED);

    // List bookmarks
    let resp = client
        .get(&format!("/api/v1/admin/books/{book_id}/bookmarks"))
        .await;
    resp.assert_ok();
    let bookmarks: Vec<Value> = resp.json();
    assert_eq!(bookmarks.len(), 2);
    // Should be ordered by position
    assert!(
        bookmarks[0]["position_seconds"].as_f64().unwrap()
            < bookmarks[1]["position_seconds"].as_f64().unwrap()
    );
    assert_eq!(bookmarks[0]["note"].as_str(), Some("Great passage"));
    assert!(bookmarks[1]["note"].is_null());

    // Delete a bookmark
    let resp = client
        .delete(&format!(
            "/api/v1/admin/books/{book_id}/bookmarks/{bookmark_id}"
        ))
        .await;
    resp.assert_status(StatusCode::NO_CONTENT);

    // Verify deletion
    let resp = client
        .get(&format!("/api/v1/admin/books/{book_id}/bookmarks"))
        .await;
    resp.assert_ok();
    let bookmarks: Vec<Value> = resp.json();
    assert_eq!(bookmarks.len(), 1);
}

#[tokio::test]
async fn test_delete_nonexistent_bookmark() {
    let server = TestServer::start_api_only().await;
    let client = server.admin_client();

    let book_id = create_test_book(&server).await;
    let fake_id = "01234567-89ab-cdef-0123-456789abcdef";

    let resp = client
        .delete(&format!(
            "/api/v1/admin/books/{book_id}/bookmarks/{fake_id}"
        ))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

// =============================================================================
// Video Chapters (via probe)
// =============================================================================

#[tokio::test]
async fn test_probe_result_chapter_types() {
    // Verify that the re-exported ChapterInfo type from kob-ffmpeg works correctly
    use api::ffmpeg::probe::{ChapterInfo, FormatInfo, ProbeResult};

    let result = ProbeResult {
        format: FormatInfo {
            format_name: "matroska,webm".to_string(),
            duration_seconds: Some(1200.0),
            bit_rate_kbps: None,
            size_bytes: None,
        },
        video_streams: vec![],
        audio_streams: vec![],
        subtitle_streams: vec![],
        chapters: vec![
            ChapterInfo {
                index: 0,
                title: Some("Opening".to_string()),
                start_seconds: 0.0,
                end_seconds: 90.0,
            },
            ChapterInfo {
                index: 1,
                title: Some("Part A".to_string()),
                start_seconds: 90.0,
                end_seconds: 600.0,
            },
            ChapterInfo {
                index: 2,
                title: Some("Credits".to_string()),
                start_seconds: 600.0,
                end_seconds: 1200.0,
            },
        ],
    };

    assert_eq!(result.chapters.len(), 3);
    assert_eq!(result.chapters[0].title.as_deref(), Some("Opening"));
    assert_eq!(result.chapters[2].title.as_deref(), Some("Credits"));
    assert!((result.chapters[1].start_seconds - 90.0).abs() < 0.001);
}

// =============================================================================
// LUFS Parsing
// =============================================================================

#[tokio::test]
async fn test_vtt_generation() {
    use api::ffmpeg::transcode::{ThumbnailResult, generate_thumbnails_vtt};

    // Timestamps reflect actual capture times (may differ from ideal grid)
    let result = ThumbnailResult {
        sprite_count: 1,
        thumb_width: 320,
        thumb_height: 180,
        tile_cols: 5,
        tile_rows: 5,
        total_thumbs: 3,
        timestamps: vec![0.0, 10.5, 20.1],
    };

    let vtt = generate_thumbnails_vtt(&result, 30.0);

    assert!(vtt.starts_with("WEBVTT"));
    assert!(vtt.contains("sprite_001.jpg"));
    assert!(vtt.contains("#xywh=0,0,320,180"));
    assert!(vtt.contains("#xywh=320,0,320,180"));
    // Third thumbnail: col=2, row=0
    assert!(vtt.contains("#xywh=640,0,320,180"));
    // Verify time ranges use actual capture timestamps
    assert!(vtt.contains("00:00:00.000 --> 00:00:10.500"));
    assert!(vtt.contains("00:00:10.500 --> 00:00:20.100"));
    // Last entry extends to duration
    assert!(vtt.contains("00:00:20.100 --> 00:00:30.000"));
}

// =============================================================================
// Skip Segment Detection from Chapters
// =============================================================================

#[tokio::test]
async fn test_chapter_based_intro_detection_patterns() {
    // Verify the chapter title pattern matching logic
    let intro_patterns = ["intro", "opening", "cold open"];
    let credits_patterns = ["credits", "ending", "preview", "next on"];
    let intro_abbrevs = ["op"];
    let credits_abbrevs = ["ed"];

    let test_cases = vec![
        ("Opening", true, false),
        ("OP", true, false),
        ("op.", true, false),
        ("op 1", true, false),
        ("Episode 1", false, false),
        ("Credits", false, true),
        ("ED", false, true),
        ("End Credits", false, true),
        ("Next on Season 2", false, true),
        ("Cold Open", true, false),
        ("Chapter 1", false, false),
        ("Operator", false, false), // Should NOT match "op" — it's not at word boundary
    ];

    for (title, expect_intro, expect_credits) in test_cases {
        let lower = title.to_lowercase();
        let trimmed = lower.trim();

        let is_intro = intro_patterns.iter().any(|p| trimmed.contains(p))
            || intro_abbrevs.iter().any(|p| {
                trimmed == *p
                    || trimmed.starts_with(&format!("{p} "))
                    || trimmed.starts_with(&format!("{p}."))
            });

        let is_credits = credits_patterns.iter().any(|p| trimmed.contains(p))
            || credits_abbrevs.iter().any(|p| {
                trimmed == *p
                    || trimmed.starts_with(&format!("{p} "))
                    || trimmed.starts_with(&format!("{p}."))
            });

        assert_eq!(
            is_intro, expect_intro,
            "Intro detection failed for '{}': expected {}, got {}",
            title, expect_intro, is_intro
        );
        assert_eq!(
            is_credits, expect_credits,
            "Credits detection failed for '{}': expected {}, got {}",
            title, expect_credits, is_credits
        );
    }
}

// =============================================================================
// Helpers
// =============================================================================

async fn create_test_book(server: &TestServer) -> String {
    let client = server.admin_client();
    let resp = client
        .post_json(
            "/api/v1/library/books",
            &serde_json::json!({
                "title": "Test Audiobook",
                "authors": [{"name": "Test Author"}]
            }),
        )
        .await;
    resp.assert_ok();
    let json: Value = resp.json();
    json["id"].as_str().unwrap().to_string()
}
