-- Allow duplicate author and series names.
-- Authors and series are now matched by hardcover_id first, then by name.
-- The unique constraint on normalized names prevented legitimate duplicates
-- (e.g., two different people named "John Smith", two series named "Foundation").

-- Drop the unique indexes on normalized names (keep as non-unique for fast lookups)
DROP INDEX IF EXISTS idx_authors_normalized_name;
CREATE INDEX idx_authors_normalized_name ON authors(normalized_name);

DROP INDEX IF EXISTS idx_series_sort_key;
CREATE INDEX idx_series_sort_key ON series(sort_key);

-- Replace the existing non-unique hardcover indexes with unique partial indexes.
-- This prevents two authors/series from sharing the same hardcover external ID,
-- while allowing rows that have no hardcover ID at all.
DROP INDEX IF EXISTS idx_authors_hardcover;
CREATE UNIQUE INDEX idx_authors_hardcover_id
    ON authors((identifiers->>'hardcover'))
    WHERE identifiers->>'hardcover' IS NOT NULL;

DROP INDEX IF EXISTS idx_series_hardcover;
CREATE UNIQUE INDEX idx_series_hardcover_id
    ON series((identifiers->>'hardcover'))
    WHERE identifiers->>'hardcover' IS NOT NULL;
