//! Client for an OpenAI-shaped `/v1/embeddings` server: llama-server directly
//! (`embed-vision` / `embed-text`) or koblem, which answers the same shape.
//!
//! Images go to the image URL; text goes to the text URL, which may be an
//! always-on text-only instance. Text and image vectors share one space, so
//! zero-shot classification is a local cosine against prompt vectors that are
//! embedded once per process.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use base64::Engine as _;
use futures::stream::{self, StreamExt, TryStreamExt};
use koblem_client::embed::{
    EmbedTask, EmbeddingInput, EmbeddingsRequest, EmbeddingsResponse, cosine, eg2_text,
    image_data_url,
};
use serde::Deserialize;
use tracing::debug;

use super::EMBED_MODEL;
use crate::runtime_config::RuntimeConfig;

/// The measured optimum on one EG2 instance: image encodes are serialised in the
/// server, so more than 4 in flight only queues, and 4 per request keeps each
/// request short enough to interleave with other callers.
const IMAGES_PER_REQUEST: usize = 4;
const IMAGE_REQUESTS_IN_FLIGHT: usize = 4;
const TEXTS_PER_REQUEST: usize = 32;

/// One pool for every embedding call, so keep-alive survives across jobs. No
/// meaningful total timeout: routed through koblem, an image request can wait
/// minutes behind the GPU gate, and that wait is the backpressure. Interactive
/// callers set their own per-request timeout.
static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    crate::http::builder()
        .user_agent("Kobbler/1.0")
        .timeout(Duration::from_secs(60 * 60))
        .build()
        .expect("Failed to create embedding HTTP client")
});

/// Prompt vectors by `(model, prompt)`. Prompts are a fixed handful per caller,
/// so this never grows past a few dozen entries.
type PromptCache = Mutex<HashMap<(&'static str, String), Arc<Vec<f32>>>>;
static PROMPT_VECTORS: LazyLock<PromptCache> = LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone)]
pub struct EmbeddingClient {
    image_url: String,
    text_url: String,
}

#[derive(Debug, Deserialize)]
struct HealthResponse {
    #[serde(default)]
    status: Option<String>,
}

fn normalise_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

impl EmbeddingClient {
    pub fn new(image_url: &str, text_url: &str) -> Self {
        Self {
            image_url: normalise_url(image_url),
            text_url: normalise_url(text_url),
        }
    }

    /// For callers that only embed images, or whose one URL serves both.
    pub fn for_url(url: &str) -> Self {
        Self::new(url, url)
    }

    /// Image URL = `effective_vision_url`, text URL =
    /// `effective_text_embedding_url`. `None` when no embedding service is set.
    pub fn from_config(config: &RuntimeConfig) -> Option<Self> {
        let image = config
            .effective_vision_url()
            .filter(|u| !u.trim().is_empty())?;
        let text = config
            .effective_text_embedding_url()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| image.clone());
        Some(Self::new(&image, &text))
    }

    pub fn image_url(&self) -> &str {
        &self.image_url
    }

    pub fn text_url(&self) -> &str {
        &self.text_url
    }

    /// The image service answers `/health`.
    pub async fn health(&self) -> bool {
        health(&self.image_url).await
    }

    /// The text service answers `/health`.
    pub async fn text_health(&self) -> bool {
        health(&self.text_url).await
    }

    /// One vector per JPEG, in order.
    pub async fn embed_images(&self, jpegs: &[Vec<u8>]) -> Result<Vec<Vec<f32>>, String> {
        if jpegs.is_empty() {
            return Ok(Vec::new());
        }
        let chunks: Vec<Vec<EmbeddingInput>> = jpegs
            .chunks(IMAGES_PER_REQUEST)
            .map(|chunk| {
                chunk
                    .iter()
                    .map(|jpeg| {
                        let b64 = base64::engine::general_purpose::STANDARD.encode(jpeg);
                        EmbeddingInput::image(image_data_url("image/jpeg", &b64))
                    })
                    .collect()
            })
            .collect();
        let url = self.image_url.as_str();
        let per_chunk: Vec<Vec<Vec<f32>>> = stream::iter(chunks)
            .map(|input| post_embeddings(url, input, None))
            .buffered(IMAGE_REQUESTS_IN_FLIGHT)
            .try_collect()
            .await?;
        let vectors: Vec<Vec<f32>> = per_chunk.into_iter().flatten().collect();
        debug!(count = vectors.len(), "Got image embeddings");
        Ok(vectors)
    }

    /// One vector per `(title, text)`, prefixed for `task` by `eg2_text`.
    pub async fn embed_texts(
        &self,
        task: EmbedTask,
        texts: &[(Option<&str>, &str)],
        timeout: Option<Duration>,
    ) -> Result<Vec<Vec<f32>>, String> {
        let mut out = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(TEXTS_PER_REQUEST) {
            let input = chunk
                .iter()
                .map(|(title, text)| EmbeddingInput::Text(eg2_text(task, *title, text)))
                .collect();
            out.extend(post_embeddings(&self.text_url, input, timeout).await?);
        }
        Ok(out)
    }

    /// A search query, for matching against `EmbedTask::Document` vectors.
    pub async fn embed_query(&self, query: &str, timeout: Duration) -> Result<Vec<f32>, String> {
        self.embed_texts(EmbedTask::Query, &[(None, query)], Some(timeout))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| "embedding service returned no vector".to_string())
    }

    /// Zero-shot prompt vectors, embedded raw (no task prefix scored best for
    /// classification prompts) and kept for the life of the process.
    pub async fn prompt_vectors(&self, prompts: &[&str]) -> Result<Vec<Arc<Vec<f32>>>, String> {
        let missing: Vec<&str> = {
            let cache = PROMPT_VECTORS.lock().unwrap_or_else(|e| e.into_inner());
            prompts
                .iter()
                .copied()
                .filter(|p| !cache.contains_key(&(EMBED_MODEL, p.to_string())))
                .collect()
        };
        if !missing.is_empty() {
            let items: Vec<(Option<&str>, &str)> = missing.iter().map(|p| (None, *p)).collect();
            let vectors = self.embed_texts(EmbedTask::Raw, &items, None).await?;
            let mut cache = PROMPT_VECTORS.lock().unwrap_or_else(|e| e.into_inner());
            for (prompt, vector) in missing.into_iter().zip(vectors) {
                cache.insert((EMBED_MODEL, prompt.to_string()), Arc::new(vector));
            }
        }
        let cache = PROMPT_VECTORS.lock().unwrap_or_else(|e| e.into_inner());
        prompts
            .iter()
            .map(|p| {
                cache
                    .get(&(EMBED_MODEL, p.to_string()))
                    .cloned()
                    .ok_or_else(|| format!("no vector for prompt {p:?}"))
            })
            .collect()
    }

    /// `scores[i][j]` = cosine of embedding `i` against prompt `j`.
    pub async fn classify_embeddings(
        &self,
        embeddings: &[Vec<f32>],
        prompts: &[&str],
    ) -> Result<Vec<Vec<f32>>, String> {
        if embeddings.is_empty() {
            return Ok(Vec::new());
        }
        let prompt_vectors = self.prompt_vectors(prompts).await?;
        Ok(embeddings
            .iter()
            .map(|e| prompt_vectors.iter().map(|p| cosine(e, p)).collect())
            .collect())
    }

    /// Embed the images, then classify them against `prompts` (cosine).
    pub async fn classify_images(
        &self,
        jpegs: &[Vec<u8>],
        prompts: &[&str],
    ) -> Result<Vec<Vec<f32>>, String> {
        let embeddings = self.embed_images(jpegs).await?;
        self.classify_embeddings(&embeddings, prompts).await
    }
}

async fn health(base_url: &str) -> bool {
    let Ok(resp) = HTTP
        .get(format!("{base_url}/health"))
        .timeout(Duration::from_secs(10))
        .send()
        .await
    else {
        return false;
    };
    if !resp.status().is_success() {
        return false;
    }
    // llama-server and koblem both answer `{"status":"ok"}`; a body that names
    // another status (e.g. "loading") is not ready.
    match resp.json::<HealthResponse>().await {
        Ok(HealthResponse {
            status: Some(status),
        }) => status == "ok",
        _ => true,
    }
}

async fn post_embeddings(
    base_url: &str,
    input: Vec<EmbeddingInput>,
    timeout: Option<Duration>,
) -> Result<Vec<Vec<f32>>, String> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let expected = input.len();
    let body = EmbeddingsRequest { input, model: None };
    let mut request = HTTP.post(format!("{base_url}/v1/embeddings")).json(&body);
    if let Some(timeout) = timeout {
        request = request.timeout(timeout);
    }
    let resp = request
        .send()
        .await
        .map_err(|e| format!("embedding request to {base_url} failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("embedding service returned {status}: {body}"));
    }
    let parsed: EmbeddingsResponse = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse embedding response: {e}"))?;
    let vectors = parsed.into_vectors();
    if vectors.len() != expected {
        return Err(format!(
            "embedding service returned {} vectors for {expected} inputs",
            vectors.len()
        ));
    }
    Ok(vectors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_config_falls_back_to_the_image_url_for_text() {
        let config = RuntimeConfig {
            vision_service_url: Some("http://embed-vision:8080/".to_string()),
            text_embedding_service_url: None,
            ..RuntimeConfig::default()
        };
        let client = EmbeddingClient::from_config(&config).unwrap();
        assert_eq!(client.image_url(), "http://embed-vision:8080");
        assert_eq!(client.text_url(), "http://embed-vision:8080");

        let config = RuntimeConfig {
            text_embedding_service_url: Some("http://embed-text:8080".to_string()),
            ..config
        };
        let client = EmbeddingClient::from_config(&config).unwrap();
        assert_eq!(client.text_url(), "http://embed-text:8080");
    }

    #[test]
    fn no_image_url_means_no_client() {
        let config = RuntimeConfig {
            vision_service_url: None,
            text_embedding_service_url: Some("http://embed-text:8080".to_string()),
            ..RuntimeConfig::default()
        };
        assert!(EmbeddingClient::from_config(&config).is_none());
    }
}
