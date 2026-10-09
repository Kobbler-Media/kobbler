-- Add web reader flag to devices table.
-- Each user can have at most one web reader device (auto-created on first progress save).
ALTER TABLE devices ADD COLUMN is_web_reader BOOLEAN NOT NULL DEFAULT FALSE;
CREATE UNIQUE INDEX idx_devices_user_web_reader ON devices (user_id) WHERE is_web_reader = TRUE;
