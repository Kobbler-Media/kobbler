-- Remove legacy sync_providers table and device sync_config
-- Hardcover sync is now per-user (users.hardcover_api_key, users.hardcover_enabled)
-- and per-device (devices.hardcover_sync_enabled boolean)

-- Drop tables in dependency order
DROP TABLE IF EXISTS sync_rate_limit;
DROP TABLE IF EXISTS sync_providers;

-- Remove legacy sync_config JSONB column from devices
-- (hardcover_sync_enabled boolean remains for device-level toggle)
ALTER TABLE devices DROP COLUMN IF EXISTS sync_config;
