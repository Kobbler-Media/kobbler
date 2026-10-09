-- Add dHash score, selection tracking, and rejection reasons to skip detection candidates.
-- Enables per-episode matching diagnostics and full pipeline audit trail.

ALTER TABLE skip_detect_candidates
ADD COLUMN dhash_score DOUBLE PRECISION,
ADD COLUMN selected BOOLEAN NOT NULL DEFAULT false,
ADD COLUMN rejection_reason TEXT;
