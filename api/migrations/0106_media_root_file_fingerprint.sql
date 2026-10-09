-- Add fast content fingerprint (xxh3 of first+last 64KB) for change detection and rename tracking.
ALTER TABLE media_root_files ADD COLUMN file_fingerprint TEXT;

CREATE INDEX idx_mrf_fingerprint
    ON media_root_files(media_root_id, file_fingerprint)
    WHERE file_fingerprint IS NOT NULL;
