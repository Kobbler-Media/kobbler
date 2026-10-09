-- Per-device auto-sync: controls whether new books automatically get DEVICE entitlements
-- Defaults to FALSE — user must explicitly opt each device in
ALTER TABLE devices ADD COLUMN auto_sync_new_books BOOLEAN NOT NULL DEFAULT FALSE;
