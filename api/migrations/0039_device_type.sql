-- Replace is_web_reader boolean with device_type text column.
-- Values: 'kobo', 'web', 'kindle', 'generic'

ALTER TABLE devices ADD COLUMN device_type TEXT NOT NULL DEFAULT 'kobo';

-- Migrate existing web reader devices
UPDATE devices SET device_type = 'web' WHERE is_web_reader = TRUE;

-- Replace the partial unique index (one web reader per user)
DROP INDEX idx_devices_user_web_reader;
CREATE UNIQUE INDEX idx_devices_user_web_reader ON devices (user_id) WHERE device_type = 'web';

-- Drop the old column
ALTER TABLE devices DROP COLUMN is_web_reader;
