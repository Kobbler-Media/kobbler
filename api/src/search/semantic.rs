//! Semantic search over title summaries and artwork.
//!
//! Each movie, show and library book carries up to two EmbeddingGemma-2 vectors,
//! stored as Meilisearch `userProvided` embeddings: its summary (`description`,
//! the primary signal — "a slow-burn thriller about grief" is a summary match) and
//! its poster or cover (`poster`, secondary). Both share one space with the query,
//! so one query vector, embedded with the search-query prefix, serves both. Meili
//! holds no model: vectors are computed by the embedding service and only stored
//! and compared there.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::warn;
use uuid::Uuid;

use super::backend::{SearchBackend, SearchError};
use super::{BookDocument, MovieDocument, TvShowDocument, indexes};
use crate::embedding::{EMBED_DIM, EMBED_MODEL, EmbeddingClient, profile};
use crate::repositories::search_embeddings::{self, EmbeddedKind, StoredVectors};
use crate::runtime_config::RuntimeConfig;

/// The primary embedder: summary + genres + year, embedded as a titled document.
pub const DESCRIPTION_EMBEDDER: &str = "description";
/// The secondary embedder: poster or cover.
pub const POSTER_EMBEDDER: &str = "poster";
pub const EMBEDDERS: [&str; 2] = [DESCRIPTION_EMBEDDER, POSTER_EMBEDDER];
pub const DIMENSIONS: usize = EMBED_DIM;
pub const INDEXES: [&str; 3] = [indexes::MOVIES, indexes::TV_SHOWS, indexes::BOOKS];

/// Federation weight of the poster-only query against the summary hybrid. Below
/// 1 so artwork fills in titles the summary missed without outranking a summary
/// match. A title found by both keeps only its best-ranked hit.
pub const POSTER_WEIGHT: f32 = 0.6;

/// The palette searches as you type; past this, keyword results are what it shows.
const QUERY_TIMEOUT: Duration = Duration::from_millis(1500);
const MIN_QUERY_CHARS: usize = 3;

pub fn description_distribution() -> profile::Distribution {
    profile::active().description_distribution
}

pub fn poster_distribution() -> profile::Distribution {
    profile::active().poster_distribution
}

/// Semantic-only hits below this corrected score are dropped.
pub fn semantic_hit_floor() -> f64 {
    profile::active().semantic_hit_floor
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TitleVectors {
    /// Both always serialized, null included: Meili rejects a whole batch if any
    /// document lacks `_vectors.<embedder>` once that embedder exists.
    pub description: Option<Vec<f32>>,
    pub poster: Option<Vec<f32>>,
}

impl From<StoredVectors> for TitleVectors {
    fn from(v: StoredVectors) -> Self {
        Self {
            description: v.description,
            poster: v.poster,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SemanticQuery {
    pub vector: Vec<f32>,
    pub ratio: f32,
}

/// The query vector for a hybrid search, or `None` to search by keyword alone —
/// whether because semantic search is off, the query is too short to mean anything,
/// or the text embedder is slow or down.
pub async fn query_vector(config: &RuntimeConfig, query: &str) -> Option<SemanticQuery> {
    if !config.semantic_search_enabled || config.semantic_search_ratio <= 0.0 {
        return None;
    }
    let trimmed = query.trim();
    if trimmed.chars().count() < MIN_QUERY_CHARS || Uuid::parse_str(trimmed).is_ok() {
        return None;
    }
    match embed_query(config, trimmed).await {
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

/// `text` as a search query, from the text embedding service.
pub async fn embed_query(config: &RuntimeConfig, text: &str) -> Result<Vec<f32>, String> {
    let client =
        EmbeddingClient::from_config(config).ok_or("no embedding service is configured")?;
    let vector = client.embed_query(text, QUERY_TIMEOUT).await?;
    if vector.len() != DIMENSIONS {
        return Err(format!(
            "the embedding service returned a {}-dimensional vector, expected {DIMENSIONS}",
            vector.len()
        ));
    }
    Ok(vector)
}

pub trait VectorDoc {
    const KIND: EmbeddedKind;
    fn doc_id(&self) -> &str;
    fn set_vectors(&mut self, vectors: TitleVectors);
}

impl VectorDoc for MovieDocument {
    const KIND: EmbeddedKind = EmbeddedKind::Movie;
    fn doc_id(&self) -> &str {
        &self.id
    }
    fn set_vectors(&mut self, vectors: TitleVectors) {
        self.vectors = Some(vectors);
    }
}

impl VectorDoc for TvShowDocument {
    const KIND: EmbeddedKind = EmbeddedKind::TvShow;
    fn doc_id(&self) -> &str {
        &self.id
    }
    fn set_vectors(&mut self, vectors: TitleVectors) {
        self.vectors = Some(vectors);
    }
}

impl VectorDoc for BookDocument {
    const KIND: EmbeddedKind = EmbeddedKind::Book;
    fn doc_id(&self) -> &str {
        &self.id
    }
    fn set_vectors(&mut self, vectors: TitleVectors) {
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
    let mut stored = search_embeddings::vectors_for(pool, D::KIND, &ids, EMBED_MODEL)
        .await
        .map_err(|e| SearchError::Backend(e.to_string()))?;
    for doc in docs.iter_mut() {
        let vectors = Uuid::parse_str(doc.doc_id())
            .ok()
            .and_then(|id| stored.remove(&id))
            .unwrap_or_default();
        doc.set_vectors(vectors.into());
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
