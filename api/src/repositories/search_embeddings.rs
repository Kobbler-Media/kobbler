//! Artwork vectors for semantic search.

use std::collections::HashMap;

use sqlx::PgPool;
use uuid::Uuid;

use super::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmbeddedKind {
    Movie,
    TvShow,
    Book,
}

impl EmbeddedKind {
    pub const ALL: [EmbeddedKind; 3] = [Self::Movie, Self::TvShow, Self::Book];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::TvShow => "tv_show",
            Self::Book => "book",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

#[derive(Debug, Clone)]
pub struct StaleEmbedding {
    pub kind: EmbeddedKind,
    pub entity_id: Uuid,
    /// The title's `images` entry for the artwork, as JSON.
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct EmbeddedTitle {
    pub kind: EmbeddedKind,
    pub entity_id: Uuid,
}

/// Titles whose artwork has no vector yet, or a different one than it was computed from.
pub async fn stale(pool: &PgPool) -> Result<Vec<StaleEmbedding>> {
    let rows = sqlx::query!(
        r#"
        SELECT s.entity_type AS "entity_type!", s.entity_id AS "entity_id!", s.source AS "source!"
        FROM search_embedding_sources s
        LEFT JOIN search_embeddings e
          ON e.entity_type = s.entity_type AND e.entity_id = s.entity_id
        WHERE e.source IS DISTINCT FROM s.source
        ORDER BY s.entity_type, s.entity_id
        "#
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|r| {
            EmbeddedKind::parse(&r.entity_type).map(|kind| StaleEmbedding {
                kind,
                entity_id: r.entity_id,
                source: r.source,
            })
        })
        .collect())
}

/// The source key is read back from the title row at write time, so artwork the
/// embedding run itself had to download and mark cached is not seen as changed next run.
pub async fn upsert(
    pool: &PgPool,
    kind: EmbeddedKind,
    entity_id: Uuid,
    embedding: &[f32],
) -> Result<bool> {
    let result = sqlx::query!(
        r#"
        INSERT INTO search_embeddings (entity_type, entity_id, source, embedding)
        SELECT s.entity_type, s.entity_id, s.source, $3
        FROM search_embedding_sources s
        WHERE s.entity_type = $1 AND s.entity_id = $2
        ON CONFLICT (entity_type, entity_id)
        DO UPDATE SET source = EXCLUDED.source, embedding = EXCLUDED.embedding, created_at = NOW()
        "#,
        kind.as_str(),
        entity_id,
        embedding as &[f32],
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Drops vectors for titles that were deleted, lost their artwork or left the library.
pub async fn prune(pool: &PgPool) -> Result<Vec<EmbeddedTitle>> {
    let rows = sqlx::query!(
        r#"
        DELETE FROM search_embeddings e
        WHERE NOT EXISTS (
            SELECT 1 FROM search_embedding_sources s
            WHERE s.entity_type = e.entity_type AND s.entity_id = e.entity_id
        )
        RETURNING e.entity_type, e.entity_id
        "#
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            EmbeddedKind::parse(&r.entity_type).map(|kind| EmbeddedTitle {
                kind,
                entity_id: r.entity_id,
            })
        })
        .collect())
}

pub async fn vectors_for(
    pool: &PgPool,
    kind: EmbeddedKind,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<f32>>> {
    let rows = sqlx::query!(
        r#"
        SELECT entity_id, embedding
        FROM search_embeddings
        WHERE entity_type = $1 AND entity_id = ANY($2)
        "#,
        kind.as_str(),
        ids,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.entity_id, r.embedding))
        .collect())
}
