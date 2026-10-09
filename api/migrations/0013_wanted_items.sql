-- Wanted items and attempts
CREATE TABLE IF NOT EXISTS wanted_items (
    id UUID PRIMARY KEY,
    kind TEXT NOT NULL,
    provider TEXT NOT NULL,
    provider_id TEXT,
    title TEXT NOT NULL,
    author TEXT,
    series TEXT,
    series_index DOUBLE PRECISION,
    isbn10 TEXT,
    isbn13 TEXT,
    status TEXT NOT NULL,
    last_attempt_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS wanted_attempts (
    id UUID PRIMARY KEY,
    wanted_id UUID NOT NULL REFERENCES wanted_items (id) ON DELETE CASCADE,
    source TEXT NOT NULL,
    external_id TEXT,
    release_payload JSONB,
    download_client_id INTEGER,
    download_url TEXT,
    status TEXT NOT NULL,
    message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS wanted_items_status_idx ON wanted_items (status);
CREATE INDEX IF NOT EXISTS wanted_items_provider_id_idx ON wanted_items (provider, provider_id);
CREATE INDEX IF NOT EXISTS wanted_items_isbn10_idx ON wanted_items (isbn10);
CREATE INDEX IF NOT EXISTS wanted_items_isbn13_idx ON wanted_items (isbn13);
CREATE INDEX IF NOT EXISTS wanted_attempts_wanted_id_idx ON wanted_attempts (wanted_id);
CREATE INDEX IF NOT EXISTS wanted_attempts_created_at_idx ON wanted_attempts (created_at);
