//! Frame-embedding similarity helpers. The embedding client itself lives in
//! [`crate::embedding`].

pub use super::primitives::cluster::find_cluster_break;
pub use super::primitives::cosine::{cluster_consistency, cosine_similarity};

#[cfg(test)]
mod tests {
    use super::*;

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
