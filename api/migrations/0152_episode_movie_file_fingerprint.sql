-- Carry the scanner's lightweight content fingerprint (xxh3 of first+last 64KB,
-- see media_root_files.file_fingerprint / 0106) onto the playback file rows so a
-- same-path content replace can be detected at upsert time and invalidate the
-- byte-exact caches (keyframes, duration, lufs, corrupt_ranges, media_cache).
-- The scan path supplies this; kobload ingest supplies file_size instead.
ALTER TABLE tv_episode_files ADD COLUMN file_fingerprint TEXT;

ALTER TABLE movie_files ADD COLUMN file_fingerprint TEXT;
