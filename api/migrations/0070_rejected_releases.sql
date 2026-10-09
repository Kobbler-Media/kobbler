-- Add rejected_releases to entity tables for tracking replaced/rejected releases.
-- Each entry: {"release_title": "...", "info_hash": "...", "reason": "user_replaced"}
-- Cleared when a successful replacement is acquired.

ALTER TABLE movies ADD COLUMN rejected_releases JSONB NOT NULL DEFAULT '[]';
ALTER TABLE tv_episodes ADD COLUMN rejected_releases JSONB NOT NULL DEFAULT '[]';
ALTER TABLE books ADD COLUMN rejected_releases JSONB NOT NULL DEFAULT '[]';
