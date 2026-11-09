//! What a subtitle track's own text says about it when its tags do not: the language it is
//! in, whether it is a forced track, and whether it is written for the hard of hearing.
//! Read from the opening stretch of a file only (see [`WINDOW_SECS`]).

use kob_ffmpeg::subtitles::{TextCue, TextTrackCues, list_subtitle_tracks, read_text_cues};
use std::path::PathBuf;

/// Twenty minutes holds a few hundred lines of a full track and next to none of a forced one.
pub const WINDOW_SECS: f64 = 1200.0;

const MIN_LANGUAGE_CHARS: usize = 150;
const FORCED_SHARE_OF_FULL: f64 = 0.25;
const FULL_TRACK_MIN_CUES: usize = 40;
const LONE_FORCED_CUES_PER_MINUTE: f64 = 0.5;
const SDH_SHARE: f64 = 0.1;

#[derive(Debug, Clone, PartialEq)]
pub struct TrackInsight {
    pub stream_index: i32,
    /// ISO 639-2/B, the form container tags use.
    pub language: Option<String>,
    pub forced: bool,
    pub sdh: bool,
}

/// [window_secs] is how much of the file was read, which can be less than [`WINDOW_SECS`]
/// for a short file.
pub fn analyse(tracks: &[TextTrackCues], window_secs: f64) -> Vec<TrackInsight> {
    let languages: Vec<Option<String>> = tracks.iter().map(|t| detect_language(&t.cues)).collect();
    tracks
        .iter()
        .zip(&languages)
        .map(|(track, language)| {
            let count = track.cues.len();
            let fullest = tracks
                .iter()
                .zip(&languages)
                .filter(|(_, other)| language.is_none() || *other == language)
                .map(|(t, _)| t.cues.len())
                .max()
                .unwrap_or(0);
            let beside_full = fullest >= FULL_TRACK_MIN_CUES
                && (count as f64) <= fullest as f64 * FORCED_SHARE_OF_FULL;
            let per_minute = count as f64 / (window_secs / 60.0).max(1.0);
            let barely_speaks = per_minute < LONE_FORCED_CUES_PER_MINUTE;
            TrackInsight {
                stream_index: track.stream_index,
                language: language.clone(),
                forced: count > 0 && (beside_full || barely_speaks),
                sdh: count > 0
                    && track.cues.iter().filter(|c| is_sound_cue(&c.text)).count() as f64
                        >= count as f64 * SDH_SHARE,
            }
        })
        .collect()
}

/// [`analyse`] over tracks already extracted to files of their own, each named by its
/// stream index in the container it came from. A track that cannot be read is left out.
pub fn analyse_extracted(tracks: &[(i32, PathBuf)], window_secs: f64) -> Vec<TrackInsight> {
    let cues: Vec<TextTrackCues> = tracks
        .iter()
        .filter_map(|(stream_index, path)| {
            let mut track = read_text_cues(path, Some(WINDOW_SECS))
                .ok()?
                .into_iter()
                .next()?;
            track.stream_index = *stream_index;
            Some(track)
        })
        .collect();
    analyse(&cues, window_secs)
}

/// Every subtitle stream of a media file in container order, tags first and what the text
/// says where they are silent, in the shape a session's track list takes. Reads only the
/// opening stretch, so it works on a file still downloading once that much is there.
pub fn describe_tracks(
    path: &std::path::Path,
    duration_secs: Option<f64>,
) -> kob_ffmpeg::Result<Vec<serde_json::Value>> {
    let tagged = list_subtitle_tracks(path)?;
    let window = duration_secs.map_or(WINDOW_SECS, |d| d.min(WINDOW_SECS));
    let insights = analyse(&read_text_cues(path, Some(WINDOW_SECS))?, window);
    Ok(tagged
        .iter()
        .map(|t| {
            let insight = insights.iter().find(|i| i.stream_index == t.stream_index);
            serde_json::json!({
                "stream_index": t.stream_index,
                "codec": t.codec,
                "language": t.language.clone().or_else(|| insight.and_then(|i| i.language.clone())),
                "title": t.title,
                "is_default": t.is_default,
                "is_forced": t.is_forced || insight.is_some_and(|i| i.forced),
                "is_hearing_impaired": insight.is_some_and(|i| i.sdh),
            })
        })
        .collect())
}

fn detect_language(cues: &[TextCue]) -> Option<String> {
    let text: String = cues
        .iter()
        .map(|c| spoken(&c.text))
        .collect::<Vec<_>>()
        .join(" ");
    if text.chars().filter(|c| c.is_alphabetic()).count() < MIN_LANGUAGE_CHARS {
        return None;
    }
    let info = whatlang::detect(&text)?;
    info.is_reliable()
        .then(|| container_code(info.lang().code()).to_string())
}

fn is_sound_cue(text: &str) -> bool {
    let t = text.trim();
    t.contains('♪')
        || (t.contains('[') && t.contains(']'))
        || (t.starts_with('(') && t.contains(')'))
        || speaker_label(t).is_some()
}

fn speaker_label(line: &str) -> Option<&str> {
    let (label, _) = line.split_once(':')?;
    (label.len() >= 2
        && label.chars().any(|c| c.is_alphabetic())
        && label
            .chars()
            .all(|c| c.is_uppercase() || " .'-".contains(c)))
    .then_some(label)
}

fn spoken(text: &str) -> String {
    text.lines()
        .map(|line| {
            let line = speaker_label(line).map_or(line, |label| &line[label.len() + 1..]);
            let mut out = String::new();
            let mut depth = 0;
            for c in line.chars() {
                match c {
                    '[' | '(' => depth += 1,
                    ']' | ')' if depth > 0 => depth -= 1,
                    '♪' => {}
                    _ if depth == 0 => out.push(c),
                    _ => {}
                }
            }
            out
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// whatlang names some languages by an individual 639-3 code where containers tag the
/// macrolanguage.
fn container_code(iso639_3: &str) -> &str {
    super::language::bibliographic(match iso639_3 {
        "cmn" => "zho",
        "pes" => "fas",
        "zsm" => "msa",
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kob_ffmpeg::subtitles::TextCue;

    const ENGLISH: &[&str] = &[
        "Where were you last night? I waited for hours.",
        "I told you, the car broke down on the motorway.",
        "You always have an excuse ready, don't you?",
        "That isn't fair and you know it.",
        "Then tell me the truth for once.",
    ];
    const FRENCH: &[&str] = &[
        "Où étais-tu hier soir ? Je t'ai attendu des heures.",
        "Je te l'ai dit, la voiture est tombée en panne sur l'autoroute.",
        "Tu as toujours une excuse toute prête, n'est-ce pas ?",
        "Ce n'est pas juste et tu le sais.",
        "Alors dis-moi la vérité pour une fois.",
    ];

    fn track(stream_index: i32, lines: &[&str], every_secs: f64, count: usize) -> TextTrackCues {
        TextTrackCues {
            stream_index,
            codec: "subrip".into(),
            cues: (0..count)
                .map(|i| TextCue {
                    start_secs: i as f64 * every_secs,
                    end_secs: i as f64 * every_secs + 2.0,
                    text: lines[i % lines.len()].to_string(),
                })
                .collect(),
        }
    }

    fn insight(insights: &[TrackInsight], stream_index: i32) -> &TrackInsight {
        insights
            .iter()
            .find(|i| i.stream_index == stream_index)
            .unwrap()
    }

    #[test]
    fn untagged_tracks_are_named_by_what_they_say_in_the_codes_files_carry() {
        let insights = analyse(
            &[track(3, ENGLISH, 6.0, 150), track(4, FRENCH, 6.0, 150)],
            1200.0,
        );
        assert_eq!(insight(&insights, 3).language.as_deref(), Some("eng"));
        assert_eq!(insight(&insights, 4).language.as_deref(), Some("fre"));
    }

    #[test]
    fn too_little_text_names_no_language() {
        let insights = analyse(&[track(3, &["Hey."], 6.0, 3)], 1200.0);
        assert_eq!(insight(&insights, 3).language, None);
    }

    #[test]
    fn a_track_far_sparser_than_its_full_sibling_is_forced() {
        let insights = analyse(
            &[track(3, ENGLISH, 6.0, 180), track(4, ENGLISH, 120.0, 9)],
            1200.0,
        );
        assert!(!insight(&insights, 3).forced);
        assert!(insight(&insights, 4).forced);
    }

    #[test]
    fn a_lone_track_is_forced_only_when_it_barely_speaks() {
        let full = analyse(&[track(3, ENGLISH, 6.0, 180)], 1200.0);
        assert!(!insight(&full, 3).forced);
        let sparse = analyse(&[track(3, ENGLISH, 200.0, 6)], 1200.0);
        assert!(insight(&sparse, 3).forced);
        let silent = analyse(&[track(3, ENGLISH, 6.0, 0)], 1200.0);
        assert!(
            !insight(&silent, 3).forced,
            "a track with nothing in the window says nothing"
        );
    }

    #[test]
    fn sound_cues_and_speaker_labels_mark_sdh() {
        let sdh_lines = [
            "[door slams]",
            "JOHN: Where were you last night?",
            "♪ soft music ♪",
            "I told you, the car broke down.",
        ];
        let insights = analyse(
            &[track(3, &sdh_lines, 6.0, 150), track(4, ENGLISH, 6.0, 150)],
            1200.0,
        );
        assert!(insight(&insights, 3).sdh);
        assert!(!insight(&insights, 4).sdh);
        assert_eq!(
            insight(&insights, 3).language.as_deref(),
            Some("eng"),
            "sound cues do not confuse the language"
        );
    }
}
