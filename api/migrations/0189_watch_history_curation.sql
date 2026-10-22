-- Rows written before this column existed cannot be told apart, so they stay 'local'.
ALTER TABLE movie_watch_states ADD COLUMN source TEXT NOT NULL DEFAULT 'local';
ALTER TABLE tv_watch_states ADD COLUMN source TEXT NOT NULL DEFAULT 'local';

CREATE TABLE for_you_dismissals (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    media_type TEXT NOT NULL CHECK (media_type IN ('movie', 'tv')),
    tmdb_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, media_type, tmdb_id)
);

CREATE TABLE watch_history_exclusions (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    media_type TEXT NOT NULL CHECK (media_type IN ('movie', 'tv')),
    tmdb_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, media_type, tmdb_id)
);
