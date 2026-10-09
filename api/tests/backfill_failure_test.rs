use crate::common::*;
use serde_json::json;
use uuid::Uuid;

/// Insert a failed job_status record for testing.
async fn insert_failed_job(pool: &sqlx::PgPool, job_type: &str, payload: serde_json::Value) {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO job_status (id, job_type, status, payload, scheduled, created_at, completed_at)
        VALUES ($1, $2, 'failed', $3, false, NOW(), NOW())
        "#,
    )
    .bind(id)
    .bind(job_type)
    .bind(&payload)
    .execute(pool)
    .await
    .unwrap();
}

/// Insert a failed job_status record with a specific completed_at offset.
async fn insert_failed_job_hours_ago(
    pool: &sqlx::PgPool,
    job_type: &str,
    payload: serde_json::Value,
    hours_ago: i64,
) {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO job_status (id, job_type, status, payload, scheduled, created_at, completed_at)
        VALUES ($1, $2, 'failed', $3, false, NOW() - make_interval(hours => $4::int), NOW() - make_interval(hours => $4::int))
        "#,
    )
    .bind(id)
    .bind(job_type)
    .bind(&payload)
    .bind(hours_ago as i32)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn test_recent_failure_count_returns_correct_count() {
    let server = TestServer::start_api_only().await;
    let book_id = Uuid::now_v7();
    let payload = json!({"book_id": book_id});

    // No failures yet
    let count = api::jobs::recent_failure_count(
        &server.pool,
        "audiobook_transcribe",
        "book_id",
        &book_id.to_string(),
        24,
    )
    .await
    .unwrap();
    assert_eq!(count, 0);

    // Add 2 recent failures
    insert_failed_job(&server.pool, "audiobook_transcribe", payload.clone()).await;
    insert_failed_job(&server.pool, "audiobook_transcribe", payload.clone()).await;

    let count = api::jobs::recent_failure_count(
        &server.pool,
        "audiobook_transcribe",
        "book_id",
        &book_id.to_string(),
        24,
    )
    .await
    .unwrap();
    assert_eq!(count, 2);

    // Add a 3rd
    insert_failed_job(&server.pool, "audiobook_transcribe", payload.clone()).await;

    let count = api::jobs::recent_failure_count(
        &server.pool,
        "audiobook_transcribe",
        "book_id",
        &book_id.to_string(),
        24,
    )
    .await
    .unwrap();
    assert_eq!(count, 3);
}

#[tokio::test]
async fn test_recent_failure_count_respects_time_window() {
    let server = TestServer::start_api_only().await;
    let book_id = Uuid::now_v7();
    let payload = json!({"book_id": book_id});

    // Add failures outside the time window (25 hours ago)
    insert_failed_job_hours_ago(&server.pool, "audiobook_transcribe", payload.clone(), 25).await;
    insert_failed_job_hours_ago(&server.pool, "audiobook_transcribe", payload.clone(), 25).await;
    insert_failed_job_hours_ago(&server.pool, "audiobook_transcribe", payload.clone(), 25).await;

    // Should be 0 — all failures are outside the 24-hour window
    let count = api::jobs::recent_failure_count(
        &server.pool,
        "audiobook_transcribe",
        "book_id",
        &book_id.to_string(),
        24,
    )
    .await
    .unwrap();
    assert_eq!(count, 0);

    // Add 1 recent failure
    insert_failed_job(&server.pool, "audiobook_transcribe", payload.clone()).await;

    let count = api::jobs::recent_failure_count(
        &server.pool,
        "audiobook_transcribe",
        "book_id",
        &book_id.to_string(),
        24,
    )
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_recent_failure_count_isolates_by_payload() {
    let server = TestServer::start_api_only().await;
    let book_a = Uuid::now_v7();
    let book_b = Uuid::now_v7();

    // 3 failures for book A
    for _ in 0..3 {
        insert_failed_job(
            &server.pool,
            "audiobook_transcribe",
            json!({"book_id": book_a}),
        )
        .await;
    }

    // 1 failure for book B
    insert_failed_job(
        &server.pool,
        "audiobook_transcribe",
        json!({"book_id": book_b}),
    )
    .await;

    let count_a = api::jobs::recent_failure_count(
        &server.pool,
        "audiobook_transcribe",
        "book_id",
        &book_a.to_string(),
        24,
    )
    .await
    .unwrap();
    assert_eq!(count_a, 3);

    let count_b = api::jobs::recent_failure_count(
        &server.pool,
        "audiobook_transcribe",
        "book_id",
        &book_b.to_string(),
        24,
    )
    .await
    .unwrap();
    assert_eq!(count_b, 1);
}

#[tokio::test]
async fn test_has_transcript_with_tag() {
    let server = TestServer::start_api_only().await;

    // Create a book and audiobook file
    let book_id = Uuid::now_v7();
    sqlx::query("INSERT INTO books (id, title) VALUES ($1, 'Test Book')")
        .bind(book_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let file_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO audiobook_files (id, book_id, file_path, file_index, duration_seconds, available, format)
        VALUES ($1, $2, '/test/file.m4b', 0, 3600.0, true, 'm4b')
        "#,
    )
    .bind(file_id)
    .bind(book_id)
    .execute(&server.pool)
    .await
    .unwrap();

    // No transcript yet
    let has = api::repositories::audiobook_transcripts::has_transcript_with_tag(
        &server.pool,
        file_id,
        "whisper-v3",
    )
    .await
    .unwrap();
    assert!(!has);

    // Insert a transcript segment with a specific config tag
    let seg_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO audiobook_transcripts (id, audiobook_file_id, segment_index, start_seconds, end_seconds, text, config_tag)
        VALUES ($1, $2, 0, 0.0, 5.0, 'hello world', 'whisper-v3')
        "#,
    )
    .bind(seg_id)
    .bind(file_id)
    .execute(&server.pool)
    .await
    .unwrap();

    // Now has transcript with that tag
    let has = api::repositories::audiobook_transcripts::has_transcript_with_tag(
        &server.pool,
        file_id,
        "whisper-v3",
    )
    .await
    .unwrap();
    assert!(has);

    // But not with a different tag
    let has = api::repositories::audiobook_transcripts::has_transcript_with_tag(
        &server.pool,
        file_id,
        "whisper-v4",
    )
    .await
    .unwrap();
    assert!(!has);
}

#[tokio::test]
async fn test_transcript_transaction_atomicity() {
    use api::repositories::audiobook_transcripts::{self, TranscriptSegmentInput, get_for_file};

    let server = TestServer::start_api_only().await;

    // Create a book and audiobook file
    let book_id = Uuid::now_v7();
    sqlx::query("INSERT INTO books (id, title) VALUES ($1, 'Test Book TX')")
        .bind(book_id)
        .execute(&server.pool)
        .await
        .unwrap();

    let file_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO audiobook_files (id, book_id, file_path, file_index, duration_seconds, available, format)
        VALUES ($1, $2, '/test/file_tx.m4b', 0, 3600.0, true, 'm4b')
        "#,
    )
    .bind(file_id)
    .bind(book_id)
    .execute(&server.pool)
    .await
    .unwrap();

    // Insert initial segments
    let initial_segments = vec![TranscriptSegmentInput {
        segment_index: 0,
        start_seconds: 0.0,
        end_seconds: 5.0,
        text: "original segment".to_string(),
        avg_logprob: None,
        no_speech_prob: None,
        compression_ratio: None,
    }];
    audiobook_transcripts::insert_segments(&server.pool, file_id, &initial_segments, Some("v1"))
        .await
        .unwrap();

    // Verify initial segments exist
    let segments = get_for_file(&server.pool, file_id).await.unwrap();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].text, "original segment");

    // Start a transaction, delete, then drop without committing (simulating failure)
    {
        let mut tx = server.pool.begin().await.unwrap();
        audiobook_transcripts::delete_for_file(&mut *tx, file_id)
            .await
            .unwrap();

        // Verify segments are gone within the transaction
        let in_tx: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM audiobook_transcripts WHERE audiobook_file_id = $1",
        )
        .bind(file_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(in_tx.0, 0);

        // Drop tx without committing — this rolls back
    }

    // Segments should still exist because the transaction was rolled back
    let segments = get_for_file(&server.pool, file_id).await.unwrap();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].text, "original segment");
}
