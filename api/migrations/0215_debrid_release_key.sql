-- A release is a torrent (its 40-hex info hash) or an imported NZB (`nzb-<sha256>`), so
-- the column is renamed rather than migrated: torrent keys, and their cache dirs, stay put.
ALTER TABLE debrid_stream_sessions RENAME COLUMN info_hash TO release_key;
ALTER TABLE debrid_cache_files RENAME COLUMN info_hash TO release_key;

-- An NZB is named by its indexer listing until kobload imports it; only kobload can turn
-- that into a download URL, so indexer API keys never reach kobbler.
ALTER TABLE debrid_stream_sessions
    ADD COLUMN nzb_ref JSONB,
    ADD COLUMN debrid_cache_url TEXT;

CREATE INDEX idx_debrid_sessions_nzb_ref ON debrid_stream_sessions (nzb_ref)
    WHERE nzb_ref IS NOT NULL AND release_key IS NOT NULL;
