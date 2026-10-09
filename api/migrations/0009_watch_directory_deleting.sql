-- Add deleting flag to track async deletion in progress
ALTER TABLE watch_directories ADD COLUMN deleting BOOLEAN NOT NULL DEFAULT FALSE;
