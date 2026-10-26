CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- Rebuilt from TMDB's daily ID exports and its discover lists, so backups leave it out.
CREATE TABLE tmdb_titles (
    media_type TEXT NOT NULL CHECK (media_type IN ('movie', 'tv')),
    tmdb_id INTEGER NOT NULL,
    original_title TEXT NOT NULL,
    title TEXT,
    popularity REAL NOT NULL,
    PRIMARY KEY (media_type, tmdb_id)
);

-- The suggestions query must repeat this expression verbatim to use the index.
CREATE INDEX idx_tmdb_titles_search
    ON tmdb_titles USING gin (lower(COALESCE(title || ' ', '') || original_title) gin_trgm_ops);
CREATE INDEX idx_tmdb_titles_popularity ON tmdb_titles (popularity DESC);

CREATE TABLE tmdb_title_syncs (
    media_type TEXT PRIMARY KEY CHECK (media_type IN ('movie', 'tv')),
    export_date DATE,
    localized_at TIMESTAMPTZ
);
