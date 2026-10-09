//! Importing a completed recording into the library (Decision 16).
//!
//! A recording joins the library as an additional copy, never a replacement: a
//! downloaded episode and a recording of the same airing must coexist and be
//! tellable apart, which is what `source_type` + `source_metadata` carry.

use std::path::Path;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::file_link_mode::FileLinkMode;
use crate::livetv::matching::{self, Match, MatchInput};
use crate::repositories::{
    RepositoryError, livetv_channels, livetv_guide, livetv_recordings, movie_files,
    tv_episode_files,
};

pub const SOURCE_TYPE: &str = "livetv";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Imported {
    Episode { file_id: Uuid, episode_id: Uuid },
    Movie { file_id: Uuid, movie_id: Uuid },
    Unmatched,
}

/// "recorded from 10 drama, Sat 09:30" — the UI must be able to say where a
/// library item came from rather than pretending it was downloaded.
pub fn provenance(
    recording_id: Uuid,
    channel_id: Uuid,
    channel_name: &str,
    lcn: Option<i32>,
    guide_id: Option<Uuid>,
    aired: OffsetDateTime,
) -> serde_json::Value {
    serde_json::json!({
        "recording_id": recording_id.to_string(),
        "channel_id": channel_id.to_string(),
        "channel_name": channel_name,
        "lcn": lcn,
        "guide_id": guide_id.map(|g| g.to_string()),
        "aired": aired
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
    })
}

/// Never blocks on a match: an unmatched recording stays watchable under its
/// channel and programme title, it just gets no siblings.
pub async fn import(
    pool: &sqlx::PgPool,
    recording: &livetv_recordings::RecordingRow,
    manifest: &Path,
) -> Result<Imported, RepositoryError> {
    // `OriginalAirdate` is the guide's, not the broadcast time: on a repeat the
    // two differ, and matching a repeat against today's date would resolve to
    // whatever else the show aired today.
    let entry = match recording.guide_id {
        Some(id) => livetv_guide::get_by_id(pool, id).await?,
        None => None,
    };
    let original_airdate = entry.as_ref().and_then(|e| {
        e.original_airdate
            .or_else(|| e.is_first.then_some(e.start_time))
    });
    let matched = matching::resolve(
        pool,
        &MatchInput {
            title: recording.title.clone(),
            episode_number: recording.episode_number.clone(),
            episode_title: recording.episode_title.clone(),
            original_airdate,
        },
    )
    .await?;

    let (episode_id, movie_id) = match matched {
        Match::Episode(id) => (Some(id), None),
        Match::Movie(id) => (None, Some(id)),
        Match::None => (None, None),
    };
    livetv_recordings::set_match(pool, recording.id, episode_id, movie_id).await?;

    if episode_id.is_none() && movie_id.is_none() {
        return Ok(Imported::Unmatched);
    }

    let (channel_name, lcn) = livetv_channels::labels_for(pool, &[recording.channel_id])
        .await?
        .into_iter()
        .next()
        .map(|(_, name, lcn)| (name, lcn))
        .unwrap_or_else(|| ("Unknown".to_string(), None));
    let channel = livetv_channels::get_by_id(pool, recording.channel_id).await?;
    // The probe walker claims any file row with a NULL codec, and probing an
    // HLS manifest as if it were a media file quarantines it as unreadable —
    // so the codec the scan already recorded is carried across.
    let video_codec = recording
        .video_codec
        .clone()
        .or_else(|| channel.as_ref().and_then(|c| c.video_codec.clone()))
        .or_else(|| Some("h264".to_string()));
    let audio_codec = recording
        .audio_codec
        .clone()
        .or_else(|| channel.as_ref().and_then(|c| c.audio_codec.clone()));
    let _ = audio_codec;

    let file_path = manifest.to_string_lossy().to_string();
    let provenance = provenance(
        recording.id,
        recording.channel_id,
        &channel_name,
        lcn,
        recording.guide_id,
        recording.actual_start.unwrap_or(recording.scheduled_start),
    );

    // A recorded film needs a real `movie_files` row for the same reason an
    // episode needs a `tv_episode_files` one: without it the movie is not in the
    // library, `/movies/{id}` has nothing to play, no `movie_watch_states`
    // position is ever written, and it can never reach Continue watching.
    if let Some(movie_id) = movie_id {
        let file_id = movie_files::create_or_update(
            pool,
            movie_files::MovieFileInput {
                movie_id,
                file_path,
                file_size: recording.size_bytes,
                format: "hls".to_string(),
                source_type: Some(SOURCE_TYPE.to_string()),
                source_metadata: Some(provenance),
                video_codec,
                duration_seconds: recording.duration_seconds,
                container: Some("hls".to_string()),
                ..Default::default()
            },
        )
        .await?;
        return Ok(Imported::Movie { file_id, movie_id });
    }
    let Some(episode_id) = episode_id else {
        return Ok(Imported::Unmatched);
    };
    let file_id = tv_episode_files::create_or_update(
        pool,
        tv_episode_files::EpisodeFileInput {
            file_path,
            file_size: recording.size_bytes,
            file_fingerprint: None,
            format: "hls".to_string(),
            quality: None,
            languages: None,
            subtitles: None,
            source_type: Some(SOURCE_TYPE.to_string()),
            source_metadata: Some(provenance),
            video_codec,
            video_width: None,
            video_height: None,
            video_bitrate_kbps: None,
            framerate: None,
            bit_depth: None,
            is_hdr: false,
            is_interlaced: false,
            hdr_format: None,
            duration_seconds: recording.duration_seconds,
            container: Some("hls".to_string()),
            overall_bitrate_kbps: None,
        },
    )
    .await?;

    // AddCopy, not Replace: Replace deletes the episode's existing exclusive
    // files, which would make a recording silently destroy a downloaded copy.
    tv_episode_files::link_episodes_with_mode(pool, file_id, &[episode_id], FileLinkMode::AddCopy)
        .await?;

    Ok(Imported::Episode {
        file_id,
        episode_id,
    })
}
