//! Tests for the author_sync_check scheduled job, which runs immediately on
//! every boot (see nats.rs's "author-sync-check" registration).

use crate::common::*;
use api::jobs::handlers::AuthorSyncCheckHandler;
use api::jobs::worker::JobHandler;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[tokio::test]
async fn an_empty_library_is_a_quiet_no_op_even_without_a_tracking_key() {
    let server = TestServer::start_api_only().await;
    // No hardcover_api_key configured, no authors in the DB — this is the
    // state of every fresh install on its first boot.

    let handler = AuthorSyncCheckHandler::new(server.pool.clone(), server.nats_publisher.clone());

    let result = handler
        .execute(Uuid::now_v7(), json!({}), CancellationToken::new())
        .await;

    assert!(
        result.is_ok(),
        "a fresh install with nothing to check must not fail just because no \
         tracking key is configured: {:?}",
        result.err()
    );
}
