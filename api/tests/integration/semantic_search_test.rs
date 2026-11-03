//! Semantic search: artwork vectors from koblem, hybrid queries to Meilisearch.
//!
//! Meilisearch and koblem are both wiremocks. The real-Meili behaviour these stubs
//! mirror — a `userProvided` embedder rejecting any batch with a document missing
//! `_vectors.poster`, and refusing to be declared while such documents exist — is why
//! the job writes documents before the embedder and why every title document carries
//! the field, null included, while semantic search is on.

use std::sync::Arc;

use crate::common::*;
use api::jobs::handlers::SearchVectorsHandler;
use api::search::{MeiliState, SearchBackend};
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::matchers::{body_string_contains, method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const DIMS: usize = 1152;

// =============================================================================
// Harness
// =============================================================================

struct Rig {
    server: TestServer,
    meili: MockServer,
    koblem: MockServer,
}

async fn rig() -> Rig {
    let meili = MockServer::start().await;
    let koblem = MockServer::start().await;
    let backend: Arc<dyn SearchBackend> =
        Arc::new(MeiliState::new(&meili.uri(), "test-master-key").unwrap());
    let server = TestServer::start_api_only_with_search(backend).await;
    server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({"meili_url": meili.uri(), "meili_master_key": "test-master-key"}),
        )
        .await
        .assert_ok();
    Rig {
        server,
        meili,
        koblem,
    }
}

impl Rig {
    async fn enable(&self) {
        self.server
            .admin_client()
            .patch_json(
                "/api/v1/admin/config",
                &json!({"semantic_search_enabled": true, "koblem_service_url": self.koblem.uri()}),
            )
            .await
            .assert_ok();
    }

    async fn disable(&self) {
        self.server
            .admin_client()
            .patch_json(
                "/api/v1/admin/config",
                &json!({"semantic_search_enabled": false}),
            )
            .await
            .assert_ok();
    }

    async fn run_vector_jobs(&self) {
        let handler = SearchVectorsHandler::new(
            self.server.pool.clone(),
            self.server.search_backend_arc(),
            self.server.runtime_config.clone(),
            self.server.storage_root(),
        );
        self.server.process_pending_jobs(&handler).await;
    }

    async fn search(&self, q: &str) -> Value {
        let resp = self
            .server
            .admin_client()
            .get(&format!("/api/v1/admin/search?q={}", urlencode(q)))
            .await;
        resp.assert_ok();
        resp.json()
    }

    async fn koblem_paths(&self) -> Vec<String> {
        requests(&self.koblem)
            .await
            .into_iter()
            .map(|(_, path, _)| path)
            .collect()
    }

    async fn multi_search_queries(&self) -> Vec<Value> {
        requests(&self.meili)
            .await
            .into_iter()
            .filter(|(_, path, _)| path == "/multi-search")
            .flat_map(|(_, _, body)| body["queries"].as_array().cloned().unwrap_or_default())
            .collect()
    }
}

fn urlencode(q: &str) -> String {
    q.replace(' ', "%20")
}

async fn requests(mock: &MockServer) -> Vec<(String, String, Value)> {
    mock.received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| {
            (
                r.method.to_string(),
                r.url.path().to_string(),
                serde_json::from_slice(&r.body).unwrap_or(Value::Null),
            )
        })
        .collect()
}

fn text_vector() -> Vec<f32> {
    vec![0.01; DIMS]
}

async fn koblem_embeds_text(koblem: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/v1/text_embeddings"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"embeddings": [text_vector()]})),
        )
        .mount(koblem)
        .await;
}

/// koblem's OpenAI-shaped `/v1/embeddings`: one vector per `images` part.
struct ImageEmbeddings;

impl Respond for ImageEmbeddings {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body = String::from_utf8_lossy(&request.body);
        let images = body.matches("name=\"images\"").count();
        let data: Vec<Value> = (0..images)
            .map(|i| json!({"object": "embedding", "index": i, "embedding": vec![0.02_f32; DIMS]}))
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({"object": "list", "data": data}))
    }
}

async fn koblem_embeds_images(koblem: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ImageEmbeddings)
        .mount(koblem)
        .await;
}

fn hit(index: &str, doc: Value, details: Option<Value>) -> Value {
    let mut hit = doc;
    hit["_federation"] =
        json!({"indexUid": index, "queriesPosition": 0, "weightedRankingScore": 0.9});
    if let Some(details) = details {
        hit["_rankingScoreDetails"] = details;
    }
    hit
}

fn movie_doc(title: &str) -> Value {
    json!({
        "id": Uuid::now_v7().to_string(),
        "title": title,
        "year": 2014,
        "directors": [],
        "genres": ["Science Fiction"],
        "poster_url": null,
        "is_in_library": true,
        "want_status": "none",
        "first_letter": "I",
    })
}

async fn meili_answers_search(meili: &MockServer, hits: Vec<Value>) {
    Mock::given(method("POST"))
        .and(path("/multi-search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "hits": hits,
            "offset": 0,
            "limit": 50,
            "estimatedTotalHits": hits.len(),
            "processingTimeMs": 1,
        })))
        .mount(meili)
        .await;
}

const TS: &str = "2026-11-03T00:00:00Z";

fn task_info() -> Value {
    json!({"taskUid": 1, "indexUid": "movies", "status": "enqueued", "type": "customs", "enqueuedAt": TS})
}

/// Enough of Meili's write and task API for the vector job. `configured` is what
/// `GET .../settings/embedders` reports.
async fn meili_accepts_writes(meili: &MockServer, configured: bool) {
    let embedders = match configured {
        true => json!({"poster": {"source": "userProvided", "dimensions": DIMS}}),
        false => json!({}),
    };
    Mock::given(method("GET"))
        .and(path_regex(r"^/indexes/[a-z_]+/settings/embedders$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(embedders))
        .mount(meili)
        .await;
    for verb in ["PATCH", "DELETE", "PUT", "POST"] {
        Mock::given(method(verb))
            .and(path_regex(
                r"^/indexes/[a-z_]+/(settings/embedders|documents)$",
            ))
            .respond_with(ResponseTemplate::new(202).set_body_json(task_info()))
            .mount(meili)
            .await;
    }
    Mock::given(method("GET"))
        .and(path_regex(r"^/tasks/\d+$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "uid": 1, "indexUid": "movies", "status": "succeeded", "type": "customs",
            "duration": "PT0.1S", "enqueuedAt": TS, "startedAt": TS, "finishedAt": TS,
        })))
        .mount(meili)
        .await;
}

async fn movie_with_cached_poster(server: &TestServer, title: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO movies (id, title, year, images) VALUES ($1, $2, 2014, \
         jsonb_build_object('poster', jsonb_build_object('source_url', 'http://127.0.0.1:9/p.jpg', 'cached_at', $3::text)))",
    )
    .bind(id)
    .bind(title)
    .bind(TS)
    .execute(&server.pool)
    .await
    .unwrap();
    let dir = std::path::Path::new(&server.storage_root()).join("images/movies");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(id.to_string()), test_jpeg_image()).unwrap();
    id
}

async fn movie_without_poster(server: &TestServer, title: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO movies (id, title, year) VALUES ($1, $2, 2001)")
        .bind(id)
        .bind(title)
        .execute(&server.pool)
        .await
        .unwrap();
    id
}

async fn stored_vector_count(server: &TestServer) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM search_embeddings")
        .fetch_one(&server.pool)
        .await
        .unwrap()
}

async fn pending_vector_jobs(server: &TestServer) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM job_status WHERE job_type = 'search_vectors' AND status = 'pending'",
    )
    .fetch_one(&server.pool)
    .await
    .unwrap()
}

fn is_title_index(q: &Value) -> bool {
    matches!(
        q["indexUid"].as_str(),
        Some("movies" | "tv_shows" | "books")
    )
}

// =============================================================================
// Unified search
// =============================================================================

#[tokio::test]
async fn search_is_keyword_only_while_semantic_search_is_off() {
    let rig = rig().await;
    koblem_embeds_text(&rig.koblem).await;
    meili_answers_search(&rig.meili, vec![]).await;

    rig.search("space adventure").await;

    let queries = rig.multi_search_queries().await;
    assert!(!queries.is_empty());
    for q in &queries {
        assert!(
            q.get("hybrid").is_none() && q.get("vector").is_none(),
            "{q}"
        );
    }
    assert!(
        rig.koblem_paths().await.is_empty(),
        "koblem was asked to embed"
    );
}

#[tokio::test]
async fn title_queries_become_hybrid_and_weak_artwork_matches_are_dropped() {
    let rig = rig().await;
    rig.enable().await;
    koblem_embeds_text(&rig.koblem).await;
    let keyword = movie_doc("Space Station 76");
    let strong = movie_doc("Interstellar");
    let weak = movie_doc("Downton Abbey");
    meili_answers_search(
        &rig.meili,
        vec![
            hit(
                "movies",
                keyword.clone(),
                Some(json!({"words": {"order": 0, "score": 1.0}})),
            ),
            hit(
                "movies",
                strong.clone(),
                Some(json!({"vectorSort": {"order": 0, "similarity": 0.82}})),
            ),
            hit(
                "movies",
                weak.clone(),
                Some(json!({"vectorSort": {"order": 0, "similarity": 0.04}})),
            ),
        ],
    )
    .await;

    let result = rig.search("space adventure").await;

    let queries = rig.multi_search_queries().await;
    let (titles, others): (Vec<_>, Vec<_>) = queries.iter().partition(|q| is_title_index(q));
    assert_eq!(titles.len(), 3, "{queries:?}");
    for q in titles {
        assert_eq!(
            q["hybrid"],
            json!({"embedder": "poster", "semanticRatio": 0.5}),
            "{q}"
        );
        assert_eq!(q["vector"].as_array().map(Vec::len), Some(DIMS), "{q}");
        assert_eq!(q["q"], "space adventure");
    }
    for q in others {
        assert!(q.get("hybrid").is_none(), "{q}");
    }
    assert_eq!(rig.koblem_paths().await, vec!["/v1/text_embeddings"]);

    let titles: Vec<&str> = result["top_hits"]
        .as_array()
        .unwrap()
        .iter()
        .chain(result["remaining"].as_array().unwrap())
        .filter_map(|h| h["title"].as_str())
        .collect();
    assert_eq!(titles, vec!["Space Station 76", "Interstellar"]);
}

#[tokio::test]
async fn the_semantic_ratio_setting_reaches_the_query() {
    let rig = rig().await;
    rig.enable().await;
    rig.server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({"semantic_search_ratio": 0.8}),
        )
        .await
        .assert_ok();
    koblem_embeds_text(&rig.koblem).await;
    meili_answers_search(&rig.meili, vec![]).await;

    rig.search("haunted house").await;

    let queries = rig.multi_search_queries().await;
    let movies = queries.iter().find(|q| q["indexUid"] == "movies").unwrap();
    assert_eq!(
        movies["hybrid"]["semanticRatio"]
            .as_f64()
            .map(|r| (r * 10.0).round()),
        Some(8.0)
    );
}

#[tokio::test]
async fn koblem_down_means_keyword_search_not_a_failure() {
    let rig = rig().await;
    rig.enable().await;
    Mock::given(method("POST"))
        .and(path("/v1/text_embeddings"))
        .respond_with(ResponseTemplate::new(503).set_body_string("vision preempted"))
        .mount(&rig.koblem)
        .await;
    meili_answers_search(
        &rig.meili,
        vec![hit("movies", movie_doc("Interstellar"), None)],
    )
    .await;

    let result = rig.search("space adventure").await;

    assert_eq!(result["top_hits"][0]["title"], "Interstellar");
    for q in rig.multi_search_queries().await {
        assert!(q.get("hybrid").is_none(), "{q}");
    }
}

#[tokio::test]
async fn an_index_without_the_embedder_falls_back_to_keyword() {
    let rig = rig().await;
    rig.enable().await;
    koblem_embeds_text(&rig.koblem).await;
    Mock::given(method("POST"))
        .and(path("/multi-search"))
        .and(body_string_contains("hybrid"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "message": "Cannot find embedder with name `poster`.",
            "code": "invalid_search_embedder",
            "type": "invalid_request",
            "link": "https://docs.meilisearch.com/errors#invalid_search_embedder",
        })))
        .with_priority(1)
        .mount(&rig.meili)
        .await;
    meili_answers_search(
        &rig.meili,
        vec![hit("movies", movie_doc("Interstellar"), None)],
    )
    .await;

    let result = rig.search("space adventure").await;

    assert_eq!(result["top_hits"][0]["title"], "Interstellar");
    assert_eq!(
        requests(&rig.meili)
            .await
            .iter()
            .filter(|(_, p, _)| p == "/multi-search")
            .count(),
        2,
        "one hybrid attempt, one keyword retry"
    );
}

#[tokio::test]
async fn short_queries_and_ids_are_not_embedded() {
    let rig = rig().await;
    rig.enable().await;
    koblem_embeds_text(&rig.koblem).await;
    meili_answers_search(&rig.meili, vec![]).await;

    rig.search("up").await;

    assert!(rig.koblem_paths().await.is_empty());
}

#[tokio::test]
async fn search_needs_a_login() {
    let rig = rig().await;
    rig.server
        .api_client()
        .get("/api/v1/admin/search?q=space")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

// =============================================================================
// Settings
// =============================================================================

#[tokio::test]
async fn semantic_settings_are_validated() {
    let server = TestServer::start_api_only().await;
    let admin = server.admin_client();

    admin
        .patch_json(
            "/api/v1/admin/config",
            &json!({"semantic_search_enabled": true}),
        )
        .await
        .assert_status(StatusCode::BAD_REQUEST);
    admin
        .patch_json(
            "/api/v1/admin/config",
            &json!({"semantic_search_ratio": 1.5}),
        )
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    let config: Value = admin.get("/api/v1/admin/config").await.json();
    assert_eq!(config["semantic_search_enabled"], false);
    assert_eq!(config["semantic_search_ratio"], 0.5);
}

#[tokio::test]
async fn only_admins_change_semantic_search() {
    let server = TestServer::start_api_only().await;
    let viewer = create_user_client(&server, "viewer@test.com", "viewer").await;
    viewer
        .patch_json(
            "/api/v1/admin/config",
            &json!({"semantic_search_enabled": true, "koblem_service_url": "http://koblem:8090"}),
        )
        .await
        .assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn toggling_semantic_search_enqueues_the_vector_job() {
    let rig = rig().await;
    assert_eq!(pending_vector_jobs(&rig.server).await, 0);

    rig.enable().await;
    assert_eq!(pending_vector_jobs(&rig.server).await, 1);

    rig.server
        .admin_client()
        .patch_json(
            "/api/v1/admin/config",
            &json!({"semantic_search_ratio": 0.7}),
        )
        .await
        .assert_ok();
    assert_eq!(
        pending_vector_jobs(&rig.server).await,
        1,
        "a ratio change needs no vectors"
    );

    rig.disable().await;
    assert_eq!(pending_vector_jobs(&rig.server).await, 2);
}

// =============================================================================
// The vector job
// =============================================================================

#[tokio::test]
async fn turning_it_on_embeds_artwork_writes_vectors_then_declares_the_embedder() {
    let rig = rig().await;
    let with_poster = movie_with_cached_poster(&rig.server, "Interstellar").await;
    let without = movie_without_poster(&rig.server, "Untitled").await;
    meili_accepts_writes(&rig.meili, false).await;
    koblem_embeds_images(&rig.koblem).await;

    rig.enable().await;
    rig.run_vector_jobs().await;

    assert_eq!(stored_vector_count(&rig.server).await, 1);
    assert_eq!(rig.koblem_paths().await, vec!["/v1/embeddings"]);

    let writes = requests(&rig.meili).await;
    let docs_at = writes
        .iter()
        .position(|(m, p, _)| m == "PUT" && p == "/indexes/movies/documents")
        .expect("movies rewritten");
    let docs = writes[docs_at].2.as_array().unwrap();
    let vector_of = |id: Uuid| {
        docs.iter()
            .find(|d| d["id"] == id.to_string())
            .map(|d| d["_vectors"].clone())
            .unwrap()
    };
    assert_eq!(
        vector_of(with_poster)["poster"].as_array().map(Vec::len),
        Some(DIMS)
    );
    assert_eq!(vector_of(without), json!({"poster": null}));

    let declared: Vec<_> = writes
        .iter()
        .enumerate()
        .filter(|(_, (m, p, _))| m == "PATCH" && p.ends_with("/settings/embedders"))
        .collect();
    assert_eq!(declared.len(), 3, "movies, tv_shows and books");
    for (at, (_, _, body)) in &declared {
        assert!(
            *at > docs_at,
            "embedder declared before the documents carried vectors"
        );
        assert_eq!(body["poster"]["source"], "userProvided");
        assert_eq!(body["poster"]["dimensions"], DIMS);
    }

    let job: (String, Value) =
        sqlx::query_as("SELECT status, result FROM job_status WHERE job_type = 'search_vectors'")
            .fetch_one(&rig.server.pool)
            .await
            .unwrap();
    assert_eq!(job.0, "completed");
    assert_eq!(job.1["embedded"], 1);
}

#[tokio::test]
async fn a_later_run_only_embeds_what_is_new() {
    let rig = rig().await;
    movie_with_cached_poster(&rig.server, "Interstellar").await;
    meili_accepts_writes(&rig.meili, false).await;
    koblem_embeds_images(&rig.koblem).await;
    rig.enable().await;
    rig.run_vector_jobs().await;

    rig.meili.reset().await;
    rig.koblem.reset().await;
    meili_accepts_writes(&rig.meili, true).await;
    koblem_embeds_images(&rig.koblem).await;
    let added = movie_with_cached_poster(&rig.server, "Gravity").await;
    rig.server.app_state.search.set_semantic_enabled(true);
    api::jobs::enqueue(
        &rig.server.pool,
        &rig.server.nats_publisher,
        "search_vectors",
        json!({}),
        api::jobs::JobContext::new("scheduled"),
    )
    .await
    .unwrap();
    rig.run_vector_jobs().await;

    assert_eq!(stored_vector_count(&rig.server).await, 2);
    assert_eq!(rig.koblem_paths().await.len(), 1);
    let writes = requests(&rig.meili).await;
    assert!(
        !writes
            .iter()
            .any(|(m, p, _)| m == "PATCH" && p.ends_with("/settings/embedders")),
        "the embedder already exists"
    );
    assert!(
        !writes
            .iter()
            .any(|(m, p, _)| m == "DELETE" && p.ends_with("/documents")),
        "no full rewrite for one new title"
    );
    let single = writes
        .iter()
        .find(|(m, p, _)| m == "PUT" && p == "/indexes/movies/documents")
        .expect("the new title re-indexed");
    assert_eq!(single.2[0]["id"], added.to_string());
    assert_eq!(
        single.2[0]["_vectors"]["poster"].as_array().map(Vec::len),
        Some(DIMS)
    );
}

#[tokio::test]
async fn turning_it_off_removes_the_embedder_then_the_vectors() {
    let rig = rig().await;
    movie_with_cached_poster(&rig.server, "Interstellar").await;
    meili_accepts_writes(&rig.meili, true).await;
    rig.server
        .runtime_config
        .update(api::runtime_config::RuntimeConfigUpdate {
            semantic_search_enabled: Some(true),
            koblem_service_url: Some(rig.koblem.uri()),
            ..Default::default()
        })
        .await
        .unwrap();
    rig.server.app_state.search.set_semantic_enabled(true);

    rig.disable().await;
    rig.run_vector_jobs().await;

    assert!(!rig.server.app_state.search.semantic_enabled());
    let writes = requests(&rig.meili).await;
    let removed: Vec<usize> = writes
        .iter()
        .enumerate()
        .filter(|(_, (m, p, _))| m == "DELETE" && p.ends_with("/settings/embedders"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(removed.len(), 3);
    let rewrite_at = writes
        .iter()
        .position(|(m, p, _)| m == "PUT" && p == "/indexes/movies/documents")
        .expect("movies rewritten");
    assert!(removed.iter().all(|&i| i < rewrite_at));
    assert!(
        writes[rewrite_at].2[0].get("_vectors").is_none(),
        "{}",
        writes[rewrite_at].2
    );
    assert!(rig.koblem_paths().await.is_empty());
}

#[tokio::test]
async fn a_run_with_nothing_to_change_writes_nothing() {
    let rig = rig().await;
    meili_accepts_writes(&rig.meili, false).await;
    api::jobs::enqueue(
        &rig.server.pool,
        &rig.server.nats_publisher,
        "search_vectors",
        json!({}),
        api::jobs::JobContext::new("scheduled"),
    )
    .await
    .unwrap();

    rig.run_vector_jobs().await;

    assert!(
        requests(&rig.meili)
            .await
            .iter()
            .all(|(m, _, _)| m == "GET"),
        "semantic search is off and Meili already agrees"
    );
}

#[tokio::test]
async fn koblem_down_fails_the_job_before_the_embedder_is_declared() {
    let rig = rig().await;
    movie_with_cached_poster(&rig.server, "Interstellar").await;
    meili_accepts_writes(&rig.meili, false).await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&rig.koblem)
        .await;

    rig.enable().await;
    rig.run_vector_jobs().await;

    let status: String =
        sqlx::query_scalar("SELECT status FROM job_status WHERE job_type = 'search_vectors'")
            .fetch_one(&rig.server.pool)
            .await
            .unwrap();
    assert_eq!(status, "failed");
    assert_eq!(stored_vector_count(&rig.server).await, 0);
    assert!(
        !requests(&rig.meili)
            .await
            .iter()
            .any(|(m, p, _)| m == "PATCH" && p.ends_with("/settings/embedders"))
    );
}
