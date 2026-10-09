-- Metadata Enrichment Enhancement
-- Adds genres table, book_genres junction table, and new book columns for ratings

--------------------------------------------------------------------------------
-- Normalized genres table
--------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS genres (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    normalized_name TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS book_genres (
    book_id UUID NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    genre_id UUID NOT NULL REFERENCES genres(id) ON DELETE CASCADE,
    PRIMARY KEY (book_id, genre_id)
);

CREATE INDEX IF NOT EXISTS idx_book_genres_genre ON book_genres(genre_id);
CREATE INDEX IF NOT EXISTS idx_genres_normalized ON genres(normalized_name);

--------------------------------------------------------------------------------
-- New book columns for ratings and page count
--------------------------------------------------------------------------------

ALTER TABLE books ADD COLUMN IF NOT EXISTS page_count INTEGER;
ALTER TABLE books ADD COLUMN IF NOT EXISTS rating DOUBLE PRECISION;
ALTER TABLE books ADD COLUMN IF NOT EXISTS rating_count INTEGER;
