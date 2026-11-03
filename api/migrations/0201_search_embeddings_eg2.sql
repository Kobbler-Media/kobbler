-- EmbeddingGemma-2 replaces SigLIP2, and a title gets two vectors: its summary (the
-- primary signal — "a slow-burn thriller about grief" is a summary match) and its
-- poster or cover. Both live in one space, so one query vector serves either.
--
-- The SigLIP2 rows are 1152-d vectors from another space; nothing can use them,
-- and the vector job re-embeds every title on its next run.
DELETE FROM search_embeddings;

ALTER TABLE search_embeddings
    ADD COLUMN kind TEXT NOT NULL CHECK (kind IN ('poster', 'description')),
    ADD COLUMN model TEXT NOT NULL;

ALTER TABLE search_embeddings DROP CONSTRAINT search_embeddings_pkey;
ALTER TABLE search_embeddings ADD PRIMARY KEY (entity_type, entity_id, kind);

-- One row per vector a title should have. `source` is what the vector is computed
-- from, so a change to it reads as stale: the images entry for artwork, the text
-- itself for summaries. `title` goes into the summary's document prefix.
-- Summaries are capped at 2000 characters (~500 tokens), inside the text
-- embedder's 1024-token input limit with room for the prefix.
DROP VIEW search_embedding_sources;
CREATE VIEW search_embedding_sources AS
SELECT 'movie'::text AS entity_type, m.id AS entity_id, 'poster'::text AS kind,
       (m.images -> 'poster')::text AS source, NULL::text AS title
FROM movies m
WHERE m.images -> 'poster' ->> 'cached_at' IS NOT NULL
   OR COALESCE(m.images -> 'poster' ->> 'source_url', '') <> ''
UNION ALL
SELECT 'tv_show'::text, t.id, 'poster'::text, (t.images -> 'poster')::text, NULL::text
FROM tv_shows t
WHERE t.images -> 'poster' ->> 'cached_at' IS NOT NULL
   OR COALESCE(t.images -> 'poster' ->> 'source_url', '') <> ''
UNION ALL
SELECT 'book'::text, b.id, 'poster'::text, (b.images -> 'cover')::text, NULL::text
FROM books b
WHERE (b.images -> 'cover' ->> 'cached_at' IS NOT NULL
       OR COALESCE(b.images -> 'cover' ->> 'source_url', '') <> '')
  AND (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)
       OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id))
UNION ALL
SELECT 'movie'::text, m.id, 'description'::text,
       left(concat_ws(E'\n',
                 btrim(m.description),
                 'Genres: ' || NULLIF(array_to_string(m.genres, ', '), ''),
                 'Year: ' || m.year), 2000),
       m.title
FROM movies m
WHERE btrim(COALESCE(m.description, '')) <> ''
UNION ALL
SELECT 'tv_show'::text, t.id, 'description'::text,
       left(concat_ws(E'\n',
                 btrim(t.description),
                 'Genres: ' || NULLIF(array_to_string(t.genres, ', '), ''),
                 'Year: ' || t.year), 2000),
       t.title
FROM tv_shows t
WHERE btrim(COALESCE(t.description, '')) <> ''
UNION ALL
-- Book descriptions are often HTML from the metadata providers.
SELECT 'book'::text, b.id, 'description'::text,
       left(concat_ws(E'\n',
                 btrim(regexp_replace(b.description, '<[^>]*>', ' ', 'g')),
                 'Genres: ' || NULLIF((SELECT string_agg(g.name, ', ' ORDER BY g.name)
                                       FROM book_genres bg JOIN genres g ON g.id = bg.genre_id
                                       WHERE bg.book_id = b.id), ''),
                 'Year: ' || EXTRACT(YEAR FROM b.pubdate)::int), 2000),
       b.title
FROM books b
WHERE btrim(COALESCE(b.description, '')) <> ''
  AND (EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)
       OR EXISTS (SELECT 1 FROM audiobook_files af WHERE af.book_id = b.id));
