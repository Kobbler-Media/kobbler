-- Consolidate Authors/Series/Books as the single source of truth.
-- Eliminates tracked_authors, tracked_author_books, wanted_items tables.
-- Authors and Series become first-class entities with tracking capabilities.
-- Books can exist without being in the library (is_in_library=false).

--------------------------------------------------------------------------------
-- Authors table - add metadata and tracking columns
--------------------------------------------------------------------------------

-- External identifiers (e.g., {"hardcover": "123", "goodreads": "456"})
ALTER TABLE authors ADD COLUMN IF NOT EXISTS identifiers JSONB NOT NULL DEFAULT '{}';

-- Author metadata
ALTER TABLE authors ADD COLUMN IF NOT EXISTS image_url TEXT;
ALTER TABLE authors ADD COLUMN IF NOT EXISTS bio TEXT;
ALTER TABLE authors ADD COLUMN IF NOT EXISTS metadata JSONB NOT NULL DEFAULT '{}';

-- Tracking properties
ALTER TABLE authors ADD COLUMN IF NOT EXISTS is_tracked BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE authors ADD COLUMN IF NOT EXISTS auto_want_mode TEXT NOT NULL DEFAULT 'none';
-- 'none' | 'new_only' | 'all'
ALTER TABLE authors ADD COLUMN IF NOT EXISTS tracking_started_at TIMESTAMPTZ;

-- Metadata refresh scheduling
ALTER TABLE authors ADD COLUMN IF NOT EXISTS last_metadata_refresh_at TIMESTAMPTZ;
ALTER TABLE authors ADD COLUMN IF NOT EXISTS next_metadata_refresh_at TIMESTAMPTZ;
ALTER TABLE authors ADD COLUMN IF NOT EXISTS metadata_refresh_interval_hours INT NOT NULL DEFAULT 168;
ALTER TABLE authors ADD COLUMN IF NOT EXISTS known_book_count INT;

-- Timestamps
ALTER TABLE authors ADD COLUMN IF NOT EXISTS created_at TIMESTAMPTZ NOT NULL DEFAULT NOW();
ALTER TABLE authors ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW();

-- Indexes for authors
CREATE INDEX IF NOT EXISTS idx_authors_is_tracked ON authors(is_tracked) WHERE is_tracked = TRUE;
CREATE INDEX IF NOT EXISTS idx_authors_next_refresh ON authors(next_metadata_refresh_at)
    WHERE identifiers != '{}';
CREATE INDEX IF NOT EXISTS idx_authors_hardcover ON authors((identifiers->>'hardcover'));

--------------------------------------------------------------------------------
-- Series table - add metadata and tracking columns
--------------------------------------------------------------------------------

ALTER TABLE series ADD COLUMN IF NOT EXISTS identifiers JSONB NOT NULL DEFAULT '{}';
ALTER TABLE series ADD COLUMN IF NOT EXISTS image_url TEXT;
ALTER TABLE series ADD COLUMN IF NOT EXISTS description TEXT;
ALTER TABLE series ADD COLUMN IF NOT EXISTS total_book_count INT;
ALTER TABLE series ADD COLUMN IF NOT EXISTS metadata JSONB NOT NULL DEFAULT '{}';

-- Tracking properties
ALTER TABLE series ADD COLUMN IF NOT EXISTS is_tracked BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE series ADD COLUMN IF NOT EXISTS auto_want_mode TEXT NOT NULL DEFAULT 'none';
-- 'none' | 'new_only' | 'all' | 'fill_gaps'
ALTER TABLE series ADD COLUMN IF NOT EXISTS tracking_started_at TIMESTAMPTZ;

-- Metadata refresh scheduling
ALTER TABLE series ADD COLUMN IF NOT EXISTS last_metadata_refresh_at TIMESTAMPTZ;
ALTER TABLE series ADD COLUMN IF NOT EXISTS next_metadata_refresh_at TIMESTAMPTZ;
ALTER TABLE series ADD COLUMN IF NOT EXISTS metadata_refresh_interval_hours INT NOT NULL DEFAULT 168;

-- Indexes for series
CREATE INDEX IF NOT EXISTS idx_series_is_tracked ON series(is_tracked) WHERE is_tracked = TRUE;
CREATE INDEX IF NOT EXISTS idx_series_hardcover ON series((identifiers->>'hardcover'));

--------------------------------------------------------------------------------
-- Books table - add library status and want status columns
--------------------------------------------------------------------------------

-- Books can exist without being in the library
ALTER TABLE books ADD COLUMN IF NOT EXISTS is_in_library BOOLEAN NOT NULL DEFAULT TRUE;

-- Want status replaces wanted_items.status for non-library books
ALTER TABLE books ADD COLUMN IF NOT EXISTS want_status TEXT NOT NULL DEFAULT 'none';
-- 'none' | 'pending' | 'wanted' | 'skipped'

-- External cover URL (before we have local file)
ALTER TABLE books ADD COLUMN IF NOT EXISTS cover_url TEXT;

-- Release info (for upcoming/unreleased books)
ALTER TABLE books ADD COLUMN IF NOT EXISTS release_date DATE;
ALTER TABLE books ADD COLUMN IF NOT EXISTS is_released BOOLEAN NOT NULL DEFAULT TRUE;

-- Discovery tracking
ALTER TABLE books ADD COLUMN IF NOT EXISTS first_seen_at TIMESTAMPTZ;
ALTER TABLE books ADD COLUMN IF NOT EXISTS discovered_from_author_id UUID REFERENCES authors(id) ON DELETE SET NULL;

-- Indexes for books
CREATE INDEX IF NOT EXISTS idx_books_is_in_library ON books(is_in_library);
CREATE INDEX IF NOT EXISTS idx_books_want_status ON books(want_status) WHERE want_status != 'none';
CREATE INDEX IF NOT EXISTS idx_books_not_owned ON books(id) WHERE is_in_library = FALSE;
CREATE INDEX IF NOT EXISTS idx_books_hardcover ON books((identifiers->>'hardcover'));

--------------------------------------------------------------------------------
-- Migrate wanted_attempts to reference books directly
--------------------------------------------------------------------------------

-- Add book_id column to wanted_attempts (parallel to wanted_id for migration)
ALTER TABLE wanted_attempts ADD COLUMN IF NOT EXISTS book_id UUID REFERENCES books(id) ON DELETE CASCADE;

--------------------------------------------------------------------------------
-- Data Migration: Move tracked_authors data to authors table
--------------------------------------------------------------------------------

-- For each tracked_author, find matching author by name and update it,
-- or create a new author if not found (using the name from tracked_authors)

-- First, update existing authors that match tracked_authors by name
UPDATE authors a
SET
    identifiers = jsonb_build_object('hardcover', ta.provider_id),
    image_url = COALESCE(a.image_url, ta.image_url),
    metadata = COALESCE(ta.metadata, '{}'),
    is_tracked = NOT ta.is_implicit AND ta.enabled,
    auto_want_mode = ta.auto_want_mode,
    tracking_started_at = ta.tracking_started_at,
    last_metadata_refresh_at = ta.last_checked_at,
    next_metadata_refresh_at = ta.next_check_at,
    known_book_count = ta.known_book_count,
    updated_at = NOW()
FROM tracked_authors ta
WHERE LOWER(a.name) = LOWER(ta.name)
  AND ta.provider = 'hardcover';

-- Insert authors that don't exist yet (tracked authors without library match)
INSERT INTO authors (id, name, identifiers, image_url, metadata, is_tracked, auto_want_mode,
                     tracking_started_at, last_metadata_refresh_at, next_metadata_refresh_at,
                     known_book_count, created_at, updated_at)
SELECT
    gen_random_uuid(),
    ta.name,
    jsonb_build_object('hardcover', ta.provider_id),
    ta.image_url,
    COALESCE(ta.metadata, '{}'),
    NOT ta.is_implicit AND ta.enabled,
    ta.auto_want_mode,
    ta.tracking_started_at,
    ta.last_checked_at,
    ta.next_check_at,
    ta.known_book_count,
    ta.created_at,
    NOW()
FROM tracked_authors ta
WHERE ta.provider = 'hardcover'
  AND NOT EXISTS (
    SELECT 1 FROM authors a WHERE LOWER(a.name) = LOWER(ta.name)
  )
ON CONFLICT (name) DO UPDATE SET
    identifiers = EXCLUDED.identifiers,
    image_url = COALESCE(authors.image_url, EXCLUDED.image_url),
    is_tracked = EXCLUDED.is_tracked,
    auto_want_mode = EXCLUDED.auto_want_mode,
    tracking_started_at = EXCLUDED.tracking_started_at,
    updated_at = NOW();

--------------------------------------------------------------------------------
-- Data Migration: Move tracked_author_books data to books table
--------------------------------------------------------------------------------

-- Create mapping table to track the relationship for migration
CREATE TEMP TABLE tracked_book_migration AS
SELECT
    tab.id as tracked_book_id,
    tab.provider_book_id,
    tab.title,
    tab.cover_url,
    tab.series_name,
    tab.series_id as provider_series_id,
    tab.series_index,
    tab.release_date,
    tab.is_released,
    tab.want_status,
    tab.wanted_item_id,
    tab.book_id as existing_book_id,
    a.id as author_id,
    ta.name as author_name
FROM tracked_author_books tab
JOIN tracked_authors ta ON ta.id = tab.tracked_author_id
LEFT JOIN authors a ON LOWER(a.name) = LOWER(ta.name);

-- For tracked books that already link to a library book, update the book's metadata
UPDATE books b
SET
    identifiers = b.identifiers || jsonb_build_object('hardcover', m.provider_book_id),
    cover_url = COALESCE(b.cover_url, m.cover_url),
    release_date = COALESCE(b.release_date, m.release_date),
    is_released = m.is_released,
    first_seen_at = COALESCE(b.first_seen_at, NOW()),
    discovered_from_author_id = COALESCE(b.discovered_from_author_id, m.author_id),
    updated_at = NOW()
FROM tracked_book_migration m
WHERE b.id = m.existing_book_id
  AND m.existing_book_id IS NOT NULL;

-- For tracked books without a library book, create new book records
INSERT INTO books (id, title, identifiers, cover_url, series_id, series_index_float,
                   is_in_library, want_status, release_date, is_released,
                   first_seen_at, discovered_from_author_id, created_at, updated_at)
SELECT
    gen_random_uuid(),
    m.title,
    jsonb_build_object('hardcover', m.provider_book_id),
    m.cover_url,
    s.id,  -- Link to existing series if we can find it
    m.series_index,
    FALSE,  -- Not in library
    CASE
        WHEN m.want_status = 'owned' THEN 'none'  -- Already handled by existing_book_id
        ELSE m.want_status
    END,
    m.release_date,
    m.is_released,
    NOW(),
    m.author_id,
    NOW(),
    NOW()
FROM tracked_book_migration m
LEFT JOIN series s ON LOWER(s.name) = LOWER(m.series_name)
WHERE m.existing_book_id IS NULL
  AND m.want_status != 'owned';  -- Don't create duplicate for owned books

-- Link the newly created books to authors
INSERT INTO book_authors (book_id, author_id)
SELECT DISTINCT b.id, m.author_id
FROM books b
JOIN tracked_book_migration m ON
    b.identifiers->>'hardcover' = m.provider_book_id
    AND b.is_in_library = FALSE
WHERE m.author_id IS NOT NULL
ON CONFLICT (book_id, author_id) DO NOTHING;

--------------------------------------------------------------------------------
-- Data Migration: Move wanted_items to books
--------------------------------------------------------------------------------

-- For wanted_items without a tracked_author_books link, create new book records
INSERT INTO books (id, title, identifiers, cover_url, is_in_library, want_status,
                   first_seen_at, created_at, updated_at)
SELECT
    wi.id,  -- Reuse the wanted_item ID as book ID for easier reference
    wi.title,
    CASE
        WHEN wi.provider = 'hardcover' AND wi.provider_id IS NOT NULL
            THEN jsonb_build_object('hardcover', wi.provider_id)
        ELSE '{}'
    END ||
    CASE WHEN wi.isbn10 IS NOT NULL THEN jsonb_build_object('isbn10', wi.isbn10) ELSE '{}' END ||
    CASE WHEN wi.isbn13 IS NOT NULL THEN jsonb_build_object('isbn13', wi.isbn13) ELSE '{}' END,
    wi.cover_url,
    FALSE,  -- Not in library
    CASE
        WHEN wi.status IN ('completed', 'downloaded') THEN 'none'
        WHEN wi.status = 'failed' THEN 'skipped'
        ELSE 'wanted'
    END,
    wi.created_at,
    wi.created_at,
    wi.updated_at
FROM wanted_items wi
WHERE NOT EXISTS (
    SELECT 1 FROM books b WHERE b.identifiers->>'hardcover' = wi.provider_id
)
AND NOT EXISTS (
    SELECT 1 FROM tracked_author_books tab WHERE tab.wanted_item_id = wi.id
)
ON CONFLICT (id) DO NOTHING;

-- Create authors from wanted_items.author if they don't exist
INSERT INTO authors (id, name, created_at, updated_at)
SELECT DISTINCT
    gen_random_uuid(),
    wi.author,
    NOW(),
    NOW()
FROM wanted_items wi
WHERE wi.author IS NOT NULL
  AND wi.author != ''
  AND NOT EXISTS (
    SELECT 1 FROM authors a WHERE LOWER(a.name) = LOWER(wi.author)
  )
ON CONFLICT (name) DO NOTHING;

-- Link wanted_items books to authors
INSERT INTO book_authors (book_id, author_id)
SELECT wi.id, a.id
FROM wanted_items wi
JOIN authors a ON LOWER(a.name) = LOWER(wi.author)
WHERE wi.author IS NOT NULL
  AND EXISTS (SELECT 1 FROM books b WHERE b.id = wi.id)
ON CONFLICT (book_id, author_id) DO NOTHING;

-- Update wanted_attempts to point to books instead of wanted_items
UPDATE wanted_attempts wa
SET book_id = COALESCE(
    -- First try to find via tracked_author_books
    (SELECT b.id FROM books b
     JOIN tracked_author_books tab ON b.identifiers->>'hardcover' = tab.provider_book_id
     WHERE tab.wanted_item_id = wa.wanted_id
     LIMIT 1),
    -- Then try direct wanted_item -> book (where we used wanted_item.id as book.id)
    (SELECT b.id FROM books b WHERE b.id = wa.wanted_id),
    -- Finally try by provider_id match
    (SELECT b.id FROM books b
     JOIN wanted_items wi ON b.identifiers->>'hardcover' = wi.provider_id
     WHERE wi.id = wa.wanted_id
     LIMIT 1)
)
WHERE wa.book_id IS NULL;

--------------------------------------------------------------------------------
-- Drop old tables after migration
--------------------------------------------------------------------------------

-- Drop the foreign key constraint first
ALTER TABLE tracked_author_books DROP CONSTRAINT IF EXISTS tracked_author_books_wanted_item_id_fkey;
ALTER TABLE tracked_author_books DROP CONSTRAINT IF EXISTS tracked_author_books_book_id_fkey;
ALTER TABLE tracked_author_books DROP CONSTRAINT IF EXISTS tracked_author_books_tracked_author_id_fkey;

-- Drop old columns from wanted_attempts
ALTER TABLE wanted_attempts DROP CONSTRAINT IF EXISTS wanted_attempts_wanted_id_fkey;
ALTER TABLE wanted_attempts DROP COLUMN IF EXISTS wanted_id;

-- Drop the old tables
DROP TABLE IF EXISTS tracked_author_books;
DROP TABLE IF EXISTS tracked_authors;
DROP TABLE IF EXISTS wanted_items;

-- Clean up temp table
DROP TABLE IF EXISTS tracked_book_migration;

--------------------------------------------------------------------------------
-- Make book_id NOT NULL in wanted_attempts (after migration)
--------------------------------------------------------------------------------

-- Delete any orphaned attempts that couldn't be migrated
DELETE FROM wanted_attempts WHERE book_id IS NULL;

-- Now make it required
ALTER TABLE wanted_attempts ALTER COLUMN book_id SET NOT NULL;
