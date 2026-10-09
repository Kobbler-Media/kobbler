// =============================================================================
// is_connection_limit_error
// =============================================================================

#[test]
fn test_connection_limit_error_detection() {
    use api::iptv::live_session::is_connection_limit_error;

    // Should detect HTTP 458
    assert!(is_connection_limit_error(
        "ffprobe exited with status exit status: 1: HTTP error 458 Max connections"
    ));

    // Should detect 429 (rate limit)
    assert!(is_connection_limit_error(
        "HTTP error 429 Too Many Requests"
    ));

    // Should detect 403 (forbidden — often used for expired/maxed sessions)
    assert!(is_connection_limit_error("Server returned 403 Forbidden"));

    // Should detect textual connection limit messages
    assert!(is_connection_limit_error("max connections exceeded"));
    assert!(is_connection_limit_error("Too many connections to server"));
    assert!(is_connection_limit_error(
        "Connection refused by remote host"
    ));

    // Should detect reqwest error_for_status() format
    assert!(is_connection_limit_error(
        "HTTP status client error (458) for url (http://provider.example.com/stream)"
    ));
    assert!(is_connection_limit_error(
        "HTTP status client error (403 Forbidden) for url"
    ));

    // Should NOT flag normal probe failures
    assert!(!is_connection_limit_error(
        "ffprobe exited with status exit status: 1: "
    ));
    assert!(!is_connection_limit_error(
        "no video stream found in probe output"
    ));
    assert!(!is_connection_limit_error(
        "Server returned 500 Internal Server Error"
    ));
    assert!(!is_connection_limit_error("timeout waiting for stream"));
}

// =============================================================================
// probe_result_to_meta
// =============================================================================

#[test]
fn test_probe_result_to_meta_valid() {
    use api::iptv::live_session::probe_result_to_meta;
    use kob_ffmpeg::probe::{AudioStream, FormatInfo, ProbeResult, SubtitleStream, VideoStream};

    let result = ProbeResult {
        format: FormatInfo {
            format_name: "mpegts".to_string(),
            duration_seconds: None,
            bit_rate_kbps: None,
            size_bytes: None,
        },
        video_streams: vec![VideoStream {
            index: 0,
            codec: "hevc".to_string(),
            profile: None,
            width: 1920,
            height: 1080,
            bitrate_kbps: None,
            framerate: Some(25.0),
            bit_depth: Some(8),
            is_hdr: false,
            hdr_format: None,
            is_interlaced: false,
        }],
        audio_streams: vec![
            AudioStream {
                index: 1,
                codec: "ac3".to_string(),
                channels: 6,
                sample_rate: Some(48000),
                bitrate_kbps: Some(640),
                language: Some("eng".to_string()),
                title: Some("Surround".to_string()),
                is_default: true,
            },
            AudioStream {
                index: 2,
                codec: "aac".to_string(),
                channels: 2,
                sample_rate: Some(48000),
                bitrate_kbps: Some(128),
                language: Some("spa".to_string()),
                title: None,
                is_default: false,
            },
        ],
        subtitle_streams: vec![],
        chapters: vec![],
    };

    let (video_codec, audio_tracks) = probe_result_to_meta(&result).unwrap();

    assert_eq!(video_codec, "hevc");
    assert_eq!(audio_tracks.len(), 2);

    assert_eq!(audio_tracks[0].stream_index, 1);
    assert_eq!(audio_tracks[0].codec, "ac3");
    assert_eq!(audio_tracks[0].channels, 6);
    assert_eq!(audio_tracks[0].language.as_deref(), Some("eng"));
    assert_eq!(audio_tracks[0].title.as_deref(), Some("Surround"));
    assert!(audio_tracks[0].is_default);

    assert_eq!(audio_tracks[1].stream_index, 2);
    assert_eq!(audio_tracks[1].codec, "aac");
    assert_eq!(audio_tracks[1].channels, 2);
    assert_eq!(audio_tracks[1].language.as_deref(), Some("spa"));
    assert!(!audio_tracks[1].is_default);
}

#[test]
fn test_probe_result_to_meta_no_video() {
    use api::iptv::live_session::probe_result_to_meta;
    use kob_ffmpeg::probe::{AudioStream, FormatInfo, ProbeResult};

    let result = ProbeResult {
        format: FormatInfo {
            format_name: "mpegts".to_string(),
            duration_seconds: None,
            bit_rate_kbps: None,
            size_bytes: None,
        },
        video_streams: vec![],
        audio_streams: vec![AudioStream {
            index: 0,
            codec: "aac".to_string(),
            channels: 2,
            sample_rate: Some(48000),
            bitrate_kbps: Some(128),
            language: None,
            title: None,
            is_default: true,
        }],
        subtitle_streams: vec![],
        chapters: vec![],
    };

    let err = probe_result_to_meta(&result);
    assert!(err.is_err());
    assert!(
        err.unwrap_err().to_string().contains("no video stream"),
        "should report missing video stream"
    );
}
