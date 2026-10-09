-- Single-pass media cache groundwork.
--
-- Per-file cache version + generation timestamp on the three media-bearing
-- tables. Used by the backfill walker (added in a later migration/PR) to find
-- files whose cache predates the current pipeline version.
--
-- New media_cache_runs table records each run of the new media_analyze job
-- (manual button or auto-ingest) with timing, per-branch outcomes, and a
-- side-by-side diff against legacy skip_detect output. This is the A/B
-- comparison surface the comparison panel reads — never written to by the
-- legacy code path.
--
-- media_roots gains a storage_kind column so the dual-write tee at ingest
-- time can skip writing to local cache when the media root is already on a
-- local filesystem (no NAS round-trip to save).

ALTER TABLE tv_episode_files ADD COLUMN media_cache_version INT NOT NULL DEFAULT 0;
ALTER TABLE tv_episode_files ADD COLUMN media_cache_generated_at TIMESTAMPTZ;
ALTER TABLE movie_files      ADD COLUMN media_cache_version INT NOT NULL DEFAULT 0;
ALTER TABLE movie_files      ADD COLUMN media_cache_generated_at TIMESTAMPTZ;
ALTER TABLE audiobook_files  ADD COLUMN media_cache_version INT NOT NULL DEFAULT 0;
ALTER TABLE audiobook_files  ADD COLUMN media_cache_generated_at TIMESTAMPTZ;

ALTER TABLE media_roots
    ADD COLUMN storage_kind TEXT NOT NULL DEFAULT 'network'
    CHECK (storage_kind IN ('local', 'network'));

CREATE TABLE media_cache_runs (
    id              UUID PRIMARY KEY,
    file_id         UUID NOT NULL,
    media_type      TEXT NOT NULL CHECK (media_type IN ('episode', 'movie', 'audiobook')),
    triggered_by    TEXT NOT NULL CHECK (triggered_by IN ('manual', 'auto_ingest', 'backfill')),
    pipeline_version INT NOT NULL,
    started_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at    TIMESTAMPTZ,
    duration_ms     BIGINT,
    -- Per-branch ok/err + size, e.g. {"trickplay": {"ok": true, "count": 27}, "vhash": {...}, ...}
    branch_results  JSONB NOT NULL DEFAULT '{}'::jsonb,
    -- Skip-detect-equivalent results from the cached path: {"credits": {start, end, conf}, "intro": {...}, ...}
    skip_detect_v2_results JSONB,
    -- Snapshot of the legacy skip_detect_candidates rows for the same file at run time, for diff display
    legacy_results  JSONB,
    -- {"credits_delta_s": 0.0, "intro_delta_s": 0.0, "confidence_delta": 0.0, "fidelity_ok": true}
    diff_summary    JSONB
);

CREATE INDEX media_cache_runs_file_idx
    ON media_cache_runs (file_id, started_at DESC);

CREATE INDEX media_cache_runs_started_idx
    ON media_cache_runs (started_at DESC);

CREATE INDEX tv_episode_files_cache_ver_idx
    ON tv_episode_files (media_cache_version);
CREATE INDEX movie_files_cache_ver_idx
    ON movie_files (media_cache_version);
CREATE INDEX audiobook_files_cache_ver_idx
    ON audiobook_files (media_cache_version);
