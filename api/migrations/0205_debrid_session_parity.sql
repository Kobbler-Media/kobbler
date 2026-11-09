-- A debrid session carries what a library file's session gets from its file row.
ALTER TABLE debrid_stream_sessions
    ADD COLUMN lufs DOUBLE PRECISION,
    ADD COLUMN chapters JSONB;

-- Debrid detection named post-credits scenes `post_credits`; everything else calls them
-- `post_credits_scene`.
UPDATE tv_episode_cached_segments
SET segments = replace(segments::text, '"post_credits"', '"post_credits_scene"')::jsonb
WHERE segments::text LIKE '%"post_credits"%';

UPDATE movie_cached_segments
SET segments = replace(segments::text, '"post_credits"', '"post_credits_scene"')::jsonb
WHERE segments::text LIKE '%"post_credits"%';

UPDATE debrid_stream_sessions
SET skip_segments = replace(skip_segments::text, '"post_credits"', '"post_credits_scene"')::jsonb
WHERE skip_segments::text LIKE '%"post_credits"%';
