use crate::common::*;

#[tokio::test]
async fn pool_connections_run_without_jit() {
    let server = TestServer::start_api_only().await;
    let jit: String = sqlx::query_scalar("SHOW jit")
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(jit, "off");
}
