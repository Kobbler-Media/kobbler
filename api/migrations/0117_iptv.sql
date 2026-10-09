-- IPTV integration: providers, channels, sources, EPG, recordings, favourites

-- ============================================================================
-- Xtream Codes provider credentials and sync config
-- ============================================================================
CREATE TABLE iptv_providers (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL,
    url         TEXT NOT NULL,
    username    TEXT NOT NULL,
    password    TEXT NOT NULL,
    preferred_format TEXT NOT NULL DEFAULT 'm3u8',
    sync_interval_secs INTEGER NOT NULL DEFAULT 3600,
    enabled     BOOLEAN NOT NULL DEFAULT TRUE,
    account_info JSONB NOT NULL DEFAULT '{}',
    last_sync_at TIMESTAMPTZ,
    last_sync_error TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(url, username)
);

-- ============================================================================
-- Channels: the primary user-facing entity. One row = one logical channel.
-- ============================================================================
CREATE TABLE iptv_channels (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name            TEXT NOT NULL,
    normalized_name TEXT NOT NULL,
    external_id     TEXT,
    language        TEXT,
    country         TEXT,
    category        TEXT,
    icon_url        TEXT,
    epg_channel_id  TEXT,
    enabled         BOOLEAN NOT NULL DEFAULT TRUE,
    display_order   INTEGER NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(normalized_name, language, country)
);
CREATE INDEX idx_iptv_channels_category ON iptv_channels(category);
CREATE INDEX idx_iptv_channels_enabled ON iptv_channels(enabled) WHERE enabled = TRUE;
CREATE INDEX idx_iptv_channels_epg ON iptv_channels(epg_channel_id) WHERE epg_channel_id IS NOT NULL;

-- ============================================================================
-- Sources: streams from providers, linked to channels. Invisible to users.
-- ============================================================================
CREATE TABLE iptv_sources (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    channel_id          UUID NOT NULL REFERENCES iptv_channels(id) ON DELETE CASCADE,
    provider_id         UUID NOT NULL REFERENCES iptv_providers(id) ON DELETE CASCADE,
    remote_stream_id    BIGINT NOT NULL,
    remote_category_id  BIGINT,
    remote_category_name TEXT,
    name                TEXT,
    icon_url            TEXT,
    epg_channel_id      TEXT,
    stream_type         TEXT NOT NULL DEFAULT 'live',
    quality             TEXT NOT NULL DEFAULT 'unknown',
    priority            INTEGER NOT NULL DEFAULT 50,
    health_score        REAL NOT NULL DEFAULT 1.0,
    last_health_check_at TIMESTAMPTZ,
    last_seen_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    match_method        TEXT,
    match_confidence    REAL NOT NULL DEFAULT 1.0,
    metadata            JSONB NOT NULL DEFAULT '{}',
    UNIQUE(provider_id, remote_stream_id)
);
CREATE INDEX idx_iptv_sources_channel ON iptv_sources(channel_id);
CREATE INDEX idx_iptv_sources_provider ON iptv_sources(provider_id);
CREATE INDEX idx_iptv_sources_failover ON iptv_sources(channel_id, priority ASC, health_score DESC);

-- ============================================================================
-- Unmatched sources: staging area for streams that couldn't be auto-matched
-- ============================================================================
CREATE TABLE iptv_unmatched_sources (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    provider_id         UUID NOT NULL REFERENCES iptv_providers(id) ON DELETE CASCADE,
    remote_stream_id    BIGINT NOT NULL,
    remote_category_id  BIGINT,
    remote_category_name TEXT,
    name                TEXT,
    icon_url            TEXT,
    epg_channel_id      TEXT,
    stream_type         TEXT NOT NULL DEFAULT 'live',
    language            TEXT,
    suggested_channel_id UUID REFERENCES iptv_channels(id) ON DELETE SET NULL,
    suggested_confidence REAL,
    dismissed           BOOLEAN NOT NULL DEFAULT FALSE,
    last_seen_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(provider_id, remote_stream_id)
);
CREATE INDEX idx_iptv_unmatched_provider ON iptv_unmatched_sources(provider_id);
CREATE INDEX idx_iptv_unmatched_pending ON iptv_unmatched_sources(dismissed) WHERE dismissed = FALSE;

-- ============================================================================
-- Channel groups: curated navigation categories (Sports, News, etc.)
-- ============================================================================
CREATE TABLE iptv_channel_groups (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name            TEXT NOT NULL,
    slug            TEXT NOT NULL UNIQUE,
    icon            TEXT,
    display_order   INTEGER NOT NULL DEFAULT 0,
    auto_assign_categories TEXT[] NOT NULL DEFAULT '{}',
    enabled         BOOLEAN NOT NULL DEFAULT TRUE
);

INSERT INTO iptv_channel_groups (name, slug, icon, display_order, auto_assign_categories) VALUES
    ('Sports',        'sports',        'trophy',   1, '{sports,football,cricket,rugby,tennis,basketball,hockey,golf,racing,boxing,mma,wrestling}'),
    ('News',          'news',          'newspaper', 2, '{news,weather}'),
    ('Movies',        'movies',        'film',      3, '{movies,cinema,film}'),
    ('Entertainment', 'entertainment', 'tv',        4, '{entertainment,general,variety}'),
    ('Kids',          'kids',          'baby',      5, '{kids,children,cartoon,animation}'),
    ('Music',         'music',         'music',     6, '{music}'),
    ('Documentary',   'documentary',   'globe',     7, '{documentary,nature,science,history,education}');

-- ============================================================================
-- EPG: program guide entries, linked to channels (not providers)
-- ============================================================================
CREATE TABLE iptv_epg (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    channel_id  UUID NOT NULL REFERENCES iptv_channels(id) ON DELETE CASCADE,
    title       TEXT,
    description TEXT,
    start_time  TIMESTAMPTZ NOT NULL,
    stop_time   TIMESTAMPTZ NOT NULL,
    categories  TEXT[] NOT NULL DEFAULT '{}',
    episode_info JSONB,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(channel_id, start_time, stop_time)
);
CREATE INDEX idx_iptv_epg_channel_time ON iptv_epg(channel_id, start_time, stop_time);
CREATE INDEX idx_iptv_epg_time_range ON iptv_epg(start_time, stop_time);

-- ============================================================================
-- Recordings: scheduled and completed program recordings
-- ============================================================================
CREATE TABLE iptv_recordings (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    channel_id      UUID NOT NULL REFERENCES iptv_channels(id) ON DELETE CASCADE,
    epg_id          UUID REFERENCES iptv_epg(id) ON DELETE SET NULL,
    title           TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'scheduled',
    start_time      TIMESTAMPTZ NOT NULL,
    end_time        TIMESTAMPTZ NOT NULL,
    pre_padding_secs  INTEGER NOT NULL DEFAULT 60,
    post_padding_secs INTEGER NOT NULL DEFAULT 120,
    file_path       TEXT,
    file_size_bytes BIGINT,
    error_message   TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_iptv_recordings_status ON iptv_recordings(status, start_time);
CREATE INDEX idx_iptv_recordings_channel ON iptv_recordings(channel_id);

-- ============================================================================
-- Series rules: "record all episodes of X"
-- ============================================================================
CREATE TABLE iptv_series_rules (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    title_pattern   TEXT NOT NULL,
    channel_id      UUID REFERENCES iptv_channels(id) ON DELETE SET NULL,
    record_new_only BOOLEAN NOT NULL DEFAULT TRUE,
    keep_count      INTEGER,
    pre_padding_secs  INTEGER NOT NULL DEFAULT 60,
    post_padding_secs INTEGER NOT NULL DEFAULT 120,
    enabled         BOOLEAN NOT NULL DEFAULT TRUE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ============================================================================
-- Channel aliases: learned from manual corrections + iptv-org alt_names
-- ============================================================================
CREATE TABLE iptv_channel_aliases (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    channel_id       UUID NOT NULL REFERENCES iptv_channels(id) ON DELETE CASCADE,
    normalized_alias TEXT NOT NULL,
    source           TEXT NOT NULL DEFAULT 'manual',
    UNIQUE(normalized_alias)
);
CREATE INDEX idx_iptv_aliases_channel ON iptv_channel_aliases(channel_id);

-- ============================================================================
-- Favourites: user's pinned channels
-- ============================================================================
CREATE TABLE iptv_favourites (
    user_id       UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    channel_id    UUID NOT NULL REFERENCES iptv_channels(id) ON DELETE CASCADE,
    display_order INTEGER NOT NULL DEFAULT 0,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, channel_id)
);

-- ============================================================================
-- Watch history: per-user, per-channel tracking
-- ============================================================================
CREATE TABLE iptv_watch_history (
    user_id        UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    channel_id     UUID NOT NULL REFERENCES iptv_channels(id) ON DELETE CASCADE,
    watch_count    INTEGER NOT NULL DEFAULT 1,
    last_watched_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    duration_secs  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (user_id, channel_id)
);

-- ============================================================================
-- Sync log: audit trail per sync run
-- ============================================================================
CREATE TABLE iptv_sync_log (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    provider_id     UUID NOT NULL REFERENCES iptv_providers(id) ON DELETE CASCADE,
    started_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at    TIMESTAMPTZ,
    status          TEXT NOT NULL DEFAULT 'running',
    channels_matched  INTEGER NOT NULL DEFAULT 0,
    sources_added     INTEGER NOT NULL DEFAULT 0,
    sources_removed   INTEGER NOT NULL DEFAULT 0,
    unmatched_count   INTEGER NOT NULL DEFAULT 0,
    epg_entries       INTEGER NOT NULL DEFAULT 0,
    error_message   TEXT
);
CREATE INDEX idx_iptv_sync_log_provider ON iptv_sync_log(provider_id, started_at DESC);

-- ============================================================================
-- Programme watches: EPG alert patterns
-- ============================================================================
CREATE TABLE iptv_programme_watches (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    title_pattern TEXT NOT NULL,
    match_mode  TEXT NOT NULL DEFAULT 'contains',
    enabled     BOOLEAN NOT NULL DEFAULT TRUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_iptv_watches_user ON iptv_programme_watches(user_id);

-- Singleton config row for IPTV settings.
CREATE TABLE iptv_config (
    id                          INTEGER PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    default_format              TEXT,
    recording_path              TEXT,
    epg_update_interval_secs    INTEGER,
    channel_cleanup_enabled     BOOLEAN,
    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
