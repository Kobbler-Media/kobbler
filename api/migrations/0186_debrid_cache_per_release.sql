-- A cached debrid file belongs to its release (provider + info_hash + file_id), not to the
-- session that happened to download it: deleting that session used to cascade the cache
-- row away and orphan a fully downloaded file that the next session for the same release
-- could have played from disk. Eviction is left to debrid_cache_cleanup (TTL/LRU/size).
ALTER TABLE debrid_stream_sessions ADD COLUMN debrid_provider_id UUID;

ALTER TABLE debrid_cache_files
    ADD COLUMN debrid_provider_id UUID,
    ADD COLUMN info_hash TEXT,
    ADD COLUMN file_id INTEGER,
    ADD COLUMN filename TEXT;

UPDATE debrid_cache_files c
SET info_hash = s.info_hash,
    file_id = s.file_id,
    filename = s.filename,
    debrid_provider_id = CASE
        WHEN s.debrid_provider ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
        THEN s.debrid_provider::uuid
    END
FROM debrid_stream_sessions s
WHERE s.id = c.session_id;

ALTER TABLE debrid_cache_files DROP COLUMN session_id;

CREATE INDEX idx_debrid_cache_release ON debrid_cache_files (info_hash, file_id);
