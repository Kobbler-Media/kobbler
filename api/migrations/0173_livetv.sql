-- Live TV: an HDHomeRun DVB-T tuner as a first-class source.
--
-- Deliberately NOT an iptv_providers row. The IPTV stack exists to match 10k+
-- channel names against iptv-org, fuzzy-dedup them, health-score providers and
-- fail over between sources. A tuner has ~107 services, one physical source per
-- channel, and changes only on rescan.

-- ============================================================================
-- Devices
-- ============================================================================
CREATE TABLE livetv_tuners (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    device_id          TEXT NOT NULL UNIQUE,
    friendly_name      TEXT NOT NULL,
    model_number       TEXT,
    firmware_version   TEXT,
    base_url           TEXT NOT NULL,
    device_auth        TEXT,
    tuner_count        INTEGER NOT NULL DEFAULT 1,
    last_scan_at       TIMESTAMPTZ,
    last_guide_sync_at TIMESTAMPTZ,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ============================================================================
-- Channels.
--
-- The PK is a uuid surrogate like everywhere else in kobbler, but the identity
-- of a service is physical: (frequency, program_number). That survives an LCN
-- renumber on rescan, which is why curation can be preserved across scans.
-- `lcn` is scan-derived and is only the join key for the guide feed.
-- ============================================================================
CREATE TABLE livetv_channels (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tuner_id        UUID NOT NULL REFERENCES livetv_tuners(id) ON DELETE CASCADE,

    -- scan-derived: a rescan overwrites all of these
    lcn             INTEGER,
    scan_name       TEXT,
    callsign        TEXT,
    frequency       BIGINT NOT NULL,
    program_number  INTEGER NOT NULL,
    onid            INTEGER,
    tsid            INTEGER,
    modulation      TEXT,
    video_codec     TEXT,
    audio_codec     TEXT,
    hd              BOOLEAN NOT NULL DEFAULT FALSE,
    signal_strength INTEGER,
    signal_quality  INTEGER,
    last_seen_at    TIMESTAMPTZ,

    -- curated: a rescan never touches any of these
    enabled         BOOLEAN NOT NULL DEFAULT FALSE,
    favourite       BOOLEAN NOT NULL DEFAULT FALSE,
    sort_order      INTEGER NOT NULL DEFAULT 0,
    display_name    TEXT,
    logo_url        TEXT,
    kind            TEXT NOT NULL DEFAULT 'tv' CHECK (kind IN ('tv', 'radio', 'dead')),
    region          TEXT,
    network         TEXT,

    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tuner_id, frequency, program_number)
);
CREATE INDEX idx_livetv_channels_lcn ON livetv_channels (tuner_id, lcn);
CREATE INDEX idx_livetv_channels_enabled ON livetv_channels (enabled) WHERE enabled = TRUE;
CREATE INDEX idx_livetv_channels_kind ON livetv_channels (kind);
CREATE INDEX idx_livetv_channels_network ON livetv_channels (network) WHERE network IS NOT NULL;

-- ============================================================================
-- Guide. Sourced from the tuner's own guide.php feed (JSON, not XMLTV) —
-- richer than the XMLTV path: genres, SeriesID, episode numbers, artwork.
-- 48 h ceiling, refreshed 6-hourly, so rows are wholly refetched and the
-- channel join is re-derived rather than migrated.
-- ============================================================================
CREATE TABLE livetv_guide (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    channel_id       UUID NOT NULL REFERENCES livetv_channels(id) ON DELETE CASCADE,
    start_time       TIMESTAMPTZ NOT NULL,
    end_time         TIMESTAMPTZ NOT NULL,
    title            TEXT NOT NULL,
    episode_number   TEXT,
    episode_title    TEXT,
    synopsis         TEXT,
    original_airdate TIMESTAMPTZ,
    series_id        TEXT,
    genres           TEXT[] NOT NULL DEFAULT '{}',
    image_url        TEXT,
    poster_url       TEXT,
    is_first         BOOLEAN NOT NULL DEFAULT FALSE,

    -- Episode identity for repeat suppression (Decision 6). Resolved at ingest
    -- from series_id plus the first available of episode_number, episode_title,
    -- original_airdate, else title-only. `episode_key_kind` records which, so
    -- the expander can refuse to auto-record on the title-only fallback —
    -- `Home Shopping` otherwise collapses into a single 45-airing blob.
    episode_key      TEXT,
    episode_key_kind TEXT CHECK (episode_key_kind IN ('episode', 'subtitle', 'airdate', 'title')),

    synced_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (channel_id, start_time)
);
CREATE INDEX idx_livetv_guide_window ON livetv_guide (start_time, end_time);
CREATE INDEX idx_livetv_guide_series ON livetv_guide (series_id) WHERE series_id IS NOT NULL;
CREATE INDEX idx_livetv_guide_episode_key ON livetv_guide (episode_key) WHERE episode_key IS NOT NULL;
CREATE INDEX idx_livetv_guide_channel_start ON livetv_guide (channel_id, start_time DESC);

-- ============================================================================
-- Series rules. A rule is series + channel allow-list + optional weekday/time
-- window + new-only — not just a series. The same SeriesID covers a network
-- first run and a sister channel's back-catalogue rerun, so scoping to channels
-- and a time window is what separates them.
-- ============================================================================
CREATE TABLE livetv_series_rules (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    series_id            TEXT NOT NULL,
    title                TEXT NOT NULL,
    enabled              BOOLEAN NOT NULL DEFAULT TRUE,
    channel_ids          UUID[] NOT NULL DEFAULT '{}',
    weekdays             SMALLINT[] NOT NULL DEFAULT '{}',
    window_start         TIME,
    window_end           TIME,
    new_only             BOOLEAN NOT NULL DEFAULT TRUE,
    min_season           INTEGER,
    keep_count           INTEGER,
    keep_until           TEXT NOT NULL DEFAULT 'space' CHECK (keep_until IN ('watched', 'space', 'deleted', 'date')),
    keep_until_date      TIMESTAMPTZ,
    padding_before_secs  INTEGER NOT NULL DEFAULT 120,
    padding_after_secs   INTEGER NOT NULL DEFAULT 360,
    on_clash             TEXT NOT NULL DEFAULT 'move' CHECK (on_clash IN ('move', 'drop', 'ask')),
    priority             INTEGER NOT NULL DEFAULT 0,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_livetv_series_rules_enabled ON livetv_series_rules (enabled) WHERE enabled = TRUE;
CREATE INDEX idx_livetv_series_rules_series ON livetv_series_rules (series_id);

-- ============================================================================
-- Recordings. Written as HLS segments so they are watchable from segment one
-- while the back half is still being captured (Decision 5).
-- ============================================================================
CREATE TABLE livetv_recordings (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    channel_id          UUID NOT NULL REFERENCES livetv_channels(id) ON DELETE CASCADE,
    guide_id            UUID REFERENCES livetv_guide(id) ON DELETE SET NULL,
    series_rule_id      UUID REFERENCES livetv_series_rules(id) ON DELETE SET NULL,
    title               TEXT NOT NULL,
    episode_number      TEXT,
    episode_title       TEXT,
    episode_key         TEXT,
    series_id           TEXT,
    synopsis            TEXT,
    scheduled_start     TIMESTAMPTZ NOT NULL,
    scheduled_end       TIMESTAMPTZ NOT NULL,
    actual_start        TIMESTAMPTZ,
    actual_end          TIMESTAMPTZ,
    padding_before_secs INTEGER NOT NULL DEFAULT 0,
    padding_after_secs  INTEGER NOT NULL DEFAULT 0,
    status              TEXT NOT NULL DEFAULT 'scheduled'
                        CHECK (status IN ('scheduled', 'recording', 'completed', 'failed', 'cancelled', 'moved')),
    tuner_index         INTEGER,
    output_dir          TEXT,
    duration_seconds    DOUBLE PRECISION,
    video_codec         TEXT,
    audio_codec         TEXT,
    container           TEXT,
    size_bytes          BIGINT,
    -- Set when the recording has been matched to a library title (Decision 16).
    episode_id          UUID REFERENCES tv_episodes(id) ON DELETE SET NULL,
    movie_id            UUID REFERENCES movies(id) ON DELETE SET NULL,
    match_attempted_at  TIMESTAMPTZ,
    -- The airing this was moved to when a clash was resolved (Decision 6.6).
    moved_to_guide_id   UUID REFERENCES livetv_guide(id) ON DELETE SET NULL,
    error               TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_livetv_recordings_status ON livetv_recordings (status);
CREATE INDEX idx_livetv_recordings_window ON livetv_recordings (scheduled_start, scheduled_end);
CREATE INDEX idx_livetv_recordings_episode_key ON livetv_recordings (episode_key) WHERE episode_key IS NOT NULL;
CREATE INDEX idx_livetv_recordings_rule ON livetv_recordings (series_rule_id) WHERE series_rule_id IS NOT NULL;
-- One recording per airing. The expander re-runs on every guide refresh
-- (48 h ceiling), so it must be able to upsert rather than duplicate.
CREATE UNIQUE INDEX idx_livetv_recordings_guide_unique ON livetv_recordings (guide_id)
    WHERE guide_id IS NOT NULL AND status NOT IN ('cancelled', 'moved');

-- ============================================================================
-- Tuner leases. 4 hard slots; a lease is held by either a viewer or a
-- recording, and a recording outranks a viewer. Rows are reconciled against the
-- device's own status.json at boot — the device happily keeps streaming to a
-- dead client and nothing times out on its own.
-- ============================================================================
CREATE TABLE livetv_tuner_leases (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tuner_id       UUID NOT NULL REFERENCES livetv_tuners(id) ON DELETE CASCADE,
    channel_id     UUID NOT NULL REFERENCES livetv_channels(id) ON DELETE CASCADE,
    tuner_index    INTEGER,
    holder_kind    TEXT NOT NULL CHECK (holder_kind IN ('viewer', 'recording')),
    session_id     TEXT,
    recording_id   UUID REFERENCES livetv_recordings(id) ON DELETE CASCADE,
    user_id        UUID REFERENCES users(id) ON DELETE SET NULL,
    buffer_dir     TEXT,
    acquired_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- A viewer lease is held briefly past a channel change so flicking back is
    -- instant; released immediately if another lease is waiting.
    expires_at     TIMESTAMPTZ,
    released_at    TIMESTAMPTZ,
    last_heartbeat TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_livetv_leases_active ON livetv_tuner_leases (tuner_id) WHERE released_at IS NULL;
CREATE UNIQUE INDEX idx_livetv_leases_session ON livetv_tuner_leases (session_id)
    WHERE session_id IS NOT NULL AND released_at IS NULL;

-- ============================================================================
-- Live sessions. One lease -> one buffer -> many viewers with independent
-- playheads. The HLS segment directory IS the pause buffer (Decision 4) — the
-- tuner has none, and neither the TS, PCR/PTS, DVB carousels nor the physical
-- layer carry any history to recover.
-- ============================================================================
CREATE TABLE livetv_sessions (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id    TEXT NOT NULL UNIQUE,
    channel_id    UUID NOT NULL REFERENCES livetv_channels(id) ON DELETE CASCADE,
    lease_id      UUID REFERENCES livetv_tuner_leases(id) ON DELETE SET NULL,
    user_id       UUID REFERENCES users(id) ON DELETE SET NULL,
    mode          TEXT NOT NULL CHECK (mode IN ('direct', 'transcode')),
    buffer_dir    TEXT NOT NULL,
    started_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ended_at      TIMESTAMPTZ,
    last_access   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Promoted to a recording; the buffer survives the channel change.
    recording_id  UUID REFERENCES livetv_recordings(id) ON DELETE SET NULL
);
CREATE INDEX idx_livetv_sessions_active ON livetv_sessions (channel_id) WHERE ended_at IS NULL;

-- ============================================================================
-- Watch history. Not analytics for its own sake — the goal is "this is on right
-- now and you always watch it". Raw events are kept; everything else derives.
-- ============================================================================
CREATE TABLE livetv_watch_events (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id        UUID REFERENCES users(id) ON DELETE CASCADE,
    channel_id     UUID NOT NULL REFERENCES livetv_channels(id) ON DELETE CASCADE,
    guide_id       UUID REFERENCES livetv_guide(id) ON DELETE SET NULL,
    series_id      TEXT,
    title          TEXT,
    episode_key    TEXT,
    started_at     TIMESTAMPTZ NOT NULL,
    ended_at       TIMESTAMPTZ,
    watched_secs   INTEGER NOT NULL DEFAULT 0,
    programme_secs INTEGER,
    source         TEXT NOT NULL CHECK (source IN ('live', 'buffer', 'recording')),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_livetv_watch_events_user ON livetv_watch_events (user_id, started_at DESC);
CREATE INDEX idx_livetv_watch_events_series ON livetv_watch_events (series_id) WHERE series_id IS NOT NULL;
CREATE INDEX idx_livetv_watch_events_channel ON livetv_watch_events (channel_id, started_at DESC);

-- One table, several grains, so series / time-of-day / channel / genre affinity
-- are one query each. mean_completion is what stops this being a naive counter:
-- a show you start and bail on five minutes in must rank below one you finish.
CREATE TABLE livetv_affinity (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    series_id       TEXT,
    channel_id      UUID REFERENCES livetv_channels(id) ON DELETE CASCADE,
    genre           TEXT,
    weekday         SMALLINT,
    hour            SMALLINT,
    watch_count     INTEGER NOT NULL DEFAULT 0,
    total_secs      BIGINT NOT NULL DEFAULT 0,
    mean_completion DOUBLE PRECISION NOT NULL DEFAULT 0,
    last_watched_at TIMESTAMPTZ,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
-- COALESCE-keyed uniqueness: nullable grain columns cannot participate in a
-- plain UNIQUE, so the rollup upserts against these expressions.
CREATE UNIQUE INDEX idx_livetv_affinity_grain ON livetv_affinity (
    user_id,
    COALESCE(series_id, ''),
    COALESCE(channel_id, '00000000-0000-0000-0000-000000000000'::uuid),
    COALESCE(genre, ''),
    COALESCE(weekday, -1),
    COALESCE(hour, -1)
);

-- ============================================================================
-- Ad detection (Decision 9). Marks only, never cuts.
--
-- The fingerprint bank is the structural advantage: a user correcting one break
-- boundary hands us a labelled ad segment, and its chromaprint then fixes every
-- future airing of that ad in every recording.
-- ============================================================================
CREATE TABLE livetv_ad_fingerprints (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Raw chromaprint, one u32 per ~0.1238 s.
    fingerprint   INTEGER[] NOT NULL,
    duration_secs DOUBLE PRECISION NOT NULL,
    label         TEXT,
    -- 'auto' from block scoring, 'confirmed' once a user has corrected or
    -- accepted a boundary. Only confirmed prints are trusted as seeds.
    origin        TEXT NOT NULL DEFAULT 'auto' CHECK (origin IN ('auto', 'confirmed')),
    seen_count    INTEGER NOT NULL DEFAULT 1,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_livetv_ad_fingerprints_origin ON livetv_ad_fingerprints (origin);

CREATE TABLE livetv_ad_breaks (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    recording_id  UUID NOT NULL REFERENCES livetv_recordings(id) ON DELETE CASCADE,
    start_seconds DOUBLE PRECISION NOT NULL,
    end_seconds   DOUBLE PRECISION NOT NULL,
    -- Which signals agreed. Two independent signals are required before a break
    -- is marked at all: a false positive is worth roughly 5-10 false negatives.
    method        TEXT NOT NULL CHECK (method IN ('fingerprint', 'black_silence', 'logo', 'combined')),
    confidence    DOUBLE PRECISION NOT NULL DEFAULT 0,
    ads_known     INTEGER NOT NULL DEFAULT 0,
    ads_total     INTEGER NOT NULL DEFAULT 0,
    corrected_by  UUID REFERENCES users(id) ON DELETE SET NULL,
    corrected_at  TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_livetv_ad_breaks_recording ON livetv_ad_breaks (recording_id, start_seconds);
