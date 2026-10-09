use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post, put},
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

/// A book in the mock Kobo store.
#[derive(Clone)]
pub struct MockStoreBook {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub series_name: Option<String>,
    pub series_index: Option<i32>,
}

impl MockStoreBook {
    pub fn new(id: &str, title: &str) -> Self {
        Self {
            id: id.to_string(),
            title: title.to_string(),
            authors: vec!["Store Author".to_string()],
            series_name: None,
            series_index: None,
        }
    }

    pub fn with_authors(mut self, authors: Vec<&str>) -> Self {
        self.authors = authors.into_iter().map(|s| s.to_string()).collect();
        self
    }

    pub fn with_series(mut self, name: &str, index: i32) -> Self {
        self.series_name = Some(name.to_string());
        self.series_index = Some(index);
        self
    }

    /// Convert to sync response format.
    pub fn to_sync_item(&self) -> Value {
        json!({
            "NewEntitlement": {
                "BookEntitlement": {
                    "Id": self.id,
                    "RevisionId": self.id,
                    "CrossRevisionId": self.id,
                    "Created": "2024-01-01T00:00:00Z",
                    "LastModified": "2024-01-01T00:00:00Z",
                    "IsRemoved": false,
                    "Accessibility": "Full",
                    "Status": "Active",
                    "OriginCategory": "Purchase"
                },
                "BookMetadata": {
                    "Title": self.title,
                    "RevisionId": self.id,
                    "ContributorRoles": self.authors.iter().map(|a| json!({
                        "Name": a,
                        "Role": "Author"
                    })).collect::<Vec<_>>(),
                    "DownloadUrls": [{
                        "Format": "EPUB",
                        "Size": 1000000,
                        "Url": format!("https://store.kobo.com/download/{}/EPUB", self.id),
                        "Platform": "Generic"
                    }],
                    "CoverImageId": self.id
                }
            }
        })
    }
}

/// Shared state for the mock server.
#[derive(Default)]
struct MockState {
    /// Books in the mock store.
    books: Vec<MockStoreBook>,
    /// Tags created by users.
    tags: HashMap<String, Value>,
    /// Reading states per book.
    reading_states: HashMap<String, Value>,
    /// Requests received (for verification).
    requests: Vec<(String, String)>, // (method, path)
    /// If set, sync returns this error status.
    sync_error: Option<u16>,
    /// If set, initialization returns this error status.
    init_error: Option<u16>,
    /// Custom resources to merge into initialization.
    custom_resources: Option<Value>,
}

/// Mock Kobo store API for merge/passthrough mode testing.
pub struct KoboMock {
    pub base_url: String,
    state: Arc<Mutex<MockState>>,
    _shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl KoboMock {
    /// Start a mock Kobo store API server.
    pub async fn start() -> Self {
        let state = Arc::new(Mutex::new(MockState::default()));
        let state_clone = state.clone();

        let app = Router::new()
            // Sync endpoint
            .route("/v1/library/sync", get(handle_sync))
            // Initialization endpoint
            .route("/v1/initialization", get(handle_init))
            // Book metadata
            .route("/v1/library/{book_id}/metadata", get(handle_metadata))
            // Reading state
            .route(
                "/v1/library/{book_id}/state",
                get(handle_get_state).put(handle_update_state),
            )
            // Delete book
            .route("/v1/library/{book_id}", delete(handle_delete_book))
            // Tags
            .route("/v1/library/tags", post(handle_create_tag))
            .route(
                "/v1/library/tags/{tag_id}",
                put(handle_update_tag).delete(handle_delete_tag),
            )
            .route(
                "/v1/library/tags/{tag_id}/items",
                post(handle_add_tag_items),
            )
            .route(
                "/v1/library/tags/{tag_id}/items/delete",
                post(handle_remove_tag_items),
            )
            // Download (just returns empty content)
            .route("/download/{book_id}/{format}", get(handle_download))
            // Cover images (matches Kobo CDN path structure)
            .route(
                "/book-images/{book_id}/{width}/{height}/{quality}/{greyscale}/image.jpg",
                get(handle_cover),
            )
            // Next read
            .route("/v1/products/{product_id}/nextread", get(handle_nextread))
            // Series books
            .route(
                "/v1/products/books/series/{series_id}",
                get(handle_series_books),
            )
            .with_state(state_clone);

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("Failed to bind mock server");
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{}", addr);

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    shutdown_rx.await.ok();
                })
                .await
                .ok();
        });

        Self {
            base_url,
            state,
            _shutdown: Some(shutdown_tx),
        }
    }

    /// Add a store book that will appear in sync responses.
    pub fn add_book(&self, book: MockStoreBook) {
        self.state.lock().unwrap().books.push(book);
    }

    /// Add a simple book with just ID and title.
    pub fn add_simple_book(&self, id: &str, title: &str) {
        self.add_book(MockStoreBook::new(id, title));
    }

    /// Clear all mock books.
    pub fn clear_books(&self) {
        self.state.lock().unwrap().books.clear();
    }

    /// Make sync return an error status code.
    pub fn set_sync_error(&self, status: u16) {
        self.state.lock().unwrap().sync_error = Some(status);
    }

    /// Clear sync error.
    pub fn clear_sync_error(&self) {
        self.state.lock().unwrap().sync_error = None;
    }

    /// Make initialization return an error status code.
    pub fn set_init_error(&self, status: u16) {
        self.state.lock().unwrap().init_error = Some(status);
    }

    /// Set custom resources to return in initialization.
    pub fn set_custom_resources(&self, resources: Value) {
        self.state.lock().unwrap().custom_resources = Some(resources);
    }

    /// Get all requests received by the mock server.
    pub fn get_requests(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().requests.clone()
    }

    /// Check if a request was received.
    pub fn was_called(&self, method: &str, path_contains: &str) -> bool {
        self.state
            .lock()
            .unwrap()
            .requests
            .iter()
            .any(|(m, p)| m == method && p.contains(path_contains))
    }

    /// Clear recorded requests.
    pub fn clear_requests(&self) {
        self.state.lock().unwrap().requests.clear();
    }

    /// Get the base URL.
    pub fn url(&self) -> &str {
        &self.base_url
    }

    /// Get number of books.
    pub fn book_count(&self) -> usize {
        self.state.lock().unwrap().books.len()
    }
}

impl Drop for KoboMock {
    fn drop(&mut self) {
        if let Some(tx) = self._shutdown.take() {
            tx.send(()).ok();
        }
    }
}

// =============================================================================
// Handler functions
// =============================================================================

async fn handle_sync(
    State(state): State<Arc<Mutex<MockState>>>,
    headers: HeaderMap,
) -> (StatusCode, Json<Value>) {
    let mut s = state.lock().unwrap();
    s.requests
        .push(("GET".to_string(), "/v1/library/sync".to_string()));

    if let Some(status) = s.sync_error {
        return (
            StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(json!({"error": "mock error"})),
        );
    }

    // Check for sync token in header (Kobo uses this for pagination)
    let _sync_token = headers
        .get("x-kobo-synctoken")
        .and_then(|v| v.to_str().ok());

    let items: Vec<Value> = s.books.iter().map(|b| b.to_sync_item()).collect();
    (StatusCode::OK, Json(json!(items)))
}

async fn handle_init(State(state): State<Arc<Mutex<MockState>>>) -> (StatusCode, Json<Value>) {
    let mut s = state.lock().unwrap();
    s.requests
        .push(("GET".to_string(), "/v1/initialization".to_string()));

    if let Some(status) = s.init_error {
        return (
            StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(json!({"error": "mock error"})),
        );
    }

    let resources = s.custom_resources.clone().unwrap_or_else(|| {
        json!({
            "library_sync": "https://storeapi.kobo.com/v1/library/sync",
            "library_items": "https://storeapi.kobo.com/v1/library/{LibraryItemId}",
            "image_host": "//cdn.kobo.com/book-images/",
            "image_url_template": "https://cdn.kobo.com/book-images/{ImageId}/{Width}/{Height}/false/image.jpg",
            "image_url_quality_template": "https://cdn.kobo.com/book-images/{ImageId}/{Width}/{Height}/{Quality}/{IsGreyscale}/image.jpg",
            "content_access_book": "https://storeapi.kobo.com/v1/products/books/{ProductId}",
        })
    });

    (StatusCode::OK, Json(json!({"Resources": resources})))
}

async fn handle_metadata(
    State(state): State<Arc<Mutex<MockState>>>,
    Path(book_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    let mut s = state.lock().unwrap();
    s.requests.push((
        "GET".to_string(),
        format!("/v1/library/{}/metadata", book_id),
    ));

    // Find the book in our store
    if let Some(book) = s.books.iter().find(|b| b.id == book_id) {
        let metadata = json!({
            "Title": book.title,
            "RevisionId": book.id,
            "ContributorRoles": book.authors.iter().map(|a| json!({
                "Name": a,
                "Role": "Author"
            })).collect::<Vec<_>>(),
            "DownloadUrls": [{
                "Format": "EPUB",
                "Size": 1000000,
                "Url": format!("https://store.kobo.com/download/{}/EPUB", book.id),
                "Platform": "Generic"
            }]
        });
        return (StatusCode::OK, Json(metadata));
    }

    (StatusCode::NOT_FOUND, Json(json!({"error": "not found"})))
}

async fn handle_get_state(
    State(state): State<Arc<Mutex<MockState>>>,
    Path(book_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    let mut s = state.lock().unwrap();
    s.requests
        .push(("GET".to_string(), format!("/v1/library/{}/state", book_id)));

    if let Some(reading_state) = s.reading_states.get(&book_id) {
        return (StatusCode::OK, Json(json!([reading_state])));
    }

    // Return default state
    (
        StatusCode::OK,
        Json(json!([{
            "EntitlementId": book_id,
            "StatusInfo": {
                "Status": "ReadyToRead"
            }
        }])),
    )
}

async fn handle_update_state(
    State(state): State<Arc<Mutex<MockState>>>,
    Path(book_id): Path<String>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let mut s = state.lock().unwrap();
    s.requests
        .push(("PUT".to_string(), format!("/v1/library/{}/state", book_id)));

    // Store the reading state
    if let Some(states) = body.get("ReadingStates").and_then(|s| s.as_array()) {
        if let Some(new_state) = states.first() {
            s.reading_states.insert(book_id.clone(), new_state.clone());
        }
    }

    (
        StatusCode::OK,
        Json(json!({
            "RequestResult": "Success",
            "UpdateResults": [{
                "EntitlementId": book_id,
                "CurrentBookmarkResult": { "Result": "Success" },
                "StatisticsResult": { "Result": "Success" },
                "StatusInfoResult": { "Result": "Success" }
            }]
        })),
    )
}

async fn handle_delete_book(
    State(state): State<Arc<Mutex<MockState>>>,
    Path(book_id): Path<String>,
) -> StatusCode {
    let mut s = state.lock().unwrap();
    s.requests
        .push(("DELETE".to_string(), format!("/v1/library/{}", book_id)));

    // Remove the book if it exists
    s.books.retain(|b| b.id != book_id);
    StatusCode::NO_CONTENT
}

async fn handle_create_tag(
    State(state): State<Arc<Mutex<MockState>>>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let mut s = state.lock().unwrap();
    s.requests
        .push(("POST".to_string(), "/v1/library/tags".to_string()));

    let tag_id = uuid::Uuid::new_v4().to_string();
    s.tags.insert(tag_id.clone(), body);

    (StatusCode::CREATED, Json(json!(tag_id)))
}

async fn handle_update_tag(
    State(state): State<Arc<Mutex<MockState>>>,
    Path(tag_id): Path<String>,
    Json(body): Json<Value>,
) -> StatusCode {
    let mut s = state.lock().unwrap();
    s.requests
        .push(("PUT".to_string(), format!("/v1/library/tags/{}", tag_id)));

    if let std::collections::hash_map::Entry::Occupied(mut e) = s.tags.entry(tag_id) {
        e.insert(body);
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn handle_delete_tag(
    State(state): State<Arc<Mutex<MockState>>>,
    Path(tag_id): Path<String>,
) -> StatusCode {
    let mut s = state.lock().unwrap();
    s.requests
        .push(("DELETE".to_string(), format!("/v1/library/tags/{}", tag_id)));

    if s.tags.remove(&tag_id).is_some() {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn handle_add_tag_items(
    State(state): State<Arc<Mutex<MockState>>>,
    Path(tag_id): Path<String>,
) -> StatusCode {
    let mut s = state.lock().unwrap();
    s.requests.push((
        "POST".to_string(),
        format!("/v1/library/tags/{}/items", tag_id),
    ));
    StatusCode::NO_CONTENT
}

async fn handle_remove_tag_items(
    State(state): State<Arc<Mutex<MockState>>>,
    Path(tag_id): Path<String>,
) -> StatusCode {
    let mut s = state.lock().unwrap();
    s.requests.push((
        "POST".to_string(),
        format!("/v1/library/tags/{}/items/delete", tag_id),
    ));
    StatusCode::NO_CONTENT
}

async fn handle_download(
    State(state): State<Arc<Mutex<MockState>>>,
    Path((book_id, format)): Path<(String, String)>,
) -> (StatusCode, Bytes) {
    let mut s = state.lock().unwrap();
    s.requests.push((
        "GET".to_string(),
        format!("/download/{}/{}", book_id, format),
    ));

    // Return fake EPUB content
    (
        StatusCode::OK,
        Bytes::from_static(b"PK\x03\x04fake kobo store epub"),
    )
}

async fn handle_cover(
    State(state): State<Arc<Mutex<MockState>>>,
    Path((book_id, _width, _height, _quality, _greyscale)): Path<(
        String,
        String,
        String,
        String,
        String,
    )>,
) -> (StatusCode, Bytes) {
    let mut s = state.lock().unwrap();
    s.requests
        .push(("GET".to_string(), format!("/{}/image.jpg", book_id)));

    // Return minimal JPEG
    (
        StatusCode::OK,
        Bytes::from_static(&[0xFF, 0xD8, 0xFF, 0xD9]),
    )
}

async fn handle_nextread(
    State(state): State<Arc<Mutex<MockState>>>,
    Path(product_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    let mut s = state.lock().unwrap();
    s.requests.push((
        "GET".to_string(),
        format!("/v1/products/{}/nextread", product_id),
    ));

    // Return empty recommendations
    (StatusCode::OK, Json(json!({})))
}

async fn handle_series_books(
    State(state): State<Arc<Mutex<MockState>>>,
    Path(series_id): Path<String>,
) -> (StatusCode, Json<Value>) {
    let mut s = state.lock().unwrap();
    s.requests.push((
        "GET".to_string(),
        format!("/v1/products/books/series/{}", series_id),
    ));

    // Return books in this series
    let books: Vec<Value> = s
        .books
        .iter()
        .filter(|b| {
            b.series_name
                .as_ref()
                .map(|s| s == &series_id)
                .unwrap_or(false)
        })
        .map(|b| {
            json!({
                "Id": b.id,
                "Title": b.title,
                "SeriesNumber": b.series_index
            })
        })
        .collect();

    (StatusCode::OK, Json(json!({"Books": books})))
}
