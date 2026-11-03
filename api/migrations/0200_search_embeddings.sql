-- One SigLIP2 vector per title artwork, computed on koblem's GPU. Kept here rather than
-- only in Meilisearch because every full reindex replaces the documents, and the vectors
-- would otherwise have to be recomputed on each one.
CREATE TABLE search_embeddings (
    entity_type TEXT NOT NULL CHECK (entity_type IN ('movie', 'tv_show', 'book')),
    entity_id UUID NOT NULL,
    source TEXT NOT NULL,
    embedding REAL[] NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (entity_type, entity_id)
);

-- The artwork each title should have a vector for, keyed by its images entry so a new
-- poster or an uploaded cover reads as a change. Books are limited to the library: the
-- other ~95% are catalogue entries whose covers have never been downloaded.
CREATE VIEW search_embedding_sources AS
SELECT 'movie'::text AS entity_type, m.id AS entity_id, (m.images -> 'poster')::text AS source
FROM movies m
WHERE m.images -> 'poster' ->> 'cached_at' IS NOT NULL
   OR COALESCE(m.images -> 'poster' ->> 'source_url', '') <> ''
UNION ALL
SELECT 'tv_show'::text, t.id, (t.images -> 'poster')::text
FROM tv_shows t
WHERE t.images -> 'poster' ->> 'cached_at' IS NOT NULL
   OR COALESCE(t.images -> 'poster' ->> 'source_url', '') <> ''
UNION ALL
SELECT 'book'::text, b.id, (b.images -> 'cover')::text
FROM books b
WHERE (b.images -> 'cover' ->> 'cached_at' IS NOT NULL
       OR COALESCE(b.images -> 'cover' ->> 'source_url', '') <> '')
  AND (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)
       OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id));
