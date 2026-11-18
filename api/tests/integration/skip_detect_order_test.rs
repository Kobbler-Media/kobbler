//! Season detection reads its episodes in episode order.
//!
//! The cross-episode pickers break ties toward the earlier candidate, and the
//! partner indices in every pair table are positions in this list, so an
//! order that follows whatever the planner returned made reruns of the same
//! season disagree. Files are created here in reverse episode order, which is
//! also the order of their UUIDv7 ids.

use crate::common::*;
use api::media_cache::episode_cache::{MediaKind, episode_cache_dir};
use api::media_cache::writers::{new_manifest_at, write_manifest_at};
use api::repositories::{debrid_files, tv_episode_files};
use api::skip_detect::cache_detect::season_episode_files;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

/// Episodes 1..=count, whose files are created last episode first. Returns
/// (episode ids, file ids), both in episode order.
async fn seed_reversed_season(pool: &PgPool, count: usize) -> (Vec<Uuid>, Vec<Uuid>) {
    let show_id = Uuid::now_v7();
    sqlx::query("INSERT INTO tv_shows (id, title) VALUES ($1, 'Order Test')")
        .bind(show_id)
        .execute(pool)
        .await
        .unwrap();
    let season_id = Uuid::now_v7();
    sqlx::query(
        r#"INSERT INTO tv_seasons (id, show_id, season_identifier, season_sort_order, is_monitored, is_special)
           VALUES ($1, $2, '1', 1, true, false)"#,
    )
    .bind(season_id)
    .bind(show_id)
    .execute(pool)
    .await
    .unwrap();

    let mut episode_ids = Vec::new();
    for i in 1..=count {
        let ep_id = Uuid::now_v7();
        sqlx::query(
            r#"INSERT INTO tv_episodes (id, season_id, episode_identifier, episode_sort_order, title)
               VALUES ($1, $2, $3, $4, $5)"#,
        )
        .bind(ep_id)
        .bind(season_id)
        .bind(i.to_string())
        .bind(i as i32)
        .bind(format!("Episode {i}"))
        .execute(pool)
        .await
        .unwrap();
        episode_ids.push(ep_id);
    }

    let mut file_ids = vec![Uuid::nil(); count];
    for i in (0..count).rev() {
        let file_id = tv_episode_files::create(
            pool,
            tv_episode_files::EpisodeFileInput {
                file_path: format!("/nonexistent/order/s01e{:02}.mkv", i + 1),
                file_size: Some(1024),
                file_fingerprint: None,
                format: "mkv".to_string(),
                quality: None,
                languages: None,
                subtitles: None,
                source_type: Some("test".to_string()),
                source_metadata: Some(json!({})),
                video_codec: None,
                video_width: None,
                video_height: None,
                video_bitrate_kbps: None,
                framerate: None,
                bit_depth: None,
                is_hdr: false,
                is_interlaced: false,
                hdr_format: None,
                duration_seconds: Some(1800.0),
                container: None,
                overall_bitrate_kbps: None,
            },
        )
        .await
        .unwrap();
        tv_episode_files::link_episodes(pool, file_id, &[episode_ids[i]])
            .await
            .unwrap();
        file_ids[i] = file_id;
    }
    (episode_ids, file_ids)
}

async fn season_of(pool: &PgPool, episode_id: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT season_id FROM tv_episodes WHERE id = $1")
        .bind(episode_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn season_files_list_in_episode_order() {
    let server = TestServer::start_api_only().await;
    let (episode_ids, file_ids) = seed_reversed_season(&server.pool, 4).await;
    let season_id = season_of(&server.pool, episode_ids[0]).await;

    let listed = tv_episode_files::list_file_ids_by_season(&server.pool, season_id)
        .await
        .unwrap();
    assert_eq!(listed, file_ids);
}

/// The download being processed stands for its own episode; its siblings are matched
/// through their library files.
#[tokio::test]
async fn debrid_season_episode_files_are_in_episode_order() {
    let server = TestServer::start_api_only().await;
    let storage_root = server.storage_root();
    let (episode_ids, file_ids) = seed_reversed_season(&server.pool, 4).await;
    let streamed = debrid_files::upsert(&server.pool, "/debrid/e3.mkv", "episode", episode_ids[2])
        .await
        .unwrap();
    let kinds = file_ids
        .iter()
        .map(|file_id| MediaKind::TvFile { file_id: *file_id })
        .chain([MediaKind::DebridFile {
            file_id: streamed.id,
        }]);
    for kind in kinds {
        let dir = episode_cache_dir(&storage_root, &kind);
        std::fs::create_dir_all(&dir).unwrap();
        write_manifest_at(&dir, &new_manifest_at(&dir, 1800.0)).unwrap();
    }

    let listed: Vec<Uuid> = season_episode_files(&server.pool, &storage_root, &streamed)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.file_id)
        .collect();
    assert_eq!(
        listed,
        vec![file_ids[0], file_ids[1], streamed.id, file_ids[3]]
    );
}

/// Chapter titles are evidence for detection; a debrid sibling's come from its download.
#[tokio::test]
async fn season_chapters_come_from_library_files_and_debrid_downloads_alike() {
    use api::routes::admin::streaming::ChapterResponse;
    use api::skip_detect::cache_detect::season_chapters;
    use api::skip_detect::tv::EpisodeFile;
    let server = TestServer::start_api_only().await;
    let (episode_ids, file_ids) = seed_reversed_season(&server.pool, 2).await;
    sqlx::query(
        "INSERT INTO video_chapters (episode_file_id, chapter_index, title, start_seconds, end_seconds) \
         VALUES ($1, 0, 'Library Intro', 30.0, 90.0)",
    )
    .bind(file_ids[0])
    .execute(&server.pool)
    .await
    .unwrap();
    let streamed = debrid_files::upsert(&server.pool, "/debrid/e2.mkv", "episode", episode_ids[1])
        .await
        .unwrap();
    debrid_files::set_chapters(
        &server.pool,
        streamed.id,
        &[ChapterResponse {
            index: 0,
            title: Some("Opening Credits".to_string()),
            start_seconds: 12.0,
            end_seconds: 70.0,
        }],
    )
    .await
    .unwrap();

    let file = |file_id, cache| EpisodeFile {
        file_id,
        file_path: String::new(),
        duration_seconds: 1800.0,
        cache,
    };
    let chapters = season_chapters(
        &server.pool,
        &[
            file(
                file_ids[0],
                MediaKind::TvFile {
                    file_id: file_ids[0],
                },
            ),
            file(
                streamed.id,
                MediaKind::DebridFile {
                    file_id: streamed.id,
                },
            ),
        ],
    )
    .await;
    assert_eq!(
        chapters[&file_ids[0]],
        vec![(30.0, Some("Library Intro".to_string()))]
    );
    assert_eq!(
        chapters[&streamed.id],
        vec![(12.0, Some("Opening Credits".to_string()))]
    );
}
