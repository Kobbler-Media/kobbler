use nats_jobs::StreamEpisode;
use uuid::Uuid;

use crate::routes::admin::debrid_stream::StreamResolveBody;

pub(crate) async fn of_body(
    pool: &sqlx::PgPool,
    body: &StreamResolveBody,
) -> Option<StreamEpisode> {
    if body.episode.is_some() {
        return body.episode;
    }
    if let Some(ac) = &body.auto_create
        && let (Some(season), Some(episode)) = (ac.season_number, ac.episode_number)
    {
        return Some(StreamEpisode {
            season: i32::try_from(season).ok()?,
            episode: i32::try_from(episode).ok()?,
        });
    }
    match (
        body.media_type.as_str(),
        body.media_id.filter(|id| !id.is_nil()),
    ) {
        ("episode", Some(id)) => numbers(pool, id).await,
        _ => None,
    }
}

pub(crate) async fn of_media(
    pool: &sqlx::PgPool,
    media_type: &str,
    media_id: Uuid,
) -> Option<StreamEpisode> {
    match media_type {
        "episode" => numbers(pool, media_id).await,
        _ => None,
    }
}

async fn numbers(pool: &sqlx::PgPool, episode_id: Uuid) -> Option<StreamEpisode> {
    let row = sqlx::query!(
        r#"SELECT s.season_identifier, e.episode_identifier
           FROM tv_episodes e JOIN tv_seasons s ON s.id = e.season_id
           WHERE e.id = $1"#,
        episode_id,
    )
    .fetch_optional(pool)
    .await
    .ok()??;
    Some(StreamEpisode {
        season: row.season_identifier.trim().parse().ok()?,
        episode: row.episode_identifier.trim().parse().ok()?,
    })
}
