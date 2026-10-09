-- Add cover_url to wanted_items for displaying cover images in UI
ALTER TABLE wanted_items ADD COLUMN IF NOT EXISTS cover_url TEXT;
