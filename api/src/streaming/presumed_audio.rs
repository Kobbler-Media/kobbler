//! A file's only audio track, left untagged, is presumed to be in the language the title
//! was made in: most single-track releases are the original audio, and without a language
//! a foreign film plays with its subtitles off.

use sqlx::PgPool;

use super::decision::{AudioTrackMeta, feature_tracks, is_untagged};
use super::language;
use crate::video_metadata::original_language::{self, Title};

const DUB_MARKERS: &[&str] = &["dub", "dubbed", "dublado", "doblado", "dual", "multi"];

// TMDB writes Cantonese as `cn`, which ISO 639-1 does not have.
const TMDB_CANTONESE: (&str, &str) = ("cn", "yue");

fn is_dub(release_name: &str) -> bool {
    release_name
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| DUB_MARKERS.contains(&word))
}

fn unlabelled_lone_track<'a>(
    tracks: &'a [AudioTrackMeta],
    release_name: &str,
) -> Option<&'a AudioTrackMeta> {
    let [track] = feature_tracks(tracks)[..] else {
        return None;
    };
    let labelled = !is_untagged(track.language.as_deref())
        || track
            .title
            .as_deref()
            .is_some_and(language::names_a_language);
    (!labelled && !is_dub(release_name)).then_some(track)
}

/// The stream to relabel and the tag to give it, in the 639-2/B form containers use.
/// `original_language` is TMDB's.
pub fn presumed_audio_language(
    tracks: &[AudioTrackMeta],
    original_language: Option<&str>,
    release_name: &str,
) -> Option<(i32, &'static str)> {
    let track = unlabelled_lone_track(tracks, release_name)?;
    let original = original_language?.trim().to_ascii_lowercase();
    let code = if original == TMDB_CANTONESE.0 {
        TMDB_CANTONESE.1
    } else {
        language::container_code(&original)?
    };
    Some((track.stream_index, code))
}

pub async fn presume(
    pool: &PgPool,
    title: Option<&Title>,
    release_name: &str,
    tracks: &mut [AudioTrackMeta],
) {
    let Some(title) = title else {
        return;
    };
    if unlabelled_lone_track(tracks, release_name).is_none() {
        return;
    }
    let original = original_language::of(pool, title).await;
    if let Some((index, code)) = presumed_audio_language(tracks, original.as_deref(), release_name)
        && let Some(track) = tracks.iter_mut().find(|t| t.stream_index == index)
    {
        track.language = Some(code.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(index: i32, language: Option<&str>, title: Option<&str>) -> AudioTrackMeta {
        AudioTrackMeta {
            stream_index: index,
            codec: "aac".to_string(),
            channels: 2,
            language: language.map(str::to_string),
            title: title.map(str::to_string),
            is_default: false,
        }
    }

    #[test]
    fn presumes_the_original_language_for_a_lone_untagged_feature_track() {
        let lone = |language, title| vec![track(1, language, title)];
        let cases = [
            (
                "untagged",
                lone(None, None),
                Some("ko"),
                "Film.2019.1080p.mkv",
                Some((1, "kor")),
            ),
            (
                "und",
                lone(Some("und"), None),
                Some("ko"),
                "film.mkv",
                Some((1, "kor")),
            ),
            (
                "blank tag",
                lone(Some(" "), None),
                Some("ko"),
                "film.mkv",
                Some((1, "kor")),
            ),
            (
                "codec title",
                lone(None, Some("DTS-HD MA 5.1")),
                Some("ko"),
                "film.mkv",
                Some((1, "kor")),
            ),
            (
                "english original",
                lone(None, None),
                Some("en"),
                "film.mkv",
                Some((1, "eng")),
            ),
            (
                "bibliographic code",
                lone(None, None),
                Some("fr"),
                "film.mkv",
                Some((1, "fre")),
            ),
            (
                "upper case",
                lone(None, None),
                Some("ZH"),
                "film.mkv",
                Some((1, "chi")),
            ),
            (
                "tmdb cantonese",
                lone(None, None),
                Some("cn"),
                "film.mkv",
                Some((1, "yue")),
            ),
            (
                "tagged",
                lone(Some("eng"), None),
                Some("ko"),
                "film.mkv",
                None,
            ),
            (
                "title names a language",
                lone(None, Some("AC3 [English]")),
                Some("ko"),
                "film.mkv",
                None,
            ),
            (
                "title names a code",
                lone(None, Some("ENG 5.1")),
                Some("ko"),
                "film.mkv",
                None,
            ),
            ("original unknown", lone(None, None), None, "film.mkv", None),
            (
                "tmdb no language",
                lone(None, None),
                Some("xx"),
                "film.mkv",
                None,
            ),
            (
                "empty original",
                lone(None, None),
                Some(""),
                "film.mkv",
                None,
            ),
            ("no tracks", vec![], Some("ko"), "film.mkv", None),
            (
                "two feature tracks",
                vec![track(1, None, None), track(2, None, None)],
                Some("ko"),
                "film.mkv",
                None,
            ),
            (
                "commentary beside the feature",
                vec![
                    track(1, Some("eng"), Some("Director's Commentary")),
                    track(2, None, None),
                ],
                Some("ko"),
                "film.mkv",
                Some((2, "kor")),
            ),
            (
                "dubbed",
                lone(None, None),
                Some("ko"),
                "Film.2019.DUBBED.1080p.mkv",
                None,
            ),
            (
                "dub",
                lone(None, None),
                Some("ko"),
                "Film 2019 Eng Dub",
                None,
            ),
            (
                "dublado",
                lone(None, None),
                Some("ko"),
                "Film.2019.Dublado.mkv",
                None,
            ),
            (
                "doblado",
                lone(None, None),
                Some("ko"),
                "Film (2019) [doblado]",
                None,
            ),
            (
                "dual audio",
                lone(None, None),
                Some("ko"),
                "Film.2019.Dual.Audio.mkv",
                None,
            ),
            (
                "multi",
                lone(None, None),
                Some("ko"),
                "Film.2019.MULTi.1080p",
                None,
            ),
            (
                "marker inside a word",
                lone(None, None),
                Some("ko"),
                "Dublin.Multiverse.Dualism.mkv",
                Some((1, "kor")),
            ),
        ];
        for (name, tracks, original, release, expected) in cases {
            assert_eq!(
                presumed_audio_language(&tracks, original, release),
                expected,
                "{name}"
            );
        }
    }
}
