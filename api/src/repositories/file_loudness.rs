//! The loudness facts players level volume by: per audio stream for movie, episode and
//! debrid files, per file for audiobooks.

use super::Result;
use crate::streaming::loudness::{PlaybackLoudness, TrackLoudness};
use crate::streaming::session::MediaType;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use sqlx::types::Json;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Movie,
    Episode,
    Audiobook,
    /// `debrid_files.id`; its measurements live on every session playing that download.
    Debrid,
}

impl FileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Episode => "episode",
            Self::Audiobook => "audiobook",
            Self::Debrid => "debrid",
        }
    }

    pub fn of_session(media_type: MediaType) -> Option<Self> {
        match media_type {
            MediaType::Movie => Some(Self::Movie),
            MediaType::Episode => Some(Self::Episode),
            MediaType::Recording => None,
        }
    }
}

/// `None` until measured; audiobooks are measured per file, see [`audiobook`].
pub async fn tracks(
    pool: &PgPool,
    kind: FileKind,
    file_id: Uuid,
) -> Result<Option<Vec<TrackLoudness>>> {
    let row = match kind {
        FileKind::Movie => sqlx::query_scalar!(
            r#"SELECT audio_loudness AS "audio_loudness: Json<Vec<TrackLoudness>>"
               FROM movie_files WHERE id = $1"#,
            file_id,
        )
        .fetch_optional(pool)
        .await?
        .flatten(),
        FileKind::Episode => sqlx::query_scalar!(
            r#"SELECT audio_loudness AS "audio_loudness: Json<Vec<TrackLoudness>>"
               FROM tv_episode_files WHERE id = $1"#,
            file_id,
        )
        .fetch_optional(pool)
        .await?
        .flatten(),
        FileKind::Debrid => sqlx::query_scalar!(
            r#"SELECT s.audio_loudness AS "audio_loudness: Json<Vec<TrackLoudness>>"
               FROM debrid_files f JOIN debrid_stream_sessions s ON s.cache_path = f.cache_path
               WHERE f.id = $1
               ORDER BY s.audio_loudness IS NULL, s.updated_at DESC
               LIMIT 1"#,
            file_id,
        )
        .fetch_optional(pool)
        .await?
        .flatten(),
        FileKind::Audiobook => None,
    };
    Ok(row.map(|Json(t)| t))
}

pub async fn audiobook(pool: &PgPool, file_id: Uuid) -> Result<PlaybackLoudness> {
    let row = sqlx::query_as!(
        PlaybackLoudness,
        "SELECT lufs, downmix_peak_db FROM audiobook_files WHERE id = $1",
        file_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.unwrap_or_default())
}

/// An audiobook keeps its first stream's figures; fields left `None` keep what it has.
pub async fn record_tracks(
    pool: &PgPool,
    kind: FileKind,
    file_id: Uuid,
    tracks: &[TrackLoudness],
) -> Result<()> {
    let measured = Json(tracks);
    match kind {
        FileKind::Movie => {
            sqlx::query!(
                "UPDATE movie_files SET audio_loudness = $2 WHERE id = $1",
                file_id,
                measured as _,
            )
            .execute(pool)
            .await?;
        }
        FileKind::Episode => {
            sqlx::query!(
                "UPDATE tv_episode_files SET audio_loudness = $2 WHERE id = $1",
                file_id,
                measured as _,
            )
            .execute(pool)
            .await?;
        }
        FileKind::Debrid => {
            sqlx::query!(
                "UPDATE debrid_stream_sessions SET audio_loudness = $2, updated_at = NOW()
                 WHERE cache_path = (SELECT cache_path FROM debrid_files WHERE id = $1)",
                file_id,
                measured as _,
            )
            .execute(pool)
            .await?;
        }
        FileKind::Audiobook => {
            record_audiobook(
                pool,
                file_id,
                tracks.first().map(|t| t.loudness).unwrap_or_default(),
            )
            .await?;
        }
    }
    Ok(())
}

/// Fields left `None` keep what the row already has.
pub async fn record_audiobook(
    pool: &PgPool,
    file_id: Uuid,
    loudness: PlaybackLoudness,
) -> Result<()> {
    sqlx::query!(
        "UPDATE audiobook_files SET lufs = COALESCE($2, lufs), \
         downmix_peak_db = COALESCE($3, downmix_peak_db) WHERE id = $1",
        file_id,
        loudness.lufs,
        loudness.downmix_peak_db,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Files analysed before each audio stream was measured on its own (audiobooks: before
/// the downmix peak was), and finished debrid downloads nothing measured. Whatever is
/// being watched or listened to goes first — a whole show, not just the episode last
/// touched — then the newest, since a full library takes the best part of a day to read.
pub async fn list_needing_loudness(pool: &PgPool, limit: i64) -> Result<Vec<(FileKind, Uuid)>> {
    let rows = sqlx::query!(
        r#"
        SELECT kind AS "kind!", id AS "id!" FROM (
            SELECT 'movie' AS kind, f.id, f.created_at AS added,
                   (SELECT MAX(w.updated_at) FROM movie_watch_states w WHERE w.movie_id = f.movie_id) AS touched
            FROM movie_files f
            WHERE f.available AND f.media_cache_version > 0 AND f.audio_loudness IS NULL
            UNION ALL
            SELECT 'episode', f.id, f.created_at,
                   (SELECT MAX(w.updated_at)
                    FROM tv_watch_states w
                    JOIN tv_episodes we ON we.id = w.episode_id
                    JOIN tv_seasons ws ON ws.id = we.season_id
                    WHERE ws.show_id = (
                        SELECT s.show_id
                        FROM tv_episode_file_episodes fe
                        JOIN tv_episodes e ON e.id = fe.episode_id
                        JOIN tv_seasons s ON s.id = e.season_id
                        WHERE fe.episode_file_id = f.id
                        LIMIT 1))
            FROM tv_episode_files f
            WHERE f.available AND f.media_cache_version > 0 AND f.audio_loudness IS NULL
            UNION ALL
            SELECT 'audiobook', f.id, f.added_at,
                   (SELECT MAX(r.updated_at) FROM reading_states r WHERE r.book_id = f.book_id)
            FROM audiobook_files f
            WHERE f.available AND f.lufs IS NOT NULL AND f.downmix_peak_db IS NULL
            UNION ALL
            SELECT 'debrid', f.id, f.created_at,
                   (SELECT MAX(s.played_at) FROM debrid_stream_sessions s
                    WHERE s.cache_path = f.cache_path)
            FROM debrid_files f
            JOIN debrid_cache_files c ON c.file_path = f.cache_path
            WHERE EXISTS (SELECT 1 FROM debrid_stream_sessions s
                          WHERE s.cache_path = f.cache_path AND s.audio_loudness IS NULL)
        ) gaps
        ORDER BY touched DESC NULLS LAST, added DESC NULLS LAST, id DESC
        LIMIT $1
        "#,
        limit,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let kind = match r.kind.as_str() {
                "movie" => FileKind::Movie,
                "episode" => FileKind::Episode,
                "audiobook" => FileKind::Audiobook,
                "debrid" => FileKind::Debrid,
                _ => return None,
            };
            Some((kind, r.id))
        })
        .collect())
}
