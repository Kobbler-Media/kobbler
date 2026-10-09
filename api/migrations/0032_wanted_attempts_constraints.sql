-- Clean up duplicate wanted_attempts rows before adding unique constraint.
-- Keep only the most recent row per (book_id, source, external_id) combination.
DELETE FROM wanted_attempts wa
WHERE wa.external_id IS NOT NULL
  AND wa.id NOT IN (
    SELECT DISTINCT ON (book_id, source, external_id) id
    FROM wanted_attempts
    WHERE external_id IS NOT NULL
    ORDER BY book_id, source, external_id, created_at DESC
  );

-- Add missing unique constraint for wanted_attempts upsert logic.
-- The upsert_by_external_id() function uses ON CONFLICT (book_id, source, external_id)
-- but this constraint was never created, causing the upsert to fail with a Postgres error
-- (silently swallowed by callers using `let _ =`). No wanted_attempt rows were ever created
-- via the upsert path, which is why download ingestion lost all book metadata.
CREATE UNIQUE INDEX IF NOT EXISTS wanted_attempts_book_source_external_idx
    ON wanted_attempts (book_id, source, external_id)
    WHERE external_id IS NOT NULL;

-- Drop stale index referencing deleted wanted_id column (removed in 0019).
DROP INDEX IF EXISTS wanted_attempts_wanted_id_idx;

-- Add index on external_id for efficient lookups by download ID.
CREATE INDEX IF NOT EXISTS wanted_attempts_external_id_idx
    ON wanted_attempts (external_id)
    WHERE external_id IS NOT NULL;
