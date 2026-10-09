-- Add name field to ingest tokens for identification purposes
ALTER TABLE ingest_tokens ADD COLUMN name TEXT;
