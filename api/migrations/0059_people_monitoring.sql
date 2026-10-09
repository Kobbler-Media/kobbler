-- People monitoring (same pattern as authors table in 0019/0021)
ALTER TABLE people ADD COLUMN is_monitored BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE people ADD COLUMN auto_want_mode TEXT NOT NULL DEFAULT 'none'
    CHECK (auto_want_mode IN ('none', 'new_only', 'all'));
ALTER TABLE people ADD COLUMN monitoring_started_at TIMESTAMPTZ;

-- Enrichment scheduling (same pattern as authors)
ALTER TABLE people ADD COLUMN last_metadata_refresh_at TIMESTAMPTZ;
ALTER TABLE people ADD COLUMN next_metadata_refresh_at TIMESTAMPTZ;
ALTER TABLE people ADD COLUMN metadata_refresh_interval_hours INT NOT NULL DEFAULT 168;

-- Dismissed credits (lightweight — no movie/show entries for dismissed items)
CREATE TABLE person_dismissed_credits (
    person_id UUID NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    tmdb_id INTEGER NOT NULL,
    media_type TEXT NOT NULL CHECK (media_type IN ('movie', 'tv')),
    dismissed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (person_id, tmdb_id, media_type)
);
