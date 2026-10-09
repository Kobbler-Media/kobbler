-- Track what reading time was last synced to external providers (for diff calculation)
ALTER TABLE reading_statistics ADD COLUMN last_synced_reading_minutes INTEGER NOT NULL DEFAULT 0;
