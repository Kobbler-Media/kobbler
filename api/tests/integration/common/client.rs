use reqwest::{Client, StatusCode, header::HeaderMap, multipart};
use serde::{Serialize, de::DeserializeOwned};

/// HTTP test client with authentication helpers.
pub struct TestClient {
    client: Client,
    base_url: String,
    auth_token: Option<String>,
}

impl TestClient {
    /// Create a new test client for the given base URL.
    pub fn new(base_url: &str) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.to_string(),
            auth_token: None,
        }
    }

    /// Set the authentication token for subsequent requests.
    pub fn with_auth(mut self, token: &str) -> Self {
        self.auth_token = Some(token.to_string());
        self
    }

    /// Make a GET request.
    pub async fn get(&self, path: &str) -> TestResponse {
        let mut req = self.client.get(self.url(path));
        if let Some(ref token) = self.auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }
        let resp = req.send().await.expect("GET request failed");
        TestResponse::from_reqwest(resp).await
    }

    pub async fn get_with_header(&self, path: &str, name: &str, value: &str) -> TestResponse {
        let mut req = self.client.get(self.url(path)).header(name, value);
        if let Some(ref token) = self.auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }
        let resp = req.send().await.expect("GET request failed");
        TestResponse::from_reqwest(resp).await
    }

    /// Make a POST request without body.
    pub async fn post(&self, path: &str) -> TestResponse {
        let mut req = self.client.post(self.url(path));
        if let Some(ref token) = self.auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }
        let resp = req.send().await.expect("POST request failed");
        TestResponse::from_reqwest(resp).await
    }

    /// Make a POST request with JSON body.
    pub async fn post_json<T: Serialize>(&self, path: &str, body: &T) -> TestResponse {
        let mut req = self.client.post(self.url(path)).json(body);
        if let Some(ref token) = self.auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }
        let resp = req.send().await.expect("POST request failed");
        TestResponse::from_reqwest(resp).await
    }

    /// Make a PUT request with JSON body.
    pub async fn put_json<T: Serialize>(&self, path: &str, body: &T) -> TestResponse {
        let mut req = self.client.put(self.url(path)).json(body);
        if let Some(ref token) = self.auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }
        let resp = req.send().await.expect("PUT request failed");
        TestResponse::from_reqwest(resp).await
    }

    /// Make a PUT request with raw bytes.
    pub async fn put_bytes(&self, path: &str, body: Vec<u8>, content_type: &str) -> TestResponse {
        let mut req = self
            .client
            .put(self.url(path))
            .header("Content-Type", content_type)
            .body(body);
        if let Some(ref token) = self.auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }
        let resp = req.send().await.expect("PUT request failed");
        TestResponse::from_reqwest(resp).await
    }

    /// Make a PUT request with multipart form.
    pub async fn put_multipart(&self, path: &str, form: multipart::Form) -> TestResponse {
        let mut req = self.client.put(self.url(path)).multipart(form);
        if let Some(ref token) = self.auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }
        let resp = req.send().await.expect("PUT multipart request failed");
        TestResponse::from_reqwest(resp).await
    }

    /// Make a POST request with multipart form.
    pub async fn post_multipart(&self, path: &str, form: multipart::Form) -> TestResponse {
        let mut req = self.client.post(self.url(path)).multipart(form);
        if let Some(ref token) = self.auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }
        let resp = req.send().await.expect("POST multipart request failed");
        TestResponse::from_reqwest(resp).await
    }

    /// Make a POST request with raw bytes.
    pub async fn post_bytes(&self, path: &str, body: Vec<u8>, content_type: &str) -> TestResponse {
        let mut req = self
            .client
            .post(self.url(path))
            .header("Content-Type", content_type)
            .body(body);
        if let Some(ref token) = self.auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }
        let resp = req.send().await.expect("POST request failed");
        TestResponse::from_reqwest(resp).await
    }

    /// Make a PATCH request with JSON body.
    pub async fn patch_json<T: Serialize>(&self, path: &str, body: &T) -> TestResponse {
        let mut req = self.client.patch(self.url(path)).json(body);
        if let Some(ref token) = self.auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }
        let resp = req.send().await.expect("PATCH request failed");
        TestResponse::from_reqwest(resp).await
    }

    /// Make a DELETE request.
    pub async fn delete(&self, path: &str) -> TestResponse {
        let mut req = self.client.delete(self.url(path));
        if let Some(ref token) = self.auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }
        let resp = req.send().await.expect("DELETE request failed");
        TestResponse::from_reqwest(resp).await
    }

    /// Make a DELETE request with JSON body.
    pub async fn delete_json<T: Serialize>(&self, path: &str, body: &T) -> TestResponse {
        let mut req = self.client.delete(self.url(path)).json(body);
        if let Some(ref token) = self.auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }
        let resp = req.send().await.expect("DELETE request failed");
        TestResponse::from_reqwest(resp).await
    }

    // =========================================================================
    // Kobo proxy helpers
    // =========================================================================

    /// Make a Kobo sync request for a device (no sync token).
    /// Uses the proxy URL: /kobo/{device_token}/v1/library/sync
    pub async fn kobo_sync(&self, device_token: &str) -> TestResponse {
        self.get(&format!("/kobo/{}/v1/library/sync", device_token))
            .await
    }

    /// Make a Kobo sync request with a sync token (for continuation syncs).
    pub async fn kobo_sync_with_token(&self, device_token: &str, sync_token: &str) -> TestResponse {
        let resp = self
            .client
            .get(self.url(&format!("/kobo/{}/v1/library/sync", device_token)))
            .header("x-kobo-synctoken", sync_token)
            .send()
            .await
            .expect("Kobo sync request failed");
        TestResponse::from_reqwest(resp).await
    }

    /// Get reading state for a book.
    pub async fn kobo_get_state(&self, device_token: &str, book_id: &str) -> TestResponse {
        self.get(&format!(
            "/kobo/{}/v1/library/{}/state",
            device_token, book_id
        ))
        .await
    }

    /// Update reading state for a book.
    pub async fn kobo_update_state(
        &self,
        device_token: &str,
        book_id: &str,
        state: &serde_json::Value,
    ) -> TestResponse {
        self.put_json(
            &format!("/kobo/{}/v1/library/{}/state", device_token, book_id),
            state,
        )
        .await
    }

    /// Get book metadata.
    pub async fn kobo_metadata(&self, device_token: &str, book_id: &str) -> TestResponse {
        self.get(&format!(
            "/kobo/{}/v1/library/{}/metadata",
            device_token, book_id
        ))
        .await
    }

    /// Download a book file.
    pub async fn kobo_download(
        &self,
        device_token: &str,
        book_id: &str,
        format: &str,
    ) -> TestResponse {
        self.get(&format!(
            "/kobo/{}/download/{}/{}",
            device_token, book_id, format
        ))
        .await
    }

    /// Create a tag.
    pub async fn kobo_create_tag(
        &self,
        device_token: &str,
        body: &serde_json::Value,
    ) -> TestResponse {
        self.post_json(&format!("/kobo/{}/v1/library/tags", device_token), body)
            .await
    }

    /// Rename a tag.
    pub async fn kobo_rename_tag(
        &self,
        device_token: &str,
        tag_id: &str,
        body: &serde_json::Value,
    ) -> TestResponse {
        self.put_json(
            &format!("/kobo/{}/v1/library/tags/{}", device_token, tag_id),
            body,
        )
        .await
    }

    /// Delete a tag.
    pub async fn kobo_delete_tag(&self, device_token: &str, tag_id: &str) -> TestResponse {
        self.delete(&format!(
            "/kobo/{}/v1/library/tags/{}",
            device_token, tag_id
        ))
        .await
    }

    /// Add items to a tag.
    pub async fn kobo_add_tag_items(
        &self,
        device_token: &str,
        tag_id: &str,
        body: &serde_json::Value,
    ) -> TestResponse {
        self.post_json(
            &format!("/kobo/{}/v1/library/tags/{}/items", device_token, tag_id),
            body,
        )
        .await
    }

    /// Remove items from a tag.
    pub async fn kobo_remove_tag_items(
        &self,
        device_token: &str,
        tag_id: &str,
        body: &serde_json::Value,
    ) -> TestResponse {
        self.post_json(
            &format!(
                "/kobo/{}/v1/library/tags/{}/items/delete",
                device_token, tag_id
            ),
            body,
        )
        .await
    }

    /// Get initialization resources.
    pub async fn kobo_init(&self, device_token: &str) -> TestResponse {
        self.get(&format!("/kobo/{}/v1/initialization", device_token))
            .await
    }

    /// Delete a book from the library.
    pub async fn kobo_delete_book(&self, device_token: &str, book_id: &str) -> TestResponse {
        self.delete(&format!("/kobo/{}/v1/library/{}", device_token, book_id))
            .await
    }

    /// Get a book cover image using the /book-images/ path format.
    pub async fn kobo_cover(
        &self,
        device_token: &str,
        image_id: &str,
        width: u32,
        height: u32,
    ) -> TestResponse {
        self.get(&format!(
            "/kobo/{}/book-images/{}/{}/{}/80/false/image.jpg",
            device_token, image_id, width, height
        ))
        .await
    }

    /// Get next read recommendations for a book.
    pub async fn kobo_nextread(&self, device_token: &str, product_id: &str) -> TestResponse {
        self.get(&format!(
            "/kobo/{}/v1/products/{}/nextread",
            device_token, product_id
        ))
        .await
    }

    /// Get books in a series.
    pub async fn kobo_series_books(&self, device_token: &str, series_id: &str) -> TestResponse {
        self.get(&format!(
            "/kobo/{}/v1/products/books/series/{}",
            device_token, series_id
        ))
        .await
    }

    /// Get product book details.
    pub async fn kobo_product_book(&self, device_token: &str, book_id: &str) -> TestResponse {
        self.get(&format!(
            "/kobo/{}/v1/products/books/{}",
            device_token, book_id
        ))
        .await
    }

    /// Get product recommendations.
    pub async fn kobo_recommendations(&self, device_token: &str, product_id: &str) -> TestResponse {
        self.get(&format!(
            "/kobo/{}/v1/products/{}/recommendations",
            device_token, product_id
        ))
        .await
    }

    /// Get reviews for a product.
    pub async fn kobo_reviews(&self, device_token: &str, product_id: &str) -> TestResponse {
        self.get(&format!(
            "/kobo/{}/v1/products/{}/reviews",
            device_token, product_id
        ))
        .await
    }

    /// Get prices for a product.
    pub async fn kobo_product_prices(&self, device_token: &str, product_id: &str) -> TestResponse {
        self.get(&format!(
            "/kobo/{}/v1/products/{}/prices",
            device_token, product_id
        ))
        .await
    }

    /// Get related items for a product.
    pub async fn kobo_product_related(&self, device_token: &str, product_id: &str) -> TestResponse {
        self.get(&format!(
            "/kobo/{}/v1/products/{}/related",
            device_token, product_id
        ))
        .await
    }

    /// Post recommendations feedback (ratings via FeedbackItems).
    pub async fn kobo_recommendations_feedback(
        &self,
        device_token: &str,
        body: &serde_json::Value,
    ) -> TestResponse {
        self.post_json(
            &format!("/kobo/{}/v1/user/recommendations/feedback", device_token),
            body,
        )
        .await
    }

    /// Post a rating for a product.
    pub async fn kobo_rate(
        &self,
        device_token: &str,
        product_id: &str,
        rating: i32,
    ) -> TestResponse {
        self.post(&format!(
            "/kobo/{}/v1/products/{}/rating/{}",
            device_token, product_id, rating
        ))
        .await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

/// Response from a test HTTP request.
pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    body: Vec<u8>,
}

impl TestResponse {
    async fn from_reqwest(resp: reqwest::Response) -> Self {
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp.bytes().await.expect("Failed to read body").to_vec();
        Self {
            status,
            headers,
            body,
        }
    }

    /// Parse the response body as JSON.
    pub fn json<T: DeserializeOwned>(&self) -> T {
        let text = self.text();
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|_| panic!("Failed to parse response as JSON: {}", text))
    }

    /// Get the response body as text.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// Get the raw response body.
    pub fn bytes(&self) -> &[u8] {
        &self.body
    }

    /// Assert the response status is 200 OK.
    pub fn assert_ok(&self) -> &Self {
        assert_eq!(
            self.status,
            StatusCode::OK,
            "Expected 200 OK but got {}. Body: {}",
            self.status,
            self.text()
        );
        self
    }

    /// Assert the response status is successful (2xx).
    pub fn assert_success(&self) -> &Self {
        assert!(
            self.status.is_success(),
            "Expected 2xx success but got {}. Body: {}",
            self.status,
            self.text()
        );
        self
    }

    /// Assert the response status matches expected.
    pub fn assert_status(&self, expected: StatusCode) -> &Self {
        assert_eq!(
            self.status,
            expected,
            "Expected {} but got {}. Body: {}",
            expected,
            self.status,
            self.text()
        );
        self
    }

    /// Get the x-kobo-synctoken header value.
    pub fn get_sync_token(&self) -> Option<String> {
        self.headers
            .get("x-kobo-synctoken")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    }

    /// Check if the response has the x-kobo-sync: continue header.
    pub fn has_continue_sync(&self) -> bool {
        self.headers
            .get("x-kobo-sync")
            .and_then(|v| v.to_str().ok())
            .map(|v| v == "continue")
            .unwrap_or(false)
    }

    /// Assert a header exists with expected value.
    pub fn assert_header(&self, name: &str, expected: &str) -> &Self {
        let value = self
            .headers
            .get(name)
            .unwrap_or_else(|| panic!("Missing header: {}", name))
            .to_str()
            .expect("Invalid header value");
        assert_eq!(value, expected, "Header {} mismatch", name);
        self
    }
}
