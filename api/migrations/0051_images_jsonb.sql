-- Migrate scattered image URL columns to unified images JSONB.

-- Authors: migrate image_url → images JSONB
ALTER TABLE authors ADD COLUMN IF NOT EXISTS images JSONB NOT NULL DEFAULT '{}'::jsonb;
UPDATE authors SET images = jsonb_build_object('profile', jsonb_build_object('source_url', image_url))
  WHERE image_url IS NOT NULL AND image_url != '';
ALTER TABLE authors DROP COLUMN IF EXISTS image_url;

-- Series: migrate image_url → images JSONB
ALTER TABLE series ADD COLUMN IF NOT EXISTS images JSONB NOT NULL DEFAULT '{}'::jsonb;
UPDATE series SET images = jsonb_build_object('cover', jsonb_build_object('source_url', image_url))
  WHERE image_url IS NOT NULL AND image_url != '';
ALTER TABLE series DROP COLUMN IF EXISTS image_url;

-- Books: migrate cover_url → images JSONB
ALTER TABLE books ADD COLUMN IF NOT EXISTS images JSONB NOT NULL DEFAULT '{}'::jsonb;
UPDATE books SET images = jsonb_build_object('cover', jsonb_build_object('source_url', cover_url))
  WHERE cover_url IS NOT NULL AND cover_url != '';
ALTER TABLE books DROP COLUMN IF EXISTS cover_url;
