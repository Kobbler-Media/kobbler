use uuid::Uuid;

use crate::AppState;
use crate::routes::admin::streaming::{SessionResponse, build_audio_responses};
use crate::streaming::decision::PlaybackMethod;

/// The resolve response for a live session as it stands now, starting from `base` (what
/// it answered when it was opened) for what the session itself does not know. `None`
/// when the session is gone or can no longer deliver: an error, or a transcode that died
/// after the probe.
pub(crate) fn live_response(
    state: &AppState,
    session_id: Uuid,
    base: Option<&SessionResponse>,
) -> Option<SessionResponse> {
    let mut session = state.playback_sessions.get_mut(&session_id)?;
    if session.error.is_some() {
        return None;
    }
    // file_path stays empty until the probe lands; a transcode is only expected after it.
    let probed = !session.file_path.as_os_str().is_empty();
    let needs_transcode = session.decision.method != PlaybackMethod::DirectPlay;
    let transcode_dead = session
        .transcode
        .as_mut()
        .map(|t| !t.is_running())
        .unwrap_or(true);
    if probed && needs_transcode && transcode_dead {
        return None;
    }

    let direct_play = session.decision.method == PlaybackMethod::DirectPlay;
    let (width, height) = session.source_resolution;
    let mut response = base.cloned().unwrap_or_else(|| SessionResponse {
        session_id: session_id.to_string(),
        method: session.decision.method,
        playlist_url: None,
        direct_url: None,
        duration_seconds: 0.0,
        subtitles: vec![],
        audio_tracks: vec![],
        thumbnails_url: None,
        lufs: None,
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
    response.duration_seconds = session.duration_seconds;
    if probed {
        response.audio_tracks =
            build_audio_responses(&session.audio_metas, session.decision.audio_stream_index);
        response.selected_audio_index = Some(session.decision.audio_stream_index);
        response.preferred_audio_stream_index = session.preferred_audio_index;
        response.video_width = (width > 0).then_some(width);
        response.video_height = (height > 0).then_some(height);
    }
    if direct_play {
        response.start_time = None;
    }
    Some(response)
}
