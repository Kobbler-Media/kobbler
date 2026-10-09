-- Prevent duplicate file records when both kobload ingest and media_root_match
-- create records for the same file on disk.

-- Deduplicate existing rows first (keep the oldest by UUIDv7 ordering)
DELETE FROM tv_episode_files a
USING tv_episode_files b
WHERE a.file_path = b.file_path
  AND a.id > b.id;

DELETE FROM movie_files a
USING movie_files b
WHERE a.file_path = b.file_path
  AND a.id > b.id;

ALTER TABLE tv_episode_files ADD CONSTRAINT uq_tv_episode_files_path UNIQUE (file_path);
ALTER TABLE movie_files ADD CONSTRAINT uq_movie_files_path UNIQUE (file_path);
