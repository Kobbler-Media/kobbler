use crate::common::server::{TestServer, admin_connect, cleanup_stale_databases};
use sqlx::Executor;

async fn exists(name: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)")
        .bind(name)
        .fetch_one(&mut admin_connect().await)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_sweep_spares_an_old_database_whose_run_is_still_going_and_drops_a_finished_one() {
    let alive = format!("kobbler_tpl_1000000000_{}", std::process::id());
    let finished = "kobbler_tpl_1000000000_4194305".to_string();
    let mut conn = admin_connect().await;
    for name in [&alive, &finished] {
        conn.execute(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {name}"
        )))
        .await
        .unwrap();
        conn.execute(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .await
            .unwrap();
    }

    cleanup_stale_databases(&mut conn).await;

    let kept = exists(&alive).await;
    conn.execute(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS {alive}"
    )))
    .await
    .unwrap();
    assert!(kept, "a database of a run still in progress was dropped");
    assert!(!exists(&finished).await);
}

#[tokio::test]
async fn a_test_servers_database_is_dropped_with_it() {
    let server = TestServer::start_api_only().await;
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&server.pool)
        .await
        .unwrap();

    drop(server);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while exists(&name).await {
        assert!(
            std::time::Instant::now() < deadline,
            "{name} outlived its TestServer"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
