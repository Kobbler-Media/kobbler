-- Rename match_status 'matched' → 'added' (terminology cleanup: "added to library")
UPDATE media_root_files SET match_status = 'added' WHERE match_status = 'matched';

ALTER TABLE media_root_files DROP CONSTRAINT IF EXISTS media_root_files_match_status_check;
ALTER TABLE media_root_files ADD CONSTRAINT media_root_files_match_status_check
    CHECK (match_status IN ('pending', 'identified', 'added', 'review', 'ambiguous', 'unmatched', 'ignored'));
