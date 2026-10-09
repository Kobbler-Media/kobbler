-- Drop `thumbnails_generated` columns — the V2 `media_analyze` pipeline writes
-- manifest-driven cache coverage under `{storage_root}/media_cache/<file_id>/`
-- and the streaming session response now derives thumbnail availability from
-- the manifest's `artefacts.trickplay.present` flag instead of a DB boolean.
-- LUFS columns stay (written by media_analyze's loudness branch post-processing
-- for movies/episodes; by enrich_audiobook for audiobooks).

ALTER TABLE movie_files       DROP COLUMN IF EXISTS thumbnails_generated;
ALTER TABLE tv_episode_files  DROP COLUMN IF EXISTS thumbnails_generated;
ALTER TABLE iptv_recordings   DROP COLUMN IF EXISTS thumbnails_generated;
