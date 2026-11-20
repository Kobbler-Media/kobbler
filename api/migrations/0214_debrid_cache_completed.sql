-- Up Next waits for a new debrid episode to be on disk, so a finished download has to be
-- visible to SQL rather than only to a stat of the file. complete_cache_entry stamps rows
-- from before this column the first time it finds them whole.
ALTER TABLE debrid_cache_files ADD COLUMN completed_at TIMESTAMPTZ;

CREATE FUNCTION debrid_episode_on_disk(episode UUID) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1 FROM debrid_stream_sessions s
        JOIN debrid_cache_files c ON c.file_path = s.cache_path
        WHERE s.media_type = 'episode' AND s.media_id = episode AND c.completed_at IS NOT NULL
    )
$$;
