-- Intermediate storage for the split skip detection pipeline.
-- Candidates are written by the audio pass and updated by the visual pass,
-- then consumed by the fusion pass to produce final skip segments.

CREATE TABLE skip_detect_candidates (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Scope: either a TV season or a movie file
    season_id UUID REFERENCES tv_seasons(id) ON DELETE CASCADE,
    movie_file_id UUID REFERENCES movie_files(id) ON DELETE CASCADE,
    -- Per-file candidate data
    file_id UUID NOT NULL,
    segment_type TEXT NOT NULL,
    start_seconds DOUBLE PRECISION NOT NULL,
    end_seconds DOUBLE PRECISION NOT NULL,
    -- Audio/boundary signals (populated by audio pass)
    chromaprint_score DOUBLE PRECISION,
    boundary_has_black_frame BOOLEAN,
    boundary_has_silence BOOLEAN,
    boundary_refined_start DOUBLE PRECISION,
    boundary_refined_end DOUBLE PRECISION,
    audio_profile TEXT,
    -- Visual signal (populated by visual pass)
    visual_score DOUBLE PRECISION,
    -- Pass tracking timestamps
    audio_pass_at TIMESTAMPTZ,
    visual_pass_at TIMESTAMPTZ,
    fused_at TIMESTAMPTZ,
    config_tag TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    CHECK (season_id IS NOT NULL OR movie_file_id IS NOT NULL)
);

CREATE INDEX idx_skip_candidates_season ON skip_detect_candidates(season_id) WHERE season_id IS NOT NULL;
CREATE INDEX idx_skip_candidates_movie ON skip_detect_candidates(movie_file_id) WHERE movie_file_id IS NOT NULL;
CREATE INDEX idx_skip_candidates_pending_visual ON skip_detect_candidates(season_id, movie_file_id)
    WHERE audio_pass_at IS NOT NULL AND visual_pass_at IS NULL;
