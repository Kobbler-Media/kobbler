-- Store the file_id from multi-file torrents (season packs) so we can
-- look up cached files by (info_hash, file_id) without hitting debrid.
ALTER TABLE debrid_stream_sessions ADD COLUMN file_id INTEGER;

-- Index for the cache lookup: find a previous session for this torrent file
CREATE INDEX idx_debrid_sessions_cache_lookup
    ON debrid_stream_sessions (info_hash, file_id)
    WHERE cache_path IS NOT NULL;
