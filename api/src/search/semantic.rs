//! Semantic search over title artwork.
//!
//! Each movie, show and library book carries one SigLIP2 vector of its poster or
//! cover, computed on koblem's GPU and stored as a Meilisearch `userProvided`
//! embedding. Queries are embedded with SigLIP2's text tower and searched as a
//! hybrid. Text-to-text over descriptions was measured and rejected: SigLIP2's
//! text tower is trained for text→image alignment, and description vectors all
//! scored within a few hundredths of each other for any query.

use std::time::Duration;

use koblem_client::KoblemClient;
use koblem_client::types::VisionTextEmbedRequest;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::warn;
use uuid::Uuid;

use super::backend::{SearchBackend, SearchError};
use super::{BookDocument, MovieDocument, TvShowDocument, indexes};
use crate::repositories::search_embeddings::{self, EmbeddedKind};
use crate::runtime_config::RuntimeConfig;

pub const EMBEDDER: &str = "poster";
pub const DIMENSIONS: usize = 1152;
pub const INDEXES: [&str; 3] = [indexes::MOVIES, indexes::TV_SHOWS, indexes::BOOKS];

/// Raw text→image cosine similarities sit between about 0.0 and 0.18, so Meili's
/// `(1 + cos) / 2` puts every semantic hit at 0.50–0.59, under any keyword hit.
/// This spread (measured on a real library) stretches them so a good artwork match
/// competes with keyword matches and an unrelated one falls towards zero.
pub const DISTRIBUTION_MEAN: f64 = 0.53;
pub const DISTRIBUTION_SIGMA: f64 = 0.01;

/// Semantic-only hits below this corrected score are noise: every query has *some*
/// nearest poster, and without a floor an unrelated one would take its type's top slot.
pub const SEMANTIC_HIT_FLOOR: f64 = 0.3;

/// The palette searches as you type; past this, keyword results are what it shows.
const QUERY_TIMEOUT: Duration = Duration::from_millis(1500);
const MIN_QUERY_CHARS: usize = 3;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PosterVectors {
    /// Always serialized, null included: Meili rejects a whole batch if any document
    /// lacks `_vectors.poster` once the embedder exists.
    pub poster: Option<Vec<f32>>,
}

#[derive(Debug, Clone)]
pub struct SemanticQuery {
    pub vector: Vec<f32>,
    pub ratio: f32,
}

pub fn koblem_url(config: &RuntimeConfig) -> Option<String> {
    config
        .koblem_service_url
        .as_deref()
        .map(|u| u.trim().trim_end_matches('/'))
        .filter(|u| !u.is_empty())
        .map(str::to_string)
}

/// The query vector for a hybrid search, or `None` to search by keyword alone —
/// whether because semantic search is off, the query is too short to mean anything,
/// or koblem is slow or down.
pub async fn query_vector(config: &RuntimeConfig, query: &str) -> Option<SemanticQuery> {
    if !config.semantic_search_enabled || config.semantic_search_ratio <= 0.0 {
        return None;
    }
    let trimmed = query.trim();
    if trimmed.chars().count() < MIN_QUERY_CHARS || Uuid::parse_str(trimmed).is_ok() {
        return None;
    }
    let url = koblem_url(config)?;
    match embed_text(&url, trimmed).await {
        Ok(vector) => Some(SemanticQuery {
            vector,
            ratio: config.semantic_search_ratio as f32,
        }),
        Err(e) => {
            warn!(error = %e, "Semantic query embedding unavailable; searching by keyword");
            None
        }
    }
}

pub async fn embed_text(koblem_url: &str, text: &str) -> Result<Vec<f32>, String> {
    let client = KoblemClient::new(koblem_url.to_string());
    let request = VisionTextEmbedRequest {
        prompts: vec![text.to_string()],
    };
    let response = tokio::time::timeout(QUERY_TIMEOUT, client.text_embeddings(&request))
        .await
        .map_err(|_| format!("koblem took longer than {QUERY_TIMEOUT:?}"))?
        .map_err(|e| e.to_string())?;
    let vector = response
        .embeddings
        .into_iter()
        .next()
        .ok_or("koblem returned no embedding")?;
    if vector.len() != DIMENSIONS {
        return Err(format!(
            "koblem returned a {}-dimensional embedding, expected {DIMENSIONS}",
            vector.len()
        ));
    }
    Ok(vector)
}

pub trait VectorDoc {
    const KIND: EmbeddedKind;
    fn doc_id(&self) -> &str;
    fn set_vectors(&mut self, vectors: PosterVectors);
}

impl VectorDoc for MovieDocument {
    const KIND: EmbeddedKind = EmbeddedKind::Movie;
    fn doc_id(&self) -> &str {
        &self.id
    }
    fn set_vectors(&mut self, vectors: PosterVectors) {
        self.vectors = Some(vectors);
    }
}

impl VectorDoc for TvShowDocument {
    const KIND: EmbeddedKind = EmbeddedKind::TvShow;
    fn doc_id(&self) -> &str {
        &self.id
    }
    fn set_vectors(&mut self, vectors: PosterVectors) {
        self.vectors = Some(vectors);
    }
}

impl VectorDoc for BookDocument {
    const KIND: EmbeddedKind = EmbeddedKind::Book;
    fn doc_id(&self) -> &str {
        &self.id
    }
    fn set_vectors(&mut self, vectors: PosterVectors) {
        self.vectors = Some(vectors);
    }
}

/// Gives each document its stored vector (or an explicit null) when the backend has
/// semantic search on, and leaves documents untouched when it does not.
pub async fn attach_vectors<D: VectorDoc>(
    search: &dyn SearchBackend,
    pool: &PgPool,
    docs: &mut [D],
) -> Result<(), SearchError> {
    if !search.semantic_enabled() || docs.is_empty() {
        return Ok(());
    }
    let ids: Vec<Uuid> = docs
        .iter()
        .filter_map(|d| Uuid::parse_str(d.doc_id()).ok())
        .collect();
    let mut stored = search_embeddings::vectors_for(pool, D::KIND, &ids)
        .await
        .map_err(|e| SearchError::Backend(e.to_string()))?;
    for doc in docs.iter_mut() {
        let poster = Uuid::parse_str(doc.doc_id())
            .ok()
            .and_then(|id| stored.remove(&id));
        doc.set_vectors(PosterVectors { poster });
    }
    Ok(())
}

type IndexResult = Result<usize, Box<dyn std::error::Error + Send + Sync>>;

pub async fn reindex_movies(search: &dyn SearchBackend, pool: &PgPool) -> IndexResult {
    let mut docs = super::fetch_all_movie_docs(pool).await?;
    attach_vectors(search, pool, &mut docs).await?;
    Ok(search.reindex_all_movies(docs).await?)
}

pub async fn reindex_tv_shows(search: &dyn SearchBackend, pool: &PgPool) -> IndexResult {
    let mut docs = super::fetch_all_tv_show_docs(pool).await?;
    attach_vectors(search, pool, &mut docs).await?;
    Ok(search.reindex_all_tv_shows(docs).await?)
}

pub async fn reindex_books(search: &dyn SearchBackend, pool: &PgPool) -> IndexResult {
    let mut docs = super::fetch_all_book_docs(pool).await?;
    attach_vectors(search, pool, &mut docs).await?;
    Ok(search.reindex_all_books(docs).await?)
}
