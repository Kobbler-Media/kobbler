//! Test helper functions for creating file records.
//!
//! Since `is_in_library` is now derived from file existence, tests that need
//! a book/movie/episode to appear "in library" must create actual file records.

use api::repositories::{book_files, movie_files, tv_episode_files};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

/// Create a minimal book file record so the book appears in-library.
pub async fn create_stub_book_file(pool: &PgPool, book_id: Uuid) -> Uuid {
    book_files::create(
        pool,
        book_files::BookFileInput {
            book_id,
            format: "EPUB".to_string(),
            size_bytes: 1024,
            file_path: format!("/test/books/{book_id}.epub"),
            source_type: Some("test".to_string()),
            source_metadata: Some(json!({})),
            ..Default::default()
        },
    )
    .await
    .expect("create_stub_book_file failed")
}

/// Create a minimal movie file record so the movie appears in-library.
pub async fn create_stub_movie_file(pool: &PgPool, movie_id: Uuid) -> Uuid {
    movie_files::create(
        pool,
        movie_files::MovieFileInput {
            movie_id,
            file_path: format!("/test/movies/{movie_id}.mkv"),
            file_size: Some(1024),
            format: "mkv".to_string(),
            ..Default::default()
        },
    )
    .await
    .expect("create_stub_movie_file failed")
}

/// Create a minimal episode file and link it so the episode appears in-library.
pub async fn create_stub_episode_file(pool: &PgPool, episode_id: Uuid) -> Uuid {
    let file_id = tv_episode_files::create(
        pool,
        tv_episode_files::EpisodeFileInput {
            file_path: format!("/test/episodes/{episode_id}.mkv"),
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
            duration_seconds: None,
            container: None,
            overall_bitrate_kbps: None,
        },
    )
    .await
    .expect("create_stub_episode_file failed");
    tv_episode_files::link_episodes(pool, file_id, &[episode_id])
        .await
        .expect("link_episodes failed");
    file_id
}
