ALTER TABLE movies
    ADD COLUMN IF NOT EXISTS preferred_languages TEXT[];

ALTER TABLE tv_shows
    ADD COLUMN IF NOT EXISTS preferred_languages TEXT[];
