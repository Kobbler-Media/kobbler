-- Clear cached empty results that block re-matching (transient API failures)
DELETE FROM metadata_cache WHERE response::text = '[]';

-- Reset unmatched/ambiguous files back to pending for re-processing
-- with improved interpretation and matching logic
UPDATE media_root_files
SET match_status = 'pending',
    match_candidates = NULL,
    interpretation = NULL,
    updated_at = NOW()
WHERE match_status IN ('unmatched', 'ambiguous');
