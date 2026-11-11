//! Every test runs on a runtime of its own, and wiremock hands one test's server
//! to the next. A shared client must not lend a test a connection that another
//! test's runtime drives.

use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::routing::get;

fn serve() -> (tokio::runtime::Runtime, SocketAddr) {
    let server = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let addr = server.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/", get(|| async { "ok" })))
                .await
                .unwrap();
        });
        addr
    });
    (server, addr)
}

async fn fetch(client: reqwest::Client, addr: SocketAddr) -> Result<String, String> {
    let resp = client
        .get(format!("http://{addr}/"))
        .send()
        .await
        .map_err(|e| format!("{e:?}"))?;
    resp.text().await.map_err(|e| format!("{e:?}"))
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// The first runtime stays alive but idle, as a test's does while its thread is
/// blocked dropping its database, so a connection it opened looks reusable.
#[test]
fn a_shared_client_answers_on_a_runtime_while_another_that_used_it_is_idle() {
    let (_server, addr) = serve();
    for (name, client) in [
        (
            "new_client",
            api::http::new_client as fn() -> reqwest::Client,
        ),
        ("new_streaming_client", api::http::new_streaming_client),
    ] {
        let idle = runtime();
        assert_eq!(idle.block_on(fetch(client(), addr)).as_deref(), Ok("ok"));

        let answer = runtime().block_on(async {
            tokio::time::timeout(Duration::from_secs(5), fetch(client(), addr)).await
        });
        assert_eq!(
            answer
                .map_err(|_| "timed out".to_string())
                .flatten()
                .as_deref(),
            Ok("ok"),
            "{name}"
        );
        drop(idle);
    }
}
