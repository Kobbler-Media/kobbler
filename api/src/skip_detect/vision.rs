//! SigLIP2 vision embedding service client.

use reqwest::multipart;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::primitives::credit_scoring::{CREDIT_PROMPTS, NON_CREDIT_PROMPTS, score_credit_text};

pub use super::primitives::cluster::find_cluster_break;
pub use super::primitives::cosine::{cluster_consistency, cosine_similarity};

/// Maximum number of frames per vision service request.
/// Caps peak VRAM regardless of segment duration — SigLIP2 SO400M on a 12 GB
/// card holds 16× 768-wide images comfortably.
const VISION_BATCH_SIZE: usize = 16;

pub struct VisionClient {
    base_url: String,
    client: reqwest::Client,
}

/// The vision hop answers in one of two shapes depending on routing.
/// siglip2 direct returns its native `{embeddings: [[..]]}`; koblem's
/// `/v1/embeddings` is OpenAI-compatible and returns
/// `{data: [{embedding: [..], index: n}]}`. Accept both — kobbler picks the
/// hop at runtime via `route_gpu_via_koblem`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum EmbeddingResponse {
    Native { embeddings: Vec<Vec<f32>> },
    OpenAi { data: Vec<OpenAiEmbedding> },
}

#[derive(Debug, Deserialize)]
struct OpenAiEmbedding {
    embedding: Vec<f32>,
    #[serde(default)]
    index: usize,
}

impl EmbeddingResponse {
    /// Callers zip the result back against the frame timestamps they
    /// submitted, so `index` — not array position — is authoritative.
    fn into_embeddings(self) -> Vec<Vec<f32>> {
        match self {
            Self::Native { embeddings } => embeddings,
            Self::OpenAi { mut data } => {
                data.sort_by_key(|e| e.index);
                data.into_iter().map(|e| e.embedding).collect()
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct HealthResponse {
    status: String,
}

#[derive(Debug, Deserialize)]
struct ClassifyResponse {
    scores: Vec<Vec<f32>>,
}

#[derive(Debug, Serialize)]
struct ClassifyFromEmbeddingsRequest<'a> {
    image_embeddings: &'a [Vec<f32>],
    prompts: &'a [&'a str],
    return_logits: bool,
}

#[derive(Debug, Deserialize)]
pub struct ClassifyFromEmbeddingsResponse {
    pub scores: Vec<Vec<f32>>,
    #[serde(default)]
    pub logits: Option<Vec<Vec<f32>>>,
}

impl VisionClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            // No client timeout: when kobbler routes through koblem, requests
            // can queue behind TTS preempts + STT cooldowns for many minutes.
            // The GPU lock is the backpressure mechanism; short network
            // timeouts just cause spurious failures.
            client: reqwest::Client::builder().build().unwrap_or_default(),
        }
    }

    /// Check if the vision service is healthy and responsive.
    pub async fn health(&self) -> bool {
        match self
            .client
            .get(format!("{}/health", self.base_url))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(body) = resp.json::<HealthResponse>().await {
                    body.status == "ok"
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Get embeddings for a batch of images.
    ///
    /// Each image should be JPEG or PNG bytes.
    /// Returns one embedding vector per image.
    ///
    /// Chunks requests into batches of `VISION_BATCH_SIZE` to cap peak VRAM usage.
    /// For long credits sequences at 0.5fps this prevents 60+ frames from being
    /// sent in a single request (which can spike VRAM from 1.2 GB to 5.6 GB).
    pub async fn embed_images(&self, images: &[Vec<u8>]) -> Result<Vec<Vec<f32>>, String> {
        if images.is_empty() {
            return Ok(vec![]);
        }

        let mut all_embeddings = Vec::with_capacity(images.len());

        for chunk in images.chunks(VISION_BATCH_SIZE) {
            let embeddings = self.embed_images_batch(chunk).await?;
            all_embeddings.extend(embeddings);
        }

        debug!(count = all_embeddings.len(), "Got vision embeddings");
        Ok(all_embeddings)
    }

    /// Zero-shot classify images against text prompts.
    /// Returns a score matrix: scores[image_idx][prompt_idx].
    ///
    /// Uses SigLIP2's zero-shot classification capability to determine
    /// how well each image matches each text description.
    pub async fn classify_images(
        &self,
        images: &[Vec<u8>],
        prompts: &[&str],
    ) -> Result<Vec<Vec<f32>>, String> {
        if images.is_empty() {
            return Ok(vec![]);
        }

        let mut all_scores = Vec::with_capacity(images.len());

        for chunk in images.chunks(VISION_BATCH_SIZE) {
            let scores = self.classify_images_batch(chunk, prompts).await?;
            all_scores.extend(scores);
        }

        Ok(all_scores)
    }

    /// Zero-shot classify pre-computed image embeddings against text prompts.
    /// Avoids re-running the vision encoder when embeddings are already cached.
    /// Returns `(scores, optional logits)` — `logits` is populated only if `return_logits`.
    pub async fn classify_from_embeddings(
        &self,
        image_embeddings: &[Vec<f32>],
        prompts: &[&str],
        return_logits: bool,
    ) -> Result<ClassifyFromEmbeddingsResponse, String> {
        if image_embeddings.is_empty() {
            return Ok(ClassifyFromEmbeddingsResponse {
                scores: Vec::new(),
                logits: None,
            });
        }

        let body = ClassifyFromEmbeddingsRequest {
            image_embeddings,
            prompts,
            return_logits,
        };

        let resp = self
            .client
            .post(format!("{}/v1/classify_from_embeddings", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("classify_from_embeddings request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!(
                "classify_from_embeddings returned {status}: {body}"
            ));
        }

        resp.json::<ClassifyFromEmbeddingsResponse>()
            .await
            .map_err(|e| format!("Failed to parse classify_from_embeddings response: {e}"))
    }

    async fn classify_images_batch(
        &self,
        images: &[Vec<u8>],
        prompts: &[&str],
    ) -> Result<Vec<Vec<f32>>, String> {
        let mut form = multipart::Form::new();
        for (i, img) in images.iter().enumerate() {
            let part = multipart::Part::bytes(img.clone())
                .file_name(format!("frame_{i:04}.jpg"))
                .mime_str("image/jpeg")
                .map_err(|e| format!("Failed to create multipart: {e}"))?;
            form = form.part("images", part);
        }
        for prompt in prompts {
            form = form.text("prompts", prompt.to_string());
        }

        let resp = self
            .client
            .post(format!("{}/v1/classify", self.base_url))
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("Vision classify request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Vision classify returned {status}: {body}"));
        }

        let result: ClassifyResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse classify response: {e}"))?;

        Ok(result.scores)
    }

    /// Send a single batch of images to the vision service.
    async fn embed_images_batch(&self, images: &[Vec<u8>]) -> Result<Vec<Vec<f32>>, String> {
        let mut form = multipart::Form::new();
        for (i, img) in images.iter().enumerate() {
            let part = multipart::Part::bytes(img.clone())
                .file_name(format!("frame_{i:04}.jpg"))
                .mime_str("image/jpeg")
                .map_err(|e| format!("Failed to create multipart: {e}"))?;
            form = form.part("images", part);
        }

        let resp = self
            .client
            .post(format!("{}/v1/embeddings", self.base_url))
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("Vision service request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Vision service returned {status}: {body}"));
        }

        let result: EmbeddingResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse vision response: {e}"))?;

        Ok(result.into_embeddings())
    }
}

/// Gap-tolerant credit text detection.
///
/// Takes pre-extracted frames (timestamp, jpeg bytes) from the last portion
/// of an episode and uses SigLIP2 zero-shot classification to identify frames
/// with text/credits overlay. Returns `(credits_start, credits_end, confidence)`.
///
/// `credits_end` is the timestamp of the last detected credit text frame in the
/// best region. Callers should check whether a post-credits scene follows
/// (if `credits_end` is well before the episode end).
///
/// Algorithm:
/// 1. Score each frame: credit_avg - non_credit_avg
/// 2. Collect all frames above threshold
/// 3. Group frames within gap_tolerance of each other into regions
/// 4. Pick the region with the most hits (most evidence of text)
/// 5. Credits start = first frame, credits end = last frame in that region
pub async fn detect_credit_text(
    client: &VisionClient,
    frames: &[(f64, Vec<u8>)],
) -> Option<(f64, f64, f64)> {
    if frames.len() < 5 {
        return None;
    }

    let images: Vec<Vec<u8>> = frames.iter().map(|(_, data)| data.clone()).collect();

    let all_prompts: Vec<&str> = CREDIT_PROMPTS
        .iter()
        .chain(NON_CREDIT_PROMPTS.iter())
        .copied()
        .collect();

    let scores = match client.classify_images(&images, &all_prompts).await {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "Vision classify failed — credit text detection skipped");
            return None;
        }
    };

    if scores.len() != frames.len() {
        return None;
    }

    let frame_times: Vec<f64> = frames.iter().map(|(t, _)| *t).collect();
    score_credit_text(&scores, &frame_times)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vision hop answers in two different shapes depending on routing:
    /// siglip2 direct returns its native `{embeddings: [[..]]}`, koblem's
    /// `/v1/embeddings` returns the OpenAI wrapper `{data: [{embedding: [..]}]}`.
    /// Parsing only the former silently broke every embed once
    /// `route_gpu_via_koblem` was turned on.
    #[test]
    fn embedding_response_accepts_native_siglip2_shape() {
        let body = r#"{"embeddings": [[1.0, 2.0], [3.0, 4.0]]}"#;
        let parsed: EmbeddingResponse = serde_json::from_str(body).expect("native shape");
        assert_eq!(
            parsed.into_embeddings(),
            vec![vec![1.0, 2.0], vec![3.0, 4.0]]
        );
    }

    #[test]
    fn embedding_response_accepts_koblem_openai_shape() {
        let body = r#"{"object":"list","model":"siglip2-naflex","data":[
            {"object":"embedding","embedding":[1.0,2.0],"index":0},
            {"object":"embedding","embedding":[3.0,4.0],"index":1}]}"#;
        let parsed: EmbeddingResponse = serde_json::from_str(body).expect("koblem shape");
        assert_eq!(
            parsed.into_embeddings(),
            vec![vec![1.0, 2.0], vec![3.0, 4.0]]
        );
    }

    /// Frame order is load-bearing — callers zip embeddings back against the
    /// timestamps they submitted, so an out-of-order `data` array must be
    /// restored by `index`, not trusted as-is.
    #[test]
    fn embedding_response_restores_openai_order_by_index() {
        let body = r#"{"data":[
            {"embedding":[3.0],"index":1},
            {"embedding":[1.0],"index":0},
            {"embedding":[2.0],"index":2}]}"#;
        let parsed: EmbeddingResponse = serde_json::from_str(body).expect("koblem shape");
        assert_eq!(
            parsed.into_embeddings(),
            vec![vec![1.0_f32], vec![3.0], vec![2.0]]
        );
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 0.001);
    }

    #[test]
    fn test_cluster_break() {
        // First 5 embeddings are similar, last 2 are different
        let embeddings: Vec<Vec<f32>> = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.99, 0.01, 0.0],
            vec![0.98, 0.02, 0.0],
            vec![0.97, 0.03, 0.0],
            vec![0.96, 0.04, 0.0],
            vec![0.0, 1.0, 0.0], // Break here
            vec![0.0, 0.98, 0.02],
        ];
        let break_idx = find_cluster_break(&embeddings, 0.5);
        assert_eq!(break_idx, Some(5));
    }
}
