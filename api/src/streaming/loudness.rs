use serde::Serialize;

/// What a player needs to level a title's volume: its integrated loudness, and the true
/// peak of the stereo fold-down it will actually be rendered as, which caps the boost.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct PlaybackLoudness {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lufs: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
