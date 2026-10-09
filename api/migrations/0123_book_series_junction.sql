-- Many-to-many relationship between books and series.
-- A book can belong to multiple series (e.g., "Into the Wild" is both
-- "Warriors #1" and "Warriors: The Prophecies Begin #1").
-- The is_primary flag marks which series is displayed in general listings.

CREATE TABLE book_series (
    book_id UUID NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    series_id UUID NOT NULL REFERENCES series(id) ON DELETE CASCADE,
    series_index DOUBLE PRECISION,
    is_primary BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (book_id, series_id)
);

-- Backfill from existing denormalized columns
INSERT INTO book_series (book_id, series_id, series_index, is_primary)
SELECT id, series_id, series_index, TRUE FROM books WHERE series_id IS NOT NULL;

CREATE INDEX idx_book_series_series_id ON book_series(series_id);

-- At most one primary series per book
CREATE UNIQUE INDEX idx_book_series_one_primary ON book_series(book_id) WHERE is_primary = TRUE;

-- Drop the old denormalized columns — junction table is now the single source of truth
ALTER TABLE books DROP COLUMN series_id;
ALTER TABLE books DROP COLUMN series_index;
