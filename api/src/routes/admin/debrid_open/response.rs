use uuid::Uuid;

use crate::AppState;
use crate::routes::admin::streaming::{
    Delivery, SessionResponse, build_audio_responses, progressive_url,
};
use crate::streaming::decision::PlaybackMethod;

/// The resolve response for a live session as it stands now, starting from `base` (what
/// it answered when it was opened) for what the session itself does not know. `None`
/// when the session is gone or can no longer deliver: an error, or a transcode that died
/// after the probe.
pub(crate) fn live_response(
    state: &AppState,
    session_id: Uuid,
    base: Option<&SessionResponse>,
    delivery: Delivery,
) -> Option<SessionResponse> {
    let mut session = state.playback_sessions.get_mut(&session_id)?;
    if session.error.is_some() {
        return None;
    }
    // file_path stays empty until the probe lands.
    let probed = !session.file_path.as_os_str().is_empty();
    // Not attached yet, or finished with every segment written, is as good as running:
    // only an encoder that failed leaves nothing to play.
    if let Some(transcode) = session.transcode.as_mut()
        && transcode.check_health().is_err()
    {
        return None;
    }

    let direct_play = session.decision.method == PlaybackMethod::DirectPlay;
    let (width, height) = session.source_resolution;
    let mut response = base.cloned().unwrap_or_else(|| SessionResponse {
        session_id: session_id.to_string(),
        method: session.decision.method,
        playlist_url: None,
        direct_url: None,
        progressive_url: None,
        duration_seconds: 0.0,
        subtitles: vec![],
        audio_tracks: vec![],
        thumbnails_url: None,
        chapters: vec![],
        skip_segments: vec![],
        start_time: None,
        media_id: None,
        media_type: None,
        file_id: None,
        selected_audio_index: None,
        preferred_audio_stream_index: None,
        selected_subtitle_index: None,
        video_width: None,
        video_height: None,
        picture: None,
    });
    response.method = session.decision.method;
    response.playlist_url =
        (!direct_play).then(|| format!("/api/v1/admin/playback/sessions/{session_id}/master.m3u8"));
    response.direct_url = direct_play.then(|| format!("/api/v1/admin/stream/{session_id}/play"));
    response.progressive_url = progressive_url(session_id, session.decision.method, delivery);
    response.duration_seconds = session.duration_seconds;
    if probed {
        response.audio_tracks = build_audio_responses(
            &session.audio_metas,
            session.decision.audio_stream_index,
            &session.audio_loudness,
        );
        response.selected_audio_index = Some(session.decision.audio_stream_index);
        response.preferred_audio_stream_index = session.preferred_audio_index;
        response.video_width = (width > 0).then_some(width);
        response.video_height = (height > 0).then_some(height);
        response.chapters = session.chapters.clone();
    }
    if direct_play {
        response.start_time = None;
    }
    Some(response)
}
