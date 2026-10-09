//! Core types for the kobo-proxy crate.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncRead;
use uuid::Uuid;

/// Custom serialization for headers that preserves duplicates but looks like a normal object.
///
/// Serializes `Vec<(String, String)>` as:
/// - `{"Host": "example.com", "Accept": "application/json"}` for unique headers
/// - `{"Set-Cookie": ["cookie1", "cookie2"]}` for duplicate headers
pub mod header_serde {
    use super::*;
    use serde::de::{MapAccess, Visitor};
    use serde::ser::SerializeMap;
    use serde::{Deserializer, Serializer};
    use std::fmt;

    pub fn serialize<S>(headers: &Vec<(String, String)>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Group by header name, preserving order of first occurrence
        let mut groups: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut order: Vec<&str> = Vec::new();

        for (name, value) in headers {
            if !groups.contains_key(name.as_str()) {
                order.push(name.as_str());
            }
            groups
                .entry(name.as_str())
                .or_default()
                .push(value.as_str());
        }

        let mut map = serializer.serialize_map(Some(order.len()))?;
        for name in order {
            let values = &groups[name];
            if values.len() == 1 {
                map.serialize_entry(name, values[0])?;
            } else {
                map.serialize_entry(name, values)?;
            }
        }
        map.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<(String, String)>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct HeaderVisitor;

        impl<'de> Visitor<'de> for HeaderVisitor {
            type Value = Vec<(String, String)>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a map of header names to string or array of strings")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut headers = Vec::new();

                while let Some(key) = map.next_key::<String>()? {
                    let value: HeaderValue = map.next_value()?;
                    match value {
                        HeaderValue::Single(v) => headers.push((key, v)),
                        HeaderValue::Multiple(vs) => {
                            for v in vs {
                                headers.push((key.clone(), v));
                            }
                        }
                    }
                }

                Ok(headers)
            }
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum HeaderValue {
            Single(String),
            Multiple(Vec<String>),
        }

        deserializer.deserialize_map(HeaderVisitor)
    }

    /// For Option<Vec<(String, String)>>
    pub mod option {
        use super::*;

        pub fn serialize<S>(
            headers: &Option<Vec<(String, String)>>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            match headers {
                Some(h) => super::serialize(h, serializer),
                None => serializer.serialize_none(),
            }
        }

        pub fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<Option<Vec<(String, String)>>, D::Error>
        where
            D: Deserializer<'de>,
        {
            struct OptionHeaderVisitor;

            impl<'de> Visitor<'de> for OptionHeaderVisitor {
                type Value = Option<Vec<(String, String)>>;

                fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                    formatter
                        .write_str("null or a map of header names to string or array of strings")
                }

                fn visit_none<E>(self) -> Result<Self::Value, E> {
                    Ok(None)
                }

                fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
                where
                    D: Deserializer<'de>,
                {
                    super::deserialize(deserializer).map(Some)
                }

                fn visit_unit<E>(self) -> Result<Self::Value, E> {
                    Ok(None)
                }

                fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
                where
                    M: MapAccess<'de>,
                {
                    use serde::de::value::MapAccessDeserializer;
                    super::deserialize(MapAccessDeserializer::new(map)).map(Some)
                }
            }

            deserializer.deserialize_option(OptionHeaderVisitor)
        }
    }

    /// Convert headers to JSON Value using the nice format.
    /// Single values: `{"Name": "value"}`
    /// Multiple values: `{"Name": ["val1", "val2"]}`
    pub fn to_json_value(headers: &[(String, String)]) -> serde_json::Value {
        let mut groups: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut order: Vec<&str> = Vec::new();

        for (name, value) in headers {
            if !groups.contains_key(name.as_str()) {
                order.push(name.as_str());
            }
            groups
                .entry(name.as_str())
                .or_default()
                .push(value.as_str());
        }

        let mut map = serde_json::Map::new();
        for name in order {
            let values = &groups[name];
            if values.len() == 1 {
                map.insert(
                    name.to_string(),
                    serde_json::Value::String(values[0].to_string()),
                );
            } else {
                map.insert(
                    name.to_string(),
                    serde_json::Value::Array(
                        values
                            .iter()
                            .map(|v| serde_json::Value::String(v.to_string()))
                            .collect(),
                    ),
                );
            }
        }
        serde_json::Value::Object(map)
    }
}

/// Parsed HTTP request information (minimal - just what's needed for routing).
#[derive(Debug, Clone)]
pub struct ProxyRequest {
    /// HTTP method (GET, POST, PUT, DELETE, etc.)
    pub method: String,

    /// Original path from request (e.g., "/kobo/TOKEN/v1/library/sync")
    pub original_path: String,

    /// Path with kobo prefix stripped (e.g., "/v1/library/sync")
    pub path: String,

    /// Device token extracted from path
    pub device_token: Option<String>,

    /// Query string (without leading ?)
    pub query: Option<String>,

    /// HTTP headers in original order (name, value pairs)
    pub headers: Vec<(String, String)>,

    /// Original Host header from incoming request (for dynamic URL generation)
    pub incoming_host: Option<String>,

    /// Request body bytes (only populated for intercept/modify decisions)
    pub body: Option<Vec<u8>>,

    /// Content-Length if present
    pub content_length: Option<usize>,

    /// Whether request uses chunked transfer encoding
    pub is_chunked: bool,
}

impl ProxyRequest {
    /// Get the first value for a header (case-insensitive).
    pub fn get_header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Response body - either buffered in memory or streaming from a reader.
pub enum ResponseBody {
    /// Buffered body in memory (for small responses like JSON).
    Buffered(Vec<u8>),

    /// Streaming body from an async reader (for large files).
    /// Contains the reader and the total size for Content-Length.
    Stream {
        reader: Pin<Box<dyn AsyncRead + Send>>,
        size: u64,
    },
}

impl std::fmt::Debug for ResponseBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResponseBody::Buffered(data) => write!(f, "Buffered({} bytes)", data.len()),
            ResponseBody::Stream { size, .. } => write!(f, "Stream({} bytes)", size),
        }
    }
}

impl ResponseBody {
    /// Create a buffered body from bytes.
    pub fn buffered(data: Vec<u8>) -> Self {
        ResponseBody::Buffered(data)
    }

    /// Create a streaming body from an async reader.
    pub fn stream(reader: impl AsyncRead + Send + 'static, size: u64) -> Self {
        ResponseBody::Stream {
            reader: Box::pin(reader),
            size,
        }
    }

    /// Get the size of the body (works for both buffered and streaming).
    pub fn size(&self) -> u64 {
        match self {
            ResponseBody::Buffered(data) => data.len() as u64,
            ResponseBody::Stream { size, .. } => *size,
        }
    }

    /// Check if this is a buffered body.
    pub fn is_buffered(&self) -> bool {
        matches!(self, ResponseBody::Buffered(_))
    }

    /// Get buffered data if available (for logging).
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            ResponseBody::Buffered(data) => Some(data),
            ResponseBody::Stream { .. } => None,
        }
    }
}

/// Callback type for deferred side effects after response delivery.
pub type OnSentCallback = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// Response to be sent back to the client.
pub struct ProxyResponse {
    /// HTTP status code
    pub status: u16,

    /// HTTP status reason phrase
    pub reason: String,

    /// Response headers (Vec to preserve duplicates like Set-Cookie)
    pub headers: Vec<(String, String)>,

    /// Response body (buffered or streaming)
    pub body: ResponseBody,

    /// For intercept responses serving a local file, the source file path.
    /// This allows blob download to reference the original file without duplicating storage.
    pub source_file_path: Option<String>,

    /// Optional callback to run after the response has been successfully sent to the client.
    /// Use this to defer side effects (like advancing sync state) until delivery is confirmed.
    pub on_sent: Option<OnSentCallback>,
}

impl std::fmt::Debug for ProxyResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyResponse")
            .field("status", &self.status)
            .field("reason", &self.reason)
            .field("headers", &self.headers)
            .field("body", &self.body)
            .field("source_file_path", &self.source_file_path)
            .field(
                "on_sent",
                &self.on_sent.as_ref().map(|_| "Some(<callback>)"),
            )
            .finish()
    }
}

impl ProxyResponse {
    /// Get the first value for a header (case-insensitive).
    pub fn get_header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Set a header, replacing any existing value(s) with the same name.
    pub fn set_header(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        self.headers.retain(|(k, _)| !k.eq_ignore_ascii_case(&name));
        self.headers.push((name, value.into()));
    }

    /// Remove all headers with the given name (case-insensitive).
    pub fn remove_header(&mut self, name: &str) {
        self.headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
    }

    /// Check if a header exists (case-insensitive).
    pub fn has_header(&self, name: &str) -> bool {
        self.headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case(name))
    }
}

impl ProxyResponse {
    /// Create a simple JSON response.
    pub fn json(status: u16, body: &impl Serialize) -> Self {
        let body_bytes = serde_json::to_vec(body).unwrap_or_default();
        let headers = vec![
            (
                "Content-Type".to_string(),
                "application/json; charset=utf-8".to_string(),
            ),
            ("Content-Length".to_string(), body_bytes.len().to_string()),
        ];

        Self {
            status,
            reason: Self::reason_for_status(status).to_string(),
            headers,
            body: ResponseBody::Buffered(body_bytes),
            source_file_path: None,
            on_sent: None,
        }
    }

    /// Create an empty JSON object response.
    pub fn empty_json(status: u16) -> Self {
        Self::json(status, &serde_json::json!({}))
    }

    /// Create a JSON response with additional headers.
    pub fn json_with_headers(
        status: u16,
        body: &impl Serialize,
        extra_headers: Vec<(String, String)>,
    ) -> Self {
        let body_bytes = serde_json::to_vec(body).unwrap_or_default();
        let mut headers = extra_headers;
        // Ensure Content-Type and Content-Length are set (replacing if present)
        headers.retain(|(k, _)| {
            !k.eq_ignore_ascii_case("content-type") && !k.eq_ignore_ascii_case("content-length")
        });
        headers.push((
            "Content-Type".to_string(),
            "application/json; charset=utf-8".to_string(),
        ));
        headers.push(("Content-Length".to_string(), body_bytes.len().to_string()));

        Self {
            status,
            reason: Self::reason_for_status(status).to_string(),
            headers,
            body: ResponseBody::Buffered(body_bytes),
            source_file_path: None,
            on_sent: None,
        }
    }

    /// Create a streaming response for file downloads.
    pub fn stream(
        status: u16,
        reader: impl AsyncRead + Send + 'static,
        size: u64,
        headers: Vec<(String, String)>,
    ) -> Self {
        Self {
            status,
            reason: Self::reason_for_status(status).to_string(),
            headers,
            body: ResponseBody::Stream {
                reader: Box::pin(reader),
                size,
            },
            source_file_path: None,
            on_sent: None,
        }
    }

    /// Set the source file path for intercept responses serving local files.
    pub fn with_source_file_path(mut self, path: String) -> Self {
        self.source_file_path = Some(path);
        self
    }

    fn reason_for_status(status: u16) -> &'static str {
        match status {
            200 => "OK",
            201 => "Created",
            204 => "No Content",
            301 => "Moved Permanently",
            302 => "Found",
            304 => "Not Modified",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            500 => "Internal Server Error",
            502 => "Bad Gateway",
            503 => "Service Unavailable",
            _ => "Unknown",
        }
    }
}

/// Decision about how to handle a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyDecision {
    /// Take over completely - call handler.handle()
    Intercept,

    /// Proxy through to upstream unchanged (still logged).
    /// When proxy disabled: return stub from handler.stub()
    Passthrough,

    /// Fetch from upstream, then call handler.modify() before returning.
    /// When proxy disabled: call handler.handle() for local-only response
    Modify,
}

/// Result of classifying a request, containing the decision and optional notes.
#[derive(Debug, Clone)]
pub struct ClassifyResult {
    /// The proxy decision
    pub decision: ProxyDecision,
    /// Optional notes to attach to the log entry (e.g., book title for local books)
    pub notes: Option<String>,
    /// Optional alert message for flagging requests (e.g., leak detection, catch-all intercepts)
    pub alert_message: Option<String>,
}

impl ClassifyResult {
    /// Create a new ClassifyResult with just a decision (no notes)
    pub fn new(decision: ProxyDecision) -> Self {
        Self {
            decision,
            notes: None,
            alert_message: None,
        }
    }

    /// Create a new ClassifyResult with notes
    pub fn with_notes(decision: ProxyDecision, notes: String) -> Self {
        Self {
            decision,
            notes: Some(notes),
            alert_message: None,
        }
    }

    /// Create a new ClassifyResult with notes and an alert message
    pub fn with_alert(decision: ProxyDecision, notes: String, alert_message: String) -> Self {
        Self {
            decision,
            notes: Some(notes),
            alert_message: Some(alert_message),
        }
    }
}

impl From<ProxyDecision> for ClassifyResult {
    fn from(decision: ProxyDecision) -> Self {
        Self::new(decision)
    }
}

/// Configuration for connecting to an upstream server.
#[derive(Debug, Clone)]
pub struct UpstreamConfig {
    /// Base URL (e.g., "https://storeapi.kobo.com")
    pub base_url: String,

    /// Connection timeout in milliseconds
    pub timeout_ms: u64,
}

impl Default for UpstreamConfig {
    fn default() -> Self {
        Self {
            base_url: "https://storeapi.kobo.com".to_string(),
            timeout_ms: 30_000,
        }
    }
}

/// Log entry for a proxied request/response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestLogEntry {
    pub id: Uuid,
    pub device_token: Option<String>,

    // Request
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    /// Request headers as Vec to preserve duplicates and order
    #[serde(with = "header_serde")]
    pub request_headers: Vec<(String, String)>,
    pub request_body: Option<String>, // JSON only, None for binary
    pub request_body_size: usize,

    // Upstream response (before modification, only for Modify decisions)
    pub upstream_status: Option<u16>,
    /// Upstream headers as Vec to preserve duplicates (e.g., multiple Set-Cookie)
    #[serde(with = "header_serde::option")]
    pub upstream_headers: Option<Vec<(String, String)>>,
    pub upstream_body: Option<String>, // JSON only, None for binary
    pub upstream_body_size: usize,

    // Final response (after modification)
    pub status: u16,
    /// Response headers as Vec to preserve duplicates (e.g., multiple Set-Cookie)
    #[serde(with = "header_serde")]
    pub response_headers: Vec<(String, String)>,
    pub response_body: Option<String>, // JSON only, None for binary
    pub response_body_size: usize,

    // Blob capture (when enabled, stores raw binary bodies to temp files)
    /// Path to temp file containing captured request body (for binary content)
    #[serde(skip_serializing, default)]
    pub request_blob_temp_path: Option<String>,
    /// Path to temp file containing captured response body (for binary content)
    #[serde(skip_serializing, default)]
    pub response_blob_temp_path: Option<String>,
    /// Original response headers before any modification (for blob download)
    /// Vec to preserve duplicates (e.g., multiple Set-Cookie)
    #[serde(with = "header_serde::option")]
    pub original_response_headers: Option<Vec<(String, String)>>,

    /// For intercept responses serving local files, the source file path.
    /// Used instead of blob capture to avoid duplicate storage.
    #[serde(skip_serializing, default)]
    pub intercept_source_path: Option<String>,

    // Routing
    pub decision: String,
    pub upstream_url: Option<String>,
    pub upstream_duration_ms: Option<u32>,
    pub total_duration_ms: u32,

    // Classification
    pub severity: String,
    pub notes: Option<String>,
    /// Alert message from classify step (e.g., catch-all intercept reason)
    pub alert_message: Option<String>,

    /// Log status: 'in_progress', 'complete', 'error'
    pub log_status: String,

    pub timestamp: DateTime<Utc>,
}

impl RequestLogEntry {
    /// Create a new log entry with defaults.
    pub fn new(method: &str, path: &str, decision: ProxyDecision) -> Self {
        Self {
            id: Uuid::new_v4(),
            device_token: None,
            method: method.to_string(),
            path: path.to_string(),
            query: None,
            request_headers: Vec::new(),
            request_body: None,
            request_body_size: 0,
            upstream_status: None,
            upstream_headers: None,
            upstream_body: None,
            upstream_body_size: 0,
            status: 0,
            response_headers: Vec::new(),
            response_body: None,
            response_body_size: 0,
            request_blob_temp_path: None,
            response_blob_temp_path: None,
            original_response_headers: None,
            intercept_source_path: None,
            decision: match decision {
                ProxyDecision::Intercept => "intercept",
                ProxyDecision::Passthrough => "passthrough",
                ProxyDecision::Modify => "modify",
            }
            .to_string(),
            upstream_url: None,
            upstream_duration_ms: None,
            total_duration_ms: 0,
            severity: "info".to_string(),
            notes: None,
            alert_message: None,
            log_status: "in_progress".to_string(),
            timestamp: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn test_on_sent_callback_fires() {
        let fired = Arc::new(Mutex::new(false));
        let fired_clone = fired.clone();

        let mut response = ProxyResponse::json(200, &serde_json::json!({"ok": true}));
        response.on_sent = Some(Box::new(move || {
            Box::pin(async move {
                *fired_clone.lock().await = true;
            })
        }));

        // Take and fire the callback
        let cb = response.on_sent.take().unwrap();
        cb().await;

        assert!(*fired.lock().await, "on_sent callback should have fired");
    }

    #[test]
    fn test_on_sent_defaults_to_none() {
        let response = ProxyResponse::json(200, &serde_json::json!({}));
        assert!(response.on_sent.is_none());

        let response = ProxyResponse::empty_json(200);
        assert!(response.on_sent.is_none());
    }

    #[test]
    fn test_proxy_response_debug_with_callback() {
        let mut response = ProxyResponse::json(200, &serde_json::json!({}));
        response.on_sent = Some(Box::new(|| Box::pin(async {})));

        let debug = format!("{:?}", response);
        assert!(debug.contains("on_sent"));
        assert!(debug.contains("Some"));
    }
}
