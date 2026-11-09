use crate::common::*;
use api::repositories::{audio_tracks, external_subtitles, subtitle_tracks};
use serde_json::Value;
use uuid::Uuid;

async fn seed_movie(server: &TestServer) -> (Uuid, Uuid) {
    let movie_id = Uuid::new_v4();
    sqlx::query("INSERT INTO movies (id, title, year) VALUES ($1, 'Names', 2001)")
        .bind(movie_id)
        .execute(&server.pool)
        .await
        .unwrap();
    (
        movie_id,
        create_stub_movie_file(&server.pool, movie_id).await,
    )
}

async fn seed_episode(server: &TestServer) -> (Uuid, Uuid) {
    let (show_id, season_id, episode_id) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    sqlx::query("INSERT INTO tv_shows (id, title, year) VALUES ($1, 'Names', 2010)")
        .bind(show_id)
        .execute(&server.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
         VALUES ($1, $2, '1', 1, true, false)",
    )
    .bind(season_id)
    .bind(show_id)
    .execute(&server.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order)
         VALUES ($1, $2, '1', 1)",
    )
    .bind(episode_id)
    .bind(season_id)
    .execute(&server.pool)
    .await
    .unwrap();
    (
        episode_id,
        create_stub_episode_file(&server.pool, episode_id).await,
    )
}

async fn add_tracks(server: &TestServer, movie_file: Option<Uuid>, episode_file: Option<Uuid>) {
    audio_tracks::create(
        &server.pool,
        &audio_tracks::AudioTrackInput {
            movie_file_id: movie_file,
            episode_file_id: episode_file,
            track_index: 1,
            codec: "aac".to_string(),
            channels: 2,
            bitrate_kbps: None,
            sample_rate: None,
            language: Some("ger".to_string()),
            title: None,
            is_default: true,
        },
    )
    .await
    .unwrap();
    for (track_index, language) in [(2, Some("cze")), (3, Some("und"))] {
        subtitle_tracks::create(
            &server.pool,
            &subtitle_tracks::SubtitleTrackInput {
                movie_file_id: movie_file,
                episode_file_id: episode_file,
                track_index,
                codec: "subrip".to_string(),
                language: language.map(str::to_string),
                title: None,
                is_forced: false,
                is_default: false,
                is_hearing_impaired: false,
            },
        )
        .await
        .unwrap();
    }
    external_subtitles::create(
        &server.pool,
        &external_subtitles::ExternalSubtitleInput {
            movie_file_id: movie_file,
            episode_file_id: episode_file,
            file_path: format!("/test/{}.gre.srt", Uuid::new_v4()),
            file_size: None,
            format: "srt".to_string(),
            language: Some("gre".to_string()),
            title: None,
            is_forced: false,
            is_hearing_impaired: false,
            source: "test".to_string(),
            source_id: None,
            source_metadata: None,
        },
    )
    .await
    .unwrap();
}

fn names(file: &Value, list: &str) -> Vec<Value> {
    file[list]
        .as_array()
        .unwrap_or_else(|| panic!("{file}"))
        .iter()
        .map(|t| t["language_name"].clone())
        .collect()
}

#[tokio::test]
async fn a_titles_tracks_are_named_by_their_language() {
    let server = TestServer::start_api_only().await;
    let (movie_id, movie_file) = seed_movie(&server).await;
    add_tracks(&server, Some(movie_file), None).await;
    let (episode_id, episode_file) = seed_episode(&server).await;
    add_tracks(&server, None, Some(episode_file)).await;

    for path in [
        format!("/api/v1/admin/movies/{movie_id}/tracks"),
        format!("/api/v1/admin/tv/episodes/{episode_id}/tracks"),
    ] {
        let resp = server.admin_client().get(&path).await;
        resp.assert_ok();
        let body: Value = resp.json();
        let file = &body["files"][0];
        assert_eq!(
            names(file, "audio_tracks"),
            [Value::from("German")],
            "{path}: {body}"
        );
        assert_eq!(
            names(file, "subtitle_tracks"),
            [Value::from("Czech"), Value::Null],
            "{path}: {body}"
        );
        assert_eq!(
            names(file, "external_subtitles"),
            [Value::from("Modern Greek")],
            "{path}: {body}"
        );
        assert_eq!(
            file["subtitle_tracks"][0]["language"], "cze",
            "{path}: {body}"
        );
    }
}
