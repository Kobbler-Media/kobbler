-- Search index metadata for tracking schema version and sync state
CREATE TABLE IF NOT EXISTS search_index_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

-- Initialize with default values
INSERT INTO search_index_meta (key, value) VALUES
    ('schema_version', '0'),  -- Will be set to current version on first reindex
    ('last_reindex', '');     -- ISO timestamp of last full reindex
