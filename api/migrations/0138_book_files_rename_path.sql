-- Rename book_files.path to file_path for consistency with all other file tables
-- (tv_episode_files, movie_files, audiobook_files, external_subtitles).
ALTER TABLE book_files RENAME COLUMN path TO file_path;
