CREATE TABLE media_root_dir_mtimes (
    media_root_id UUID NOT NULL REFERENCES media_roots(id) ON DELETE CASCADE,
    relative_path TEXT NOT NULL,
    last_mtime TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (media_root_id, relative_path)
);

CREATE INDEX idx_media_root_dir_mtimes_root ON media_root_dir_mtimes (media_root_id);

COMMENT ON TABLE media_root_dir_mtimes IS
    'Per-directory mtime cache used by media_root_scan to skip read_dir on '
    'unchanged directories. A match means no files were added/removed/renamed '
    'within that directory since the last scan — so walking it is unnecessary. '
    'Does not catch in-place file modifications; those are caught by the '
    'filesystem watcher and by (size, mtime) file-level checks.';
