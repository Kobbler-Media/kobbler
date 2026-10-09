//! Tests for the discover_sync scheduled job, which runs on every boot.
//!
//! TMDB is seeded `enabled = true` with an empty config by migration
//! 0049_video_foundation.sql, so every fresh install starts with it "enabled"
//! but unconfigured — this must not be treated as a real failure.

use crate::common::*;
use api::jobs::handlers::DiscoverSyncHandler;
use api::jobs::worker::JobHandler;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[tokio::test]
async fn tmdb_enabled_with_no_api_key_is_a_quiet_no_op() {
    let server = TestServer::start_api_only().await;
    // Fresh DB: TMDB is `enabled = true` (migration default) with `config = '{}'`
    // — no api_key. This is every fresh install's state, not a misconfiguration.

    let handler = DiscoverSyncHandler::new(server.pool.clone(), server.runtime_config.clone());

    let result = handler
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await
        .expect("the job itself always succeeds; the bug is what it reports");

    let errors = result["errors"].as_array().expect("errors is an array");
    assert!(
        errors.is_empty(),
        "TMDB enabled-but-unconfigured on a fresh install must not be reported \
         as a per-section error, one per discover category: {errors:?}"
    );
}
