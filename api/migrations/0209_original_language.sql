-- TMDB's `original_language` (ISO 639-1) for the title: what a file's only audio track is
-- presumed to be in when the track itself carries no language tag.
ALTER TABLE movies ADD COLUMN original_language TEXT;
ALTER TABLE tv_shows ADD COLUMN original_language TEXT;
