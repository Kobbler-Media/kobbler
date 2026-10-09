-- Generalize sync provider config: replace hardcoded hardcover columns with
-- provider-agnostic tables (mirrors video_sync_providers pattern).

-- Per-user sync provider configuration
CREATE TABLE user_sync_providers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider_type TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    config JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(user_id, provider_type)
);

-- Per-device sync provider toggles
CREATE TABLE device_sync_providers (
    device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    provider_type TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    PRIMARY KEY(device_id, provider_type)
);

-- Generic provider read ID tracking (replaces hardcover_read_id)
ALTER TABLE user_book_data ADD COLUMN provider_read_ids JSONB NOT NULL DEFAULT '{}';

-- Migrate existing hardcover user settings
INSERT INTO user_sync_providers (user_id, provider_type, enabled, config)
SELECT id, 'hardcover', COALESCE(hardcover_enabled, true),
       CASE WHEN hardcover_api_key IS NOT NULL
            THEN jsonb_build_object('api_key', hardcover_api_key)
            ELSE '{}'::jsonb END
FROM users
WHERE hardcover_api_key IS NOT NULL OR hardcover_enabled = false;

-- Migrate device settings (only non-default values)
INSERT INTO device_sync_providers (device_id, provider_type, enabled)
SELECT id, 'hardcover', hardcover_sync_enabled
FROM devices
WHERE hardcover_sync_enabled = false;

-- Migrate read IDs
UPDATE user_book_data
SET provider_read_ids = jsonb_build_object('hardcover', hardcover_read_id)
WHERE hardcover_read_id IS NOT NULL;

-- Drop old columns
ALTER TABLE users DROP COLUMN hardcover_api_key;
ALTER TABLE users DROP COLUMN hardcover_enabled;
ALTER TABLE devices DROP COLUMN hardcover_sync_enabled;
ALTER TABLE user_book_data DROP COLUMN hardcover_read_id;
