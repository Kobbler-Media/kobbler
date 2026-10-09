-- Monitoring System Restructure
-- - Rename is_tracked -> is_monitored on authors (clearer terminology)
-- - Add is_monitored to series (parallel to authors)
-- - Both authors and series can now be independently monitored

--------------------------------------------------------------------------------
-- Authors: Rename is_tracked -> is_monitored
--------------------------------------------------------------------------------

-- Rename columns
ALTER TABLE authors RENAME COLUMN is_tracked TO is_monitored;
ALTER TABLE authors RENAME COLUMN tracking_started_at TO monitoring_started_at;

-- Update index
DROP INDEX IF EXISTS idx_authors_is_tracked;
CREATE INDEX IF NOT EXISTS idx_authors_is_monitored ON authors(is_monitored) WHERE is_monitored = TRUE;

--------------------------------------------------------------------------------
-- Series: Add monitoring columns (parallel to authors)
--------------------------------------------------------------------------------

-- Add is_monitored column (series can be independently monitored)
ALTER TABLE series ADD COLUMN IF NOT EXISTS is_monitored BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE series ADD COLUMN IF NOT EXISTS monitoring_started_at TIMESTAMPTZ;

-- Index for efficient filtering
CREATE INDEX IF NOT EXISTS idx_series_is_monitored ON series(is_monitored) WHERE is_monitored = TRUE;
