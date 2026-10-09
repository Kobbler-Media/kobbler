-- Track provenance of book files (where each file came from)

-- Source type: 'kobload', 'calibre_plugin', 'watch_directory', 'manual_upload', 'unknown'
ALTER TABLE book_files ADD COLUMN IF NOT EXISTS source_type TEXT NOT NULL DEFAULT 'unknown';

-- Source metadata as JSON
-- For kobload: {download_id, release_title, indexer, info_hash, size, protocol, download_url}
-- For calibre: {plugin_version}
-- For watch_dir: {scan_job_id}
ALTER TABLE book_files ADD COLUMN IF NOT EXISTS source_metadata JSONB NOT NULL DEFAULT '{}';

-- When the file was added (more reliable than created_at which may be updated on UPSERT)
ALTER TABLE book_files ADD COLUMN IF NOT EXISTS added_at TIMESTAMPTZ NOT NULL DEFAULT NOW();

-- Index for filtering by source type
CREATE INDEX IF NOT EXISTS idx_book_files_source_type ON book_files(source_type);

-- Index for matching releases by info_hash (torrents)
CREATE INDEX IF NOT EXISTS idx_book_files_info_hash ON book_files ((source_metadata->>'info_hash'))
  WHERE source_metadata->>'info_hash' IS NOT NULL;

-- Index for matching releases by release_title (usenet/fallback)
CREATE INDEX IF NOT EXISTS idx_book_files_release_title ON book_files ((source_metadata->>'release_title'))
  WHERE source_metadata->>'release_title' IS NOT NULL;

-- Update existing files to have watch_directory source type if they came from a watch directory
UPDATE book_files
SET source_type = 'watch_directory'
WHERE watch_directory_id IS NOT NULL
  AND source_type = 'unknown';
