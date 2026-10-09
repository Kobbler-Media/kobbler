-- Add full metadata fields to ingest queue (matching book metadata fields)
ALTER TABLE ingest_queue ADD COLUMN IF NOT EXISTS extracted_series TEXT;
ALTER TABLE ingest_queue ADD COLUMN IF NOT EXISTS extracted_series_index DOUBLE PRECISION;
ALTER TABLE ingest_queue ADD COLUMN IF NOT EXISTS extracted_language TEXT;
ALTER TABLE ingest_queue ADD COLUMN IF NOT EXISTS extracted_publisher TEXT;
ALTER TABLE ingest_queue ADD COLUMN IF NOT EXISTS extracted_pubdate TIMESTAMPTZ;
