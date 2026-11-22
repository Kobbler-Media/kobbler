use crate::AppState;
use crate::routes::admin::debrid_stream::StreamResolveBody;
use crate::runtime_config::RuntimeConfig;
use crate::streaming::decision::{ViewerPrefs, decide, preferred_audio};
use crate::streaming::session::PlaybackSession;

/// Whether a repeat resolve of this release can be handed `session`: it plays exactly what
/// opening the release afresh for this request would. Before the probe nothing is known to
/// differ, and the client follows the probe event to whatever it settles on.
pub(crate) fn suits(
    state: &AppState,
    runtime: &RuntimeConfig,
    session: &PlaybackSession,
    body: &StreamResolveBody,
) -> bool {
    let Some(file) = &session.file_meta else {
        return true;
    };
    let language = super::audio_language(body, runtime);
    let wanted = decide(
        file,
        &session.audio_metas,
        &[],
        &body.capabilities.clone().unwrap_or_default(),
        ViewerPrefs {
            preferred_audio_index: preferred_audio(
                &session.audio_metas,
                body.preferred_audio_title.as_deref(),
                language,
            ),
            preferred_subtitle_index: None,
            understood_language: language,
            subtitle_language: "",
        },
        &state.encoder_caps.preferred_codecs(),
    );
    // A session forced to re-encode after decode errors plays a clean copy of the same
    // track, which any client that wanted that track can take.
    wanted == session.decision
        || (session.force_video_transcode
            && wanted.audio_stream_index == session.decision.audio_stream_index)
}
