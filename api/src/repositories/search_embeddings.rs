//! Summary and artwork vectors for semantic search.

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

/// What a vector is computed from (the `kind` column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VectorKind {
    /// The poster, or a book's cover.
    Poster,
    /// The summary, with genres and year.
    Description,
}

impl VectorKind {
    pub const ALL: [VectorKind; 2] = [Self::Poster, Self::Description];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Poster => "poster",
            Self::Description => "description",
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
    pub vector: VectorKind,
    /// The title's `images` entry for artwork, as JSON; the text itself for a
    /// summary.
    pub source: String,
    /// The title, for a summary's document prefix.
    pub title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EmbeddedTitle {
    pub kind: EmbeddedKind,
    pub entity_id: Uuid,
}

/// A title's stored vectors from one model.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StoredVectors {
    pub poster: Option<Vec<f32>>,
    pub description: Option<Vec<f32>>,
}

/// Vectors that are missing, were computed from a different source, or come from
/// a model other than `model` — so a model switch re-embeds by itself.
pub async fn stale(pool: &PgPool, model: &str) -> Result<Vec<StaleEmbedding>> {
    let rows = sqlx::query!(
        r#"
        SELECT s.entity_type AS "entity_type!", s.entity_id AS "entity_id!",
               s.kind AS "kind!", s.source AS "source!", s.title
        FROM search_embedding_sources s
        LEFT JOIN search_embeddings e
          ON e.entity_type = s.entity_type AND e.entity_id = s.entity_id AND e.kind = s.kind
        WHERE e.source IS DISTINCT FROM s.source
           OR e.model IS DISTINCT FROM $1
        ORDER BY s.kind, s.entity_type, s.entity_id
        "#,
        model,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some(StaleEmbedding {
                kind: EmbeddedKind::parse(&r.entity_type)?,
                entity_id: r.entity_id,
                vector: VectorKind::parse(&r.kind)?,
                source: r.source,
                title: r.title,
            })
        })
        .collect())
}

/// The source key is read back from the view at write time, so artwork the
/// embedding run itself had to download and mark cached is not seen as changed next run.
pub async fn upsert(
    pool: &PgPool,
    kind: EmbeddedKind,
    entity_id: Uuid,
    vector: VectorKind,
    model: &str,
    embedding: &[f32],
) -> Result<bool> {
    let result = sqlx::query!(
        r#"
        INSERT INTO search_embeddings (entity_type, entity_id, kind, model, source, embedding)
        SELECT s.entity_type, s.entity_id, s.kind, $4, s.source, $5
        FROM search_embedding_sources s
        WHERE s.entity_type = $1 AND s.entity_id = $2 AND s.kind = $3
        ON CONFLICT (entity_type, entity_id, kind)
        DO UPDATE SET source = EXCLUDED.source, model = EXCLUDED.model,
                      embedding = EXCLUDED.embedding, created_at = NOW()
        "#,
        kind.as_str(),
        entity_id,
        vector.as_str(),
        model,
        embedding as &[f32],
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Drops vectors for titles that were deleted, lost their artwork or summary, or
/// left the library.
pub async fn prune(pool: &PgPool) -> Result<Vec<EmbeddedTitle>> {
    let rows = sqlx::query!(
        r#"
        DELETE FROM search_embeddings e
        WHERE NOT EXISTS (
            SELECT 1 FROM search_embedding_sources s
            WHERE s.entity_type = e.entity_type AND s.entity_id = e.entity_id
              AND s.kind = e.kind
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

/// Stored vectors from `model` for the given titles. Titles with none are absent.
pub async fn vectors_for(
    pool: &PgPool,
    kind: EmbeddedKind,
    ids: &[Uuid],
    model: &str,
) -> Result<HashMap<Uuid, StoredVectors>> {
    let rows = sqlx::query!(
        r#"
        SELECT entity_id, kind, embedding
        FROM search_embeddings
        WHERE entity_type = $1 AND entity_id = ANY($2) AND model = $3
        "#,
        kind.as_str(),
        ids,
        model,
    )
    .fetch_all(pool)
    .await?;
    let mut out: HashMap<Uuid, StoredVectors> = HashMap::new();
    for r in rows {
        let slot = out.entry(r.entity_id).or_default();
        match VectorKind::parse(&r.kind) {
            Some(VectorKind::Poster) => slot.poster = Some(r.embedding),
            Some(VectorKind::Description) => slot.description = Some(r.embedding),
            None => {}
        }
    }
    Ok(out)
}
