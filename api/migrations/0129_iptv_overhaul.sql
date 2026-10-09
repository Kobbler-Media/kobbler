-- IPTV overhaul: EPG performance, channel grouping, catchup, favourite groups

-- Batch EPG index for multi-channel time-range queries (replaces N+1 per-channel queries)
CREATE INDEX IF NOT EXISTS idx_iptv_epg_batch ON iptv_epg(start_time, stop_time, channel_id);

-- EPG retention config (7 days default for catchup support)
ALTER TABLE iptv_config ADD COLUMN IF NOT EXISTS epg_retention_days INTEGER NOT NULL DEFAULT 7;

-- Favourite groups: allow users to favourite entire channel groups
CREATE TABLE IF NOT EXISTS iptv_favourite_groups (
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    group_id   UUID NOT NULL REFERENCES iptv_channel_groups(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, group_id)
);

-- New auto-groups for provider-specific content types
INSERT INTO iptv_channel_groups (name, slug, icon, display_order, auto_assign_categories) VALUES
    ('24/7',  '24-7',  'repeat',      8,  '{24/7}'),
    ('VOD',   'vod',   'play-circle', 9,  '{vod}'),
    ('PPV',   'ppv',   'ticket',      10, '{ppv}')
ON CONFLICT DO NOTHING;
