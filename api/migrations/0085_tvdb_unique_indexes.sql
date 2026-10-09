-- Add unique indexes on identifiers->>'tvdb_id' for provider-agnostic upserts.
-- Mirrors the existing tmdb_id unique indexes on these tables.

CREATE UNIQUE INDEX idx_tv_shows_identifiers_tvdb
    ON tv_shows ((identifiers->>'tvdb_id'));

CREATE UNIQUE INDEX idx_movies_identifiers_tvdb
    ON movies ((identifiers->>'tvdb_id'));

CREATE UNIQUE INDEX idx_people_identifiers_tvdb
    ON people ((identifiers->>'tvdb_id'));
