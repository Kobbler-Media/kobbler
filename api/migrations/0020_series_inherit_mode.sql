-- Series auto_want_mode 'inherit' feature
-- Author is the tracked element; series has auto_want_mode as an optional override.
-- 'inherit' (default) = use author's mode; explicit values override author for that series.

--------------------------------------------------------------------------------
-- Remove tracking columns from series (author is the tracked element)
--------------------------------------------------------------------------------

-- Remove is_tracked column (series doesn't have its own tracking state)
ALTER TABLE series DROP COLUMN IF EXISTS is_tracked;

-- Remove tracking_started_at (use author's tracking_started_at for date checks)
ALTER TABLE series DROP COLUMN IF EXISTS tracking_started_at;

-- Drop the index that referenced is_tracked
DROP INDEX IF EXISTS idx_series_is_tracked;

--------------------------------------------------------------------------------
-- Update auto_want_mode default and existing values
--------------------------------------------------------------------------------

-- Change default auto_want_mode to 'inherit' for series
ALTER TABLE series ALTER COLUMN auto_want_mode SET DEFAULT 'inherit';

-- Update existing series with 'none' to use 'inherit' (the new default behavior)
-- This preserves the same behavior: 'none' meant "use author's mode" which is now 'inherit'
UPDATE series SET auto_want_mode = 'inherit' WHERE auto_want_mode = 'none';
