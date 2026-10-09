-- Cross-episode / per-movie cached skip-segments keyed by the canonical
-- DB id (not by file_id). Populated by debrid_process after a successful
-- analyze + cross-episode/movie detect, so subsequent debrid streams of the
-- same episode/movie reuse the result without re-running detect. Native
-- ingest may also write here as an authoritative cache (separate from the
-- existing per-file episode_skip_segments / movie_skip_segments tables,
-- which stay file-keyed for in-library playback).
--
-- ON DELETE CASCADE handles cleanup automatically when the parent row goes.

CREATE TABLE tv_episode_cached_segments (
    episode_id UUID PRIMARY KEY REFERENCES tv_episodes(id) ON DELETE CASCADE,
    -- Wire-format match to ChapterSkipSegment[] — same shape the debrid
    -- session row stores in skip_segments JSONB.
    segments JSONB NOT NULL,
    -- Pipeline version of the analyze that produced these segments.
    -- Future readers reject rows below MIN_SUPPORTED_PIPELINE_VERSION.
    pipeline_version INTEGER NOT NULL,
    generated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE movie_cached_segments (
    movie_id UUID PRIMARY KEY REFERENCES movies(id) ON DELETE CASCADE,
    segments JSONB NOT NULL,
    pipeline_version INTEGER NOT NULL,
    generated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
