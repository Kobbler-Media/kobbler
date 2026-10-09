-- Add unique index on thesportsdb_id for tv_shows so ON CONFLICT upsert works
CREATE UNIQUE INDEX idx_tv_shows_identifiers_thesportsdb
    ON tv_shows ((identifiers->>'thesportsdb_id'));
