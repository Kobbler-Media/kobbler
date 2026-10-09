-- Add recursive scanning option to watch directories
ALTER TABLE watch_directories ADD COLUMN recursive BOOLEAN NOT NULL DEFAULT true;
