//! Playback decision engine.
//!
//! Pure function that determines the optimal playback method for a media file
//! given client capabilities. No I/O — easy to test.

use serde::{Deserialize, Serialize};

/// FFmpeg encoder name kobbler uses for AAC output. We build ffmpeg with
/// `--enable-libfdk-aac --enable-nonfree` (see docker/tools.Dockerfile),
/// because the native FFmpeg `aac` encoder has known psy-acoustic weaknesses
/// below ~320 kbps that produce audible ringing on transients in our default
/// 256 kbps stereo HLS path. libfdk_aac is the reference high-quality AAC LC
/// encoder and produces materially cleaner output at the same bitrate.
///
/// The output codec on the wire is still `aac` (the AAC LC bitstream); only
/// the encoder identity changes. Browsers see standard AAC.
pub const AAC_ENCODER: &str = "libfdk_aac";

/// The image ships with `--enable-libfdk-aac`; a plain distro ffmpeg does not,
/// and asking for a missing encoder produces output with no segments in it.
pub fn aac_encoder() -> &'static str {
    if kob_ffmpeg::detect::encoder_available(AAC_ENCODER) {
        AAC_ENCODER
    } else {
        "aac"
    }
}

/// How the server will deliver content to the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackMethod {
    /// Client can play the file unchanged (MP4 + supported codecs).
    DirectPlay,
    /// Container remux only (MKV → fMP4), all streams copied.
    Remux,
    /// Video copied, audio transcoded (e.g., AC3 → AAC).
    AudioTranscode,
    /// Full video transcode (e.g., HEVC → H.264) + possible audio transcode.
    VideoTranscode,
}

/// What to do with the video stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoMode {
    /// Copy the video stream as-is.
    Copy,
    /// Transcode to the specified codec.
    Transcode { target_codec: String },
}

/// What to do with the audio stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioMode {
    /// Copy the audio stream as-is.
    Copy,
    /// Transcode to the specified codec with target bitrate.
    Transcode {
        target_codec: String,
        bitrate_kbps: i32,
    },
}

/// How to handle subtitles for this playback session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubtitleHandling {
    /// No subtitles selected.
    None,
    /// Extract and convert to WebVTT (for SRT/ASS text-based subs).
    ExtractToVtt { stream_index: i32 },
    /// Serve raw ASS for client-side JASSUB rendering.
    ServeAss { stream_index: i32 },
    /// Burn bitmap subtitle (PGS/DVDSub) into the video via FFmpeg overlay filter.
    /// Forces video transcode.
    BurnIn { stream_index: i32 },
}

impl SubtitleHandling {
    /// Stream index of the subtitle track this decision activates, if any.
    pub fn stream_index(&self) -> Option<i32> {
        match self {
            SubtitleHandling::None => None,
            SubtitleHandling::ExtractToVtt { stream_index }
            | SubtitleHandling::ServeAss { stream_index }
            | SubtitleHandling::BurnIn { stream_index } => Some(*stream_index),
        }
    }
}

/// The complete playback decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackDecision {
    pub method: PlaybackMethod,
    pub video_mode: VideoMode,
    pub audio_mode: AudioMode,
    /// Which audio stream index (from ffprobe) to use.
    pub audio_stream_index: i32,
    pub subtitle_handling: SubtitleHandling,
    #[serde(default)]
    pub deinterlace: bool,
}

/// Metadata about a file needed for the decision.
#[derive(Debug, Clone)]
pub struct FileMeta {
    pub container: String,
    pub video_codec: String,
    pub video_width: u32,
    pub video_height: u32,
    /// Field-coded (PAFF/MBAFF) source.
    pub interlaced: bool,
}

/// Metadata about an audio track.
#[derive(Debug, Clone)]
pub struct AudioTrackMeta {
    pub stream_index: i32,
    pub codec: String,
    pub channels: i32,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_default: bool,
}

/// Metadata about a subtitle track.
#[derive(Debug, Clone)]
pub struct SubtitleTrackMeta {
    pub stream_index: i32,
    pub codec: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_default: bool,
    /// Matroska/MP4 "forced" disposition — a partial track covering only the
    /// foreign-language lines in otherwise-understood dialogue.
    pub is_forced: bool,
}

/// What the client (browser) can natively play.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCapabilities {
    /// Supported video codecs (e.g., ["h264", "hevc"]).
    pub video_codecs: Vec<String>,
    /// Supported audio codecs (e.g., ["aac", "opus"]).
    pub audio_codecs: Vec<String>,
    /// Supported container formats (e.g., ["mp4", "webm"]).
    pub containers: Vec<String>,
    /// `None` is taken as yes; only a client that says no is sent a deinterlaced encode.
    #[serde(default)]
    pub supports_interlaced: Option<bool>,
}

impl Default for ClientCapabilities {
    /// Conservative baseline: H.264 + AAC in MP4. Works everywhere.
    fn default() -> Self {
        Self {
            video_codecs: vec!["h264".to_string()],
            audio_codecs: vec!["aac".to_string()],
            containers: vec!["mp4".to_string()],
            supports_interlaced: None,
        }
    }
}

/// What this viewer asked for, and what they can read.
///
/// `understood_language` is the language whose dialogue needs no subtitles; empty
/// means "assume everything is understood".
#[derive(Debug, Clone, Copy, Default)]
pub struct ViewerPrefs<'a> {
    pub preferred_audio_index: Option<i32>,
    pub preferred_subtitle_index: Option<i32>,
    pub understood_language: &'a str,
    pub subtitle_language: &'a str,
}

/// Determine the optimal playback method.
///
/// Decision waterfall (cheapest to most expensive):
/// 1. Direct Play — container + all codecs supported
/// 2. Remux — codecs fine, just need container change
/// 3. Audio Transcode — video fine, audio needs conversion
/// 4. Video Transcode — video codec not supported
///
/// `server_preferred_codecs` lists codecs the server can encode, ordered
/// by preference (most efficient first, e.g. `["hevc", "h264"]`).
/// When transcoding, the engine picks the first codec from this list
/// that the client also supports.
pub fn decide(
    file: &FileMeta,
    audio_tracks: &[AudioTrackMeta],
    subtitle_tracks: &[SubtitleTrackMeta],
    capabilities: &ClientCapabilities,
    prefs: ViewerPrefs<'_>,
    server_preferred_codecs: &[&str],
) -> PlaybackDecision {
    let deinterlace = file.interlaced && capabilities.supports_interlaced == Some(false);
    let video_supported = !deinterlace
        && capabilities
            .video_codecs
            .iter()
            .any(|c| normalize_codec(c) == normalize_codec(&file.video_codec));

    // Pick audio track: preferred > first supported > default > first
    let audio = select_audio_track(audio_tracks, capabilities, prefs.preferred_audio_index);
    let audio_supported = audio.as_ref().is_some_and(|a| {
        capabilities
            .audio_codecs
            .iter()
            .any(|c| normalize_codec(c) == normalize_codec(&a.codec))
    });

    let container_supported = is_container_compatible(&file.container, &capabilities.containers);

    // Subtitle decision. Auto-selection depends on whether the audio we just
    // picked is a language the viewer understands.
    // An untagged audio track counts as understood: most single-track English
    // rips omit the language tag, and guessing "foreign" there would switch
    // subtitles on for the whole library.
    let audio_understood = prefs.understood_language.is_empty()
        || audio.as_ref().is_none_or(|a| {
            a.language.is_none()
                || language_matches(a.language.as_deref(), prefs.understood_language)
        });
    let subtitle_handling = decide_subtitles(
        subtitle_tracks,
        prefs.preferred_subtitle_index,
        audio_understood,
        prefs.subtitle_language,
    );

    // Audio stream index (fallback to 0 if no tracks)
    let audio_stream_index = audio.as_ref().map(|a| a.stream_index).unwrap_or(0);

    // Decision waterfall
    let mut decision = if video_supported && audio_supported && container_supported {
        PlaybackDecision {
            method: PlaybackMethod::DirectPlay,
            video_mode: VideoMode::Copy,
            audio_mode: AudioMode::Copy,
            audio_stream_index,
            subtitle_handling,
            deinterlace,
        }
    } else if video_supported && audio_supported {
        // Codecs fine, container needs remux (MKV → fMP4)
        PlaybackDecision {
            method: PlaybackMethod::Remux,
            video_mode: VideoMode::Copy,
            audio_mode: AudioMode::Copy,
            audio_stream_index,
            subtitle_handling,
            deinterlace,
        }
    } else if video_supported {
        // Video fine, audio needs transcode
        let audio_mode = AudioMode::Transcode {
            target_codec: AAC_ENCODER.to_string(),
            bitrate_kbps: suggest_audio_bitrate(audio.as_ref()),
        };
        PlaybackDecision {
            method: PlaybackMethod::AudioTranscode,
            video_mode: VideoMode::Copy,
            audio_mode,
            audio_stream_index,
            subtitle_handling,
            deinterlace,
        }
    } else {
        // Video needs transcode (and possibly audio too)
        let audio_mode = if audio_supported {
            AudioMode::Copy
        } else {
            AudioMode::Transcode {
                target_codec: AAC_ENCODER.to_string(),
                bitrate_kbps: suggest_audio_bitrate(audio.as_ref()),
            }
        };
        let target_codec = best_transcode_codec(server_preferred_codecs, capabilities);
        PlaybackDecision {
            method: PlaybackMethod::VideoTranscode,
            video_mode: VideoMode::Transcode { target_codec },
            audio_mode,
            audio_stream_index,
            subtitle_handling,
            deinterlace,
        }
    };

    // Bitmap subtitle burn-in forces full video transcode (overlay filter needs
    // decoded frames). Override the waterfall decision if necessary.
    if matches!(decision.subtitle_handling, SubtitleHandling::BurnIn { .. }) {
        decision.method = PlaybackMethod::VideoTranscode;
        let target_codec = best_transcode_codec(server_preferred_codecs, capabilities);
        decision.video_mode = VideoMode::Transcode { target_codec };
        if matches!(decision.audio_mode, AudioMode::Copy) && !audio_supported {
            decision.audio_mode = AudioMode::Transcode {
                target_codec: AAC_ENCODER.to_string(),
                bitrate_kbps: suggest_audio_bitrate(audio.as_ref()),
            };
        }
    }

    decision
}

/// Select the best audio track based on preferences and client capabilities.
fn select_audio_track(
    tracks: &[AudioTrackMeta],
    capabilities: &ClientCapabilities,
    preferred_index: Option<i32>,
) -> Option<AudioTrackMeta> {
    if tracks.is_empty() {
        return None;
    }

    // If user explicitly chose a track, use it
    if let Some(idx) = preferred_index
        && let Some(t) = tracks.iter().find(|t| t.stream_index == idx)
    {
        return Some(t.clone());
    }

    // Prefer a directly-playable track (avoids transcode)
    let supported_track = tracks.iter().find(|t| {
        capabilities
            .audio_codecs
            .iter()
            .any(|c| normalize_codec(c) == normalize_codec(&t.codec))
    });

    if let Some(t) = supported_track {
        return Some(t.clone());
    }

    // Fall back to default track, then first track
    tracks
        .iter()
        .find(|t| t.is_default)
        .or_else(|| tracks.first())
        .cloned()
}

/// Whether a subtitle codec is bitmap-based (requires burn-in for web playback).
pub fn is_bitmap_codec(codec: &str) -> bool {
    matches!(normalize_codec(codec), "pgs" | "dvdsub")
}

/// Decide how to handle subtitles.
///
/// `audio_understood` says whether the audio track chosen for this session is in
/// a language the viewer reads. When it is, only *forced* tracks auto-select —
/// they carry the handful of foreign lines the film expects you to read. A
/// merely `default`-flagged full track (SDH rips routinely ship one) is left off
/// unless the viewer asks for it, because subtitling dialogue you already
/// understand is not what the disposition flag was meant to request.
fn decide_subtitles(
    tracks: &[SubtitleTrackMeta],
    preferred_index: Option<i32>,
    audio_understood: bool,
    subtitle_language: &str,
) -> SubtitleHandling {
    let track = if let Some(idx) = preferred_index {
        tracks.iter().find(|t| t.stream_index == idx)
    } else {
        // Bitmap subs force an expensive video transcode, so they auto-select
        // only as the viewer's subtitle language under foreign audio.
        let is_text = |t: &SubtitleTrackMeta| !is_bitmap_codec(&t.codec);
        if let Some(forced) = tracks.iter().find(|t| t.is_forced && is_text(t)) {
            Some(forced)
        } else if audio_understood {
            None
        } else {
            find_preferred_subtitle(tracks, subtitle_language)
                .and_then(|idx| tracks.iter().find(|t| t.stream_index == idx))
                .or_else(|| tracks.iter().find(|t| t.is_default && is_text(t)))
        }
    };

    let Some(track) = track else {
        return SubtitleHandling::None;
    };

    match normalize_codec(&track.codec) {
        "srt" | "subrip" | "webvtt" => SubtitleHandling::ExtractToVtt {
            stream_index: track.stream_index,
        },
        "ass" => SubtitleHandling::ServeAss {
            stream_index: track.stream_index,
        },
        "pgs" | "dvdsub" => SubtitleHandling::BurnIn {
            stream_index: track.stream_index,
        },
        // Unknown text formats — try VTT extraction
        _ => SubtitleHandling::ExtractToVtt {
            stream_index: track.stream_index,
        },
    }
}

/// Normalize ffprobe codec names to canonical short names.
///
/// ffprobe reports non-standard names (e.g. "e-ac-3" instead of "eac3",
/// "mpeg2video" instead of "mpeg2"). Clients send the canonical short names,
/// so we normalize before comparison.
pub(crate) fn normalize_codec(codec: &str) -> &str {
    match codec {
        // Audio — ffprobe names and encoder aliases
        "e-ac-3" => "eac3",
        "dca" => "dts",
        "libmp3lame" | "mp3float" | "mp3adu" | "mp3adufloat" | "mp3on4float" | "mp3on4" => "mp3",
        "libfdk_aac" => "aac",
        "libopus" => "opus",
        // PCM variants — ffprobe distinguishes bit depth and endianness
        c if c.starts_with("pcm_") => "pcm",
        // Video — ffprobe names, encoder names, and client aliases
        "mpeg2video" => "mpeg2",
        "mpeg1video" => "mpeg1",
        "avc" | "avc1" | "libx264" | "h264_nvenc" | "h264_qsv" | "h264_amf" | "h264_vaapi"
        | "h264_videotoolbox" | "h264_v4l2m2m" => "h264",
        "h265" | "libx265" | "hevc_nvenc" | "hevc_qsv" | "hevc_amf" | "hevc_vaapi"
        | "hevc_videotoolbox" | "hevc_v4l2m2m" => "hevc",
        "libvpx" => "vp8",
        "libvpx-vp9" => "vp9",
        "libaom-av1" | "libsvtav1" | "librav1e" | "av1_nvenc" | "av1_qsv" | "av1_amf"
        | "av1_vaapi" => "av1",
        // Subtitle — ffprobe long names to canonical short names
        "subrip" => "srt",
        "ssa" => "ass",
        "hdmv_pgs_subtitle" => "pgs",
        "dvd_subtitle" => "dvdsub",
        other => other,
    }
}

/// Check if container format is directly playable.
pub fn is_container_compatible(container: &str, supported: &[String]) -> bool {
    let normalize = |c: &str| {
        match c {
            "matroska,webm" | "matroska" | "mkv" => "mkv",
            "mov,mp4,m4a,3gp,3g2,mj2" | "mp4" | "m4v" | "mov" => "mp4",
            "webm" => "webm",
            // ffprobe says "mpegts"; every client that can play one calls it "ts".
            // Without the alias a tuner stream can never direct play, which is the
            // whole reason the Live TV path exists.
            "mpegts" | "ts" | "mpeg-ts" => "ts",
            other => other,
        }
        .to_string()
    };
    let normalized = normalize(container);
    supported.iter().any(|s| normalize(s) == normalized)
}

/// Pick the best transcode target codec from the intersection of
/// server-preferred codecs and client-supported codecs.
///
/// Falls back to H.264 if there's no overlap (every browser supports H.264).
pub fn best_transcode_codec(
    server_preferred: &[&str],
    capabilities: &ClientCapabilities,
) -> String {
    for codec in server_preferred {
        if capabilities.video_codecs.iter().any(|c| c == codec) {
            return codec.to_string();
        }
    }
    "h264".to_string()
}

/// Suggest AAC bitrate based on source audio.
pub(crate) fn suggest_audio_bitrate(track: Option<&AudioTrackMeta>) -> i32 {
    match track.map(|t| t.channels) {
        Some(c) if c >= 6 => 256, // 5.1 surround → 256kbps AAC
        Some(c) if c > 2 => 192,  // > stereo → 192kbps
        _ => 128,                 // stereo or unknown → 128kbps
    }
}

/// Check if a track's language tag matches a preferred language name.
///
/// Handles ISO 639-1 (2-letter), ISO 639-2/B (3-letter), and full English names.
/// Case-insensitive. Returns false if the track has no language tag.
pub fn language_matches(track_lang: Option<&str>, preferred: &str) -> bool {
    let Some(lang) = track_lang else {
        return false;
    };
    let lang = lang.to_lowercase();
    let pref = preferred.to_lowercase();

    // Exact match (covers full names and codes)
    if lang == pref {
        return true;
    }

    match language_codes(&pref) {
        Some(codes) => codes.contains(&lang.as_str()),
        None => lang.starts_with(&pref) || pref.starts_with(&lang),
    }
}

fn language_codes(pref: &str) -> Option<&'static [&'static str]> {
    Some(match pref {
        "english" | "en" | "eng" => &["en", "eng", "english"],
        "french" | "fr" | "fra" | "fre" => &["fr", "fra", "fre", "french"],
        "german" | "de" | "deu" | "ger" => &["de", "deu", "ger", "german"],
        "spanish" | "es" | "spa" => &["es", "spa", "spanish"],
        "italian" | "it" | "ita" => &["it", "ita", "italian"],
        "russian" | "ru" | "rus" => &["ru", "rus", "russian"],
        "japanese" | "ja" | "jpn" => &["ja", "jpn", "japanese"],
        "korean" | "ko" | "kor" => &["ko", "kor", "korean"],
        "chinese" | "zh" | "zho" | "chi" => &["zh", "zho", "chi", "chinese"],
        "portuguese" | "pt" | "por" => &["pt", "por", "portuguese"],
        "dutch" | "nl" | "nld" | "dut" => &["nl", "nld", "dut", "dutch"],
        "swedish" | "sv" | "swe" => &["sv", "swe", "swedish"],
        "danish" | "da" | "dan" => &["da", "dan", "danish"],
        "norwegian" | "no" | "nor" | "nob" | "nno" => &["no", "nor", "nob", "nno", "norwegian"],
        "finnish" | "fi" | "fin" => &["fi", "fin", "finnish"],
        "polish" | "pl" | "pol" => &["pl", "pol", "polish"],
        "czech" | "cs" | "ces" | "cze" => &["cs", "ces", "cze", "czech"],
        "hungarian" | "hu" | "hun" => &["hu", "hun", "hungarian"],
        "turkish" | "tr" | "tur" => &["tr", "tur", "turkish"],
        "arabic" | "ar" | "ara" => &["ar", "ara", "arabic"],
        "hindi" | "hi" | "hin" => &["hi", "hin", "hindi"],
        "thai" | "th" | "tha" => &["th", "tha", "thai"],
        "vietnamese" | "vi" | "vie" => &["vi", "vie", "vietnamese"],
        "indonesian" | "id" | "ind" => &["id", "ind", "indonesian"],
        "malay" | "ms" | "msa" | "may" => &["ms", "msa", "may", "malay"],
        "greek" | "el" | "ell" | "gre" => &["el", "ell", "gre", "greek"],
        "romanian" | "ro" | "ron" | "rum" => &["ro", "ron", "rum", "romanian"],
        "ukrainian" | "uk" | "ukr" => &["uk", "ukr", "ukrainian"],
        "hebrew" | "he" | "heb" => &["he", "heb", "hebrew"],
        _ => return None,
    })
}

fn is_untagged(language: Option<&str>) -> bool {
    language.is_none_or(|l| {
        l.trim().is_empty()
            || matches!(
                l.trim().to_lowercase().as_str(),
                "und" | "unk" | "mis" | "zxx"
            )
    })
}

/// Muxers often leave the language tag empty and name the language only in the title
/// ("AC3 [English]"), so an untagged track is matched by the words of its title.
fn title_names_language(title: Option<&str>, preferred: &str) -> bool {
    let Some(title) = title else {
        return false;
    };
    let pref = preferred.to_lowercase();
    let codes = language_codes(&pref).unwrap_or(&[]);
    title
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word == pref || (word.len() > 2 && codes.contains(&word)))
}

fn is_commentary(title: Option<&str>) -> bool {
    let Some(title) = title else {
        return false;
    };
    let title = title.to_lowercase();
    [
        "commentary",
        "commentaries",
        "audio description",
        "descriptive",
        "described video",
    ]
    .iter()
    .any(|marker| title.contains(marker))
}

fn speaks(track: &AudioTrackMeta, preferred: &str) -> bool {
    if is_untagged(track.language.as_deref()) {
        title_names_language(track.title.as_deref(), preferred)
    } else {
        language_matches(track.language.as_deref(), preferred)
    }
}

/// Find the first audio track matching a preferred language.
/// Returns the stream index if found.
/// The viewer's own audio language when they sent one, else the server default.
pub fn audio_language_pref<'a>(requested: Option<&'a str>, server_default: &'a str) -> &'a str {
    requested
        .filter(|s| !s.is_empty())
        .unwrap_or(server_default)
}

pub fn find_preferred_audio(tracks: &[AudioTrackMeta], preferred_language: &str) -> Option<i32> {
    if preferred_language.is_empty() {
        return None;
    }
    tracks
        .iter()
        .find(|t| !is_commentary(t.title.as_deref()) && speaks(t, preferred_language))
        .map(|t| t.stream_index)
}

/// The audio languages a stream search should favour, in the form kobload takes them:
/// lowercase English names or ISO 639 codes, most preferred first.
pub fn search_audio_languages(preferred: &str) -> Vec<String> {
    preferred
        .split(',')
        .map(|l| l.trim().to_lowercase())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Find the first subtitle track matching a preferred language.
/// Returns the stream index if found.
pub fn find_preferred_subtitle(
    tracks: &[SubtitleTrackMeta],
    preferred_language: &str,
) -> Option<i32> {
    if preferred_language.is_empty() {
        return None;
    }
    tracks
        .iter()
        .find(|t| language_matches(t.language.as_deref(), preferred_language))
        .map(|t| t.stream_index)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default server codecs for tests (H.264 only, like software-only server).
    const H264_ONLY: &[&str] = &["h264"];
    /// Server with HEVC HW encoding available.
    const HEVC_H264: &[&str] = &["hevc", "h264"];

    fn firefox_caps() -> ClientCapabilities {
        ClientCapabilities {
            video_codecs: vec!["h264".to_string()],
            audio_codecs: vec!["aac".to_string(), "opus".to_string(), "flac".to_string()],
            containers: vec!["mp4".to_string(), "webm".to_string()],
            supports_interlaced: None,
        }
    }

    fn safari_caps() -> ClientCapabilities {
        ClientCapabilities {
            video_codecs: vec!["h264".to_string(), "hevc".to_string()],
            audio_codecs: vec!["aac".to_string()],
            containers: vec!["mp4".to_string()],
            supports_interlaced: None,
        }
    }

    fn chrome_caps() -> ClientCapabilities {
        ClientCapabilities {
            video_codecs: vec!["h264".to_string(), "hevc".to_string()],
            audio_codecs: vec!["aac".to_string(), "opus".to_string(), "flac".to_string()],
            containers: vec!["mp4".to_string(), "webm".to_string()],
            supports_interlaced: None,
        }
    }

    /// kobapp advertises the container as "ts"; ffprobe and the tuner call the
    /// same thing "mpegts". Without the alias no client can ever direct play a
    /// tuner stream, and Live TV silently transcodes for a player that did not
    /// need it.
    #[test]
    fn a_tuner_stream_direct_plays_for_a_client_that_says_ts() {
        let caps = ClientCapabilities {
            video_codecs: vec!["h264".to_string(), "mpeg2video".to_string()],
            audio_codecs: vec!["ac3".to_string(), "aac".to_string()],
            containers: vec!["mp4".to_string(), "mkv".to_string(), "ts".to_string()],
            supports_interlaced: Some(true),
        };
        assert!(is_container_compatible("mpegts", &caps.containers));
        assert!(is_container_compatible("ts", &caps.containers));
        // The client says "mpeg2video" (ffprobe's name), the engine normalises the
        // file's to "mpeg2" — both sides have to be normalised or they never meet.
        assert!(
            caps.video_codecs
                .iter()
                .any(|c| normalize_codec(c) == normalize_codec("mpeg2video"))
        );

        let file = FileMeta {
            container: "mpegts".to_string(),
            video_codec: "mpeg2video".to_string(),
            video_width: 720,
            video_height: 576,
            interlaced: true,
        };
        let audio = [AudioTrackMeta {
            stream_index: 0,
            codec: "ac3".to_string(),
            channels: 2,
            language: None,
            title: None,
            is_default: true,
        }];
        let decision = decide(&file, &audio, &[], &caps, ViewerPrefs::default(), H264_ONLY);
        assert_eq!(decision.method, PlaybackMethod::DirectPlay);
    }

    /// A browser lists neither, so it must still be sent a transcode.
    #[test]
    fn a_tuner_stream_still_transcodes_for_a_browser() {
        assert!(!is_container_compatible(
            "mpegts",
            &firefox_caps().containers
        ));
    }

    #[test]
    fn only_a_client_that_says_it_cannot_show_fields_is_sent_a_deinterlaced_transcode() {
        let cases = [
            (false, None, PlaybackMethod::DirectPlay),
            (false, Some(true), PlaybackMethod::DirectPlay),
            (false, Some(false), PlaybackMethod::DirectPlay),
            (true, None, PlaybackMethod::DirectPlay),
            (true, Some(true), PlaybackMethod::DirectPlay),
            (true, Some(false), PlaybackMethod::VideoTranscode),
        ];
        for (interlaced, supports_interlaced, method) in cases {
            let (mut file, audio) = mp4_h264_aac();
            file.interlaced = interlaced;
            let caps = ClientCapabilities {
                supports_interlaced,
                ..safari_caps()
            };
            let decision = decide(&file, &audio, &[], &caps, ViewerPrefs::default(), H264_ONLY);
            let case = format!("interlaced={interlaced} supports={supports_interlaced:?}");
            assert_eq!(decision.method, method, "{case}");
            assert_eq!(
                decision.deinterlace,
                method == PlaybackMethod::VideoTranscode,
                "{case}"
            );
        }
    }

    fn mp4_h264_aac() -> (FileMeta, Vec<AudioTrackMeta>) {
        (
            FileMeta {
                container: "mp4".to_string(),
                video_codec: "h264".to_string(),
                video_width: 1920,
                video_height: 1080,
                interlaced: false,
            },
            vec![AudioTrackMeta {
                stream_index: 1,
                codec: "aac".to_string(),
                channels: 2,
                language: None,
                title: None,
                is_default: true,
            }],
        )
    }

    fn mkv_h264_aac() -> (FileMeta, Vec<AudioTrackMeta>) {
        (
            FileMeta {
                container: "matroska,webm".to_string(),
                video_codec: "h264".to_string(),
                video_width: 1920,
                video_height: 1080,
                interlaced: false,
            },
            vec![AudioTrackMeta {
                stream_index: 1,
                codec: "aac".to_string(),
                channels: 2,
                language: None,
                title: None,
                is_default: true,
            }],
        )
    }

    fn mkv_h264_ac3() -> (FileMeta, Vec<AudioTrackMeta>) {
        (
            FileMeta {
                container: "matroska,webm".to_string(),
                video_codec: "h264".to_string(),
                video_width: 1920,
                video_height: 1080,
                interlaced: false,
            },
            vec![AudioTrackMeta {
                stream_index: 1,
                codec: "ac3".to_string(),
                channels: 6,
                language: None,
                title: None,
                is_default: true,
            }],
        )
    }

    fn mkv_hevc_dts() -> (FileMeta, Vec<AudioTrackMeta>) {
        (
            FileMeta {
                container: "matroska,webm".to_string(),
                video_codec: "hevc".to_string(),
                video_width: 1920,
                video_height: 1080,
                interlaced: false,
            },
            vec![AudioTrackMeta {
                stream_index: 1,
                codec: "dts".to_string(),
                channels: 6,
                language: None,
                title: None,
                is_default: true,
            }],
        )
    }

    fn mkv_hevc_aac() -> (FileMeta, Vec<AudioTrackMeta>) {
        (
            FileMeta {
                container: "matroska,webm".to_string(),
                video_codec: "hevc".to_string(),
                video_width: 1920,
                video_height: 1080,
                interlaced: false,
            },
            vec![AudioTrackMeta {
                stream_index: 1,
                codec: "aac".to_string(),
                channels: 2,
                language: None,
                title: None,
                is_default: true,
            }],
        )
    }

    #[test]
    fn test_mp4_h264_aac_direct_play() {
        let (file, audio) = mp4_h264_aac();
        let d = decide(
            &file,
            &audio,
            &[],
            &firefox_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            H264_ONLY,
        );
        assert_eq!(d.method, PlaybackMethod::DirectPlay);
        assert_eq!(d.video_mode, VideoMode::Copy);
        assert_eq!(d.audio_mode, AudioMode::Copy);
    }

    #[test]
    fn test_mkv_h264_aac_remux() {
        let (file, audio) = mkv_h264_aac();
        let d = decide(
            &file,
            &audio,
            &[],
            &firefox_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            H264_ONLY,
        );
        assert_eq!(d.method, PlaybackMethod::Remux);
        assert_eq!(d.video_mode, VideoMode::Copy);
        assert_eq!(d.audio_mode, AudioMode::Copy);
    }

    #[test]
    fn test_mkv_h264_ac3_audio_transcode() {
        let (file, audio) = mkv_h264_ac3();
        let d = decide(
            &file,
            &audio,
            &[],
            &firefox_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            H264_ONLY,
        );
        assert_eq!(d.method, PlaybackMethod::AudioTranscode);
        assert_eq!(d.video_mode, VideoMode::Copy);
        assert!(matches!(d.audio_mode, AudioMode::Transcode { .. }));
    }

    #[test]
    fn test_mkv_hevc_dts_video_transcode_firefox() {
        let (file, audio) = mkv_hevc_dts();
        let d = decide(
            &file,
            &audio,
            &[],
            &firefox_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            H264_ONLY,
        );
        assert_eq!(d.method, PlaybackMethod::VideoTranscode);
        assert!(matches!(d.video_mode, VideoMode::Transcode { .. }));
        assert!(matches!(d.audio_mode, AudioMode::Transcode { .. }));
    }

    #[test]
    fn test_mkv_hevc_aac_remux_on_safari() {
        let (file, audio) = mkv_hevc_aac();
        let d = decide(
            &file,
            &audio,
            &[],
            &safari_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            H264_ONLY,
        );
        assert_eq!(d.method, PlaybackMethod::Remux);
        assert_eq!(d.video_mode, VideoMode::Copy);
        assert_eq!(d.audio_mode, AudioMode::Copy);
    }

    #[test]
    fn test_mkv_hevc_dts_on_chrome_audio_transcode_only() {
        let (file, audio) = mkv_hevc_dts();
        let d = decide(
            &file,
            &audio,
            &[],
            &chrome_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            H264_ONLY,
        );
        // Chrome supports HEVC, so only audio needs transcode
        assert_eq!(d.method, PlaybackMethod::AudioTranscode);
        assert_eq!(d.video_mode, VideoMode::Copy);
        assert!(matches!(d.audio_mode, AudioMode::Transcode { .. }));
    }

    #[test]
    fn test_prefers_supported_audio_track() {
        let file = FileMeta {
            container: "matroska,webm".to_string(),
            video_codec: "h264".to_string(),
            video_width: 1920,
            video_height: 1080,
            interlaced: false,
        };
        let audio = vec![
            AudioTrackMeta {
                stream_index: 1,
                codec: "dts".to_string(),
                channels: 6,
                language: None,
                title: None,
                is_default: true,
            },
            AudioTrackMeta {
                stream_index: 2,
                codec: "aac".to_string(),
                channels: 2,
                language: None,
                title: None,
                is_default: false,
            },
        ];
        let d = decide(
            &file,
            &audio,
            &[],
            &firefox_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            H264_ONLY,
        );
        // Should pick AAC track to avoid audio transcode → Remux instead of AudioTranscode
        assert_eq!(d.method, PlaybackMethod::Remux);
        assert_eq!(d.audio_stream_index, 2);
    }

    #[test]
    fn test_preferred_audio_override() {
        let file = FileMeta {
            container: "matroska,webm".to_string(),
            video_codec: "h264".to_string(),
            video_width: 1920,
            video_height: 1080,
            interlaced: false,
        };
        let audio = vec![
            AudioTrackMeta {
                stream_index: 1,
                codec: "dts".to_string(),
                channels: 6,
                language: None,
                title: None,
                is_default: true,
            },
            AudioTrackMeta {
                stream_index: 2,
                codec: "aac".to_string(),
                channels: 2,
                language: None,
                title: None,
                is_default: false,
            },
        ];
        // User explicitly wants the DTS track
        let d = decide(
            &file,
            &audio,
            &[],
            &firefox_caps(),
            ViewerPrefs {
                preferred_audio_index: Some(1),
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            H264_ONLY,
        );
        assert_eq!(d.method, PlaybackMethod::AudioTranscode);
        assert_eq!(d.audio_stream_index, 1);
    }

    #[test]
    fn test_subtitle_srt_extracted_to_vtt() {
        let (file, audio) = mp4_h264_aac();
        let subs = vec![SubtitleTrackMeta {
            stream_index: 3,
            codec: "srt".to_string(),
            language: None,
            title: None,
            is_default: true,
            is_forced: true,
        }];
        let d = decide(
            &file,
            &audio,
            &subs,
            &firefox_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            H264_ONLY,
        );
        assert_eq!(
            d.subtitle_handling,
            SubtitleHandling::ExtractToVtt { stream_index: 3 }
        );
    }

    #[test]
    fn test_subtitle_subrip_extracted_to_vtt() {
        // ffprobe reports "subrip" for SRT files
        let (file, audio) = mp4_h264_aac();
        let subs = vec![SubtitleTrackMeta {
            stream_index: 3,
            codec: "subrip".to_string(),
            language: None,
            title: None,
            is_default: true,
            is_forced: true,
        }];
        let d = decide(
            &file,
            &audio,
            &subs,
            &firefox_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            H264_ONLY,
        );
        assert_eq!(
            d.subtitle_handling,
            SubtitleHandling::ExtractToVtt { stream_index: 3 }
        );
    }

    #[test]
    fn test_subtitle_ass_served_raw() {
        let (file, audio) = mp4_h264_aac();
        let subs = vec![SubtitleTrackMeta {
            stream_index: 3,
            codec: "ass".to_string(),
            language: None,
            title: None,
            is_default: true,
            is_forced: true,
        }];
        let d = decide(
            &file,
            &audio,
            &subs,
            &firefox_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            H264_ONLY,
        );
        assert_eq!(
            d.subtitle_handling,
            SubtitleHandling::ServeAss { stream_index: 3 }
        );
    }

    #[test]
    fn test_subtitle_pgs_not_auto_selected() {
        // PGS with is_default but no explicit preference → skipped (forces transcode)
        let (file, audio) = mp4_h264_aac();
        let subs = vec![SubtitleTrackMeta {
            stream_index: 3,
            codec: "pgs".to_string(),
            language: None,
            title: None,
            is_default: true,
            is_forced: false,
        }];
        let d = decide(
            &file,
            &audio,
            &subs,
            &firefox_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            H264_ONLY,
        );
        assert_eq!(d.subtitle_handling, SubtitleHandling::None);
        // Should remain DirectPlay since no burn-in triggered
        assert_eq!(d.method, PlaybackMethod::DirectPlay);
    }

    #[test]
    fn test_subtitle_pgs_burn_in_when_explicit() {
        // PGS explicitly selected → BurnIn + forces VideoTranscode
        let (file, audio) = mp4_h264_aac();
        let subs = vec![SubtitleTrackMeta {
            stream_index: 3,
            codec: "pgs".to_string(),
            language: None,
            title: None,
            is_default: false,
            is_forced: false,
        }];
        let d = decide(
            &file,
            &audio,
            &subs,
            &firefox_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: Some(3),
                understood_language: "english",
                subtitle_language: "",
            },
            H264_ONLY,
        );
        assert!(matches!(
            d.subtitle_handling,
            SubtitleHandling::BurnIn { stream_index: 3 }
        ));
        assert_eq!(d.method, PlaybackMethod::VideoTranscode);
        assert!(matches!(d.video_mode, VideoMode::Transcode { .. }));
    }

    #[test]
    fn test_subtitle_dvdsub_burn_in() {
        let (file, audio) = mp4_h264_aac();
        let subs = vec![SubtitleTrackMeta {
            stream_index: 4,
            codec: "dvd_subtitle".to_string(),
            language: None,
            title: None,
            is_default: false,
            is_forced: false,
        }];
        let d = decide(
            &file,
            &audio,
            &subs,
            &firefox_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: Some(4),
                understood_language: "english",
                subtitle_language: "",
            },
            H264_ONLY,
        );
        assert!(matches!(
            d.subtitle_handling,
            SubtitleHandling::BurnIn { stream_index: 4 }
        ));
    }

    #[test]
    fn test_no_subtitle_when_none_default() {
        let (file, audio) = mp4_h264_aac();
        let subs = vec![SubtitleTrackMeta {
            stream_index: 3,
            codec: "srt".to_string(),
            language: None,
            title: None,
            is_default: false,
            is_forced: false,
        }];
        let d = decide(
            &file,
            &audio,
            &subs,
            &firefox_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            H264_ONLY,
        );
        assert_eq!(d.subtitle_handling, SubtitleHandling::None);
    }

    #[test]
    fn test_default_sdh_track_not_auto_selected_for_understood_audio() {
        // Real-world case: a rip flags its full "English [SDH]" track as default
        // over English audio. Nothing is foreign here, so it stays off.
        let (file, mut audio) = mp4_h264_aac();
        audio[0].language = Some("eng".to_string());
        let subs = vec![SubtitleTrackMeta {
            stream_index: 2,
            codec: "subrip".to_string(),
            language: Some("eng".to_string()),
            title: Some("English [SDH]".to_string()),
            is_default: true,
            is_forced: false,
        }];
        let d = decide(
            &file,
            &audio,
            &subs,
            &firefox_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            H264_ONLY,
        );
        assert_eq!(d.subtitle_handling, SubtitleHandling::None);
    }

    #[test]
    fn test_default_track_auto_selected_for_foreign_audio() {
        // Same file with Japanese audio — now the default track is the only way
        // to follow the dialogue, so it comes on.
        let (file, mut audio) = mp4_h264_aac();
        audio[0].language = Some("jpn".to_string());
        let subs = vec![SubtitleTrackMeta {
            stream_index: 2,
            codec: "subrip".to_string(),
            language: Some("eng".to_string()),
            title: None,
            is_default: true,
            is_forced: false,
        }];
        let d = decide(
            &file,
            &audio,
            &subs,
            &firefox_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            H264_ONLY,
        );
        assert_eq!(
            d.subtitle_handling,
            SubtitleHandling::ExtractToVtt { stream_index: 2 }
        );
    }

    #[test]
    fn test_forced_track_auto_selected_over_understood_audio() {
        // Forced tracks are exactly the foreign lines you are meant to read.
        let (file, mut audio) = mp4_h264_aac();
        audio[0].language = Some("eng".to_string());
        let subs = vec![SubtitleTrackMeta {
            stream_index: 2,
            codec: "subrip".to_string(),
            language: Some("eng".to_string()),
            title: Some("Forced".to_string()),
            is_default: false,
            is_forced: true,
        }];
        let d = decide(
            &file,
            &audio,
            &subs,
            &firefox_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            H264_ONLY,
        );
        assert_eq!(
            d.subtitle_handling,
            SubtitleHandling::ExtractToVtt { stream_index: 2 }
        );
    }

    #[test]
    fn test_untagged_audio_treated_as_understood() {
        // Most single-track English rips omit the language tag; guessing
        // "foreign" there would switch subtitles on across the library.
        let (file, audio) = mp4_h264_aac();
        assert!(audio[0].language.is_none());
        let subs = vec![SubtitleTrackMeta {
            stream_index: 2,
            codec: "subrip".to_string(),
            language: Some("eng".to_string()),
            title: None,
            is_default: true,
            is_forced: false,
        }];
        let d = decide(
            &file,
            &audio,
            &subs,
            &firefox_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            H264_ONLY,
        );
        assert_eq!(d.subtitle_handling, SubtitleHandling::None);
    }

    #[test]
    fn test_hevc_transcode_when_server_and_client_support() {
        // HEVC source unsupported by Firefox, but server has HEVC HW encoding
        // and Chrome supports HEVC → should transcode to HEVC (not H.264)
        let (file, audio) = mkv_hevc_dts();
        let d = decide(
            &file,
            &audio,
            &[],
            &chrome_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            HEVC_H264,
        );
        // Chrome supports HEVC so no video transcode needed (copy)
        assert_eq!(d.video_mode, VideoMode::Copy);
    }

    #[test]
    fn test_hevc_transcode_target_for_unsupported_source() {
        // VP9 source, Chrome supports HEVC, server can encode HEVC
        let file = FileMeta {
            container: "matroska,webm".to_string(),
            video_codec: "vp9".to_string(),
            video_width: 1920,
            video_height: 1080,
            interlaced: false,
        };
        let audio = vec![AudioTrackMeta {
            stream_index: 1,
            codec: "aac".to_string(),
            channels: 2,
            language: None,
            title: None,
            is_default: true,
        }];
        // Chrome says it supports h264+hevc, server prefers hevc
        let d = decide(
            &file,
            &audio,
            &[],
            &chrome_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            HEVC_H264,
        );
        assert_eq!(d.method, PlaybackMethod::VideoTranscode);
        assert_eq!(
            d.video_mode,
            VideoMode::Transcode {
                target_codec: "hevc".to_string()
            }
        );
    }

    #[test]
    fn test_falls_back_to_h264_when_client_lacks_hevc() {
        // Server has HEVC but Firefox doesn't support it → fall back to H.264
        let file = FileMeta {
            container: "matroska,webm".to_string(),
            video_codec: "vp9".to_string(),
            video_width: 1920,
            video_height: 1080,
            interlaced: false,
        };
        let audio = vec![AudioTrackMeta {
            stream_index: 1,
            codec: "aac".to_string(),
            channels: 2,
            language: None,
            title: None,
            is_default: true,
        }];
        let d = decide(
            &file,
            &audio,
            &[],
            &firefox_caps(),
            ViewerPrefs {
                preferred_audio_index: None,
                preferred_subtitle_index: None,
                understood_language: "english",
                subtitle_language: "",
            },
            HEVC_H264,
        );
        assert_eq!(
            d.video_mode,
            VideoMode::Transcode {
                target_codec: "h264".to_string()
            }
        );
    }

    #[test]
    fn test_audio_bitrate_suggestions() {
        assert_eq!(suggest_audio_bitrate(None), 128);
        assert_eq!(
            suggest_audio_bitrate(Some(&AudioTrackMeta {
                stream_index: 0,
                codec: "ac3".to_string(),
                channels: 6,
                language: None,
                title: None,
                is_default: true,
            })),
            256
        );
        assert_eq!(
            suggest_audio_bitrate(Some(&AudioTrackMeta {
                stream_index: 0,
                codec: "aac".to_string(),
                channels: 2,
                language: None,
                title: None,
                is_default: true,
            })),
            128
        );
    }

    #[test]
    fn test_normalize_codec_audio() {
        assert_eq!(normalize_codec("e-ac-3"), "eac3");
        assert_eq!(normalize_codec("pcm_s16le"), "pcm");
        assert_eq!(normalize_codec("pcm_s24le"), "pcm");
        assert_eq!(normalize_codec("pcm_f32le"), "pcm");
        assert_eq!(normalize_codec("pcm_u8"), "pcm");
        assert_eq!(normalize_codec("dca"), "dts");
        assert_eq!(normalize_codec("mp3float"), "mp3");
        assert_eq!(normalize_codec("libmp3lame"), "mp3");
        assert_eq!(normalize_codec("libfdk_aac"), "aac");
        assert_eq!(normalize_codec("libopus"), "opus");
        assert_eq!(normalize_codec("aac"), "aac"); // passthrough
        assert_eq!(normalize_codec("ac3"), "ac3"); // passthrough
        assert_eq!(normalize_codec("vorbis"), "vorbis"); // passthrough (not ogg!)
    }

    #[test]
    fn test_normalize_codec_video() {
        assert_eq!(normalize_codec("mpeg2video"), "mpeg2");
        assert_eq!(normalize_codec("mpeg1video"), "mpeg1");
        assert_eq!(normalize_codec("libx264"), "h264");
        assert_eq!(normalize_codec("h264_nvenc"), "h264");
        assert_eq!(normalize_codec("avc"), "h264");
        assert_eq!(normalize_codec("avc1"), "h264");
        assert_eq!(normalize_codec("libx265"), "hevc");
        assert_eq!(normalize_codec("h265"), "hevc");
        assert_eq!(normalize_codec("hevc_nvenc"), "hevc");
        assert_eq!(normalize_codec("libvpx"), "vp8");
        assert_eq!(normalize_codec("libvpx-vp9"), "vp9");
        assert_eq!(normalize_codec("libaom-av1"), "av1");
        assert_eq!(normalize_codec("av1_qsv"), "av1");
        assert_eq!(normalize_codec("h264"), "h264"); // passthrough
    }

    #[test]
    fn test_normalize_codec_subtitle() {
        assert_eq!(normalize_codec("subrip"), "srt");
        assert_eq!(normalize_codec("ssa"), "ass");
        assert_eq!(normalize_codec("hdmv_pgs_subtitle"), "pgs");
        assert_eq!(normalize_codec("dvd_subtitle"), "dvdsub");
        assert_eq!(normalize_codec("srt"), "srt"); // passthrough
        assert_eq!(normalize_codec("ass"), "ass"); // passthrough
    }

    #[test]
    fn test_ffprobe_eac3_recognized_as_supported() {
        // File has e-ac-3 from ffprobe, client declares eac3 support
        let file = FileMeta {
            container: "mp4".to_string(),
            video_codec: "h264".to_string(),
            video_width: 1920,
            video_height: 1080,
            interlaced: false,
        };
        let audio = vec![AudioTrackMeta {
            stream_index: 1,
            codec: "e-ac-3".to_string(),
            channels: 6,
            language: None,
            title: None,
            is_default: true,
        }];
        let caps = ClientCapabilities {
            video_codecs: vec!["h264".to_string()],
            audio_codecs: vec!["aac".to_string(), "eac3".to_string()],
            containers: vec!["mp4".to_string()],
            supports_interlaced: None,
        };
        let d = decide(
            &file,
            &audio,
            &[],
            &caps,
            ViewerPrefs {
                understood_language: "english",
                ..Default::default()
            },
            H264_ONLY,
        );
        assert_eq!(d.method, PlaybackMethod::DirectPlay);
        assert_eq!(d.audio_mode, AudioMode::Copy);
    }

    #[test]
    fn test_ffprobe_mpeg2video_recognized() {
        // File has mpeg2video from ffprobe, client declares mpeg2 support
        let file = FileMeta {
            container: "mp4".to_string(),
            video_codec: "mpeg2video".to_string(),
            video_width: 720,
            video_height: 480,
            interlaced: false,
        };
        let audio = vec![AudioTrackMeta {
            stream_index: 1,
            codec: "aac".to_string(),
            channels: 2,
            language: None,
            title: None,
            is_default: true,
        }];
        let caps = ClientCapabilities {
            video_codecs: vec!["h264".to_string(), "mpeg2".to_string()],
            audio_codecs: vec!["aac".to_string()],
            containers: vec!["mp4".to_string()],
            supports_interlaced: None,
        };
        let d = decide(
            &file,
            &audio,
            &[],
            &caps,
            ViewerPrefs {
                understood_language: "english",
                ..Default::default()
            },
            H264_ONLY,
        );
        assert_eq!(d.method, PlaybackMethod::DirectPlay);
        assert_eq!(d.video_mode, VideoMode::Copy);
    }

    #[test]
    fn test_ffprobe_pcm_track_skipped_for_supported() {
        // PCM track (from ffprobe as pcm_s16le) + AAC track; client supports pcm + aac
        // Should prefer the first supported track
        let file = FileMeta {
            container: "matroska,webm".to_string(),
            video_codec: "h264".to_string(),
            video_width: 1920,
            video_height: 1080,
            interlaced: false,
        };
        let audio = vec![
            AudioTrackMeta {
                stream_index: 1,
                codec: "pcm_s24le".to_string(),
                channels: 2,
                language: None,
                title: None,
                is_default: true,
            },
            AudioTrackMeta {
                stream_index: 2,
                codec: "aac".to_string(),
                channels: 2,
                language: None,
                title: None,
                is_default: false,
            },
        ];
        let caps = ClientCapabilities {
            video_codecs: vec!["h264".to_string()],
            audio_codecs: vec!["aac".to_string()],
            containers: vec!["mkv".to_string()],
            supports_interlaced: None,
        };
        let d = decide(
            &file,
            &audio,
            &[],
            &caps,
            ViewerPrefs {
                understood_language: "english",
                ..Default::default()
            },
            H264_ONLY,
        );
        // Should pick AAC track (stream 2) since pcm isn't in client caps
        assert_eq!(d.audio_stream_index, 2);
        assert_eq!(d.method, PlaybackMethod::DirectPlay);
    }

    // =========================================================================
    // language_matches tests
    // =========================================================================

    #[test]
    fn language_matches_iso_codes() {
        // Full name matches short codes
        assert!(language_matches(Some("en"), "english"));
        assert!(language_matches(Some("eng"), "english"));
        assert!(language_matches(Some("english"), "english"));

        // Short code as preferred
        assert!(language_matches(Some("eng"), "en"));
        assert!(language_matches(Some("english"), "eng"));
    }

    #[test]
    fn language_matches_case_insensitive() {
        assert!(language_matches(Some("EN"), "english"));
        assert!(language_matches(Some("English"), "english"));
        assert!(language_matches(Some("ENG"), "English"));
    }

    #[test]
    fn language_matches_non_english() {
        assert!(language_matches(Some("fr"), "french"));
        assert!(language_matches(Some("fra"), "french"));
        assert!(language_matches(Some("fre"), "french"));
        assert!(language_matches(Some("ja"), "japanese"));
        assert!(language_matches(Some("jpn"), "japanese"));
        assert!(language_matches(Some("de"), "german"));
        assert!(language_matches(Some("deu"), "german"));
        assert!(language_matches(Some("ger"), "german"));
    }

    #[test]
    fn language_matches_no_match() {
        assert!(!language_matches(Some("fr"), "english"));
        assert!(!language_matches(Some("japanese"), "english"));
        assert!(!language_matches(None, "english"));
    }

    #[test]
    fn find_preferred_audio_picks_first_match() {
        let tracks = vec![
            AudioTrackMeta {
                stream_index: 1,
                codec: "aac".to_string(),
                channels: 2,
                language: Some("fra".to_string()),
                title: None,
                is_default: true,
            },
            AudioTrackMeta {
                stream_index: 2,
                codec: "aac".to_string(),
                channels: 6,
                language: Some("eng".to_string()),
                title: None,
                is_default: false,
            },
            AudioTrackMeta {
                stream_index: 3,
                codec: "aac".to_string(),
                channels: 2,
                language: Some("eng".to_string()),
                title: None,
                is_default: false,
            },
        ];
        // Should pick the first English track (stream 2)
        assert_eq!(find_preferred_audio(&tracks, "english"), Some(2));
        // French should pick stream 1
        assert_eq!(find_preferred_audio(&tracks, "french"), Some(1));
        // Japanese should not match
        assert_eq!(find_preferred_audio(&tracks, "japanese"), None);
        // Empty preferred returns None
        assert_eq!(find_preferred_audio(&tracks, ""), None);
    }

    fn audio(
        stream_index: i32,
        language: Option<&str>,
        title: Option<&str>,
        is_default: bool,
    ) -> AudioTrackMeta {
        AudioTrackMeta {
            stream_index,
            codec: "ac3".to_string(),
            channels: 6,
            language: language.map(str::to_string),
            title: title.map(str::to_string),
            is_default,
        }
    }

    #[test]
    fn find_preferred_audio_reads_language_from_untagged_title() {
        let tracks = vec![
            audio(1, Some("ita"), Some("AC3 [Italiano]"), true),
            audio(2, None, Some("AC3 [English]"), false),
        ];
        assert_eq!(find_preferred_audio(&tracks, "english"), Some(2));

        let undetermined = vec![
            audio(1, Some("ita"), None, true),
            audio(2, Some("und"), Some("English 5.1"), false),
        ];
        assert_eq!(find_preferred_audio(&undetermined, "eng"), Some(2));
    }

    #[test]
    fn find_preferred_audio_trusts_a_real_tag_over_the_title() {
        let tracks = vec![
            audio(1, Some("ita"), Some("English dub removed"), true),
            audio(2, Some("eng"), None, false),
        ];
        assert_eq!(find_preferred_audio(&tracks, "english"), Some(2));
    }

    #[test]
    fn find_preferred_audio_skips_commentary_and_description() {
        let tracks = vec![
            audio(1, Some("eng"), Some("Director's Commentary"), false),
            audio(2, Some("eng"), Some("Audio Description"), false),
            audio(3, Some("eng"), Some("English DTS-HD MA 5.1"), false),
        ];
        assert_eq!(find_preferred_audio(&tracks, "english"), Some(3));

        let only_commentary = vec![
            audio(1, Some("ita"), None, true),
            audio(2, Some("eng"), Some("Commentary with the cast"), false),
        ];
        assert_eq!(find_preferred_audio(&only_commentary, "english"), None);
    }
}
