use crate::common::server::{admin_connect, cleanup_stale_databases, get_admin_pool};
use sqlx::Executor;

async fn exists(name: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)")
        .bind(name)
        .fetch_one(get_admin_pool().await)
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

    cleanup_stale_databases(get_admin_pool().await).await;

    let kept = exists(&alive).await;
    conn.execute(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS {alive}"
    )))
    .await
    .unwrap();
    assert!(kept, "a database of a run still in progress was dropped");
    assert!(!exists(&finished).await);
}
