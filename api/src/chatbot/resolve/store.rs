//! The Postgres side of resolution.
//!
//! Two things cannot come from the search index: watch state and reading
//! history are per-user, and a document every user shares cannot hold them
//! (that was the v32 bug). `content_category` is not indexed at all, so sports
//! are told apart from television here rather than in Meili.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use uuid::Uuid;

use super::types::{FormatSkew, ResolveError};

/// A book, flattened to what a candidate needs. Ordered by publication date,
/// which the book documents do not carry.
#[derive(Debug, Clone, PartialEq)]
pub struct BookBrief {
    pub id: Uuid,
    pub title: String,
    pub authors: Vec<String>,
    pub author_ids: Vec<Uuid>,
    pub year: Option<i32>,
    pub description: Option<String>,
    pub is_in_library: bool,
    pub has_audiobook: bool,
    pub has_ebook: bool,
}

#[async_trait]
pub trait ResolveStore: Send + Sync {
    /// `tv_shows.content_category` for the given shows. Anything mapping to
    /// `"sport"` is labelled a fixture rather than an episode.
    async fn content_categories(
        &self,
        show_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, String>, ResolveError>;

    /// Which of `episode_ids` this user has already watched.
    async fn watched_episode_ids(
        &self,
        user_id: Uuid,
        episode_ids: &[Uuid],
    ) -> Result<HashSet<Uuid>, ResolveError>;

    /// Which of `movie_ids` this user has already watched.
    async fn watched_movie_ids(
        &self,
        user_id: Uuid,
        movie_ids: &[Uuid],
    ) -> Result<HashSet<Uuid>, ResolveError>;

    /// This user's own ebook/audiobook split, for format inference.
    async fn format_skew(&self, user_id: Uuid) -> Result<FormatSkew, ResolveError>;

    /// Books by an author or in a series, newest first.
    async fn newest_books(
        &self,
        author_id: Option<Uuid>,
        series_id: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<BookBrief>, ResolveError>;
}

pub struct PgResolveStore {
    pool: sqlx::PgPool,
}

impl PgResolveStore {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ResolveStore for PgResolveStore {
    async fn content_categories(
        &self,
        show_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, String>, ResolveError> {
        if show_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = sqlx::query!(
            r#"SELECT id, content_category FROM tv_shows WHERE id = ANY($1)"#,
            show_ids
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.id, r.content_category))
            .collect())
    }

    async fn watched_episode_ids(
        &self,
        user_id: Uuid,
        episode_ids: &[Uuid],
    ) -> Result<HashSet<Uuid>, ResolveError> {
        if episode_ids.is_empty() {
            return Ok(HashSet::new());
        }
        let rows = sqlx::query!(
            r#"
            SELECT episode_id
            FROM tv_watch_states
            WHERE user_id = $1 AND episode_id = ANY($2) AND watched
            "#,
            user_id,
            episode_ids
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.episode_id).collect())
    }

    async fn watched_movie_ids(
        &self,
        user_id: Uuid,
        movie_ids: &[Uuid],
    ) -> Result<HashSet<Uuid>, ResolveError> {
        if movie_ids.is_empty() {
            return Ok(HashSet::new());
        }
        let rows = sqlx::query!(
            r#"
            SELECT movie_id
            FROM movie_watch_states
            WHERE user_id = $1 AND movie_id = ANY($2) AND watched
            "#,
            user_id,
            movie_ids
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.movie_id).collect())
    }

    async fn format_skew(&self, user_id: Uuid) -> Result<FormatSkew, ResolveError> {
        // `reading_bookmark.location_type` is set to 'audiobook' by the audio
        // player and left as a Kobo/EPUB location by the readers, so the split
        // is readable without a second table.
        let row = sqlx::query!(
            r#"
            SELECT
              COUNT(*) FILTER (WHERE bm.location_type = 'audiobook')      AS "audiobook!",
              COUNT(*) FILTER (WHERE bm.location_type IS DISTINCT FROM 'audiobook') AS "ebook!"
            FROM reading_states rs
            JOIN reading_bookmark bm ON bm.reading_state_id = rs.id
            WHERE rs.user_id = $1
            "#,
            user_id
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(FormatSkew {
            ebook: row.ebook,
            audiobook: row.audiobook,
        })
    }

    async fn newest_books(
        &self,
        author_id: Option<Uuid>,
        series_id: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<BookBrief>, ResolveError> {
        let rows = sqlx::query!(
            r#"
            SELECT b.id,
                   b.title,
                   b.description,
                   ARRAY(
                     SELECT a.name FROM book_authors ba
                     JOIN authors a ON a.id = ba.author_id
                     WHERE ba.book_id = b.id
                     ORDER BY ba.author_id
                   ) AS "authors!: Vec<String>",
                   ARRAY(
                     SELECT ba.author_id FROM book_authors ba
                     WHERE ba.book_id = b.id
                     ORDER BY ba.author_id
                   ) AS "author_ids!: Vec<Uuid>",
                   EXTRACT(YEAR FROM COALESCE(b.release_date, b.pubdate::date))::int AS "year?",
                   EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id) AS "has_audiobook!",
                   EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) AS "has_ebook!"
            FROM books b
            WHERE ($1::uuid IS NULL OR EXISTS (
                     SELECT 1 FROM book_authors ba2
                     WHERE ba2.book_id = b.id AND ba2.author_id = $1
                   ))
              AND ($2::uuid IS NULL OR EXISTS (
                     SELECT 1 FROM book_series bs
                     WHERE bs.book_id = b.id AND bs.series_id = $2
                   ))
              AND b.ingest_status IS NULL
            ORDER BY COALESCE(b.release_date, b.pubdate::date) DESC NULLS LAST, b.title ASC
            LIMIT $3
            "#,
            author_id,
            series_id,
            limit
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| BookBrief {
                id: r.id,
                title: r.title,
                authors: r.authors,
                author_ids: r.author_ids,
                year: r.year,
                description: r.description,
                is_in_library: r.has_ebook || r.has_audiobook,
                has_audiobook: r.has_audiobook,
                has_ebook: r.has_ebook,
            })
            .collect())
    }
}
