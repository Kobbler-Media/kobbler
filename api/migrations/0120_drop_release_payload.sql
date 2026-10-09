-- Remove release_payload column from wanted_attempts.
-- This untyped JSONB blob has been replaced by typed release_info
-- on the DownloadCompletedEvent callback. Kobbler no longer reads it
-- during ingest — provenance data comes from the structured callback.
ALTER TABLE wanted_attempts DROP COLUMN IF EXISTS release_payload;
