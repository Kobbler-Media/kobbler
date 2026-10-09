-- Fix show-level CHECK constraint to match AutoWantMode enum values
ALTER TABLE tv_shows DROP CONSTRAINT tv_shows_auto_want_mode_check;
UPDATE tv_shows SET auto_want_mode = 'new_only' WHERE auto_want_mode = 'new';
ALTER TABLE tv_shows ADD CONSTRAINT tv_shows_auto_want_mode_check
    CHECK (auto_want_mode IN ('none', 'new_only', 'all', 'fill_gaps'));

-- Add per-season auto_want_mode with 'inherit' default
ALTER TABLE tv_seasons ADD COLUMN auto_want_mode TEXT NOT NULL DEFAULT 'inherit'
    CHECK (auto_want_mode IN ('inherit', 'none', 'new_only', 'all', 'fill_gaps'));
