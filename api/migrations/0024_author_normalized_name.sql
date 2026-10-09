-- Add normalized_name column to authors for case-insensitive deduplication
-- Similar to how series uses sort_key and genres uses normalized_name

-- Add the column
ALTER TABLE authors ADD COLUMN IF NOT EXISTS normalized_name TEXT;

-- Populate for existing rows
UPDATE authors SET normalized_name = LOWER(name) WHERE normalized_name IS NULL;

-- Make it non-null and unique
ALTER TABLE authors ALTER COLUMN normalized_name SET NOT NULL;

-- Drop the old unique constraint on name and add one on normalized_name
ALTER TABLE authors DROP CONSTRAINT IF EXISTS authors_name_key;
CREATE UNIQUE INDEX IF NOT EXISTS idx_authors_normalized_name ON authors(normalized_name);
