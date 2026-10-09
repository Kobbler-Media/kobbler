-- Track watch sync state for wanted items
ALTER TABLE wanted_items ADD COLUMN IF NOT EXISTS watch_synced_at TIMESTAMPTZ;
ALTER TABLE wanted_items ADD COLUMN IF NOT EXISTS watch_enabled BOOLEAN DEFAULT TRUE;

-- Index for finding items that need sync
CREATE INDEX IF NOT EXISTS wanted_items_watch_sync_idx ON wanted_items (watch_enabled, watch_synced_at);
