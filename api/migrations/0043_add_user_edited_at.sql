-- Add user_edited_at timestamp to books table.
-- This tracks when a user explicitly edited a book via the UI.
-- Background jobs (series_enrich, author_sync) will skip metadata updates
-- for books where this is set, preserving user customizations.

ALTER TABLE books ADD COLUMN user_edited_at TIMESTAMPTZ;

-- No index needed - this is checked rarely (during batch job processing)
-- and we don't query "all user-edited books" directly.
