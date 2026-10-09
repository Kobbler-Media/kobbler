-- Debrid stream session history (persists across server restarts for resume/replay)
CREATE TABLE debrid_stream_sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    media_type TEXT NOT NULL,          -- 'movie' or 'episode'
    media_id UUID NOT NULL,            -- FK to movies or tv_episodes
    release_title TEXT NOT NULL,
    info_hash TEXT,
    debrid_provider TEXT NOT NULL,
    quality JSONB,                     -- parsed quality snapshot
    filename TEXT NOT NULL,
    filesize BIGINT NOT NULL,
    cache_path TEXT,                   -- local temp file path (NULL if evicted)
    cache_expires_at TIMESTAMPTZ,      -- TTL expiry
    pinned BOOLEAN NOT NULL DEFAULT false,
    progress_seconds FLOAT DEFAULT 0,
    probe_data JSONB,                  -- cached ProbeResult
    skip_segments JSONB,               -- cached skip segments
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_debrid_stream_sessions_user ON debrid_stream_sessions(user_id);
CREATE INDEX idx_debrid_stream_sessions_media ON debrid_stream_sessions(media_type, media_id);

-- Debrid cache files (for LRU eviction tracking)
CREATE TABLE debrid_cache_files (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id UUID NOT NULL REFERENCES debrid_stream_sessions(id) ON DELETE CASCADE,
    file_path TEXT NOT NULL UNIQUE,
    file_size BIGINT NOT NULL,
    last_accessed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    pinned BOOLEAN NOT NULL DEFAULT false
);

CREATE INDEX idx_debrid_cache_expires ON debrid_cache_files(expires_at) WHERE NOT pinned;
CREATE INDEX idx_debrid_cache_lru ON debrid_cache_files(last_accessed_at) WHERE NOT pinned;

-- Stremio API keys (per-user, for Stremio addon auth)
CREATE TABLE stremio_api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    api_key TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL DEFAULT 'Default',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_stremio_api_keys_key ON stremio_api_keys(api_key);
