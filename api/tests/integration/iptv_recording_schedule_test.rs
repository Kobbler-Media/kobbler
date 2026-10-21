use crate::common::*;
use api::repositories::iptv_recordings::{self, CreateRecording};
use time::OffsetDateTime;

async fn make_channel(pool: &sqlx::PgPool) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO iptv_channels (id, name, normalized_name, enabled) VALUES ($1, $2, $3, true)",
    )
    .bind(id)
    .bind("Sched Channel")
    .bind("sched channel")
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn schedule(
    pool: &sqlx::PgPool,
    channel_id: uuid::Uuid,
    start_in_secs: i64,
    pre_padding: i32,
) -> uuid::Uuid {
    let start = OffsetDateTime::now_utc() + time::Duration::seconds(start_in_secs);
    iptv_recordings::create(
        pool,
        CreateRecording {
            channel_id,
            epg_id: None,
            title: "Test".into(),
            start_time: start,
            end_time: start + time::Duration::seconds(600),
            pre_padding_secs: Some(pre_padding),
            post_padding_secs: Some(0),
        },
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn list_due_lookahead_picks_up_recordings_before_start() {
    let server = TestServer::start_api_only().await;
    let channel_id = make_channel(&server.pool).await;

    // Capture-start is 90s out (120s start - 30s pre-padding).
    let rec = schedule(&server.pool, channel_id, 120, 30).await;
    let now = OffsetDateTime::now_utc();

    // No lookahead: not yet due, capture-start is in the future.
    let due_now = iptv_recordings::list_due(&server.pool, now, std::time::Duration::ZERO)
        .await
        .unwrap();
    assert!(
        !due_now.iter().any(|r| r.id == rec),
        "recording should not be due without lookahead"
    );

    // 120s lookahead: picked up early so the handler can wait for the mark.
    let due_ahead =
        iptv_recordings::list_due(&server.pool, now, std::time::Duration::from_secs(120))
            .await
            .unwrap();
    assert!(
        due_ahead.iter().any(|r| r.id == rec),
        "recording should be due within the lookahead window"
    );
}

#[tokio::test]
async fn list_due_ignores_recordings_beyond_lookahead() {
    let server = TestServer::start_api_only().await;
    let channel_id = make_channel(&server.pool).await;

    // Capture-start is ~590s out — far beyond a 120s lookahead.
    let rec = schedule(&server.pool, channel_id, 600, 10).await;
    let due = iptv_recordings::list_due(
        &server.pool,
        OffsetDateTime::now_utc(),
        std::time::Duration::from_secs(120),
    )
    .await
    .unwrap();
    assert!(!due.iter().any(|r| r.id == rec));
}

#[tokio::test]
async fn try_claim_for_recording_is_single_winner() {
    let server = TestServer::start_api_only().await;
    let channel_id = make_channel(&server.pool).await;
    let rec = schedule(&server.pool, channel_id, 0, 0).await;

    // First claim wins; a duplicate enqueue loses.
    assert!(
        iptv_recordings::try_claim_for_recording(&server.pool, rec)
            .await
            .unwrap()
    );
    assert!(
        !iptv_recordings::try_claim_for_recording(&server.pool, rec)
            .await
            .unwrap()
    );

    // Once claimed (status != scheduled) it is no longer listed as due.
    let due = iptv_recordings::list_due(
        &server.pool,
        OffsetDateTime::now_utc(),
        std::time::Duration::from_secs(120),
    )
    .await
    .unwrap();
    assert!(!due.iter().any(|r| r.id == rec));
}
