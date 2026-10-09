-- Sync providers for external reading trackers (Hardcover, etc.)

-- Device-level sync provider configuration
-- Uses JSONB to support multiple providers per device
-- Example: {"hardcover": {"enabled": true, "use_global": true, "api_key": null}}
ALTER TABLE devices ADD COLUMN IF NOT EXISTS sync_config JSONB NOT NULL DEFAULT '{}';

-- Global sync provider configuration
CREATE TABLE IF NOT EXISTS sync_providers (
    id TEXT PRIMARY KEY,                        -- e.g., "hardcover"
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    config JSONB NOT NULL DEFAULT '{}',         -- e.g., {"api_key": "..."}
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Seed Hardcover provider (disabled by default until user configures API key)
INSERT INTO sync_providers (id, enabled, config)
VALUES ('hardcover', FALSE, '{}')
ON CONFLICT (id) DO NOTHING;

-- Sync log for debugging and tracking
CREATE TABLE IF NOT EXISTS sync_log (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    book_id UUID NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    action TEXT NOT NULL,               -- "started", "finished", "progress", "reading_time", "rating"
    payload JSONB,
    synced_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    response JSONB,
    error TEXT
);

CREATE INDEX IF NOT EXISTS idx_sync_log_device_book ON sync_log(device_id, book_id);
CREATE INDEX IF NOT EXISTS idx_sync_log_synced_at ON sync_log(synced_at DESC);

-- Rate limiting per device per provider
CREATE TABLE IF NOT EXISTS sync_rate_limit (
    device_id UUID REFERENCES devices(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    last_request_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    requests_in_window INT NOT NULL DEFAULT 0,
    window_start TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (device_id, provider)
);
