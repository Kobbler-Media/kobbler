-- A debrid download is analysed as a library file is: per file. An episode or movie
-- can have several releases, each timed differently, so intro/credits, chapters,
-- corrupt ranges and detection diagnostics belong to the file, not to the title nor to
-- whichever session played it. The cache path is the file's identity: it is derived
-- from the release, so a re-download of an evicted file lands on the same row.
CREATE TABLE debrid_files (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    cache_path TEXT NOT NULL UNIQUE,
    media_type TEXT NOT NULL,
    media_id UUID NOT NULL,
    chapters JSONB,
    skip_segments JSONB,
    corrupt_ranges JSONB NOT NULL DEFAULT '[]'::jsonb,
    skip_detect_ran_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_debrid_files_media ON debrid_files (media_type, media_id);

INSERT INTO debrid_files (cache_path, media_type, media_id, chapters, skip_segments)
SELECT DISTINCT ON (cache_path) cache_path, media_type, media_id, chapters, skip_segments
FROM debrid_stream_sessions
WHERE cache_path IS NOT NULL
ORDER BY cache_path, updated_at DESC;

ALTER TABLE debrid_stream_sessions DROP COLUMN skip_segments, DROP COLUMN chapters;

-- Segments cached per title were timed to whichever release produced them.
DROP TABLE tv_episode_cached_segments;
DROP TABLE movie_cached_segments;

ALTER TABLE skip_detect_candidates
    ADD COLUMN debrid_file_id UUID REFERENCES debrid_files(id) ON DELETE CASCADE,
    DROP CONSTRAINT skip_detect_candidates_check,
    ADD CONSTRAINT skip_detect_candidates_check
        CHECK (season_id IS NOT NULL OR movie_file_id IS NOT NULL OR debrid_file_id IS NOT NULL);

CREATE INDEX idx_skip_candidates_debrid_file ON skip_detect_candidates (debrid_file_id)
    WHERE debrid_file_id IS NOT NULL;
