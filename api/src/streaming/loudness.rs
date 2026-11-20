use serde::{Deserialize, Serialize};

/// What a player needs to level a track's volume: its integrated loudness, and the true
/// peak of the stereo fold-down it will actually be rendered as, which caps the boost.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct PlaybackLoudness {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lufs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downmix_peak_db: Option<f64>,
}

impl From<kob_ffmpeg::audio::Loudness> for PlaybackLoudness {
    fn from(l: kob_ffmpeg::audio::Loudness) -> Self {
        Self {
            lufs: l.integrated_lufs,
            downmix_peak_db: l.downmix_peak_db,
        }
    }
}

/// One audio stream's [`PlaybackLoudness`], keyed by its absolute stream index — the index
/// audio tracks are addressed by everywhere else.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrackLoudness {
    pub stream_index: i32,
    #[serde(flatten)]
    pub loudness: PlaybackLoudness,
}

impl From<kob_ffmpeg::track_loudness::TrackLoudness> for TrackLoudness {
    fn from(t: kob_ffmpeg::track_loudness::TrackLoudness) -> Self {
        Self {
            stream_index: t.stream_index as i32,
            loudness: t.loudness.into(),
        }
    }
}

pub fn of_track(tracks: &[TrackLoudness], stream_index: i32) -> PlaybackLoudness {
    tracks
        .iter()
        .find(|t| t.stream_index == stream_index)
        .map(|t| t.loudness)
        .unwrap_or_default()
}
