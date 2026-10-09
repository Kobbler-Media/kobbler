-- Add unique constraint on series.sort_key
-- Required for ON CONFLICT upsert in atomic ingest endpoint
CREATE UNIQUE INDEX IF NOT EXISTS idx_series_sort_key ON series(sort_key);
