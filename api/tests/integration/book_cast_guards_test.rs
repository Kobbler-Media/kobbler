//! What a cast's long jobs and its start-over check before they act: a queued
//! render waits on approvals made since it was queued, an approval job on the
//! cast still being vetted, a redo on the book still having text, and a cast
//! left rendering with no render running can be rendered or redone again.

use std::path::PathBuf;

use crate::book_cast_render_test::{approve_all, id_of, render, start};
use crate::book_cast_vetting_test::{Rig, rig};
use crate::common::*;
use api::jobs::handlers::BookCastApproveHandler;
use api::repositories::book_casts::{self, CastState};
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;

async fn vetted() -> Rig {
    let rig = rig().await;
    rig.seed().await;
    approve_all(&rig, &[]).await;
    rig
}

#[tokio::test]
async fn a_queued_render_fails_on_a_voice_unapproved_since() {
    let rig = vetted().await;
    start(&rig).await.assert_status(StatusCode::ACCEPTED);
    let ann = id_of(&rig, "Ann").await;
    rig.admin()
        .delete(&rig.url(&format!("/characters/{ann}/voices/0/approval")))
        .await
        .assert_ok();
    let before = rig.speech_requests().await.len();

    render(&rig).await;

    let cast = rig.cast().await;
    assert_eq!(cast["state"], "failed");
    let error = cast["error"].as_str().unwrap();
    assert!(
        error.contains("approved") && error.contains("Ann"),
        "{error}"
    );
    assert_eq!(rig.speech_requests().await.len(), before, "nothing is read");
}

#[tokio::test]
async fn approving_all_stops_once_the_cast_is_back_in_analysis() {
    let rig = rig().await;
    rig.seed().await;
    rig.admin()
        .post(&rig.url("/voices/approve-all"))
        .await
        .assert_status(StatusCode::ACCEPTED);
    rig.admin()
        .post_json(&rig.url(""), &json!({"rerun": "bible"}))
        .await
        .assert_status(StatusCode::ACCEPTED);
    let before = rig.speech_requests().await.len();

    rig.server
        .process_pending_jobs(&BookCastApproveHandler::new(rig.server.app_state.clone()))
        .await;

    assert_eq!(
        rig.speech_requests().await.len(),
        before,
        "no take is minted"
    );
    let approved: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM book_cast_voices v JOIN book_cast_characters c ON c.id = v.character_id
         WHERE c.book_id = $1 AND v.approved_at IS NOT NULL",
    )
    .bind(rig.book_id)
    .fetch_one(&rig.server.pool)
    .await
    .unwrap();
    assert_eq!(approved, 0);
}

#[tokio::test]
async fn a_redo_that_cannot_start_keeps_the_cast_and_its_clips() {
    let rig = vetted().await;
    let clips: Vec<PathBuf> = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM user_voice_presets WHERE cast_book_id = $1 AND clip_filename IS NOT NULL",
    )
    .bind(rig.book_id)
    .fetch_all(&rig.server.pool)
    .await
    .unwrap()
    .into_iter()
    .map(|p| {
        PathBuf::from(rig.server.storage_root())
            .join("voices")
            .join(rig.admin_id.to_string())
            .join(format!("{p}.wav"))
    })
    .collect();
    assert!(!clips.is_empty() && clips.iter().all(|c| c.exists()));
    sqlx::query(
        "DELETE FROM ebook_segments WHERE book_file_id IN (SELECT id FROM book_files WHERE book_id = $1)",
    )
    .bind(rig.book_id)
    .execute(&rig.server.pool)
    .await
    .unwrap();

    rig.admin()
        .post_json(&rig.url(""), &json!({"fresh": true}))
        .await
        .assert_status(StatusCode::CONFLICT);

    assert!(
        clips.iter().all(|c| c.exists()),
        "the clips the cast still points to stay"
    );
    assert_eq!(rig.cast().await["state"], "casting");
}

#[tokio::test]
async fn a_cast_left_rendering_with_no_render_running_can_go_again() {
    for (url, body) in [("/render", json!({})), ("", json!({"fresh": true}))] {
        let rig = vetted().await;
        book_casts::set_state(&rig.server.pool, rig.book_id, CastState::Rendering, None)
            .await
            .unwrap();
        rig.admin()
            .post_json(&rig.url(url), &body)
            .await
            .assert_status(StatusCode::ACCEPTED);
    }
    let rig = vetted().await;
    start(&rig).await.assert_status(StatusCode::ACCEPTED);
    start(&rig).await.assert_status(StatusCode::CONFLICT);
}
