//! The loudness facts of movie, episode and audiobook files that players level volume by.

use super::Result;
use crate::streaming::loudness::PlaybackLoudness;
use crate::streaming::session::MediaType;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Movie,
    Episode,
    Audiobook,
}

impl FileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Episode => "episode",
            Self::Audiobook => "audiobook",
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

pub async fn get(pool: &PgPool, kind: FileKind, file_id: Uuid) -> Result<PlaybackLoudness> {
    let row = match kind {
        FileKind::Movie => {
            sqlx::query_as!(
                PlaybackLoudness,
                "SELECT lufs, downmix_peak_db FROM movie_files WHERE id = $1",
                file_id,
            )
            .fetch_optional(pool)
            .await?
        }
        FileKind::Episode => {
            sqlx::query_as!(
                PlaybackLoudness,
                "SELECT lufs, downmix_peak_db FROM tv_episode_files WHERE id = $1",
                file_id,
            )
            .fetch_optional(pool)
            .await?
        }
        FileKind::Audiobook => {
            sqlx::query_as!(
                PlaybackLoudness,
                "SELECT lufs, downmix_peak_db FROM audiobook_files WHERE id = $1",
                file_id,
            )
            .fetch_optional(pool)
            .await?
        }
    };
    Ok(row.unwrap_or_default())
}

/// Fields left `None` keep what the row already has.
pub async fn record(
    pool: &PgPool,
    media_type: &str,
    file_id: Uuid,
    loudness: PlaybackLoudness,
) -> Result<()> {
    let PlaybackLoudness {
        lufs,
        downmix_peak_db,
    } = loudness;
    match media_type {
        "movie" => {
            sqlx::query!(
                "UPDATE movie_files SET lufs = COALESCE($2, lufs), \
             downmix_peak_db = COALESCE($3, downmix_peak_db) WHERE id = $1",
                file_id,
                lufs,
                downmix_peak_db,
            )
            .execute(pool)
            .await?
        }
        "episode" => {
            sqlx::query!(
                "UPDATE tv_episode_files SET lufs = COALESCE($2, lufs), \
             downmix_peak_db = COALESCE($3, downmix_peak_db) WHERE id = $1",
                file_id,
                lufs,
                downmix_peak_db,
            )
            .execute(pool)
            .await?
        }
        "audiobook" => {
            sqlx::query!(
                "UPDATE audiobook_files SET lufs = COALESCE($2, lufs), \
             downmix_peak_db = COALESCE($3, downmix_peak_db) WHERE id = $1",
                file_id,
                lufs,
                downmix_peak_db,
            )
            .execute(pool)
            .await?
        }
        _ => return Ok(()),
    };
    Ok(())
}

/// Files whose loudness was measured before the downmix peak was, so players cannot cap
/// their levelling boost yet. Whatever is being watched or listened to goes first — a whole
/// show, not just the episode last touched — then the newest, since a full library takes
/// the best part of a day to read.
pub async fn list_needing_downmix_peak(pool: &PgPool, limit: i64) -> Result<Vec<(FileKind, Uuid)>> {
    let rows = sqlx::query!(
        r#"
        SELECT kind AS "kind!", id AS "id!" FROM (
            SELECT 'movie' AS kind, f.id, f.created_at AS added,
                   (SELECT MAX(w.updated_at) FROM movie_watch_states w WHERE w.movie_id = f.movie_id) AS touched
            FROM movie_files f
            WHERE f.lufs IS NOT NULL AND f.downmix_peak_db IS NULL
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
            WHERE f.lufs IS NOT NULL AND f.downmix_peak_db IS NULL
            UNION ALL
            SELECT 'audiobook', f.id, uuid_extract_timestamp(f.id),
                   (SELECT MAX(r.updated_at) FROM reading_states r WHERE r.book_id = f.book_id)
            FROM audiobook_files f
            WHERE f.lufs IS NOT NULL AND f.downmix_peak_db IS NULL
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
                _ => return None,
            };
            Some((kind, r.id))
        })
        .collect())
}
