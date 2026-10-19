-- A NULL user_id is the instance-wide account every profile falls back to; a profile's own
-- account (user_id set) takes precedence over it.
CREATE TABLE stremio_accounts (
    id UUID PRIMARY KEY,
    user_id UUID UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    email TEXT NOT NULL,
    auth_key TEXT NOT NULL,
    last_synced_at TIMESTAMPTZ,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX stremio_accounts_one_instance_account
    ON stremio_accounts ((user_id IS NULL)) WHERE user_id IS NULL;

-- kind: 'resume' (part-watched), 'up_next' (Stremio advanced past a finished episode),
-- 'new_episode' (caught up, and TMDB has the episode after the last one watched).
CREATE TABLE stremio_continue_items (
    account_id UUID NOT NULL REFERENCES stremio_accounts(id) ON DELETE CASCADE,
    stremio_id TEXT NOT NULL,
    media_type TEXT NOT NULL CHECK (media_type IN ('movie', 'tv')),
    kind TEXT NOT NULL CHECK (kind IN ('resume', 'up_next', 'new_episode')),
    tmdb_id TEXT NOT NULL,
    title TEXT NOT NULL,
    year INT,
    description TEXT,
    poster_url TEXT,
    backdrop_url TEXT,
    genres TEXT[] NOT NULL DEFAULT '{}',
    rating DOUBLE PRECISION,
    content_rating TEXT,
    season INT,
    episode INT,
    episode_title TEXT,
    air_date DATE,
    season_premiere BOOLEAN NOT NULL DEFAULT FALSE,
    position_ms BIGINT NOT NULL DEFAULT 0,
    duration_ms BIGINT NOT NULL DEFAULT 0,
    last_watched TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (account_id, stremio_id)
);

CREATE INDEX stremio_continue_items_tmdb ON stremio_continue_items (media_type, tmdb_id);

-- Everything the account has ever played, removed items included. Series record Stremio's
-- position marker rather than a per-episode list: everything before (season, episode) has been
-- watched, and the marked episode too when marker_watched.
CREATE TABLE stremio_history_items (
    account_id UUID NOT NULL REFERENCES stremio_accounts(id) ON DELETE CASCADE,
    stremio_id TEXT NOT NULL,
    media_type TEXT NOT NULL CHECK (media_type IN ('movie', 'tv')),
    name TEXT NOT NULL,
    tmdb_id TEXT,
    title TEXT,
    year INT,
    poster_url TEXT,
    content_rating TEXT,
    resolved_at TIMESTAMPTZ,
    season INT,
    episode INT,
    marker_watched BOOLEAN NOT NULL DEFAULT FALSE,
    watched BOOLEAN NOT NULL DEFAULT FALSE,
    position_ms BIGINT NOT NULL DEFAULT 0,
    last_watched TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (account_id, stremio_id)
);

-- applied_watched_at is the history item's last_watched as of the last import, so a re-run only
-- touches titles Stremio has played since.
CREATE TABLE stremio_history_imports (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    account_id UUID NOT NULL,
    stremio_id TEXT NOT NULL,
    applied_watched_at TIMESTAMPTZ,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, account_id, stremio_id),
    FOREIGN KEY (account_id, stremio_id)
        REFERENCES stremio_history_items(account_id, stremio_id) ON DELETE CASCADE
);

-- A title the library holds is judged by the library's Kids rule (manual override included);
-- anything else by the TMDB rating stored at sync time.
CREATE FUNCTION stremio_title_kids_allowed(p_media_type TEXT, p_tmdb_id TEXT, p_rating TEXT)
RETURNS BOOLEAN LANGUAGE sql STABLE AS $$
    SELECT COALESCE(
        (SELECT kids_allowed(m.kids_override, m.content_rating) FROM movies m
         WHERE p_media_type = 'movie' AND m.identifiers->>'tmdb_id' = p_tmdb_id LIMIT 1),
        (SELECT kids_allowed(s.kids_override, s.content_rating) FROM tv_shows s
         WHERE p_media_type = 'tv' AND s.identifiers->>'tmdb_id' = p_tmdb_id LIMIT 1),
        kids_allowed(NULL, p_rating)
    )
$$;
