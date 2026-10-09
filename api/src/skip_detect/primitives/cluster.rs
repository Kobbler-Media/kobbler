//! Cluster-break detection over embedding sequences.

use super::cosine::cosine_similarity;

/// Find where a cluster of embeddings "breaks" -- the index where
/// similarity to the cluster centroid drops significantly.
/// Used for detecting post-credits scenes within a credits region.
pub fn find_cluster_break(embeddings: &[Vec<f32>], threshold: f64) -> Option<usize> {
    if embeddings.len() < 3 {
        return None;
    }

    // Use first 3 frames as the reference cluster
    let reference_count = 3.min(embeddings.len() / 2);
    let dim = embeddings[0].len();
    let mut centroid = vec![0.0f32; dim];

    for emb in &embeddings[..reference_count] {
        for (c, &e) in centroid.iter_mut().zip(emb.iter()) {
            *c += e;
        }
    }
    for c in &mut centroid {
        *c /= reference_count as f32;
    }

    // Find where similarity to centroid drops below threshold
    for (i, emb) in embeddings.iter().enumerate().skip(reference_count) {
        let sim = cosine_similarity(&centroid, emb);
        if sim < threshold {
            return Some(i);
        }
    }

    None
}
