-- Add provider_metadata column to books for storing raw provider data (Hardcover, etc.)
ALTER TABLE books ADD COLUMN IF NOT EXISTS provider_metadata JSONB NOT NULL DEFAULT '{}';
