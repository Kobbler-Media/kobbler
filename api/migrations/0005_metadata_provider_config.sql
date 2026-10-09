-- Add flexible JSONB config column to metadata_providers
-- This allows providers to declare and store arbitrary config fields

ALTER TABLE metadata_providers ADD COLUMN IF NOT EXISTS
    config JSONB NOT NULL DEFAULT '{}'::jsonb;

-- Migrate existing api_key values into the config JSONB column
UPDATE metadata_providers
SET config = jsonb_build_object('api_key', api_key)
WHERE api_key IS NOT NULL AND api_key != '';

-- Note: We keep the api_key column for now to avoid breaking existing code
-- It can be removed in a future migration after the code is updated
