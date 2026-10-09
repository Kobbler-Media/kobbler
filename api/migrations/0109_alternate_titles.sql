ALTER TABLE movies ADD COLUMN alternate_titles TEXT[] NOT NULL DEFAULT '{}';
ALTER TABLE tv_shows ADD COLUMN alternate_titles TEXT[] NOT NULL DEFAULT '{}';
