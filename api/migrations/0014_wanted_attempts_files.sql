-- Add files_json column to wanted_attempts for storing the list of files from Kobload
-- This supports multi-file downloads where user review may be needed

ALTER TABLE wanted_attempts ADD COLUMN IF NOT EXISTS files_json JSONB;
