-- Track which config was active when enrichment jobs ran.
-- NULL = legacy (pre-config-tag), treated as stale by backfill.
ALTER TABLE episode_skip_segments ADD COLUMN config_tag TEXT;
ALTER TABLE movie_skip_segments ADD COLUMN config_tag TEXT;
ALTER TABLE audiobook_transcripts ADD COLUMN config_tag TEXT;
