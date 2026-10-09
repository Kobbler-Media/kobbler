-- TVDB uses a single entity where TMDB may have multiple (e.g. show reboots),
-- so tvdb_id is not necessarily unique across rows. Replace unique indexes with
-- regular indexes to allow shared tvdb_id values while keeping lookup performance.

DROP INDEX idx_tv_shows_identifiers_tvdb;
DROP INDEX idx_movies_identifiers_tvdb;
DROP INDEX idx_people_identifiers_tvdb;

CREATE INDEX idx_tv_shows_identifiers_tvdb
    ON tv_shows ((identifiers->>'tvdb_id'));

CREATE INDEX idx_movies_identifiers_tvdb
    ON movies ((identifiers->>'tvdb_id'));

CREATE INDEX idx_people_identifiers_tvdb
    ON people ((identifiers->>'tvdb_id'));
