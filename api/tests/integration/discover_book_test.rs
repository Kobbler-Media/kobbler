use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::matchers::{body_partial_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Rig {
    server: TestServer,
    upstream: MockServer,
}

async fn rig() -> Rig {
    let server = TestServer::start_api_only().await;
    let upstream = MockServer::start().await;
    sqlx::query(
        "UPDATE metadata_providers
         SET config = config || jsonb_build_object('api_key', 'k', 'base_url', $1), enabled = true",
    )
    .bind(upstream.uri())
    .execute(&server.pool)
    .await
    .unwrap();
    Rig { server, upstream }
}

fn detail_path(provider: &str, id: &str) -> String {
    format!(
        "/api/v1/admin/discover/book/{provider}/{}",
        urlencoding::encode(id)
    )
}

impl Rig {
    async fn mount_hardcover(&self) {
        Mock::given(method("POST"))
            .and(path("/v1/graphql"))
            .and(body_partial_json(json!({ "variables": { "id": 427 } })))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({ "data": { "books_by_pk": {
                    "id": 427,
                    "title": "The Final Empire",
                    "slug": "the-final-empire",
                    "description": "Ash falls.",
                    "image": { "url": "https://img.hardcover.app/427.jpg" },
                    "contributions": [{ "author": { "id": 1, "name": "Brandon Sanderson" } }],
                    "book_series": [{ "position": 1.0, "series": { "id": 9, "name": "Mistborn" } }],
                    "pages": 541,
                    "release_year": 2006,
                    "release_date": "2006-07-17",
                    "rating": 4.4,
                    "ratings_count": 1200,
                    "taggings": [{ "tag": { "tag": "Fantasy" } }],
                    "isbn_editions": [{ "isbn_13": "9780765311788", "isbn_10": "076531178X" }],
                    "audiobook_editions": [{ "asin": "B002UZMLXM", "audio_seconds": 89000 }]
                }}}),
            ))
            .mount(&self.upstream)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/graphql"))
            .and(body_partial_json(json!({ "variables": { "id": 404 } })))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "data": { "books_by_pk": null } })),
            )
            .mount(&self.upstream)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/graphql"))
            .and(body_partial_json(json!({ "variables": { "id": 500 } })))
            .respond_with(ResponseTemplate::new(500))
            .mount(&self.upstream)
            .await;
    }

    async fn mount_openlibrary(&self) {
        Mock::given(method("GET"))
            .and(path("/works/OL45883W.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "key": "/works/OL45883W",
                "title": "Fantastic Mr Fox",
                "description": { "type": "/type/text", "value": "Three farmers." },
                "covers": [6498519],
                "subjects": ["Foxes"],
                "first_publish_date": "1970"
            })))
            .mount(&self.upstream)
            .await;
        Mock::given(method("GET"))
            .and(path("/search.json"))
            .and(query_param("q", "key:/works/OL45883W"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "num_found": 1,
                "docs": [{
                    "key": "/works/OL45883W",
                    "title": "Fantastic Mr Fox",
                    "author_name": ["Roald Dahl"],
                    "isbn": ["9780142410349"],
                    "first_publish_year": 1970,
                    "subject": ["Foxes", "Farmers"],
                    "cover_i": 6498519
                }]
            })))
            .mount(&self.upstream)
            .await;
    }

    async fn mount_googlebooks(&self) {
        Mock::given(method("GET"))
            .and(path("/books/v1/volumes/zyTCAlFPjgYC"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "zyTCAlFPjgYC",
                "volumeInfo": {
                    "title": "The Google Story",
                    "authors": ["David A. Vise", "Mark Malseed"],
                    "publishedDate": "2005-11-15",
                    "description": "Inside the company.",
                    "industryIdentifiers": [
                        { "type": "ISBN_10", "identifier": "055380457X" },
                        { "type": "ISBN_13", "identifier": "9780553804577" }
                    ],
                    "categories": ["Business"],
                    "averageRating": 3.5,
                    "ratingsCount": 136,
                    "imageLinks": { "thumbnail": "http://books.google.com/t.jpg" }
                }
            })))
            .mount(&self.upstream)
            .await;
    }

    async fn mount_audnexus(&self) {
        Mock::given(method("GET"))
            .and(path("/books/B002V0QK4C"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "asin": "B002V0QK4C",
                "title": "The Hobbit",
                "authors": [{ "name": "J.R.R. Tolkien" }],
                "narrators": [{ "name": "Rob Inglis" }],
                "description": "Bilbo goes there and back again.",
                "releaseDate": "2012-09-20T00:00:00.000Z",
                "runtimeLengthMin": 662,
                "image": "https://m.media-amazon.com/hobbit.jpg",
                "genres": [{ "name": "Fantasy" }],
                "rating": "4.8"
            })))
            .mount(&self.upstream)
            .await;
    }

    async fn detail(&self, provider: &str, id: &str) -> Value {
        let resp = self
            .server
            .admin_client()
            .get(&detail_path(provider, id))
            .await;
        resp.assert_ok();
        resp.json()
    }

    async fn want(&self, detail: &Value) -> Uuid {
        let d = &detail["details"];
        let resp = self
            .server
            .admin_client()
            .post_json(
                "/api/v1/library/books",
                &json!({
                    "title": d["title"],
                    "want": true,
                    "identifiers": d["identifiers"],
                }),
            )
            .await;
        resp.assert_success();
        resp.json::<Value>()["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    }
}

struct Case {
    provider: &'static str,
    id: &'static str,
    title: &'static str,
    authors: &'static [&'static str],
    year: i64,
    release_date: Option<&'static str>,
    identifier: (&'static str, &'static str),
}

const CASES: &[Case] = &[
    Case {
        provider: "hardcover",
        id: "427",
        title: "The Final Empire",
        authors: &["Brandon Sanderson"],
        year: 2006,
        release_date: Some("2006-07-17"),
        identifier: ("isbn13", "9780765311788"),
    },
    Case {
        provider: "openlibrary",
        id: "/works/OL45883W",
        title: "Fantastic Mr Fox",
        authors: &["Roald Dahl"],
        year: 1970,
        release_date: None,
        identifier: ("isbn13", "9780142410349"),
    },
    Case {
        provider: "googlebooks",
        id: "zyTCAlFPjgYC",
        title: "The Google Story",
        authors: &["David A. Vise", "Mark Malseed"],
        year: 2005,
        release_date: Some("2005-11-15"),
        identifier: ("isbn10", "055380457X"),
    },
    Case {
        provider: "audnexus",
        id: "B002V0QK4C",
        title: "The Hobbit",
        authors: &["J.R.R. Tolkien"],
        year: 2012,
        release_date: Some("2012-09-20T00:00:00.000Z"),
        identifier: ("asin", "B002V0QK4C"),
    },
];

async fn mounted_rig() -> Rig {
    let rig = rig().await;
    rig.mount_hardcover().await;
    rig.mount_openlibrary().await;
    rig.mount_googlebooks().await;
    rig.mount_audnexus().await;
    rig
}

#[tokio::test]
async fn book_detail_from_each_provider() {
    let rig = mounted_rig().await;
    for case in CASES {
        let body = rig.detail(case.provider, case.id).await;
        let d = &body["details"];
        let who = case.provider;
        assert_eq!(body["media_type"], "book", "{who}");
        assert_eq!(body["provider"], case.provider, "{who}");
        assert_eq!(body["library_id"], Value::Null, "{who}");
        assert_eq!(body["is_in_library"], Value::Null, "{who}");
        assert_eq!(d["title"], case.title, "{who}");
        assert_eq!(d["authors"], json!(case.authors), "{who}");
        assert_eq!(d["year"], case.year, "{who}");
        assert_eq!(d["release_date"], json!(case.release_date), "{who}");
        assert!(
            d["description"].as_str().is_some_and(|s| !s.is_empty()),
            "{who}"
        );
        assert!(
            d["cover_url"]
                .as_str()
                .is_some_and(|s| s.starts_with("https://")),
            "{who}"
        );
        assert!(
            d["subjects"].as_array().is_some_and(|s| !s.is_empty()),
            "{who}"
        );
        assert_eq!(d["identifiers"][case.provider], case.id, "{who}");
        assert_eq!(
            d["identifiers"][case.identifier.0], case.identifier.1,
            "{who}"
        );
    }
}

#[tokio::test]
async fn book_detail_carries_ratings_and_series() {
    let rig = mounted_rig().await;
    let d = rig.detail("hardcover", "427").await["details"].clone();
    assert_eq!(d["rating"], 4.4);
    assert_eq!(d["rating_count"], 1200);
    assert_eq!(d["series"], "Mistborn");
    assert_eq!(d["identifiers"]["hardcover_slug"], "the-final-empire");
    assert_eq!(d["identifiers"]["asin"], "B002UZMLXM");
}

#[tokio::test]
async fn wanting_a_book_detail_links_it_to_the_library_book() {
    let rig = mounted_rig().await;
    for case in CASES {
        let who = case.provider;
        let id = rig.want(&rig.detail(case.provider, case.id).await).await;
        let body = rig.detail(case.provider, case.id).await;
        assert_eq!(body["library_id"], id.to_string(), "{who}");
        assert_eq!(body["want_status"], "wanted", "{who}");
        assert_eq!(body["is_in_library"], false, "{who}");
    }

    let id = rig.want(&rig.detail("hardcover", "427").await).await;
    sqlx::query("DELETE FROM books WHERE id <> $1 AND identifiers->>'hardcover' = '427'")
        .bind(id)
        .execute(&rig.server.pool)
        .await
        .unwrap();
    create_stub_book_file(&rig.server.pool, id).await;
    let body = rig.detail("hardcover", "427").await;
    assert_eq!(body["library_id"], id.to_string());
    assert_eq!(body["is_in_library"], true);
}

#[tokio::test]
async fn book_detail_unknown_provider_is_bad_request() {
    let rig = rig().await;
    rig.server
        .admin_client()
        .get(&detail_path("goodreads", "1"))
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn book_detail_missing_book_is_not_found() {
    let rig = mounted_rig().await;
    for (provider, id) in [
        ("hardcover", "404"),
        ("hardcover", "not-a-number"),
        ("openlibrary", "/works/OL1W"),
        ("googlebooks", "missing"),
        ("audnexus", "B000000000"),
    ] {
        let resp = rig
            .server
            .admin_client()
            .get(&detail_path(provider, id))
            .await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND, "{provider}/{id}");
    }
}

#[tokio::test]
async fn book_detail_provider_failure_is_bad_gateway() {
    let rig = mounted_rig().await;
    rig.server
        .admin_client()
        .get(&detail_path("hardcover", "500"))
        .await
        .assert_status(StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn book_detail_requires_auth() {
    let rig = mounted_rig().await;
    rig.server
        .api_client()
        .get(&detail_path("hardcover", "427"))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}
