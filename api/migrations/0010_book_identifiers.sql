-- Add identifiers JSONB column to books table
-- Stores external identifiers like ISBN, ASIN, Goodreads ID, etc.
ALTER TABLE books ADD COLUMN identifiers JSONB NOT NULL DEFAULT '{}';

-- Index for ISBN lookups (common query pattern for metadata matching)
CREATE INDEX idx_books_isbn ON books ((identifiers->>'isbn'));
