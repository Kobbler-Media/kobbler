-- Prevent duplicate book titles within the same series.
-- This is a DB-level backstop for application-level dedup checks.

-- Clean up any existing duplicates first: keep the library book, or the oldest.
WITH dupes AS (
    SELECT id,
           ROW_NUMBER() OVER (
               PARTITION BY series_id, LOWER(title)
               ORDER BY is_in_library DESC, created_at ASC
           ) AS rn
    FROM books
    WHERE series_id IS NOT NULL
),
to_delete AS (
    SELECT id FROM dupes WHERE rn > 1
)
DELETE FROM books WHERE id IN (SELECT id FROM to_delete);

CREATE UNIQUE INDEX idx_books_unique_title_in_series
ON books (series_id, LOWER(title))
WHERE series_id IS NOT NULL;
