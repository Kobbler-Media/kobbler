-- Merge ingest_queue into books with ingest_status column.
-- Books can now represent both library items (ingest_status IS NULL) and
-- items in the ingest queue (ingest_status = 'extracting', 'pending', 'converting', 'error', 'rejected').

--------------------------------------------------------------------------------
-- Step 1: Add ingest_status column to books table
--------------------------------------------------------------------------------

ALTER TABLE books ADD COLUMN IF NOT EXISTS ingest_status TEXT;

-- Index for efficient filtering: ingest queue queries need fast access to non-null statuses
CREATE INDEX IF NOT EXISTS idx_books_ingest_status ON books(ingest_status) WHERE ingest_status IS NOT NULL;

--------------------------------------------------------------------------------
-- Step 2: Create book_ingest_source table for ingest-specific metadata
--------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS book_ingest_source (
    book_id UUID PRIMARY KEY REFERENCES books(id) ON DELETE CASCADE,
    -- Source file info
    file_path TEXT NOT NULL,
    file_name TEXT NOT NULL,
    file_size BIGINT NOT NULL,
    file_hash TEXT,
    source_mtime TIMESTAMPTZ,
    source_format TEXT NOT NULL,
    -- Cover extracted from file (stored here until approval writes to filesystem)
    extracted_cover BYTEA,
    -- Pending authors/series: rich JSONB data, created as real records on approval
    -- Format: [{"name": "...", "hardcover_id": "...", "image_url": "..."}]
    pending_authors JSONB DEFAULT '[]'::jsonb,
    -- Format: {"name": "...", "hardcover_id": "..."}
    pending_series JSONB,
    pending_series_index FLOAT,
    -- Source metadata: provenance info (source_type, provider, external_id, subjects, etc.)
    -- Used during approval to populate book_files.source_metadata, genres, identifiers
    source_metadata JSONB,
    -- Error tracking
    conversion_error TEXT,
    -- References
    watch_directory_id UUID REFERENCES watch_directories(id) ON DELETE SET NULL,
    update_target_book_id UUID REFERENCES books(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_book_ingest_source_file_hash
    ON book_ingest_source(file_hash) WHERE file_hash IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_book_ingest_source_watch_dir
    ON book_ingest_source(watch_directory_id) WHERE watch_directory_id IS NOT NULL;

--------------------------------------------------------------------------------
-- Step 3: Migrate existing ingest_queue data
--------------------------------------------------------------------------------

-- For non-approved items: create book + ingest_source record
-- These become books with ingest_status set to their queue status
INSERT INTO books (id, title, description, language, publisher, pubdate,
                   is_in_library, want_status, ingest_status, created_at, updated_at)
SELECT
    iq.id,
    COALESCE(iq.extracted_title, iq.file_name),
    iq.extracted_description,
    iq.extracted_language,
    iq.extracted_publisher,
    iq.extracted_pubdate,
    FALSE,
    'none',
    iq.status,  -- 'pending', 'extracting', 'converting', 'error', 'rejected'
    iq.created_at,
    COALESCE(iq.updated_at, iq.created_at)
FROM ingest_queue iq
WHERE iq.status != 'approved'
  AND NOT EXISTS (SELECT 1 FROM books b WHERE b.id = iq.id);

-- Create ingest_source records for non-approved items
-- Authors/series go to pending_* fields (will link existing ones in step 4)
INSERT INTO book_ingest_source (book_id, file_path, file_name, file_size, file_hash,
                                 source_mtime, source_format, extracted_cover,
                                 pending_authors, pending_series, pending_series_index,
                                 conversion_error, watch_directory_id,
                                 update_target_book_id, created_at)
SELECT
    iq.id,
    iq.file_path,
    iq.file_name,
    iq.file_size,
    iq.file_hash,
    iq.source_mtime,
    iq.source_format,
    iq.extracted_cover,
    -- Convert extracted_authors array to JSONB array of objects
    COALESCE(
        (SELECT jsonb_agg(jsonb_build_object('name', author_name))
         FROM unnest(iq.extracted_authors) AS author_name
         WHERE author_name IS NOT NULL AND author_name != ''),
        '[]'::jsonb
    ),
    -- Convert extracted_series to JSONB object
    CASE
        WHEN iq.extracted_series IS NOT NULL AND iq.extracted_series != ''
        THEN jsonb_build_object('name', iq.extracted_series)
        ELSE NULL
    END,
    iq.extracted_series_index,
    iq.conversion_error,
    iq.watch_directory_id,
    iq.update_target_book_id,
    iq.created_at
FROM ingest_queue iq
WHERE iq.status != 'approved';

-- For approved items: add ingest_source to existing book for provenance tracking
INSERT INTO book_ingest_source (book_id, file_path, file_name, file_size, file_hash,
                                 source_mtime, source_format,
                                 watch_directory_id, created_at)
SELECT
    iq.book_id,
    iq.file_path,
    iq.file_name,
    iq.file_size,
    iq.file_hash,
    iq.source_mtime,
    iq.source_format,
    iq.watch_directory_id,
    iq.created_at
FROM ingest_queue iq
WHERE iq.status = 'approved' AND iq.book_id IS NOT NULL
ON CONFLICT (book_id) DO NOTHING;

--------------------------------------------------------------------------------
-- Step 4: Link ingest books to EXISTING authors (don't create new ones)
--------------------------------------------------------------------------------

-- Create temp table with author names from pending_authors
CREATE TEMP TABLE temp_ingest_authors AS
SELECT
    bis.book_id,
    (author_obj->>'name') AS author_name
FROM book_ingest_source bis,
     jsonb_array_elements(bis.pending_authors) AS author_obj
WHERE bis.pending_authors IS NOT NULL
  AND jsonb_array_length(bis.pending_authors) > 0;

-- Link books to authors that ALREADY EXIST (case-insensitive match on normalized_name)
INSERT INTO book_authors (book_id, author_id)
SELECT DISTINCT tia.book_id, a.id
FROM temp_ingest_authors tia
JOIN authors a ON LOWER(a.name) = LOWER(tia.author_name)
   OR a.normalized_name = LOWER(REGEXP_REPLACE(tia.author_name, '[^a-zA-Z0-9]', '', 'g'))
WHERE tia.author_name IS NOT NULL AND tia.author_name != ''
ON CONFLICT (book_id, author_id) DO NOTHING;

-- Remove linked authors from pending_authors
UPDATE book_ingest_source bis
SET pending_authors = (
    SELECT COALESCE(jsonb_agg(author_obj), '[]'::jsonb)
    FROM jsonb_array_elements(bis.pending_authors) AS author_obj
    WHERE NOT EXISTS (
        SELECT 1 FROM book_authors ba
        JOIN authors a ON a.id = ba.author_id
        WHERE ba.book_id = bis.book_id
          AND (LOWER(a.name) = LOWER(author_obj->>'name')
               OR a.normalized_name = LOWER(REGEXP_REPLACE(author_obj->>'name', '[^a-zA-Z0-9]', '', 'g')))
    )
)
WHERE pending_authors IS NOT NULL AND jsonb_array_length(pending_authors) > 0;

DROP TABLE temp_ingest_authors;

--------------------------------------------------------------------------------
-- Step 5: Link ingest books to EXISTING series (don't create new ones)
--------------------------------------------------------------------------------

-- Link books to series that ALREADY EXIST
UPDATE books b
SET
    series_id = s.id,
    series_index_float = bis.pending_series_index
FROM book_ingest_source bis
JOIN series s ON LOWER(s.name) = LOWER(bis.pending_series->>'name')
WHERE b.id = bis.book_id
  AND bis.pending_series IS NOT NULL
  AND bis.pending_series->>'name' IS NOT NULL
  AND b.ingest_status IS NOT NULL;

-- Clear pending_series for books that got linked
UPDATE book_ingest_source bis
SET pending_series = NULL, pending_series_index = NULL
WHERE EXISTS (
    SELECT 1 FROM books b
    WHERE b.id = bis.book_id AND b.series_id IS NOT NULL
)
AND pending_series IS NOT NULL;

--------------------------------------------------------------------------------
-- Step 6: Drop old ingest_queue table
--------------------------------------------------------------------------------

DROP TABLE IF EXISTS ingest_queue;
