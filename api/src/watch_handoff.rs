//! Cross-device playback handoff.
//!
//! Someone pauses an episode on one device, carries on on another — often several
//! episodes further — and comes back to the first. That first player is still sitting on
//! the old episode at the old position, and pressing play there would write that stale
//! position straight back over the newer one.
//!
//! Every position save that names the device it came from (`client_id`) is announced to
//! the same user's other open players as a `watch_progress` event, and remembered per
//! show (or movie) so a player whose event stream dropped — a phone that was
//! backgrounded, a laptop that slept — can ask for it on the way back instead of waiting
//! for the next save. The player decides what to do with it; the server only relays.
//!
//! In memory only: a restart forgets the last device, and the cost of that is a prompt
//! not offered, never a position lost.

use crate::AppState;
use crate::events::LibraryEvent;
use crate::watch_completion::PlaybackExtent;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Longest a device label is kept. It is display text, and it arrives from the client.
const MAX_LABEL_LEN: usize = 64;

/// The device a position save came from, as the client describes itself.
///
/// Optional on the wire: a save without it (an older client, a cast receiver, a script)
/// still saves, it just is not relayed — there is no way to tell a player its own
/// save apart from someone else's without it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlaybackClient {
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub client_label: Option<String>,
}

impl PlaybackClient {
    fn id(&self) -> Option<String> {
        self.client_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .map(str::to_owned)
    }

    fn label(&self) -> Option<String> {
        self.client_label
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(MAX_LABEL_LEN).collect())
    }
}

/// Enough about an episode for a player to switch to it without asking first.
#[derive(Debug, Clone, Serialize)]
pub struct HandoffEpisode {
    pub show_title: String,
    pub title: Option<String>,
    pub season_identifier: String,
    pub episode_identifier: String,
    pub still_url: Option<String>,
    /// First available local file, or null for an episode only streamed via debrid.
    pub file_id: Option<Uuid>,
}

/// The newest position a device saved for one show or movie.
#[derive(Debug, Clone, Serialize)]
pub struct WatchProgress {
    /// `"episode"` or `"movie"`.
    pub media_type: String,
    pub media_id: Uuid,
    /// The show for an episode, the movie itself for a movie. Players match on this: a
    /// device three episodes ahead is still the same show.
    pub scope_id: Uuid,
    pub position_seconds: i32,
    pub duration_seconds: Option<f64>,
    pub watched: bool,
    pub client_id: String,
    pub client_label: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub saved_at: OffsetDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode: Option<HandoffEpisode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movie_title: Option<String>,
}

/// Last save per (user, scope). Bounded by users × titles they have played on a
/// device that identifies itself, which is small.
pub type HandoffStore = dashmap::DashMap<(Uuid, Uuid), WatchProgress>;

/// Record and announce an episode position save. Never fails the save it rides on.
pub async fn episode_saved(
    state: &AppState,
    user_id: Uuid,
    episode_id: Uuid,
    position_seconds: i32,
    extent: Option<PlaybackExtent>,
    client: &PlaybackClient,
) {
    let Some(client_id) = client.id() else {
        return;
    };
    let row = sqlx::query!(
        r#"
        SELECT s.show_id, e.title, e.episode_identifier, e.images,
               s.season_identifier, sh.title AS "show_title!",
               (SELECT f.id FROM tv_episode_file_episodes efe
                JOIN tv_episode_files f ON f.id = efe.episode_file_id
                WHERE efe.episode_id = e.id AND f.available = true
                ORDER BY f.created_at DESC, f.id DESC
                LIMIT 1) AS "file_id?"
        FROM tv_episodes e
        JOIN tv_seasons s ON s.id = e.season_id
        JOIN tv_shows sh ON sh.id = s.show_id
        WHERE e.id = $1
        "#,
        episode_id,
    )
    .fetch_optional(&state.pool)
    .await;
    let row = match row {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(error = %e, %episode_id, "watch handoff: episode lookup failed");
            return;
        }
    };
    let still_url = row
        .images
        .get("still")
        .and_then(|v| v.get("source_url")?.as_str())
        .map(str::to_owned);
    publish(
        state,
        user_id,
        WatchProgress {
            media_type: "episode".into(),
            media_id: episode_id,
            scope_id: row.show_id,
            position_seconds,
            duration_seconds: extent.map(|e| e.duration_seconds),
            watched: extent.is_some_and(|e| e.is_finished_at(position_seconds)),
            client_id,
            client_label: client.label(),
            saved_at: OffsetDateTime::now_utc(),
            episode: Some(HandoffEpisode {
                show_title: row.show_title,
                title: row.title,
                season_identifier: row.season_identifier,
                episode_identifier: row.episode_identifier,
                still_url,
                file_id: row.file_id,
            }),
            movie_title: None,
        },
    );
}

/// Record and announce a movie position save. Never fails the save it rides on.
pub async fn movie_saved(
    state: &AppState,
    user_id: Uuid,
    movie_id: Uuid,
    position_seconds: i32,
    extent: Option<PlaybackExtent>,
    client: &PlaybackClient,
) {
    let Some(client_id) = client.id() else {
        return;
    };
    let title = sqlx::query_scalar!("SELECT title FROM movies WHERE id = $1", movie_id)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten();
    publish(
        state,
        user_id,
        WatchProgress {
            media_type: "movie".into(),
            media_id: movie_id,
            scope_id: movie_id,
            position_seconds,
            duration_seconds: extent.map(|e| e.duration_seconds),
            watched: extent.is_some_and(|e| e.is_finished_at(position_seconds)),
            client_id,
            client_label: client.label(),
            saved_at: OffsetDateTime::now_utc(),
            episode: None,
            movie_title: title,
        },
    );
}

/// What a player asks for when it opens, changes episode, or its event stream comes back.
#[derive(Debug, Serialize)]
pub struct HandoffResponse {
    /// What `watch_progress` events for this title carry as `scope_id` — the show for an
    /// episode. Null when the title does not exist.
    pub scope_id: Option<Uuid>,
    /// The newest save for that scope from any of the caller's devices, including the
    /// caller's own; the player filters by `client_id`.
    pub latest: Option<WatchProgress>,
}

pub async fn latest_for(
    state: &AppState,
    user_id: Uuid,
    media_type: &str,
    media_id: Uuid,
) -> HandoffResponse {
    let scope_id = match media_type {
        "movie" => Some(media_id),
        "episode" => sqlx::query_scalar!(
            "SELECT s.show_id FROM tv_episodes e JOIN tv_seasons s ON s.id = e.season_id \
             WHERE e.id = $1",
            media_id,
        )
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten(),
        _ => None,
    };
    let latest = scope_id.and_then(|scope| {
        state
            .watch_handoffs
            .get(&(user_id, scope))
            .map(|p| p.value().clone())
    });
    HandoffResponse { scope_id, latest }
}

fn publish(state: &AppState, user_id: Uuid, progress: WatchProgress) {
    state
        .watch_handoffs
        .insert((user_id, progress.scope_id), progress.clone());
    let _ = state.library_events.send(LibraryEvent::WatchProgress {
        user_id,
        progress: Box::new(progress),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_or_oversized_client_id_is_no_client() {
        let blank = PlaybackClient {
            client_id: Some("   ".into()),
            client_label: None,
        };
        assert_eq!(blank.id(), None);
        let huge = PlaybackClient {
            client_id: Some("x".repeat(200)),
            client_label: None,
        };
        assert_eq!(huge.id(), None);
    }

    #[test]
    fn a_long_label_is_cut_on_a_char_boundary() {
        let c = PlaybackClient {
            client_id: Some("a".into()),
            client_label: Some("é".repeat(100)),
        };
        assert_eq!(c.label().unwrap().chars().count(), MAX_LABEL_LEN);
    }
}
