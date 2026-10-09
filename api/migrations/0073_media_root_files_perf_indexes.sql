-- Composite indexes for media_root_files query performance.
-- Covers the common (media_root_id + match_status + available) filter pattern
-- used by get_pending, get_identified, list_for_root, count_by_status, etc.

-- Supersedes idx_media_root_files_match_status(media_root_id, match_status)
-- but we leave that in place to avoid migration complexity.
CREATE INDEX IF NOT EXISTS idx_media_root_files_root_status_avail
    ON media_root_files (media_root_id, match_status, available);

-- Covers get_known_file_state, mark_missing_unavailable, count_ungrouped
CREATE INDEX IF NOT EXISTS idx_media_root_files_root_avail
    ON media_root_files (media_root_id, available);
