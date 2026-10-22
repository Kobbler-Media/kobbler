//! Every way a book is wanted or searched for sends the names it is published
//! under: Hardcover's edition titles (fetched once, through the cache), the
//! series name with its position, and the title without its series prefix.

use crate::common::*;
use api::jobs::handlers::BookWantHandler;
use api::jobs::worker::JobHandler;
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const EXPECTED: &[&str] = &[
    "Mistborn 1",
    "Mistborn Book 1",
    "Mistborn: The Final Empire",
    "Final Empire",
    "The Final Empire: Tenth Anniversary Edition",
];

struct Rig {
    server: TestServer,
    kobload: MockServer,
    hardcover: MockServer,
}

async fn rig() -> Rig {
    let kobload = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "results": [] })))
        .mount(&kobload)
        .await;

    let hardcover = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/graphql"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "data": { "books_by_pk": {
                "title": "The Final Empire",
                "subtitle": null,
                "editions": [
                    { "title": "Mistborn: The Final Empire", "subtitle": null },
                    { "title": "The Final Empire", "subtitle": "Tenth Anniversary Edition" },
                ],
            }}})),
        )
        .mount(&hardcover)
        .await;

    let server = TestServer::start_api_only().await;
    configure_kobload_at(&server, &kobload.uri()).await;
    sqlx::query(
        "UPDATE metadata_providers
         SET config = jsonb_build_object('api_key', 'test-hc-key', 'base_url', $1), enabled = true
         WHERE provider_type = 'hardcover'",
    )
    .bind(hardcover.uri())
    .execute(&server.pool)
    .await
    .unwrap();
    Rig {
        server,
        kobload,
        hardcover,
    }
}

impl Rig {
    async fn sent_to_kobload(&self, route: &str) -> Value {
        let requests = self.kobload.received_requests().await.unwrap();
        let last = requests
            .iter()
            .rev()
            .find(|r| r.url.path() == route)
            .unwrap_or_else(|| panic!("nothing sent to {route}"));
        serde_json::from_slice::<Value>(&last.body).unwrap()["media_want"].clone()
    }

    async fn hardcover_calls(&self) -> usize {
        self.hardcover.received_requests().await.unwrap().len()
    }
}

fn assert_alternates(entry: &str, want: &Value) {
    let titles: Vec<&str> = want["alternate_titles"]
        .as_array()
        .unwrap_or_else(|| panic!("{entry}: no alternate_titles in {want}"))
        .iter()
        .map(|t| t.as_str().unwrap())
        .collect();
    for title in EXPECTED {
        assert!(titles.contains(title), "{entry}: {title} not in {titles:?}");
    }
    for never in ["Mistborn", "The Final Empire"] {
        assert!(!titles.contains(&never), "{entry}: {never} in {titles:?}");
    }
}

#[tokio::test]
async fn book_wants_carry_edition_and_series_titles() {
    let rig = rig().await;

    let created = rig
        .server
        .admin_client()
        .post_json(
            "/api/v1/library/books",
            &json!({
                "title": "The Final Empire",
                "authors": [{ "name": "Brandon Sanderson" }],
                "series_name": "Mistborn",
                "series_index": 1.0,
                "identifiers": { "hardcover": "1234" },
                "want": true,
                "want_format": "ebook",
            }),
        )
        .await;
    created.assert_success();
    let book: Uuid = created.json::<Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let handler = BookWantHandler::new(
        rig.server.pool.clone(),
        rig.server.runtime_config.clone(),
        rig.server.nats_publisher.clone(),
        rig.server.library_events.clone(),
    );
    let tip = stream_tip(&rig.server).await;
    rig.server.process_pending_jobs(&handler).await;
    let sent = want_requests_since(&rig.server, tip, &book.to_string()).await;
    let library_add = sent.last().expect("adding a wanted book sent nothing")["media_want"].clone();

    rig.server
        .admin_client()
        .get(&format!("/api/v1/admin/books/{book}/releases"))
        .await
        .assert_ok();
    let release_search = rig.sent_to_kobload("/api/v1/search").await;

    let wanted = rig
        .server
        .admin_client()
        .post_json(
            "/api/v1/admin/wanted",
            &json!({
                "title": "The Final Empire",
                "author": "Brandon Sanderson",
                "series": "Mistborn",
                "series_index": 1.0,
                "identifiers": { "hardcover": "1234" },
            }),
        )
        .await;
    wanted.assert_ok();
    let wanted_id = wanted.json::<Value>()["id"].as_str().unwrap().to_string();
    let tip = stream_tip(&rig.server).await;
    rig.server.process_pending_jobs(&handler).await;
    let sent = want_requests_since(&rig.server, tip, &wanted_id).await;
    let create_wanted =
        sent.last().expect("creating a wanted book sent nothing")["media_want"].clone();

    for (entry, want) in [
        ("library add", library_add),
        ("release search", release_search),
        ("create wanted", create_wanted),
    ] {
        assert_alternates(entry, &want);
    }
    assert_eq!(
        rig.hardcover_calls().await,
        1,
        "Hardcover is asked once per book"
    );
}
