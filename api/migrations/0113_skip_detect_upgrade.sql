-- Upgrade skip detection: expand segment types, add confidence scoring,
-- movie skip segments, and fingerprint caching.

-- Expand segment types for post-credits scenes and recaps
ALTER TABLE episode_skip_segments
  DROP CONSTRAINT episode_skip_segments_segment_type_check;
ALTER TABLE episode_skip_segments
  ADD CONSTRAINT episode_skip_segments_segment_type_check
  CHECK (segment_type IN ('intro', 'credits', 'post_credits_scene', 'recap'));

-- Allow multiple segments per file (e.g. credits + post-credits scene)
-- Old index was unique on (episode_file_id, segment_type) — too restrictive
DROP INDEX idx_skip_segments_file_type;
CREATE UNIQUE INDEX idx_skip_segments_file_type_start
  ON episode_skip_segments(episode_file_id, segment_type, start_seconds);

-- Add confidence score from detection pipeline
ALTER TABLE episode_skip_segments ADD COLUMN confidence DOUBLE PRECISION;

-- Movie skip segments (movies have credits + post-credits, no intros)
CREATE TABLE movie_skip_segments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    movie_file_id UUID NOT NULL REFERENCES movie_files(id) ON DELETE CASCADE,
    segment_type TEXT NOT NULL CHECK (segment_type IN ('credits', 'post_credits_scene')),
    start_seconds DOUBLE PRECISION NOT NULL,
    end_seconds DOUBLE PRECISION NOT NULL,
    source TEXT NOT NULL DEFAULT 'pipeline',
    confidence DOUBLE PRECISION,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX idx_movie_skip_segments_file_type_start
  ON movie_skip_segments(movie_file_id, segment_type, start_seconds);

-- Chromaprint fingerprint cache (avoid re-extracting on reprocessing)
CREATE TABLE media_fingerprints (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    file_id UUID NOT NULL,
    file_type TEXT NOT NULL CHECK (file_type IN ('movie', 'episode')),
    region TEXT NOT NULL CHECK (region IN ('start', 'end')),
    fingerprint BYTEA NOT NULL,
    duration_seconds DOUBLE PRECISION NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(file_id, file_type, region)
);
