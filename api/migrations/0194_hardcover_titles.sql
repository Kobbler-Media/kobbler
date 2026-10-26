-- Rebuilt weekly from Hardcover's most-read books, so backups leave it out.
CREATE TABLE hardcover_titles (
    hardcover_id INTEGER PRIMARY KEY,
    title TEXT NOT NULL,
    author TEXT,
    users_count INTEGER NOT NULL
);

-- The suggestions query must repeat these expressions verbatim to use the indexes.
CREATE INDEX idx_hardcover_titles_title ON hardcover_titles USING gin (lower(title) gin_trgm_ops);
CREATE INDEX idx_hardcover_titles_author ON hardcover_titles USING gin (lower(author) gin_trgm_ops);
CREATE INDEX idx_hardcover_titles_users ON hardcover_titles (users_count DESC);

CREATE TABLE hardcover_title_syncs (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    attempted_at TIMESTAMPTZ,
    synced_at TIMESTAMPTZ
);
