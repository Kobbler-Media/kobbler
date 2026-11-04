//! Subtitle processing for web playback.
//!
//! Handles SRT → WebVTT conversion, subtitle file serving, and cache paths.

use std::path::PathBuf;
use uuid::Uuid;

/// Convert SRT subtitle content to WebVTT format.
///
/// Transforms:
/// - Adds `WEBVTT` header
/// - Replaces `,` with `.` in timestamps (SRT uses comma, VTT uses period)
/// - Strips UTF-8 BOM if present
pub fn srt_to_vtt(srt: &str) -> String {
    let srt = srt.strip_prefix('\u{feff}').unwrap_or(srt);

    let mut output = String::with_capacity(srt.len() + 16);
    output.push_str("WEBVTT\n\n");

    for line in srt.lines() {
        // SRT timestamp line: "00:01:23,456 --> 00:01:25,789"
        if line.contains(" --> ") {
            output.push_str(&line.replace(',', "."));
        } else {
            output.push_str(line);
        }
        output.push('\n');
    }

    output
}

/// Move every cue `offset` seconds earlier, for a player whose zero is that far into the
/// title. Cues that end before the new zero are dropped.
pub fn shift_vtt(vtt: &str, offset: f64) -> String {
    if offset <= 0.0 {
        return vtt.to_string();
    }
    let mut out = String::with_capacity(vtt.len());
    for block in vtt.split("\n\n") {
        let Some(timing) = block.lines().find(|l| l.contains(" --> ")) else {
            push_block(&mut out, block);
            continue;
        };
        let (start, rest) = timing.split_once(" --> ").unwrap_or_default();
        let (end, settings) = rest.split_once(' ').unwrap_or((rest, ""));
        let (Some(start), Some(end)) = (parse_vtt_time(start), parse_vtt_time(end)) else {
            push_block(&mut out, block);
            continue;
        };
        if end - offset <= 0.0 {
            continue;
        }
        let mut shifted = format!(
            "{} --> {}",
            format_vtt_time((start - offset).max(0.0)),
            format_vtt_time(end - offset)
        );
        if !settings.is_empty() {
            shifted.push(' ');
            shifted.push_str(settings);
        }
        push_block(&mut out, &block.replacen(timing, &shifted, 1));
    }
    out
}

fn push_block(out: &mut String, block: &str) {
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(block);
}

fn parse_vtt_time(t: &str) -> Option<f64> {
    let mut secs = 0.0;
    for part in t.trim().split(':') {
        secs = secs * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(secs)
}

fn format_vtt_time(secs: f64) -> String {
    let ms = (secs * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

/// Deterministic cache path for a pre-extracted subtitle track.
///
/// Used by both the media_probe job (writes) and playback session (reads).
pub fn cache_path(storage_root: &str, file_id: Uuid, track_index: i32, codec: &str) -> PathBuf {
    let ext = subtitle_extension(codec).unwrap_or("srt");
    PathBuf::from(storage_root)
        .join("cache")
        .join("subtitles")
        .join(file_id.to_string())
        .join(format!("{track_index}.{ext}"))
}

/// Whether the codec is a bitmap-based subtitle format (PGS, VOBSUB).
///
/// Bitmap subtitles cannot be extracted to text-based formats (SRT/VTT) and
/// are not renderable by web browsers — they need OCR to convert.
pub fn is_bitmap_subtitle(codec: &str) -> bool {
    matches!(
        codec,
        "pgs" | "hdmv_pgs_subtitle" | "dvdsub" | "dvd_subtitle" | "dvb_subtitle" | "dvb_teletext"
    )
}

/// Determine the appropriate subtitle file extension for a codec.
pub fn subtitle_extension(codec: &str) -> Option<&str> {
    Some(match codec {
        "srt" | "subrip" => "srt",
        "ass" | "ssa" => "ass",
        "webvtt" => "vtt",
        "mov_text" => "srt",
        "pgs" | "hdmv_pgs_subtitle" => "sup",
        "dvdsub" | "dvd_subtitle" => "sub",
        "dvb_subtitle" | "dvb_teletext" => return None,
        other => {
            tracing::warn!(codec = other, "Unknown subtitle codec, skipping extraction");
            return None;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cues_move_onto_a_timeline_that_begins_part_way_in() {
        let vtt = "WEBVTT\n\n1\n00:00:05.000 --> 00:00:08.000\nGone\n\n2\n00:23:01.000 --> 00:23:04.500 line:90%\nStraddles\n\n3\n01:00:00.250 --> 01:00:02.000\nLater\n";
        assert_eq!(
            shift_vtt(vtt, 1382.0),
            "WEBVTT\n\n2\n00:00:00.000 --> 00:00:02.500 line:90%\nStraddles\n\n3\n00:36:58.250 --> 00:37:00.000\nLater\n"
        );
        assert_eq!(shift_vtt(vtt, 0.0), vtt);
    }

    #[test]
    fn test_srt_to_vtt_basic() {
        let srt = "1\n00:00:01,000 --> 00:00:04,000\nHello, world!\n\n2\n00:00:05,500 --> 00:00:08,200\nSecond subtitle.\n";
        let vtt = srt_to_vtt(srt);

        assert!(vtt.starts_with("WEBVTT\n\n"));
        assert!(vtt.contains("00:00:01.000 --> 00:00:04.000"));
        assert!(vtt.contains("00:00:05.500 --> 00:00:08.200"));
        assert!(vtt.contains("Hello, world!"));
        // Timestamps should use '.' not ','
        assert!(!vtt.contains("00:00:01,"));
        assert!(!vtt.contains("00:00:04,"));
    }

    #[test]
    fn test_srt_to_vtt_strips_bom() {
        let srt = "\u{feff}1\n00:00:01,000 --> 00:00:02,000\nBOM test\n";
        let vtt = srt_to_vtt(srt);

        assert!(vtt.starts_with("WEBVTT"));
        assert!(!vtt.contains('\u{feff}'));
    }

    #[test]
    fn test_srt_to_vtt_preserves_multiline() {
        let srt = "1\n00:00:01,000 --> 00:00:04,000\nLine one\nLine two\n\n";
        let vtt = srt_to_vtt(srt);

        assert!(vtt.contains("Line one\nLine two"));
    }

    #[test]
    fn test_srt_to_vtt_empty() {
        let vtt = srt_to_vtt("");
        assert_eq!(vtt, "WEBVTT\n\n");
    }

    #[test]
    fn test_is_bitmap_subtitle() {
        assert!(is_bitmap_subtitle("pgs"));
        assert!(is_bitmap_subtitle("hdmv_pgs_subtitle"));
        assert!(is_bitmap_subtitle("dvdsub"));
        assert!(is_bitmap_subtitle("dvd_subtitle"));
        assert!(is_bitmap_subtitle("dvb_subtitle"));
        assert!(is_bitmap_subtitle("dvb_teletext"));
        assert!(!is_bitmap_subtitle("srt"));
        assert!(!is_bitmap_subtitle("subrip"));
        assert!(!is_bitmap_subtitle("ass"));
        assert!(!is_bitmap_subtitle("webvtt"));
        assert!(!is_bitmap_subtitle("mov_text"));
    }

    #[test]
    fn test_subtitle_extensions() {
        assert_eq!(subtitle_extension("srt"), Some("srt"));
        assert_eq!(subtitle_extension("subrip"), Some("srt"));
        assert_eq!(subtitle_extension("ass"), Some("ass"));
        assert_eq!(subtitle_extension("ssa"), Some("ass"));
        assert_eq!(subtitle_extension("webvtt"), Some("vtt"));
        assert_eq!(subtitle_extension("mov_text"), Some("srt"));
        assert_eq!(subtitle_extension("pgs"), Some("sup"));
        assert_eq!(subtitle_extension("hdmv_pgs_subtitle"), Some("sup"));
        assert_eq!(subtitle_extension("dvdsub"), Some("sub"));
        assert_eq!(subtitle_extension("dvd_subtitle"), Some("sub"));
        // Bitmap DVB formats return None (not extractable to text)
        assert_eq!(subtitle_extension("dvb_subtitle"), None);
        assert_eq!(subtitle_extension("dvb_teletext"), None);
        // Unknown codecs return None
        assert_eq!(subtitle_extension("xyzzy"), None);
    }
}
