//! Cosine similarity over embedding vectors.

/// Compute cosine similarity between two embedding vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;

    for (&ai, &bi) in a.iter().zip(b.iter()) {
        dot += ai as f64 * bi as f64;
        norm_a += (ai as f64).powi(2);
        norm_b += (bi as f64).powi(2);
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom < 1e-10 {
        return 0.0;
    }

    dot / denom
}

/// Find the average pairwise similarity within a set of embeddings.
/// High similarity = consistent content (e.g., credits text on same background).
pub fn cluster_consistency(embeddings: &[Vec<f32>]) -> f64 {
    if embeddings.len() < 2 {
        return 1.0;
    }

    let mut total = 0.0;
    let mut count = 0;
    for i in 0..embeddings.len() {
        for j in (i + 1)..embeddings.len() {
            total += cosine_similarity(&embeddings[i], &embeddings[j]);
            count += 1;
        }
    }

    if count > 0 { total / count as f64 } else { 0.0 }
}
