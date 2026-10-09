-- Add 'review' to the match_status CHECK constraint.
-- Used for confident matches on auto_add=false roots that need user confirmation.

ALTER TABLE media_root_files
    DROP CONSTRAINT IF EXISTS media_root_files_match_status_check;

ALTER TABLE media_root_files
    ADD CONSTRAINT media_root_files_match_status_check
    CHECK (match_status IN ('pending', 'identified', 'matched', 'review', 'ambiguous', 'unmatched', 'ignored'));
